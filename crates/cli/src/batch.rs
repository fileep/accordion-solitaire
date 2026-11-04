//! `acc batch`: solve a range of seeded deals on every core, one deal per thread at a time (PLAN.md Phases 4–5).
//! Every win is verified by the reference engine before it counts; every deal that isn't won gets a dossier.
//! The summary gives the win rate with a Wilson 95% interval, counting not-won deals both ways.

use crate::dossier;
use crate::runner::{self, Options, percentile, wilson};
use accordion_core::Deal;
use accordion_solver::{Config, Verdict};
use clap::Args;
use std::collections::{BTreeMap, HashMap};
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
    /// Worker threads (0 = every core).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Directory holding endgame tables (`acc endgame build`).
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Endgame tables to use: a pile count, `off`, or `auto` (the largest found in --endgame-dir).
    #[arg(long, default_value = "auto")]
    endgame: String,
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
    let mut out = match &args.out {
        Some(p) => Some(BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("creating {}: {e}", p.display()))?,
        )),
        None => None,
    };
    eprintln!(
        "seeds {}..{} ({} deals), up to {} positions per deal, {threads} threads",
        args.from,
        args.from + args.count - 1,
        args.count,
        args.max_nodes
    );
    let seeds: Vec<u64> = (args.from..args.from + args.count).collect();
    let opts = Options { threads, table_log2: args.table, endgame, records: out.is_some() };
    let start = Instant::now();
    let (mut won, mut exhausted, mut unknown, mut failed) = (0u64, 0u64, 0u64, 0u64);
    let mut stages: HashMap<String, u64> = HashMap::new();
    let mut nodes = Vec::with_capacity(seeds.len());
    let mut micros = Vec::with_capacity(seeds.len());
    let mut total_nodes = 0u64;
    // Output in seed order: hold records that finish early until the ones before them are in.
    let mut pending: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut written = 0usize;
    let mut any_dossier = false;
    runner::run(
        &seeds,
        &config,
        &opts,
        |s| {
            nodes.push(s.report.nodes);
            micros.push(s.us);
            total_nodes += s.report.nodes;
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
    if let Some(mut w) = out {
        w.flush().map_err(|e| e.to_string())?;
    }
    if any_dossier {
        dossier::write_index(&args.dossiers)?;
    }

    let elapsed = start.elapsed().as_secs_f64();
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
    println!(
        "win rate         {:.4}%   95% CI [{:.4}%, {:.4}%] (Wilson; not-won deals counted as losses for the lower bound, wins for the upper)",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    println!(
        "positions/deal   mean {:.0}  median {}  p90 {}  p99 {}  p99.9 {}  max {}",
        total_nodes as f64 / n as f64,
        percentile(&nodes, 0.5),
        percentile(&nodes, 0.9),
        percentile(&nodes, 0.99),
        percentile(&nodes, 0.999),
        nodes.last().copied().unwrap_or(0)
    );
    println!(
        "ms/deal (1 core) mean {:.2}  median {:.2}  p90 {:.2}  p99 {:.2}  p99.9 {:.2}  max {:.1}",
        micros.iter().sum::<u64>() as f64 / n as f64 / 1e3,
        percentile(&micros, 0.5) as f64 / 1e3,
        percentile(&micros, 0.9) as f64 / 1e3,
        percentile(&micros, 0.99) as f64 / 1e3,
        percentile(&micros, 0.999) as f64 / 1e3,
        micros.last().copied().unwrap_or(0) as f64 / 1e3
    );
    println!(
        "throughput       {:.0} deals/s, {:.0}M positions/s on {threads} threads ({elapsed:.1} s wall)",
        n as f64 / elapsed,
        total_nodes as f64 / elapsed / 1e6
    );
    Ok(failed == 0)
}
