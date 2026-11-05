//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time, and on the GPU when
//! there is one (PLAN.md Phases 4, 5 and 8). Every win is verified by the reference engine before it counts; every
//! deal that isn't won gets a dossier. The summary gives the win rate with a Wilson 95% interval, counting not-won
//! deals both ways, and checks that the deals are all different games.

use crate::dossier;
use crate::runner::{self, Options, percentile, wilson};
use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_gpu::Settings;
use accordion_solver::{Config, Search, Stage, Verdict, solve};
use clap::Args;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
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
    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();
    let opts = Options { threads: workers, table_log2: args.table, endgame, records: out.is_some(), gpu };
    let start = Instant::now();
    let (mut won, mut exhausted, mut unknown, mut failed) = (0u64, 0u64, 0u64, 0u64);
    let mut stages: HashMap<String, u64> = HashMap::new();
    let mut nodes = Vec::with_capacity(seeds.len());
    let mut micros = Vec::with_capacity(seeds.len());
    let mut total_nodes = 0u64;
    let mut fingerprints = vec![0u64; seeds.len()];
    // Output in seed order: hold records that finish early until the ones before them are in.
    let mut pending: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut written = 0usize;
    let mut any_dossier = false;
    let mut hardest: BinaryHeap<Reverse<(u64, u64)>> = BinaryHeap::new();
    let gpu_totals = runner::run(
        &seeds,
        &config,
        &opts,
        |s| {
            fingerprints[s.index] = s.fingerprint;
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
                        *stages.entry(s.report.stage.to_string()).or_default() += 1;
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
        |done| {
            let elapsed = start.elapsed().as_secs_f64();
            eprint!("\r  {done} / {} deals · {:.0} deals/s   ", seeds.len(), done as f64 / elapsed);
        },
    )?;
    eprintln!();
    let elapsed = start.elapsed().as_secs_f64();
    let same = runner::same_games(&seeds, &fingerprints);
    drop(fingerprints);
    if let Some(mut w) = out {
        w.flush().map_err(|e| e.to_string())?;
    }
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
    let (lo, _) = wilson(won, n);
    let (_, hi) = wilson(won + exhausted + unknown, n);
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
        "not won          {} exhausted, {unknown} hit the limit of {} positions",
        exhausted, args.max_nodes
    );
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
        "win rate         {:.6}%   95% CI [{:.6}%, {:.6}%] (Wilson; not-won deals counted as losses for the lower bound, wins for the upper)",
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
    Ok(failed == 0 && gpu_totals.is_none_or(|t| t.failed == 0))
}
