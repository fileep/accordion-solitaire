//! Runs one solver configuration over many seeded deals on worker threads (PLAN.md Phase 5 track B). Workers take
//! the next deal from a shared counter, so a slow deal never holds the others up; each worker solves, verifies
//! and serializes its own deals and keeps one search (and table) for the whole run. The calling thread only
//! receives finished deals, in completion order.

use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_core::notation::encode_compact;
use accordion_core::verify::verify;
use accordion_core::{Outcome, SolveRecord};
use accordion_solver::{Config, Endgame, Report, Search, Verdict, solve};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub struct Options {
    pub threads: usize,
    /// Dead-position table per thread, as log2 of 64-byte buckets.
    pub table_log2: u32,
    pub endgame: Option<Arc<Endgame>>,
    /// Serialize each deal's solve record (compact moves) for output.
    pub records: bool,
}

/// One finished deal.
pub struct Solved {
    /// Position in the seed list.
    pub index: usize,
    pub seed: u64,
    pub report: Report,
    /// Time to solve on one thread, in microseconds.
    pub us: u64,
    /// The reference engine's verdict on the line (the win, or the best line of a deal not won).
    pub verified: Result<(), String>,
    /// The solve record as one JSON line, when asked for.
    pub record: Option<String>,
}

/// Every thread on this machine.
pub fn all_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// How many piles the endgame tables in `dir` cover (the largest k with files for 1..=k), 0 if none.
pub fn available_endgame(dir: &Path) -> usize {
    (1..=accordion_solver::endgame::MAX_PILES)
        .take_while(|k| dir.join(format!("endgame-{k}.bin")).exists())
        .last()
        .unwrap_or(0)
}

/// Applies `--endgame` to `config`: a pile count, `off`, or `auto` (the largest tables in `dir` up to `cap` piles,
/// unless the config already chose).
pub fn choose_endgame(config: &mut Config, flag: &str, dir: &Path, cap: usize) -> Result<(), String> {
    match flag {
        "auto" if config.search.endgame_piles == 0 => {
            config.search.endgame_piles = available_endgame(dir).min(cap);
            if config.search.endgame_piles == 0 {
                eprintln!(
                    "hint: no endgame tables in {}; `acc endgame build --piles 11` builds them (about 7 minutes, 1.2 GB) and makes solving about 1.2-1.3x faster",
                    dir.display()
                );
            }
        }
        "auto" => {}
        "off" => config.search.endgame_piles = 0,
        n => {
            config.search.endgame_piles =
                n.parse().map_err(|_| format!("--endgame: {n:?} (a pile count, auto or off)"))?
        }
    }
    Ok(())
}

/// The endgame tables for up to `piles` piles from `dir` (`acc endgame build`), or none when `piles` is 0.
pub fn load_endgame(dir: &Path, piles: usize) -> Result<Option<Arc<Endgame>>, String> {
    if piles == 0 {
        return Ok(None);
    }
    let start = Instant::now();
    let tables = Endgame::load(dir, piles)
        .map_err(|e| format!("{e}\n(build them with `acc endgame build --piles {piles}`)"))?;
    eprintln!(
        "endgame tables for up to {piles} piles loaded ({:.0} MB, {:.1} s)",
        tables.bytes() as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
    Ok(Some(Arc::new(tables)))
}

/// Solves every deal in `seeds` with `config` (move-order seed = deal seed) and hands each result to `sink`.
/// `progress` is called about twice a second with the number of deals finished. Stops early if `sink` fails.
pub fn run(
    seeds: &[u64],
    config: &Config,
    opts: &Options,
    mut sink: impl FnMut(Solved) -> Result<(), String>,
    mut progress: impl FnMut(usize),
) -> Result<(), String> {
    let next = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let (tx, rx) = mpsc::sync_channel::<Solved>(4096);
    std::thread::scope(|scope| {
        for _ in 0..opts.threads.max(1) {
            let tx = tx.clone();
            let (next, abort) = (&next, &abort);
            scope.spawn(move || {
                let mut search = Search::with_options(opts.table_log2, config.search);
                search.set_endgame(opts.endgame.clone());
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= seeds.len() || abort.load(Ordering::Relaxed) {
                        break;
                    }
                    let solved = solve_one(index, seeds[index], config, &mut search, opts.records);
                    if tx.send(solved).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut done = 0;
        let mut result = Ok(());
        let mut last = Instant::now();
        for solved in rx {
            if result.is_ok() {
                result = sink(solved);
                if result.is_err() {
                    abort.store(true, Ordering::Relaxed);
                }
            }
            done += 1;
            if last.elapsed() >= Duration::from_millis(500) {
                last = Instant::now();
                progress(done);
            }
        }
        progress(done);
        result
    })
}

fn solve_one(index: usize, seed: u64, config: &Config, search: &mut Search, records: bool) -> Solved {
    let deal = Deal::from_seed(seed);
    search.clear_table();
    let start = Instant::now();
    let report = solve(&Position::from_deal(&deal), &Config { seed, ..*config }, search, None);
    let us = start.elapsed().as_micros() as u64;
    let (outcome, line) = match &report.verdict {
        Verdict::Win(line) => (Outcome::Win, line),
        _ => (Outcome::Unknown, &report.best_line),
    };
    // Nothing counts as a win until the reference engine has replayed it.
    let (verified, record) = match SolveRecord::new(&deal, Some(seed), outcome, line) {
        Err(e) => (Err(e.to_string()), None),
        Ok(mut record) => {
            record.moves_b64 = Some(encode_compact(line));
            record.moves.clear();
            let verified = verify(&record).map(|_| ()).map_err(|e| e.to_string());
            let text = records.then(|| {
                record.stats = json!({
                    "nodes": report.nodes,
                    "attempts": report.attempts,
                    "stage": report.stage.to_string(),
                    "us": us,
                })
                .as_object()
                .cloned();
                record.to_json_line()
            });
            (verified, text)
        }
    };
    Solved { index, seed, report, us, verified, record }
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

/// The `q` quantile of sorted values (nearest rank).
pub fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_interval_matches_known_values() {
        let (lo, hi) = wilson(0, 10);
        assert!(lo == 0.0 && (hi - 0.2775).abs() < 1e-3, "{hi}");
        let (lo, hi) = wilson(100_000, 100_000);
        assert!((lo - 0.999_962).abs() < 1e-5 && (hi - 1.0).abs() < 1e-12, "{lo} {hi}");
    }

    #[test]
    fn every_deal_comes_back_once_and_verified() {
        let seeds: Vec<u64> = (500..560).collect();
        let opts = Options { threads: 4, table_log2: 12, endgame: None, records: true };
        let mut seen = vec![0u32; seeds.len()];
        run(
            &seeds,
            &Config::default(),
            &opts,
            |s| {
                seen[s.index] += 1;
                assert_eq!(s.seed, seeds[s.index]);
                assert!(s.verified.is_ok() && matches!(s.report.verdict, Verdict::Win(_)));
                assert!(s.record.is_some());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert!(seen.iter().all(|&n| n == 1));
    }
}
