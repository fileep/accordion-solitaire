//! Do seeds give deals that look like uniformly random shuffles? The win rate is a statement about uniform deals,
//! and the runs take their deals from consecutive seeds, so the seed-to-deal map is what is tested here, not the
//! generator's raw output. Every test has an exact expected result, and each compares it with the deals of a range
//! of seeds. Results don't depend on the number of threads.
//!
//!   cargo run --release -p accordion-core --example deal_stats -- --from 0 --count 22000000000
//!   cargo run --release -p accordion-core --example deal_stats -- --count 1000000 --naive      (should fail)
//!
//! `--stride` takes every n-th seed. `--naive` deals by the textbook wrong shuffle, each card swapped with any
//! position, to show that the tests can catch a bad shuffle.

use accordion_core::Deal;
use accordion_core::rng::Xoshiro256StarStar;
use std::sync::atomic::{AtomicU64, Ordering};

const N: usize = 52;
const CHUNK: u64 = 1 << 20;
/// Bins 0..BINS-1 count that many agreeing cards, and the last bin that many or more.
const BINS: usize = 9;
/// Pairs of positions tested jointly: slides and leaps, at the left end, the middle and the right end.
const PAIRS: [(usize, usize); 6] = [(0, 1), (25, 26), (50, 51), (0, 3), (24, 27), (48, 51)];
/// Seed differences whose deals must be unrelated. The runs use seeds below 2^40.
const DELTAS: [u64; 12] = [1, 2, 3, 4, 5, 8, 16, 256, 1 << 16, 1 << 24, 1 << 32, 1 << 39];

struct Tally {
    deals: u64,
    card_at: Vec<u64>,
    pairs: Vec<u64>,
    first_three: Vec<u64>,
    fixed: [u64; BINS],
    even: u64,
    inversions: u64,
    inversions_sq: u64,
    slides: u64,
    slides_sq: u64,
    leaps: u64,
    leaps_sq: u64,
    delta_deals: u64,
    agree: [[u64; BINS]; DELTAS.len()],
}

impl Tally {
    fn new() -> Tally {
        Tally {
            deals: 0,
            card_at: vec![0; N * N],
            pairs: vec![0; PAIRS.len() * N * N],
            first_three: vec![0; N * N * N],
            fixed: [0; BINS],
            even: 0,
            inversions: 0,
            inversions_sq: 0,
            slides: 0,
            slides_sq: 0,
            leaps: 0,
            leaps_sq: 0,
            delta_deals: 0,
            agree: [[0; BINS]; DELTAS.len()],
        }
    }

    fn add(&mut self, other: &Tally) {
        let add = |a: &mut [u64], b: &[u64]| a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
        self.deals += other.deals;
        add(&mut self.card_at, &other.card_at);
        add(&mut self.pairs, &other.pairs);
        add(&mut self.first_three, &other.first_three);
        add(&mut self.fixed, &other.fixed);
        self.even += other.even;
        self.inversions += other.inversions;
        self.inversions_sq += other.inversions_sq;
        self.slides += other.slides;
        self.slides_sq += other.slides_sq;
        self.leaps += other.leaps;
        self.leaps_sq += other.leaps_sq;
        self.delta_deals += other.delta_deals;
        for (a, b) in self.agree.iter_mut().zip(&other.agree) {
            add(a, b);
        }
    }

    fn count(&mut self, seed: u64, naive: bool, deltas: bool) {
        let c = indices(seed, naive);
        self.deals += 1;
        let (mut seen, mut inversions, mut fixed) = (0u64, 0u64, 0usize);
        for (p, &card) in c.iter().enumerate() {
            self.card_at[p * N + card] += 1;
            inversions += u64::from((seen >> card).count_ones());
            seen |= 1 << card;
            fixed += usize::from(p == card);
        }
        self.inversions += inversions;
        self.inversions_sq += inversions * inversions;
        self.even += 1 - (inversions & 1);
        self.fixed[fixed.min(BINS - 1)] += 1;
        for (k, &(a, b)) in PAIRS.iter().enumerate() {
            self.pairs[(k * N + c[a]) * N + c[b]] += 1;
        }
        self.first_three[(c[0] * N + c[1]) * N + c[2]] += 1;
        let matches = |a: usize, b: usize| (c[a] % 13 == c[b] % 13 || c[a] / 13 == c[b] / 13) as u64;
        let slides: u64 = (1..N).map(|i| matches(i, i - 1)).sum();
        let leaps: u64 = (3..N).map(|i| matches(i, i - 3)).sum();
        self.slides += slides;
        self.slides_sq += slides * slides;
        self.leaps += leaps;
        self.leaps_sq += leaps * leaps;
        if deltas {
            self.delta_deals += 1;
            for (bins, &delta) in self.agree.iter_mut().zip(&DELTAS) {
                let d = indices(seed.wrapping_add(delta), naive);
                let same = c.iter().zip(&d).filter(|(x, y)| x == y).count();
                bins[same.min(BINS - 1)] += 1;
            }
        }
    }
}

fn indices(seed: u64, naive: bool) -> [usize; N] {
    if naive {
        let mut rng = Xoshiro256StarStar::from_seed(seed);
        let mut cards: [usize; N] = std::array::from_fn(|i| i);
        for i in (0..N).rev() {
            cards.swap(i, rng.below(N as u64) as usize);
        }
        cards
    } else {
        Deal::from_seed(seed).cards().map(|c| usize::from(c.index()))
    }
}

/// Natural log of the gamma function (Lanczos, g = 7, n = 9).
fn ln_gamma(x: f64) -> f64 {
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    let x = x - 1.0;
    let t = x + 7.5;
    let a = C.iter().enumerate().skip(1).fold(C[0], |a, (i, c)| a + c / (x + i as f64));
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// The regularized upper incomplete gamma function Q(a, x), by series below a + 1 and continued fraction above.
fn q_gamma(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let scale = (a * x.ln() - x - ln_gamma(a)).exp();
    if x < a + 1.0 {
        let (mut term, mut sum, mut n) = (1.0 / a, 1.0 / a, a);
        for _ in 0..10_000_000 {
            n += 1.0;
            term *= x / n;
            sum += term;
            if term < sum * 1e-16 {
                break;
            }
        }
        (1.0 - sum * scale).max(0.0)
    } else {
        let small = 1e-300;
        let mut b = x + 1.0 - a;
        let (mut c, mut d) = (1.0 / small, 1.0 / b);
        let mut h = d;
        for i in 1..10_000_000 {
            let an = -f64::from(i) * (f64::from(i) - a);
            b += 2.0;
            d = an * d + b;
            d = if d.abs() < small { small } else { d };
            c = b + an / c;
            c = if c.abs() < small { small } else { c };
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < 1e-16 {
                break;
            }
        }
        h * scale
    }
}

struct Report {
    tests: u32,
    smallest: f64,
}

impl Report {
    fn line(&mut self, name: &str, stat: &str, p: f64) {
        self.tests += 1;
        self.smallest = self.smallest.min(p);
        println!("{name:<38} {stat:<34} p = {p:.6}");
    }

    fn chi2(&mut self, name: &str, stat: f64, df: u64) {
        let p = q_gamma(df as f64 / 2.0, stat / 2.0);
        self.line(name, &format!("chi2 {stat:.2}, df {df}"), p);
    }

    fn z(&mut self, name: &str, z: f64) {
        self.line(name, &format!("z {z:+.3}"), q_gamma(0.5, z * z / 2.0));
    }
}

/// Probability that a uniform random permutation of N has exactly k fixed points, for k below BINS - 1, then the rest.
fn fixed_point_odds() -> [f64; BINS] {
    let mut odds = [0.0; BINS];
    let mut k_fact = 1.0;
    for (k, p) in odds.iter_mut().enumerate().take(BINS - 1) {
        if k > 0 {
            k_fact *= k as f64;
        }
        let (mut sum, mut term) = (0.0, 1.0);
        for j in 0..=(N - k) {
            if j > 0 {
                term *= -1.0 / j as f64;
            }
            sum += term;
        }
        *p = sum / k_fact;
    }
    odds[BINS - 1] = 1.0 - odds[..BINS - 1].iter().sum::<f64>();
    odds
}

fn chi2_of(counts: &[u64], odds: &[f64], total: u64) -> f64 {
    counts
        .iter()
        .zip(odds)
        .map(|(&o, &p)| {
            let e = p * total as f64;
            (o as f64 - e).powi(2) / e
        })
        .sum()
}

fn arg(args: &[String], name: &str, default: u64) -> u64 {
    args.iter()
        .position(|a| a == name)
        .map_or(default, |i| args[i + 1].parse().unwrap_or_else(|_| panic!("{name} needs a number")))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let from = arg(&args, "--from", 0);
    let count = arg(&args, "--count", 1_000_000);
    let stride = arg(&args, "--stride", 1);
    let delta_count = arg(&args, "--delta-count", 1_000_000_000).min(count);
    let default_threads = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let threads = arg(&args, "--threads", default_threads);
    let naive = args.iter().any(|a| a == "--naive");

    let next = AtomicU64::new(0);
    let start = std::time::Instant::now();
    let tally = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut t = Tally::new();
                    loop {
                        let begin = next.fetch_add(CHUNK, Ordering::Relaxed);
                        if begin >= count {
                            return t;
                        }
                        for i in begin..(begin + CHUNK).min(count) {
                            t.count(from + i * stride, naive, i < delta_count);
                        }
                    }
                })
            })
            .collect();
        let mut total = Tally::new();
        for w in workers {
            total.add(&w.join().expect("a worker panicked"));
        }
        total
    });

    let shuffle = if naive { "the naive shuffle" } else { "Deal::from_seed" };
    println!(
        "{count} deals by {shuffle}: seeds from {from}, stride {stride}; seed differences on the first {delta_count}"
    );
    let total = tally.deals as f64;
    let mut r = Report { tests: 0, smallest: 1.0 };

    // A deal is a permutation matrix, so these cells are not multinomial: on the (N-1)^2 free directions the
    // covariance of a cell count is deals/(N-1), against deals/N for a multinomial. Hence the factor (N-1)/N.
    let e = total / N as f64;
    let stat: f64 = tally.card_at.iter().map(|&o| (o as f64 - e).powi(2) / e).sum();
    r.chi2("card by position", stat * (N as f64 - 1.0) / N as f64, ((N - 1) * (N - 1)) as u64);

    for (k, &(a, b)) in PAIRS.iter().enumerate() {
        let e = total / (N * (N - 1)) as f64;
        let cells = &tally.pairs[k * N * N..(k + 1) * N * N];
        let stat: f64 = cells
            .iter()
            .enumerate()
            .filter(|(i, _)| i / N != i % N)
            .map(|(_, &o)| (o as f64 - e).powi(2) / e)
            .sum();
        r.chi2(&format!("cards at positions {a} and {b}"), stat, (N * (N - 1) - 1) as u64);
    }

    let e = total / (N * (N - 1) * (N - 2)) as f64;
    let distinct = |i: usize| {
        let (a, b, c) = (i / (N * N), i / N % N, i % N);
        a != b && a != c && b != c
    };
    let stat: f64 = tally
        .first_three
        .iter()
        .enumerate()
        .filter(|&(i, _)| distinct(i))
        .map(|(_, &o)| (o as f64 - e).powi(2) / e)
        .sum();
    r.chi2("first three cards", stat, (N * (N - 1) * (N - 2) - 1) as u64);

    let odds = fixed_point_odds();
    r.chi2("fixed points", chi2_of(&tally.fixed, &odds, tally.deals), (BINS - 1) as u64);
    r.chi2("parity", chi2_of(&[tally.even, tally.deals - tally.even], &[0.5, 0.5], tally.deals), 1);

    let n = N as f64;
    let (mean, var) = (n * (n - 1.0) / 4.0, n * (n - 1.0) * (2.0 * n + 5.0) / 72.0);
    r.z("inversions, mean", (tally.inversions as f64 / total - mean) / (var / total).sqrt());
    let sample_z = |sum: u64, sum_sq: u64, mean: f64| {
        let m = sum as f64 / total;
        (m - mean) / ((sum_sq as f64 / total - m * m) / total).sqrt()
    };
    r.z("opening slides, mean", sample_z(tally.slides, tally.slides_sq, 15.0));
    r.z("opening leaps, mean", sample_z(tally.leaps, tally.leaps_sq, 49.0 * 15.0 / 51.0));

    for (bins, delta) in tally.agree.iter().zip(DELTAS) {
        let stat = chi2_of(bins, &odds, tally.delta_deals);
        r.chi2(&format!("same cards as seed + {delta}"), stat, (BINS - 1) as u64);
    }

    let threshold = 0.05 / f64::from(r.tests);
    let verdict = if r.smallest < threshold { "FAIL" } else { "pass" };
    println!(
        "{} tests, smallest p = {:.6}, Bonferroni 5% line {threshold:.6}: {verdict}",
        r.tests, r.smallest
    );
    eprintln!("{:.1} s on {threads} threads", start.elapsed().as_secs_f64());
}
