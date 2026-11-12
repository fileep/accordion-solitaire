//! Runs one solver configuration over many seeded deals on worker threads (PLAN.md Phase 5 track B, Phase 8).
//! Workers take the next deal from a shared counter, so a slow deal never holds the others up; each worker solves,
//! verifies and serializes its own deals and keeps one search (and table) for the whole run. The calling thread only
//! receives finished deals, and hands them on in seed order, so where a run stands is one number: every deal before it
//! is done. That is what lets a run pause and carry on ([`Options::from`], [`Options::pause`]).
//!
//! With a GPU, a feeder thread takes batches of deals from the same counter for GPU ladder attempts and only
//! launches and collects. Workers check the GPU's lines before taking new deals, so nothing waits on them: each line
//! is replayed by the reference rules, and a deal the GPU didn't win goes through the CPU solver from the start, like
//! any other.

use crate::games::Sieve;
use accordion_core::fast::Position;
use accordion_core::game::{replay_codes, replay_tops};
use accordion_core::notation::encode_compact;
use accordion_core::quick;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Move, Outcome, SolveRecord};
use accordion_gpu::{BatchResult, Gpu, Settings, deal_codes};
use accordion_solver::{Config, Endgame, Report, Search, Stage, Stats, Verdict, solve};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ops::{AddAssign, Range};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The deals of a run, by position: a range of seeds (nothing to hold, for runs of billions) or a list.
#[derive(Clone, Copy)]
pub enum Seeds<'a> {
    Range {
        from: u64,
        count: usize,
    },
    List(&'a [u64]),
    /// A range in which every `every`th deal is the deal before it: repeats, to test the check for them with.
    Repeating {
        from: u64,
        count: usize,
        every: usize,
    },
}

impl Seeds<'_> {
    pub fn count(&self) -> usize {
        match *self {
            Seeds::Range { count, .. } | Seeds::Repeating { count, .. } => count,
            Seeds::List(seeds) => seeds.len(),
        }
    }

    /// The seed at position `i`.
    pub fn seed(&self, i: usize) -> u64 {
        match *self {
            Seeds::Range { from, .. } => from + i as u64,
            Seeds::List(seeds) => seeds[i],
            Seeds::Repeating { from, every, .. } => from + i as u64 - u64::from(i % every == every - 1),
        }
    }
}

#[derive(Default)]
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
    /// Where an interrupted run had got to: its deals are not solved again, and its totals carry on.
    pub from: Standing,
    /// Once this is set, no more deals are started, those in hand are finished, and the run returns short of its count.
    pub pause: Option<&'static AtomicBool>,
    /// Hand on [`Finished::Checkpoint`] this often.
    pub checkpoint_every: Option<Duration>,
    /// If the GPU fails, pause rather than solve the rest on the CPU, which at these counts would take a month; the GPU
    /// may well work again for a new process.
    pub pause_if_gpu_fails: bool,
    /// Hand on [`Finished::Sampled`] for the deals in this sample.
    pub sieve: Option<Sieve>,
    /// Make the GPU fail after this many launches, with the next batch in its hands.
    #[cfg(test)]
    pub fail_gpu_after: Option<u64>,
}

/// What [`run`] hands to its sink.
pub enum Finished {
    /// A deal, with all that is known of it.
    Deal(Solved),
    /// This many more deals won on the GPU, all verified, when no records of them are wanted.
    GpuWins(u64),
    /// The log's records ([`crate::games`]) of the deals about to be handed on, or counted in [`Finished::GpuWins`].
    Sampled(Vec<u8>),
    /// Where the run stands, with all that the sink has been handed so far.
    Checkpoint(Standing),
}

/// Where a run stands: every deal before position `done` has gone to the sink, and `gpu` is what the GPU did for them.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Standing {
    pub done: usize,
    pub gpu: GpuTotals,
}

/// How a run ended.
pub struct Ran {
    /// Every deal before this position is done: all of them, unless the run paused.
    pub done: usize,
    /// What the GPU did, if there was one.
    pub gpu: Option<GpuTotals>,
}

/// One finished deal.
pub struct Solved {
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
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct GpuTotals {
    pub launches: u64,
    /// Deals it was given, and how many of them it won (every line verified).
    pub deals: u64,
    pub won: u64,
    /// Lines that failed verification: a bug. Each of those deals was solved on the CPU instead.
    pub failed: u64,
    pub attempts: u64,
    pub positions: u64,
    /// Why the GPU stopped early, if it did. Unless the run paused for it, the CPU solved the deals it had in hand, and
    /// the rest.
    #[serde(skip)]
    pub error: Option<String>,
    /// CPU worker thread time, in microseconds, by what it went to.
    pub cpu: CpuTime,
}

impl GpuTotals {
    /// Adds what the GPU did for some more deals.
    fn add(&mut self, more: &GpuTotals) {
        self.deals += more.deals;
        self.won += more.won;
        self.failed += more.failed;
        self.attempts += more.attempts;
        self.positions += more.positions;
    }
}

/// CPU worker thread time in a run with the GPU, in microseconds (all workers added up).
#[derive(Default, Clone, Copy, Serialize, Deserialize)]
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

impl AddAssign for CpuTime {
    fn add_assign(&mut self, more: CpuTime) {
        self.verifying += more.verifying;
        self.misses += more.misses;
        self.own += more.own;
        self.waiting += more.waiting;
        self.handing_on += more.handing_on;
        self.total += more.total;
    }
}

/// How a run stands, for [`run`]'s `progress`.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Deals finished, including any waiting for an earlier one to be handed on.
    pub done: usize,
    /// Deals the GPU has won, and positions it has generated (0 without one).
    pub gpu_won: u64,
    pub gpu_positions: u64,
    /// GPU results waiting for a CPU worker to verify them, or to solve the deals the GPU didn't win.
    pub backlog: usize,
    /// GPU lines that failed verification so far (a bug), counted as results come in.
    pub gpu_failed: u64,
}

/// How often [`run`] reports progress.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);
/// The GPU waits while this many of its batches wait for the CPU threads.
const MAX_BACKLOG_BATCHES: usize = 8;
/// The longest a worker sleeps with nothing from the GPU to do, should the run pause.
const WAIT_FOR_GPU: Duration = Duration::from_millis(50);
/// GPU results a CPU thread takes at a time, and hands on the outcome of at once.
const RUN: usize = 256;
/// One deal in this many has its line replayed by the reference engine as well as by its rules on the top cards.
const FULL_REPLAY_EVERY: usize = 256;

/// A launch's results, which the CPU workers take a run at a time. Nothing is done per deal until a worker gets to it,
/// so a deal costs no lock, allocation or shared counter of its own: past a few hundred thousand deals a second, all
/// the more with the CPU slowed by a hot laptop, those held the GPU back.
struct GpuBatch {
    /// Consecutive positions.
    indices: Vec<usize>,
    result: BatchResult,
    /// The GPU had these deals, not ones it lost when it failed.
    given: bool,
    /// The next result to take.
    next: AtomicUsize,
}

/// Consecutive deals a worker has finished, handed on together.
#[derive(Default)]
struct Stretch {
    first: usize,
    count: usize,
    /// How many of them the GPU won, when no record of those is wanted.
    wins: u64,
    /// The log's records of those in the sample.
    sampled: Vec<u8>,
    /// The others, in order.
    solved: Vec<Solved>,
    /// What the GPU did for them.
    gpu: GpuTotals,
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
                    "note: no endgame tables in {}; `acc endgame build --piles 11` builds them (about 7 minutes, 1.2 GB) and makes solving about 1.2-1.3x faster",
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
    open_gpu_kernel(flag, settings, None, &[])
}

/// [`open_gpu`] with another kernel source and compiler defines (`acc batch --gpu-kernel`, `--gpu-define`), to time a
/// variant without a rebuild. With neither, the built-in kernel, as in `open_gpu`.
pub fn open_gpu_kernel(
    flag: &str,
    settings: Settings,
    kernel: Option<&std::path::Path>,
    defines: &[String],
) -> Result<Option<Arc<Gpu>>, String> {
    let required = match flag {
        "off" => return Ok(None),
        "on" => true,
        "auto" => false,
        f => return Err(format!("--gpu: {f:?} (auto, on or off)")),
    };
    let source = match kernel {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
        None => accordion_gpu::builtin_source().to_string(),
    };
    let start = Instant::now();
    match Gpu::with_defines(settings, &source, defines) {
        Ok(gpu) => {
            eprintln!(
                "GPU: {} ({} multiprocessors), kernel compiled in {:.1} s",
                gpu.name,
                gpu.sms,
                start.elapsed().as_secs_f64()
            );
            if kernel.is_some() || !defines.is_empty() {
                let name = kernel.map_or("built-in".to_string(), |p| p.display().to_string());
                eprintln!("GPU kernel: {name}, defines {defines:?} (NOT the built-in kernel as it stands)");
            }
            Ok(Some(Arc::new(gpu)))
        }
        Err(e) if required => Err(format!("--gpu on: {e}")),
        Err(e) if e == accordion_gpu::NO_NVRTC => {
            eprintln!("{e} Continuing on the CPU, which is much slower.");
            Ok(None)
        }
        Err(e) => {
            eprintln!("no GPU ({e}); solving on the CPU only");
            Ok(None)
        }
    }
}

/// Solves every deal in `seeds` from `opts.from` on with `config` (move-order seed = deal seed) and hands each result to
/// `sink`, in seed order. `progress` is called about once a second, results or not, and once at the end. Stops early if
/// `sink` fails.
pub fn run(
    seeds: Seeds,
    config: &Config,
    opts: &Options,
    mut sink: impl FnMut(Finished) -> Result<(), String>,
    mut progress: impl FnMut(Progress),
) -> Result<Ran, String> {
    let so_far = &opts.from;
    let next = AtomicUsize::new(so_far.done);
    let abort = AtomicBool::new(false);
    let gpu_down = AtomicBool::new(false);
    let pausing =
        || gpu_down.load(Ordering::Relaxed) || opts.pause.is_some_and(|p| p.load(Ordering::Relaxed));
    let from_gpu: Mutex<VecDeque<Arc<GpuBatch>>> = Mutex::new(VecDeque::new());
    let more_from_gpu = Condvar::new();
    let feeding = AtomicBool::new(opts.gpu.is_some());
    let gpu_won = AtomicU64::new(so_far.gpu.won);
    let gpu_positions = AtomicU64::new(so_far.gpu.positions);
    let gpu_failed = AtomicU64::new(so_far.gpu.failed);
    let launches = AtomicU64::new(so_far.gpu.launches);
    let cpu_time = Mutex::new(so_far.gpu.cpu);
    let started = Instant::now();
    let (tx, rx) = mpsc::sync_channel::<Stretch>(64);
    std::thread::scope(|scope| {
        let pausing = &pausing;
        let feeder = opts.gpu.as_deref().map(|gpu| {
            let (next, abort, from_gpu, feeding, gpu_down) = (&next, &abort, &from_gpu, &feeding, &gpu_down);
            let more_from_gpu = &more_from_gpu;
            let (launches, positions) = (&launches, &gpu_positions);
            scope.spawn(move || {
                #[cfg(test)]
                let fails = || opts.fail_gpu_after.is_some_and(|n| launches.load(Ordering::Relaxed) >= n);
                #[cfg(not(test))]
                let fails = || false;
                let stopping = || abort.load(Ordering::Relaxed) || pausing() || fails();
                let fed = feed(gpu, seeds, next, &stopping, (from_gpu, more_from_gpu), launches, positions);
                let fed = fed.and_then(|()| {
                    if !fails() {
                        return Ok(());
                    }
                    let first = next.fetch_add(gpu.settings.batch, Ordering::Relaxed);
                    let lost = first..(first + gpu.settings.batch).min(seeds.count());
                    Err(("as the test asked".to_string(), lost.collect()))
                });
                let error = fed.err().map(|(e, mut lost)| {
                    if opts.pause_if_gpu_fails {
                        eprintln!(
                            "\nGPU stopped: {e}\n(pausing: the {} deals it had in hand are done again)",
                            lost.len()
                        );
                        gpu_down.store(true, Ordering::Relaxed);
                    } else {
                        eprintln!(
                            "\nGPU stopped: {e}\n(the CPU solves the {} deals it had in hand, and the rest)",
                            lost.len()
                        );
                        lost.sort_unstable();
                        let result = BatchResult::none(lost.len());
                        from_gpu.lock().expect("no panics").push_back(Arc::new(GpuBatch {
                            indices: lost,
                            result,
                            given: false,
                            next: AtomicUsize::new(0),
                        }));
                    }
                    e
                });
                feeding.store(false, Ordering::Release);
                more_from_gpu.notify_all();
                error
            })
        });
        for _ in 0..opts.threads.max(1) {
            let tx = tx.clone();
            let (next, abort, from_gpu, feeding) = (&next, &abort, &from_gpu, &feeding);
            let (gpu_won, cpu_time) = (&gpu_won, &cpu_time);
            let more_from_gpu = &more_from_gpu;
            scope.spawn(move || {
                let mut search = Search::with_options(opts.table_log2, config.search);
                search.set_endgame(opts.endgame.clone());
                let mut time = CpuTime::default();
                let us = |t: Instant| t.elapsed().as_micros() as u64;
                let mut moves = Vec::new();
                while !abort.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    let mut done = Stretch::default();
                    if let Some((batch, run)) = take_run(from_gpu) {
                        done.first = batch.indices[run.start];
                        done.count = run.len();
                        let mut misses = 0;
                        for i in run {
                            let index = batch.indices[i];
                            let seed = seeds.seed(index);
                            // The deal is made only when something wants it: the full replay that is due, and the few
                            // deals in the sample (the GPU's batches have the cards' codes to take the sample by).
                            if let Some(sieve) = &opts.sieve {
                                if batch.given {
                                    sieve.note_codes(batch.result.codes(i), index, &mut done.sampled, || {
                                        Deal::from_seed(seed)
                                    });
                                } else {
                                    sieve.note(&Deal::from_seed(seed), index, &mut done.sampled);
                                }
                            }
                            let deal = index.is_multiple_of(FULL_REPLAY_EVERY).then(|| Deal::from_seed(seed));
                            if batch.given {
                                done.gpu.deals += 1;
                                done.gpu.attempts += u64::from(batch.result.tries(i));
                                done.gpu.positions += batch.result.nodes(i);
                            }
                            if batch.result.is_won(i) {
                                let verified = if opts.records || opts.each_gpu_win {
                                    gpu_win_recorded(&batch.result, i, index, seed, opts.records)
                                        .map(|solved| done.solved.push(solved))
                                } else {
                                    gpu_line_verifies(&batch.result, i, index, deal.as_ref(), &mut moves)
                                        .map(|()| done.wins += 1)
                                };
                                match verified {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        done.gpu.failed += 1;
                                        eprintln!(
                                            "\nseed {seed}: GPU line FAILED verification ({e}); solving it on the CPU"
                                        );
                                    }
                                }
                            }
                            let miss = Instant::now();
                            done.solved.push(solve_one(index, seed, config, &mut search, opts.records));
                            misses += us(miss);
                        }
                        done.gpu.won = done.wins
                            + done.solved.iter().filter(|s| s.report.stage == Stage::Gpu).count() as u64;
                        gpu_won.fetch_add(done.gpu.won, Ordering::Relaxed);
                        time.verifying += us(t).saturating_sub(misses);
                        time.misses += misses;
                    } else if feeding.load(Ordering::Acquire) {
                        // While the GPU is feeding, no deals of their own: a few thousand a second at most, for heat
                        // that slows the GPU (the laptop's CPU and GPU share a power limit), by 5 to 10%.
                        let queue = from_gpu.lock().expect("no panics");
                        if queue.is_empty() && feeding.load(Ordering::Acquire) {
                            let _ = more_from_gpu.wait_timeout(queue, WAIT_FOR_GPU).expect("no panics");
                        }
                        time.waiting += us(t);
                        continue;
                    } else {
                        let index = if pausing() { seeds.count() } else { next.fetch_add(1, Ordering::Relaxed) };
                        if index >= seeds.count() {
                            if from_gpu.lock().expect("no panics").is_empty() {
                                break;
                            }
                            continue;
                        }
                        done.first = index;
                        done.count = 1;
                        if let Some(sieve) = &opts.sieve {
                            sieve.note(&Deal::from_seed(seeds.seed(index)), index, &mut done.sampled);
                        }
                        done.solved.push(solve_one(index, seeds.seed(index), config, &mut search, opts.records));
                        time.own += us(t);
                    }
                    let t = Instant::now();
                    if tx.send(done).is_err() {
                        break;
                    }
                    time.handing_on += us(t);
                    *cpu_time.lock().expect("no panics") += std::mem::take(&mut time);
                }
                *cpu_time.lock().expect("no panics") += time;
            });
        }
        drop(tx);
        let mut standing = so_far.clone();
        let mut received = so_far.done;
        // Finished deals wait here for the ones before them: a hard deal holds up nothing else, and a minute of its
        // solving is a few hundred thousand of these, tens of MB.
        let mut waiting: BTreeMap<usize, Stretch> = BTreeMap::new();
        let mut result = Ok(());
        let stand = |received| Progress {
            done: received,
            gpu_won: gpu_won.load(Ordering::Relaxed),
            gpu_positions: gpu_positions.load(Ordering::Relaxed),
            backlog: from_gpu
                .lock()
                .expect("no panics")
                .iter()
                .map(|b| b.indices.len().saturating_sub(b.next.load(Ordering::Relaxed)))
                .sum(),
            gpu_failed: gpu_failed.load(Ordering::Relaxed),
        };
        let with_times = |standing: &Standing| {
            let mut whole = standing.clone();
            whole.gpu.launches = launches.load(Ordering::Relaxed);
            whole.gpu.cpu = *cpu_time.lock().expect("no panics");
            whole.gpu.cpu.total += started.elapsed().as_micros() as u64 * opts.threads.max(1) as u64;
            whole
        };
        let mut last = Instant::now();
        let mut last_checkpoint = Instant::now();
        loop {
            // Wake for the next report even when no deal finishes (the last hard deals, or a stalled GPU).
            match rx.recv_timeout(PROGRESS_EVERY.saturating_sub(last.elapsed())) {
                Ok(stretch) => {
                    received += stretch.count;
                    waiting.insert(stretch.first, stretch);
                    while result.is_ok()
                        && let Some(stretch) = waiting.remove(&standing.done)
                    {
                        standing.done += stretch.count;
                        standing.gpu.add(&stretch.gpu);
                        if stretch.gpu.failed > 0 {
                            gpu_failed.fetch_add(stretch.gpu.failed, Ordering::Relaxed);
                        }
                        let sampled =
                            (!stretch.sampled.is_empty()).then_some(Finished::Sampled(stretch.sampled));
                        let wins = (stretch.wins > 0).then_some(Finished::GpuWins(stretch.wins));
                        result = sampled
                            .into_iter()
                            .chain(wins)
                            .chain(stretch.solved.into_iter().map(Finished::Deal))
                            .try_for_each(&mut sink);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                progress(stand(received));
            }
            if result.is_ok()
                && let Some(every) = opts.checkpoint_every
                && last_checkpoint.elapsed() >= every
            {
                last_checkpoint = Instant::now();
                result = sink(Finished::Checkpoint(with_times(&standing)));
            }
            if result.is_err() {
                abort.store(true, Ordering::Relaxed);
            }
        }
        progress(stand(received));
        let on_gpu = feeder.is_some();
        let error = feeder.and_then(|h| h.join().expect("the feeder does not panic"));
        let whole = with_times(&standing);
        result.map(|()| Ran { done: whole.done, gpu: on_gpu.then_some(GpuTotals { error, ..whole.gpu }) })
    })
}

/// Threads making the GPU's deals. A ChaCha20 deal (the `second-generator` feature) costs about 40 times a main one, and 3
/// threads made only about 1.2M a second of them, so it gets more.
const GPU_PREPARERS: usize = if cfg!(feature = "second-generator") { 10 } else { accordion_gpu::PREPARERS };

/// The GPU feeder: batches of deals from the shared counter until it runs out (or the run stops), results into `queue`.
/// If the GPU fails, gives back the deals it had in hand.
fn feed(
    gpu: &Gpu,
    seeds: Seeds,
    next: &AtomicUsize,
    stopping: &(impl Fn() -> bool + Sync),
    (queue, more): (&Mutex<VecDeque<Arc<GpuBatch>>>, &Condvar),
    launches: &AtomicU64,
    positions: &AtomicU64,
) -> Result<(), (String, Vec<usize>)> {
    let batch = gpu.settings.batch;
    gpu.feed(
        GPU_PREPARERS,
        |&index: &usize| quick::deal_codes(seeds.seed(index)),
        || {
            // CPU threads that fall behind the GPU (a hot laptop) hold it back, so what waits for them stays small.
            while queue.lock().expect("no panics").len() >= MAX_BACKLOG_BATCHES {
                if stopping() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if stopping() {
                return None;
            }
            let first = next.fetch_add(batch, Ordering::Relaxed);
            (first < seeds.count()).then(|| (first..(first + batch).min(seeds.count())).collect())
        },
        |indices, result| {
            launches.fetch_add(1, Ordering::Relaxed);
            positions.fetch_add(result.total_nodes(), Ordering::Relaxed);
            queue.lock().expect("no panics").push_back(Arc::new(GpuBatch {
                indices,
                result,
                given: true,
                next: AtomicUsize::new(0),
            }));
            more.notify_all();
        },
    )
}

/// Verifies the line of the deal the GPU won at `i` of `result`, without allocating (`moves`: a buffer, used over and
/// over). `deal` is the seed's deal, which the caller makes (once) when the full replay is due.
fn gpu_line_verifies(
    result: &BatchResult,
    i: usize,
    index: usize,
    deal: Option<&Deal>,
    moves: &mut Vec<Move>,
) -> Result<(), String> {
    let line = result.line(i).expect("it is won");
    // A line that plays out, as nearly all do, needs nothing more unless the full replay is due.
    if !index.is_multiple_of(FULL_REPLAY_EVERY) && quick::line_wins(result.codes(i), line) {
        return Ok(());
    }
    moves.clear();
    moves.extend(line.iter().map(|&b| Move::from_byte(b)));
    let codes = result.codes(i);
    let piles = replay_codes(codes, moves).map_err(|(i, e)| format!("move {}: {e}", i + 1))?;
    if piles != 1 {
        return Err(format!("the line ends with {piles} piles, not one"));
    }
    // Now and then all the way from the seed, with the reference engine: that the GPU had this seed's deal, and the rules.
    if index.is_multiple_of(FULL_REPLAY_EVERY) {
        let deal = deal.expect("the worker makes the deal for every full replay");
        if deal_codes(deal) != *codes {
            return Err("the GPU's deal isn't this seed's".to_string());
        }
        replay(index, deal, moves, true)?;
    }
    Ok(())
}

/// A deal the GPU won with all that is known of it, verified, or the error if its line fails.
fn gpu_win_recorded(
    result: &BatchResult,
    i: usize,
    index: usize,
    seed: u64,
    records: bool,
) -> Result<Solved, String> {
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
        return Solved { seed, report, us, verified, record: None };
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
    Solved { seed, report, us, verified, record }
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

/// SplitMix64's finisher: every bit of `z` in every bit of the result.
pub fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A 64-bit fingerprint of the deal with suits and ranks renamed ([`Deal::relabelled`]): deals that play alike
/// share it, and different ones almost never do.
pub fn fingerprint(deal: &Deal) -> u64 {
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
    same_among(members)
}

/// Of `(fingerprint, seed)`s, the pairs of seeds with the same fingerprint that are the same game.
fn same_among(members: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
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

/// Seeds fingerprinted at a time when working from disk.
const CHUNK: usize = 1 << 20;
/// Bytes of a fingerprint kept in a bucket; the bucket's number is more of it.
const KEY_BYTES: usize = 5;

/// What [`same_games_on_disk`] needs on disk at most.
pub fn same_games_disk_bytes(count: u64) -> u64 {
    count.saturating_mul(KEY_BYTES as u64)
}

/// Into how many parts, as a power of two, [`same_games_on_disk`] splits `count` deals.
fn disk_part_bits(count: u64) -> u32 {
    (count >> 24).max(1).next_power_of_two().trailing_zeros().clamp(4, 12)
}

/// What [`same_games_on_disk`] holds in memory at most, on `threads` threads: a part of the deals each, read whole and
/// spread out into keys of 8 bytes beside the 5 read.
pub fn same_games_disk_memory(count: u64, threads: usize) -> u64 {
    (count >> disk_part_bits(count)).saturating_mul(KEY_BYTES as u64 + 8).saturating_mul(threads as u64)
}

/// `f` over the positions of `count` seeds, a chunk at a time in parallel, saying how far it has got.
fn over_chunks<R: Send>(count: usize, doing: &str, f: impl Fn(Range<usize>) -> R + Sync) -> Vec<R> {
    let chunks = count.div_ceil(CHUNK);
    let finished = AtomicUsize::new(0);
    (0..chunks)
        .into_par_iter()
        .map(|c| {
            let result = f(c * CHUNK..((c + 1) * CHUNK).min(count));
            let n = finished.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of((chunks / 10).max(1)) {
                eprintln!("  {doing}: {}%", 100 * n / chunks);
            }
            result
        })
        .collect()
}

/// [`same_games`] for more deals than memory can hold 8 bytes each of: 25 billion would take 200 GB. Each fingerprint
/// goes to a file in `dir` (removed at the end) by its first bits, as the next 5 bytes, about 16 million deals to a
/// file so that one sorts in memory; what repeats in a file is looked up by fingerprinting again, as there. A repeat of
/// those bits needn't be one of the whole fingerprint (at 25 billion, about 140,000 are not), which costs nothing.
pub fn same_games_on_disk(
    seeds: Seeds,
    print: impl Fn(u64) -> u64 + Sync,
    dir: &Path,
) -> Result<Vec<(u64, u64)>, String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let result = same_games_in(seeds, &print, dir);
    let _ = std::fs::remove_dir_all(dir);
    result
}

fn same_games_in(
    seeds: Seeds,
    print: &(impl Fn(u64) -> u64 + Sync),
    dir: &Path,
) -> Result<Vec<(u64, u64)>, String> {
    let count = seeds.count();
    let bits = disk_part_bits(count as u64);
    let key_bits = 8 * KEY_BYTES as u32;
    let key = |f: u64| f >> (64 - bits - key_bits);
    let paths: Vec<PathBuf> = (0..1usize << bits).map(|b| dir.join(format!("{b}.bin"))).collect();
    let files = paths
        .iter()
        .map(|path| {
            let file = File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?;
            Ok(Mutex::new(BufWriter::with_capacity(1 << 16, file)))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let writing = |e: std::io::Error| format!("writing to {}: {e}", dir.display());
    for written in over_chunks(count, "fingerprinting the deals", |positions| {
        let mut parts = vec![Vec::new(); files.len()];
        for i in positions {
            let k = key(print(seeds.seed(i)));
            let part: &mut Vec<u8> = &mut parts[(k >> key_bits) as usize];
            part.extend_from_slice(&k.to_le_bytes()[..KEY_BYTES]);
        }
        parts.iter().zip(&files).try_for_each(|(part, file)| file.lock().expect("no panics").write_all(part))
    }) {
        written.map_err(writing)?;
    }
    for file in files {
        file.into_inner().expect("no panics").flush().map_err(writing)?;
    }
    let repeated = (0..paths.len())
        .into_par_iter()
        .map(|b| {
            let path = &paths[b];
            let bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
            let mut keys: Vec<u64> = bytes
                .as_chunks::<KEY_BYTES>()
                .0
                .iter()
                .map(|c| {
                    let mut word = [0; 8];
                    word[..KEY_BYTES].copy_from_slice(c);
                    u64::from_le_bytes(word)
                })
                .collect();
            drop(bytes);
            let _ = std::fs::remove_file(path);
            keys.sort_unstable();
            Ok(keys
                .windows(2)
                .filter(|w| w[0] == w[1])
                .map(|w| (b as u64) << key_bits | w[0])
                .collect::<Vec<u64>>())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let repeated: HashSet<u64> = repeated.into_iter().flatten().collect();
    if repeated.is_empty() {
        return Ok(Vec::new());
    }
    eprintln!("  {} fingerprints to look up", repeated.len());
    let members = over_chunks(count, "looking them up", |positions| {
        positions
            .filter_map(|i| {
                let seed = seeds.seed(i);
                let f = print(seed);
                repeated.contains(&key(f)).then_some((f, seed))
            })
            .collect::<Vec<_>>()
    });
    Ok(same_among(members.into_iter().flatten().collect()))
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
    fn wilson_at_a_trillion_deals_is_good_to_the_digits_printed() {
        // By Python's decimal module, to the 10 decimals of a percentage that the report prints.
        let (lo, hi) = wilson(1_000_000_000_000 - 1_040, 1_000_000_000_000);
        assert!((100.0 * lo - 99.9999998895_f64).abs() < 1e-10, "{}", 100.0 * lo);
        assert!((100.0 * hi - 99.9999999021_f64).abs() < 1e-10, "{}", 100.0 * hi);
    }

    #[test]
    fn wilson_at_100_trillion_deals_is_good_to_the_digits_printed() {
        // By Python's decimal module, with the same z, to the 12 decimals printed from 10 trillion deals on.
        let (lo, hi) = wilson(100_000_000_000_000 - 74_000, 100_000_000_000_000);
        assert!((100.0 * lo - 99.999_999_925_464_9_f64).abs() < 1e-12, "{}", 100.0 * lo);
        assert!((100.0 * hi - 99.999_999_926_531_25_f64).abs() < 1e-12, "{}", 100.0 * hi);
    }

    #[test]
    fn the_memory_of_the_check_on_disk_goes_by_its_parts() {
        // 4,096 parts at most, as `same_games_in` makes them.
        assert_eq!(same_games_disk_memory(1 << 40, 16), 13 * (1 << 28) * 16);
        // Fewer parts for fewer deals: each still has up to 2^24.
        assert_eq!(same_games_disk_memory(1 << 34, 2), 13 * (1 << 24) * 2);
        assert_eq!(same_games_disk_memory(5_000, 4), 13 * (5_000 / 16) * 4);
        assert_eq!(same_games_disk_memory(u64::MAX, usize::MAX), u64::MAX, "and no overflow");
    }

    #[test]
    fn wilson_interval_matches_known_values() {
        let (lo, hi) = wilson(0, 10);
        assert!(lo == 0.0 && (hi - 0.2775).abs() < 1e-3, "{hi}");
        let (lo, hi) = wilson(100_000, 100_000);
        assert!((lo - 0.999_962).abs() < 1e-5 && (hi - 1.0).abs() < 1e-12, "{lo} {hi}");
        // A deal in a billion lost, as in the 20 billion run: doubles still hold it, to the digits the report prints.
        let (lo, hi) = wilson(19_999_999_980, 20_000_000_000);
        assert!(
            (lo - 0.999_999_998_455).abs() < 1e-11 && (hi - 0.999_999_999_353).abs() < 1e-11,
            "{lo} {hi}"
        );
    }

    fn check_run(gpu: Option<Arc<Gpu>>, count: u64, records: bool) {
        let seeds: Vec<u64> = (500..500 + count).collect();
        let on_gpu = gpu.is_some();
        let opts = Options { threads: 4, table_log2: 12, records, gpu, ..Options::default() };
        let mut seen = vec![0u32; seeds.len()];
        let mut stages: HashMap<String, u32> = HashMap::new();
        let mut counted_wins = 0u64;
        let mut last_index = None;
        let ran = run(
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
                    Finished::Checkpoint(_) => panic!("none were asked for"),
                    Finished::Sampled(_) => panic!("no sample was asked for"),
                };
                let index = (s.seed - seeds[0]) as usize;
                assert!(last_index.is_none_or(|last| last < index), "out of order");
                last_index = Some(index);
                seen[index] += 1;
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
        assert_eq!(ran.done, seeds.len());
        match ran.gpu {
            Some(t) => {
                assert!(on_gpu && t.error.is_none() && t.failed == 0);
                assert_eq!(t.deals, count);
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

    /// Pauses after `pause_after` deals, then carries on to the end, in as many goes as that takes: every deal comes
    /// back once and in order, and the totals come out as for one run.
    fn check_pause(gpu: Option<Arc<Gpu>>, count: usize, pause_after: usize, pause: &'static AtomicBool) {
        let seeds: Vec<u64> = (500..500 + count as u64).collect();
        let on_gpu = gpu.is_some();
        let mut from = Standing::default();
        let mut seen = vec![0u32; count];
        let mut handed_on = 0;
        let mut goes = 0;
        while from.done < count {
            goes += 1;
            assert!(goes < 1000, "it stopped getting anywhere");
            pause.store(false, Ordering::Relaxed);
            let opts = Options {
                threads: 4,
                table_log2: 12,
                each_gpu_win: true,
                gpu: gpu.clone(),
                from: from.clone(),
                pause: Some(pause),
                checkpoint_every: Some(Duration::ZERO),
                ..Options::default()
            };
            let start = from.done;
            let mut last = None;
            let mut in_this_go = 0;
            let ran = run(
                Seeds::List(&seeds),
                &Config::default(),
                &opts,
                |finished| {
                    match finished {
                        Finished::Deal(s) => {
                            let index = (s.seed - seeds[0]) as usize;
                            assert!(index >= start && last.is_none_or(|l| l < index), "out of order");
                            last = Some(index);
                            seen[index] += 1;
                            handed_on += 1;
                            in_this_go += 1;
                            if in_this_go >= pause_after {
                                pause.store(true, Ordering::Relaxed);
                            }
                        }
                        Finished::GpuWins(_) => panic!("each win was asked for"),
                        Finished::Sampled(_) => panic!("no sample was asked for"),
                        // Every deal before a checkpoint has been handed on, and no others.
                        Finished::Checkpoint(standing) => assert_eq!(standing.done, handed_on),
                    }
                    Ok(())
                },
                |_| {},
            )
            .unwrap();
            assert!(ran.done > from.done && ran.done <= count);
            assert_eq!(ran.done, handed_on, "deals handed on");
            from = Standing { done: ran.done, gpu: ran.gpu.unwrap_or_default() };
        }
        assert!(goes > 1, "it never paused");
        assert!(seen.iter().all(|&n| n == 1));
        if on_gpu {
            let t = &from.gpu;
            assert_eq!(t.deals, count as u64);
            assert!(t.won > 0 && t.won <= t.deals && t.failed == 0 && t.attempts >= t.won);
            assert!(t.launches as usize >= goes && t.cpu.total > 0);
        }
    }

    /// The GPU fails part of the way through a run.
    fn check_gpu_failure(gpu: Arc<Gpu>) {
        let count = 12_000;
        let seeds: Vec<u64> = (500..500 + count as u64).collect();
        let run_it = |seen: &mut Vec<u32>, opts: &Options| {
            run(
                Seeds::List(&seeds),
                &Config::default(),
                opts,
                |finished| {
                    if let Finished::Deal(s) = finished {
                        seen[(s.seed - seeds[0]) as usize] += 1;
                    }
                    Ok(())
                },
                |_| {},
            )
            .unwrap()
        };
        let options = || Options {
            threads: 4,
            table_log2: 12,
            each_gpu_win: true,
            gpu: Some(Arc::clone(&gpu)),
            ..Options::default()
        };

        // Left to the CPU: it solves the deals in the GPU's hands and all the rest.
        let mut seen = vec![0; count];
        let opts = Options { fail_gpu_after: Some(3), ..options() };
        let ran = run_it(&mut seen, &opts);
        assert!(seen.iter().all(|&n| n == 1));
        let t = ran.gpu.unwrap();
        assert_eq!(ran.done, count);
        assert!(t.error.is_some() && t.deals > 0 && (t.deals as usize) < count, "{} deals", t.deals);

        // Pausing for it: nothing from the first deal it had lost on, and a new run takes them all from there.
        let mut seen = vec![0; count];
        let opts = Options { fail_gpu_after: Some(3), pause_if_gpu_fails: true, ..options() };
        let ran = run_it(&mut seen, &opts);
        let t = ran.gpu.unwrap();
        assert!(t.error.is_some() && ran.done > 0 && ran.done < count, "at {}", ran.done);
        assert_eq!(t.deals as usize, ran.done);
        assert!(seen[..ran.done].iter().all(|&n| n == 1) && seen[ran.done..].iter().all(|&n| n == 0));
        let opts = Options { from: Standing { done: ran.done, gpu: t }, ..options() };
        let ran = run_it(&mut seen, &opts);
        let t = ran.gpu.unwrap();
        assert!(seen.iter().all(|&n| n == 1));
        assert!(t.error.is_none() && ran.done == count && t.deals as usize == count);
    }

    #[test]
    fn a_paused_run_carries_on_where_it_stopped() {
        static PAUSE: AtomicBool = AtomicBool::new(false);
        check_pause(None, 300, 40, &PAUSE);
    }

    #[test]
    fn with_the_gpu_too() {
        static PAUSE: AtomicBool = AtomicBool::new(false);
        match open_gpu("auto", Settings { batch: 512, ..Settings::default() }).unwrap() {
            Some(gpu) => {
                check_run(Some(Arc::clone(&gpu)), 3_000, true);
                check_run(Some(Arc::clone(&gpu)), 3_000, false);
                // A go can take 10 batches before it sees the pause.
                check_pause(Some(Arc::clone(&gpu)), 30_000, 400, &PAUSE);
                check_gpu_failure(gpu);
            }
            None => eprintln!("skipped: no GPU"),
        }
    }

    /// A sample's records come out in seed order, each once, whatever happens to the run on the way.
    fn check_sample(gpu: Option<Arc<Gpu>>, count: usize, pause: &'static AtomicBool) {
        let seeds = Seeds::Range { from: 1 << 40, count };
        let sieve = Sieve { below: 1 << 61 };
        let mut want = Vec::new();
        for i in 0..count {
            sieve.note(&Deal::from_seed(seeds.seed(i)), i, &mut want);
        }
        assert!(want.len() > 10 * crate::games::RECORD, "{} bytes", want.len());
        let (mut got, mut from, mut goes) = (Vec::new(), Standing::default(), 0);
        while from.done < count {
            goes += 1;
            pause.store(false, Ordering::Relaxed);
            let opts = Options {
                threads: 4,
                table_log2: 12,
                gpu: gpu.clone(),
                from: from.clone(),
                pause: Some(pause),
                sieve: Some(sieve),
                ..Options::default()
            };
            let mut stretches = 0;
            let ran = run(
                seeds,
                &Config::default(),
                &opts,
                |finished| {
                    if let Finished::Sampled(records) = finished {
                        got.extend(records);
                        stretches += 1;
                        if stretches == 8 {
                            pause.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                },
                |_| {},
            )
            .unwrap();
            from = Standing { done: ran.done, gpu: ran.gpu.unwrap_or_default() };
        }
        assert!(got == want, "{} bytes, not {}", got.len(), want.len());
        if gpu.is_some() {
            assert!(goes > 1, "it never paused");
        }
    }

    #[test]
    fn a_sample_comes_out_in_order() {
        static PAUSE: AtomicBool = AtomicBool::new(false);
        check_sample(None, 3_000, &PAUSE);
    }

    #[test]
    fn a_sample_comes_out_in_order_with_the_gpu_too() {
        static PAUSE: AtomicBool = AtomicBool::new(false);
        match open_gpu("auto", Settings::default()).unwrap() {
            // More than the five batches (of 262144 deals) the feed may hold when the pause comes: it finishes those, so with fewer it never pauses.
            Some(gpu) => check_sample(Some(gpu), 2_000_000, &PAUSE),
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
    fn on_disk_it_finds_the_same_games() {
        let dir = std::env::temp_dir().join(format!("acc-games-test-{}", std::process::id()));
        let print = |s: u64| fingerprint(&Deal::from_seed(s));
        let on_disk = |seeds: Seeds, print: &(dyn Fn(u64) -> u64 + Sync)| {
            same_games_on_disk(seeds, print, &dir).unwrap()
        };
        // More than a chunk, so that a repeat is seen across chunks and buckets.
        let range = Seeds::Range { from: 0, count: 2 * CHUNK + 5 };
        assert!(on_disk(range, &print).is_empty());
        assert!(!dir.exists(), "it cleans up");
        // The same game: seed 2 twice, far apart.
        let mut seeds: Vec<u64> = (0..CHUNK as u64 + 5).collect();
        seeds.push(2);
        assert_eq!(on_disk(Seeds::List(&seeds), &print), vec![(2, 2)]);
        // A whole fingerprint in common, but not the same cards.
        let last = CHUNK as u64 + 4;
        let same_print = |s: u64| print(if s == last { 2 } else { s });
        assert!(on_disk(Seeds::List(&seeds[..seeds.len() - 1]), &same_print).is_empty());
        // The same first bits but not the whole fingerprint.
        let same_bits = |s: u64| if s == last { print(2) ^ 1 } else { print(s) };
        assert!(on_disk(Seeds::List(&seeds[..seeds.len() - 1]), &same_bits).is_empty());
        // Agreeing with the check in memory, with many repeats.
        let many: Vec<u64> = (0..50_000).map(|i| i % 49_990).collect();
        assert_eq!(on_disk(Seeds::List(&many), &print), same_games(Seeds::List(&many), print));
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
