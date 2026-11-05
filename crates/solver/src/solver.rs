//! Solving a position: a chain of stages sharing one dead-position table (PLAN.md Phases 4–5).
//!
//! 1. **Ladder** (optional; RESEARCH.md §5.5–5.6). Each attempt plays a random prefix, then a first-win search of
//!    what is left with a small budget. Rungs start with long prefixes (cheap searches that win surprisingly often)
//!    and move the switch point earlier, with a bigger budget, rung by rung.
//! 2. **Thorough search, always last.** Randomized first-win search from the root under a restart schedule, until
//!    the total budget runs out. This is the Phase 4 solver: every fast stage only ever goes in front of it.
//!
//! Search effort is heavy-tailed: a fixed move order sometimes commits to a huge dead subtree. Restarting with
//! a fresh random order after a budget from the Luby sequence (1 1 2 1 1 2 4 …) × `luby_unit` is within a log
//! factor of the best fixed cut-off for any distribution (Luby, Sinclair & Zuckerman 1993), and dead positions
//! learned in earlier attempts (of either stage) are skipped in later ones.

use crate::dfs::{Order, Search, SearchOptions, SearchResult, Stats, pick};
use crate::endgame::Endgame;
use accordion_core::Move;
use accordion_core::fast::Position;
use accordion_core::rng::Xoshiro256StarStar;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Restart budgets for the thorough stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restarts {
    /// `luby_unit` × the Luby sequence.
    #[default]
    Luby,
    /// `luby_unit`, then each next budget `percent`% of the last.
    Geometric { percent: u32 },
    /// One attempt with the whole budget.
    Never,
}

/// The random-prefix ladder in front of the thorough search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ladder {
    /// Random moves before the brute force, on the first rung.
    pub start: u32,
    /// Random moves on the last rung.
    pub end: u32,
    /// Random moves fewer on each next rung (0: just one rung).
    pub step: u32,
    /// Attempts per rung.
    pub tries: u32,
    /// Positions each brute force may search on the first rung.
    pub budget: u64,
    /// Each next rung's budget, in percent of the last (200 doubles it).
    pub growth: u32,
    /// How the prefix moves are chosen.
    pub prefix: Order,
}

impl Default for Ladder {
    /// Tuned in PLAN.md Phase 5 (docs/BENCHMARKS.md): many cheap tries at 36 random moves (16 piles left) win
    /// 94% of deals on the first rung; the optimum is flat around 36 moves, 256 tries, a 1,000–4,000 budget.
    fn default() -> Ladder {
        Ladder { start: 36, end: 24, step: 2, tries: 256, budget: 4_000, growth: 200, prefix: Order::Uniform }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Give up after generating this many positions in total.
    pub max_nodes: u64,
    /// The thorough stage's first restart budget, in positions.
    pub luby_unit: u64,
    pub restarts: Restarts,
    pub search: SearchOptions,
    pub ladder: Option<Ladder>,
    /// Seeds the move-order randomness, so runs are reproducible.
    pub seed: u64,
}

impl Config {
    /// The Phase 4 solver: no ladder, no sleep sets, Luby restarts of 10,000 positions.
    pub fn phase4() -> Config {
        Config {
            luby_unit: 10_000,
            ladder: None,
            search: SearchOptions { sleep_sets: false, ..SearchOptions::default() },
            ..Config::default()
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        // PLAN.md Phase 5's adopted strategy: the ladder, then the thorough search with short Luby restarts.
        // Endgame tables (`search.endgame_piles`) need files, so callers turn them on when they have them.
        Config {
            max_nodes: 100_000_000,
            luby_unit: 1_000,
            restarts: Restarts::Luby,
            search: SearchOptions::default(),
            ladder: Some(Ladder::default()),
            seed: 0,
        }
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

/// The stage a solve ended in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Ladder {
        rung: u32,
    },
    Thorough,
    /// Ladder attempts on the GPU, in front of this solver (crates/gpu, `acc batch --gpu`); [`solve`] itself
    /// never ends here.
    Gpu,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stage::Ladder { rung } => write!(f, "ladder-{rung}"),
            Stage::Thorough => f.write_str("thorough"),
            Stage::Gpu => f.write_str("gpu"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub verdict: Verdict,
    /// Positions generated, prefix moves included.
    pub nodes: u64,
    pub attempts: u32,
    pub stage: Stage,
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

/// Running totals of one solve.
struct Run<'a> {
    nodes: u64,
    attempts: u32,
    best_piles: usize,
    best_line: Vec<Move>,
    shared: Option<&'a Shared>,
}

impl Run<'_> {
    /// Books one search: its positions, and its best line if it beats the best so far (`prefix` + its line).
    fn book(&mut self, search: &Search, used: u64, prefix: &[Move]) {
        self.nodes += used;
        let (piles, line) = search.best();
        if piles < self.best_piles {
            self.best_piles = piles;
            self.best_line = prefix.iter().chain(line).copied().collect();
        }
        if let Some(s) = self.shared {
            s.nodes.fetch_add(used, Ordering::Relaxed);
            s.best_piles.fetch_min(self.best_piles, Ordering::Relaxed);
        }
    }

    fn won(&self, line: Vec<Move>) -> Verdict {
        if let Some(s) = self.shared {
            s.stop.store(true, Ordering::Relaxed);
        }
        Verdict::Win(line)
    }
}

/// One prefix move by `order` (uniform is the O(1) random move), or `None` when stuck.
fn prefix_move(pos: &Position, order: Order, rng: &mut Xoshiro256StarStar) -> Option<Move> {
    if pos.move_count() == 0 {
        return None;
    }
    if order == Order::Uniform {
        return pos.random_move(rng);
    }
    let (pile, leap) = pick(pos, pos.slides(), pos.leaps(), order, rng);
    Some(Move { pile: pile as u8, leap })
}

/// Solves `root` with `search` (whose dead-position table is kept and grown). With `shared`, the run stops when
/// another solver wins, stops the others when it wins, and adds to the shared totals after every attempt.
pub fn solve(root: &Position, cfg: &Config, search: &mut Search, shared: Option<&Shared>) -> Report {
    let mut rng = Xoshiro256StarStar::from_seed(cfg.seed);
    search.set_options(cfg.search);
    let before = search.stats;
    let stop = shared.map(|s| &s.stop);
    let mut run = Run { nodes: 0, attempts: 0, best_piles: usize::MAX, best_line: Vec::new(), shared };
    let mut stage: Stage;
    let verdict = 'solve: {
        if let Some(l) = cfg.ladder {
            let (mut k, mut budget, mut rung) = (l.start, l.budget, 0);
            let mut prefix = Vec::with_capacity(52);
            loop {
                stage = Stage::Ladder { rung };
                for _ in 0..l.tries {
                    if run.nodes >= cfg.max_nodes {
                        break 'solve Verdict::Unknown;
                    }
                    run.attempts += 1;
                    let mut pos = *root;
                    prefix.clear();
                    while prefix.len() < k as usize {
                        let Some(mv) = prefix_move(&pos, l.prefix, &mut rng) else { break };
                        pos.apply(mv.pile as usize, mv.leap);
                        prefix.push(mv);
                    }
                    search.reset_best();
                    let start = search.stats.nodes;
                    let result = search.run_with(
                        &pos,
                        budget.min(cfg.max_nodes.saturating_sub(run.nodes + prefix.len() as u64)).max(1),
                        Some(&mut rng),
                        stop,
                    );
                    // The prefix's positions count too, booked with the search's.
                    run.book(search, search.stats.nodes - start + prefix.len() as u64, &prefix);
                    match result {
                        SearchResult::Win(line) => {
                            prefix.extend(line);
                            break 'solve run.won(prefix);
                        }
                        SearchResult::Stopped => break 'solve Verdict::Unknown,
                        SearchResult::Dead | SearchResult::OutOfBudget => {}
                    }
                }
                if l.step == 0 || k < l.end + l.step {
                    break;
                }
                k -= l.step;
                budget = budget.saturating_mul(u64::from(l.growth)) / 100;
                rung += 1;
            }
        }
        stage = Stage::Thorough;
        let mut geometric = cfg.luby_unit;
        for attempt in 1u64.. {
            if run.nodes >= cfg.max_nodes {
                break 'solve Verdict::Unknown;
            }
            run.attempts += 1;
            let budget = match cfg.restarts {
                Restarts::Luby => cfg.luby_unit.saturating_mul(luby(attempt)),
                Restarts::Geometric { percent } => {
                    let b = geometric;
                    geometric = geometric.saturating_mul(u64::from(percent)) / 100;
                    b.max(1)
                }
                Restarts::Never => u64::MAX,
            };
            search.reset_best();
            let start = search.stats.nodes;
            let result =
                search.run_with(root, budget.min(cfg.max_nodes - run.nodes).max(1), Some(&mut rng), stop);
            run.book(search, search.stats.nodes - start, &[]);
            match result {
                SearchResult::Win(line) => break 'solve run.won(line),
                SearchResult::Dead => break 'solve Verdict::Exhausted,
                SearchResult::Stopped => break 'solve Verdict::Unknown,
                SearchResult::OutOfBudget => {}
            }
        }
        unreachable!("the attempt loop only ends by breaking out")
    };
    Report {
        verdict,
        nodes: run.nodes,
        attempts: run.attempts,
        stage,
        stats: search.stats - before,
        best_piles: run.best_piles,
        best_line: run.best_line,
    }
}

/// Solves `root` on `threads` threads at once (a portfolio), each with its own dead-position table of
/// `2^table_log2` buckets and its own move-order seed (`cfg.seed + i`); `cfg.max_nodes` applies to each thread.
/// The first win stops the others, and so does a thread that exhausts the search. `shared` shows progress while it
/// runs. The report adds up nodes, attempts and stats over all threads and keeps the best line any of them found.
pub fn portfolio(
    root: &Position,
    cfg: &Config,
    threads: usize,
    table_log2: u32,
    endgame: Option<Arc<Endgame>>,
    shared: &Shared,
) -> Report {
    let reports: Vec<Report> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.max(1))
            .map(|i| {
                let cfg = Config { seed: cfg.seed.wrapping_add(i as u64), ..*cfg };
                let endgame = endgame.clone();
                scope.spawn(move || {
                    let mut search = Search::with_options(table_log2, cfg.search);
                    search.set_endgame(endgame);
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
    let winner = reports.iter().find(|r| matches!(r.verdict, Verdict::Win(_)));
    let verdict = winner
        .map(|r| &r.verdict)
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
        stage: winner.map_or(Stage::Thorough, |r| r.stage),
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
        let report = portfolio(&root, &Config::default(), 4, 14, None, &shared);
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

    #[test]
    fn ladder_wins_verify_and_report_their_rung() {
        let cfg = Config::default();
        let mut search = Search::with_options(14, cfg.search);
        let mut on_ladder = 0;
        for seed in 0..40 {
            search.clear_table();
            let root = Position::from_deal(&Deal::from_seed(seed));
            let report = solve(&root, &Config { seed, ..cfg }, &mut search, None);
            let Verdict::Win(line) = &report.verdict else { panic!("seed {seed}: {:?}", report.verdict) };
            assert!(line.iter().fold(root, |p, &m| p.play(m)).is_won(), "seed {seed}");
            on_ladder += matches!(report.stage, Stage::Ladder { .. }) as u32;
            let end = report.best_line.iter().fold(root, |p, &m| p.play(m));
            assert_eq!(end.len(), report.best_piles);
        }
        assert!(on_ladder > 30, "{on_ladder}");
    }

    #[test]
    fn configs_read_from_partial_json() {
        let cfg: Config = serde_json::from_str(
            r#"{"luby_unit": 3000, "search": {"sleep_sets": true}, "ladder": {"start": 34, "prefix": "mobility"},
                "restarts": {"geometric": {"percent": 150}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.luby_unit, 3000);
        assert!(cfg.search.sleep_sets);
        assert_eq!(cfg.search.gates, Config::default().search.gates);
        assert_eq!(cfg.ladder.unwrap().start, 34);
        assert_eq!(cfg.ladder.unwrap().prefix, Order::Mobility);
        assert_eq!(cfg.restarts, Restarts::Geometric { percent: 150 });
        assert!(serde_json::from_str::<Config>(r#"{"lubyunit": 1}"#).is_err());
    }
}
