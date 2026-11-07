//! The batch's cheap path for checking a line and making a deal. The reference engine stays the judge: [`line_wins`]
//! says yes only when the line is a legal, complete win of a full deck, and anything it doesn't accept goes to the
//! reference engine, which decides. [`deal_codes`] gives exactly the bytes of [`Deal::from_seed`].

use crate::deal::Deal;
use crate::rng::{SplitMix64, Xoshiro256StarStar};

/// Bit `code` set for each of the 52 cards' codes (`suit << 4 | rank`, rank below 13).
const DECK: u64 = 0x1FFF_1FFF_1FFF_1FFF;

/// True only if `codes` is a full deck and `line` is 51 legal moves on it, so one pile is left. False means "ask the
/// reference engine", not "illegal": this also says false on a CPU without BMI2 and AVX2.
#[inline]
pub fn line_wins(codes: &[u8; 52], line: &[u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    if let Ok(line) = <&[u8; 51]>::try_from(line)
        && is_x86_feature_detected!("bmi2")
        && is_x86_feature_detected!("avx2")
    {
        // SAFETY: BMI2 and AVX2 were just detected.
        return unsafe { line_wins_avx2(codes, line) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (codes, line);
    false
}

/// Slots never move: `tops[slot]` is the top card of the pile that lives there, and `alive` has a bit for each slot
/// still holding a pile, so pile `p` is the slot of `alive`'s `p`th set bit, which PDEP finds in one step. No move shifts
/// any bytes and no branch depends on the cards. What's cheap in bulk is checked at the end.
///
/// Why a yes is right. The bytes are below 128, so each pile is below 64 and its shift is exact. PDEP gives 0 for a pile
/// not below the number of bits in `alive`, and then nothing is removed; each move removes at most one bit, so `alive`
/// going from 52 bits to 1 in 51 moves means that every move found its pile, with 52 - k bits before move k. `target`
/// isn't negative, so the pile is at least as far from 0 as the move's distance, and the target exists too (and bit 0,
/// never a mover, is the one left). The slots in order hold the reference engine's top cards in order, by induction:
/// the mover's top goes onto the target's slot, the mover's slot goes. And each mover and the card it covers, saved in
/// `movers` and `unders`, share a rank or suit, as in `Card::matches`. The bytes past move 51 are 0 in both, so they pass.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,bmi1,bmi2")]
fn line_wins_avx2(codes: &[u8; 52], line: &[u8; 51]) -> bool {
    use std::arch::x86_64::*;
    let (mut seen, mut any) = (0u64, 0u8);
    for &c in codes {
        seen |= 1u64 << (c & 63);
        any |= c;
    }
    let mut tops = [0u8; 64];
    tops[..52].copy_from_slice(codes);
    let (mut movers, mut unders) = ([0u8; 64], [0u8; 64]);
    let mut alive = (1u64 << 52) - 1;
    let mut fail = 0u32;
    for k in 0..51 {
        let b = line[k];
        let pile = u32::from(b >> 1);
        let target = pile.wrapping_sub(1 + 2 * u32::from(b & 1));
        fail |= target;
        let from_bit = _pdep_u64(1u64 << (pile & 63), alive);
        let to_bit = _pdep_u64(1u64 << (target & 63), alive);
        let (from, to) = (from_bit.trailing_zeros() as usize & 63, to_bit.trailing_zeros() as usize & 63);
        let (mover, under) = (tops[from], tops[to]);
        movers[k] = mover;
        unders[k] = under;
        tops[to] = mover;
        alive &= !from_bit;
    }
    // SAFETY: the loads are of 32 bytes at offsets 0 and 32 of 64, and at 0 and 19 of 51.
    unsafe {
        let (rank, suit) = (_mm256_set1_epi8(0x0F), _mm256_set1_epi8(0x70));
        let zero = _mm256_setzero_si256();
        let all_match = |at: usize| {
            let d = _mm256_xor_si256(
                _mm256_loadu_si256(movers.as_ptr().add(at).cast()),
                _mm256_loadu_si256(unders.as_ptr().add(at).cast()),
            );
            _mm256_movemask_epi8(_mm256_or_si256(
                _mm256_cmpeq_epi8(_mm256_and_si256(d, rank), zero),
                _mm256_cmpeq_epi8(_mm256_and_si256(d, suit), zero),
            )) == -1
        };
        let bytes = _mm256_or_si256(
            _mm256_loadu_si256(line.as_ptr().cast()),
            _mm256_loadu_si256(line.as_ptr().add(19).cast()),
        );
        (fail >> 31 == 0)
            & (alive == 1)
            & (seen == DECK)
            & (any & 0xC0 == 0)
            & (_mm256_movemask_epi8(bytes) == 0)
            & all_match(0)
            & all_match(32)
    }
}

/// The cards' codes in deck order, what [`Deal::from_seed`] starts from.
const DECK_CODES: [u8; 52] = {
    let mut codes = [0u8; 52];
    let mut i = 0;
    while i < 52 {
        codes[i] = ((i / 13) as u8) << 4 | (i % 13) as u8;
        i += 1;
    }
    codes
};

/// The bytes of `Deal::from_seed(seed)`, each card's code.
#[inline]
pub fn deal_codes(seed: u64) -> [u8; 52] {
    let mut sm = SplitMix64::new(seed);
    let state = std::array::from_fn(|_| sm.next_u64());
    shuffled(state).unwrap_or_else(|| Deal::from_seed(seed).cards().map(|c| c.code()))
}

/// [`Deal::from_seed`]'s shuffle from a generator state: all the draws first, so that a swap's addresses wait for nothing
/// (done together, the same shuffle takes 2 to 3 times as long). None in the 1 draw in 2^58 or so where Lemire's method
/// might redraw ([`Xoshiro256StarStar::below`]), left to the reference generator.
#[inline]
fn shuffled(state: [u64; 4]) -> Option<[u8; 52]> {
    let mut rng = Xoshiro256StarStar::from_state(state);
    let mut js = [0u8; 52];
    let mut rare = false;
    for i in (1..52usize).rev() {
        let n = i as u64 + 1;
        let m = u128::from(rng.next_u64()) * u128::from(n);
        rare |= (m as u64) < n;
        js[i] = (m >> 64) as u8;
    }
    if rare {
        return None;
    }
    let mut cards = [0u8; 64];
    cards[..52].copy_from_slice(&DECK_CODES);
    for i in (1..52usize).rev() {
        let j = usize::from(js[i]) & 63;
        let (a, b) = (cards[i], cards[j]);
        cards[i] = b;
        cards[j] = a;
    }
    cards[..52].try_into().ok()
}

/// [`crate::fast::Position::masks_of_codes`] of 52 piles, each card once: slides, leaps, cards present.
#[inline]
pub fn masks52(codes: &[u8; 52]) -> (u64, u64, u64) {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        // SAFETY: AVX2 was just detected.
        return unsafe { masks52_avx2(codes) };
    }
    crate::fast::Position::masks_of_codes(codes)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn masks52_avx2(codes: &[u8; 52]) -> (u64, u64, u64) {
    use std::arch::x86_64::*;
    let p = codes.as_ptr();
    let (rank, suit) = (_mm256_set1_epi8(0x0F), _mm256_set1_epi8(0x70));
    // 32 bits: where bytes a..a+32 match bytes b..b+32.
    let matches = |a: usize, b: usize| -> u64 {
        // SAFETY: the callers' a and b are at most 20, so all 32 bytes are in the 52.
        let (x, y) = unsafe { (_mm256_loadu_si256(p.add(a).cast()), _mm256_loadu_si256(p.add(b).cast())) };
        let d = _mm256_xor_si256(x, y);
        let same_rank = _mm256_cmpeq_epi8(_mm256_and_si256(d, rank), _mm256_setzero_si256());
        let same_suit = _mm256_cmpeq_epi8(_mm256_and_si256(d, suit), _mm256_setzero_si256());
        u64::from(_mm256_movemask_epi8(_mm256_or_si256(same_rank, same_suit)) as u32)
    };
    let l1 = matches(1, 0) << 1 | matches(20, 19) << 20;
    let l3 = matches(3, 0) << 3 | matches(20, 17) << 20;
    (l1, l3, DECK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Game, Move, replay_codes};

    fn reference_codes(seed: u64) -> [u8; 52] {
        Deal::from_seed(seed).cards().map(|c| c.code())
    }

    /// A random legal line on a deal, as far as it goes, and the piles left.
    fn playout(seed: u64) -> (Vec<u8>, usize) {
        let mut game = Game::new(&Deal::from_seed(seed));
        let mut rng = Xoshiro256StarStar::from_seed(seed ^ 0x5555);
        let mut line = Vec::new();
        loop {
            let legal = game.legal_moves();
            if legal.is_empty() {
                return (line, game.pile_count());
            }
            let mv = legal[rng.below(legal.len() as u64) as usize];
            line.push(mv.to_byte());
            game.apply(mv).expect("legal");
        }
    }

    fn reference_wins(codes: &[u8; 52], line: &[u8]) -> bool {
        let moves: Vec<Move> = line.iter().map(|&b| Move::from_byte(b)).collect();
        replay_codes(codes, &moves) == Ok(1)
    }

    #[test]
    fn the_same_deals_as_the_reference_generator() {
        for seed in (0..20_000).chain([u64::MAX, 1 << 40, 700_000_000_000]) {
            assert_eq!(deal_codes(seed), reference_codes(seed), "seed {seed}");
        }
    }

    #[test]
    fn a_draw_that_might_be_redrawn_goes_to_the_reference_generator() {
        // A word 1 of 0 makes the next output 0: below(n) sees a low word below n.
        assert!(shuffled([1, 0, 3, 4]).is_none());
    }

    #[test]
    fn the_same_masks_as_the_reference() {
        for seed in 0..20_000 {
            let codes = reference_codes(seed);
            assert_eq!(masks52(&codes), crate::fast::Position::masks_of_codes(&codes), "seed {seed}");
        }
    }

    #[test]
    fn says_yes_only_to_what_the_reference_engine_does() {
        // Random playouts seldom win; a stuck line must be turned down, and so must mutations of it that the reference
        // engine turns down.
        for seed in 0..3_000 {
            let codes = reference_codes(seed);
            let (line, left) = playout(seed);
            assert_eq!(line_wins(&codes, &line), left == 1, "seed {seed}");
            for k in 0..line.len() {
                for v in [line[k] ^ 1, line[k] ^ 2, line[k].wrapping_add(2), 0xFF, 0] {
                    let mut bad = line.clone();
                    bad[k] = v;
                    assert!(!line_wins(&codes, &bad) || reference_wins(&codes, &bad), "seed {seed} move {k} byte {v}");
                }
            }
        }
    }

    #[test]
    fn says_yes_to_a_win_and_to_no_change_of_it_the_reference_engine_refuses() {
        // Thirteen ranks in four suits, in order: every slide matches, so sliding pile 1 down 51 times wins.
        let codes: [u8; 52] = std::array::from_fn(|i| DECK_CODES[i]);
        let line = [Move::slide(1).to_byte(); 51];
        assert!(line_wins(&codes, &line) == reference_wins(&codes, &line));
        for k in 0..51 {
            for v in 0..=255u8 {
                let mut m = line;
                m[k] = v;
                assert_eq!(line_wins(&codes, &m), reference_wins(&codes, &m), "move {k} byte {v}");
            }
        }
        assert!(!line_wins(&codes, &line[..50]));
    }

    #[test]
    fn not_a_deck_is_never_accepted() {
        // One suit's ranks over and over: pile 1 slides down every time, so only the deck check can say no.
        let codes: [u8; 52] = std::array::from_fn(|i| (i % 13) as u8);
        let line = [Move::slide(1).to_byte(); 51];
        assert!(reference_wins(&codes, &line));
        assert!(!line_wins(&codes, &line));
        // A byte that isn't a card, in a line that would otherwise play.
        let mut deck = DECK_CODES;
        deck[51] = 0x7F;
        assert!(!line_wins(&deck, &line));
    }
}
