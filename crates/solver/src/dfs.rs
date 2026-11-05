//! First-win depth-first search over the fast engine, with a dead-position table and dead-end tests that run
//! only where they pay (RESEARCH.md §5.10). No recursion and no allocation per node: a fixed stack of frames,
//! each holding its position and its untried moves as two bitmasks. Moves are tried in a fixed order or, with
//! an RNG, in random order; restart schedules and random prefixes (`solver`) build on this.
//!
//! Options (PLAN.md Phase 5): the random move order's policy, sleep sets, where the table is used, and complete
//! endgame tables for the last few piles.
//!
//! **Sleep sets.** Two moves commute (both stay legal, and either order reaches the same position) whenever the
//! left move's pile lies strictly left of the right move's target: neither touches the piles the other reads, and
//! the gap the left one closes shifts the right one's mover and target together. So after a node's move `a` has
//! been searched, a later sibling `b` that commutes with `a` need not try `a` again: `b` then `a` reaches a position
//! already searched under `a` then `b`. Each frame carries the moves that are asleep there. This is safe with the
//! table because the table only ever holds positions proven dead, and a frame is only marked dead when every move
//! was searched or is asleep, i.e. covered by a completed search of an earlier sibling.

use crate::endgame::{Endgame, RANK_PILES, pattern_key};
use crate::table::DeadTable;
use accordion_core::Move;
use accordion_core::fast::{Position, breaks_link, connected, select_bit};
use accordion_core::rng::Xoshiro256StarStar;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Run each dead-end test only at positions with at most this many piles (0 turns a test off).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gates {
    /// Connectivity, triggered by a "last shared rank removed" move or on entering the gated zone.
    pub connectivity: usize,
    /// Left-leaf pair.
    pub left_leaf: usize,
    /// Run the connectivity closure at every gated position instead of only on trigger events
    /// (for measuring what the triggers save; the answers are the same).
    pub every_node: bool,
    /// Within the connectivity gate, use the stronger test that leaves out pile 0's card
    /// ([`Position::connected_without_leftmost`]); it also runs when a pile moves onto pile 0, or a move breaks a link
    /// among the other cards.
    pub leftmost: bool,
}

impl Gates {
    pub const OFF: Gates = Gates { connectivity: 0, left_leaf: 0, every_node: false, leftmost: false };
    /// Every test at every position: the "check continuously" baseline.
    pub const ALWAYS: Gates = Gates { connectivity: 52, left_leaf: 52, every_node: true, leftmost: true };
}

impl Default for Gates {
    /// Tuned with `acc bench --sweep` (docs/BENCHMARKS.md): connectivity on trigger at <= 19 piles was fastest or
    /// within noise of it at every depth. The left-leaf test cuts 5–9% of nodes but costs more time than it saves,
    /// so it is off by default. Leaving pile 0's card out of the connectivity test cuts 17% of positions and is
    /// about 30% faster (docs/BENCHMARKS.md, after Phase 8).
    fn default() -> Gates {
        Gates { connectivity: 19, left_leaf: 0, every_node: false, leftmost: true }
    }
}

/// How the next move is picked when the search has an RNG.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Every untried move equally likely.
    #[default]
    Uniform,
    /// A random untried leap while there is one, then random slides.
    LeapsFirst,
    /// A random untried slide while there is one, then random leaps.
    SlidesFirst,
    /// The move leaving the most legal moves, ties broken at random. Builds every candidate child at each pick,
    /// so it costs more per node; it is here to measure whether the ordering saves enough nodes.
    Mobility,
}

/// Search behaviour beyond the dead-end gates (PLAN.md Phase 5). The defaults are what Phase 5 adopted: sleep sets
/// on, everything else as in Phase 4 (endgame tables need files, so callers turn them on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchOptions {
    pub gates: Gates,
    pub order: Order,
    /// Skip move orders that only permute commuting moves (sleep sets; see the module docs).
    pub sleep_sets: bool,
    /// Use the dead table only for positions with more piles than this.
    pub table_min_piles: usize,
    /// Look the child up in the table before building it (its hash is known first), instead of prefetching its
    /// slot and looking it up after.
    pub probe_first: bool,
    /// Answer positions with at most this many piles from the endgame tables (0 = off; needs tables set with
    /// [`Search::set_endgame`] that reach this far).
    pub endgame_piles: usize,
    /// Key dead positions with at most this many piles (up to 16; above the endgame tables) by their suit/rank
    /// pattern in a table of their own that is kept from deal to deal, instead of by their cards (0 = off).
    pub pattern_piles: usize,
    /// Size of that table, as log2 of 64-byte buckets.
    pub pattern_table: u32,
}

impl Default for SearchOptions {
    fn default() -> SearchOptions {
        SearchOptions {
            gates: Gates::default(),
            order: Order::Uniform,
            sleep_sets: true,
            table_min_piles: 0,
            probe_first: false,
            endgame_piles: 0,
            pattern_piles: 0,
            pattern_table: 16,
        }
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
    /// Positions answered by the endgame tables.
    pub endgame_lookups: u64,
    /// Moves skipped as asleep (sleep sets).
    pub asleep: u64,
    /// Children skipped because the pattern table knew them dead.
    pub pattern_hits: u64,
}

impl std::ops::AddAssign for Stats {
    fn add_assign(&mut self, o: Stats) {
        self.nodes += o.nodes;
        self.table_hits += o.table_hits;
        self.dead_end_cuts += o.dead_end_cuts;
        self.tests_run += o.tests_run;
        self.endgame_lookups += o.endgame_lookups;
        self.asleep += o.asleep;
        self.pattern_hits += o.pattern_hits;
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
            endgame_lookups: self.endgame_lookups - o.endgame_lookups,
            asleep: self.asleep - o.asleep,
            pattern_hits: self.pattern_hits - o.pattern_hits,
        }
    }
}

#[derive(Clone, Copy)]
struct Frame {
    pos: Position,
    /// Untried, awake moves.
    slides: u64,
    leaps: u64,
    /// Moves tried here or asleep here (sleep sets only).
    done_slides: u64,
    done_leaps: u64,
    /// Pattern key, for positions in the pattern table's range.
    key: u64,
    via: Move,
}

/// The moves asleep in the child after `pile` moves (1 or 3 left): those `done` here that commute with it,
/// renumbered for the child, where every pile right of `pile` has moved down one place.
#[inline(always)]
pub(crate) fn asleep_after(done_slides: u64, done_leaps: u64, pile: u32, leap: bool) -> (u64, u64) {
    let target = pile - if leap { 3 } else { 1 };
    // Left of the move: piles whose move (and target) lies left of its target. Unchanged numbering.
    let left = (1u64 << target) - 1;
    // Right of the move: a slide from pile >= pile+2 or a leap from pile >= pile+4 has its target right of `pile`.
    let right_slides = (done_slides >> 1) & !((1u64 << (pile + 1)) - 1);
    let right_leaps = (done_leaps >> 1) & !((1u64 << (pile + 3)) - 1);
    ((done_slides & left) | right_slides, (done_leaps & left) | right_leaps)
}

pub struct Search {
    table: DeadTable,
    /// Dead positions keyed by suit/rank pattern; deal-independent, so never cleared between deals.
    patterns: Option<DeadTable>,
    opts: SearchOptions,
    endgame: Option<Arc<Endgame>>,
    stack: Vec<Frame>,
    pub stats: Stats,
    /// Fewest piles reached since the last [`Search::reset_best`], and the line from the root that reached it.
    best_piles: usize,
    best_line: Vec<Move>,
}

impl Search {
    pub fn new(table_log2_buckets: u32, gates: Gates) -> Search {
        Search::with_options(table_log2_buckets, SearchOptions { gates, ..SearchOptions::default() })
    }

    pub fn with_options(table_log2_buckets: u32, opts: SearchOptions) -> Search {
        Search {
            table: DeadTable::new(table_log2_buckets),
            patterns: (opts.pattern_piles > 0).then(|| DeadTable::new(opts.pattern_table)),
            opts,
            endgame: None,
            stack: Vec::with_capacity(53),
            stats: Stats::default(),
            best_piles: usize::MAX,
            best_line: Vec::new(),
        }
    }

    pub fn gates(&self) -> Gates {
        self.opts.gates
    }

    pub fn options(&self) -> SearchOptions {
        self.opts
    }

    /// Changes the options; the tables (dead facts, whatever the options) are kept.
    pub fn set_options(&mut self, opts: SearchOptions) {
        if opts.pattern_piles > 0 && self.patterns.is_none() {
            self.patterns = Some(DeadTable::new(opts.pattern_table));
        }
        self.opts = opts;
    }

    /// Endgame tables to answer small positions from, up to `options().endgame_piles` piles.
    pub fn set_endgame(&mut self, tables: Option<Arc<Endgame>>) {
        self.endgame = tables;
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

    /// Pile count at or below which positions are answered by the endgame tables (0 when there are none).
    #[inline(always)]
    fn endgame_reach(&self) -> usize {
        match &self.endgame {
            Some(e) => self.opts.endgame_piles.min(e.max_piles()),
            None => 0,
        }
    }

    /// Dead-end tests for a new position, gated by its pile count. `parent_len` tells whether the position just
    /// entered the connectivity zone: connected positions only become disconnected on a link-broken move, so
    /// inside the zone the full check runs only on entry and on those moves. With `gates.leftmost`, the move into
    /// `pos` covered `covered`, and `onto_leftmost` says whether it landed on pile 0: the stronger test leaves pile
    /// 0's card out, so it also runs when that card changes or a move breaks a link among the others.
    #[inline]
    fn dead_end(&mut self, pos: &Position, parent_len: usize, onto_leftmost: bool, covered: u8) -> bool {
        let n = pos.len();
        let g = self.opts.gates;
        let triggered = g.every_node || pos.link_broken() || parent_len > g.connectivity;
        if n <= g.connectivity {
            if g.leftmost {
                if triggered
                    || onto_leftmost
                    || breaks_link(pos.present() & !(1u64 << pos.leftmost()), covered)
                {
                    self.stats.tests_run += 1;
                    if !pos.connected_without_leftmost() {
                        return true;
                    }
                }
            } else if triggered {
                self.stats.tests_run += 1;
                if !connected(pos.present()) {
                    return true;
                }
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

    /// The line on the stack (moves from the root to the top frame), followed by `last`.
    fn line_to(&self, last: &[Move]) -> Vec<Move> {
        self.stack.iter().skip(1).map(|f| f.via).chain(last.iter().copied()).collect()
    }

    /// [`Search::run`] with options: with `rng`, moves are tried in random order (see [`Order`]); with `stop`, the
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
        let opts = self.opts;
        let reach = self.endgame_reach();
        if root.len() <= reach {
            return self.endgame_root(root);
        }
        // Pile counts in (reach, pattern_top] use the pattern table; above table_min, the card-keyed table.
        let pattern_top =
            if self.patterns.is_some() { opts.pattern_piles.min(RANK_PILES).max(reach) } else { reach };
        let in_patterns = |n: usize| n > reach && n <= pattern_top;
        let table_min = opts.table_min_piles.max(pattern_top);
        let root_key = if in_patterns(root.len()) { pattern_key(root.codes()) } else { 0 };
        if (root.len() > table_min && self.table.contains(root.hash()))
            || (in_patterns(root.len()) && self.patterns.as_ref().is_some_and(|t| t.contains(root_key)))
            || self.dead_end(root, usize::MAX, true, 0)
        {
            return SearchResult::Dead;
        }
        let start = self.stats.nodes;
        self.stack.clear();
        self.stack.push(Frame {
            pos: *root,
            slides: root.slides(),
            leaps: root.leaps(),
            done_slides: 0,
            done_leaps: 0,
            key: root_key,
            via: Move::slide(0),
        });
        while let Some(top) = self.stack.last_mut() {
            let (n1, n3) = (top.slides.count_ones(), top.leaps.count_ones());
            if n1 + n3 == 0 {
                let dead = self.stack.pop().expect("non-empty");
                if dead.pos.len() > table_min {
                    self.table.insert(dead.pos.hash());
                } else if in_patterns(dead.pos.len()) {
                    self.patterns.as_mut().expect("pattern range needs the table").insert(dead.key);
                }
                continue;
            }
            let (pile, leap) = match rng.as_deref_mut() {
                // Lowest pile first, slide before leap: the reference engine's move order.
                None => {
                    let pile = (top.slides | top.leaps).trailing_zeros();
                    (pile, top.slides >> pile & 1 == 0)
                }
                // Every move alike: weighting leaps tied here, though it pays in the ladder's prefix
                // (docs/BENCHMARKS.md).
                Some(r) => pick(&top.pos, top.slides, top.leaps, opts.order, 1, r),
            };
            let bit = 1u64 << pile;
            let (sleep_slides, sleep_leaps) = if opts.sleep_sets {
                asleep_after(top.done_slides, top.done_leaps, pile, leap)
            } else {
                (0, 0)
            };
            if leap {
                top.leaps &= !bit;
                top.done_leaps |= bit;
            } else {
                top.slides &= !bit;
                top.done_slides |= bit;
            }
            let hash = top.pos.hash_after(pile as usize, leap);
            let parent_len = top.pos.len();
            let child_len = parent_len - 1;
            let probe = child_len > table_min;
            let via = Move { pile: pile as u8, leap };

            self.stats.nodes += 1;
            if child_len < self.best_piles {
                // Reuses the buffer: short searches (ladder attempts) reach a new depth many times each.
                self.best_piles = child_len;
                self.best_line.clear();
                self.best_line.extend(self.stack.iter().skip(1).map(|f| f.via));
                self.best_line.push(via);
            }
            if child_len == 1 {
                return SearchResult::Win(self.line_to(&[via]));
            }
            if self.stats.nodes - start > budget {
                return SearchResult::OutOfBudget;
            }
            if self.stats.nodes & 0xFFF == 0 && stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
                return SearchResult::Stopped;
            }
            if probe {
                if opts.probe_first {
                    if self.table.contains(hash) {
                        self.stats.table_hits += 1;
                        continue;
                    }
                } else {
                    self.table.prefetch(hash);
                }
            }
            let top = self.stack.last().expect("non-empty");
            let target = pile as usize - if leap { 3 } else { 1 };
            let covered = top.pos.codes()[target];
            let mut child = top.pos;
            child.apply_with_hash(pile as usize, leap, hash);

            if child_len <= reach {
                self.stats.endgame_lookups += 1;
                let tables = self.endgame.as_ref().expect("reach > 0 needs tables");
                if tables.winnable(&child) == Some(true) {
                    let rest = tables.finish(&child).expect("a winnable position has a line");
                    let mut line = self.line_to(&[via]);
                    line.extend(rest);
                    return SearchResult::Win(line);
                }
                continue;
            }
            if probe && !opts.probe_first && self.table.contains(hash) {
                self.stats.table_hits += 1;
                continue;
            }
            let key = if in_patterns(child_len) {
                let key = pattern_key(child.codes());
                if self.patterns.as_ref().expect("pattern range needs the table").contains(key) {
                    self.stats.pattern_hits += 1;
                    continue;
                }
                key
            } else {
                0
            };
            if self.dead_end(&child, parent_len, target == 0, covered) {
                self.stats.dead_end_cuts += 1;
                if probe {
                    self.table.insert(hash);
                } else if in_patterns(child_len) {
                    self.patterns.as_mut().expect("pattern range needs the table").insert(key);
                }
                continue;
            }
            let (slides, leaps) = (child.slides() & !sleep_slides, child.leaps() & !sleep_leaps);
            debug_assert_eq!(sleep_slides & !child.slides(), 0, "an asleep move must stay legal");
            debug_assert_eq!(sleep_leaps & !child.leaps(), 0, "an asleep move must stay legal");
            self.stats.asleep += u64::from(sleep_slides.count_ones() + sleep_leaps.count_ones());
            self.stack.push(Frame {
                pos: child,
                slides,
                leaps,
                done_slides: sleep_slides,
                done_leaps: sleep_leaps,
                key,
                via,
            });
        }
        SearchResult::Dead
    }

    /// A root small enough for the endgame tables.
    fn endgame_root(&mut self, root: &Position) -> SearchResult {
        self.stats.endgame_lookups += 1;
        let tables = self.endgame.as_ref().expect("reach > 0 needs tables");
        match tables.winnable(root) {
            Some(true) => SearchResult::Win(tables.finish(root).expect("a winnable position has a line")),
            _ => SearchResult::Dead,
        }
    }
}

/// The next move to try at `pos` among the `slides` and `leaps` still untried there (not both empty). Where the
/// order picks at random among them, a leap is `leap_weight` times as likely as a slide (1: every move alike).
#[inline]
pub(crate) fn pick(
    pos: &Position,
    slides: u64,
    leaps: u64,
    order: Order,
    leap_weight: u32,
    rng: &mut Xoshiro256StarStar,
) -> (u32, bool) {
    let (n1, n3) = (slides.count_ones(), leaps.count_ones());
    let uniform = |rng: &mut Xoshiro256StarStar| {
        let w = leap_weight.max(1);
        let k = rng.below(u64::from(n1) + u64::from(n3) * u64::from(w)) as u32;
        if k < n1 { (select_bit(slides, k), false) } else { (select_bit(leaps, (k - n1) / w), true) }
    };
    match order {
        Order::Uniform => uniform(rng),
        Order::LeapsFirst if n3 > 0 => (select_bit(leaps, rng.below(u64::from(n3)) as u32), true),
        Order::SlidesFirst if n1 > 0 => (select_bit(slides, rng.below(u64::from(n1)) as u32), false),
        Order::LeapsFirst | Order::SlidesFirst => uniform(rng),
        Order::Mobility => {
            let (mut best, mut ties, mut choice) = (0u32, 0u64, (0, false));
            for (mask, leap) in [(slides, false), (leaps, true)] {
                let mut m = mask;
                while m != 0 {
                    let pile = m.trailing_zeros();
                    m &= m - 1;
                    let mut child = *pos;
                    child.apply(pile as usize, leap);
                    let score = child.move_count() + 1;
                    if score > best {
                        (best, ties, choice) = (score, 1, (pile, leap));
                    } else if score == best {
                        ties += 1;
                        if rng.below(ties) == 0 {
                            choice = (pile, leap);
                        }
                    }
                }
            }
            choice
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::verify::verify;
    use accordion_core::{Deal, Outcome, SolveRecord};
    use std::collections::HashSet;

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
    fn options_change_speed_not_answers() {
        // With a table large enough to hold everything, the verdict can't depend on tests, order, sleep sets or
        // where the table is used.
        let variants = [
            SearchOptions { gates: Gates::ALWAYS, ..SearchOptions::default() },
            SearchOptions { sleep_sets: true, ..SearchOptions::default() },
            SearchOptions {
                sleep_sets: true,
                probe_first: true,
                order: Order::LeapsFirst,
                ..SearchOptions::default()
            },
            SearchOptions { table_min_piles: 12, order: Order::Mobility, ..SearchOptions::default() },
            SearchOptions {
                sleep_sets: true,
                gates: Gates::OFF,
                order: Order::SlidesFirst,
                ..SearchOptions::default()
            },
            SearchOptions { sleep_sets: true, pattern_piles: 16, ..SearchOptions::default() },
        ];
        for seed in 0..40 {
            let root = residual(seed, 32);
            let verdict = |opts: SearchOptions, random: bool| {
                let mut s = Search::with_options(18, opts);
                let mut rng = Xoshiro256StarStar::from_seed(seed);
                let r = s.run_with(&root, 5_000_000, random.then_some(&mut rng), None);
                assert_ne!(r, SearchResult::OutOfBudget);
                if let SearchResult::Win(line) = &r {
                    assert!(line.iter().fold(root, |p, &m| p.play(m)).is_won());
                }
                matches!(r, SearchResult::Win(_))
            };
            let plain = SearchOptions { gates: Gates::OFF, sleep_sets: false, ..SearchOptions::default() };
            let base = verdict(plain, false);
            for (i, &opts) in variants.iter().enumerate() {
                assert_eq!(verdict(opts, false), base, "seed {seed}, variant {i}");
                assert_eq!(verdict(opts, true), base, "seed {seed}, variant {i}, random order");
            }
        }
    }

    /// Every position reachable from `pos`, by tree search with sleep sets and no cache of any kind.
    fn reach_with_sleep_sets(pos: &Position, done: (u64, u64), seen: &mut HashSet<Vec<u8>>) {
        seen.insert(pos.codes().to_vec());
        let (mut done_s, mut done_l) = done;
        let (awake_s, awake_l) = (pos.slides() & !done_s, pos.leaps() & !done_l);
        for (mask, leap) in [(awake_s, false), (awake_l, true)] {
            let mut m = mask;
            while m != 0 {
                let pile = m.trailing_zeros();
                m &= m - 1;
                let sleep = asleep_after(done_s, done_l, pile, leap);
                assert_eq!(sleep.0 & !pos.play(Move { pile: pile as u8, leap }).slides(), 0);
                if leap {
                    done_l |= 1 << pile;
                } else {
                    done_s |= 1 << pile;
                }
                reach_with_sleep_sets(&pos.play(Move { pile: pile as u8, leap }), sleep, seen);
            }
        }
    }

    fn reach_plain(pos: &Position, seen: &mut HashSet<Vec<u8>>) {
        if seen.insert(pos.codes().to_vec()) {
            for mv in pos.legal_moves() {
                reach_plain(&pos.play(mv), seen);
            }
        }
    }

    #[test]
    fn sleep_sets_still_reach_every_position() {
        // Sleep sets prune move orders, never positions: a tree search with them (and no cache) must reach exactly
        // the positions an exhaustive search does.
        for seed in 0..60 {
            let root = residual(seed, 38);
            let (mut a, mut b) = (HashSet::new(), HashSet::new());
            reach_plain(&root, &mut a);
            reach_with_sleep_sets(&root, (0, 0), &mut b);
            assert_eq!(a, b, "seed {seed}");
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

    #[test]
    fn endgame_tables_answer_the_tail_of_the_search() {
        let tables = Arc::new(Endgame::build(8, 4, |_, _, _| {}));
        for seed in 0..30 {
            let root = residual(seed, 30);
            let plain =
                matches!(Search::new(18, Gates::default()).run(&root, u64::MAX), SearchResult::Win(_));
            let mut s =
                Search::with_options(18, SearchOptions { endgame_piles: 8, ..SearchOptions::default() });
            s.set_endgame(Some(tables.clone()));
            let r = s.run(&root, u64::MAX);
            assert_eq!(matches!(r, SearchResult::Win(_)), plain, "seed {seed}");
            if let SearchResult::Win(line) = r {
                assert!(line.iter().fold(root, |p, &m| p.play(m)).is_won(), "seed {seed}");
            }
        }
    }
}
