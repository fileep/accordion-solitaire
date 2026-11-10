//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time, and on the GPU when
//! there is one (PLAN.md Phases 4, 5 and 8). Every win is verified by the reference rules before it counts; every
//! deal that isn't won gets a dossier, and a proof attempt on a thread of its own while the run goes on: two
//! exhaustive runs that must agree before a loss counts ([`prove::prove_both`]). The summary gives the win rate with
//! a Wilson 95% interval, counting undecided deals both ways, and the odds that any two deals are the same game, which it
//! checks: exactly when memory holds all the deals, by a sample past that ([`crate::games`]).
//!
//! A long run can stop and carry on. Results are applied in seed order, so where a run stands is one number, and every
//! minute that number and all the totals go to a checkpoint (written whole or not at all, after the results it counts
//! are safely on disk). Ctrl+C or a file beside the checkpoint pauses the run; a crash costs the last minute;
//! `--resume` carries on from either, with the settings the run began with. At the end the checkpoint gives way to the
//! report, saved beside it. Beside it too, for reading from elsewhere while it goes, are a status file (`.status.json`)
//! and a log of what has happened to the run (`.events.log`).

use crate::dossier;
use crate::games::{self, Found, Sieve};
use crate::numbers::{self, parse_count};
use crate::pause;
use crate::progress::{Meter, Trouble};
use crate::prove::{self, Finding, Limits};
use crate::runner::{self, Finished, Options, Progress, Seeds, Standing, percentile, wilson};
use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_gpu::Settings;
use accordion_solver::{Config, Endgame, Search, Stage, Verdict, solve};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

// The groups of `acc batch --help`, which come in the order of the first flag in each.
const WHICH_DEALS: &str = "Which deals";
const SPEED: &str = "Speed and machine";
const STOPPING: &str = "Stopping and carrying on";
const OUTPUT: &str = "Output";
const PROVING: &str = "Proving losses";
const GAMES: &str = "Repeated-game check";
const RESEARCH: &str = "Research";

const EXAMPLES: &str = "\
Examples:
  acc batch --count 1m                         the first million deals
  acc batch --from 5b --count 100k --gpu off   100,000 deals from seed 5,000,000,000, on the CPU alone
  acc batch --count 2t --stop-after 8          two trillion deals, pausing after 8 hours
  acc batch --resume checkpoints/batch-0-2000000000000.json
                                               carry on with that run";

#[derive(Args, Clone, Serialize, Deserialize)]
#[command(after_help = EXAMPLES)]
pub struct BatchArgs {
    /// First seed.
    #[arg(long, default_value_t = 0, value_parser = parse_count, help_heading = WHICH_DEALS)]
    from: u64,
    /// Number of consecutive seeds. Here and in other counts, 5000, 2m, 1.5b, 100t and 1e12 all work.
    #[arg(long, default_value_t = 1000, value_parser = parse_count, help_heading = WHICH_DEALS)]
    count: u64,
    /// Threads (0 = every core); with the GPU, one of them feeds it.
    #[arg(long, default_value_t = 0, help_heading = SPEED)]
    threads: usize,
    /// GPU ladder attempts in front of the CPU solver: `auto` (use an NVIDIA GPU if there is one), `on` or `off`.
    /// Which deals the GPU wins, and its lines, depend on timing; every line is still verified.
    #[arg(long, default_value = "auto", value_parser = ["auto", "on", "off"], help_heading = SPEED)]
    gpu: String,
    /// Endgame tables to use: a pile count, `off`, or `auto` (the largest found in --endgame-dir).
    #[arg(long, default_value = "auto", hide_short_help = true, help_heading = SPEED)]
    endgame: String,
    /// Directory holding endgame tables (`acc endgame build`).
    #[arg(long, default_value = "endgame", hide_short_help = true, help_heading = SPEED)]
    endgame_dir: PathBuf,
    /// Carry on with the paused or crashed run in this checkpoint, from the directory it started in, with the settings
    /// it started with: only --threads, --checkpoint-every, --log-dir, --no-log, --games-memory, --no-games-check,
    /// --keep-free, --until and --stop-after may be given.
    #[arg(long, help_heading = STOPPING, conflicts_with_all = [
        "from", "count", "max_nodes", "config", "out", "dossiers", "no_dossiers", "table", "gpu", "gpu_config",
        "gpu_kernel", "gpu_define",
        "endgame_dir", "endgame", "gallery", "prove", "prove_memory", "checkpoint", "games_disk", "plant_repeats",
    ])]
    resume: Option<PathBuf>,
    /// Pause once this many of the run's deals are done: a way to run in legs.
    #[arg(long, value_parser = parse_count, help_heading = STOPPING)]
    until: Option<u64>,
    /// Pause after this many hours of this session: a way to run in legs.
    #[arg(long, help_heading = STOPPING)]
    stop_after: Option<f64>,
    /// Where to keep the state that lets this run pause and carry on, with the run's other files beside it. Default
    /// checkpoints/batch-<from>-<count>.json. Ctrl+C pauses the run (a second one stops it at once), and so does a file
    /// named like the checkpoint but ending .pause.
    #[arg(long, hide_short_help = true, help_heading = STOPPING)]
    checkpoint: Option<PathBuf>,
    /// Keep no checkpoint: the run can't pause, and if the GPU fails the CPU solves the rest.
    #[arg(long, hide_short_help = true, help_heading = STOPPING, conflicts_with_all = ["checkpoint", "resume", "until", "stop_after"])]
    no_checkpoint: bool,
    /// How often to save the checkpoint, in seconds.
    #[arg(long, default_value_t = 60, hide_short_help = true, help_heading = STOPPING)]
    checkpoint_every: u64,
    /// Pause, with exit code 74, when less than this is free on the checkpoint's disk, in GB.
    #[arg(long, default_value_t = 5.0, hide_short_help = true, help_heading = STOPPING)]
    keep_free: f64,
    /// Write one verified solve record per deal here (JSON Lines, compact moves), in seed order.
    #[arg(long, help_heading = OUTPUT)]
    out: Option<PathBuf>,
    /// Where deals that aren't won get their dossiers.
    #[arg(long, default_value = "output/hard-deals", hide_short_help = true, help_heading = OUTPUT)]
    dossiers: PathBuf,
    /// Don't write dossiers.
    #[arg(long, hide_short_help = true, help_heading = OUTPUT)]
    no_dossiers: bool,
    /// Keep this run's N hardest deals (most positions) as verified solve files in <dossiers>/gallery/, replacing
    /// the gallery there, and list them in its INDEX.md. With the GPU, only deals solved on the CPU are ranked.
    #[arg(long, default_value_t = 0, hide_short_help = true, help_heading = OUTPUT)]
    gallery: usize,
    /// Where the speed log goes: a CSV row a second of how fast the run is going, in batch-<UTC time>Z.csv.
    #[arg(long, default_value = "logs", hide_short_help = true, help_heading = OUTPUT)]
    log_dir: PathBuf,
    /// Don't write the speed log.
    #[arg(long, hide_short_help = true, help_heading = OUTPUT)]
    no_log: bool,
    /// Positions each of the two exhaustive runs may generate when proving a deal the solver didn't win (0: don't
    /// prove). Proofs run one at a time on a thread of their own while the run goes on, and the run waits for the
    /// last; one can take an hour.
    #[arg(long, default_value_t = prove::DEFAULT_POSITIONS, value_parser = parse_count, hide_short_help = true, help_heading = PROVING)]
    prove: u64,
    /// Memory each proof run's set of dead positions may take, in GB.
    #[arg(long, default_value_t = 8, hide_short_help = true, help_heading = PROVING)]
    prove_memory: usize,
    /// Memory the check that the deals are all different games may take, in GB. A run that fits has them all checked at
    /// the end. Past that, it checks a sample of them as it goes.
    #[arg(long, default_value_t = 16, hide_short_help = true, help_heading = GAMES)]
    games_memory: u64,
    /// Room the sample may take on disk, in GB, at 11 bytes a deal (in the checkpoint's directory, or the system's temporary
    /// one without a checkpoint): by default 50, or half of what is free if that's less. Room for 5 bytes for every deal asks
    /// for the full check on disk instead, which takes two more passes over the deals at the end.
    #[arg(long, hide_short_help = true, help_heading = GAMES)]
    games_disk: Option<f64>,
    /// Skip the check that the deals are all different games.
    #[arg(long, hide_short_help = true, help_heading = GAMES)]
    no_games_check: bool,
    /// Positions one deal may search (on one thread) before it counts as not won.
    #[arg(long, default_value_t = 50_000_000, value_parser = parse_count, hide_short_help = true, help_heading = RESEARCH)]
    max_nodes: u64,
    /// Solver settings: the name of a JSON file, or the JSON itself, e.g. '{"ladder": {"start": 36}}'. Fields left out
    /// keep their defaults; --max-nodes overrides `max_nodes`.
    #[arg(long, hide_short_help = true, help_heading = RESEARCH)]
    config: Option<String>,
    /// GPU settings: the name of a JSON file, or the JSON itself, e.g. '{"budget": 4000}'.
    #[arg(long, hide_short_help = true, help_heading = RESEARCH)]
    gpu_config: Option<String>,
    /// Dead-position table per thread, as log2 of 64-byte buckets (14 = 1 MB; small tables that stay in the core's
    /// own cache were fastest, see docs/BENCHMARKS.md).
    #[arg(long, default_value_t = 14, hide_short_help = true, help_heading = RESEARCH)]
    table: u32,
    /// Compile this kernel source instead of the built-in one (to time a variant without a rebuild).
    #[arg(long, hide = true)]
    #[serde(default)]
    gpu_kernel: Option<PathBuf>,
    /// A define for the kernel's compile, NAME=VALUE; may be repeated.
    #[arg(long, value_name = "NAME=VALUE", hide = true)]
    #[serde(default)]
    gpu_define: Vec<String>,
    /// For testing the check for repeated games: every Nth deal is the deal before it.
    #[arg(long, hide = true)]
    plant_repeats: Option<usize>,
}

/// How a run checks that its deals are different games.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
enum GamesPlan {
    Off,
    /// Every deal, at the end.
    Whole,
    /// A sample of the deals, noted as they are solved.
    Sampled(Sieve),
}

/// What the proofs of the deals the solver didn't win found.
#[derive(Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Proofs {
    lost: u64,
    won: u64,
    undecided: u64,
    /// The runs disagreed, or a winning line failed the reference engine: bugs.
    failed: u64,
}

/// The prover's work, as the run saves it.
#[derive(Default)]
struct ProofState {
    tally: Proofs,
    /// Deals sent to the prover and not finished. A resumed run proves these again from the start.
    pending: BTreeSet<u64>,
}

/// Proves the deals sent to it, one at a time, and records each in its dossier (with a loss record in
/// `<dossiers>/proofs/` for a proven loss) when there are dossiers.
fn prover(
    seeds: mpsc::Receiver<u64>,
    limits: Limits,
    tables: Option<Arc<Endgame>>,
    dossiers: Option<PathBuf>,
    state: &Mutex<ProofState>,
    events: &Events,
) {
    for seed in seeds {
        let mut found = Proofs::default();
        prove_one(seed, &limits, tables.as_deref(), dossiers.as_deref(), &mut found);
        events.note(&format!(
            "seed {seed}: {}",
            if found.lost > 0 {
                "proven lost"
            } else if found.won > 0 {
                "won by the exhaustive search"
            } else if found.undecided > 0 {
                "undecided"
            } else {
                "PROOF FAILED"
            }
        ));
        let mut state = state.lock().expect("no panics");
        state.tally.lost += found.lost;
        state.tally.won += found.won;
        state.tally.undecided += found.undecided;
        state.tally.failed += found.failed;
        state.pending.remove(&seed);
    }
}

fn prove_one(
    seed: u64,
    limits: &Limits,
    tables: Option<&Endgame>,
    dossiers: Option<&Path>,
    tally: &mut Proofs,
) {
    let deal = Deal::from_seed(seed);
    eprintln!("\nseed {seed}: not won; proving it lost, up to {} positions a run", limits.positions);
    let say = |line: String| eprintln!("seed {seed}: {line}");
    let proved = prove::prove_both(&Position::from_deal(&deal), limits, tables, say)
        .and_then(|p| p.record(&deal, Some(seed)).map(|record| (p, record)));
    let (p, record) = match proved {
        Ok(x) => x,
        Err(e) => {
            tally.failed += 1;
            eprintln!("seed {seed}: PROOF FAILED: {e}");
            return;
        }
    };
    eprintln!(
        "seed {seed}: {}",
        match p.finding {
            Finding::Lost => {
                tally.lost += 1;
                "proven lost"
            }
            Finding::Won(_) => {
                tally.won += 1;
                "won by the exhaustive search (the line verified)"
            }
            Finding::Undecided => {
                tally.undecided += 1;
                "undecided: a run hit its limit"
            }
        }
    );
    let Some(dir) = dossiers else { return };
    let written = dossier::record_proof(dir, &deal, Some(seed), &p.finding, p.best(), p.json.clone())
        .and_then(|_| {
            if p.finding != Finding::Lost {
                return Ok(());
            }
            let proofs = dir.join("proofs");
            std::fs::create_dir_all(&proofs).map_err(|e| format!("creating {}: {e}", proofs.display()))?;
            let path = proofs.join(format!("seed-{seed}-loss.json"));
            std::fs::write(&path, record.to_json() + "\n")
                .map_err(|e| format!("writing {}: {e}", path.display()))
        });
    if let Err(e) = written {
        eprintln!("seed {seed}: {e}");
    }
}

/// The run's tallies of the deals handed on so far.
#[derive(Default, Clone, Serialize, Deserialize)]
struct Tally {
    won: u64,
    exhausted: u64,
    unknown: u64,
    failed: u64,
    /// Wins on the GPU; the other stages' are in `stages`.
    gpu_stage: u64,
    stages: BTreeMap<String, u64>,
    total_nodes: u64,
    /// Deals solved on the CPU whose positions and times are in the file of costs: the first [`COST_SAMPLE_MOST`].
    cpu_deals: u64,
    /// All the deals solved on the CPU, however many are in the file, and what they cost. A checkpoint from before there was
    /// a limit has none of these, and every deal in the file ([`Tally::load_costs`]).
    #[serde(default)]
    all_cpu_deals: u64,
    #[serde(default)]
    total_micros: u64,
    #[serde(default)]
    most_nodes: u64,
    #[serde(default)]
    most_micros: u64,
    /// Whether any dossier was written.
    any_dossier: bool,
    hardest: BinaryHeap<Reverse<(u64, u64)>>,
}

impl Tally {
    /// Counts one more deal solved on the CPU in the totals over all of them.
    fn add_cost(&mut self, nodes: u64, micros: u64) {
        self.all_cpu_deals += 1;
        self.total_micros += micros;
        self.most_nodes = self.most_nodes.max(nodes);
        self.most_micros = self.most_micros.max(micros);
    }

    /// The positions and times in the file of costs, up to the first `most` deals. A checkpoint from before there was a limit
    /// has no totals and every deal in the file, so they are worked out from all of it; what is over the limit is then left
    /// out, and cut from the file once the next save's checkpoint is in place.
    fn load_costs(&mut self, file: &Path, most: usize) -> Result<(Vec<u64>, Vec<u64>), String> {
        let reading = |e: std::io::Error| {
            format!(
                "reading {}: {e}; it goes with the checkpoint, so put it back, or delete {} to start again",
                file.display(),
                family(file)
            )
        };
        let deals = self.cpu_deals as usize;
        let no_totals = self.all_cpu_deals < self.cpu_deals;
        let keep = deals.min(most);
        if no_totals && keep < deals {
            eprintln!(
                "note: this run's file of costs has {deals} deals, more than this acc keeps ({most}); it keeps the first {keep} \
                 and exact totals of all of them from here, so finish the run with this acc: an older one would report the \
                 costs of the first {keep} alone"
            );
        }
        // On the powers of two a first run's vectors go through, or a resume could double them to twice the limit.
        let room = (keep + 1).next_power_of_two().min(most);
        let (mut nodes, mut micros) = (Vec::with_capacity(room), Vec::with_capacity(room));
        if deals > 0 {
            let mut costs = BufReader::with_capacity(1 << 20, File::open(file).map_err(reading)?);
            let mut entry = [0; COST_BYTES];
            for i in 0..if no_totals { deals } else { keep } {
                costs.read_exact(&mut entry).map_err(reading)?;
                let (n, us) = entry.split_at(8);
                let word = |c: &[u8]| u64::from_le_bytes(c.try_into().expect("8 bytes"));
                let (n, us) = (word(n), word(us));
                if no_totals {
                    self.add_cost(n, us);
                }
                if i < keep {
                    nodes.push(n);
                    micros.push(us);
                }
            }
        }
        self.cpu_deals = keep as u64;
        Ok((nodes, micros))
    }
}

/// The files of the run with this checkpoint, for telling someone what to delete: `checkpoints/batch-0-1000.*`.
fn family(checkpoint: &Path) -> String {
    checkpoint.with_extension("*").display().to_string()
}

/// A checkpoint's format. 3: the games log's records are 11 bytes (a 5-byte key and a 6-byte position), not 10.
const VERSION: u32 = 3;
/// The format before, whose checkpoints are the same but for the games log of a sample, whose records were 10 bytes.
const VERSION_WITH_SHORT_RECORDS: u32 = 2;
/// The bytes of a deal in the file of costs: its positions, and its microseconds.
const COST_BYTES: usize = 16;
/// The most deals whose positions and times are kept, in memory and in the file of costs: 2 GB. They are there for the
/// percentiles in the report, and since deals are random the first ones are as fair a sample as any. Everything else in
/// the report is exact over all the deals, from the totals in [`Tally`].
const COST_SAMPLE_MOST: usize = 125_000_000;
/// From this many deals the win rate has two more decimals, as the interval has narrowed to the last ones it had.
pub const LONGER_WIN_RATE_FROM: u64 = 10_000_000_000_000;
/// The exit code of a run that paused because the GPU failed: try again, as a new process may well get the GPU back.
const EXIT_GPU_FAILED: i32 = 75;
/// The exit code of a run that paused or stopped because it can't save: make room on the disk, and carry on.
const EXIT_NO_ROOM: i32 = 74;
/// Checkpoints in a row that fail to save before the run pauses, rather than go on with nothing to show for it.
const SAVES_FAILED_AT_MOST: u32 = 5;
/// With no deal finished for this long, the GPU must have stuck where its watchdog didn't catch it.
const STUCK_AFTER: Duration = Duration::from_secs(15 * 60);
/// The most room the sample takes unless asked for more. It has only to back the assumption that the generator behaves like
/// a random one: 4.5 billion records, enough in a trillion deals to show a flaw that made more than a few hundred repeats
/// (and in 100 trillion, more than some 65,000).
const GAMES_DISK_MOST: u64 = 50_000_000_000;
/// The number of deals, and of games: deals with suits and ranks renamed, each game being 4!·13! different deals, since no
/// renaming leaves a deal of 52 different cards as it was.
const DEALS: f64 = 8.065_817_517_094_388e67;
const RENAMINGS: f64 = 149_448_499_200.0;
/// How large a solve record is, about, to warn when --out won't fit (and for `acc run --save-wins` to say).
pub const RECORD_BYTES: u64 = 475;

/// All that a run needs to carry on. The positions and times of the first deals solved on the CPU are in the file of
/// costs beside it, and the records of `--out` in that file: each is cut back to what this counts.
#[derive(Serialize, Deserialize)]
struct Checkpoint {
    version: u32,
    /// The directory the run started in, which its paths are relative to.
    dir: PathBuf,
    args: BatchArgs,
    /// The settings in full, with `--config`, `--gpu-config` and any tables found.
    config: Config,
    settings: Settings,
    /// The GPU's name if it has one, and what the deals were solved on.
    gpu_name: Option<String>,
    hardware: String,
    /// The commits this run has been built from, oldest first.
    commits: Vec<String>,
    sessions: u32,
    /// The time spent on deals, over the sessions.
    elapsed_s: f64,
    standing: Standing,
    tally: Tally,
    proofs: Proofs,
    proofs_pending: Vec<u64>,
    out_bytes: u64,
    games: GamesPlan,
    /// How much of the log of the sample counts, and how far checking it has got.
    games_bytes: u64,
    games_found: Found,
    /// The check for repeated games, once it is made.
    same: Option<Vec<(u64, u64)>>,
}

/// Held for as long as a run has its checkpoint, so that a second process can't take up the same one: it would cut the run's
/// files back to its own idea of them. The system lets go of it however the process ends, so a crash can't leave it stuck;
/// a file left behind by one means nothing until someone holds a lock on it.
struct RunLock {
    file: Option<File>,
    path: PathBuf,
    /// The folders made for it, deepest first, which go with it if nothing else has been put in them.
    made: Vec<PathBuf>,
}

impl RunLock {
    fn take(checkpoint: &Path) -> Result<RunLock, String> {
        let parent = checkpoint.parent().filter(|p| !p.as_os_str().is_empty());
        let made: Vec<PathBuf> = parent
            .into_iter()
            .flat_map(Path::ancestors)
            .take_while(|p| !p.as_os_str().is_empty() && !p.exists())
            .map(Path::to_path_buf)
            .collect();
        if let Some(parent) = parent {
            std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        let path = checkpoint.with_extension("lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
        // `acc status` and the like look at a lock by taking it for a moment, so it is tried again for a second.
        let mut tried = 0;
        let locked = loop {
            match file.try_lock() {
                Err(TryLockError::WouldBlock) if tried < 10 => {
                    tried += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                other => break other,
            }
        };
        match locked {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err("this run is already going in another window".into()),
            // A file system that can't lock is no worse off than before there were locks.
            Err(TryLockError::Error(e)) => {
                eprintln!("note: can't lock {}: {e}; don't start this run twice", path.display());
            }
        }
        Ok(RunLock { file: Some(file), path, made })
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        drop(self.file.take());
        for dir in &self.made {
            if std::fs::remove_dir(dir).is_err() {
                break;
            }
        }
    }
}

/// Refuses what could only go wrong later, or silently: seeds past the last one wrapping round to the first, say.
fn check_args(args: &BatchArgs) -> Result<(), String> {
    if args.count == 0 {
        return Err("--count must be at least 1".into());
    }
    if args.from.checked_add(args.count - 1).is_none() {
        return Err(format!(
            "--from {} with --count {} goes past the last seed there is, {}",
            args.from,
            args.count,
            u64::MAX
        ));
    }
    if let Some(hours) = args.stop_after
        && Duration::try_from_secs_f64(hours * 3600.0).is_err()
    {
        return Err(format!("--stop-after must be a number of hours, 0 or more, not {hours}"));
    }
    numbers::check_gigabytes("--games-memory", args.games_memory)?;
    numbers::check_gigabytes("--prove-memory", args.prove_memory as u64)
}

/// [`COST_SAMPLE_MOST`], or for testing what the environment says instead.
fn cost_sample_most() -> usize {
    std::env::var("ACC_COST_SAMPLE_MOST").ok().and_then(|v| v.parse().ok()).unwrap_or(COST_SAMPLE_MOST)
}

/// A percentage to `places` decimals, or to two figures when it is so small that those would hardly show it.
fn percent(value: f64, places: usize) -> String {
    let smallest_as_before = 10f64.powi(1 - places as i32);
    if value <= 0.0 || value >= smallest_as_before {
        return format!("{value:.places$}");
    }
    let places = (1 - value.log10().floor() as i32) as usize;
    format!("{value:.places$}")
}

/// What has happened to a long run, a line each, with the time: `.events.log` beside the checkpoint, for reading after
/// the terminal's history has gone. Nothing is written for a run with no checkpoint on disk, so short runs leave no trace.
#[derive(Clone)]
struct Events(Option<PathBuf>);

impl Events {
    fn note(&self, what: &str) {
        let Some(checkpoint) = self.0.as_ref().filter(|p| p.exists()) else { return };
        let log = OpenOptions::new().append(true).create(true).open(checkpoint.with_extension("events.log"));
        if let Ok(mut log) = log {
            let _ = writeln!(log, "{} {what}", dossier::now_utc());
        }
    }
}

/// Whether a run has stuck: no deals finished for [`STUCK_AFTER`].
struct Stuck {
    done: usize,
    since: Instant,
}

impl Stuck {
    fn new() -> Stuck {
        Stuck { done: 0, since: Instant::now() }
    }

    fn after(&mut self, done: usize, now: Instant) -> bool {
        if done != self.done {
            self.done = done;
            self.since = now;
        }
        now.duration_since(self.since) >= STUCK_AFTER
    }
}

impl Checkpoint {
    fn load(path: &Path) -> Result<Checkpoint, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let mut checkpoint: Checkpoint = serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} can't be read ({e}): it is damaged, or this checkpoint was made by a different version of acc: finish it with that version, or delete {} to start again",
                path.display(),
                family(path)
            )
        })?;
        let short_records = checkpoint.version == VERSION_WITH_SHORT_RECORDS;
        if checkpoint.version != VERSION && !short_records {
            return Err(format!(
                "{}: this checkpoint was made by a different version of acc (its format is {}, this one's {VERSION}): finish it with that version, or delete {} to start again",
                path.display(),
                checkpoint.version,
                family(path)
            ));
        }
        // Its log would be read 11 bytes to a record, and every one after the first misread.
        if short_records && matches!(checkpoint.games, GamesPlan::Sampled(_)) {
            return Err(format!(
                "{}: made by an older acc, whose log of a sample of the games has 10 bytes a deal; this one writes 11, so it can't carry on with that log: finish the run with the acc that started it, or delete {} to start again",
                path.display(),
                family(path)
            ));
        }
        checkpoint.version = VERSION;
        // Compared as the file system sees them: on Windows a path can be typed in any case.
        let here = std::env::current_dir().and_then(std::fs::canonicalize).map_err(|e| e.to_string())?;
        if std::fs::canonicalize(&checkpoint.dir).ok().as_ref() != Some(&here) {
            return Err(format!("this run started in {}: resume it from there", checkpoint.dir.display()));
        }
        Ok(checkpoint)
    }

    /// Picks up what the prover has finished.
    fn note_proofs(&mut self, proofs: &Mutex<ProofState>) {
        let proofs = proofs.lock().expect("no panics");
        self.proofs = proofs.tally;
        self.proofs_pending = proofs.pending.iter().copied().collect();
    }

    /// Writes the status file: where the run stands, for anything but the run to read. `session` is when this session
    /// started and the deals done by then. It doesn't matter if this fails.
    fn write_status(&self, path: &Path, word: &str, session: (Instant, usize), free: Option<u64>) {
        let (done, count) = (self.standing.done as u64, self.args.count);
        let per_s = (done - session.1 as u64) as f64 / session.0.elapsed().as_secs_f64().max(1e-3);
        let status = serde_json::json!({
            "state": word,
            "updated": dossier::now_utc(),
            "from": self.args.from,
            "count": count,
            "done": done,
            "left": count - done,
            "percent": 100.0 * done as f64 / count.max(1) as f64,
            "deals_per_s_this_session": per_s.round(),
            "deals_per_s_overall": (done as f64 / self.elapsed_s.max(1e-3)).round(),
            "hours_left": if per_s > 0.0 { Some((count - done) as f64 / per_s / 3600.0) } else { None },
            "hours_so_far": self.elapsed_s / 3600.0,
            "sessions": self.sessions,
            "commit": self.commits.last(),
            "hardware": self.hardware,
            // Failures so far, as the progress line shows them (as of the last checkpoint): `lost` is proven lost, and
            // what is neither won nor proven lost stays visible as `proving` (proof in hand), `undecided` or `unverified`.
            "lost": self.proofs.lost,
            "proving": self.proofs_pending.len(),
            "undecided": self.proofs.undecided,
            "unverified": self.tally.failed + self.standing.gpu.failed,
            "proof_failed": self.proofs.failed,
            "not_won_by_solver": self.tally.exhausted + self.tally.unknown,
            "failed_verification": self.tally.failed + self.standing.gpu.failed,
            "proofs": { "lost": self.proofs.lost, "won": self.proofs.won, "undecided": self.proofs.undecided,
                        "failed": self.proofs.failed, "in_hand": self.proofs_pending.len() },
            "gpu": { "won": self.standing.gpu.won, "launches": self.standing.gpu.launches,
                     "attempts": self.standing.gpu.attempts },
            "deals_on_cpu": self.tally.all_cpu_deals,
            "games_log_gb": self.games_bytes as f64 / 1e9,
            "disk_free_gb": free.map(|f| f as f64 / 1e9),
            // As in the report: the solver's wins, and those of the proofs.
            "won": self.tally.won + self.proofs.won,
            "dossiers": (!self.args.no_dossiers).then(|| self.args.dossiers.display().to_string()),
            "save_every_s": self.args.checkpoint_every,
            "checkpoint": path.display().to_string(),
        });
        let text = serde_json::to_string_pretty(&status).expect("serializable") + "\n";
        let (new, file) = (path.with_extension("status.tmp"), path.with_extension("status.json"));
        let _ = std::fs::write(&new, text).and_then(|()| std::fs::rename(&new, &file));
    }

    /// Saves it at `path`: first what it counts (`out`, the log of the sample and the costs of the deals solved on the
    /// CPU), so that it never counts what a crash could lose, then itself, replacing the last one only once it is whole.
    fn save(
        &mut self,
        path: &Path,
        nodes: &[u64],
        micros: &[u64],
        out: Option<&mut BufWriter<File>>,
        games: Option<&mut BufWriter<File>>,
    ) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        if let Some(w) = out {
            let synced =
                w.flush().and_then(|()| w.get_ref().sync_data()).and_then(|()| w.get_ref().metadata());
            self.out_bytes = synced.map_err(|e| format!("writing --out: {e}"))?.len();
        }
        if let Some(w) = games {
            let synced =
                w.flush().and_then(|()| w.get_ref().sync_data()).and_then(|()| w.get_ref().metadata());
            self.games_bytes = synced.map_err(|e| format!("writing the log of the sample: {e}"))?.len();
        }
        let costs = path.with_extension("costs");
        let writing = |e: std::io::Error| format!("writing {}: {e}", costs.display());
        let file =
            OpenOptions::new().write(true).create(true).truncate(false).open(&costs).map_err(writing)?;
        let saved = self.tally.cpu_deals as usize;
        // What is past the deals counted is from a session that ended before it saved, and is written over. Only when there
        // is something to add: the checkpoint on disk may count more than `saved` (one from before the limit, over it), and
        // until the new one has replaced it the file must hold all those.
        let mut w = BufWriter::new(file);
        if nodes.len() > saved {
            w.get_ref().set_len((saved * COST_BYTES) as u64).map_err(writing)?;
            w.seek(SeekFrom::End(0)).map_err(writing)?;
            for (n, us) in nodes[saved..].iter().zip(&micros[saved..]) {
                w.write_all(&n.to_le_bytes())
                    .and_then(|()| w.write_all(&us.to_le_bytes()))
                    .map_err(writing)?;
            }
        }
        w.flush().and_then(|()| w.get_ref().sync_data()).map_err(writing)?;
        self.tally.cpu_deals = nodes.len() as u64;
        let text = serde_json::to_string_pretty(self).expect("serializable") + "\n";
        let new = path.with_extension("tmp");
        File::create(&new)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .and_then(|()| std::fs::rename(&new, path))
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        // Now the checkpoint counts what the file has up to here, and the rest can go. If it can't, it does no harm.
        let _ = w.get_ref().set_len((nodes.len() * COST_BYTES) as u64);
        Ok(())
    }
}

pub fn run(given: &BatchArgs) -> Result<bool, String> {
    let (began, began_at) = (Instant::now(), SystemTime::now());
    check_args(given)?;
    let path = match (&given.resume, &given.checkpoint) {
        (Some(path), _) => Some(path.clone()),
        (None, _) if given.no_checkpoint => None,
        (None, Some(path)) => Some(path.clone()),
        (None, None) => Some(PathBuf::from(format!("checkpoints/batch-{}-{}.json", given.from, given.count))),
    };
    // Before any of the run's files is touched. With no checkpoint to resume, loading it is what says so.
    let _lock =
        path.as_deref().filter(|p| given.resume.is_none() || p.exists()).map(RunLock::take).transpose()?;
    let resumed = given.resume.as_deref().map(Checkpoint::load).transpose()?;
    let is_resumed = resumed.is_some();
    // Only what says nothing about the results may change when a run carries on.
    let args = match &resumed {
        Some(c) => BatchArgs {
            threads: given.threads,
            checkpoint_every: given.checkpoint_every,
            log_dir: given.log_dir.clone(),
            no_log: given.no_log,
            games_memory: given.games_memory,
            no_games_check: given.no_games_check,
            keep_free: given.keep_free,
            until: given.until,
            stop_after: given.stop_after,
            ..c.args.clone()
        },
        None => given.clone(),
    };
    let args = &args;
    if is_resumed {
        check_args(args)?;
    }
    if let Some(path) = path.as_ref().filter(|p| !is_resumed && p.exists()) {
        return Err(format!(
            "a run stopped here is waiting at {0}: carry on with `acc batch --resume {0}`, or delete {1} to start again",
            path.display(),
            family(path)
        ));
    }
    let threads = if args.threads == 0 { runner::all_threads() } else { args.threads };
    let (config, settings) = match &resumed {
        Some(c) => (c.config, c.settings.clone()),
        None => {
            let mut config: Config = match &args.config {
                Some(text) => crate::json_setting("--config", text)?,
                None => Config::default(),
            };
            config.max_nodes = args.max_nodes;
            runner::choose_endgame(&mut config, &args.endgame, &args.endgame_dir, usize::MAX)?;
            let settings: Settings = match &args.gpu_config {
                Some(text) => crate::json_setting("--gpu-config", text)?,
                None => Settings::default(),
            };
            (config, settings)
        }
    };
    let endgame = runner::load_endgame(&args.endgame_dir, config.search.endgame_piles)?;
    let mut state = match resumed {
        Some(c) => c,
        None => Checkpoint {
            version: VERSION,
            dir: std::env::current_dir().map_err(|e| e.to_string())?,
            args: args.clone(),
            config,
            settings: settings.clone(),
            gpu_name: None,
            hardware: String::new(),
            commits: Vec::new(),
            sessions: 0,
            elapsed_s: 0.0,
            standing: Standing::default(),
            tally: Tally::default(),
            proofs: Proofs::default(),
            proofs_pending: Vec::new(),
            out_bytes: 0,
            games: GamesPlan::Off,
            games_bytes: 0,
            games_found: Found::default(),
            same: None,
        },
    };
    let events = Events(path.clone());
    // Only for the status file: a resume takes this from its own command line.
    state.args.checkpoint_every = args.checkpoint_every;
    state.sessions += 1;
    let commit = crate::experiment::commit();
    if state.commits.last() != Some(&commit) {
        if let Some(last) = state.commits.last() {
            eprintln!("note: this run went as far as here on commit {last}, and carries on with {commit}");
        }
        state.commits.push(commit.clone());
    }
    let count = args.count as usize;
    let some_left = state.standing.done < count;
    let gpu = match (is_resumed, some_left) {
        (_, false) => None,
        (true, true) if state.gpu_name.is_some() => {
            match runner::open_gpu_kernel("on", settings, args.gpu_kernel.as_deref(), &args.gpu_define) {
                Ok(gpu) => gpu,
                Err(e) => {
                    // Not there for now, after a driver reset, say: this is as good as its failing during the run.
                    eprintln!("{e}");
                    events.note(&format!("session {} could not open the GPU: {e}", state.sessions));
                    std::process::exit(EXIT_GPU_FAILED);
                }
            }
        }
        (true, true) => None,
        (false, true) => {
            runner::open_gpu_kernel(&args.gpu, settings, args.gpu_kernel.as_deref(), &args.gpu_define)?
        }
    };
    // The GPU's feeder needs a thread of its own, or the GPU waits on it (docs/BENCHMARKS.md, Phase 5 track F).
    let workers = if gpu.is_some() { threads.saturating_sub(1).max(1) } else { threads };
    if some_left {
        state.hardware =
            if gpu.is_some() { format!("{workers} CPU threads + GPU") } else { format!("{workers} threads") };
        state.gpu_name = gpu.as_ref().map(|g| g.name.clone());
    }
    let hardware = state.hardware.clone();
    let cost_most = cost_sample_most();
    let (mut nodes, mut micros) = match (&path, is_resumed) {
        // Grown as needed: only the CPU's deals have them, and they stop at the limit.
        (Some(path), true) => state.tally.load_costs(&path.with_extension("costs"), cost_most)?,
        _ => (Vec::new(), Vec::new()),
    };
    let mut out = match &args.out {
        Some(p) if is_resumed => {
            let writing = |e: std::io::Error| format!("opening {}: {e}", p.display());
            let mut file = OpenOptions::new().write(true).open(p).map_err(writing)?;
            if file.metadata().map_err(writing)?.len() < state.out_bytes {
                let (name, run) = (p.display(), path.as_deref().map_or(String::new(), family));
                return Err(format!(
                    "{name} is shorter than the checkpoint says: put it back, or delete {run} to start again"
                ));
            }
            file.set_len(state.out_bytes).map_err(writing)?;
            file.seek(SeekFrom::End(0)).map_err(writing)?;
            Some(BufWriter::new(file))
        }
        Some(p) => {
            Some(BufWriter::new(File::create(p).map_err(|e| format!("creating {}: {e}", p.display()))?))
        }
        None => None,
    };
    let done_before = state.standing.done;
    let last_seed = args.from + args.count - 1;
    let carrying_on = if is_resumed { format!(", carrying on after {done_before}") } else { String::new() };
    eprintln!(
        "seeds {}..{last_seed} ({} deals{carrying_on}), up to {} positions per deal, {hardware}",
        args.from, args.count, args.max_nodes
    );
    let seeds = match args.plant_repeats {
        Some(every) => Seeds::Repeating { from: args.from, count, every: every.max(2) },
        None => Seeds::Range { from: args.from, count },
    };
    let about = format!(
        "acc batch: seeds {}..{last_seed} ({} deals{carrying_on}), {hardware}, commit {commit}",
        args.from, args.count
    );
    let first = Progress {
        done: done_before,
        gpu_won: state.standing.gpu.won,
        gpu_positions: state.standing.gpu.positions,
        backlog: 0,
        gpu_failed: state.standing.gpu.failed,
    };
    let mut meter =
        Meter::new(seeds.count(), first, (!args.no_log).then_some(args.log_dir.as_path()), &about)?;
    stay_awake();
    let every = Duration::from_secs(args.checkpoint_every.max(1));
    if let Some(path) = &path {
        let pause_file = path.with_extension("pause");
        // One made before this process began was for a session that has ended. One made since is for this one, made
        // while it was starting up (by acc pause, say), and is kept for the watch to find. (A file system may date a file
        // a few milliseconds early, which only a request in the process's first moments could fall foul of.)
        let made = std::fs::metadata(&pause_file).and_then(|m| m.modified());
        if made.is_ok_and(|made| made < began_at) {
            let _ = std::fs::remove_file(&pause_file);
        }
        pause::catch_ctrl_c();
        pause::watch(pause_file.clone());
        if let Some(hours) = args.stop_after {
            pause::after(Duration::from_secs_f64(hours.max(0.0) * 3600.0));
        }
        eprintln!(
            "checkpoint: {}; to pause, press Ctrl+C or create {}",
            path.display(),
            pause_file.display()
        );
    }
    // The check for repeated games: at the end, in memory if it fits, and on disk if it doesn't; of a sample if that won't
    // fit either.
    let on_disk = args.count.saturating_mul(8) > args.games_memory.saturating_mul(1 << 30);
    let work_dir = path.as_deref().and_then(Path::parent).filter(|p| !p.as_os_str().is_empty()).map_or_else(
        || if path.is_some() { PathBuf::from(".") } else { std::env::temp_dir() },
        Path::to_path_buf,
    );
    let games_dir = work_dir.join(format!("games-{}-{}", args.from, args.count));
    let games_log = match &path {
        Some(path) => path.with_extension("games"),
        None => std::env::temp_dir().join(format!("acc-games-{}.log", std::process::id())),
    };
    if !is_resumed {
        state.games = if args.no_games_check {
            GamesPlan::Off
        } else if !on_disk {
            GamesPlan::Whole
        } else {
            std::fs::create_dir_all(&work_dir)
                .map_err(|e| format!("creating {}: {e}", work_dir.display()))?;
            let asked = args.games_disk.map(|gb| (gb * 1e9) as u64);
            let room = asked.unwrap_or_else(|| {
                free_bytes(&work_dir).map_or(GAMES_DISK_MOST, |free| (free / 2).min(GAMES_DISK_MOST))
            });
            let has_room = asked.is_some_and(|room| runner::same_games_disk_bytes(args.count) <= room);
            // Its threads are all the cores', whatever --threads says. However little is allowed, a gigabyte is taken as
            // fitting: the check needs some memory to work in, and small runs nothing like that.
            let memory = runner::same_games_disk_memory(args.count, rayon::current_num_threads());
            let fits = memory <= (args.games_memory << 30).max(1 << 30);
            if has_room && fits {
                GamesPlan::Whole
            } else if args.count > games::MAX_DEALS {
                return Err(format!(
                    "a run of more than {} deals is too long for the check for repeated games: run it in two, or add --no-games-check",
                    games::MAX_DEALS
                ));
            } else {
                if has_room {
                    eprintln!(
                        "check for repeated games: the full check on disk would want about {:.0} GB of memory at once, more than --games-memory {}, so it will be of a sample",
                        memory as f64 / 1e9,
                        args.games_memory
                    );
                }
                GamesPlan::Sampled(Sieve::for_bytes(room, args.count))
            }
        };
    } else if args.no_games_check && state.games != GamesPlan::Off {
        // For good: a log that had missed some deals couldn't be taken up again.
        state.games = GamesPlan::Off;
        let _ = std::fs::remove_file(&games_log);
    }
    let check_games = state.games != GamesPlan::Off && state.same.is_none();
    match state.games {
        GamesPlan::Whole if check_games && on_disk => {
            std::fs::create_dir_all(&work_dir)
                .map_err(|e| format!("creating {}: {e}", work_dir.display()))?;
            warn_of_space(
                &work_dir,
                runner::same_games_disk_bytes(args.count),
                "the check for repeated games at the end",
            );
        }
        GamesPlan::Sampled(sieve) => {
            let room = (sieve.fraction() * args.count as f64) as u64 * games::RECORD as u64;
            eprintln!(
                "check for repeated games: a sample of {}% of the deals, up to {:.0} GB in {}",
                percent(100.0 * sieve.fraction(), 2),
                room as f64 / 1e9,
                games_log.display()
            );
            warn_of_space(&work_dir, room.saturating_sub(state.games_bytes), "the log of the sample");
        }
        _ => {}
    }
    let sieve = match state.games {
        GamesPlan::Sampled(sieve) if some_left => Some(sieve),
        _ => None,
    };
    let mut games_out = match sieve {
        None => None,
        Some(_) if is_resumed => {
            let writing = |e: std::io::Error| format!("opening {}: {e}", games_log.display());
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&games_log)
                .map_err(writing)?;
            if file.metadata().map_err(writing)?.len() < state.games_bytes {
                return Err(format!(
                    "{} is shorter than the checkpoint says: put it back, or carry on without the check by adding --no-games-check",
                    games_log.display()
                ));
            }
            file.set_len(state.games_bytes).map_err(writing)?;
            file.seek(SeekFrom::End(0)).map_err(writing)?;
            Some(BufWriter::new(file))
        }
        Some(_) => {
            std::fs::create_dir_all(&work_dir)
                .map_err(|e| format!("creating {}: {e}", work_dir.display()))?;
            Some(BufWriter::new(
                File::create(&games_log).map_err(|e| format!("creating {}: {e}", games_log.display()))?,
            ))
        }
    };
    if let Some(p) = &args.out {
        let dir = p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        warn_of_space(
            dir,
            (args.count - done_before as u64).saturating_mul(RECORD_BYTES),
            "the solve records of --out",
        );
    }
    let proofs_state = Arc::new(Mutex::new(ProofState {
        tally: state.proofs,
        pending: state.proofs_pending.iter().copied().collect(),
    }));
    let (to_prover, proving) = if args.prove > 0 {
        let (tx, rx) = mpsc::channel();
        for &seed in &state.proofs_pending {
            tx.send(seed).map_err(|_| "the prover thread stopped".to_string())?;
        }
        let limits =
            Limits { positions: args.prove, bytes: args.prove_memory << 30, rng_seed: 1, pattern_piles: 16 };
        let (tables, dir) = (endgame.clone(), (!args.no_dossiers).then(|| args.dossiers.clone()));
        let proofs_state = Arc::clone(&proofs_state);
        let events = events.clone();
        (Some(tx), Some(std::thread::spawn(move || prover(rx, limits, tables, dir, &proofs_state, &events))))
    } else {
        (None, None)
    };
    let start = Instant::now();
    let elapsed_before = state.elapsed_s;
    let mut gpu_error = None;
    let session = (start, done_before);
    let mut no_room = false;
    let mut saves_failed = 0;
    let mut starting =
        Some(format!("session {} started at {done_before} deals, on commit {commit}", state.sessions));
    if is_resumed {
        events.note(starting.take().as_deref().unwrap_or_default());
    }
    let on_the_gpu = gpu.is_some();
    // For the progress line, which can't see `state` while the sink has it: what the sink has counted so far of deals the
    // solver didn't win and lines that failed verification. Set only for the deals the CPU solved, never per GPU win.
    let (not_won, unverified) =
        (Cell::new(state.tally.exhausted + state.tally.unknown), Cell::new(state.tally.failed));
    let mut stuck = Stuck::new();
    if some_left {
        let opts = Options {
            threads: workers,
            table_log2: args.table,
            endgame: endgame.clone(),
            records: out.is_some(),
            gpu,
            from: state.standing.clone(),
            pause: path.is_some().then_some(&pause::PAUSE),
            checkpoint_every: path.is_some().then_some(every),
            pause_if_gpu_fails: path.is_some(),
            sieve,
            ..Options::default()
        };
        let ran = runner::run(
            seeds,
            &config,
            &opts,
            |finished| {
                let s = match finished {
                    Finished::Deal(s) => s,
                    Finished::GpuWins(n) => {
                        state.tally.won += n;
                        state.tally.gpu_stage += n;
                        return Ok(());
                    }
                    Finished::Sampled(records) => {
                        let log = games_out.as_mut().expect("a sample is only asked for with a log");
                        if let Err(e) = log.write_all(&records) {
                            // The last checkpoint stands, and what it counts of the log.
                            let what =
                                format!("writing {}: {e}: make room and carry on", games_log.display());
                            eprintln!("\n{what}");
                            events.note(&what);
                            std::process::exit(EXIT_NO_ROOM);
                        }
                        return Ok(());
                    }
                    Finished::Checkpoint(standing) => {
                        state.standing = standing;
                        state.elapsed_s = elapsed_before + start.elapsed().as_secs_f64();
                        state.note_proofs(&proofs_state);
                        let path = path.as_ref().expect("a checkpoint is only asked for with a path");
                        let free = free_bytes(&work_dir);
                        if free.is_some_and(|free| (free as f64) < args.keep_free * 1e9) && !no_room {
                            let what = format!(
                                "less than {} GB free in {}: pausing; make room and carry on",
                                args.keep_free,
                                work_dir.display()
                            );
                            eprintln!("\n{what}");
                            events.note(&what);
                            no_room = true;
                            pause::ask();
                        }
                        // The last one stands, if something has this one's file open for the moment.
                        match state.save(path, &nodes, &micros, out.as_mut(), games_out.as_mut()) {
                            Ok(()) => {
                                saves_failed = 0;
                                if let Some(text) = starting.take() {
                                    events.note(&text);
                                }
                            }
                            Err(e) => {
                                saves_failed += 1;
                                eprintln!("\ncheckpoint: {e}; trying again next time");
                                events.note(&format!("checkpoint: {e}"));
                                if saves_failed >= SAVES_FAILED_AT_MOST && !no_room {
                                    no_room = true;
                                    pause::ask();
                                }
                            }
                        }
                        state.write_status(path, "running", session, free);
                        return Ok(());
                    }
                };
                let tally = &mut state.tally;
                // Per-deal costs are the CPU solver's; the GPU works in batches and counts its own positions.
                if s.report.stage != Stage::Gpu {
                    // Only while those kept are all there have been, so that they stay the first ones.
                    if nodes.len() < cost_most && nodes.len() as u64 == tally.all_cpu_deals {
                        nodes.push(s.report.nodes);
                        micros.push(s.us);
                    }
                    tally.add_cost(s.report.nodes, s.us);
                    tally.total_nodes += s.report.nodes;
                    if args.gallery > 0 {
                        tally.hardest.push(Reverse((s.report.nodes, s.seed)));
                        if tally.hardest.len() > args.gallery {
                            tally.hardest.pop();
                        }
                    }
                }
                if let Err(e) = &s.verified {
                    tally.failed += 1;
                    eprintln!("\nseed {}: VERIFICATION FAILED: {e}", s.seed);
                    events.note(&format!("seed {}: VERIFICATION FAILED: {e}", s.seed));
                } else {
                    match s.report.verdict {
                        Verdict::Win(_) => {
                            tally.won += 1;
                            if s.report.stage == Stage::Gpu {
                                tally.gpu_stage += 1;
                            } else {
                                *tally.stages.entry(s.report.stage.to_string()).or_default() += 1;
                            }
                        }
                        Verdict::Exhausted => tally.exhausted += 1,
                        Verdict::Unknown => tally.unknown += 1,
                    }
                    if !matches!(s.report.verdict, Verdict::Win(_)) && !args.no_dossiers {
                        let attempt = dossier::Attempt {
                            stage: "batch",
                            config: &Config { seed: s.seed, ..config },
                            threads: 1,
                            report: &s.report,
                            seconds: s.us as f64 / 1e6,
                        };
                        dossier::record(&args.dossiers, &Deal::from_seed(s.seed), Some(s.seed), &attempt)?;
                        tally.any_dossier = true;
                    }
                    if !matches!(s.report.verdict, Verdict::Win(_))
                        && let Some(tx) = &to_prover
                    {
                        proofs_state.lock().expect("no panics").pending.insert(s.seed);
                        tx.send(s.seed).map_err(|_| "the prover thread stopped".to_string())?;
                    }
                }
                not_won.set(tally.exhausted + tally.unknown);
                unverified.set(tally.failed);
                if let Some(w) = out.as_mut()
                    && let Some(line) = s.verified.is_ok().then_some(s.record).flatten()
                {
                    writeln!(w, "{line}").map_err(|e| e.to_string())?;
                }
                Ok(())
            },
            |p| {
                let trouble = {
                    let proofs = proofs_state.lock().expect("no panics");
                    Trouble {
                        not_won: not_won.get(),
                        lost: proofs.tally.lost,
                        proving: proofs.pending.len() as u64,
                        undecided: proofs.tally.undecided,
                        unverified: unverified.get() + p.gpu_failed,
                        proof_failed: proofs.tally.failed,
                        proving_on: args.prove > 0,
                    }
                };
                meter.update(&p, &trouble);
                if args.until.is_some_and(|until| p.done as u64 >= until) {
                    pause::ask();
                }
                // Nothing else would notice a GPU that never answers, and it would hold the run up for good.
                if on_the_gpu && events.0.is_some() && stuck.after(p.done, Instant::now()) {
                    let what =
                        format!("no deal finished in {} minutes: stopping", STUCK_AFTER.as_secs() / 60);
                    eprintln!("\n{what}; the last checkpoint stands");
                    events.note(&what);
                    std::process::exit(if pause::requested() { 0 } else { EXIT_GPU_FAILED });
                }
            },
        )?;
        gpu_error = ran.gpu.as_ref().and_then(|t| t.error.clone());
        state.standing = Standing { done: ran.done, gpu: ran.gpu.unwrap_or_default() };
        state.elapsed_s = elapsed_before + start.elapsed().as_secs_f64();
        eprintln!();
    }
    if state.standing.done < count {
        let Some(path) = &path else { return Err("the run stopped early, with no checkpoint".into()) };
        state.note_proofs(&proofs_state);
        let saved = state.save(path, &nodes, &micros, out.as_mut(), games_out.as_mut());
        if let Err(e) = saved {
            if no_room {
                eprintln!("{e}; the last checkpoint stands");
                events.note(&format!("{e}; the last checkpoint stands"));
                std::process::exit(EXIT_NO_ROOM);
            }
            return Err(e);
        }
        eprintln!(
            "paused after {} of {} deals\ncarry on with: acc batch --resume {}",
            state.standing.done,
            args.count,
            path.display()
        );
        if let Some(text) = starting.take() {
            events.note(&text);
        }
        let free = free_bytes(&work_dir);
        if let Some(e) = &gpu_error {
            events.note(&format!("paused at {} deals: the GPU failed: {e}", state.standing.done));
            state.write_status(path, "paused: the GPU failed", session, free);
            std::process::exit(EXIT_GPU_FAILED);
        }
        if no_room {
            state.write_status(path, "paused: no room", session, free);
            std::process::exit(EXIT_NO_ROOM);
        }
        events.note(&format!("paused at {} deals", state.standing.done));
        state.write_status(path, "paused", session, free);
        return Ok(true);
    }
    if let Some(w) = out.as_mut() {
        // The checkpoint counts these bytes from here on: a resume cuts --out back to the length the checkpoint has, so a pause
        // or a crash while the last proofs and the check run must not leave it at the length before the last records.
        let synced = w.flush().and_then(|()| w.get_ref().sync_data()).and_then(|()| w.get_ref().metadata());
        state.out_bytes = synced.map_err(|e| format!("writing --out: {e}"))?.len();
    }
    drop(out);
    if let Some(w) = games_out.as_mut() {
        let synced = w.flush().and_then(|()| w.get_ref().sync_data()).and_then(|()| w.get_ref().metadata());
        state.games_bytes = synced.map_err(|e| format!("writing {}: {e}", games_log.display()))?.len();
    }
    drop(games_out);
    // The rest can take hours, and it can pause too: it is all done again from here, but for the proofs finished.
    drop(to_prover);
    let pending = proofs_state.lock().expect("no panics").pending.len();
    match pending {
        0 => {}
        1 => eprintln!("finishing the proof of the deal not won"),
        _ => eprintln!("finishing the proofs of the {pending} deals not won"),
    }
    let games_found = Arc::new(Mutex::new(state.games_found.clone()));
    let mut check = check_games.then(|| {
        let sampled = matches!(state.games, GamesPlan::Sampled(_));
        if sampled {
            eprintln!(
                "checking the sample for repeats: {:.0} GB in {}, at most {} GB of it at a time",
                state.games_bytes as f64 / 1e9,
                games_log.display(),
                args.games_memory
            );
        } else if on_disk {
            eprintln!(
                "checking that the deals all differ, on disk: up to {:.0} GB in {} for now",
                runner::same_games_disk_bytes(args.count) as f64 / 1e9,
                games_dir.display()
            );
        }
        let dir = games_dir.clone();
        let (log, bytes, memory, found) =
            (games_log.clone(), state.games_bytes, args.games_memory << 30, Arc::clone(&games_found));
        std::thread::spawn(move || {
            let print = |s| runner::fingerprint(&Deal::from_seed(s));
            if sampled {
                games::check(&log, bytes, seeds, memory, &found)?;
                Ok(found.lock().expect("no panics").same.clone())
            } else if on_disk {
                runner::same_games_on_disk(seeds, print, &dir)
            } else {
                Ok(runner::same_games(seeds, print))
            }
        })
    });
    let mut games_error = None;
    let final_stage = Instant::now();
    let mut unsaved = true;
    loop {
        if let Some(h) = check.take_if(|h| h.is_finished()) {
            match h.join().expect("the check does not panic") {
                Ok(same) => state.same = Some(same),
                Err(e) if path.is_some() => {
                    let path = path.as_ref().expect("just checked");
                    state.note_proofs(&proofs_state);
                    state.save(path, &nodes, &micros, None, None)?;
                    return Err(format!(
                        "the check for repeated games: {e}\nnothing is lost: carry on with `acc batch --resume {}`, adding --no-games-check to skip it",
                        path.display()
                    ));
                }
                Err(e) => games_error = Some(e),
            }
            unsaved = true;
        }
        let proved = proving.as_ref().is_none_or(|h| h.is_finished());
        let before = (state.proofs, state.proofs_pending.len());
        state.note_proofs(&proofs_state);
        unsaved |= before != (state.proofs, state.proofs_pending.len());
        // How far the check of the sample has got, to carry on from there.
        if let Ok(found) = games_found.try_lock()
            && *found != state.games_found
        {
            state.games_found = found.clone();
            unsaved = true;
        }
        let over = check.is_none() && proved;
        // Runs short enough to end here at once leave no files behind.
        let paused = pause::requested();
        if let Some(path) = &path
            && unsaved
            && !over
            && (paused || final_stage.elapsed() >= Duration::from_secs(1))
        {
            state.save(path, &nodes, &micros, None, None)?;
            state.write_status(path, "finishing", session, free_bytes(&work_dir));
            unsaved = false;
        }
        if over {
            break;
        }
        if paused {
            let path = path.as_ref().expect("only a run with a checkpoint pauses");
            eprintln!("paused after all the deals\ncarry on with: acc batch --resume {}", path.display());
            events.note("paused after all the deals");
            state.write_status(path, "paused", session, free_bytes(&work_dir));
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Some(h) = proving {
        h.join().expect("the prover does not panic");
    }
    let proofs = state.proofs;
    let Tally {
        won,
        exhausted,
        unknown,
        failed,
        gpu_stage,
        mut stages,
        total_nodes,
        all_cpu_deals,
        total_micros,
        most_nodes,
        most_micros,
        any_dossier,
        hardest,
        ..
    } = state.tally.clone();
    let elapsed = state.elapsed_s;
    let gpu_totals = state.gpu_name.is_some().then_some(state.standing.gpu.clone());
    let same = state.same.clone();
    if gpu_stage > 0 {
        stages.insert(Stage::Gpu.to_string(), gpu_stage);
    }
    if any_dossier {
        dossier::write_index(&args.dossiers)?;
    }
    if args.gallery > 0 {
        // Re-solve the hardest deals with the same settings (deterministic, so the same lines) to keep their wins.
        let mut top: Vec<(u64, u64)> = hardest.into_iter().map(|Reverse(x)| x).collect();
        top.sort_unstable_by(|a, b| b.cmp(a));
        let mut search = Search::with_options(args.table, config.search);
        search.set_endgame(endgame.clone());
        let mut reports = Vec::new();
        for &(n, seed) in &top {
            search.clear_table();
            let report = solve(
                &Position::from_deal(&Deal::from_seed(seed)),
                &Config { seed, ..config },
                &mut search,
                None,
            );
            if report.nodes != n {
                eprintln!(
                    "note: seed {seed} took {} positions when re-solved for the gallery, {n} in the run",
                    report.nodes
                );
            }
            reports.push((seed, report));
        }
        let entries: Vec<dossier::GalleryEntry> = reports
            .iter()
            .map(|(seed, report)| dossier::GalleryEntry { deal: Deal::from_seed(*seed), seed: *seed, report })
            .collect();
        let cpu_only = if gpu_totals.is_some() {
            format!("; only the {all_cpu_deals} solved on the CPU are ranked")
        } else {
            String::new()
        };
        let run = format!(
            "`acc batch` over seeds {}–{last_seed} ({} deals, commit {}{cpu_only})",
            args.from,
            args.count,
            state.commits.join(", ")
        );
        dossier::write_gallery(&args.dossiers, &entries, &run)?;
        eprintln!(
            "gallery: the {} hardest deals in {}",
            entries.len(),
            args.dossiers.join("gallery").display()
        );
    }

    // The report goes to a file too, when the run was long.
    let mut report = String::new();
    macro_rules! say {
        ($($text:tt)*) => {
            report.push_str(&format!($($text)*));
            report.push('\n');
        };
    }
    let n = args.count;
    // Deals the solver didn't win count as wins once a proof wins them, as losses once proven lost, and both ways
    // in the interval while undecided.
    let undecided = if args.prove > 0 { proofs.undecided + proofs.failed } else { exhausted + unknown };
    if proofs.won > 0 {
        stages.insert("prove".into(), proofs.won);
    }
    let won_by_solver = won;
    let won = won + proofs.won;
    let (lo, _) = wilson(won, n);
    let (_, hi) = wilson(won + undecided, n);
    nodes.sort_unstable();
    micros.sort_unstable();
    let by_stage: Vec<_> = stages.into_iter().collect();
    say!("deals            {n} (seeds {}..{last_seed})", args.from);
    if cfg!(feature = "second-generator") {
        say!("generator        {}: not the deals of the main runs", accordion_core::deal::DEAL_GENERATOR);
    }
    say!(
        "won              {won} (every line replayed: by the reference rules, or a GPU line by the quick check and 1 in 256 by the reference engine too){}",
        if by_stage.len() > 1 || by_stage.first().is_some_and(|(s, _)| s != "thorough") {
            format!(
                "; by stage: {}",
                by_stage.iter().map(|(s, k)| format!("{s} {k}")).collect::<Vec<_>>().join(", ")
            )
        } else {
            String::new()
        }
    );
    say!(
        "not won          {} by the solver ({exhausted} exhausted, {unknown} hit the limit of {} positions)",
        n - won_by_solver - failed,
        args.max_nodes
    );
    if args.prove > 0 {
        say!(
            "proofs           {} proven lost, {} won, {} undecided (two exhaustive runs of up to {} positions each){}",
            proofs.lost,
            proofs.won,
            proofs.undecided,
            args.prove,
            if proofs.failed > 0 { format!("; {} FAILED", proofs.failed) } else { String::new() }
        );
    }
    say!("failed to verify {failed}");
    let in_sample = state.games_bytes / games::RECORD as u64;
    let fraction = in_sample as f64 / n as f64;
    let found = games_found.lock().expect("no panics").clone();
    let pairs = n as f64 * (n as f64 - 1.0) / 2.0;
    say!(
        "repeat odds      {:.1e} that any two of these deals are the same game (suits and ranks renamed), {:.1e} the same deal, if the generator behaves like a random one (n²/2 over 52!/(4!·13!), and over 52!)",
        pairs / (DEALS / RENAMINGS),
        pairs / DEALS
    );
    match &same {
        Some(_) if matches!(state.games, GamesPlan::Sampled(_)) && found.repeats == 0 && in_sample == n => {
            say!("distinct games   all {n} deals differ, even with suits and ranks renamed");
        }
        Some(_) if matches!(state.games, GamesPlan::Sampled(_)) => {
            let chance = format!(
                "{} other pairs share 40 bits of their fingerprints, against {:.0} by chance",
                found.chance,
                games::chance_matches(in_sample)
            );
            if found.repeats == 0 {
                say!(
                    "distinct games   not all checked: no repeats among the {in_sample} deals a hash of the game picked, {}% of the {n}, even with suits and ranks renamed. Any two the same are picked together, so {} or more pairs the same among the {n} would have been seen with 95% chance. {chance}",
                    percent(100.0 * fraction, 3),
                    games::repeats_ruled_out(fraction)
                );
            } else {
                let shown: Vec<String> =
                    found.same.iter().take(20).map(|(a, b)| format!("{a}={b}")).collect();
                say!(
                    "distinct games   {} pairs of seeds deal the same game (up to renamed suits and ranks), among the {in_sample} deals a hash of the game picked, {}% of the {n}: {}. {chance}",
                    found.repeats,
                    percent(100.0 * fraction, 3),
                    shown.join(", ")
                );
            }
        }
        Some(same) if same.is_empty() => {
            say!("distinct games   all {n} deals differ, even with suits and ranks renamed");
        }
        Some(same) => {
            let shown: Vec<String> = same.iter().take(20).map(|(a, b)| format!("{a}={b}")).collect();
            say!(
                "distinct games   {} pairs of seeds deal the same game (up to renamed suits and ranks): {}",
                same.len(),
                shown.join(", ")
            );
        }
        None => match &games_error {
            Some(e) => {
                say!("distinct games   NOT CHECKED: {e}");
            }
            None => {
                say!("distinct games   not checked (--no-games-check)");
            }
        },
    }
    let places = if n >= LONGER_WIN_RATE_FROM { 12 } else { 10 };
    say!(
        "win rate         {:.places$}%   95% CI [{:.places$}%, {:.places$}%] (Wilson; undecided deals counted as losses for the lower bound, wins for the upper)",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    let cpu_deals = all_cpu_deals;
    let on_cpu =
        if gpu_totals.is_some() { format!(" ({cpu_deals} deals solved on the CPU)") } else { String::new() };
    say!(
        "positions/deal   mean {:.0}  median {}  p90 {}  p99 {}  p99.9 {}  max {}{on_cpu}",
        total_nodes as f64 / cpu_deals.max(1) as f64,
        percentile(&nodes, 0.5),
        percentile(&nodes, 0.9),
        percentile(&nodes, 0.99),
        percentile(&nodes, 0.999),
        most_nodes
    );
    say!(
        "ms/deal (1 core) mean {:.2}  median {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.1}{on_cpu}",
        total_micros as f64 / cpu_deals.max(1) as f64 / 1e3,
        percentile(&micros, 0.5) as f64 / 1e3,
        percentile(&micros, 0.9) as f64 / 1e3,
        percentile(&micros, 0.99) as f64 / 1e3,
        percentile(&micros, 0.999) as f64 / 1e3,
        most_micros as f64 / 1e3
    );
    if (nodes.len() as u64) < cpu_deals {
        say!(
            "cost sample      the median and the percentiles above cover the first {} of the {cpu_deals} deals solved on the CPU: deals are random, so these are a fair sample. The means and maxima cover them all",
            nodes.len()
        );
    }
    if let (Some(t), Some(gpu_name)) = (&gpu_totals, &state.gpu_name) {
        say!(
            "GPU              {}: won {} of the {} deals it was given ({:.2}%), {} lines failed verification; {:.1}M attempts, {:.0}M positions/s{}",
            gpu_name,
            t.won,
            t.deals,
            100.0 * t.won as f64 / t.deals.max(1) as f64,
            t.failed,
            t.attempts as f64 / 1e6,
            t.positions as f64 / elapsed / 1e6,
            t.error.as_ref().map_or(String::new(), |e| format!("; STOPPED EARLY: {e}"))
        );
        let c = t.cpu;
        let share = |x: u64| 100.0 * x as f64 / c.total.max(1) as f64;
        let rest = c.total.saturating_sub(c.verifying + c.misses + c.own + c.waiting + c.handing_on);
        say!(
            "CPU threads      verifying GPU lines {:.1}% ({:.1} us a line), the GPU's misses {:.1}%, own deals {:.1}%, waiting for the GPU {:.1}%, handing results on {:.1}%, other {:.1}%",
            share(c.verifying),
            c.verifying as f64 / t.won.max(1) as f64,
            share(c.misses),
            share(c.own),
            share(c.waiting),
            share(c.handing_on),
            share(rest)
        );
    }
    let sessions =
        if state.sessions > 1 { format!(", over {} sessions", state.sessions) } else { String::new() };
    say!(
        "throughput       {:.0} deals/s, {:.0}M CPU positions/s on {hardware} ({elapsed:.1} s wall{sessions})",
        n as f64 / elapsed,
        total_nodes as f64 / elapsed / 1e6
    );
    if state.commits.len() > 1 {
        say!("commits          {}", state.commits.join(", "));
    }
    print!("{report}");
    if let Some(path) = &path {
        // Only now can the checkpoint go. If the report can't be saved, it stays, and carrying on makes the report again.
        let report_file = path.with_extension("report.txt");
        let saved = if began.elapsed() >= every || is_resumed {
            let saved = std::fs::write(&report_file, &report);
            match &saved {
                Ok(()) => eprintln!("report: {}", report_file.display()),
                Err(e) => eprintln!("report: writing {}: {e}; the checkpoint stays", report_file.display()),
            }
            saved.is_ok()
        } else {
            true
        };
        if saved {
            if began.elapsed() >= every || is_resumed {
                events.note(&format!("finished: {} deals, report in {}", args.count, report_file.display()));
                state.write_status(path, "finished", session, free_bytes(&work_dir));
            } else {
                // A run this short leaves nothing behind.
                let _ = std::fs::remove_file(path.with_extension("status.json"));
                let _ = std::fs::remove_file(path.with_extension("events.log"));
            }
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(&games_log);
            let _ = std::fs::remove_file(path.with_extension("costs"));
            let _ = std::fs::remove_file(path.with_extension("tmp"));
            let _ = std::fs::remove_dir_all(&games_dir);
        }
    }
    if path.is_none() {
        let _ = std::fs::remove_file(&games_log);
    }
    Ok(failed == 0 && proofs.failed == 0 && gpu_totals.is_none_or(|t| t.failed == 0))
}

/// Warns if `dir`'s disk hasn't `needed` bytes free for `what`.
fn warn_of_space(dir: &Path, needed: u64, what: &str) {
    if let Some(free) = free_bytes(dir)
        && free < needed
    {
        eprintln!(
            "warning: {what} will want about {:.0} GB in {}, and only {:.0} GB are free there now",
            needed as f64 / 1e9,
            dir.display(),
            free as f64 / 1e9
        );
    }
}

/// The bytes free on `dir`'s disk, if it can tell.
#[cfg(windows)]
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, free: *mut u64, total: *mut u64, all_free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0;
    // SAFETY: a null-terminated path and one place for the answer; the other two may be null.
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let output = std::process::Command::new("df").args(["-Pk", "--"]).arg(dir).output().ok()?;
    output.status.success().then(|| df_free_bytes(&String::from_utf8_lossy(&output.stdout))).flatten()
}

/// The bytes free, from what `df -Pk` prints: a heading, then `name blocks used free percent mount`, in which the name and
/// the mount may have spaces in them. So the free blocks are found as what comes before the percentage.
#[cfg(any(not(windows), test))]
fn df_free_bytes(text: &str) -> Option<u64> {
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let percent = fields.iter().position(|f| f.ends_with('%'))?;
    let kilobytes: u64 = fields.get(percent.checked_sub(1)?)?.parse().ok()?;
    kilobytes.checked_mul(1024)
}

/// Keeps the computer from going to sleep while this thread (the run's) lives, as a video player does; the screen
/// may still turn off, and closing the lid still sleeps.
#[cfg(windows)]
fn stay_awake() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    // SAFETY: takes flags only; the request ends when this thread exits. It returns the previous state, 0 on failure.
    if unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } == 0 {
        eprintln!("note: Windows refused to stay awake for the run; it may go to sleep");
    }
}

#[cfg(not(windows))]
fn stay_awake() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stuck_means_no_deals_for_the_whole_time() {
        let mut stuck = Stuck::new();
        let t = stuck.since;
        assert!(!stuck.after(0, t + STUCK_AFTER / 2));
        assert!(stuck.after(0, t + STUCK_AFTER));
        // A deal finishing starts the time again.
        assert!(!stuck.after(1, t + STUCK_AFTER * 2));
        assert!(!stuck.after(1, t + STUCK_AFTER * 2 + STUCK_AFTER / 2));
        assert!(stuck.after(1, t + STUCK_AFTER * 3));
    }

    #[test]
    fn df_is_read_however_the_disk_is_named() {
        let heading = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n";
        let linux = format!("{heading}/dev/sda1        41152736  20000000  19035256      52% /\n");
        assert_eq!(df_free_bytes(&linux), Some(19_035_256 * 1024));
        let spaces = format!("{heading}//server/my share 100 90 10 90% /Volumes/My Disk\n");
        assert_eq!(df_free_bytes(&spaces), Some(10 * 1024));
        assert_eq!(df_free_bytes(heading), None);
        assert_eq!(df_free_bytes(""), None);
        assert_eq!(df_free_bytes(&format!("{heading}proc 0 0 0 - /proc\n")), None);
    }

    #[test]
    fn small_percentages_keep_two_figures() {
        // As they always were.
        assert_eq!(percent(45.4545, 2), "45.45");
        assert_eq!(percent(0.1, 2), "0.10");
        assert_eq!(percent(0.455, 3), "0.455");
        assert_eq!(percent(0.0, 2), "0.00");
        // At 100 trillion deals and 50 GB, which used to read 0.00.
        assert_eq!(percent(0.004545, 2), "0.0045");
        assert_eq!(percent(0.004545, 3), "0.0045");
        assert_eq!(percent(0.00000123, 2), "0.0000012");
    }

    fn write_costs(path: &Path, costs: &[(u64, u64)]) {
        let bytes: Vec<u8> =
            costs.iter().flat_map(|(n, us)| [n.to_le_bytes(), us.to_le_bytes()].concat()).collect();
        std::fs::write(path, bytes).unwrap();
    }

    fn costs_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("acc-costs-test-{}-{name}.costs", std::process::id()))
    }

    #[test]
    fn a_checkpoint_without_totals_gets_them_from_the_whole_file() {
        let file = costs_file("old");
        write_costs(&file, &[(5, 50), (9, 10), (2, 70), (7, 20)]);
        // As it comes from a checkpoint that hasn't the new fields.
        let mut tally: Tally = serde_json::from_value(serde_json::json!({
            "won": 4, "exhausted": 0, "unknown": 0, "failed": 0, "gpu_stage": 0, "stages": {}, "total_nodes": 23,
            "cpu_deals": 4, "any_dossier": false, "hardest": []
        }))
        .unwrap();
        let (nodes, micros) = tally.load_costs(&file, 100).unwrap();
        assert_eq!((nodes, micros), (vec![5, 9, 2, 7], vec![50, 10, 70, 20]));
        assert_eq!((tally.cpu_deals, tally.all_cpu_deals), (4, 4));
        assert_eq!((tally.total_micros, tally.most_nodes, tally.most_micros), (150, 9, 70));

        // Over the limit: the first ones are kept, and the totals still count every one.
        let mut tally = Tally { cpu_deals: 4, ..Tally::default() };
        let (nodes, micros) = tally.load_costs(&file, 2).unwrap();
        assert_eq!((nodes, micros), (vec![5, 9], vec![50, 10]));
        assert_eq!((tally.cpu_deals, tally.all_cpu_deals), (2, 4));
        assert_eq!((tally.total_micros, tally.most_nodes, tally.most_micros), (150, 9, 70));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn a_checkpoint_with_totals_keeps_them_and_reads_no_more_than_it_keeps() {
        let file = costs_file("new");
        write_costs(&file, &[(5, 50), (9, 10)]);
        let mut tally = Tally {
            cpu_deals: 2,
            all_cpu_deals: 1_000,
            total_micros: 123_456,
            most_nodes: 77,
            most_micros: 88,
            ..Tally::default()
        };
        let (nodes, _) = tally.load_costs(&file, 100).unwrap();
        assert_eq!(nodes, [5, 9]);
        assert_eq!(
            (tally.all_cpu_deals, tally.total_micros, tally.most_nodes, tally.most_micros),
            (1_000, 123_456, 77, 88)
        );
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn no_file_of_costs_is_needed_for_no_costs() {
        let mut tally = Tally::default();
        assert_eq!(tally.load_costs(Path::new("no-such-file.costs"), 100), Ok((Vec::new(), Vec::new())));
    }
}
