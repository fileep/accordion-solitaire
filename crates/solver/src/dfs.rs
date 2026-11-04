//! First-win depth-first search over the fast engine, with a dead-position table and dead-end tests that run
//! only where they pay (RESEARCH.md §5.10). No recursion and no allocation per node: a fixed stack of frames,
//! each holding its position and its untried moves as two bitmasks. Restart schedules and random prefixes
//! (PLAN.md Phases 4–5) are built on top of this.

use crate::table::DeadTable;
use accordion_core::Move;
use accordion_core::fast::{Position, connected};

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
}

impl Search {
    pub fn new(table_log2_buckets: u32, gates: Gates) -> Search {
        Search {
            table: DeadTable::new(table_log2_buckets),
            gates,
            stack: Vec::with_capacity(53),
            stats: Stats::default(),
        }
    }

    pub fn gates(&self) -> Gates {
        self.gates
    }

    /// Forgets every dead position, e.g. between unrelated deals. Keep the table across attempts on the same deal.
    pub fn clear_table(&mut self) {
        self.table.clear();
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

    /// Searches for a win from `root`, generating at most `budget` positions.
    pub fn run(&mut self, root: &Position, budget: u64) -> SearchResult {
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
            let untried = top.slides | top.leaps;
            if untried == 0 {
                let dead = self.stack.pop().expect("non-empty");
                self.table.insert(dead.pos.hash());
                continue;
            }
            // Lowest pile first, slide before leap: the reference engine's move order.
            let pile = untried.trailing_zeros();
            let bit = 1u64 << pile;
            let leap = top.slides & bit == 0;
            if leap {
                top.leaps &= !bit;
            } else {
                top.slides &= !bit;
            }
            let hash = top.pos.hash_after(pile as usize, leap);
            self.table.prefetch(hash);
            let parent_len = top.pos.len();
            let mut child = top.pos;
            child.apply_with_hash(pile as usize, leap, hash);
            let via = Move { pile: pile as u8, leap };

            self.stats.nodes += 1;
            if child.is_won() {
                let mut line: Vec<Move> = self.stack.iter().skip(1).map(|f| f.via).collect();
                line.push(via);
                return SearchResult::Win(line);
            }
            if self.stats.nodes - start > budget {
                return SearchResult::OutOfBudget;
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
    use accordion_core::rng::Xoshiro256StarStar;
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
    fn gates_change_speed_not_answers() {
        // With an exact table (large enough to hold everything) the verdict can't depend on the tests.
        for seed in 0..40 {
            let root = residual(seed, 32);
            let verdict = |gates| {
                let mut s = Search::new(18, gates);
                matches!(s.run(&root, 5_000_000), SearchResult::Win(_))
            };
            let base = verdict(Gates::OFF);
            assert_eq!(verdict(Gates::default()), base, "seed {seed}");
            assert_eq!(verdict(Gates::ALWAYS), base, "seed {seed}");
        }
    }
}
