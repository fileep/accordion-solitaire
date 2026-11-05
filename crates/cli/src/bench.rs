//! `acc bench`: single-thread search speed on a fixed workload, the Phase 2 speed gate (PLAN.md §4).
//!
//! The workload is the brute-force half of the Masten-style attempt: from each of `--deals` seeded deals, play
//! `k` uniformly random moves (for each `k` in `--depths`), then run the first-win search on what is left.
//! Every configuration sees exactly the same positions. With `--prove`, the exhaustive prover's runs are timed on
//! them instead.

use accordion_core::Deal;
use accordion_core::fast::Position;
use accordion_core::rng::Xoshiro256StarStar;
use accordion_solver::{Endgame, Gates, Proof, ProveOptions, Search, SearchResult, Stats, prove};
use clap::Args;
use std::path::Path;
use std::time::Instant;

#[derive(Args)]
pub struct BenchArgs {
    /// Deals per prefix depth.
    #[arg(long, default_value_t = 300)]
    deals: u64,
    /// Random moves played before the search starts (comma-separated); 0 searches the full deal.
    #[arg(long, value_delimiter = ',', default_value = "26,30,34")]
    depths: Vec<usize>,
    /// Positions a single search may generate before giving up.
    #[arg(long, default_value_t = 2_000_000)]
    budget: u64,
    /// Dead-position table size, as log2 of 64-byte buckets (16 = 4 MB).
    #[arg(long, default_value_t = 16)]
    table: u32,
    /// Compare dead-end gate settings instead of measuring only the default.
    #[arg(long)]
    sweep: bool,
    /// Time the exhaustive prover instead (`acc prove`'s runs, with the endgame tables in ./endgame), with --budget
    /// positions a run; totals are over the positions every run proves lost.
    #[arg(long)]
    prove: bool,
}

fn residuals(deals: u64, k: usize) -> Vec<Position> {
    (0..deals)
        .filter_map(|seed| {
            let mut pos = Position::from_deal(&Deal::from_seed(seed));
            let mut rng = Xoshiro256StarStar::from_seed(seed.wrapping_mul(31) ^ k as u64);
            for _ in 0..k {
                pos = pos.play(pos.random_move(&mut rng)?);
            }
            Some(pos)
        })
        .collect()
}

struct Run {
    stats: Stats,
    seconds: f64,
    wins: usize,
    dead: usize,
    budget_outs: usize,
}

fn measure(work: &[Position], gates: Gates, args: &BenchArgs) -> Run {
    let mut search = Search::new(args.table, gates);
    let (mut wins, mut dead, mut budget_outs) = (0, 0, 0);
    let start = Instant::now();
    for root in work {
        search.clear_table();
        match search.run(root, args.budget) {
            SearchResult::Win(_) => wins += 1,
            SearchResult::Dead => dead += 1,
            SearchResult::OutOfBudget | SearchResult::Stopped => budget_outs += 1,
        }
    }
    Run { stats: search.stats, seconds: start.elapsed().as_secs_f64(), wins, dead, budget_outs }
}

fn label(g: Gates) -> String {
    match g {
        Gates::OFF => "no dead-end tests".into(),
        Gates::ALWAYS => "every test at every node".into(),
        g => format!(
            "connectivity <= {}{} | left-leaf {}",
            g.connectivity,
            if g.every_node { " every node" } else { " on trigger" },
            if g.left_leaf == 0 { "off".to_string() } else { format!("<= {}", g.left_leaf) }
        ),
    }
}

fn prove_bench(args: &BenchArgs) -> Result<(), String> {
    let dir = Path::new("endgame");
    let piles = crate::runner::available_endgame(dir).min(11);
    let tables = if piles > 0 { Some(Endgame::load(dir, piles)?) } else { None };
    let independent = ProveOptions::independent(1, args.budget);
    let configs = [
        ("independent, by pattern to 16 piles", independent, true),
        ("independent, by cards only", ProveOptions { pattern_piles: 0, ..independent }, true),
        ("plain", ProveOptions::plain(args.budget), false),
    ];
    println!(
        "exhaustive prover, up to {} positions a run, endgame tables to {piles} piles, single thread",
        args.budget
    );
    for &k in &args.depths {
        let work = residuals(args.deals, k);
        let runs: Vec<Vec<_>> = configs
            .iter()
            .map(|(_, opts, use_tables)| {
                let tables = if *use_tables { tables.as_ref() } else { None };
                work.iter()
                    .map(|pos| {
                        let t = Instant::now();
                        let r = prove(pos, opts, tables);
                        (r, t.elapsed().as_secs_f64())
                    })
                    .collect()
            })
            .collect();
        let lost: Vec<usize> =
            (0..work.len()).filter(|&i| runs.iter().all(|r| r[i].0.proof == Proof::Lost)).collect();
        let won = (0..work.len()).filter(|&i| matches!(runs[0][i].0.proof, Proof::Won(_))).count();
        println!(
            "\n{} positions after {k} random moves: {} lost in every run, {won} won",
            work.len(),
            lost.len()
        );
        println!("{:<38} {:>14} {:>12} {:>9} {:>10}", "run", "positions", "dead", "s", "Mpos/s");
        for ((name, ..), r) in configs.iter().zip(&runs) {
            let positions: u64 = lost.iter().map(|&i| r[i].0.positions).sum();
            let dead: usize = lost.iter().map(|&i| r[i].0.dead).sum();
            let secs: f64 = lost.iter().map(|&i| r[i].1).sum();
            println!(
                "{name:<38} {positions:>14} {dead:>12} {secs:>9.2} {:>10.1}",
                positions as f64 / secs.max(1e-9) / 1e6
            );
        }
    }
    Ok(())
}

pub fn run(args: &BenchArgs) -> Result<(), String> {
    if args.prove {
        return prove_bench(args);
    }
    let configs: Vec<Gates> = if args.sweep {
        let mut c = vec![Gates::OFF, Gates::ALWAYS];
        for connectivity in [52, 22, 19, 16, 13] {
            for left_leaf in [0, 19, 16, 13] {
                c.push(Gates { connectivity, left_leaf, every_node: false, leftmost: false });
            }
        }
        c.push(Gates { connectivity: 19, left_leaf: 16, every_node: true, leftmost: false });
        c
    } else {
        vec![Gates::default()]
    };
    println!(
        "table {} MB, budget {} positions per search, single thread",
        (64usize << args.table) >> 20,
        args.budget
    );
    for &k in &args.depths {
        let work = residuals(args.deals, k);
        println!(
            "\n{} positions after {k} random moves ({} piles left at the start of each search)",
            work.len(),
            52 - k
        );
        println!(
            "{:<46} {:>12} {:>9} {:>10} {:>11} {:>10} {:>7}",
            "dead-end tests", "nodes", "ms", "Mnodes/s", "tests run", "cuts", "w/d/b"
        );
        for &gates in &configs {
            let r = measure(&work, gates, args);
            println!(
                "{:<46} {:>12} {:>9.1} {:>10.1} {:>11} {:>10} {:>7}",
                label(gates),
                r.stats.nodes,
                r.seconds * 1e3,
                r.stats.nodes as f64 / r.seconds / 1e6,
                r.stats.tests_run,
                r.stats.dead_end_cuts,
                format!("{}/{}/{}", r.wins, r.dead, r.budget_outs)
            );
        }
    }
    Ok(())
}
