//! Solving a position: randomized first-win search under a Luby restart schedule, keeping the dead-position
//! table across restarts (PLAN.md Phase 4). Random-prefix attempts and tuned schedules follow in Phase 5.
//!
//! Search effort is heavy-tailed: a fixed move order sometimes commits to a huge dead subtree. Restarting with
//! a fresh random order after a budget from the Luby sequence (1 1 2 1 1 2 4 …) × `luby_unit` is within a log
//! factor of the best fixed cut-off for any distribution (Luby, Sinclair & Zuckerman 1993), and dead positions
//! learned in earlier attempts are skipped in later ones.

use crate::dfs::{Gates, Search, SearchResult, Stats};
use accordion_core::Move;
use accordion_core::fast::Position;
use accordion_core::rng::Xoshiro256StarStar;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Give up after generating this many positions in total.
    pub max_nodes: u64,
    /// Restart budgets are this many positions times the Luby sequence.
    pub luby_unit: u64,
    pub gates: Gates,
    /// Seeds the move-order randomness, so runs are reproducible.
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config { max_nodes: 100_000_000, luby_unit: 10_000, gates: Gates::default(), seed: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A winning line from the root.
    Win(Vec<Move>),
    /// Every line was searched and none wins. Not a proof (the dead-position table is lossy); PLAN.md §5 reserves
    /// `loss` for a completed lossless search confirmed by an independent run.
    Exhausted,
    /// The node limit, or a stop request, came first.
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub verdict: Verdict,
    pub nodes: u64,
    pub attempts: u32,
    pub stats: Stats,
    /// Fewest piles reached, and a line from the root that reaches them.
    pub best_piles: usize,
    pub best_line: Vec<Move>,
}

/// Shared by a portfolio of solvers working on one position: a cooperative stop flag (set by whoever wins) and
/// totals for progress display.
pub struct Shared {
    pub stop: AtomicBool,
    pub nodes: AtomicU64,
    pub best_piles: AtomicUsize,
}

impl Default for Shared {
    fn default() -> Shared {
        Shared {
            stop: AtomicBool::new(false),
            nodes: AtomicU64::new(0),
            best_piles: AtomicUsize::new(usize::MAX),
        }
    }
}

/// The `i`-th term (from 1) of the Luby sequence: 1 1 2 1 1 2 4 1 1 2 1 1 2 4 8 …
pub fn luby(i: u64) -> u64 {
    assert!(i >= 1, "the Luby sequence starts at 1");
    let mut i = i;
    loop {
        let k = 64 - i.leading_zeros(); // 2^(k-1) <= i < 2^k
        if i == (1u64 << k) - 1 {
            return 1u64 << (k - 1);
        }
        i -= (1u64 << (k - 1)) - 1;
    }
}

/// Solves `root` with `search` (whose dead-position table is kept and grown). With `shared`, the run stops when
/// another solver wins, stops the others when it wins, and adds to the shared totals after every attempt.
pub fn solve(root: &Position, cfg: &Config, search: &mut Search, shared: Option<&Shared>) -> Report {
    let mut rng = Xoshiro256StarStar::from_seed(cfg.seed);
    search.reset_best();
    let before = search.stats;
    let (mut nodes, mut attempts) = (0u64, 0u32);
    let verdict = loop {
        attempts += 1;
        let budget =
            cfg.luby_unit.saturating_mul(luby(u64::from(attempts))).min(cfg.max_nodes - nodes).max(1);
        let start = search.stats.nodes;
        let result = search.run_with(root, budget, Some(&mut rng), shared.map(|s| &s.stop));
        let used = search.stats.nodes - start;
        nodes += used;
        if let Some(s) = shared {
            s.nodes.fetch_add(used, Ordering::Relaxed);
            s.best_piles.fetch_min(search.best().0, Ordering::Relaxed);
        }
        match result {
            SearchResult::Win(line) => {
                if let Some(s) = shared {
                    s.stop.store(true, Ordering::Relaxed);
                }
                break Verdict::Win(line);
            }
            SearchResult::Dead => break Verdict::Exhausted,
            SearchResult::Stopped => break Verdict::Unknown,
            SearchResult::OutOfBudget if nodes >= cfg.max_nodes => break Verdict::Unknown,
            SearchResult::OutOfBudget => {}
        }
    };
    let (best_piles, best_line) = search.best();
    Report {
        verdict,
        nodes,
        attempts,
        stats: search.stats - before,
        best_piles,
        best_line: best_line.to_vec(),
    }
}

/// Solves `root` on `threads` threads at once (a portfolio), each with its own dead-position table of
/// `2^table_log2` buckets and its own move-order seed (`cfg.seed + i`); `cfg.max_nodes` applies to each thread.
/// The first win stops the others, and so does a thread that exhausts the search. `shared` shows progress while it
/// runs. The report adds up nodes, attempts and stats over all threads and keeps the best line any of them found.
pub fn portfolio(root: &Position, cfg: &Config, threads: usize, table_log2: u32, shared: &Shared) -> Report {
    let reports: Vec<Report> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.max(1))
            .map(|i| {
                let cfg = Config { seed: cfg.seed.wrapping_add(i as u64), ..*cfg };
                scope.spawn(move || {
                    let mut search = Search::new(table_log2, cfg.gates);
                    let report = solve(root, &cfg, &mut search, Some(shared));
                    if report.verdict == Verdict::Exhausted {
                        shared.stop.store(true, Ordering::Relaxed);
                    }
                    report
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("solver thread panicked")).collect()
    });
    let verdict = reports
        .iter()
        .map(|r| &r.verdict)
        .find(|v| matches!(v, Verdict::Win(_)))
        .or_else(|| reports.iter().map(|r| &r.verdict).find(|v| **v == Verdict::Exhausted))
        .cloned()
        .unwrap_or(Verdict::Unknown);
    let best = reports.iter().min_by_key(|r| r.best_piles).expect("at least one thread");
    let mut stats = Stats::default();
    for r in &reports {
        stats += r.stats;
    }
    Report {
        verdict,
        nodes: reports.iter().map(|r| r.nodes).sum(),
        attempts: reports.iter().map(|r| r.attempts).sum(),
        stats,
        best_piles: best.best_piles,
        best_line: best.best_line.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::Deal;

    #[test]
    fn portfolio_wins_and_stops_the_other_threads() {
        let root = Position::from_deal(&Deal::from_seed(11));
        let shared = Shared::default();
        let report = portfolio(&root, &Config::default(), 4, 14, &shared);
        let Verdict::Win(line) = report.verdict else { panic!("{:?}", report.verdict) };
        assert_eq!(line.len(), 51);
        assert!(shared.stop.load(Ordering::Relaxed));
        assert_eq!(report.nodes, shared.nodes.load(Ordering::Relaxed));
    }

    #[test]
    fn luby_sequence() {
        let first: Vec<u64> = (1..=15).map(luby).collect();
        assert_eq!(first, [1, 1, 2, 1, 1, 2, 4, 1, 1, 2, 1, 1, 2, 4, 8]);
    }
}
