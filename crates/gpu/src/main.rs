//! `accgpu`: the Phase 5 GPU measurement (PLAN.md track F). Runs ladder attempts for a range of seeded deals on
//! the GPU, verifies every GPU win with the reference engine, and gives every deal the GPU did not win to the CPU
//! solver (the thorough fallback, so every deal still gets the full search).
//!
//! Modes:
//! - `measure`: the GPU rungs over all deals, then the CPU on what is left; reports each part.
//! - `hybrid`: GPU and CPU at once on one shared stream of deals; the CPU takes GPU leftovers first.
//! - `cpu`: the same runner without the GPU, for a like-for-like comparison.
//!
//! NVRTC must be on PATH (PLAN.md Phase 0 notes: `.tools/cuda-nvrtc-12.9.86/nvidia/cuda_nvrtc/bin`).

use accordion_core::fast::Position;
use accordion_core::verify::verify;
use accordion_core::{Deal, Move, Outcome, SolveRecord};
use accordion_gpu::{Attempts, BatchResult, Gpu, Pending};
use accordion_solver::{Config, Endgame, Search, Verdict, solve};
use clap::{Parser, ValueEnum};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Measure,
    Hybrid,
    Cpu,
}

#[derive(Parser)]
#[command(
    name = "accgpu",
    about = "GPU ladder attempts with CPU verification and fallback (PLAN.md Phase 5 track F)"
)]
struct Args {
    #[arg(long, value_enum, default_value = "measure")]
    mode: Mode,
    /// First seed.
    #[arg(long, default_value_t = 200_000)]
    from: u64,
    /// Number of consecutive seeds.
    #[arg(long, default_value_t = 10_000)]
    count: u64,
    /// GPU rungs as PREFIX:TRIES, run in order on the deals still not won (prefixes of at least 36 leave at most
    /// 16 piles, the kernel's limit).
    #[arg(long, default_value = "38:256,36:1024")]
    rungs: String,
    /// Positions each GPU search may generate.
    #[arg(long, default_value_t = 2000)]
    budget: u32,
    /// Deals per kernel launch (keep launches well under Windows' 2 s GPU watchdog).
    #[arg(long, default_value_t = 4096)]
    batch: usize,
    /// Thread blocks per launch (0 = 16 per multiprocessor).
    #[arg(long, default_value_t = 0)]
    blocks: u32,
    /// Threads per block (the kernel's shared memory is sized for it: 272 bytes per thread).
    #[arg(long, default_value_t = 32)]
    threads: u32,
    /// CPU solver, as JSON (accordion_solver::Config).
    #[arg(
        long,
        default_value = r#"{"luby_unit": 1000, "ladder": {"start": 36, "tries": 256, "budget": 4000}, "search": {"sleep_sets": true, "endgame_piles": 11}}"#
    )]
    cpu_config: String,
    /// CPU worker threads (0 = every core).
    #[arg(long, default_value_t = 0)]
    cpu_threads: usize,
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Hybrid/cpu modes: run unmeasured for this many seconds first, so the laptop reaches sustained clocks.
    #[arg(long, default_value_t = 0.0)]
    warmup: f64,
    /// Hybrid/cpu modes: measured runs.
    #[arg(long, default_value_t = 1)]
    repeat: u32,
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

struct Setup {
    seeds: Vec<u64>,
    deals: Vec<[u8; 52]>,
    attempts: Attempts,
    blocks: u32,
    cpu: Config,
    cpu_threads: usize,
    endgame: Option<Arc<Endgame>>,
}

fn run(args: &Args) -> Result<bool, String> {
    let gpu = if args.mode == Mode::Cpu {
        None
    } else {
        let t = Instant::now();
        let gpu = Gpu::new(args.threads)?;
        eprintln!(
            "{} ({} multiprocessors); kernel compiled in {:.2} s",
            gpu.name,
            gpu.sms,
            t.elapsed().as_secs_f64()
        );
        Some(gpu)
    };
    let cpu: Config = serde_json::from_str(&args.cpu_config).map_err(|e| format!("--cpu-config: {e}"))?;
    let endgame = match cpu.search.endgame_piles {
        0 => None,
        k => Some(Arc::new(Endgame::load(&args.endgame_dir, k)?)),
    };
    let rungs = args
        .rungs
        .split(',')
        .map(|r| {
            let (p, t) = r.split_once(':').ok_or(format!("rung {r:?}: expected PREFIX:TRIES"))?;
            Ok((
                p.trim().parse().map_err(|e| format!("{r}: {e}"))?,
                t.trim().parse().map_err(|e| format!("{r}: {e}"))?,
            ))
        })
        .collect::<Result<Vec<(u32, u32)>, String>>()?;
    if rungs.is_empty() || rungs.len() > 2 {
        return Err("--rungs: one or two rungs (both run in one launch)".into());
    }
    let (prefix2, tries2) = rungs.get(1).copied().unwrap_or((0, 0));
    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();
    let deals = seeds
        .iter()
        .map(|&s| {
            let mut codes = [0u8; 52];
            for (c, card) in codes.iter_mut().zip(Deal::from_seed(s).cards()) {
                *c = card.code();
            }
            codes
        })
        .collect();
    let setup = Setup {
        seeds,
        deals,
        attempts: Attempts {
            tries: rungs[0].1,
            prefix: rungs[0].0,
            tries2,
            prefix2,
            budget: args.budget,
            conn_gate: 19,
            seed: 0x5EED,
        },
        blocks: match (&gpu, args.blocks) {
            (Some(g), 0) => g.sms * 16,
            (_, b) => b.max(1),
        },
        cpu,
        cpu_threads: if args.cpu_threads == 0 {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        } else {
            args.cpu_threads
        },
        endgame,
    };
    match args.mode {
        Mode::Measure => measure(&setup, gpu.as_ref().expect("GPU mode"), args),
        Mode::Hybrid | Mode::Cpu => {
            if args.warmup > 0.0 {
                let start = Instant::now();
                while start.elapsed().as_secs_f64() < args.warmup {
                    stream(&setup, gpu.as_ref(), args.batch)?;
                }
                eprintln!("warmed up for {:.0} s", start.elapsed().as_secs_f64());
            }
            let mut ok = true;
            let mut rates = Vec::new();
            for rep in 1..=args.repeat {
                let r = stream(&setup, gpu.as_ref(), args.batch)?;
                let rate = setup.seeds.len() as f64 / r.wall;
                rates.push(rate);
                println!(
                    "run {rep}: {} deals in {:.2} s = {rate:.0} deals/s; won on GPU {}, on CPU {} ({} GPU leftovers), failed {}, not won {}",
                    setup.seeds.len(),
                    r.wall,
                    r.gpu_won,
                    r.cpu_won,
                    r.leftovers,
                    r.failed,
                    setup.seeds.len() - r.gpu_won - r.cpu_won
                );
                ok &= r.failed == 0 && r.gpu_won + r.cpu_won == setup.seeds.len();
            }
            rates.sort_by(f64::total_cmp);
            println!("median {:.0} deals/s over {} runs", rates[rates.len() / 2], rates.len());
            Ok(ok)
        }
    }
}

/// Checks a GPU line with the reference engine.
fn verify_gpu_line(s: &Setup, i: usize, bytes: &[u8]) -> bool {
    let moves: Vec<Move> = bytes.iter().map(|&b| Move::from_byte(b)).collect();
    let deal = Deal::from_seed(s.seeds[i]);
    let ok = SolveRecord::new(&deal, Some(s.seeds[i]), Outcome::Win, &moves)
        .map_err(|e| e.to_string())
        .and_then(|rec| verify(&rec).map(|_| ()).map_err(|e| e.to_string()));
    if let Err(e) = &ok {
        eprintln!("seed {}: GPU line FAILED verification: {e}", s.seeds[i]);
    }
    ok.is_ok()
}

/// Runs the GPU rungs on batches of deals (indices into the setup) from `next_batch`, keeping two launches in
/// flight on two streams so the GPU never waits for the host; hands every finished batch to `done`.
fn feed(
    s: &Setup,
    gpu: &Gpu,
    mut next_batch: impl FnMut() -> Option<Vec<usize>>,
    mut done: impl FnMut(Vec<usize>, BatchResult),
) -> Result<(), String> {
    let streams = [gpu.stream()?, gpu.stream()?];
    let mut in_flight: VecDeque<(Vec<usize>, Pending)> = VecDeque::new();
    let mut launch = 0u64;
    loop {
        if in_flight.len() < 2
            && let Some(idx) = next_batch()
        {
            let deals: Vec<[u8; 52]> = idx.iter().map(|&i| s.deals[i]).collect();
            let a = Attempts { seed: s.attempts.seed.wrapping_add(launch), ..s.attempts };
            let pending = gpu.submit(&streams[(launch % 2) as usize], &deals, &a, s.blocks)?;
            in_flight.push_back((idx, pending));
            launch += 1;
            continue;
        }
        let Some((idx, pending)) = in_flight.pop_front() else { return Ok(()) };
        done(idx, pending.collect()?);
    }
}

/// Solves one deal on the CPU with the full stage chain; true if won and verified.
fn cpu_solve(s: &Setup, search: &mut Search, i: usize) -> bool {
    let seed = s.seeds[i];
    let deal = Deal::from_seed(seed);
    search.clear_table();
    let report = solve(&Position::from_deal(&deal), &Config { seed, ..s.cpu }, search, None);
    matches!(&report.verdict, Verdict::Win(line)
        if SolveRecord::new(&deal, Some(seed), Outcome::Win, line).is_ok_and(|r| verify(&r).is_ok()))
}

fn new_search(s: &Setup) -> Search {
    let mut search = Search::with_options(14, s.cpu.search);
    search.set_endgame(s.endgame.clone());
    search
}

struct StreamResult {
    wall: f64,
    gpu_won: usize,
    cpu_won: usize,
    leftovers: usize,
    failed: usize,
}

/// One pass over every deal, GPU (if any) and CPU workers drawing from the same counter. The GPU feeder only
/// launches and collects; the CPU workers verify GPU lines first, then take GPU leftovers, then fresh deals.
fn stream(s: &Setup, gpu: Option<&Gpu>, batch: usize) -> Result<StreamResult, String> {
    let n = s.seeds.len();
    let next = AtomicUsize::new(0);
    // GPU results waiting for a CPU worker: (deal, winning line or None for a leftover).
    let from_gpu: Mutex<VecDeque<(usize, Option<Vec<u8>>)>> = Mutex::new(VecDeque::new());
    let gpu_done = AtomicBool::new(gpu.is_none());
    let (gpu_won, cpu_won, left_total, failed) =
        (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let gpu_error: Mutex<Option<String>> = Mutex::new(None);
    let start = Instant::now();
    std::thread::scope(|scope| {
        if let Some(g) = gpu {
            scope.spawn(|| {
                let result = feed(
                    s,
                    g,
                    || {
                        let first = next.fetch_add(batch, Ordering::Relaxed);
                        (first < n).then(|| (first..(first + batch).min(n)).collect())
                    },
                    |idx, result| {
                        let left = result.lines.iter().filter(|l| l.is_none()).count();
                        left_total.fetch_add(left, Ordering::Relaxed);
                        from_gpu.lock().expect("no panics").extend(idx.into_iter().zip(result.lines));
                    },
                );
                if let Err(e) = result {
                    *gpu_error.lock().expect("no panics") = Some(e);
                }
                gpu_done.store(true, Ordering::Release);
            });
        }
        for _ in 0..s.cpu_threads {
            scope.spawn(|| {
                let mut search = new_search(s);
                loop {
                    let job = from_gpu.lock().expect("no panics").pop_front();
                    let i = match job {
                        Some((i, Some(line))) => {
                            if verify_gpu_line(s, i, &line) {
                                gpu_won.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            failed.fetch_add(1, Ordering::Relaxed);
                            i // the CPU still tries to win it
                        }
                        Some((i, None)) => i,
                        None => {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i < n {
                                i
                            } else if gpu_done.load(Ordering::Acquire)
                                && from_gpu.lock().expect("no panics").is_empty()
                            {
                                break;
                            } else {
                                std::thread::sleep(Duration::from_micros(200));
                                continue;
                            }
                        }
                    };
                    if cpu_solve(s, &mut search, i) {
                        cpu_won.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    if let Some(e) = gpu_error.into_inner().expect("no panics") {
        return Err(e);
    }
    Ok(StreamResult {
        wall: start.elapsed().as_secs_f64(),
        gpu_won: gpu_won.into_inner(),
        cpu_won: cpu_won.into_inner(),
        leftovers: left_total.into_inner(),
        failed: failed.into_inner(),
    })
}

/// The GPU rungs over every deal, then the CPU verifies its wins and solves the rest; reports both.
fn measure(s: &Setup, gpu: &Gpu, args: &Args) -> Result<bool, String> {
    let n = s.seeds.len();
    let mut lines: Vec<Option<Vec<u8>>> = vec![None; n];
    let (mut nodes, mut attempts) = (0u64, 0u64);
    let mut first = 0;
    let start = Instant::now();
    feed(
        s,
        gpu,
        || {
            let idx: Vec<usize> = (first..(first + args.batch).min(n)).collect();
            first += idx.len();
            (!idx.is_empty()).then_some(idx)
        },
        |idx, result| {
            nodes += result.nodes;
            attempts += u64::from(result.attempts_run);
            for (i, line) in idx.into_iter().zip(result.lines) {
                lines[i] = line;
            }
        },
    )?;
    let gpu_time = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let next = AtomicUsize::new(0);
    let (won, failed, cpu_won, left) =
        (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for _ in 0..s.cpu_threads {
            scope.spawn(|| {
                let mut search = new_search(s);
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    if let Some(line) = &lines[i] {
                        if verify_gpu_line(s, i, line) {
                            won.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                    left.fetch_add(1, Ordering::Relaxed);
                    if cpu_solve(s, &mut search, i) {
                        cpu_won.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let cpu_time = start.elapsed().as_secs_f64();
    let (won, failed, cpu_won, left) =
        (won.into_inner(), failed.into_inner(), cpu_won.into_inner(), left.into_inner());
    println!("deals              {n} (seeds {}..{})", args.from, args.from + args.count - 1);
    println!(
        "GPU                {won} won and verified ({:.2}%), {failed} failed verification; rungs {}, budget {}",
        100.0 * won as f64 / n as f64,
        args.rungs,
        args.budget
    );
    println!(
        "GPU time           {gpu_time:.2} s ({:.0} deals/s, {:.2}M attempts/s, {:.0}M positions/s)",
        n as f64 / gpu_time,
        attempts as f64 / gpu_time / 1e6,
        nodes as f64 / gpu_time / 1e6
    );
    println!(
        "CPU                verified the GPU wins and solved {left} deals ({cpu_won} won) in {cpu_time:.2} s on {} threads",
        s.cpu_threads
    );
    println!("all won            {}", if won + cpu_won == n { "yes" } else { "NO" });
    Ok(failed == 0 && won + cpu_won == n)
}
