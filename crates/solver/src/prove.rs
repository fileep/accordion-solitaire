//! Exhaustive search, for proving a loss (PLAN.md §5 stage 3; §6.3 Phase 7).
//!
//! The solver's dead-position table is lossy: an eviction only costs time, but a hash collision could hide a win, so
//! its "every line searched" is not a proof. Here dead positions go in an exact set keyed by the whole row of top
//! cards ([`ExactSet`]), and a search that finishes without finding a win proves the position lost. A loss is only
//! claimed when two independent runs agree (`acc prove`, and `acc batch` for the deals it doesn't win):
//! - **plain**: fixed move order, no dead-end tests, no sleep sets, no tables;
//! - **independent**: random move order, the sound dead-end tests (RESEARCH.md §5.8 and the leftmost-card test), sleep
//!   sets and the endgame tables, each with its own set.
//!
//! Exhaustive search of a typical deal is far out of reach (around 10^17 positions), but a lost deal can be small: the
//! billion-deal run's two had 74,970 and 19.4M reachable positions (docs/RESULTS.md).

use crate::dfs::{Order, asleep_after, pick};
use crate::endgame::{Endgame, RANK_PILES, pattern_key};
use accordion_core::Move;
use accordion_core::fast::Position;
use accordion_core::rng::Xoshiro256StarStar;

/// Memory for one run's set unless told otherwise.
pub const DEFAULT_SET_BYTES: usize = 8 << 30;

#[derive(Clone, Copy, Debug)]
pub struct ProveOptions {
    /// Random move order from this seed (`None`: lowest pile first, slide before leap).
    pub order_seed: Option<u64>,
    /// Run the dead-end tests at every position: connectivity without the leftmost card, and the left-leaf pair (all
    /// proven sound).
    pub dead_end_tests: bool,
    pub sleep_sets: bool,
    /// Give up after generating this many positions.
    pub max_positions: u64,
    /// Most memory the set of dead positions may take (it starts small and doubles); past it, positions are forgotten
    /// and searched again when met.
    pub max_bytes: usize,
    /// Positions of up to this many piles (at most 16) are stored by their suit/rank pattern, so that positions that
    /// are the same game with suits and ranks renamed are searched once; 0 stores every position by its cards.
    pub pattern_piles: usize,
}

impl ProveOptions {
    /// The plain run: nothing but the rules.
    pub fn plain(max_positions: u64) -> ProveOptions {
        ProveOptions {
            order_seed: None,
            dead_end_tests: false,
            sleep_sets: false,
            max_positions,
            max_bytes: DEFAULT_SET_BYTES,
            pattern_piles: 0,
        }
    }

    /// The independent run: everything the solver uses, in a random order, and pattern keys.
    pub fn independent(seed: u64, max_positions: u64) -> ProveOptions {
        ProveOptions {
            order_seed: Some(seed),
            dead_end_tests: true,
            sleep_sets: true,
            max_positions,
            max_bytes: DEFAULT_SET_BYTES,
            pattern_piles: RANK_PILES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proof {
    /// A winning line from the root.
    Won(Vec<Move>),
    /// Every line was searched and none wins: a proof.
    Lost,
    /// The position limit came first.
    Unfinished,
}

#[derive(Clone, Debug)]
pub struct ProofReport {
    pub proof: Proof,
    /// Positions generated.
    pub positions: u64,
    /// Positions in the dead set at the end (the root included when lost). For the plain run of a lost root with
    /// nothing evicted, this is every position reachable from it.
    pub dead: usize,
    /// Dead positions forgotten to stay within memory (each may have been searched again).
    pub evicted: u64,
    /// Memory the dead set took at the end.
    pub set_bytes: usize,
    /// Fewest piles reached, and a line from the root that reaches them.
    pub best_piles: usize,
    pub best_line: Vec<Move>,
}

/// Dead positions stored whole, so a hit is never wrong. A slot is a position's 64-byte row with its pile count in
/// byte 55 and its hash in bytes 56–63 (all sentinels in a row); all zero is free. A position lives in one of the
/// [`WINDOW`] slots from its hash (linear probing). The set doubles at half full until it reaches its memory cap;
/// there it fills to three quarters, and then a newcomer replaces the position near its home with the fewest piles,
/// whose subtree costs the least to search again, unless it has fewer itself.
pub struct ExactSet {
    slots: Vec<Slot>,
    bits: u32,
    max_bits: u32,
    len: usize,
    evicted: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C, align(64))]
struct Slot([u8; 64]);

const FREE: Slot = Slot([0; 64]);
/// Most slots a position may sit from its home. Lookups stop at the first free slot, so at half full they read about
/// two; the window only bounds the rare long run.
const WINDOW: usize = 64;
/// At the memory cap, a newcomer may replace the cheapest of this many slots from its home.
const EVICT_SCAN: usize = 8;
const MIN_BITS: u32 = 12;

impl Slot {
    fn of(pos: &Position) -> Slot {
        let mut row = *pos.row();
        row[55] = pos.len() as u8;
        row[56..].copy_from_slice(&pos.hash().to_le_bytes());
        Slot(row)
    }

    fn is_free(&self) -> bool {
        self.0[0] == 0
    }

    fn piles(&self) -> u8 {
        self.0[55]
    }

    fn hash(&self) -> u64 {
        u64::from_le_bytes(self.0[56..].try_into().expect("8 bytes"))
    }
}

impl ExactSet {
    /// A set that grows to at most `max_bytes` (at least 256 KB).
    pub fn new(max_bytes: usize) -> ExactSet {
        let max_bits = (max_bytes / 64).max(1 << MIN_BITS).ilog2();
        ExactSet { slots: vec![FREE; 1 << MIN_BITS], bits: MIN_BITS, max_bits, len: 0, evicted: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn bytes(&self) -> usize {
        self.slots.len() * 64
    }

    #[inline(always)]
    fn home(&self, hash: u64) -> usize {
        (hash.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - self.bits)) as usize
    }

    #[inline]
    pub fn contains(&self, pos: &Position) -> bool {
        let (home, mask) = (self.home(pos.hash()), self.slots.len() - 1);
        let key = Slot::of(pos);
        for k in 0..WINDOW {
            let slot = &self.slots[(home + k) & mask];
            if slot.is_free() {
                return false;
            }
            if *slot == key {
                return true;
            }
        }
        false
    }

    pub fn insert(&mut self, pos: &Position) {
        if self.len * 2 >= self.slots.len() && self.bits < self.max_bits {
            self.grow();
        }
        self.place(Slot::of(pos));
    }

    fn place(&mut self, key: Slot) {
        let (home, mask) = (self.home(key.hash()), self.slots.len() - 1);
        // Below the cap the set stays under half full; at the cap it fills to three quarters, then forgets.
        let room = self.bits < self.max_bits || self.len * 4 < self.slots.len() * 3;
        let mut cheapest: Option<usize> = None;
        for k in 0..WINDOW {
            let i = (home + k) & mask;
            let slot = &self.slots[i];
            if slot.is_free() {
                if room {
                    self.slots[i] = key;
                    self.len += 1;
                    return;
                }
                break;
            }
            if *slot == key {
                return;
            }
            if k < EVICT_SCAN && cheapest.is_none_or(|c| slot.piles() < self.slots[c].piles()) {
                cheapest = Some(i);
            }
        }
        if self.bits < self.max_bits {
            self.grow();
            return self.place(key);
        }
        // At the cap: forget whichever of the newcomer and the cheapest position near its home has the fewest piles.
        // Slots are only ever overwritten, never emptied, so every other position stays findable from its home.
        self.evicted += 1;
        if let Some(c) = cheapest
            && key.piles() > self.slots[c].piles()
        {
            self.slots[c] = key;
        }
    }

    fn grow(&mut self) {
        let old = std::mem::replace(&mut self.slots, vec![FREE; 1 << (self.bits + 1)]);
        self.bits += 1;
        self.len = 0;
        for slot in old.into_iter().filter(|s| !s.is_free()) {
            self.place(slot);
        }
    }

    /// Starts loading the window for a position with this hash, so a later lookup doesn't wait on memory.
    #[inline(always)]
    pub fn prefetch(&self, hash: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            let ptr = &self.slots[self.home(hash)] as *const Slot as *const i8;
            // SAFETY: a prefetch is only a hint; the pointer is in bounds of `slots` regardless.
            unsafe { core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(ptr) };
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = hash;
    }
}

/// Dead positions by their pattern ([`pattern_key`]: exact, and the same for every renaming of suits and ranks), for
/// positions small enough to have one. Open addressing on the 64-bit keys, doubling at half full up to its memory
/// cap; past three quarters full at the cap, new keys are dropped (they will be searched again when met).
struct PatternSet {
    keys: Vec<u64>,
    bits: u32,
    max_bits: u32,
    len: usize,
    dropped: u64,
}

impl PatternSet {
    fn new(max_bytes: usize) -> PatternSet {
        let max_bits = (max_bytes / 8).max(1 << MIN_BITS).ilog2();
        PatternSet { keys: vec![0; 1 << MIN_BITS], bits: MIN_BITS, max_bits, len: 0, dropped: 0 }
    }

    /// Where `key`'s probe starts (keys are already well mixed).
    fn home(&self, key: u64) -> usize {
        (key >> (64 - self.bits)) as usize
    }

    fn contains(&self, key: u64) -> bool {
        let mask = self.keys.len() - 1;
        let mut i = self.home(key);
        loop {
            match self.keys[i] {
                0 => return false,
                k if k == key => return true,
                _ => i = (i + 1) & mask,
            }
        }
    }

    fn insert(&mut self, key: u64) {
        if key == 0 {
            return; // 0 marks a free slot; the one pattern with key 0 is never stored
        }
        if self.len * 2 >= self.keys.len() && self.bits < self.max_bits {
            let old = std::mem::replace(&mut self.keys, vec![0; 1 << (self.bits + 1)]);
            self.bits += 1;
            self.len = 0;
            old.into_iter().filter(|&k| k != 0).for_each(|k| self.insert(k));
        }
        if self.len * 4 >= self.keys.len() * 3 {
            self.dropped += 1;
            return;
        }
        let mask = self.keys.len() - 1;
        let mut i = self.home(key);
        while self.keys[i] != 0 {
            if self.keys[i] == key {
                return;
            }
            i = (i + 1) & mask;
        }
        self.keys[i] = key;
        self.len += 1;
    }
}

/// The dead positions of one run: by pattern up to `pattern_piles` piles, by their cards above.
struct Dead {
    exact: ExactSet,
    patterns: PatternSet,
    pattern_piles: usize,
}

impl Dead {
    fn contains(&self, pos: &Position) -> bool {
        if pos.len() <= self.pattern_piles {
            self.patterns.contains(pattern_key(pos.codes()))
        } else {
            self.exact.contains(pos)
        }
    }

    fn insert(&mut self, pos: &Position) {
        if pos.len() <= self.pattern_piles {
            self.patterns.insert(pattern_key(pos.codes()));
        } else {
            self.exact.insert(pos);
        }
    }

    fn prefetch(&self, pos: &Position, pile: usize, leap: bool) {
        if pos.len() - 1 > self.pattern_piles {
            self.exact.prefetch(pos.hash_after(pile, leap));
        }
    }
}

struct Frame {
    pos: Position,
    slides: u64,
    leaps: u64,
    done_slides: u64,
    done_leaps: u64,
    via: Move,
}

impl Frame {
    /// A frame for `pos` with the moves in `asleep` already covered elsewhere; starts loading its children's slots.
    fn new(pos: Position, asleep: (u64, u64), via: Move, dead: &Dead) -> Frame {
        let (slides, leaps) = (pos.slides() & !asleep.0, pos.leaps() & !asleep.1);
        for (mut moves, leap) in [(slides, false), (leaps, true)] {
            while moves != 0 {
                dead.prefetch(&pos, moves.trailing_zeros() as usize, leap);
                moves &= moves - 1;
            }
        }
        Frame { pos, slides, leaps, done_slides: asleep.0, done_leaps: asleep.1, via }
    }
}

fn dead_end(pos: &Position) -> bool {
    pos.len() > 1 && (!pos.connected_without_leftmost() || pos.left_leaf_pair())
}

/// Searches every line from `root` (or until a win or the limit), with an exact set of dead positions. With
/// `endgame`, positions the tables cover are answered from them (they are exact too, so they aren't stored).
pub fn prove(root: &Position, opts: &ProveOptions, endgame: Option<&Endgame>) -> ProofReport {
    let mut rng = opts.order_seed.map(Xoshiro256StarStar::from_seed);
    let mut dead = Dead {
        exact: ExactSet::new(opts.max_bytes),
        patterns: PatternSet::new(opts.max_bytes / 4),
        pattern_piles: opts.pattern_piles.min(RANK_PILES),
    };
    let mut report = ProofReport {
        proof: Proof::Lost,
        positions: 0,
        dead: 0,
        evicted: 0,
        set_bytes: 0,
        best_piles: root.len(),
        best_line: Vec::new(),
    };
    let reach = endgame.map_or(0, Endgame::max_piles);
    let line_to = |stack: &[Frame], last: Move| -> Vec<Move> {
        stack.iter().skip(1).map(|f| f.via).chain(std::iter::once(last)).collect()
    };
    if root.is_won() {
        report.proof = Proof::Won(Vec::new());
        return report;
    }
    if root.len() <= reach {
        let tables = endgame.expect("reach > 0 needs tables");
        report.proof = match tables.finish(root) {
            Some(line) if tables.winnable(root) == Some(true) => Proof::Won(line),
            _ => Proof::Lost,
        };
        report.dead = usize::from(report.proof == Proof::Lost);
        return report;
    }
    if opts.dead_end_tests && dead_end(root) {
        report.dead = 1;
        return report;
    }
    let mut stack = vec![Frame::new(*root, (0, 0), Move::slide(0), &dead)];
    while let Some(top) = stack.last_mut() {
        if top.slides | top.leaps == 0 {
            let frame = stack.pop().expect("non-empty");
            dead.insert(&frame.pos);
            continue;
        }
        if report.positions >= opts.max_positions {
            report.proof = Proof::Unfinished;
            break;
        }
        let (pile, leap) = match rng.as_mut() {
            None => {
                let pile = (top.slides | top.leaps).trailing_zeros();
                (pile, top.slides >> pile & 1 == 0)
            }
            Some(r) => pick(&top.pos, top.slides, top.leaps, Order::Uniform, 1, r),
        };
        let bit = 1u64 << pile;
        let sleep =
            if opts.sleep_sets { asleep_after(top.done_slides, top.done_leaps, pile, leap) } else { (0, 0) };
        if leap {
            top.leaps &= !bit;
            top.done_leaps |= bit;
        } else {
            top.slides &= !bit;
            top.done_slides |= bit;
        }
        let child = top.pos.play(Move { pile: pile as u8, leap });
        let via = Move { pile: pile as u8, leap };
        report.positions += 1;
        if child.len() < report.best_piles {
            report.best_piles = child.len();
            report.best_line = line_to(&stack, via);
        }
        if child.is_won() {
            report.proof = Proof::Won(line_to(&stack, via));
            break;
        }
        if child.len() <= reach {
            let tables = endgame.expect("reach > 0 needs tables");
            if tables.winnable(&child) == Some(true) {
                let mut line = line_to(&stack, via);
                line.extend(tables.finish(&child).expect("a winnable position has a line"));
                report.proof = Proof::Won(line);
                break;
            }
            continue;
        }
        if dead.contains(&child) {
            continue;
        }
        if opts.dead_end_tests && dead_end(&child) {
            dead.insert(&child);
            continue;
        }
        stack.push(Frame::new(child, sleep, via, &dead));
    }
    report.dead = dead.exact.len() + dead.patterns.len;
    report.evicted = dead.exact.evicted() + dead.patterns.dropped;
    report.set_bytes = dead.exact.bytes() + dead.patterns.keys.len() * 8;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::Deal;
    use std::collections::HashSet;

    /// Positions after 32–39 random moves from seeded deals: a mix of won and lost, small enough to search.
    fn residuals(n: u64) -> Vec<Position> {
        let mut rng = Xoshiro256StarStar::from_seed(5);
        (0..n)
            .map(|i| {
                let mut pos = Position::from_deal(&Deal::from_seed(1000 + i));
                for _ in 0..32 + rng.below(8) {
                    match pos.random_move(&mut rng) {
                        Some(mv) => pos = pos.play(mv),
                        None => break,
                    }
                }
                pos
            })
            .collect()
    }

    /// Whether a win is reachable, by the plainest search there is: recursion and a standard hash set.
    fn winnable(pos: &Position, dead: &mut HashSet<Vec<u8>>) -> bool {
        if pos.is_won() {
            return true;
        }
        if dead.contains(pos.codes()) {
            return false;
        }
        if pos.legal_moves().into_iter().any(|mv| winnable(&pos.play(mv), dead)) {
            return true;
        }
        dead.insert(pos.codes().to_vec());
        false
    }

    #[test]
    fn small_positions_prove_the_same_every_way() {
        let tables = Endgame::build(8, 4, |_, _, _| {});
        let (mut won, mut lost) = (0, 0);
        for (i, pos) in residuals(400).iter().enumerate() {
            let mut reachable = HashSet::new();
            let expect = winnable(pos, &mut reachable);
            let plain = prove(pos, &ProveOptions::plain(u64::MAX), None);
            let independent = prove(pos, &ProveOptions::independent(i as u64, u64::MAX), Some(&tables));
            // A set far too small for the search: it forgets nearly everything, which must only cost time.
            let tiny =
                prove(pos, &ProveOptions { max_bytes: 0, ..ProveOptions::independent(7, u64::MAX) }, None);
            for p in [&plain.proof, &independent.proof, &tiny.proof] {
                assert_eq!(*p != Proof::Lost, expect, "residual {i}");
                if let Proof::Won(line) = p {
                    assert!(line.iter().fold(*pos, |q, &m| q.play(m)).is_won());
                }
            }
            if expect {
                won += 1;
            } else {
                assert_eq!(plain.dead, reachable.len(), "the plain run stores every reachable position");
                lost += 1;
            }
        }
        assert!(won > 10 && lost > 10, "{won} {lost}");
    }

    #[test]
    fn the_set_finds_what_it_holds_and_forgets_only_at_its_cap() {
        let positions = residuals(6000);
        let distinct: HashSet<&[u8]> = positions.iter().map(Position::codes).collect();
        let mut big = ExactSet::new(1 << 30);
        let mut small = ExactSet::new(0);
        for (i, p) in positions.iter().enumerate() {
            assert!(!big.contains(p) || positions[..i].iter().any(|q| q.codes() == p.codes()));
            big.insert(p);
            small.insert(p);
            assert!(big.contains(p) && big.contains(&positions[i / 2]));
        }
        assert_eq!((big.len(), big.evicted()), (distinct.len(), 0));
        assert!(small.evicted() > 0 && small.len() <= 1 << MIN_BITS);
        // Whatever the small set still holds, it finds.
        let held = distinct.iter().filter(|c| small.contains(&Position::from_codes(c))).count();
        assert_eq!(held, small.len());
    }
}
