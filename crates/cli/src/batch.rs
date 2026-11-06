//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time, and on the GPU when
//! there is one (PLAN.md Phases 4, 5 and 8). Every win is verified by the reference engine before it counts; every
//! deal that isn't won gets a dossier, and a proof attempt on a thread of its own while the run goes on: two
//! exhaustive runs that must agree before a loss counts ([`prove::prove_both`]). The summary gives the win rate with
//! a Wilson 95% interval, counting undecided deals both ways, and checks that the deals are all different games.

use crate::dossier;
use crate::progress::Meter;
use crate::prove::{self, Finding, Limits};
use crate::runner::{self, Options, Seeds, percentile, wilson};
use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_gpu::Settings;
use accordion_solver::{Config, Endgame, Search, Stage, Verdict, solve};
use clap::Args;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Instant;

#[derive(Args)]
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
}

/// What the proofs of the deals the solver didn't win found.
#[derive(Default)]
struct Proofs {
    lost: u64,
    won: u64,
    undecided: u64,
    /// The runs disagreed, or a winning line failed the reference engine: bugs.
    failed: u64,
}

/// Proves the deals sent to it, one at a time, and records each in its dossier (with a loss record in
/// `<dossiers>/proofs/` for a proven loss) when there are dossiers.
fn prover(
    seeds: mpsc::Receiver<u64>,
    limits: Limits,
    tables: Option<Arc<Endgame>>,
    dossiers: Option<PathBuf>,
) -> Proofs {
    let mut tally = Proofs::default();
    for seed in seeds {
        let deal = Deal::from_seed(seed);
        eprintln!("\nseed {seed}: not won; proving it lost, up to {} positions a run", limits.positions);
        let say = |line: String| eprintln!("seed {seed}: {line}");
        let proved = prove::prove_both(&Position::from_deal(&deal), &limits, tables.as_deref(), say)
            .and_then(|p| p.record(&deal, Some(seed)).map(|record| (p, record)));
        let (p, record) = match proved {
            Ok(x) => x,
            Err(e) => {
                tally.failed += 1;
                eprintln!("seed {seed}: PROOF FAILED: {e}");
                continue;
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
        let Some(dir) = &dossiers else { continue };
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
    tally
}

pub fn run(args: &BatchArgs) -> Result<bool, String> {
    let threads = if args.threads == 0 { runner::all_threads() } else { args.threads };
    let mut config: Config = match &args.config {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("--config: {e}"))?,
        None => Config::default(),
    };
    config.max_nodes = args.max_nodes;
    runner::choose_endgame(&mut config, &args.endgame, &args.endgame_dir, usize::MAX)?;
    let endgame = runner::load_endgame(&args.endgame_dir, config.search.endgame_piles)?;
    let settings: Settings = match &args.gpu_config {
        Some(text) => serde_json::from_str(text).map_err(|e| format!("--gpu-config: {e}"))?,
        None => Settings::default(),
    };
    let gpu = runner::open_gpu(&args.gpu, settings)?;
    // The GPU's feeder needs a thread of its own, or the GPU waits on it (docs/BENCHMARKS.md, Phase 5 track F).
    let workers = if gpu.is_some() { threads.saturating_sub(1).max(1) } else { threads };
    let mut out = match &args.out {
        Some(p) => Some(BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("creating {}: {e}", p.display()))?,
        )),
        None => None,
    };
    let hardware =
        if gpu.is_some() { format!("{workers} CPU threads + GPU") } else { format!("{workers} threads") };
    eprintln!(
        "seeds {}..{} ({} deals), up to {} positions per deal, {hardware}",
        args.from,
        args.from + args.count - 1,
        args.count,
        args.max_nodes
    );
    let seeds = Seeds::Range { from: args.from, count: args.count as usize };
    let about = format!(
        "acc batch: seeds {}..{} ({} deals), {hardware}, commit {}",
        args.from,
        args.from + args.count - 1,
        args.count,
        crate::experiment::commit()
    );
    let mut meter = Meter::new(seeds.count(), (!args.no_log).then_some(args.log_dir.as_path()), &about)?;
    stay_awake();
    let opts = Options { threads: workers, table_log2: args.table, endgame, records: out.is_some(), gpu };
    let (to_prover, proving) = if args.prove > 0 {
        let (tx, rx) = mpsc::channel();
        let limits =
            Limits { positions: args.prove, bytes: args.prove_memory << 30, rng_seed: 1, pattern_piles: 16 };
        let (tables, dir) = (opts.endgame.clone(), (!args.no_dossiers).then(|| args.dossiers.clone()));
        (Some(tx), Some(std::thread::spawn(move || prover(rx, limits, tables, dir))))
    } else {
        (None, None)
    };
    let mut sent = 0u64;
    let start = Instant::now();
    let (mut won, mut exhausted, mut unknown, mut failed) = (0u64, 0u64, 0u64, 0u64);
    let mut stages: HashMap<String, u64> = HashMap::new();
    let mut gpu_stage = 0u64;
    // Grown as needed: only the CPU's deals have them (a fifth or so), and a billion deals' worth is 16 GB.
    let mut nodes = Vec::new();
    let mut micros = Vec::new();
    let mut total_nodes = 0u64;
    // Output in seed order: hold records that finish early until the ones before them are in.
    let mut pending: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut written = 0usize;
    let mut any_dossier = false;
    let mut hardest: BinaryHeap<Reverse<(u64, u64)>> = BinaryHeap::new();
    let gpu_totals = runner::run(
        seeds,
        &config,
        &opts,
        |s| {
            // Per-deal costs are the CPU solver's; the GPU works in batches and counts its own positions.
            if s.report.stage != Stage::Gpu {
                nodes.push(s.report.nodes);
                micros.push(s.us);
                total_nodes += s.report.nodes;
                if args.gallery > 0 {
                    hardest.push(Reverse((s.report.nodes, s.seed)));
                    if hardest.len() > args.gallery {
                        hardest.pop();
                    }
                }
            }
            if let Err(e) = &s.verified {
                failed += 1;
                eprintln!("\nseed {}: VERIFICATION FAILED: {e}", s.seed);
            } else {
                match s.report.verdict {
                    Verdict::Win(_) => {
                        won += 1;
                        if s.report.stage == Stage::Gpu {
                            gpu_stage += 1;
                        } else {
                            *stages.entry(s.report.stage.to_string()).or_default() += 1;
                        }
                    }
                    Verdict::Exhausted => exhausted += 1,
                    Verdict::Unknown => unknown += 1,
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
                    any_dossier = true;
                }
                if !matches!(s.report.verdict, Verdict::Win(_))
                    && let Some(tx) = &to_prover
                {
                    tx.send(s.seed).map_err(|_| "the prover thread stopped".to_string())?;
                    sent += 1;
                }
            }
            if let Some(w) = out.as_mut() {
                pending.insert(s.index, s.verified.is_ok().then_some(s.record).flatten());
                while let Some(record) = pending.remove(&written) {
                    if let Some(line) = record {
                        writeln!(w, "{line}").map_err(|e| e.to_string())?;
                    }
                    written += 1;
                }
            }
            Ok(())
        },
        |p| meter.update(&p),
    )?;
    if gpu_stage > 0 {
        stages.insert(Stage::Gpu.to_string(), gpu_stage);
    }
    eprintln!();
    let elapsed = start.elapsed().as_secs_f64();
    let same = runner::same_games(seeds, |s| runner::fingerprint(&Deal::from_seed(s)));
    if let Some(mut w) = out {
        w.flush().map_err(|e| e.to_string())?;
    }
    drop(to_prover);
    match sent {
        0 => {}
        1 => eprintln!("finishing the proof of the deal not won"),
        _ => eprintln!("finishing the proofs of the {sent} deals not won"),
    }
    let proofs = proving.map(|h| h.join().expect("the prover panicked")).unwrap_or_default();
    if any_dossier {
        dossier::write_index(&args.dossiers)?;
    }
    if args.gallery > 0 {
        // Re-solve the hardest deals with the same settings (deterministic, so the same lines) to keep their wins.
        let mut top: Vec<(u64, u64)> = hardest.into_iter().map(|Reverse(x)| x).collect();
        top.sort_unstable_by(|a, b| b.cmp(a));
        let mut search = Search::with_options(args.table, config.search);
        search.set_endgame(opts.endgame.clone());
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
            "`acc batch` over seeds {}–{} ({} deals, commit {}{cpu_only})",
            args.from,
            args.from + args.count - 1,
            args.count,
            crate::experiment::commit()
        );
        dossier::write_gallery(&args.dossiers, &entries, &run)?;
        eprintln!(
            "gallery: the {} hardest deals in {}",
            entries.len(),
            args.dossiers.join("gallery").display()
        );
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
    let mut by_stage: Vec<_> = stages.into_iter().collect();
    by_stage.sort();
    println!("deals            {n} (seeds {}..{})", args.from, args.from + n - 1);
    println!(
        "won              {won} (every one verified by the reference engine){}",
        if by_stage.len() > 1 || by_stage.first().is_some_and(|(s, _)| s != "thorough") {
            format!(
                "; by stage: {}",
                by_stage.iter().map(|(s, k)| format!("{s} {k}")).collect::<Vec<_>>().join(", ")
            )
        } else {
            String::new()
        }
    );
    println!(
        "not won          {} by the solver ({exhausted} exhausted, {unknown} hit the limit of {} positions)",
        n - won_by_solver - failed,
        args.max_nodes
    );
    if args.prove > 0 {
        println!(
            "proofs           {} proven lost, {} won, {} undecided (two exhaustive runs of up to {} positions each){}",
            proofs.lost,
            proofs.won,
            proofs.undecided,
            args.prove,
            if proofs.failed > 0 { format!("; {} FAILED", proofs.failed) } else { String::new() }
        );
    }
    println!("failed to verify {failed}");
    if same.is_empty() {
        println!("distinct games   all {n} deals differ, even with suits and ranks renamed");
    } else {
        let shown: Vec<String> = same.iter().take(20).map(|(a, b)| format!("{a}={b}")).collect();
        println!(
            "distinct games   {} pairs of seeds deal the same game (up to renamed suits and ranks): {}",
            same.len(),
            shown.join(", ")
        );
    }
    println!(
        "win rate         {:.6}%   95% CI [{:.6}%, {:.6}%] (Wilson; undecided deals counted as losses for the lower bound, wins for the upper)",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    let cpu_deals = nodes.len();
    let on_cpu =
        if gpu_totals.is_some() { format!(" ({cpu_deals} deals solved on the CPU)") } else { String::new() };
    println!(
        "positions/deal   mean {:.0}  median {}  p90 {}  p99 {}  p99.9 {}  max {}{on_cpu}",
        total_nodes as f64 / cpu_deals.max(1) as f64,
        percentile(&nodes, 0.5),
        percentile(&nodes, 0.9),
        percentile(&nodes, 0.99),
        percentile(&nodes, 0.999),
        nodes.last().copied().unwrap_or(0)
    );
    println!(
        "ms/deal (1 core) mean {:.2}  median {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.1}{on_cpu}",
        micros.iter().sum::<u64>() as f64 / cpu_deals.max(1) as f64 / 1e3,
        percentile(&micros, 0.5) as f64 / 1e3,
        percentile(&micros, 0.9) as f64 / 1e3,
        percentile(&micros, 0.99) as f64 / 1e3,
        percentile(&micros, 0.999) as f64 / 1e3,
        micros.last().copied().unwrap_or(0) as f64 / 1e3
    );
    if let (Some(t), Some(gpu)) = (&gpu_totals, &opts.gpu) {
        println!(
            "GPU              {}: won {} of the {} deals it was given ({:.2}%), {} lines failed verification; {:.1}M attempts, {:.0}M positions/s{}",
            gpu.name,
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
        println!(
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
    println!(
        "throughput       {:.0} deals/s, {:.0}M CPU positions/s on {hardware} ({elapsed:.1} s wall)",
        n as f64 / elapsed,
        total_nodes as f64 / elapsed / 1e6
    );
    Ok(failed == 0 && proofs.failed == 0 && gpu_totals.is_none_or(|t| t.failed == 0))
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
