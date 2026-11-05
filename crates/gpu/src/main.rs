//! `accgpu`: measures the GPU on its own (PLAN.md Phase 5 track F). Runs the GPU rungs over a range of seeded
//! deals, then the CPU verifies every GPU win with the reference engine and solves every deal the GPU did not win
//! (the thorough fallback, so every deal still gets the full search); reports each part. `acc batch --gpu` runs
//! the two side by side instead.
//!
//! NVRTC must be on PATH, or in `.tools/` (PLAN.md Phase 0 notes).

use accordion_core::fast::Position;
use accordion_core::verify::verify;
use accordion_core::{Deal, Move, Outcome, SolveRecord};
use accordion_gpu::{Gpu, PROFILE_SLOTS, Settings, deal_codes};
use accordion_solver::{Config, Endgame, Search, Verdict, solve};
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

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
    /// CPU worker threads (0 = every core).
    #[arg(long, default_value_t = 0)]
    cpu_threads: usize,
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Write the seeds of the deals the GPU did not win here, one per line.
    #[arg(long)]
    misses: Option<PathBuf>,
    /// Compile the kernel with its profile counters and report where its time goes (slower; for measuring).
    #[arg(long)]
    profile: bool,
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
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
    let t = Instant::now();
    let gpu = Gpu::new(settings)?;
    eprintln!(
        "{} ({} multiprocessors); kernel compiled in {:.2} s",
        gpu.name,
        gpu.sms,
        t.elapsed().as_secs_f64()
    );
    let (regs, local, blocks) = gpu.occupancy()?;
    eprintln!(
        "attempt kernel: {regs} registers and {local} bytes of local memory a thread; {blocks} blocks of {} a multiprocessor",
        gpu.settings.block
    );
    let cpu: Config = serde_json::from_str(&args.cpu_config).map_err(|e| format!("--cpu-config: {e}"))?;
    let endgame = match cpu.search.endgame_piles {
        0 => None,
        k => Some(Arc::new(Endgame::load(&args.endgame_dir, k)?)),
    };
    let cpu_threads = match args.cpu_threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();
    let n = seeds.len();

    let mut lines: Vec<Option<Vec<u8>>> = vec![None; n];
    let (mut nodes, mut attempts) = (0u64, 0u64);
    let mut profile = [0u64; PROFILE_SLOTS];
    let mut first = 0;
    let start = Instant::now();
    gpu.feed(
        |&i: &usize| deal_codes(&Deal::from_seed(seeds[i])),
        || {
            let idx: Vec<usize> = (first..(first + gpu.settings.batch).min(n)).collect();
            first += idx.len();
            (!idx.is_empty()).then_some(idx)
        },
        |idx, result| {
            nodes += result.nodes.iter().sum::<u64>();
            for (sum, c) in profile.iter_mut().zip(&result.profile) {
                *sum += c;
            }
            attempts += result.tries.iter().map(|&t| u64::from(t)).sum::<u64>();
            for (i, line) in idx.into_iter().zip(result.lines) {
                lines[i] = line;
            }
        },
    )
    .map_err(|(e, _)| e)?;
    let gpu_time = start.elapsed().as_secs_f64();

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
                        SolveRecord::new(&deal, Some(seed), Outcome::Win, line)
                            .map_err(|e| e.to_string())
                            .and_then(|rec| verify(&rec).map(|_| ()).map_err(|e| e.to_string()))
                    };
                    if let Some(line) = &lines[i] {
                        let t = Instant::now();
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
    println!("deals              {n} (seeds {}..{})", args.from, args.from + args.count - 1);
    println!(
        "GPU                {won} won and verified ({:.2}%), {failed} failed verification; {:?}",
        100.0 * won as f64 / n as f64,
        gpu.settings
    );
    println!(
        "GPU time           {gpu_time:.2} s ({:.0} deals/s, {:.2}M attempts/s, {:.0}M positions/s)",
        n as f64 / gpu_time,
        attempts as f64 / gpu_time / 1e6,
        nodes as f64 / gpu_time / 1e6
    );
    println!(
        "CPU                verified the GPU wins and solved {left} deals ({cpu_won} won) in {cpu_time:.2} s on {cpu_threads} threads"
    );
    let (verify_s, left_s) = (verify_us.into_inner() as f64 / 1e6, left_us.into_inner() as f64 / 1e6);
    println!(
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
    if gpu.settings.profile {
        print_profile(&profile, gpu_time, gpu.threads(), attempts, nodes);
    }
    println!("all won            {}", if won + cpu_won == n { "yes" } else { "NO" });
    Ok(failed == 0 && won + cpu_won == n)
}

/// Where the kernel's thread time went (kernel.cu's profile counters, summed over launches).
fn print_profile(p: &[u64; PROFILE_SLOTS], gpu_time: f64, threads: u64, attempts: u64, nodes: u64) {
    let names = [
        "taking work",
        "loading the deal",
        "random prefix",
        "dead-end gate",
        "search",
        "writing results",
        "skipping won deals",
    ];
    let cycles: u64 = p[..names.len()].iter().sum();
    println!(
        "kernel profile     thread time by phase (a thread waiting on others in its warp counts where it is):"
    );
    for (name, &c) in names.iter().zip(p) {
        println!(
            "                     {name:<19} {:5.1}%  {:7.0} cycles an attempt run",
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
    let searches: u64 = p[13..19].iter().sum();
    let share = |i: usize| 100.0 * p[i] as f64 / searches.max(1) as f64;
    println!(
        "                   {searches} searches by positions: 0-15 {:.1}%, 16-63 {:.1}%, 64-255 {:.1}%, 256-1023 {:.1}%, 1024+ {:.1}%, the whole budget {:.1}%",
        share(13),
        share(14),
        share(15),
        share(16),
        share(17),
        share(18)
    );
    println!(
        "                   {:.0} cycles of thread time a position (prefix moves and search positions)",
        cycles as f64 / nodes.max(1) as f64
    );
    println!(
        "                   threads busy {:.1}% of the GPU time ({threads} threads resident, {gpu_time:.2} s)",
        100.0 * p[19] as f64 / 1e9 / (threads as f64 * gpu_time)
    );
}
