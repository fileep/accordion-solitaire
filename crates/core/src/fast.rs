//! Fast engine (RESEARCH.md §6). A position is the top cards only, as one-byte codes, with the legal moves
//! kept as two bitmasks and every update O(1). The reference engine ([`crate::game`]) is the oracle it is
//! tested against ([`crate::selfcheck`]).
//!
//! Layout: `buf[3..3 + len]` holds the top cards left to right and every other byte is [`SENTINEL`], so the
//! cards "1 left" and "3 left" of any pile, and the card right of the last one, can be read without bounds
//! branches. `present` has bit `code` set for each top card, which makes suit `s` the 13 bits at `16 * s`.

use crate::card::Card;
use crate::deal::Deal;
use crate::game::Move;
use crate::rng::Xoshiro256StarStar;

/// Padding byte. Its rank nibble (F) and 3-bit suit field (7) never equal a real card's, so it matches nothing.
pub const SENTINEL: u8 = 0x7F;
const OFF: usize = 3;
const ROW: u64 = 0x1FFF;
const RANK_COLUMN: u64 = 0x0001_0001_0001_0001;

/// Same suit or same rank, on engine bytes.
#[inline(always)]
pub const fn codes_match(a: u8, b: u8) -> bool {
    let x = a ^ b;
    (x & 0x0F == 0) | (x & 0x70 == 0) // non-short-circuit: no branch
}

// Zobrist hashing over adjacency pairs (RESEARCH.md §6.4): a row of distinct cards is fully determined by
// which card sits next to which, with the sentinel standing for both ends, so a slide changes 3 table entries
// and a leap 7, wherever in the row they happen. The table is indexed by the low 6 bits of each byte: cards
// are 0x00–0x3C and the sentinel lands on 0x3F, which no card uses (32 KB, no lookup step, no bounds checks).
const ZN: usize = 64;

const fn zobrist_table() -> [u64; ZN * ZN] {
    let mut table = [0u64; ZN * ZN];
    let mut state: u64 = 0xACC0_8D10_2026_0001;
    let mut i = 0;
    while i < ZN * ZN {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        table[i] = z ^ (z >> 31);
        i += 1;
    }
    table
}

static ZOBRIST: [u64; ZN * ZN] = zobrist_table();

/// Byte indices 0..64, for building "at or after k" masks in [`Position::close_gap`].
#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[repr(C, align(32))]
struct Lanes([i8; 64]);

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
static LANES: Lanes = {
    let mut lanes = [0i8; 64];
    let mut i = 0;
    while i < 64 {
        lanes[i] = i as i8;
        i += 1;
    }
    Lanes(lanes)
};

#[inline(always)]
fn z(left: u8, right: u8) -> u64 {
    ZOBRIST[((left & 0x3F) as usize) << 6 | (right & 0x3F) as usize]
}

/// The `r`-th set bit of `mask` (0-based), in O(1) with BMI2.
#[inline]
pub fn select_bit(mask: u64, r: u32) -> u32 {
    #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
    #[allow(unused_unsafe)]
    // SAFETY: this branch only compiles when BMI2 is enabled for the whole build.
    unsafe {
        core::arch::x86_64::_pdep_u64(1u64 << r, mask).trailing_zeros()
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
    {
        let mut m = mask;
        for _ in 0..r {
            m &= m - 1;
        }
        m.trailing_zeros()
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C, align(64))]
pub struct Position {
    buf: [u8; 64],
    len: u8,
    /// The move into this position removed the last rank some two suits shared: the only way a connected
    /// position can become disconnected (RESEARCH.md §5.10).
    link_broken: bool,
    present: u64,
    l1: u64,
    l3: u64,
    hash: u64,
}

impl Position {
    pub fn from_deal(deal: &Deal) -> Position {
        Position::from_tops(deal.cards())
    }

    /// A position from its top cards, left to right (distinct, 1–52 of them).
    pub fn from_tops(tops: &[Card]) -> Position {
        assert!(!tops.is_empty() && tops.len() <= 52, "1 to 52 piles");
        let mut buf = [SENTINEL; 64];
        let mut present = 0u64;
        for (i, card) in tops.iter().enumerate() {
            buf[OFF + i] = card.code();
            present |= 1 << card.code();
        }
        assert_eq!(present.count_ones() as usize, tops.len(), "top cards must be distinct");
        let mut pos =
            Position { buf, len: tops.len() as u8, link_broken: false, present, l1: 0, l3: 0, hash: 0 };
        (pos.l1, pos.l3) = pos.full_masks();
        pos.hash = pos.full_hash();
        pos
    }

    /// A position from engine bytes, left to right (distinct card codes, 1–52 of them), without allocating.
    pub fn from_codes(codes: &[u8]) -> Position {
        debug_assert!(!codes.is_empty() && codes.len() <= 52, "1 to 52 piles");
        let mut buf = [SENTINEL; 64];
        let mut present = 0u64;
        for (i, &c) in codes.iter().enumerate() {
            debug_assert!(Card::from_code(c).is_some(), "not a card code: {c:#x}");
            buf[OFF + i] = c;
            present |= 1 << c;
        }
        debug_assert_eq!(present.count_ones() as usize, codes.len(), "top cards must be distinct");
        let mut pos =
            Position { buf, len: codes.len() as u8, link_broken: false, present, l1: 0, l3: 0, hash: 0 };
        (pos.l1, pos.l3) = pos.full_masks();
        pos.hash = pos.full_hash();
        pos
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_won(&self) -> bool {
        self.len == 1
    }

    /// Top cards as engine bytes, left to right.
    pub fn codes(&self) -> &[u8] {
        &self.buf[OFF..OFF + self.len as usize]
    }

    /// The whole layout: the top cards at bytes 3.., [`SENTINEL`] in every other byte. Two positions are the same
    /// exactly when their rows are equal, and a row never starts with a zero byte.
    pub fn row(&self) -> &[u8; 64] {
        &self.buf
    }

    pub fn tops(&self) -> Vec<Card> {
        self.codes().iter().map(|&c| Card::from_code(c).expect("only cards between the sentinels")).collect()
    }

    /// Bit `i` set: pile `i` may slide onto pile `i - 1`.
    pub fn slides(&self) -> u64 {
        self.l1
    }

    /// Bit `i` set: pile `i` may leap onto pile `i - 3`.
    pub fn leaps(&self) -> u64 {
        self.l3
    }

    pub fn move_count(&self) -> u32 {
        self.l1.count_ones() + self.l3.count_ones()
    }

    /// Legal moves in the reference engine's order: by pile, slide before leap.
    pub fn legal_moves(&self) -> Vec<Move> {
        let mut out = Vec::with_capacity(self.move_count() as usize);
        let mut m = self.l1 | self.l3;
        while m != 0 {
            let p = m.trailing_zeros();
            m &= m - 1;
            if self.l1 >> p & 1 == 1 {
                out.push(Move::slide(p as u8));
            }
            if self.l3 >> p & 1 == 1 {
                out.push(Move::leap(p as u8));
            }
        }
        out
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Bit `code` set for each top card.
    pub fn present(&self) -> u64 {
        self.present
    }

    pub fn link_broken(&self) -> bool {
        self.link_broken
    }

    /// The top card of pile 0.
    pub fn leftmost(&self) -> u8 {
        self.buf[OFF]
    }

    /// Dead-end test, stronger than [`connected`]: pile 0 never moves (nothing lies to its left), so its top card
    /// never covers anything. Every other top card can only be covered by a matching card that moves, so within any
    /// group of them that shares no suit or rank with the rest, covers stay inside the group and its last card is
    /// never covered. So the top cards other than pile 0's must be connected among themselves, and pile 0's card
    /// must match one of them, since it has to be covered for one pile to remain. Holds in every position: whatever
    /// card is on pile 0 never moves again.
    pub fn connected_without_leftmost(&self) -> bool {
        if self.len < 2 {
            return true;
        }
        let left = self.buf[OFF];
        let rest = self.present & !(1u64 << left);
        let suit = (rest >> (16 * u32::from(left >> 4))) & ROW;
        let rank = (rest >> (left & 0x0F)) & RANK_COLUMN;
        (suit | rank) != 0 && connected(rest)
    }

    /// Legal-move masks computed from scratch (the incremental update must always agree).
    pub fn full_masks(&self) -> (u64, u64) {
        let (mut l1, mut l3) = (0u64, 0u64);
        for i in 1..self.len as usize {
            let c = self.buf[OFF + i];
            l1 |= (codes_match(c, self.buf[OFF + i - 1]) as u64) << i;
            l3 |= (codes_match(c, self.buf[OFF + i - 3]) as u64) << i; // sentinels below pile 3
        }
        (l1, l3)
    }

    /// The adjacency-pair hash computed from scratch.
    pub fn full_hash(&self) -> u64 {
        let n = self.len as usize;
        (OFF - 1..OFF + n).fold(0, |h, i| h ^ z(self.buf[i], self.buf[i + 1]))
    }

    /// Hash of the position after a legal move, without building it, so the caller can prefetch its table slot.
    #[inline]
    pub fn hash_after(&self, pile: usize, leap: bool) -> u64 {
        let b = &self.buf;
        let i = OFF + pile;
        let mover = b[i];
        if leap {
            // ... a t x y mover next ...  ->  ... a mover x y next ...
            let (a, t, x, y, next) = (b[i - 4], b[i - 3], b[i - 2], b[i - 1], b[i + 1]);
            self.hash
                ^ z(a, t)
                ^ z(t, x)
                ^ z(y, mover)
                ^ z(mover, next)
                ^ z(a, mover)
                ^ z(mover, x)
                ^ z(y, next)
        } else {
            // ... a t mover next ...  ->  ... a mover next ...
            let (a, t) = (b[i - 2], b[i - 1]);
            self.hash ^ z(a, t) ^ z(t, mover) ^ z(a, mover)
        }
    }

    /// Plays a legal move: pile `pile` moves 1 (slide) or 3 (leap) piles left.
    #[inline]
    pub fn apply(&mut self, pile: usize, leap: bool) {
        let hash = self.hash_after(pile, leap);
        self.apply_with_hash(pile, leap, hash);
    }

    /// [`Position::apply`] with the new hash already known from [`Position::hash_after`].
    #[inline]
    pub fn apply_with_hash(&mut self, pile: usize, leap: bool, hash: u64) {
        let n = self.len as usize;
        let d = if leap { 3 } else { 1 };
        debug_assert!(pile < n && pile >= d, "move out of range");
        debug_assert!(codes_match(self.buf[OFF + pile], self.buf[OFF + pile - d]), "illegal move");
        self.hash = hash;

        let mover = self.buf[OFF + pile];
        let covered = self.buf[OFF + pile - d];
        self.buf[OFF + pile - d] = mover;
        self.close_gap(OFF + pile);
        self.len -= 1;

        self.present &= !(1u64 << covered);
        self.link_broken = breaks_link(self.present, covered);

        // Relations right of the move only shift down one place; recompute the few the move changed. The new
        // bits are computed independently and merged once, so there is no serial chain of updates.
        let low = (1u64 << pile) - 1;
        let l1 = (self.l1 & low) | ((self.l1 >> 1) & !low);
        let l3 = (self.l3 & low) | ((self.l3 >> 1) & !low);
        let (window1, bits1, window3, bits3) = if leap {
            (
                0b1011u64 << (pile - 3),
                self.slide_bit(pile - 3) | self.slide_bit(pile - 2) | self.slide_bit(pile),
                0b11_1001u64 << (pile - 3), // piles i-3, i, i+1, i+2
                self.leap_bit(pile - 3)
                    | self.leap_bit(pile)
                    | self.leap_bit(pile + 1)
                    | self.leap_bit(pile + 2),
            )
        } else {
            (
                1u64 << (pile - 1),
                self.slide_bit(pile - 1),
                0b111u64 << (pile - 1),
                self.leap_bit(pile - 1) | self.leap_bit(pile) | self.leap_bit(pile + 1),
            )
        };
        // Pile 0 can't slide and piles 0–2 can't leap; the shift could carry a stale bit into those slots.
        let valid = (1u64 << self.len) - 1;
        self.l1 = ((l1 & !window1) | bits1) & valid & !0b1;
        self.l3 = ((l3 & !window3) | bits3) & valid & !0b111;
    }

    /// Deletes the byte at `k`: everything after it moves down one place, and the sentinel padding follows.
    #[inline(always)]
    fn close_gap(&mut self, k: usize) {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        #[allow(unused_unsafe)]
        // SAFETY: `buf` is the first field of this 128-byte, 64-aligned struct, so the loads (struct bytes 0..65)
        // stay inside it and the aligned ones are aligned; the stores write exactly `buf`.
        unsafe {
            use core::arch::x86_64::*;
            let base = self as *mut Position as *mut u8;
            let lanes = LANES.0.as_ptr() as *const __m256i;
            let from = _mm256_set1_epi8(k as i8 - 1);
            let lo = _mm256_load_si256(base as *const __m256i);
            let hi = _mm256_load_si256(base.add(32) as *const __m256i);
            let lo_next = _mm256_loadu_si256(base.add(1) as *const __m256i);
            let hi_next = _mm256_loadu_si256(base.add(33) as *const __m256i);
            let lo = _mm256_blendv_epi8(lo, lo_next, _mm256_cmpgt_epi8(_mm256_load_si256(lanes), from));
            let hi =
                _mm256_blendv_epi8(hi, hi_next, _mm256_cmpgt_epi8(_mm256_load_si256(lanes.add(1)), from));
            _mm256_store_si256(base as *mut __m256i, lo);
            _mm256_store_si256(base.add(32) as *mut __m256i, hi);
        }
        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        self.buf.copy_within(k + 1..64, k);
        self.buf[63] = SENTINEL;
    }

    /// Bit `j` of the slide mask, recomputed from the cards (sentinels make out-of-row positions 0).
    #[inline(always)]
    fn slide_bit(&self, j: usize) -> u64 {
        (codes_match(self.buf[OFF + j], self.buf[OFF + j - 1]) as u64) << j
    }

    /// Bit `j` of the leap mask, recomputed from the cards.
    #[inline(always)]
    fn leap_bit(&self, j: usize) -> u64 {
        (codes_match(self.buf[OFF + j], self.buf[OFF + j - 3]) as u64) << j
    }

    pub fn play(&self, mv: Move) -> Position {
        let mut next = *self;
        next.apply(mv.pile as usize, mv.leap);
        next
    }

    /// A uniformly random legal move, or `None` when stuck. O(1): count both masks, pick the r-th set bit.
    #[inline]
    pub fn random_move(&self, rng: &mut Xoshiro256StarStar) -> Option<Move> {
        let (n1, n3) = (self.l1.count_ones(), self.l3.count_ones());
        if n1 + n3 == 0 {
            return None;
        }
        let r = rng.below(u64::from(n1 + n3)) as u32;
        Some(if r < n1 {
            Move::slide(select_bit(self.l1, r) as u8)
        } else {
            Move::leap(select_bit(self.l3, r - n1) as u8)
        })
    }

    /// Dead-end test (RESEARCH.md §5.8): two cards that each have exactly one match, lying to their left.
    /// Each would have to finish on top, so the position is lost.
    pub fn left_leaf_pair(&self) -> bool {
        let p = self.present;
        let mut leaves = 0;
        for s in 0..4 {
            let row = (p >> (16 * s)) & ROW;
            let in_suit = row.count_ones();
            if in_suit == 0 || in_suit > 2 {
                continue; // a card with one match needs its suit down to at most 2 cards
            }
            let mut cards = row;
            while cards != 0 {
                let r = cards.trailing_zeros();
                cards &= cards - 1;
                let column = (p >> r) & RANK_COLUMN;
                if in_suit + column.count_ones() != 3 {
                    continue;
                }
                let code = (16 * s + r) as u8;
                let neighbours = (row << (16 * s) | column << r) & !(1u64 << code);
                let neighbour = neighbours.trailing_zeros() as u8;
                let at = |c: u8| self.codes().iter().position(|&x| x == c);
                if at(neighbour) < at(code) {
                    leaves += 1;
                    if leaves == 2 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// After `covered` left the tops: did its suit just lose the last rank it shared with some other suit?
/// Covering a card only weakens links between its suit and the other suits holding its rank, so this is the
/// only way a connected position can become disconnected.
/// Branch-free over the four suits. If the covered card was its suit's last, the suits it linked still share its
/// rank directly, so that can't disconnect anything.
#[inline]
pub fn breaks_link(present: u64, covered: u8) -> bool {
    let (s, r) = (u32::from(covered >> 4), u32::from(covered & 0x0F));
    let row = (present >> (16 * s)) & ROW;
    let mut broken = false;
    for s2 in 0..4 {
        let other = (present >> (16 * s2)) & ROW;
        let had_rank = (other >> r) & 1 == 1; // it shared rank r with suit s until now
        broken |= (s2 != s) & had_rank & (other & row == 0);
    }
    broken & (row != 0)
}

/// Dead-end test (RESEARCH.md §5.8): the top cards are connected when the non-empty suits are linked through
/// ranks they share. A disconnected position can never be won.
pub fn connected(present: u64) -> bool {
    let rows = [present & ROW, (present >> 16) & ROW, (present >> 32) & ROW, (present >> 48) & ROW];
    let nonempty = (0..4).filter(|&s| rows[s] != 0).fold(0u32, |m, s| m | 1 << s);
    if nonempty == 0 {
        return true;
    }
    let mut reached = 1u32 << nonempty.trailing_zeros();
    loop {
        let ranks = (0..4).filter(|&s| reached >> s & 1 == 1).fold(0, |m, s| m | rows[s]);
        let next = (0..4).filter(|&s| rows[s] & ranks != 0).fold(reached, |m, s| m | 1 << s);
        if next == reached {
            return reached == nonempty;
        }
        reached = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(text: &str) -> Vec<Card> {
        text.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn layout_and_hash_basics() {
        let pos = Position::from_deal(&Deal::from_seed(1));
        assert_eq!(pos.len(), 52);
        assert_eq!(pos.tops(), Deal::from_seed(1).cards().to_vec());
        assert_eq!(pos.present().count_ones(), 52);
        assert_eq!(pos.hash(), pos.full_hash());
        let other = Position::from_deal(&Deal::from_seed(2));
        assert_ne!(pos.hash(), other.hash());
    }

    #[test]
    fn slides_and_leaps_match_the_reference_example() {
        let mut pos = Position::from_tops(&cards("AC 2C 3D 4C"));
        assert_eq!(pos.legal_moves(), vec![Move::slide(1), Move::leap(3)]);
        pos.apply(3, true);
        assert_eq!(pos.tops(), cards("4C 2C 3D"));
        assert_eq!((pos.slides(), pos.leaps()), pos.full_masks());
        assert_eq!(pos.hash(), pos.full_hash());
    }

    #[test]
    fn random_move_is_uniform_over_legal_moves() {
        let pos = Position::from_deal(&Deal::from_seed(5));
        let legal = pos.legal_moves();
        let mut rng = Xoshiro256StarStar::from_seed(9);
        let mut counts = vec![0u32; legal.len()];
        let trials = 200 * legal.len() as u32;
        for _ in 0..trials {
            let mv = pos.random_move(&mut rng).unwrap();
            counts[legal.iter().position(|&m| m == mv).expect("a legal move")] += 1;
        }
        assert!(counts.iter().all(|&c| (100..=320).contains(&c)), "{counts:?}");
    }

    #[test]
    fn connectivity_and_left_leaf_on_small_positions() {
        // AC and 2D share nothing with each other: disconnected.
        assert!(!connected(Position::from_tops(&cards("AC 2D")).present()));
        assert!(connected(Position::from_tops(&cards("AC 2C 2D")).present()));
        // Connected (5S-9S-9C-2C, 5S-5H), but 5H's only match is 5S and 2C's only match is 9C, both to
        // their left: each would have to finish on top.
        let pos = Position::from_tops(&cards("5S 9C 9S 5H 2C"));
        assert!(connected(pos.present()));
        assert!(pos.left_leaf_pair());
        // 2H gives 5H and 2C a second match each.
        let pos = Position::from_tops(&cards("5S 9C 9S 5H 2C 2H"));
        assert!(connected(pos.present()));
        assert!(!pos.left_leaf_pair());
    }

    #[test]
    fn the_leftmost_card_cannot_link_the_others() {
        // AH links 2H and AS, but pile 0 never moves: 2H can only slide onto AH, which leaves AS stranded.
        let pos = Position::from_tops(&cards("AH 2H AS"));
        assert!(connected(pos.present()));
        assert!(!pos.connected_without_leftmost());
        assert!(!explore_won(pos));
        // AS and 2S share a suit, and AH matches AS: AS slides onto AH, then 2S onto AS.
        let pos = Position::from_tops(&cards("AH AS 2S"));
        assert!(pos.connected_without_leftmost());
        assert!(explore_won(pos));
        // Pile 0's card must match one of the others, even when they are connected among themselves.
        assert!(!Position::from_tops(&cards("AH 2S 3S")).connected_without_leftmost());
        assert!(Position::from_tops(&cards("AH")).connected_without_leftmost());
    }

    /// Whether any line from `pos` wins (exhaustive; small positions only).
    fn explore_won(pos: Position) -> bool {
        pos.is_won() || pos.legal_moves().into_iter().any(|mv| explore_won(pos.play(mv)))
    }
}
