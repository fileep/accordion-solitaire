//! `acc experiment`: run solver configurations over a fixed deal set and record what each costs (PLAN.md Phase 5
//! track A). Each run appends one summary line to `<out>/summary.jsonl` (machine, commit, set, configuration,
//! wins by stage, positions and time percentiles, throughput) and can write one row per deal. Repeats are
//! interleaved (A B A B …) so laptop heat and clock drift hit every configuration alike. A configuration with
//! `"gpu"` runs GPU ladder attempts in front of the CPU solver, as `acc batch --gpu` does (PLAN.md Phase 8); its
//! position and time percentiles cover the deals solved on the CPU.

use crate::dossier::now_utc;
use crate::runner::{self, GpuTotals, Options, percentile};
use accordion_gpu::{Gpu, Settings};
use accordion_solver::{Config, Endgame, Stage, Stats, Verdict};
use clap::Args;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Args)]
pub struct ExperimentArgs {
    /// Deal set: train (seeds 200,000–209,999), validation (300,000–309,999), test (1,000,000–1,099,999),
    /// regression (0–99,999), hard (<out>/sets/hard.txt), a range FROM:COUNT, or a file of seeds.
    #[arg(long)]
    set: String,
    /// A configuration as JSON: {"name": "...", "solver": {Config fields}, "table": 14, "threads": 16, "gpu":
    /// {accordion_gpu::Settings fields}, "note": ".."}. Repeatable; fields left out keep their defaults, and without
    /// "gpu" the GPU isn't used.
    #[arg(long = "config")]
    configs: Vec<String>,
    /// A JSON file holding a list of configurations like --config, run after them.
    #[arg(long)]
    grid: Option<PathBuf>,
    /// Run every configuration this many times, interleaved, to measure the timing spread.
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// Worker threads (0 = every core), unless a configuration says otherwise.
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Dead-position table per thread (log2 of 64-byte buckets), unless a configuration says otherwise.
    #[arg(long, default_value_t = 14)]
    table: u32,
    /// Directory holding endgame tables, for configurations that set `search.endgame_piles`.
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Where summary.jsonl, sets/ and deals/ live.
    #[arg(long, default_value = "research/experiments/phase5")]
    out: PathBuf,
    /// Also write one row per deal (first repeat only) to <out>/deals/<set>-<name>.jsonl.
    #[arg(long)]
    deals: bool,
    /// Print results without appending them to summary.jsonl.
    #[arg(long)]
    dry: bool,
    /// Before measuring, run the first configuration (unrecorded) for this many seconds, so the laptop leaves its
    /// short burst-power state and every measured run sees the same sustained clocks.
    #[arg(long, default_value_t = 0.0)]
    warmup: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    name: String,
    #[serde(default)]
    solver: Config,
    table: Option<u32>,
    threads: Option<usize>,
    gpu: Option<Settings>,
    #[serde(default)]
    note: String,
}

impl Variant {
    /// Options for a run: `threads` in all (0 = every core), one of them the GPU's feeder.
    fn options(
        &self,
        threads: usize,
        table: u32,
        endgame: &Option<Arc<Endgame>>,
        gpu: &Option<Arc<Gpu>>,
    ) -> Options {
        let threads = match self.threads.unwrap_or(threads) {
            0 => runner::all_threads(),
            n => n,
        };
        Options {
            threads: if gpu.is_some() { threads.saturating_sub(1).max(1) } else { threads },
            table_log2: self.table.unwrap_or(table),
            endgame: endgame.clone(),
            records: false,
            gpu: gpu.clone(),
        }
    }
}

/// The seeds of a named deal set (PLAN.md Phase 5 track A; the sets are disjoint).
pub fn deal_set(name: &str, base: &Path) -> Result<Vec<u64>, String> {
    let range = |from: u64, count: u64| (from..from + count).collect();
    match name {
        "train" => Ok(range(200_000, 10_000)),
        "validation" => Ok(range(300_000, 10_000)),
        "test" => Ok(range(1_000_000, 100_000)),
        "regression" => Ok(range(0, 100_000)),
        "hard" => read_seeds(&base.join("sets/hard.txt")),
        // FROM:COUNT only when both sides are numbers, so a Windows path (C:\...) is read as a file.
        _ => match name.split_once(':').map(|(f, c)| (f.trim().parse::<u64>(), c.trim().parse::<u64>())) {
            Some((Ok(from), Ok(count))) => Ok(range(from, count)),
            _ => read_seeds(Path::new(name)),
        },
    }
}

fn read_seeds(path: &Path) -> Result<Vec<u64>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.parse().map_err(|e| format!("{}: {l:?}: {e}", path.display())))
        .collect()
}

/// `git describe`-like identity of the code being measured.
pub fn commit() -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let head = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    format!("{head}{}", if dirty { "+dirty" } else { "" })
}

/// One configuration's run over the set.
struct Outcome {
    won: u64,
    not_won: Vec<u64>,
    failed: u64,
    stages: BTreeMap<String, u64>,
    nodes: Vec<u64>,
    micros: Vec<u64>,
    stats: Stats,
    wall: f64,
    gpu: Option<GpuTotals>,
}

pub fn run(args: &ExperimentArgs) -> Result<bool, String> {
    let seeds = deal_set(&args.set, &args.out)?;
    let mut variants: Vec<Variant> = args
        .configs
        .iter()
        .map(|c| serde_json::from_str(c).map_err(|e| format!("--config {c}: {e}")))
        .collect::<Result<_, _>>()?;
    if let Some(path) = &args.grid {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let more: Vec<Variant> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        variants.extend(more);
    }
    if variants.is_empty() {
        return Err("no configurations: give --config or --grid".into());
    }
    let max_endgame = variants.iter().map(|v| v.solver.search.endgame_piles).max().unwrap_or(0);
    let endgame = runner::load_endgame(&args.endgame_dir, max_endgame)?;
    let gpus: Vec<Option<Arc<Gpu>>> = variants
        .iter()
        .map(|v| match &v.gpu {
            Some(settings) => runner::open_gpu("on", settings.clone()),
            None => Ok(None),
        })
        .collect::<Result<_, _>>()?;
    let (commit, date) = (commit(), now_utc());
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default();
    eprintln!(
        "set {} ({} deals), {} configurations × {} repeats, code {commit}",
        args.set,
        seeds.len(),
        variants.len(),
        args.repeat
    );

    if args.warmup > 0.0 {
        let v = &variants[0];
        let opts = v.options(args.threads, args.table, &endgame, &gpus[0]);
        let start = Instant::now();
        while start.elapsed().as_secs_f64() < args.warmup {
            runner::run(&seeds, &v.solver, &opts, |_| Ok(()), |_| {})?;
        }
        eprintln!("warmed up for {:.0} s", start.elapsed().as_secs_f64());
    }
    let mut results: Vec<Vec<Outcome>> = variants.iter().map(|_| Vec::new()).collect();
    let mut all_won = true;
    for rep in 0..args.repeat {
        for (vi, v) in variants.iter().enumerate() {
            let opts = v.options(args.threads, args.table, &endgame, &gpus[vi]);
            let (threads, table) = (opts.threads + usize::from(opts.gpu.is_some()), opts.table_log2);
            let mut deal_rows = if args.deals && rep == 0 {
                let dir = args.out.join("deals");
                std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
                let set = args.set.replace(['/', '\\', ':'], "_");
                let path = dir.join(format!("{set}-{}.jsonl", v.name));
                Some(BufWriter::new(
                    std::fs::File::create(&path).map_err(|e| format!("creating {}: {e}", path.display()))?,
                ))
            } else {
                None
            };
            let mut o = Outcome {
                won: 0,
                not_won: Vec::new(),
                failed: 0,
                stages: BTreeMap::new(),
                nodes: Vec::with_capacity(seeds.len()),
                micros: Vec::with_capacity(seeds.len()),
                stats: Stats::default(),
                wall: 0.0,
                gpu: None,
            };
            let start = Instant::now();
            let label = format!("{} (run {}/{})", v.name, rep + 1, args.repeat);
            o.gpu = runner::run(
                &seeds,
                &v.solver,
                &opts,
                |s| {
                    let won = matches!(s.report.verdict, Verdict::Win(_)) && s.verified.is_ok();
                    if s.verified.is_err() {
                        o.failed += 1;
                        eprintln!("\nseed {}: VERIFICATION FAILED: {:?}", s.seed, s.verified);
                    }
                    if won {
                        o.won += 1;
                        *o.stages.entry(s.report.stage.to_string()).or_default() += 1;
                    } else {
                        o.not_won.push(s.seed);
                    }
                    if s.report.stage != Stage::Gpu {
                        o.nodes.push(s.report.nodes);
                        o.micros.push(s.us);
                    }
                    o.stats += s.report.stats;
                    if let Some(w) = deal_rows.as_mut() {
                        let row = json!({
                            "seed": s.seed,
                            "won": won,
                            "stage": s.report.stage.to_string(),
                            "nodes": s.report.nodes,
                            "attempts": s.report.attempts,
                            "us": s.us,
                        });
                        writeln!(w, "{row}").map_err(|e| e.to_string())?;
                    }
                    Ok(())
                },
                |done| eprint!("\r  {label}: {done} / {} deals   ", seeds.len()),
            )?;
            o.wall = start.elapsed().as_secs_f64();
            if let Some(mut w) = deal_rows {
                w.flush().map_err(|e| e.to_string())?;
            }
            o.nodes.sort_unstable();
            o.micros.sort_unstable();
            o.not_won.sort_unstable();
            all_won &= o.not_won.is_empty() && o.failed == 0 && o.gpu.as_ref().is_none_or(|t| t.failed == 0);
            let n = seeds.len() as f64;
            let mean = |v: &[u64]| v.iter().sum::<u64>() as f64 / v.len().max(1) as f64;
            let pct = |v: &[u64]| {
                json!({
                    "mean": (mean(v) * 10.0).round() / 10.0,
                    "p50": percentile(v, 0.5), "p90": percentile(v, 0.9), "p99": percentile(v, 0.99),
                    "p999": percentile(v, 0.999), "max": v.last().copied().unwrap_or(0),
                })
            };
            eprintln!(
                "\r  {label}: won {}/{}, mean {:.0} positions (p99 {}), {:.2} ms/deal, {:.0} deals/s ({:.1} s)",
                o.won,
                seeds.len(),
                mean(&o.nodes),
                percentile(&o.nodes, 0.99),
                mean(&o.micros) / 1e3,
                n / o.wall,
                o.wall
            );
            if !args.dry {
                let summary = json!({
                    "date": date, "commit": commit, "cpu": cpu, "set": args.set, "deals": seeds.len(),
                    "name": v.name, "note": v.note, "repeat": rep + 1, "threads": threads, "table": table,
                    "solver": v.solver, "gpu": v.gpu,
                    "won": o.won, "failed_verify": o.failed, "not_won": o.not_won.iter().take(100).collect::<Vec<_>>(),
                    "stages": o.stages,
                    "nodes": pct(&o.nodes), "us": pct(&o.micros),
                    "stats": {
                        "table_hits": o.stats.table_hits, "dead_end_cuts": o.stats.dead_end_cuts,
                        "tests_run": o.stats.tests_run, "endgame_lookups": o.stats.endgame_lookups,
                        "asleep": o.stats.asleep, "search_nodes": o.stats.nodes,
                    },
                    "gpu_totals": o.gpu.as_ref().map(|t| json!({
                        "deals": t.deals, "won": t.won, "failed_verify": t.failed, "attempts": t.attempts,
                        "positions": t.positions, "launches": t.launches, "error": t.error,
                    })),
                    "wall_s": (o.wall * 1000.0).round() / 1000.0,
                    "deals_per_s": (n / o.wall).round(),
                });
                append(&args.out.join("summary.jsonl"), &summary)?;
            }
            results[vi].push(o);
        }
    }
    print_table(&variants, &results, seeds.len());
    Ok(all_won)
}

fn append(path: &Path, row: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("opening {}: {e}", path.display()))?;
    writeln!(f, "{row}").map_err(|e| format!("writing {}: {e}", path.display()))
}

/// One line per configuration: wins, positions (identical in every repeat), and time (median over repeats,
/// with the spread).
fn print_table(variants: &[Variant], results: &[Vec<Outcome>], deals: usize) {
    let n = deals as f64;
    println!(
        "{:<28} {:>13} {:>10} {:>9} {:>10} {:>9} {:>9} {:>17}",
        "configuration", "won", "mean pos", "p99 pos", "max pos", "ms/deal", "p99 ms", "deals/s (spread)"
    );
    for (v, runs) in variants.iter().zip(results) {
        let first = &runs[0];
        let mut rates: Vec<f64> = runs.iter().map(|o| n / o.wall).collect();
        rates.sort_by(f64::total_cmp);
        let mut ms: Vec<f64> = runs
            .iter()
            .map(|o| o.micros.iter().sum::<u64>() as f64 / o.micros.len().max(1) as f64 / 1e3)
            .collect();
        ms.sort_by(f64::total_cmp);
        let median = |v: &[f64]| v[v.len() / 2];
        let spread = if rates.len() > 1 {
            format!(" ±{:.0}%", 50.0 * (rates[rates.len() - 1] - rates[0]) / median(&rates))
        } else {
            String::new()
        };
        println!(
            "{:<28} {:>13} {:>10.0} {:>9} {:>10} {:>9.3} {:>9.2} {:>17}",
            v.name,
            format!("{}/{}{}", first.won, deals, if first.failed > 0 { " FAIL" } else { "" }),
            first.nodes.iter().sum::<u64>() as f64 / first.nodes.len().max(1) as f64,
            percentile(&first.nodes, 0.99),
            first.nodes.last().copied().unwrap_or(0),
            median(&ms),
            percentile(&first.micros, 0.99) as f64 / 1e3,
            format!("{:.0}{spread}", median(&rates)),
        );
    }
}
