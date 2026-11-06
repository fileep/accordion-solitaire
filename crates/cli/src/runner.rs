//! Runs one solver configuration over many seeded deals on worker threads (PLAN.md Phase 5 track B, Phase 8).
//! Workers take the next deal from a shared counter, so a slow deal never holds the others up; each worker solves,
//! verifies and serializes its own deals and keeps one search (and table) for the whole run. The calling thread only
//! receives finished deals, in completion order.
//!
//! With a GPU, a feeder thread takes batches of deals from the same counter for GPU ladder attempts and only
//! launches and collects. Workers check the GPU's lines before taking new deals, so nothing waits on them: each line
//! is replayed by the reference rules, and a deal the GPU didn't win goes through the CPU solver from the start, like
//! any other.

use accordion_core::fast::Position;
use accordion_core::game::replay_tops;
use accordion_core::notation::encode_compact;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Move, Outcome, SolveRecord};
use accordion_gpu::{BatchResult, Gpu, Settings, deal_codes};
use accordion_solver::{Config, Endgame, Report, Search, Stage, Stats, Verdict, solve};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The deals of a run, by position: a range of seeds (nothing to hold, for runs of billions) or a list.
#[derive(Clone, Copy)]
pub enum Seeds<'a> {
    Range { from: u64, count: usize },
    List(&'a [u64]),
}

impl Seeds<'_> {
    pub fn count(&self) -> usize {
        match *self {
            Seeds::Range { count, .. } => count,
            Seeds::List(seeds) => seeds.len(),
        }
    }

    /// The seed at position `i`.
    pub fn seed(&self, i: usize) -> u64 {
        match *self {
            Seeds::Range { from, .. } => from + i as u64,
            Seeds::List(seeds) => seeds[i],
        }
    }
}

pub struct Options {
    /// CPU worker threads (a GPU feeder, if any, is one more).
    pub threads: usize,
    /// Dead-position table per thread, as log2 of 64-byte buckets.
    pub table_log2: u32,
    pub endgame: Option<Arc<Endgame>>,
    /// Serialize each deal's solve record (compact moves) for output.
    pub records: bool,
    /// Hand on each deal the GPU wins as [`Finished::Deal`], even with no record of it wanted.
    pub each_gpu_win: bool,
    /// GPU ladder attempts in front of the CPU solver (`--gpu`).
    pub gpu: Option<Arc<Gpu>>,
}

/// What [`run`] hands to its sink.
pub enum Finished {
    /// A deal, with all that is known of it.
    Deal(Solved),
    /// This many more deals won on the GPU, all verified, when no records of them are wanted.
    GpuWins(u64),
}

/// One finished deal.
pub struct Solved {
    /// Position in the seed list.
    pub index: usize,
    pub seed: u64,
    pub report: Report,
    /// Time to solve on one thread, in microseconds; 0 for a deal won on the GPU, which works in batches.
    pub us: u64,
    /// The reference engine's verdict on the line (the win, or the best line of a deal not won).
    pub verified: Result<(), String>,
    /// The solve record as one JSON line, when asked for.
    pub record: Option<String>,
}

/// What the GPU did in a run.
#[derive(Default)]
pub struct GpuTotals {
    pub launches: u64,
    /// Deals it was given, and how many of them it won (every line verified).
    pub deals: u64,
    pub won: u64,
    /// Lines that failed verification: a bug. Each of those deals was solved on the CPU instead.
    pub failed: u64,
    pub attempts: u64,
    pub positions: u64,
    /// Why the GPU stopped early, if it did; the CPU solved the deals it had in hand, and the rest.
    pub error: Option<String>,
    /// CPU worker thread time, in microseconds, by what it went to.
    pub cpu: CpuTime,
}

/// CPU worker thread time in a run with the GPU, in microseconds (all workers added up).
#[derive(Default, Clone, Copy)]
pub struct CpuTime {
    /// Verifying the GPU's winning lines.
    pub verifying: u64,
    /// Solving the deals the GPU didn't win.
    pub misses: u64,
    /// Solving deals the workers took for themselves.
    pub own: u64,
    /// Waiting for the GPU's last deals, once there were none left to take.
    pub waiting: u64,
    /// Waiting to hand a result on (the main thread was behind).
    pub handing_on: u64,
    /// Wall time times worker threads: everything else is the difference.
    pub total: u64,
}

/// How a run stands, for [`run`]'s `progress`.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Deals finished.
    pub done: usize,
    /// Deals the GPU has won, and positions it has generated (0 without one).
    pub gpu_won: u64,
    pub gpu_positions: u64,
    /// GPU results waiting for a CPU worker to verify them, or to solve the deals the GPU didn't win.
    pub backlog: usize,
}

/// How often [`run`] reports progress.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);
/// The GPU waits while this many of its batches wait for the CPU threads.
const MAX_BACKLOG_BATCHES: usize = 8;
/// GPU results a CPU thread takes at a time, and hands on the outcome of at once.
const RUN: usize = 256;
/// One deal in this many has its line replayed by the reference engine as well as by its rules on the top cards.
const FULL_REPLAY_EVERY: usize = 256;

/// A launch's results, which the CPU workers take a run at a time. Nothing is done per deal until a worker gets to it,
/// so a deal costs no lock, allocation or shared counter of its own: past a few hundred thousand deals a second, all
/// the more with the CPU slowed by a hot laptop, those held the GPU back.
struct GpuBatch {
    indices: Vec<usize>,
    result: BatchResult,
    /// The next result to take.
    next: AtomicUsize,
}

/// The next run of results from the oldest launch with any left.
fn take_run(queue: &Mutex<VecDeque<Arc<GpuBatch>>>) -> Option<(Arc<GpuBatch>, Range<usize>)> {
    let mut queue = queue.lock().expect("no panics");
    loop {
        let batch = queue.front()?;
        let first = batch.next.fetch_add(RUN, Ordering::Relaxed);
        if first < batch.indices.len() {
            let last = (first + RUN).min(batch.indices.len());
            return Some((Arc::clone(batch), first..last));
        }
        queue.pop_front();
    }
}

/// Every thread on this machine.
pub fn all_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// How many piles the endgame tables in `dir` cover (the largest k with files for 1..=k), 0 if none.
pub fn available_endgame(dir: &Path) -> usize {
    (1..=accordion_solver::endgame::MAX_PILES)
        .take_while(|k| dir.join(format!("endgame-{k}.bin")).exists())
        .last()
        .unwrap_or(0)
}

/// Applies `--endgame` to `config`: a pile count, `off`, or `auto` (the largest tables in `dir` up to `cap` piles,
/// unless the config already chose).
pub fn choose_endgame(config: &mut Config, flag: &str, dir: &Path, cap: usize) -> Result<(), String> {
    match flag {
        "auto" if config.search.endgame_piles == 0 => {
            config.search.endgame_piles = available_endgame(dir).min(cap);
            if config.search.endgame_piles == 0 {
                eprintln!(
                    "hint: no endgame tables in {}; `acc endgame build --piles 11` builds them (about 7 minutes, 1.2 GB) and makes solving about 1.2-1.3x faster",
                    dir.display()
                );
            }
        }
        "auto" => {}
        "off" => config.search.endgame_piles = 0,
        n => {
            config.search.endgame_piles =
                n.parse().map_err(|_| format!("--endgame: {n:?} (a pile count, auto or off)"))?
        }
    }
    Ok(())
}

/// The endgame tables for up to `piles` piles from `dir` (`acc endgame build`), or none when `piles` is 0.
pub fn load_endgame(dir: &Path, piles: usize) -> Result<Option<Arc<Endgame>>, String> {
    if piles == 0 {
        return Ok(None);
    }
    let start = Instant::now();
    let tables = Endgame::load(dir, piles)
        .map_err(|e| format!("{e}\n(build them with `acc endgame build --piles {piles}`)"))?;
    eprintln!(
        "endgame tables for up to {piles} piles loaded ({:.0} MB, {:.1} s)",
        tables.bytes() as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
    Ok(Some(Arc::new(tables)))
}

/// Opens the GPU for `--gpu`: `on` (an error without one), `auto` (CPU only, with a note, without one) or `off`.
pub fn open_gpu(flag: &str, settings: Settings) -> Result<Option<Arc<Gpu>>, String> {
    let required = match flag {
        "off" => return Ok(None),
        "on" => true,
        "auto" => false,
        f => return Err(format!("--gpu: {f:?} (auto, on or off)")),
    };
    let start = Instant::now();
    match Gpu::new(settings) {
        Ok(gpu) => {
            eprintln!(
                "GPU: {} ({} multiprocessors), kernel compiled in {:.1} s",
                gpu.name,
                gpu.sms,
                start.elapsed().as_secs_f64()
            );
            Ok(Some(Arc::new(gpu)))
        }
        Err(e) if required => Err(format!("--gpu on: {e}")),
        Err(e) => {
            eprintln!("no GPU ({e}); solving on the CPU only");
            Ok(None)
        }
    }
}

/// Solves every deal in `seeds` with `config` (move-order seed = deal seed) and hands each result to `sink`.
/// `progress` is called about once a second, results or not, and once at the end. Stops early if `sink` fails.
/// Returns what the GPU did, if there was one.
pub fn run(
    seeds: Seeds,
    config: &Config,
    opts: &Options,
    mut sink: impl FnMut(Finished) -> Result<(), String>,
    mut progress: impl FnMut(Progress),
) -> Result<Option<GpuTotals>, String> {
    let next = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let from_gpu: Mutex<VecDeque<Arc<GpuBatch>>> = Mutex::new(VecDeque::new());
    let feeding = AtomicBool::new(opts.gpu.is_some());
    let (gpu_won, gpu_failed, gpu_positions) = (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0));
    let cpu_time = Mutex::new(CpuTime::default());
    let started = Instant::now();
    let (tx, rx) = mpsc::sync_channel::<(u64, Vec<Solved>)>(64);
    std::thread::scope(|scope| {
        let feeder = opts.gpu.as_deref().map(|gpu| {
            let (next, abort, from_gpu, feeding, positions) =
                (&next, &abort, &from_gpu, &feeding, &gpu_positions);
            scope.spawn(move || {
                let totals = feed(gpu, seeds, next, abort, from_gpu, positions);
                feeding.store(false, Ordering::Release);
                totals
            })
        });
        for _ in 0..opts.threads.max(1) {
            let tx = tx.clone();
            let (next, abort, from_gpu, feeding) = (&next, &abort, &from_gpu, &feeding);
            let (gpu_won, gpu_failed, cpu_time) = (&gpu_won, &gpu_failed, &cpu_time);
            scope.spawn(move || {
                let mut search = Search::with_options(opts.table_log2, config.search);
                search.set_endgame(opts.endgame.clone());
                let mut time = CpuTime::default();
                let us = |t: Instant| t.elapsed().as_micros() as u64;
                let mut moves = Vec::new();
                while !abort.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    let mut wins = 0u64;
                    let mut finished: Vec<Solved> = Vec::new();
                    if let Some((batch, run)) = take_run(from_gpu) {
                        let mut misses = 0;
                        for i in run {
                            let index = batch.indices[i];
                            let seed = seeds.seed(index);
                            if batch.result.is_won(i) {
                                let verified = if opts.records || opts.each_gpu_win {
                                    gpu_win_recorded(&batch.result, i, index, seed, opts.records)
                                        .map(|solved| finished.push(solved))
                                } else {
                                    gpu_line_verifies(&batch.result, i, index, seed, &mut moves).map(|()| wins += 1)
                                };
                                match verified {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        gpu_failed.fetch_add(1, Ordering::Relaxed);
                                        eprintln!(
                                            "\nseed {seed}: GPU line FAILED verification ({e}); solving it on the CPU"
                                        );
                                    }
                                }
                            }
                            let miss = Instant::now();
                            finished.push(solve_one(index, seed, config, &mut search, opts.records));
                            misses += us(miss);
                        }
                        gpu_won.fetch_add(
                            wins + finished.iter().filter(|s| s.report.stage == Stage::Gpu).count() as u64,
                            Ordering::Relaxed,
                        );
                        time.verifying += us(t).saturating_sub(misses);
                        time.misses += misses;
                    } else if feeding.load(Ordering::Acquire) {
                        // While the GPU is feeding, no deals of their own: a few thousand a second at most, for heat
                        // that slows the GPU (the laptop's CPU and GPU share a power limit), by 5 to 10%.
                        std::thread::sleep(Duration::from_micros(200));
                        time.waiting += us(t);
                        continue;
                    } else {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= seeds.count() {
                            if from_gpu.lock().expect("no panics").is_empty() {
                                break;
                            }
                            continue;
                        }
                        finished.push(solve_one(index, seeds.seed(index), config, &mut search, opts.records));
                        time.own += us(t);
                    }
                    let t = Instant::now();
                    if tx.send((wins, finished)).is_err() {
                        break;
                    }
                    time.handing_on += us(t);
                }
                let mut all = cpu_time.lock().expect("no panics");
                all.verifying += time.verifying;
                all.misses += time.misses;
                all.own += time.own;
                all.waiting += time.waiting;
                all.handing_on += time.handing_on;
            });
        }
        drop(tx);
        let mut done = 0;
        let mut result = Ok(());
        let stand = |done| Progress {
            done,
            gpu_won: gpu_won.load(Ordering::Relaxed),
            gpu_positions: gpu_positions.load(Ordering::Relaxed),
            backlog: from_gpu
                .lock()
                .expect("no panics")
                .iter()
                .map(|b| b.indices.len().saturating_sub(b.next.load(Ordering::Relaxed)))
                .sum(),
        };
        let mut last = Instant::now();
        loop {
            // Wake for the next report even when no deal finishes (the last hard deals, or a stalled GPU).
            match rx.recv_timeout(PROGRESS_EVERY.saturating_sub(last.elapsed())) {
                Ok((wins, solved)) => {
                    done += wins as usize + solved.len();
                    let wins = (wins > 0).then_some(Finished::GpuWins(wins));
                    for finished in wins.into_iter().chain(solved.into_iter().map(Finished::Deal)) {
                        if result.is_ok() {
                            result = sink(finished);
                            if result.is_err() {
                                abort.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                progress(stand(done));
            }
        }
        progress(stand(done));
        let mut cpu = *cpu_time.lock().expect("no panics");
        cpu.total = started.elapsed().as_micros() as u64 * opts.threads.max(1) as u64;
        let totals = feeder.map(|h| GpuTotals {
            won: gpu_won.load(Ordering::Relaxed),
            failed: gpu_failed.load(Ordering::Relaxed),
            cpu,
            ..h.join().expect("the feeder does not panic")
        });
        result.map(|()| totals)
    })
}

/// The GPU feeder: batches of deals from the shared counter until it runs out (or the run is aborted), results
/// into `queue`. If the GPU fails, the deals it had in hand go into the queue as not won.
fn feed(
    gpu: &Gpu,
    seeds: Seeds,
    next: &AtomicUsize,
    abort: &AtomicBool,
    queue: &Mutex<VecDeque<Arc<GpuBatch>>>,
    positions: &AtomicU64,
) -> GpuTotals {
    let mut totals = GpuTotals::default();
    let batch = gpu.settings.batch;
    let result = gpu.feed(
        |&index: &usize| deal_codes(&Deal::from_seed(seeds.seed(index))),
        || {
            // CPU threads that fall behind the GPU (a hot laptop) hold it back, so what waits for them stays small.
            while queue.lock().expect("no panics").len() >= MAX_BACKLOG_BATCHES {
                if abort.load(Ordering::Relaxed) {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if abort.load(Ordering::Relaxed) {
                return None;
            }
            let first = next.fetch_add(batch, Ordering::Relaxed);
            (first < seeds.count()).then(|| (first..(first + batch).min(seeds.count())).collect())
        },
        |indices, result| {
            totals.launches += 1;
            totals.deals += indices.len() as u64;
            totals.attempts += result.total_tries();
            let generated = result.total_nodes();
            totals.positions += generated;
            positions.fetch_add(generated, Ordering::Relaxed);
            queue.lock().expect("no panics").push_back(Arc::new(GpuBatch { indices, result, next: AtomicUsize::new(0) }));
        },
    );
    if let Err((e, lost)) = result {
        eprintln!(
            "\nGPU stopped: {e}\n(the CPU solves the {} deals it had in hand, and the rest)",
            lost.len()
        );
        let result = BatchResult::none(lost.len());
        queue.lock().expect("no panics").push_back(Arc::new(GpuBatch { indices: lost, result, next: AtomicUsize::new(0) }));
        totals.error = Some(e);
    }
    totals
}

/// Verifies the line of the deal the GPU won at `i` of `result`, without allocating (`moves`: a buffer, used over and
/// over).
fn gpu_line_verifies(
    result: &BatchResult,
    i: usize,
    index: usize,
    seed: u64,
    moves: &mut Vec<Move>,
) -> Result<(), String> {
    let line = result.line(i).expect("it is won");
    moves.clear();
    moves.extend(line.iter().map(|&b| Move::from_byte(b)));
    replay(index, &Deal::from_seed(seed), moves, true)
}

/// A deal the GPU won with all that is known of it, verified, or the error if its line fails.
fn gpu_win_recorded(result: &BatchResult, i: usize, index: usize, seed: u64, records: bool) -> Result<Solved, String> {
    let deal = Deal::from_seed(seed);
    let line = result.line(i).expect("it is won");
    let report = Report {
        verdict: Verdict::Win(line.iter().map(|&b| Move::from_byte(b)).collect()),
        nodes: result.nodes(i),
        attempts: result.tries(i),
        stage: Stage::Gpu,
        stats: Stats::default(),
        best_piles: 1,
        best_line: Vec::new(),
    };
    let solved = finish(index, seed, &deal, report, 0, records);
    match &solved.verified {
        Ok(()) => Ok(solved),
        Err(e) => Err(e.clone()),
    }
}

fn solve_one(index: usize, seed: u64, config: &Config, search: &mut Search, records: bool) -> Solved {
    let deal = Deal::from_seed(seed);
    search.clear_table();
    let start = Instant::now();
    let report = solve(&Position::from_deal(&deal), &Config { seed, ..*config }, search, None);
    let us = start.elapsed().as_micros() as u64;
    finish(index, seed, &deal, report, us, records)
}

/// Verifies a deal's line and serializes its record. Without records, the line is replayed directly by the reference
/// rules ([`replay`]): the same check `verify` makes of a record with the reference engine, without writing the
/// record's card notation and parsing it back, or allocating its piles, which took most of the CPU workers' time next
/// to the GPU.
fn finish(index: usize, seed: u64, deal: &Deal, report: Report, us: u64, records: bool) -> Solved {
    let (outcome, line) = match &report.verdict {
        Verdict::Win(line) => (Outcome::Win, line),
        _ => (Outcome::Unknown, &report.best_line),
    };
    // Nothing counts as a win until the reference engine has replayed it.
    if !records {
        let verified = replay(index, deal, line, outcome == Outcome::Win);
        return Solved { index, seed, report, us, verified, record: None };
    }
    let (verified, record) = match SolveRecord::new(deal, Some(seed), outcome, line) {
        Err(e) => (Err(e.to_string()), None),
        Ok(mut record) => {
            record.moves_b64 = Some(encode_compact(line));
            record.moves.clear();
            let verified = verify(&record).map(|_| ()).map_err(|e| e.to_string());
            let text = records.then(|| {
                record.stats = json!({
                    "nodes": report.nodes,
                    "attempts": report.attempts,
                    "stage": report.stage.to_string(),
                    "us": us,
                })
                .as_object()
                .cloned();
                record.to_json_line()
            });
            (verified, text)
        }
    };
    Solved { index, seed, report, us, verified, record }
}

/// The reference rules' replay of `line` from `deal` ([`replay_tops`]): every move legal, and one pile at the end when
/// `won` is claimed (after exactly 51 moves, since each move takes one pile away). Every [`FULL_REPLAY_EVERY`]th deal
/// also goes through the reference engine itself, which has to agree.
fn replay(index: usize, deal: &Deal, line: &[Move], won: bool) -> Result<(), String> {
    let piles = replay_tops(deal, line).map_err(|(i, e)| format!("move {}: {e}", i + 1))?;
    if index.is_multiple_of(FULL_REPLAY_EVERY) {
        let mut game = Game::new(deal);
        for (i, &mv) in line.iter().enumerate() {
            game.apply(mv).map_err(|e| format!("move {}: the reference engine: {e}", i + 1))?;
        }
        if game.pile_count() != piles {
            return Err(format!(
                "{} piles left by the reference engine, {piles} by the replay",
                game.pile_count()
            ));
        }
    }
    if won && piles != 1 {
        return Err(format!("the line ends with {piles} piles, not one"));
    }
    Ok(())
}

/// A 64-bit fingerprint of the deal with suits and ranks renamed ([`Deal::relabelled`]): deals that play alike
/// share it, and different ones almost never do.
pub fn fingerprint(deal: &Deal) -> u64 {
    let mix = |mut z: u64| {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    deal.relabelled().cards().chunks(8).fold(0x9E37_79B9_7F4A_7C15, |h, chunk| {
        mix(h ^ chunk.iter().fold(0u64, |w, c| w << 8 | u64::from(c.code())))
    })
}

/// The pairs of seeds whose deals are the same game, even with suits and ranks renamed: deals whose fingerprints
/// (`print`: [`fingerprint`], but for tests) agree, compared card by card. Holds one fingerprint per deal, sorted in
/// place (8 GB for a billion deals); the few seeds behind a repeated one are found by fingerprinting again.
pub fn same_games(seeds: Seeds, print: impl Fn(u64) -> u64 + Sync) -> Vec<(u64, u64)> {
    let mut prints: Vec<u64> = (0..seeds.count()).into_par_iter().map(|i| print(seeds.seed(i))).collect();
    prints.par_sort_unstable();
    let repeated: HashSet<u64> = prints.windows(2).filter(|w| w[0] == w[1]).map(|w| w[0]).collect();
    drop(prints);
    if repeated.is_empty() {
        return Vec::new();
    }
    let members: Vec<(u64, u64)> = (0..seeds.count())
        .into_par_iter()
        .filter_map(|i| {
            let seed = seeds.seed(i);
            let f = print(seed);
            repeated.contains(&f).then_some((f, seed))
        })
        .collect();
    let mut groups: HashMap<u64, Vec<u64>> = HashMap::new();
    for (f, seed) in members {
        groups.entry(f).or_default().push(seed);
    }
    let mut pairs = Vec::new();
    for group in groups.values() {
        for (i, &a) in group.iter().enumerate() {
            for &b in &group[i + 1..] {
                if Deal::from_seed(a).relabelled() == Deal::from_seed(b).relabelled() {
                    pairs.push((a.min(b), a.max(b)));
                }
            }
        }
    }
    pairs.sort_unstable();
    pairs
}

/// Wilson score interval (95%) for `k` successes in `n` trials.
pub fn wilson(k: u64, n: u64) -> (f64, f64) {
    let (k, n, z) = (k as f64, n as f64, 1.959_964);
    let p = k / n;
    let d = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / d;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / d;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// The `q` quantile of sorted values (nearest rank).
pub fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_interval_matches_known_values() {
        let (lo, hi) = wilson(0, 10);
        assert!(lo == 0.0 && (hi - 0.2775).abs() < 1e-3, "{hi}");
        let (lo, hi) = wilson(100_000, 100_000);
        assert!((lo - 0.999_962).abs() < 1e-5 && (hi - 1.0).abs() < 1e-12, "{lo} {hi}");
    }

    fn check_run(gpu: Option<Arc<Gpu>>, count: u64, records: bool) {
        let seeds: Vec<u64> = (500..500 + count).collect();
        let on_gpu = gpu.is_some();
        let opts = Options { threads: 4, table_log2: 12, endgame: None, records, each_gpu_win: false, gpu };
        let mut seen = vec![0u32; seeds.len()];
        let mut stages: HashMap<String, u32> = HashMap::new();
        let mut counted_wins = 0u64;
        let totals = run(
            Seeds::List(&seeds),
            &Config::default(),
            &opts,
            |finished| {
                let s = match finished {
                    Finished::Deal(s) => s,
                    Finished::GpuWins(n) => {
                        assert!(!records, "records were asked for");
                        counted_wins += n;
                        return Ok(());
                    }
                };
                seen[s.index] += 1;
                assert_eq!(s.seed, seeds[s.index]);
                assert!(s.verified.is_ok() && matches!(s.report.verdict, Verdict::Win(_)));
                assert_eq!(s.record.is_some(), records);
                *stages.entry(s.report.stage.to_string()).or_default() += 1;
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        // Without records, the GPU's wins come back as a count, and every other deal once.
        assert!(seen.iter().all(|&n| n <= 1));
        assert_eq!(seen.iter().map(|&n| u64::from(n)).sum::<u64>() + counted_wins, count);
        match totals {
            Some(t) => {
                assert!(on_gpu && t.error.is_none() && t.failed == 0);
                assert_eq!(stages.get("gpu").map_or(0, |&n| u64::from(n)) + counted_wins, t.won);
                assert!(t.won > 0 && t.won <= t.deals, "{} of {}", t.won, t.deals);
            }
            None => assert!(!on_gpu && !stages.contains_key("gpu") && counted_wins == 0),
        }
    }

    #[test]
    fn every_deal_comes_back_once_and_verified() {
        check_run(None, 60, true);
    }

    #[test]
    fn with_the_gpu_too() {
        match open_gpu("auto", Settings { batch: 512, ..Settings::default() }).unwrap() {
            Some(gpu) => {
                check_run(Some(Arc::clone(&gpu)), 3_000, true);
                check_run(Some(gpu), 3_000, false);
            }
            None => eprintln!("skipped: no GPU"),
        }
    }

    #[test]
    fn the_same_game_is_found_under_new_names() {
        let print = |s: u64| fingerprint(&Deal::from_seed(s));
        let seeds = [1, 2, 3, 4];
        assert!(same_games(Seeds::List(&seeds), print).is_empty());
        // Seed 4 given seed 2's fingerprint: compared card by card, they still differ.
        assert!(same_games(Seeds::List(&seeds), |s| print(if s == 4 { 2 } else { s })).is_empty());
        // The same seed twice is the same game.
        assert_eq!(same_games(Seeds::List(&[1, 2, 1]), print), vec![(1, 1)]);
        assert!(same_games(Seeds::Range { from: 0, count: 10_000 }, print).is_empty());
    }

    #[test]
    fn the_lean_replay_agrees_with_the_record_check() {
        // A solved deal's line passes both checks, with and without records; spoiled lines fail both.
        let mut search = Search::with_options(14, Config::default().search);
        let seed = 7;
        let deal = Deal::from_seed(seed);
        let report =
            solve(&Position::from_deal(&deal), &Config { seed, ..Config::default() }, &mut search, None);
        let Verdict::Win(line) = report.verdict.clone() else { panic!("seed {seed} not won") };
        let spoil = |line: Vec<Move>| Report { verdict: Verdict::Win(line), ..report.clone() };
        let mut illegal = line.clone();
        illegal[10] = Move::leap(1);
        let short = line[..50].to_vec();
        for records in [false, true] {
            assert!(finish(0, seed, &deal, report.clone(), 0, records).verified.is_ok(), "records {records}");
            for (name, bad) in [("illegal move", illegal.clone()), ("50 moves", short.clone())] {
                let solved = finish(0, seed, &deal, spoil(bad), 0, records);
                assert!(solved.verified.is_err(), "{name} passed (records {records})");
            }
        }
    }
}
