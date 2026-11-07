//! The check that a run's deals are different games, past what memory holds. By the birthday bound a repeat is out of the
//! question if the generator behaves like a random one (about 1e-33 at a trillion deals), so this has only to back that
//! assumption; looking at every deal would take 5 bytes a deal and two passes over the deals at the end, 5 TB and a day at a
//! trillion deals. So a run this long goes by a sample. A hash of the game (which renaming suits and ranks doesn't change)
//! picks a fraction of the deals as they are solved, when their cards are at hand anyway, and each one picked goes in a log.
//! Both deals of any repeat are picked together, so the sample sees a given repeat with the chance the fraction says, where a
//! sample of seeds would see it with that chance squared.
//!
//! The log is in seed order, like everything else a run counts, so that a checkpoint can say how much of it counts. At the
//! end it is read a part at a time, by the first bits of the key, and what shares a key is compared card by card.

use crate::runner::{Seeds, fingerprint, mix};
use accordion_core::Deal;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rayon::slice::{ParallelSlice, ParallelSliceMut};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

/// Bytes of a fingerprint in the log, which are enough that the matches by chance take little time to compare.
const KEY_BYTES: usize = 5;
/// Bytes of a deal's position in the run.
const INDEX_BYTES: usize = 5;
/// A deal's bytes in the log.
pub const RECORD: usize = KEY_BYTES + INDEX_BYTES;
/// The most deals a run's log can number.
pub const MAX_DEALS: u64 = 1 << (8 * INDEX_BYTES);
/// Cards the sample goes by. A game's hash has to be worked out for every deal; this many cards are a few nanoseconds, and
/// so many patterns that none of them shows in the fraction.
const SAMPLE_CARDS: usize = 16;
/// Pairs of the same game kept to report. A generator that had gone wrong could make far more than memory holds.
const PAIRS_KEPT: usize = 1000;

/// Which deals are in the sample: those whose game hashes below this.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Sieve {
    pub below: u64,
}

impl Sieve {
    /// The sample of `count` deals that takes about `bytes` on disk.
    pub fn for_bytes(bytes: u64, count: u64) -> Sieve {
        let fraction = bytes as f64 / (count as f64 * RECORD as f64);
        Sieve { below: (fraction.min(1.0) * 2f64.powi(64)) as u64 }
    }

    pub fn fraction(&self) -> f64 {
        self.below as f64 / 2f64.powi(64)
    }

    /// Adds the deal at position `index` to `records` if it is in the sample.
    #[inline]
    pub fn note(&self, deal: &Deal, index: usize, records: &mut Vec<u8>) {
        if game_hash(deal) <= self.below {
            Sieve::record(deal, index, records);
        }
    }

    /// [`Sieve::note`] for a deal that the GPU was given, which is in `codes` as the engine's card bytes: the deal itself,
    /// shuffled from its seed again, is made by `deal` only for the few in the sample (a shuffle costs ten times the hash).
    #[inline]
    pub fn note_codes(&self, codes: &[u8; 52], index: usize, records: &mut Vec<u8>, deal: impl FnOnce() -> Deal) {
        if game_hash_codes(codes.iter().copied()) <= self.below {
            Sieve::record(&deal(), index, records);
        }
    }

    fn record(deal: &Deal, index: usize, records: &mut Vec<u8>) {
        records.extend_from_slice(&fingerprint(deal).to_le_bytes()[..KEY_BYTES]);
        records.extend_from_slice(&(index as u64).to_le_bytes()[..INDEX_BYTES]);
    }
}

/// A hash of the deal's first cards with suits and ranks renamed in order of first appearance, as [`Deal::relabelled`]
/// does for them all, but without branches: whether a rank is new among the first cards can't be predicted, and with them
/// this took 85 ns a deal, not 30.
#[inline]
fn game_hash(deal: &Deal) -> u64 {
    game_hash_codes(deal.cards().iter().map(|card| card.code()))
}

/// [`game_hash`] from the cards' engine bytes (`suit << 4 | rank`), as the GPU is given them.
#[inline]
fn game_hash_codes(codes: impl Iterator<Item = u8>) -> u64 {
    #[inline]
    fn renamed(names: &mut [u8], next: &mut u8, old: usize) -> u8 {
        let name = names[old];
        let new = name == u8::MAX;
        let name = if new { *next } else { name };
        names[old] = name;
        *next += u8::from(new);
        name
    }
    let (mut suits, mut ranks) = ([u8::MAX; 4], [u8::MAX; 13]);
    let (mut next_suit, mut next_rank) = (0, 0);
    let mut words = [0u64; SAMPLE_CARDS / 8];
    for (i, code) in codes.take(SAMPLE_CARDS).enumerate() {
        let suit = renamed(&mut suits, &mut next_suit, (code >> 4) as usize);
        let rank = renamed(&mut ranks, &mut next_rank, (code & 0x0F) as usize);
        words[i / 8] = words[i / 8] << 8 | u64::from(suit << 4 | rank);
    }
    words.iter().fold(0x9E37_79B9_7F4A_7C15, |h, &w| mix(h ^ w))
}

/// How far the check of a log has got, and what it has found.
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// The parts of the log read so far, of how many.
    pub parts_done: u64,
    pub parts: u64,
    /// Pairs of deals with the same key that are not the same game, which should number about what [`chance_matches`] says.
    pub chance: u64,
    /// Pairs of deals that are the same game, and some of them.
    pub repeats: u64,
    pub same: Vec<(u64, u64)>,
}

/// How many pairs of `records` deals share a key by chance, if the deals are as good as random: a test of the generator
/// that a flaw too small to make a repeat in the sample might still fail.
pub fn chance_matches(records: u64) -> f64 {
    let n = records as f64;
    n * (n - 1.0) / 2.0 / 2f64.powi(8 * KEY_BYTES as i32)
}

/// How many pairs of deals could be the same with `95%` confidence after a sample of this fraction finds none of them.
pub fn repeats_ruled_out(fraction: f64) -> u64 {
    if fraction >= 1.0 { 0 } else { (0.05f64.ln() / (1.0 - fraction).ln()).ceil() as u64 }
}

fn key_of(record: &[u8; RECORD]) -> u64 {
    let mut word = [0; 8];
    word[..KEY_BYTES].copy_from_slice(&record[..KEY_BYTES]);
    u64::from_le_bytes(word)
}

fn index_of(record: &[u8; RECORD]) -> usize {
    let mut word = [0; 8];
    word[..INDEX_BYTES].copy_from_slice(&record[KEY_BYTES..]);
    u64::from_le_bytes(word) as usize
}

/// Goes on with the check of the first `bytes` of the log, holding at most about `memory` bytes of it, from where `found`
/// has got to, keeping `found` up to date so that a run can save it and carry on.
pub fn check(log: &Path, bytes: u64, seeds: Seeds, memory: u64, found: &Mutex<Found>) -> Result<(), String> {
    let parts = bytes.div_ceil(memory.max(1 << 20) / 10 * 9).max(1);
    let first = {
        let mut found = found.lock().expect("no panics");
        // Asked to use less or more memory than it began with, it has to start over.
        if found.parts != parts {
            *found = Found { parts, ..Found::default() };
        }
        found.parts_done
    };
    for part in first..parts {
        if parts > 1 {
            eprintln!("  checking part {} of {parts} of {:.1} GB", part + 1, bytes as f64 / 1e9);
        }
        check_part(log, bytes, part, parts, seeds, found)?;
    }
    Ok(())
}

fn check_part(log: &Path, bytes: u64, part: u64, parts: u64, seeds: Seeds, found: &Mutex<Found>) -> Result<(), String> {
    let mut members = read_part(log, bytes, part, parts)?;
    members.par_sort_unstable_by_key(key_of);
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 1..=members.len() {
        if i == members.len() || key_of(&members[i]) != key_of(&members[start]) {
            if i - start > 1 {
                runs.push(start..i);
            }
            start = i;
        }
    }
    let (pairs, repeats, same) = runs
        .par_iter()
        .map(|run| compare(&members[run.clone()], seeds))
        .reduce(|| (0, 0, Vec::new()), |a, b| (a.0 + b.0, a.1 + b.1, least_pairs(a.2, b.2)));
    drop(members);
    let mut found = found.lock().expect("no panics");
    found.chance += pairs - repeats;
    found.repeats += repeats;
    found.same = least_pairs(std::mem::take(&mut found.same), same);
    found.parts_done = part + 1;
    Ok(())
}

/// The first [`PAIRS_KEPT`] of these pairs, in order.
fn least_pairs(mut a: Vec<(u64, u64)>, b: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if b.is_empty() && a.is_sorted() {
        return a;
    }
    a.extend(b);
    a.sort_unstable();
    a.truncate(PAIRS_KEPT);
    a
}

/// The records of one part of the log: its keys are spread evenly, so each part is about as large.
fn read_part(log: &Path, bytes: u64, part: u64, parts: u64) -> Result<Vec<[u8; RECORD]>, String> {
    let reading = |e: std::io::Error| format!("reading {}: {e}", log.display());
    let mut file = File::open(log).map_err(reading)?;
    file.seek(SeekFrom::Start(0)).map_err(reading)?;
    let mut members = Vec::with_capacity((bytes / parts / RECORD as u64 + 1024) as usize * 21 / 20);
    let mut block = vec![0u8; (1 << 26) / RECORD * RECORD];
    let mut left = bytes;
    while left > 0 {
        let size = left.min(block.len() as u64) as usize;
        file.read_exact(&mut block[..size]).map_err(reading)?;
        left -= size as u64;
        let picked: Vec<Vec<[u8; RECORD]>> = block[..size]
            .par_chunks((1 << 20) / RECORD * RECORD)
            .map(|chunk| {
                let (records, _) = chunk.as_chunks::<RECORD>();
                records.iter().filter(|r| (key_of(r) * parts) >> (8 * KEY_BYTES) == part).copied().collect()
            })
            .collect();
        for some in picked {
            members.extend(some);
        }
    }
    Ok(members)
}

/// Of records with the same key: how many pairs they make, how many of those are the same game, and some of them.
fn compare(run: &[[u8; RECORD]], seeds: Seeds) -> (u64, u64, Vec<(u64, u64)>) {
    let pairs = run.len() as u64 * (run.len() as u64 - 1) / 2;
    let mut games: Vec<(Deal, u64)> = run
        .iter()
        .map(|r| {
            let seed = seeds.seed(index_of(r));
            (Deal::from_seed(seed).relabelled(), seed)
        })
        .collect();
    games.sort_unstable_by_key(|(deal, seed)| (*deal.cards(), *seed));
    let (mut repeats, mut same) = (0, Vec::new());
    for class in games.chunk_by(|a, b| a.0 == b.0) {
        let k = class.len() as u64;
        repeats += k * (k - 1) / 2;
        for other in class.iter().skip(1).take(PAIRS_KEPT) {
            same.push((class[0].1, other.1));
        }
    }
    (pairs, repeats, same)
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::card::Card;

    /// The sample taken from a deal's codes, as the GPU's batches have them, is the sample taken from the deal.
    #[test]
    fn sample_from_codes_is_sample_from_deal() {
        let sieve = Sieve { below: u64::MAX / 4 };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for seed in 0..20_000u64 {
            let deal = Deal::from_seed(seed);
            sieve.note(&deal, seed as usize, &mut a);
            sieve.note_codes(&accordion_gpu::deal_codes(&deal), seed as usize, &mut b, || deal.clone());
        }
        assert!(a.len() > 10 * 4000 / 2, "a quarter of the deals are in the sample");
        assert_eq!(a, b);
    }

    /// A directory for one test.
    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("acc-games-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn log_of(seeds: Seeds, sieve: Sieve, path: &Path) -> Vec<u8> {
        let mut records = Vec::new();
        for i in 0..seeds.count() {
            sieve.note(&Deal::from_seed(seeds.seed(i)), i, &mut records);
        }
        std::fs::write(path, &records).unwrap();
        records
    }

    fn checked(seeds: Seeds, sieve: Sieve, dir: &Path, memory: u64) -> Found {
        let path = dir.join("log");
        let records = log_of(seeds, sieve, &path);
        let found = Mutex::new(Found::default());
        check(&path, records.len() as u64, seeds, memory, &found).unwrap();
        found.into_inner().unwrap()
    }

    #[test]
    fn room_for_all_the_deals_picks_them_all() {
        let (sieve, mut records) = (Sieve::for_bytes(1 << 40, 1000), Vec::new());
        for i in 0..1000 {
            sieve.note(&Deal::from_seed(i), i as usize, &mut records);
        }
        assert_eq!(records.len(), 1000 * RECORD);
    }

    #[test]
    fn a_game_under_new_names_is_picked_alike() {
        let sieve = Sieve { below: 1 << 63 };
        let (mut picked, mut records) = (0, Vec::new());
        for seed in 0..2_000 {
            let deal = Deal::from_seed(seed);
            let cards: Vec<Card> = deal
                .cards()
                .iter()
                .map(|c| Card::new([2, 0, 3, 1][c.suit() as usize], 12 - c.rank()))
                .collect();
            let renamed = Deal::from_cards(&cards).unwrap();
            assert_ne!(renamed, deal);
            assert_eq!(game_hash(&renamed), game_hash(&deal));
            assert_eq!(fingerprint(&renamed), fingerprint(&deal));
            records.clear();
            sieve.note(&deal, 7, &mut records);
            picked += usize::from(!records.is_empty());
        }
        assert!((800..1200).contains(&picked), "{picked} of 2000");
    }

    #[test]
    fn the_fraction_asked_for_is_the_fraction_picked() {
        let sieve = Sieve::for_bytes(1 << 30, (1 << 30) * 8 / RECORD as u64);
        assert!((sieve.fraction() - 0.125).abs() < 1e-6, "{}", sieve.fraction());
        assert_eq!(Sieve::for_bytes(100, 1).fraction(), 1.0);
        let n = 2_000_000u64;
        let picked = (0..n).filter(|&s| game_hash(&Deal::from_seed(s)) < sieve.below).count() as f64;
        // Five standard deviations.
        let sd = (n as f64 * 0.125 * 0.875).sqrt();
        assert!((picked - n as f64 * 0.125).abs() < 5.0 * sd, "{picked} of {n}");
    }

    #[test]
    fn it_finds_what_it_should_in_any_number_of_parts() {
        let dir = temp("parts");
        let every = Sieve { below: u64::MAX };
        // No repeats: a few matches by chance would need far more deals, so nothing at all.
        let range = Seeds::Range { from: 1_000, count: 20_000 };
        let found = checked(range, every, &dir, 1 << 30);
        assert_eq!((found.parts, found.parts_done, found.repeats, found.same.len()), (1, 1, 0, 0));
        // Every 100th deal is the deal before it, and every one of them is found, in one part or in many.
        let planted = Seeds::Repeating { from: 1_000, count: 20_000, every: 100 };
        let want: Vec<(u64, u64)> = (0..200).map(|i| (1_000 + 98 + i * 100, 1_000 + 98 + i * 100)).collect();
        let mut sorted = want.clone();
        sorted.sort_unstable();
        let one = checked(planted, every, &dir, 1 << 30);
        assert_eq!((one.repeats, &one.same), (200, &sorted));
        let path = dir.join("log");
        let bytes = std::fs::metadata(&path).unwrap().len();
        assert_eq!(bytes, 20_000 * RECORD as u64);
        // Parts: at least 1 MB each, so the log has to be bigger than one.
        let large = Seeds::Repeating { from: 1_000, count: 300_000, every: 100 };
        let in_one = checked(large, every, &dir, 1 << 30);
        let in_many = checked(large, every, &dir, 1 << 20);
        assert!(in_many.parts >= 3, "{}", in_many.parts);
        assert_eq!(in_one.repeats, 3_000);
        assert_eq!((in_one.repeats, in_one.chance, &in_one.same), (in_many.repeats, in_many.chance, &in_many.same));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sample_finds_the_share_of_repeats_it_should_and_carries_on() {
        let dir = temp("share");
        let planted = Seeds::Repeating { from: 0, count: 1_000_000, every: 50 };
        // A quarter of the deals, so a quarter of the 20,000 repeats, give or take a few standard deviations.
        let quarter = Sieve { below: 1 << 62 };
        let found = checked(planted, quarter, &dir, 1 << 30);
        assert!((4_500..5_500).contains(&found.repeats), "{}", found.repeats);
        assert!(found.same.len() == PAIRS_KEPT);

        // Stopped after its first part, it carries on from what it saved to the same answer.
        let path = dir.join("log");
        let bytes = std::fs::metadata(&path).unwrap().len();
        let whole = Mutex::new(Found::default());
        check(&path, bytes, planted, 1 << 20, &whole).unwrap();
        let whole = whole.into_inner().unwrap();
        assert!(whole.parts >= 2 && whole.parts_done == whole.parts, "{}", whole.parts);
        assert_eq!(whole.repeats, found.repeats);
        let stopped = Mutex::new(Found { parts: whole.parts, ..Found::default() });
        check_part(&path, bytes, 0, whole.parts, planted, &stopped).unwrap();
        assert_eq!(stopped.lock().unwrap().parts_done, 1);
        assert!(stopped.lock().unwrap().repeats < whole.repeats);
        check(&path, bytes, planted, 1 << 20, &stopped).unwrap();
        let carried = stopped.into_inner().unwrap();
        assert!(carried == whole, "{} repeats, not {}", carried.repeats, whole.repeats);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chance_matches_come_at_the_rate_the_formula_says() {
        // 2^40 is a lot of keys, so take a short key's worth of the same arithmetic on real deals: 40-bit keys of two
        // million deals share a key about 1.8 times.
        let dir = temp("chance");
        let found = checked(Seeds::Range { from: 0, count: 2_000_000 }, Sieve { below: u64::MAX }, &dir, 1 << 30);
        assert_eq!(found.repeats, 0);
        let expected = chance_matches(2_000_000);
        assert!((1.8..1.83).contains(&expected), "{expected}");
        assert!(found.chance < 12, "{}", found.chance);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What the check costs, and how true its numbers come out at scale, with no solving: `cargo test --release -p
    /// accordion-cli rehearse -- --ignored --nocapture`, with the deals in ACC_DEALS, the first seed in ACC_FROM, the fraction
    /// in ACC_FRACTION, every how many deals to plant a repeat in ACC_EVERY, the memory in ACC_GB, and ACC_DIR for the log.
    #[test]
    #[ignore = "takes minutes and gigabytes"]
    fn rehearse() {
        use rayon::iter::{IntoParallelIterator, ParallelIterator};
        use std::io::Write;
        use std::time::Instant;
        let var = |name: &str, default: f64| std::env::var(name).map_or(default, |v| v.parse::<f64>().unwrap());
        let count = var("ACC_DEALS", 2e9) as usize;
        let from = var("ACC_FROM", 0.0) as u64;
        let fraction = var("ACC_FRACTION", 0.125);
        let every = var("ACC_EVERY", 0.0) as usize;
        let memory = (var("ACC_GB", 16.0) * (1u64 << 30) as f64) as u64;
        let seeds = if every > 0 { Seeds::Repeating { from, count, every } } else { Seeds::Range { from, count } };
        let sieve = Sieve { below: (fraction.min(1.0) * 2f64.powi(64)) as u64 };

        // One thread's costs, for the run's hot path.
        let n = 5_000_000u64;
        let t = Instant::now();
        let deals: Vec<Deal> = (0..n / 50).map(Deal::from_seed).collect();
        let shuffle = t.elapsed().as_secs_f64() / (n / 50) as f64;
        let time = |f: &dyn Fn(&Deal) -> u64| {
            let t = Instant::now();
            let mut sum = 0u64;
            for _ in 0..50 {
                for deal in &deals {
                    sum = sum.wrapping_add(f(deal));
                }
            }
            std::hint::black_box(sum);
            t.elapsed().as_secs_f64() / n as f64
        };
        let (hash, print) = (time(&game_hash), time(&fingerprint));
        eprintln!(
            "one thread: from_seed {:.0} ns, game_hash {:.0} ns, fingerprint {:.0} ns",
            shuffle * 1e9,
            hash * 1e9,
            print * 1e9
        );

        let dir = std::env::var("ACC_DIR").map_or_else(|_| temp("rehearse"), std::path::PathBuf::from);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rehearse.log");
        let t = Instant::now();
        let mut log = std::io::BufWriter::with_capacity(1 << 24, File::create(&path).unwrap());
        let chunk = 1usize << 24;
        for start in (0..count).step_by(chunk * 64) {
            let parts: Vec<Vec<u8>> = (0..64)
                .into_par_iter()
                .map(|c| {
                    let mut records = Vec::new();
                    for i in (start + c * chunk)..(start + (c + 1) * chunk).min(count) {
                        sieve.note(&Deal::from_seed(seeds.seed(i)), i, &mut records);
                    }
                    records
                })
                .collect();
            for part in parts {
                log.write_all(&part).unwrap();
            }
        }
        log.flush().unwrap();
        drop(log);
        let writing = t.elapsed().as_secs_f64();
        let bytes = std::fs::metadata(&path).unwrap().len();
        let records = bytes / RECORD as u64;
        eprintln!(
            "the log: {records} records ({:.5} of the deals, {} asked for), {:.2} GB, {writing:.1} s, {:.1}M deals/s",
            records as f64 / count as f64,
            fraction,
            bytes as f64 / 1e9,
            count as f64 / writing / 1e6
        );
        let t = Instant::now();
        let found = Mutex::new(Found::default());
        check(&path, bytes, seeds, memory, &found).unwrap();
        let found = found.into_inner().unwrap();
        let checking = t.elapsed().as_secs_f64();
        eprintln!(
            "the check: {} parts, {checking:.1} s, {:.2} GB/s; {} chance matches against {:.0} by the formula; {} repeats{}",
            found.parts,
            bytes as f64 / 1e9 / checking,
            found.chance,
            chance_matches(records),
            found.repeats,
            count.checked_div(every).map_or(String::new(), |pairs| format!(" of {pairs} planted")),
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn what_a_sample_rules_out() {
        assert_eq!(repeats_ruled_out(1.0), 0);
        assert_eq!(repeats_ruled_out(0.5), 5);
        // About 3 over the fraction.
        assert_eq!(repeats_ruled_out(0.026), 114);
    }
}
