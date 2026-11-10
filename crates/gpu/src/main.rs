//! `accgpu`: measures the GPU on its own (PLAN.md Phase 5 track F). Runs the GPU rungs over a range of seeded
//! deals, then the CPU verifies every GPU win with the reference engine and solves every deal the GPU did not win
//! (the thorough fallback, so every deal still gets the full search); reports each part. `acc batch --gpu` runs
//! the two side by side instead.
//!
//! NVRTC must be where the system finds libraries, in `ACC_NVRTC_DIR`, or in `.tools/` (tools/get-nvrtc.ps1 or .sh).

use accordion_core::fast::Position;
use accordion_core::game::Game;
use accordion_core::verify::verify;
use accordion_core::{Deal, Move, Outcome, SolveRecord};
use accordion_gpu::{BatchResult, Gpu, PROFILE_SLOTS, Settings, builtin_source, deal_codes};
use accordion_solver::{Config, Endgame, Search, Verdict, solve};
use clap::Parser;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "accgpu", about = "GPU ladder attempts on their own, with CPU verification and fallback")]
struct Args {
    /// First seed.
    #[arg(long, default_value_t = 200_000)]
    from: u64,
    /// Number of consecutive seeds.
    #[arg(long, default_value_t = 10_000)]
    count: u64,
    /// GPU settings as JSON (accordion_gpu::Settings), e.g. '{"rungs": [[38, 256], [36, 1024]], "budget": 2000}'.
    #[arg(long)]
    gpu_config: Option<String>,
    /// CPU solver, as JSON (accordion_solver::Config).
    #[arg(long, default_value = r#"{"search": {"endgame_piles": 11}}"#)]
    cpu_config: String,
    /// CPU threads verifying lines and solving misses, after the GPU has finished (0 = every core).
    #[arg(long, visible_alias = "verify-threads", default_value_t = 0)]
    cpu_threads: usize,
    /// Stop after the GPU: no line is checked and no miss is solved, and the output says so. For speed runs only.
    #[arg(long)]
    no_verify: bool,
    /// Verify each line by writing and checking a full solve record, as this tool used to: the same moves checked by
    /// the same reference engine, with the format of the record too, and slower. By default the engine replays
    /// each line directly.
    #[arg(long)]
    verify_records: bool,
    /// A define for the kernel's compile, NAME=VALUE; may be repeated, so a variant can be a flag. Names the source
    /// never mentions, or sets without #ifndef, or the host sets from --gpu-config, are refused.
    #[arg(long = "define", value_name = "NAME=VALUE")]
    defines: Vec<String>,
    /// The result as one line of JSON on stdout; the report goes to stderr. With --profile, the raw counters are in it.
    #[arg(long)]
    json: bool,
    /// Where the endgame tables are. If it isn't there and is relative, looks above the current directory and the
    /// executable too.
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Write the seeds of the deals the GPU did not win here, one per line.
    #[arg(long)]
    misses: Option<PathBuf>,
    /// Compile the kernel with its profile counters and report where its time goes (slower; for measuring).
    #[arg(long)]
    profile: bool,
    /// Write the kernels' PTX here and stop (to read what the code compiles to).
    #[arg(long)]
    ptx: Option<PathBuf>,
    /// Compile this kernel source instead of the built-in kernel.cu (to try a change without rebuilding).
    #[arg(long)]
    kernel: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    // On Linux, this may start accgpu again with a copy of NVRTC on the loader's path; before anything else is done.
    accordion_gpu::restart_with_bundled_nvrtc();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("accgpu: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<bool, String> {
    let mut settings: Settings = match &args.gpu_config {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("--gpu-config: {e}"))?,
        None => Settings::default(),
    };
    settings.profile |= args.profile;
    let source = match &args.kernel {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?),
        None => None,
    };
    if let Some(path) = &args.ptx {
        let ptx = Gpu::ptx_with_defines(&settings, source.as_deref(), &args.defines)?;
        std::fs::write(path, ptx).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(true);
    }
    let t = Instant::now();
    let gpu = Gpu::with_defines(settings, source.as_deref().unwrap_or(builtin_source()), &args.defines)?;
    let compile_time = t.elapsed().as_secs_f64();
    eprintln!("{} ({} multiprocessors); kernel compiled in {compile_time:.2} s", gpu.name, gpu.sms);
    let (regs, local, blocks) = gpu.occupancy()?;
    eprintln!(
        "attempt kernel: {regs} registers and {local} bytes of local memory a thread; {blocks} blocks of {} a multiprocessor",
        gpu.settings.block
    );
    let cpu: Config = serde_json::from_str(&args.cpu_config).map_err(|e| format!("--cpu-config: {e}"))?;
    let endgame = match cpu.search.endgame_piles {
        0 => None,
        _ if args.no_verify => None,
        k => Some(Arc::new(Endgame::load(&find_endgame(&args.endgame_dir), k)?)),
    };
    let cpu_threads = match args.cpu_threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();
    let n = seeds.len();

    let mut results: Vec<BatchResult> = Vec::new();
    let (mut nodes, mut attempts) = (0u64, 0u64);
    let mut profile = [0u64; PROFILE_SLOTS];
    let mut first = 0;
    let unix_now = || SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let gpu_start_unix = unix_now();
    let start = Instant::now();
    gpu.feed(
        |&i: &usize| deal_codes(&Deal::from_seed(seeds[i])),
        || {
            let idx: Vec<usize> = (first..(first + gpu.settings.batch).min(n)).collect();
            first += idx.len();
            (!idx.is_empty()).then_some(idx)
        },
        |_, result| {
            nodes += result.total_nodes();
            for (sum, c) in profile.iter_mut().zip(&result.profile) {
                *sum += c;
            }
            attempts += result.total_tries();
            results.push(result);
        },
    )
    .map_err(|(e, _)| e)?;
    let gpu_time = start.elapsed().as_secs_f64();
    let gpu_end_unix = unix_now();
    let gpu_missed: Vec<u64> = seeds
        .iter()
        .enumerate()
        .filter(|&(i, _)| !results[i / gpu.settings.batch].is_won(i % gpu.settings.batch))
        .map(|(_, &seed)| seed)
        .collect();
    let misses = gpu_missed.len();
    // The report goes to stdout, or to stderr when the JSON line has stdout to itself.
    macro_rules! say {
        ($($arg:tt)*) => { if args.json { eprintln!($($arg)*) } else { println!($($arg)*) } };
    }
    let summary = |verify: serde_json::Value| {
        serde_json::json!({
            "kernel": args.kernel.as_ref().map(|p| p.display().to_string()),
            "defines": args.defines,
            "from": args.from,
            "count": n,
            "gpu_seconds": gpu_time,
            "gpu_start_unix": gpu_start_unix,
            "gpu_end_unix": gpu_end_unix,
            "compile_seconds": compile_time,
            "deals_per_s": n as f64 / gpu_time,
            "attempts": attempts,
            "attempts_per_deal": attempts as f64 / n as f64,
            "positions": nodes,
            "positions_per_deal": nodes as f64 / n as f64,
            "gpu_claimed_won": n - misses,
            "misses": misses,
            "misses_per_100k": misses as f64 * 1e5 / n as f64,
            "registers": regs,
            "local_bytes": local,
            "blocks_per_sm": blocks,
            "settings": gpu.settings,
            "profile": gpu.settings.profile.then(|| profile.to_vec()),
            "verify": verify,
        })
    };
    if args.no_verify {
        say!("deals              {n} (seeds {}..{})", args.from, args.from + args.count - 1);
        say!(
            "GPU                {} claimed won ({:.2}%), {misses} misses; LINES NOT VERIFIED (--no-verify); {:?}",
            n - misses,
            100.0 * (n - misses) as f64 / n as f64,
            gpu.settings
        );
        say!(
            "GPU time           {gpu_time:.2} s ({:.0} deals/s, {:.2}M attempts/s, {:.0}M positions/s)",
            n as f64 / gpu_time,
            attempts as f64 / gpu_time / 1e6,
            nodes as f64 / gpu_time / 1e6
        );
        say!("CPU                skipped (--no-verify): no line checked, no miss solved");
        say!("all won            NOT CHECKED (--no-verify)");
        if let Some(path) = &args.misses {
            let text: String = gpu_missed.iter().map(|s| format!("{s}\n")).collect();
            std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if gpu.settings.profile && !args.json {
            print_profile(&profile, gpu_time, gpu.threads(), attempts, nodes);
        }
        if args.json {
            println!(
                "{}",
                summary(serde_json::json!({"verified": false, "note": "--no-verify: lines not checked, misses not solved"}))
            );
        }
        return Ok(true);
    }

    let start = Instant::now();
    let next = AtomicUsize::new(0);
    let (won, failed, cpu_won, left) =
        (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    // Thread time spent verifying GPU lines and solving the GPU's misses, in microseconds.
    let (verify_us, left_us) = (AtomicU64::new(0), AtomicU64::new(0));
    let missed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..cpu_threads {
            scope.spawn(|| {
                let mut search = Search::with_options(14, cpu.search);
                search.set_endgame(endgame.clone());
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let (seed, deal) = (seeds[i], Deal::from_seed(seeds[i]));
                    let verifies = |line: &[Move]| {
                        if args.verify_records {
                            return SolveRecord::new(&deal, Some(seed), Outcome::Win, line)
                                .map_err(|e| e.to_string())
                                .and_then(|rec| verify(&rec).map(|_| ()).map_err(|e| e.to_string()));
                        }
                        // The reference engine plays every move, each checked, and must end on one pile.
                        let mut game = Game::new(&deal);
                        for (i, &mv) in line.iter().enumerate() {
                            game.apply(mv).map_err(|e| format!("move {}: {e}", i + 1))?;
                        }
                        match game.pile_count() {
                            1 => Ok(()),
                            piles => Err(format!("the line ends with {piles} piles, not one")),
                        }
                    };
                    let (result, at) = (&results[i / gpu.settings.batch], i % gpu.settings.batch);
                    if result.is_won(at) {
                        let t = Instant::now();
                        let line = result.line(at).expect("it is won");
                        let moves: Vec<Move> = line.iter().map(|&b| Move::from_byte(b)).collect();
                        let verified = verifies(&moves);
                        verify_us.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                        match verified {
                            Ok(()) => {
                                won.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            Err(e) => {
                                eprintln!("seed {seed}: GPU line FAILED verification: {e}");
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    left.fetch_add(1, Ordering::Relaxed);
                    missed.lock().unwrap().push(seed);
                    let t = Instant::now();
                    search.clear_table();
                    let report =
                        solve(&Position::from_deal(&deal), &Config { seed, ..cpu }, &mut search, None);
                    if matches!(&report.verdict, Verdict::Win(line) if verifies(line).is_ok()) {
                        cpu_won.fetch_add(1, Ordering::Relaxed);
                    }
                    left_us.fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
            });
        }
    });
    let cpu_time = start.elapsed().as_secs_f64();
    let (won, failed, cpu_won, left) =
        (won.into_inner(), failed.into_inner(), cpu_won.into_inner(), left.into_inner());
    say!("deals              {n} (seeds {}..{})", args.from, args.from + args.count - 1);
    say!(
        "GPU                {won} won and verified ({:.2}%), {failed} failed verification; {:?}",
        100.0 * won as f64 / n as f64,
        gpu.settings
    );
    say!(
        "GPU time           {gpu_time:.2} s ({:.0} deals/s, {:.2}M attempts/s, {:.0}M positions/s)",
        n as f64 / gpu_time,
        attempts as f64 / gpu_time / 1e6,
        nodes as f64 / gpu_time / 1e6
    );
    say!(
        "CPU                verified the GPU wins and solved {left} deals ({cpu_won} won) in {cpu_time:.2} s on {cpu_threads} threads"
    );
    let (verify_s, left_s) = (verify_us.into_inner() as f64 / 1e6, left_us.into_inner() as f64 / 1e6);
    say!(
        "CPU thread time    verifying {verify_s:.2} s ({:.1} us a line), the GPU's misses {left_s:.2} s ({:.1} ms each)",
        verify_s / won.max(1) as f64 * 1e6,
        left_s / left.max(1) as f64 * 1e3
    );
    if let Some(path) = &args.misses {
        let mut seeds = missed.into_inner().unwrap();
        seeds.sort_unstable();
        let text: String = seeds.iter().map(|s| format!("{s}\n")).collect();
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if gpu.settings.profile && !args.json {
        print_profile(&profile, gpu_time, gpu.threads(), attempts, nodes);
    }
    say!("all won            {}", if won + cpu_won == n { "yes" } else { "NO" });
    if args.json {
        println!(
            "{}",
            summary(serde_json::json!({
                "verified": true,
                "method": if args.verify_records { "reference engine, full solve records" } else { "reference engine replay" },
                "threads": cpu_threads,
                "seconds": cpu_time,
                "gpu_lines_verified": won,
                "failed_lines": failed,
                "cpu_solved": left,
                "cpu_won": cpu_won,
                "all_won": won + cpu_won == n,
                "us_a_line": verify_s / won.max(1) as f64 * 1e6,
                "ms_a_miss": left_s / left.max(1) as f64 * 1e3,
            }))
        );
    }
    Ok(failed == 0 && won + cpu_won == n)
}

/// `given` if it exists; if not and it is relative, the same path above the current directory or the executable.
fn find_endgame(given: &Path) -> PathBuf {
    if given.exists() || given.is_absolute() {
        return given.to_path_buf();
    }
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    let found = starts.iter().flatten().flat_map(|p| p.ancestors()).map(|d| d.join(given)).find(|p| p.exists());
    match found {
        Some(path) => {
            eprintln!("endgame tables: {}", path.display());
            path
        }
        None => given.to_path_buf(),
    }
}

/// Where the kernel's thread time went (kernel.cu's profile counters, summed over launches).
fn print_profile(
    p: &[u64; PROFILE_SLOTS],
    gpu_time: f64,
    threads: u64,
    attempts: u64,
    nodes: u64,
) {
    let names = [
        "prefix: taking work",
        "prefix: loading",
        "prefix: moves",
        "prefix: queueing",
        "search: searching",
        "search: taking, results",
        "search: waiting",
    ];
    let cycles: u64 = p[..names.len()].iter().sum();
    println!(
        "kernel profile     thread time by phase (a thread waiting on others in its warp counts where it is):"
    );
    for (name, &c) in names.iter().zip(p) {
        println!(
            "                     {name:<23} {:5.1}%  {:7.0} cycles an attempt run",
            100.0 * c as f64 / cycles as f64,
            c as f64 / attempts.max(1) as f64
        );
    }
    let in_step = |warp_steps: u64, steps: u64| steps as f64 / warp_steps.max(1) as f64;
    println!(
        "                   threads of a warp stepping together: {:.1} of 32 in the search loop, {:.1} in the prefix loop",
        in_step(p[7], p[8]),
        in_step(p[9], p[10])
    );
    println!(
        "                   {} attempts taken, {} skipped ({:.1}%) because their deal was already won",
        p[11],
        p[12],
        100.0 * p[12] as f64 / p[11].max(1) as f64
    );
    println!(
        "                   prefix threads waiting for room in the queue {:.1}% of their turns, search threads with no residual to take {:.1}%",
        100.0 * p[16] as f64 / (32 * p[9]).max(1) as f64,
        100.0 * p[17] as f64 / (32 * p[7]).max(1) as f64
    );
    println!(
        "                   {:.0} cycles of thread time a position (prefix moves and search positions)",
        cycles as f64 / nodes.max(1) as f64
    );
    println!(
        "                   threads busy {:.1}% of the GPU time ({threads} threads resident, {gpu_time:.2} s)",
        100.0 * p[13] as f64 / 1e9 / (threads as f64 * gpu_time)
    );
}
