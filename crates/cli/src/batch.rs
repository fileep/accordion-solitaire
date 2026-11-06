//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time, and on the GPU when
//! there is one (PLAN.md Phases 4, 5 and 8). Every win is verified by the reference rules before it counts; every
//! deal that isn't won gets a dossier, and a proof attempt on a thread of its own while the run goes on: two
//! exhaustive runs that must agree before a loss counts ([`prove::prove_both`]). The summary gives the win rate with
//! a Wilson 95% interval, counting undecided deals both ways, and checks that the deals are all different games.
//!
//! A long run can stop and carry on. Results are applied in seed order, so where a run stands is one number, and every
//! minute that number and all the totals go to a checkpoint (written whole or not at all, after the results it counts
//! are safely on disk). Ctrl+C or a file beside the checkpoint pauses the run; a crash costs the last minute;
//! `--resume` carries on from either, with the settings the run began with. At the end the checkpoint gives way to the
//! report, saved beside it.

use crate::dossier;
use crate::pause;
use crate::progress::Meter;
use crate::prove::{self, Finding, Limits};
use crate::runner::{self, Finished, Options, Progress, Seeds, Standing, percentile, wilson};
use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_gpu::Settings;
use accordion_solver::{Config, Endgame, Search, Stage, Verdict, solve};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Args, Clone, Serialize, Deserialize)]
pub struct BatchArgs {
    /// First seed.
    #[arg(long, default_value_t = 0)]
    from: u64,
    /// Number of consecutive seeds.
    #[arg(long, default_value_t = 1000)]
    count: u64,
    /// Positions one deal may search (on one thread) before it counts as not won.
    #[arg(long, default_value_t = 50_000_000)]
    max_nodes: u64,
    /// Solver settings as JSON (see `accordion_solver::Config`), e.g. '{"ladder": {"start": 36}}'. Fields left out
    /// keep their defaults; --max-nodes overrides `max_nodes`.
    #[arg(long)]
    config: Option<String>,
    /// Write one verified solve record per deal here (JSON Lines, compact moves), in seed order.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where deals that aren't won get their dossiers.
    #[arg(long, default_value = "hard-deals")]
    dossiers: PathBuf,
    /// Don't write dossiers.
    #[arg(long)]
    no_dossiers: bool,
    /// Dead-position table per thread, as log2 of 64-byte buckets (14 = 1 MB; small tables that stay in the core's
    /// own cache were fastest, see docs/BENCHMARKS.md).
    #[arg(long, default_value_t = 14)]
    table: u32,
    /// Threads (0 = every core); with the GPU, one of them feeds it.
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// GPU ladder attempts in front of the CPU solver: `auto` (use an NVIDIA GPU if there is one), `on` or `off`.
    /// Which deals the GPU wins, and its lines, depend on timing; every line is still verified.
    #[arg(long, default_value = "auto")]
    gpu: String,
    /// GPU settings as JSON (see `accordion_gpu::Settings`), e.g. '{"budget": 4000}'.
    #[arg(long)]
    gpu_config: Option<String>,
    /// Directory holding endgame tables (`acc endgame build`).
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Endgame tables to use: a pile count, `off`, or `auto` (the largest found in --endgame-dir).
    #[arg(long, default_value = "auto")]
    endgame: String,
    /// Keep this run's N hardest deals (most positions) as verified solve files in <dossiers>/gallery/, replacing
    /// the gallery there, and list them in its INDEX.md. With the GPU, only deals solved on the CPU are ranked.
    #[arg(long, default_value_t = 0)]
    gallery: usize,
    /// Where the speed log goes: a CSV row a second of how fast the run is going, in batch-<UTC time>Z.csv.
    #[arg(long, default_value = "logs")]
    log_dir: PathBuf,
    /// Don't write the speed log.
    #[arg(long)]
    no_log: bool,
    /// Positions each of the two exhaustive runs may generate when proving a deal the solver didn't win (0: don't
    /// prove). Proofs run one at a time on a thread of their own while the run goes on, and the run waits for the
    /// last; one can take an hour.
    #[arg(long, default_value_t = prove::DEFAULT_POSITIONS)]
    prove: u64,
    /// Memory each proof run's set of dead positions may take, in GB.
    #[arg(long, default_value_t = 8)]
    prove_memory: usize,
    /// Where to keep the state that lets this run pause and carry on, with the run's other files beside it. Default
    /// checkpoints/batch-<from>-<count>.json. Ctrl+C pauses the run (a second one stops it at once), and so does a file
    /// named like the checkpoint but ending .pause.
    #[arg(long)]
    checkpoint: Option<PathBuf>,
    /// Keep no checkpoint: the run can't pause, and if the GPU fails the CPU solves the rest.
    #[arg(long, conflicts_with_all = ["checkpoint", "resume"])]
    no_checkpoint: bool,
    /// How often to save the checkpoint, in seconds.
    #[arg(long, default_value_t = 60)]
    checkpoint_every: u64,
    /// Carry on with the paused or crashed run in this checkpoint, from the directory it started in, with the settings
    /// it started with: only --threads, --checkpoint-every, --log-dir, --no-log, --games-memory and --no-games-check
    /// may be given.
    #[arg(long, conflicts_with_all = [
        "from", "count", "max_nodes", "config", "out", "dossiers", "no_dossiers", "table", "gpu", "gpu_config",
        "endgame_dir", "endgame", "gallery", "prove", "prove_memory", "checkpoint",
    ])]
    resume: Option<PathBuf>,
    /// Memory the check that the deals are all different games may take, in GB; a run too large for it works from
    /// disk, at about 5 bytes a deal (in the checkpoint's directory, or the system's temporary one without a
    /// checkpoint), and it takes longer.
    #[arg(long, default_value_t = 16)]
    games_memory: u64,
    /// Skip the check that the deals are all different games.
    #[arg(long)]
    no_games_check: bool,
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
) {
    for seed in seeds {
        let mut found = Proofs::default();
        prove_one(seed, &limits, tables.as_deref(), dossiers.as_deref(), &mut found);
        let mut state = state.lock().expect("no panics");
        state.tally.lost += found.lost;
        state.tally.won += found.won;
        state.tally.undecided += found.undecided;
        state.tally.failed += found.failed;
        state.pending.remove(&seed);
    }
}

fn prove_one(seed: u64, limits: &Limits, tables: Option<&Endgame>, dossiers: Option<&Path>, tally: &mut Proofs) {
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
            std::fs::create_dir_all(&proofs)
                .map_err(|e| format!("creating {}: {e}", proofs.display()))?;
            let path = proofs.join(format!("seed-{seed}-loss.json"));
            std::fs::write(&path, record.to_json() + "\n")
                .map_err(|e| format!("writing {}: {e}", path.display()))
        });
    if let Err(e) = written {
        eprintln!("seed {seed}: {e}");
    }
}

/// The run's tallies of the deals handed on so far.
#[derive(Default, Serialize, Deserialize)]
struct Tally {
    won: u64,
    exhausted: u64,
    unknown: u64,
    failed: u64,
    /// Wins on the GPU; the other stages' are in `stages`.
    gpu_stage: u64,
    stages: BTreeMap<String, u64>,
    total_nodes: u64,
    /// Deals solved on the CPU, whose positions and times are in the file of costs.
    cpu_deals: u64,
    /// Whether any dossier was written.
    any_dossier: bool,
    hardest: BinaryHeap<Reverse<(u64, u64)>>,
}

/// A checkpoint's format.
const VERSION: u32 = 1;
/// The bytes of a deal in the file of costs: its positions, and its microseconds.
const COST_BYTES: usize = 16;
/// The exit code of a run that paused because the GPU failed: try again, as a new process may well get the GPU back.
const EXIT_GPU_FAILED: i32 = 75;
/// How large a solve record is, about, to warn when --out won't fit.
const RECORD_BYTES: u64 = 350;

/// All that a run needs to carry on. The positions and times of the deals solved on the CPU, two million or so, are in
/// the file of costs beside it, and the records of `--out` in that file: each is cut back to what this counts.
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
    /// The check for repeated games, once it is made.
    same: Option<Vec<(u64, u64)>>,
}

impl Checkpoint {
    fn load(path: &Path) -> Result<Checkpoint, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let checkpoint: Checkpoint =
            serde_json::from_str(&text).map_err(|e| format!("reading {}: {e}", path.display()))?;
        if checkpoint.version != VERSION {
            return Err(format!("{}: format {}, not {VERSION}", path.display(), checkpoint.version));
        }
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

    /// Saves it at `path`: first what it counts (`out`, and the costs of the deals solved on the CPU), so that it never
    /// counts what a crash could lose, then itself, replacing the last one only once it is whole.
    fn save(
        &mut self,
        path: &Path,
        nodes: &[u64],
        micros: &[u64],
        out: Option<&mut BufWriter<File>>,
    ) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        if let Some(w) = out {
            let synced = w.flush().and_then(|()| w.get_ref().sync_data()).and_then(|()| w.get_ref().metadata());
            self.out_bytes = synced.map_err(|e| format!("writing --out: {e}"))?.len();
        }
        let costs = path.with_extension("costs");
        let writing = |e: std::io::Error| format!("writing {}: {e}", costs.display());
        let file = OpenOptions::new().write(true).create(true).truncate(false).open(&costs).map_err(writing)?;
        let saved = self.tally.cpu_deals as usize;
        file.set_len((saved * COST_BYTES) as u64).map_err(writing)?;
        let mut w = BufWriter::new(file);
        w.seek(SeekFrom::End(0)).map_err(writing)?;
        for (n, us) in nodes[saved..].iter().zip(&micros[saved..]) {
            w.write_all(&n.to_le_bytes()).and_then(|()| w.write_all(&us.to_le_bytes())).map_err(writing)?;
        }
        w.flush().and_then(|()| w.get_ref().sync_data()).map_err(writing)?;
        self.tally.cpu_deals = nodes.len() as u64;
        let text = serde_json::to_string_pretty(self).expect("serializable") + "\n";
        let new = path.with_extension("tmp");
        File::create(&new)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .and_then(|()| std::fs::rename(&new, path))
            .map_err(|e| format!("writing {}: {e}", path.display()))
    }

    /// The positions and times of the deals solved on the CPU.
    fn load_costs(&self, path: &Path) -> Result<(Vec<u64>, Vec<u64>), String> {
        let costs = path.with_extension("costs");
        let reading = |e: std::io::Error| format!("reading {}: {e}", costs.display());
        let deals = self.tally.cpu_deals as usize;
        let mut bytes = vec![0u8; deals * COST_BYTES];
        if deals > 0 {
            File::open(&costs).and_then(|mut f| f.read_exact(&mut bytes)).map_err(reading)?;
        }
        let word = |c: &[u8]| u64::from_le_bytes(c.try_into().expect("8 bytes"));
        let (entries, _) = bytes.as_chunks::<COST_BYTES>();
        let nodes = entries.iter().map(|c| word(&c[..8])).collect();
        let micros = entries.iter().map(|c| word(&c[8..])).collect();
        Ok((nodes, micros))
    }
}

pub fn run(given: &BatchArgs) -> Result<bool, String> {
    let began = Instant::now();
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
            ..c.args.clone()
        },
        None => given.clone(),
    };
    let args = &args;
    let path = match (&given.resume, &args.checkpoint) {
        (Some(path), _) => Some(path.clone()),
        (None, _) if args.no_checkpoint => None,
        (None, Some(path)) => Some(path.clone()),
        (None, None) => Some(PathBuf::from(format!("checkpoints/batch-{}-{}.json", args.from, args.count))),
    };
    if let Some(path) = path.as_ref().filter(|p| !is_resumed && p.exists()) {
        return Err(format!(
            "a run stopped here is waiting at {0}: carry on with `acc batch --resume {0}`, or delete it (and the files \
             beside it) to start again",
            path.display()
        ));
    }
    let threads = if args.threads == 0 { runner::all_threads() } else { args.threads };
    let (config, settings) = match &resumed {
        Some(c) => (c.config, c.settings.clone()),
        None => {
            let mut config: Config = match &args.config {
                Some(text) => serde_json::from_str(text).map_err(|e| format!("--config: {e}"))?,
                None => Config::default(),
            };
            config.max_nodes = args.max_nodes;
            runner::choose_endgame(&mut config, &args.endgame, &args.endgame_dir, usize::MAX)?;
            let settings: Settings = match &args.gpu_config {
                Some(text) => serde_json::from_str(text).map_err(|e| format!("--gpu-config: {e}"))?,
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
            same: None,
        },
    };
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
        (true, true) if state.gpu_name.is_some() => runner::open_gpu("on", settings)?,
        (true, true) => None,
        (false, true) => runner::open_gpu(&args.gpu, settings)?,
    };
    // The GPU's feeder needs a thread of its own, or the GPU waits on it (docs/BENCHMARKS.md, Phase 5 track F).
    let workers = if gpu.is_some() { threads.saturating_sub(1).max(1) } else { threads };
    if some_left {
        state.hardware =
            if gpu.is_some() { format!("{workers} CPU threads + GPU") } else { format!("{workers} threads") };
        state.gpu_name = gpu.as_ref().map(|g| g.name.clone());
    }
    let hardware = state.hardware.clone();
    let (mut nodes, mut micros) = match (&path, is_resumed) {
        // Grown as needed: only the CPU's deals have them, and a billion deals' worth is 16 GB.
        (Some(path), true) => state.load_costs(path)?,
        _ => (Vec::new(), Vec::new()),
    };
    let mut out = match &args.out {
        Some(p) if is_resumed => {
            let writing = |e: std::io::Error| format!("opening {}: {e}", p.display());
            let mut file = OpenOptions::new().write(true).open(p).map_err(writing)?;
            if file.metadata().map_err(writing)?.len() < state.out_bytes {
                return Err(format!("{} is shorter than the checkpoint says", p.display()));
            }
            file.set_len(state.out_bytes).map_err(writing)?;
            file.seek(SeekFrom::End(0)).map_err(writing)?;
            Some(BufWriter::new(file))
        }
        Some(p) => Some(BufWriter::new(
            File::create(p).map_err(|e| format!("creating {}: {e}", p.display()))?,
        )),
        None => None,
    };
    let done_before = state.standing.done;
    let last_seed = args.from + args.count - 1;
    let carrying_on = if is_resumed { format!(", carrying on after {done_before}") } else { String::new() };
    eprintln!(
        "seeds {}..{last_seed} ({} deals{carrying_on}), up to {} positions per deal, {hardware}",
        args.from, args.count, args.max_nodes
    );
    let seeds = Seeds::Range { from: args.from, count };
    let about = format!(
        "acc batch: seeds {}..{last_seed} ({} deals{carrying_on}), {hardware}, commit {commit}",
        args.from, args.count
    );
    let first = Progress {
        done: done_before,
        gpu_won: state.standing.gpu.won,
        gpu_positions: state.standing.gpu.positions,
        backlog: 0,
    };
    let mut meter = Meter::new(seeds.count(), first, (!args.no_log).then_some(args.log_dir.as_path()), &about)?;
    stay_awake();
    let every = Duration::from_secs(args.checkpoint_every.max(1));
    if let Some(path) = &path {
        let pause_file = path.with_extension("pause");
        let _ = std::fs::remove_file(&pause_file);
        pause::catch_ctrl_c();
        pause::watch(pause_file.clone());
        eprintln!(
            "checkpoint: {}; to pause, press Ctrl+C or create {}",
            path.display(),
            pause_file.display()
        );
    }
    // The check for repeated games, at the end, in memory if it fits.
    let check_games = !args.no_games_check && state.same.is_none();
    let on_disk = args.count.saturating_mul(8) > args.games_memory.saturating_mul(1 << 30);
    let work_dir = path
        .as_deref()
        .and_then(Path::parent)
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| if path.is_some() { PathBuf::from(".") } else { std::env::temp_dir() }, Path::to_path_buf);
    let games_dir = work_dir.join(format!("games-{}-{}", args.from, args.count));
    if check_games && on_disk {
        std::fs::create_dir_all(&work_dir).map_err(|e| format!("creating {}: {e}", work_dir.display()))?;
        warn_of_space(&work_dir, runner::same_games_disk_bytes(args.count), "the check for repeated games at the end");
    }
    if let Some(p) = &args.out {
        let dir = p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        warn_of_space(dir, (args.count - done_before as u64) * RECORD_BYTES, "the solve records of --out");
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
        (Some(tx), Some(std::thread::spawn(move || prover(rx, limits, tables, dir, &proofs_state))))
    } else {
        (None, None)
    };
    let start = Instant::now();
    let elapsed_before = state.elapsed_s;
    let mut gpu_error = None;
    if some_left {
        let opts = Options {
            threads: workers,
            table_log2: args.table,
            endgame: endgame.clone(),
            records: out.is_some(),
            each_gpu_win: false,
            gpu,
            from: state.standing.clone(),
            pause: path.is_some().then_some(&pause::PAUSE),
            checkpoint_every: path.is_some().then_some(every),
            pause_if_gpu_fails: path.is_some(),
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
                    Finished::Checkpoint(standing) => {
                        state.standing = standing;
                        state.elapsed_s = elapsed_before + start.elapsed().as_secs_f64();
                        state.note_proofs(&proofs_state);
                        let path = path.as_ref().expect("a checkpoint is only asked for with a path");
                        // The last one stands, if something has this one's file open for the moment.
                        if let Err(e) = state.save(path, &nodes, &micros, out.as_mut()) {
                            eprintln!("\ncheckpoint: {e}; trying again next time");
                        }
                        return Ok(());
                    }
                };
                let tally = &mut state.tally;
                // Per-deal costs are the CPU solver's; the GPU works in batches and counts its own positions.
                if s.report.stage != Stage::Gpu {
                    nodes.push(s.report.nodes);
                    micros.push(s.us);
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
                if let Some(w) = out.as_mut()
                    && let Some(line) = s.verified.is_ok().then_some(s.record).flatten()
                {
                    writeln!(w, "{line}").map_err(|e| e.to_string())?;
                }
                Ok(())
            },
            |p| meter.update(&p),
        )?;
        gpu_error = ran.gpu.as_ref().and_then(|t| t.error.clone());
        state.standing = Standing { done: ran.done, gpu: ran.gpu.unwrap_or_default() };
        state.elapsed_s = elapsed_before + start.elapsed().as_secs_f64();
        eprintln!();
    }
    if state.standing.done < count {
        let Some(path) = &path else { return Err("the run stopped early, with no checkpoint".into()) };
        state.note_proofs(&proofs_state);
        state.save(path, &nodes, &micros, out.as_mut())?;
        eprintln!(
            "paused after {} of {} deals\ncarry on with: acc batch --resume {}",
            state.standing.done,
            args.count,
            path.display()
        );
        if gpu_error.is_some() {
            std::process::exit(EXIT_GPU_FAILED);
        }
        return Ok(true);
    }
    if let Some(w) = out.as_mut() {
        w.flush().map_err(|e| e.to_string())?;
    }
    drop(out);
    // The rest can take hours, and it can pause too: it is all done again from here, but for the proofs finished.
    drop(to_prover);
    let pending = proofs_state.lock().expect("no panics").pending.len();
    match pending {
        0 => {}
        1 => eprintln!("finishing the proof of the deal not won"),
        _ => eprintln!("finishing the proofs of the {pending} deals not won"),
    }
    let mut check = check_games.then(|| {
        if on_disk {
            eprintln!(
                "checking that the deals all differ, on disk: up to {:.0} GB in {} for now",
                runner::same_games_disk_bytes(args.count) as f64 / 1e9,
                games_dir.display()
            );
        }
        let dir = games_dir.clone();
        std::thread::spawn(move || {
            let print = |s| runner::fingerprint(&Deal::from_seed(s));
            if on_disk { runner::same_games_on_disk(seeds, print, &dir) } else { Ok(runner::same_games(seeds, print)) }
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
                    state.save(path, &nodes, &micros, None)?;
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
        let over = check.is_none() && proved;
        // Runs short enough to end here at once leave no files behind.
        let paused = pause::requested();
        if let Some(path) = &path
            && unsaved
            && !over
            && (paused || final_stage.elapsed() >= Duration::from_secs(1))
        {
            state.save(path, &nodes, &micros, None)?;
            unsaved = false;
        }
        if over {
            break;
        }
        if paused {
            let path = path.as_ref().expect("only a run with a checkpoint pauses");
            eprintln!("paused after all the deals\ncarry on with: acc batch --resume {}", path.display());
            std::process::exit(0);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Some(h) = proving {
        h.join().expect("the prover does not panic");
    }
    let proofs = state.proofs;
    let Tally { won, exhausted, unknown, failed, gpu_stage, mut stages, total_nodes, any_dossier, hardest, .. } = state.tally;
    let elapsed = state.elapsed_s;
    let gpu_totals = state.gpu_name.is_some().then_some(state.standing.gpu);
    let same = state.same;
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
            format!("; only the {} solved on the CPU are ranked", nodes.len())
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
    say!(
        "won              {won} (every one verified by the reference rules){}",
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
    match &same {
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
    say!(
        "win rate         {:.10}%   95% CI [{:.10}%, {:.10}%] (Wilson; undecided deals counted as losses for the lower bound, wins for the upper)",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    let cpu_deals = nodes.len();
    let on_cpu =
        if gpu_totals.is_some() { format!(" ({cpu_deals} deals solved on the CPU)") } else { String::new() };
    say!(
        "positions/deal   mean {:.0}  median {}  p90 {}  p99 {}  p99.9 {}  max {}{on_cpu}",
        total_nodes as f64 / cpu_deals.max(1) as f64,
        percentile(&nodes, 0.5),
        percentile(&nodes, 0.9),
        percentile(&nodes, 0.99),
        percentile(&nodes, 0.999),
        nodes.last().copied().unwrap_or(0)
    );
    say!(
        "ms/deal (1 core) mean {:.2}  median {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.1}{on_cpu}",
        micros.iter().sum::<u64>() as f64 / cpu_deals.max(1) as f64 / 1e3,
        percentile(&micros, 0.5) as f64 / 1e3,
        percentile(&micros, 0.9) as f64 / 1e3,
        percentile(&micros, 0.99) as f64 / 1e3,
        percentile(&micros, 0.999) as f64 / 1e3,
        micros.last().copied().unwrap_or(0) as f64 / 1e3
    );
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
    let sessions = if state.sessions > 1 { format!(", over {} sessions", state.sessions) } else { String::new() };
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
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(path.with_extension("costs"));
            let _ = std::fs::remove_file(path.with_extension("tmp"));
            let _ = std::fs::remove_dir_all(&games_dir);
        }
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
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, free: *mut u64, total: *mut u64, all_free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0;
    // SAFETY: a null-terminated path and one place for the answer; the other two may be null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
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
