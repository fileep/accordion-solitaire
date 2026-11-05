//! `accgpu`: measures the GPU on its own (PLAN.md Phase 5 track F). Runs the GPU rungs over a range of seeded
//! deals, then the CPU verifies every GPU win with the reference engine and solves every deal the GPU did not win
//! (the thorough fallback, so every deal still gets the full search); reports each part. `acc batch --gpu` runs
//! the two side by side instead.
//!
//! NVRTC must be on PATH, or in `.tools/` (PLAN.md Phase 0 notes).

use accordion_core::fast::Position;
use accordion_core::verify::verify;
use accordion_core::{Deal, Move, Outcome, SolveRecord};
use accordion_gpu::{Gpu, Settings, deal_codes};
use accordion_solver::{Config, Endgame, Search, Verdict, solve};
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    let settings: Settings = match &args.gpu_config {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("--gpu-config: {e}"))?,
        None => Settings::default(),
    };
    let t = Instant::now();
    let gpu = Gpu::new(settings)?;
    eprintln!(
        "{} ({} multiprocessors); kernel compiled in {:.2} s",
        gpu.name,
        gpu.sms,
        t.elapsed().as_secs_f64()
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
                        let moves: Vec<Move> = line.iter().map(|&b| Move::from_byte(b)).collect();
                        match verifies(&moves) {
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
                    search.clear_table();
                    let report =
                        solve(&Position::from_deal(&deal), &Config { seed, ..cpu }, &mut search, None);
                    if matches!(&report.verdict, Verdict::Win(line) if verifies(line).is_ok()) {
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
    println!("all won            {}", if won + cpu_won == n { "yes" } else { "NO" });
    Ok(failed == 0 && won + cpu_won == n)
}
