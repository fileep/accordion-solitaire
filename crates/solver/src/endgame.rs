//! Complete endgame tables (PLAN.md Phase 5 track E; RESEARCH.md §4.4, §5.3).
//!
//! Legality only asks "same suit?" and "same rank?", so a position's fate depends only on its **pattern**: the
//! top cards with suits and ranks relabelled in order of first appearance, left to right. The tables hold one bit
//! per pattern (winnable or not) for every pile count up to some K, and are valid for every deal.
//!
//! Patterns are numbered by a perfect ranking. Reading a pattern left to right, each next card is one of
//! (in this order): an unused pair of a suit and a rank already seen, a seen suit with a new rank, a new suit with a
//! seen rank, or both new. The number of ways to finish a pattern from a prefix depends only on (cards so far,
//! suits seen, ranks seen), since the prefix's cards are distinct pairs inside the seen suits × seen ranks grid, so
//! a small table of those counts turns a pattern into its index and back in one pass.

use accordion_core::Move;
use accordion_core::fast::{Position, connected, select_bit};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

/// Largest pile count with a table (12 piles is 163 billion patterns, 20 GB).
pub const MAX_PILES: usize = 12;

/// Largest pile count [`rank`] numbers (16 piles is about 2×10^16 patterns, well inside 64 bits), so the index is
/// an exact, collision-free key for the symmetry-keyed cache too.
pub const RANK_PILES: usize = 16;

/// `COUNTS[k][i][s][r]`: ways to complete a length-`k` pattern after `i` cards using `s` suits and `r` ranks
/// (0 for states no prefix can reach).
type Counts = [[[u64; 14]; 5]; RANK_PILES + 1];

/// Whether some prefix of `i` distinct cards uses exactly `s` suits and `r` ranks.
const fn reachable(i: usize, s: usize, r: usize) -> bool {
    if i == 0 { s == 0 && r == 0 } else { s >= 1 && r >= 1 && s <= i && r <= i && s * r >= i }
}

const fn counts_for(k: usize) -> Counts {
    let mut n = [[[0u64; 14]; 5]; RANK_PILES + 1];
    let mut s = 0;
    while s <= 4 {
        let mut r = 0;
        while r <= 13 {
            n[k][s][r] = 1;
            r += 1;
        }
        s += 1;
    }
    let mut i = k;
    while i > 0 {
        i -= 1;
        let mut s: usize = 0;
        while s <= 4 {
            let mut r: usize = 0;
            while r <= 13 {
                if !reachable(i, s, r) {
                    r += 1;
                    continue;
                }
                let old_old = (s * r - i) as u64;
                let mut total = old_old * n[i + 1][s][r];
                if r < 13 {
                    total += s as u64 * n[i + 1][s][r + 1];
                }
                if s < 4 {
                    total += r as u64 * n[i + 1][s + 1][r];
                }
                if s < 4 && r < 13 {
                    total += n[i + 1][s + 1][r + 1];
                }
                n[i][s][r] = total;
                r += 1;
            }
            s += 1;
        }
    }
    n
}

static COUNTS: [Counts; RANK_PILES + 1] = {
    let mut all = [[[[0u64; 14]; 5]; RANK_PILES + 1]; RANK_PILES + 1];
    let mut k = 1;
    while k <= RANK_PILES {
        all[k] = counts_for(k);
        k += 1;
    }
    all
};

/// `GRID[s][r]`: bit `rank * 4 + suit` for every pair inside the first `s` suits × first `r` ranks.
static GRID: [[u64; 14]; 5] = {
    let mut g = [[0u64; 14]; 5];
    let mut s = 0;
    while s <= 4 {
        let mut r = 0;
        while r <= 13 {
            let mut bits = 0u64;
            let mut cr = 0;
            while cr < r {
                let mut cs = 0;
                while cs < s {
                    bits |= 1 << (cr * 4 + cs);
                    cs += 1;
                }
                cr += 1;
            }
            g[s][r] = bits;
            r += 1;
        }
        s += 1;
    }
    g
};

/// Number of patterns with `k` piles.
pub fn patterns(k: usize) -> u64 {
    assert!((1..=RANK_PILES).contains(&k), "1 to {RANK_PILES} piles");
    COUNTS[k][0][0][0]
}

/// The index of the pattern of `codes` (engine bytes, left to right) among all patterns of its length.
#[inline]
pub fn rank(codes: &[u8]) -> u64 {
    let k = codes.len();
    debug_assert!((1..=RANK_PILES).contains(&k));
    let n = &COUNTS[k];
    let (mut suits, mut ranks) = ([u8::MAX; 8], [u8::MAX; 16]);
    let (mut s, mut r, mut used, mut index) = (0usize, 0usize, 0u64, 0u64);
    for (i, &c) in codes.iter().enumerate() {
        let (cs, cr) = (suits[(c >> 4 & 7) as usize], ranks[(c & 0xF) as usize]);
        let old_old = (s * r - i) as u64 * n[i + 1][s][r];
        let old_suit_new_rank = if r < 13 { s as u64 * n[i + 1][s][r + 1] } else { 0 };
        let (ps, pr) = match (cs != u8::MAX, cr != u8::MAX) {
            (true, true) => {
                let p = u32::from(cr) * 4 + u32::from(cs);
                let before = GRID[s][r] & !used & ((1u64 << p) - 1);
                index += u64::from(before.count_ones()) * n[i + 1][s][r];
                (cs as usize, cr as usize)
            }
            (true, false) => {
                index += old_old + u64::from(cs) * n[i + 1][s][r + 1];
                ranks[(c & 0xF) as usize] = r as u8;
                r += 1;
                (cs as usize, r - 1)
            }
            (false, true) => {
                index += old_old + old_suit_new_rank + u64::from(cr) * n[i + 1][s + 1][r];
                suits[(c >> 4 & 7) as usize] = s as u8;
                s += 1;
                (s - 1, cr as usize)
            }
            (false, false) => {
                let new_suit_old_rank = r as u64 * n[i + 1][s + 1][r];
                index += old_old + old_suit_new_rank + new_suit_old_rank;
                suits[(c >> 4 & 7) as usize] = s as u8;
                ranks[(c & 0xF) as usize] = r as u8;
                s += 1;
                r += 1;
                (s - 1, r - 1)
            }
        };
        used |= 1 << (pr * 4 + ps);
    }
    index
}

/// An exact key for the pattern of `codes` (1 to [`RANK_PILES`] cards), mixed for use in a hash table: the same
/// for every relabelling of suits and ranks, different for different patterns (the mix is a bijection).
#[inline]
pub fn pattern_key(codes: &[u8]) -> u64 {
    let mut z = (rank(codes) << 5 | codes.len() as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The pattern with `index` among patterns of `k` piles, as engine bytes in canonical labels (suit and rank ids
/// by first appearance). Writes `k` bytes into `out`.
pub fn unrank(mut index: u64, k: usize, out: &mut [u8]) {
    let n = &COUNTS[k];
    debug_assert!(index < n[0][0][0]);
    let (mut s, mut r, mut used) = (0usize, 0usize, 0u64);
    for (i, slot) in out[..k].iter_mut().enumerate() {
        let (cs, cr);
        let old_old = (s * r - i) as u64 * n[i + 1][s][r];
        let old_suit_new_rank = if r < 13 { s as u64 * n[i + 1][s][r + 1] } else { 0 };
        let new_suit_old_rank = if s < 4 { r as u64 * n[i + 1][s + 1][r] } else { 0 };
        if index < old_old {
            let m = index / n[i + 1][s][r];
            index %= n[i + 1][s][r];
            let p = select_bit(GRID[s][r] & !used, m as u32) as usize;
            (cs, cr) = (p % 4, p / 4);
        } else if index - old_old < old_suit_new_rank {
            index -= old_old;
            cs = (index / n[i + 1][s][r + 1]) as usize;
            index %= n[i + 1][s][r + 1];
            cr = r;
            r += 1;
        } else if index - old_old - old_suit_new_rank < new_suit_old_rank {
            index -= old_old + old_suit_new_rank;
            cr = (index / n[i + 1][s + 1][r]) as usize;
            index %= n[i + 1][s + 1][r];
            cs = s;
            s += 1;
        } else {
            index -= old_old + old_suit_new_rank + new_suit_old_rank;
            (cs, cr) = (s, r);
            s += 1;
            r += 1;
        }
        used |= 1 << (cr * 4 + cs);
        *slot = (cs << 4 | cr) as u8;
    }
}

/// One bit per pattern, for every pile count from 1 to `max_piles`.
pub struct Endgame {
    /// `bits[k]`: the table for `k` piles (`bits[0]` is unused).
    bits: Vec<Vec<u64>>,
}

const MAGIC: &[u8; 8] = b"ACCEGTB1";

impl Endgame {
    /// Builds the tables up to `max_piles`, each from the one below, on `threads` threads. `progress` hears about
    /// each finished table: (piles, patterns, winnable patterns).
    pub fn build(max_piles: usize, threads: usize, mut progress: impl FnMut(usize, u64, u64)) -> Endgame {
        assert!((1..=MAX_PILES).contains(&max_piles), "1 to {MAX_PILES} piles");
        let mut tables = Endgame { bits: vec![Vec::new()] };
        for k in 1..=max_piles {
            let table = tables.build_level(k, threads.max(1));
            let wins = table.iter().map(|w| u64::from(w.count_ones())).sum();
            tables.bits.push(table);
            progress(k, patterns(k), wins);
        }
        tables
    }

    fn build_level(&self, k: usize, threads: usize) -> Vec<u64> {
        let total = patterns(k);
        let mut bits = vec![0u64; total.div_ceil(64) as usize];
        const WORDS_PER_TASK: usize = 1024;
        let tasks = Mutex::new(bits.chunks_mut(WORDS_PER_TASK).enumerate());
        let work = || {
            let mut codes = [0u8; MAX_PILES];
            loop {
                let Some((task, chunk)) = tasks.lock().expect("no panics while held").next() else { break };
                for (w, word) in chunk.iter_mut().enumerate() {
                    let first = ((task * WORDS_PER_TASK + w) * 64) as u64;
                    for b in 0..64.min(total.saturating_sub(first)) {
                        unrank(first + b, k, &mut codes);
                        if self.decide(&Position::from_codes(&codes[..k])) {
                            *word |= 1 << b;
                        }
                    }
                }
            }
        };
        if threads == 1 {
            work();
        } else {
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(work);
                }
            });
        }
        bits
    }

    /// Winnable, from the table one pile below.
    fn decide(&self, pos: &Position) -> bool {
        if pos.len() == 1 {
            return true;
        }
        if !connected(pos.present()) {
            return false;
        }
        let below = &self.bits[pos.len() - 1];
        let (mut slides, mut leaps) = (pos.slides(), pos.leaps());
        while slides | leaps != 0 {
            let leap = slides == 0;
            let pile = if leap { leaps.trailing_zeros() } else { slides.trailing_zeros() };
            if leap {
                leaps &= leaps - 1;
            } else {
                slides &= slides - 1;
            }
            let mut child = *pos;
            child.apply(pile as usize, leap);
            let i = rank(child.codes());
            if below[(i / 64) as usize] >> (i % 64) & 1 == 1 {
                return true;
            }
        }
        false
    }

    /// Largest pile count the tables cover.
    pub fn max_piles(&self) -> usize {
        self.bits.len() - 1
    }

    /// Bytes held by the tables.
    pub fn bytes(&self) -> usize {
        self.bits.iter().map(|t| t.len() * 8).sum()
    }

    /// Whether `pos` can be won, if it has few enough piles for the tables.
    #[inline]
    pub fn winnable(&self, pos: &Position) -> Option<bool> {
        let table = self.bits.get(pos.len())?;
        let i = rank(pos.codes());
        Some(table[(i / 64) as usize] >> (i % 64) & 1 == 1)
    }

    /// A winning line from a position the tables call winnable: at each step, any move to a winnable child.
    pub fn finish(&self, pos: &Position) -> Option<Vec<Move>> {
        let mut p = *pos;
        let mut line = Vec::with_capacity(p.len().saturating_sub(1));
        while !p.is_won() {
            let mv = p.legal_moves().into_iter().find(|&m| self.winnable(&p.play(m)) == Some(true))?;
            line.push(mv);
            p = p.play(mv);
        }
        Some(line)
    }

    /// Writes the tables to `dir/endgame-<k>.bin`, one file per pile count.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        for (k, table) in self.bits.iter().enumerate().skip(1) {
            let mut f = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("endgame-{k}.bin")))?);
            f.write_all(MAGIC)?;
            f.write_all(&(k as u64).to_le_bytes())?;
            f.write_all(&patterns(k).to_le_bytes())?;
            let mut check = Fnv::default();
            for &w in table {
                f.write_all(&w.to_le_bytes())?;
                check.add(w);
            }
            f.write_all(&check.0.to_le_bytes())?;
            f.flush()?;
        }
        Ok(())
    }

    /// Reads the tables for 1 to `max_piles` piles from `dir`, checking each file's header and checksum.
    pub fn load(dir: &Path, max_piles: usize) -> Result<Endgame, String> {
        let mut tables = Endgame { bits: vec![Vec::new()] };
        for k in 1..=max_piles.min(MAX_PILES) {
            let path = dir.join(format!("endgame-{k}.bin"));
            let err = |e: std::io::Error| format!("{}: {e}", path.display());
            let mut f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(&path).map_err(err)?);
            let mut head = [0u8; 24];
            f.read_exact(&mut head).map_err(err)?;
            let field = |i: usize| u64::from_le_bytes(head[i..i + 8].try_into().expect("8 bytes"));
            if &head[..8] != MAGIC || field(8) != k as u64 || field(16) != patterns(k) {
                return Err(format!("{}: not an endgame table for {k} piles", path.display()));
            }
            let mut words = vec![0u64; patterns(k).div_ceil(64) as usize];
            let mut check = Fnv::default();
            let mut buf = vec![0u8; 1 << 20];
            let mut filled = 0;
            while filled < words.len() {
                let n = (words.len() - filled).min(buf.len() / 8);
                f.read_exact(&mut buf[..n * 8]).map_err(err)?;
                let (chunks, _) = buf.as_chunks::<8>();
                for (w, bytes) in words[filled..filled + n].iter_mut().zip(chunks) {
                    *w = u64::from_le_bytes(*bytes);
                    check.add(*w);
                }
                filled += n;
            }
            let mut sum = [0u8; 8];
            f.read_exact(&mut sum).map_err(err)?;
            if u64::from_le_bytes(sum) != check.0 {
                return Err(format!(
                    "{}: checksum mismatch (rebuild with `acc endgame build`)",
                    path.display()
                ));
            }
            tables.bits.push(words);
        }
        Ok(tables)
    }
}

/// FNV-1a over 64-bit words, for file integrity.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn add(&mut self, w: u64) {
        self.0 = (self.0 ^ w).wrapping_mul(0x100_0000_01b3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfs::{Gates, Search, SearchResult};
    use accordion_core::Deal;
    use accordion_core::rng::Xoshiro256StarStar;

    #[test]
    fn pattern_counts_match_the_research_probe() {
        // research/probes/patterns.py, an independent Python DP (RESEARCH.md §5.3). Two cards: same suit, same
        // rank, or neither.
        let expected = [
            1,
            3,
            15,
            113,
            1_101,
            12_657,
            162_863,
            2_281_239,
            34_260_563,
            546_727_587,
            9_212_838_075,
            163_161_254_555,
        ];
        for (k, n) in (1..=MAX_PILES).zip(expected) {
            assert_eq!(patterns(k), n, "{k} piles");
        }
    }

    #[test]
    fn rank_and_unrank_are_inverse_and_ignore_labels() {
        let mut out = [0u8; RANK_PILES];
        for k in 1..=6 {
            for i in 0..patterns(k) {
                unrank(i, k, &mut out);
                assert_eq!(rank(&out[..k]), i, "k={k}");
            }
        }
        // Relabelling suits and ranks keeps the index.
        let mut rng = Xoshiro256StarStar::from_seed(3);
        for _ in 0..2000 {
            let k = 1 + rng.below(RANK_PILES as u64) as usize;
            let deal = Deal::from_seed(rng.next_u64());
            let codes: Vec<u8> = deal.cards()[..k].iter().map(|c| c.code()).collect();
            let (sp, rp) = (permutation(4, &mut rng), permutation(13, &mut rng));
            let relabelled: Vec<u8> =
                codes.iter().map(|&c| (sp[(c >> 4) as usize] << 4) | rp[(c & 0xF) as usize]).collect();
            let i = rank(&codes);
            assert!(i < patterns(k));
            assert_eq!(rank(&relabelled), i);
            unrank(i, k, &mut out);
            assert_eq!(rank(&out[..k]), i);
        }
    }

    fn permutation(n: usize, rng: &mut Xoshiro256StarStar) -> Vec<u8> {
        let mut p: Vec<u8> = (0..n as u8).collect();
        for i in (1..n).rev() {
            p.swap(i, rng.below(i as u64 + 1) as usize);
        }
        p
    }

    #[test]
    fn tables_agree_with_plain_search_and_finish_wins() {
        let tables = Endgame::build(8, 4, |_, _, _| {});
        let mut rng = Xoshiro256StarStar::from_seed(11);
        let mut search = Search::new(14, Gates::OFF);
        let (mut wins, mut losses) = (0, 0);
        for _ in 0..3000 {
            let deal = Deal::from_seed(rng.next_u64());
            let k = 2 + rng.below(7) as usize;
            let pos = Position::from_tops(&deal.cards()[..k]);
            search.clear_table();
            let plain = matches!(search.run(&pos, u64::MAX), SearchResult::Win(_));
            assert_eq!(tables.winnable(&pos), Some(plain), "{:?}", pos.tops());
            if plain {
                wins += 1;
                let line = tables.finish(&pos).expect("a winnable position has a line");
                assert!(line.iter().fold(pos, |p, &m| p.play(m)).is_won());
            } else {
                losses += 1;
            }
        }
        assert!(wins > 100 && losses > 100, "{wins} {losses}");
    }

    #[test]
    fn tables_survive_a_save_and_load() {
        let tables = Endgame::build(6, 2, |_, _, _| {});
        let dir = std::env::temp_dir().join(format!("acc-egtb-test-{}", std::process::id()));
        tables.save(&dir).unwrap();
        let back = Endgame::load(&dir, 6).unwrap();
        assert_eq!(back.bits, tables.bits);
        std::fs::write(dir.join("endgame-5.bin"), b"ACCEGTB1junk").unwrap();
        assert!(Endgame::load(&dir, 6).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
