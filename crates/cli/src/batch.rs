//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time (PLAN.md Phase 4).
//! Every win is verified by the reference engine before it counts; every deal that isn't won gets a dossier.
//! The summary gives the win rate with a Wilson 95% interval, counting not-won deals both ways.

use crate::dossier;
use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_core::notation::encode_compact;
use accordion_core::verify::verify;
use accordion_core::{Outcome, SolveRecord};
use accordion_solver::{Config, Gates, Report, Search, Verdict, solve};
use clap::Args;
use rayon::prelude::*;
use serde_json::json;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Args)]
pub struct BatchArgs {
    /// First seed.
    #[arg(long, default_value_t = 0)]
    from: u64,
    /// Number of consecutive seeds.
    #[arg(long, default_value_t = 1000)]
    count: u64,
    /// Positions one deal may search (on one thread) before it counts as not won.
    #[arg(long, default_value_t = 50_000_000)]
    max_nodes: u64,
    /// Write one verified solve record per deal here (JSON Lines, compact moves).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where deals that aren't won get their dossiers.
    #[arg(long, default_value = "hard-deals")]
    dossiers: PathBuf,
    /// Don't write dossiers.
    #[arg(long)]
    no_dossiers: bool,
    /// Dead-position table per thread, as log2 of 64-byte buckets (14 = 1 MB; small tables that stay in the core's
    /// own cache were fastest, see docs/BENCHMARKS.md).
    #[arg(long, default_value_t = 14)]
    table: u32,
}

thread_local! {
    static SEARCH: std::cell::RefCell<Option<Search>> = const { std::cell::RefCell::new(None) };
}

/// Wilson score interval (95%) for `k` successes in `n` trials.
pub fn wilson(k: u64, n: u64) -> (f64, f64) {
    let (k, n, z) = (k as f64, n as f64, 1.959_964);
    let p = k / n;
    let d = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / d;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / d;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

pub fn run(args: &BatchArgs) -> Result<bool, String> {
    let threads = rayon::current_num_threads();
    let config = Config { max_nodes: args.max_nodes, ..Config::default() };
    let mut out = match &args.out {
        Some(p) => Some(BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("creating {}: {e}", p.display()))?,
        )),
        None => None,
    };
    eprintln!(
        "seeds {}..{} ({} deals), up to {} positions per deal, {threads} threads",
        args.from,
        args.from + args.count - 1,
        args.count,
        args.max_nodes
    );
    let start = Instant::now();
    let (mut won, mut exhausted, mut unknown, mut failed) = (0u64, 0u64, 0u64, 0u64);
    let mut nodes = Vec::with_capacity(args.count as usize);
    let mut micros = Vec::with_capacity(args.count as usize);
    let mut total_nodes = 0u64;
    let chunk = 4096u64;
    let mut done = 0u64;
    while done < args.count {
        let seeds = args.from + done..args.from + (done + chunk).min(args.count);
        let results: Vec<(u64, Report, u64)> = seeds
            .into_par_iter()
            .map(|seed| {
                // One long-lived search (and table) per worker thread: rayon's map_init would build a fresh
                // 16 MB table for every split of the work.
                SEARCH.with_borrow_mut(|search| {
                    let search = search.get_or_insert_with(|| Search::new(args.table, Gates::default()));
                    search.clear_table();
                    let t = Instant::now();
                    let cfg = Config { seed, ..config };
                    let report = solve(&Position::from_deal(&Deal::from_seed(seed)), &cfg, search, None);
                    (seed, report, t.elapsed().as_micros() as u64)
                })
            })
            .collect();
        for (seed, report, us) in results {
            let deal = Deal::from_seed(seed);
            nodes.push(report.nodes);
            micros.push(us);
            total_nodes += report.nodes;
            let (outcome, line) = match &report.verdict {
                Verdict::Win(line) => (Outcome::Win, line.clone()),
                _ => (Outcome::Unknown, report.best_line.clone()),
            };
            let mut record = SolveRecord::new(&deal, Some(seed), outcome, &line)
                .map_err(|e| format!("seed {seed}: {e}"))?;
            record.moves_b64 = Some(encode_compact(&line));
            record.moves.clear();
            record.stats =
                json!({ "nodes": report.nodes, "attempts": report.attempts, "us": us }).as_object().cloned();
            // Nothing counts as a win until the reference engine has replayed it.
            if let Err(e) = verify(&record) {
                failed += 1;
                eprintln!("\nseed {seed}: VERIFICATION FAILED: {e}");
                continue;
            }
            match report.verdict {
                Verdict::Win(_) => won += 1,
                Verdict::Exhausted => exhausted += 1,
                Verdict::Unknown => unknown += 1,
            }
            if !matches!(report.verdict, Verdict::Win(_)) && !args.no_dossiers {
                let attempt = dossier::Attempt {
                    stage: "batch",
                    config: &Config { seed, ..config },
                    threads: 1,
                    report: &report,
                    seconds: us as f64 / 1e6,
                };
                dossier::record(&args.dossiers, &deal, Some(seed), &attempt)?;
            }
            if let Some(w) = out.as_mut() {
                writeln!(w, "{}", record.to_json_line()).map_err(|e| e.to_string())?;
            }
        }
        done = (done + chunk).min(args.count);
        let elapsed = start.elapsed().as_secs_f64();
        eprint!("\r  {done} / {} deals · {won} won · {:.0} deals/s   ", args.count, done as f64 / elapsed);
    }
    eprintln!();
    if let Some(mut w) = out {
        w.flush().map_err(|e| e.to_string())?;
    }
    if exhausted + unknown > 0 && !args.no_dossiers {
        dossier::write_index(&args.dossiers)?;
    }

    let elapsed = start.elapsed().as_secs_f64();
    let n = args.count;
    let (lo, _) = wilson(won, n);
    let (_, hi) = wilson(won + exhausted + unknown, n);
    nodes.sort_unstable();
    micros.sort_unstable();
    println!("deals            {n} (seeds {}..{})", args.from, args.from + n - 1);
    println!("won              {won} (every one verified by the reference engine)");
    println!(
        "not won          {} exhausted, {unknown} hit the limit of {} positions",
        exhausted, args.max_nodes
    );
    println!("failed to verify {failed}");
    println!(
        "win rate         {:.4}%   95% CI [{:.4}%, {:.4}%] (Wilson; not-won deals counted as losses for the lower bound, wins for the upper)",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    println!(
        "positions/deal   median {}  p90 {}  p99 {}  p99.9 {}  max {}",
        percentile(&nodes, 0.5),
        percentile(&nodes, 0.9),
        percentile(&nodes, 0.99),
        percentile(&nodes, 0.999),
        nodes[nodes.len() - 1]
    );
    println!(
        "ms/deal (1 core) median {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.1}",
        percentile(&micros, 0.5) as f64 / 1e3,
        percentile(&micros, 0.9) as f64 / 1e3,
        percentile(&micros, 0.99) as f64 / 1e3,
        percentile(&micros, 0.999) as f64 / 1e3,
        micros[micros.len() - 1] as f64 / 1e3
    );
    println!(
        "throughput       {:.0} deals/s, {:.0}M positions/s on {threads} threads ({elapsed:.1} s wall)",
        n as f64 / elapsed,
        total_nodes as f64 / elapsed / 1e6
    );
    Ok(failed == 0)
}

#[cfg(test)]
mod tests {
    use super::wilson;

    #[test]
    fn wilson_interval_matches_known_values() {
        let (lo, hi) = wilson(0, 10);
        assert!(lo == 0.0 && (hi - 0.2775).abs() < 1e-3, "{hi}");
        let (lo, hi) = wilson(100_000, 100_000);
        assert!((lo - 0.999_962).abs() < 1e-5 && (hi - 1.0).abs() < 1e-12, "{lo} {hi}");
    }
}
