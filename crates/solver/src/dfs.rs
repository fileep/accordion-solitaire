//! First-win depth-first search over the fast engine, with a dead-position table and dead-end tests that run
//! only where they pay (RESEARCH.md §5.10). No recursion and no allocation per node: a fixed stack of frames,
//! each holding its position and its untried moves as two bitmasks. Moves are tried in a fixed order or, with
//! an RNG, in random order; restart schedules (`solver`) and random prefixes (PLAN.md Phase 5) build on this.

use crate::table::DeadTable;
use accordion_core::Move;
use accordion_core::fast::{Position, connected, select_bit};
use accordion_core::rng::Xoshiro256StarStar;
use std::sync::atomic::{AtomicBool, Ordering};

/// Run each dead-end test only at positions with at most this many piles (0 turns a test off).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gates {
    /// Connectivity, triggered by a "last shared rank removed" move or on entering the gated zone.
    pub connectivity: usize,
    /// Left-leaf pair.
    pub left_leaf: usize,
    /// Run the connectivity closure at every gated position instead of only on trigger events
    /// (for measuring what the triggers save; the answers are the same).
    pub every_node: bool,
}

impl Gates {
    pub const OFF: Gates = Gates { connectivity: 0, left_leaf: 0, every_node: false };
    /// Every test at every position: the "check continuously" baseline.
    pub const ALWAYS: Gates = Gates { connectivity: 52, left_leaf: 52, every_node: true };
}

impl Default for Gates {
    /// Tuned with `acc bench --sweep` (docs/BENCHMARKS.md): connectivity on trigger at <= 19 piles was fastest or
    /// within noise of it at every depth. The left-leaf test cuts 5–9% of nodes but costs more time than it saves,
    /// so it is off by default.
    fn default() -> Gates {
        Gates { connectivity: 19, left_leaf: 0, every_node: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchResult {
    /// A winning line from the root.
    Win(Vec<Move>),
    /// Every line from the root was searched (up to table losses) and none wins.
    Dead,
    /// The node budget ran out first.
    OutOfBudget,
    /// Another thread asked the search to stop.
    Stopped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Positions generated (children created).
    pub nodes: u64,
    /// Children skipped because the table already knew them dead.
    pub table_hits: u64,
    /// Children cut by a dead-end test.
    pub dead_end_cuts: u64,
    /// Dead-end tests actually evaluated (connectivity closures plus left-leaf scans).
    pub tests_run: u64,
}

impl std::ops::AddAssign for Stats {
    fn add_assign(&mut self, o: Stats) {
        self.nodes += o.nodes;
        self.table_hits += o.table_hits;
        self.dead_end_cuts += o.dead_end_cuts;
        self.tests_run += o.tests_run;
    }
}

impl std::ops::Sub for Stats {
    type Output = Stats;

    fn sub(self, o: Stats) -> Stats {
        Stats {
            nodes: self.nodes - o.nodes,
            table_hits: self.table_hits - o.table_hits,
            dead_end_cuts: self.dead_end_cuts - o.dead_end_cuts,
            tests_run: self.tests_run - o.tests_run,
        }
    }
}

#[derive(Clone, Copy)]
struct Frame {
    pos: Position,
    slides: u64,
    leaps: u64,
    via: Move,
}

pub struct Search {
    table: DeadTable,
    gates: Gates,
    stack: Vec<Frame>,
    pub stats: Stats,
    /// Fewest piles reached since the last [`Search::reset_best`], and the line from the root that reached it.
    best_piles: usize,
    best_line: Vec<Move>,
}

impl Search {
    pub fn new(table_log2_buckets: u32, gates: Gates) -> Search {
        Search {
            table: DeadTable::new(table_log2_buckets),
            gates,
            stack: Vec::with_capacity(53),
            stats: Stats::default(),
            best_piles: usize::MAX,
            best_line: Vec::new(),
        }
    }

    pub fn gates(&self) -> Gates {
        self.gates
    }

    /// Forgets every dead position. Dead is a fact about a position, whatever the root, so this is only to make
    /// room between unrelated deals; keep the table across attempts on the same deal.
    pub fn clear_table(&mut self) {
        self.table.clear();
    }

    /// Starts tracking the best line afresh (for a new root).
    pub fn reset_best(&mut self) {
        self.best_piles = usize::MAX;
        self.best_line.clear();
    }

    /// Fewest piles reached since the last reset, and the line from the root that reached them.
    pub fn best(&self) -> (usize, &[Move]) {
        (self.best_piles, &self.best_line)
    }

    /// Dead-end tests for a new position, gated by its pile count. `parent_len` tells whether the position just
    /// entered the connectivity zone: connected positions only become disconnected on a link-broken move, so
    /// inside the zone the full check runs only on entry and on those moves.
    #[inline]
    fn dead_end(&mut self, pos: &Position, parent_len: usize) -> bool {
        let n = pos.len();
        let g = self.gates;
        if n <= g.connectivity && (g.every_node || pos.link_broken() || parent_len > g.connectivity) {
            self.stats.tests_run += 1;
            if !connected(pos.present()) {
                return true;
            }
        }
        if n <= g.left_leaf {
            self.stats.tests_run += 1;
            if pos.left_leaf_pair() {
                return true;
            }
        }
        false
    }

    /// Searches for a win from `root` in a fixed move order, generating at most `budget` positions.
    pub fn run(&mut self, root: &Position, budget: u64) -> SearchResult {
        self.run_with(root, budget, None, None)
    }

    /// [`Search::run`] with options: with `rng`, moves are tried in uniformly random order; with `stop`, the
    /// search returns [`SearchResult::Stopped`] soon after the flag is set.
    pub fn run_with(
        &mut self,
        root: &Position,
        budget: u64,
        mut rng: Option<&mut Xoshiro256StarStar>,
        stop: Option<&AtomicBool>,
    ) -> SearchResult {
        if root.len() < self.best_piles {
            self.best_piles = root.len();
            self.best_line.clear();
        }
        if root.is_won() {
            return SearchResult::Win(Vec::new());
        }
        if self.table.contains(root.hash()) || self.dead_end(root, usize::MAX) {
            return SearchResult::Dead;
        }
        let start = self.stats.nodes;
        self.stack.clear();
        self.stack.push(Frame {
            pos: *root,
            slides: root.slides(),
            leaps: root.leaps(),
            via: Move::slide(0),
        });
        while let Some(top) = self.stack.last_mut() {
            let (n1, n3) = (top.slides.count_ones(), top.leaps.count_ones());
            if n1 + n3 == 0 {
                let dead = self.stack.pop().expect("non-empty");
                self.table.insert(dead.pos.hash());
                continue;
            }
            let (pile, leap) = match rng.as_deref_mut() {
                // Lowest pile first, slide before leap: the reference engine's move order.
                None => {
                    let pile = (top.slides | top.leaps).trailing_zeros();
                    (pile, top.slides >> pile & 1 == 0)
                }
                Some(r) => {
                    let k = r.below(u64::from(n1 + n3)) as u32;
                    if k < n1 {
                        (select_bit(top.slides, k), false)
                    } else {
                        (select_bit(top.leaps, k - n1), true)
                    }
                }
            };
            if leap {
                top.leaps &= !(1u64 << pile);
            } else {
                top.slides &= !(1u64 << pile);
            }
            let hash = top.pos.hash_after(pile as usize, leap);
            self.table.prefetch(hash);
            let parent_len = top.pos.len();
            let mut child = top.pos;
            child.apply_with_hash(pile as usize, leap, hash);
            let via = Move { pile: pile as u8, leap };

            self.stats.nodes += 1;
            if child.len() < self.best_piles {
                self.best_piles = child.len();
                self.best_line.clear();
                self.best_line.extend(self.stack.iter().skip(1).map(|f| f.via));
                self.best_line.push(via);
            }
            if child.is_won() {
                let mut line: Vec<Move> = self.stack.iter().skip(1).map(|f| f.via).collect();
                line.push(via);
                return SearchResult::Win(line);
            }
            if self.stats.nodes - start > budget {
                return SearchResult::OutOfBudget;
            }
            if self.stats.nodes & 0xFFF == 0 && stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
                return SearchResult::Stopped;
            }
            if self.table.contains(hash) {
                self.stats.table_hits += 1;
                continue;
            }
            if self.dead_end(&child, parent_len) {
                self.stats.dead_end_cuts += 1;
                self.table.insert(hash);
                continue;
            }
            self.stack.push(Frame { pos: child, slides: child.slides(), leaps: child.leaps(), via });
        }
        SearchResult::Dead
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::verify::verify;
    use accordion_core::{Deal, Outcome, SolveRecord};

    fn residual(seed: u64, k: usize) -> Position {
        let mut pos = Position::from_deal(&Deal::from_seed(seed));
        let mut rng = Xoshiro256StarStar::from_seed(seed ^ 0xABCD);
        for _ in 0..k {
            match pos.random_move(&mut rng) {
                Some(mv) => pos = pos.play(mv),
                None => break,
            }
        }
        pos
    }

    #[test]
    fn wins_found_from_the_deal_verify() {
        let mut search = Search::new(16, Gates::default());
        let mut wins = 0;
        for seed in 0..20 {
            search.clear_table();
            if let SearchResult::Win(line) =
                search.run(&Position::from_deal(&Deal::from_seed(seed)), 3_000_000)
            {
                let record =
                    SolveRecord::new(&Deal::from_seed(seed), Some(seed), Outcome::Win, &line).unwrap();
                assert_eq!(verify(&record).map(|v| v.moves), Ok(51), "seed {seed}");
                wins += 1;
            }
        }
        assert!(wins >= 15, "{wins}");
    }

    #[test]
    fn gates_and_ordering_change_speed_not_answers() {
        // With an exact table (large enough to hold everything) the verdict can't depend on tests or order.
        for seed in 0..40 {
            let root = residual(seed, 32);
            let verdict = |gates, random: bool| {
                let mut s = Search::new(18, gates);
                let mut rng = Xoshiro256StarStar::from_seed(seed);
                let r = s.run_with(&root, 5_000_000, random.then_some(&mut rng), None);
                matches!(r, SearchResult::Win(_))
            };
            let base = verdict(Gates::OFF, false);
            assert_eq!(verdict(Gates::default(), false), base, "seed {seed}");
            assert_eq!(verdict(Gates::ALWAYS, false), base, "seed {seed}");
            assert_eq!(verdict(Gates::default(), true), base, "seed {seed} (random order)");
        }
    }

    #[test]
    fn best_line_reaches_the_reported_pile_count() {
        let root = residual(3, 30);
        let mut s = Search::new(16, Gates::default());
        let _ = s.run(&root, 2_000);
        let (piles, line) = s.best();
        let end = line.iter().fold(root, |p, &mv| p.play(mv));
        assert_eq!(end.len(), piles);
        assert!(piles < root.len());
    }

    #[test]
    fn a_raised_stop_flag_stops_the_search() {
        let stop = AtomicBool::new(true);
        let mut s = Search::new(16, Gates::default());
        let r = s.run_with(&Position::from_deal(&Deal::from_seed(1)), u64::MAX, None, Some(&stop));
        assert!(matches!(r, SearchResult::Stopped | SearchResult::Win(_)), "{r:?}");
    }
}
