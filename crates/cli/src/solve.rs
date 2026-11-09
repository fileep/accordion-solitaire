//! `acc solve`: solve one deal (from a seed, typed or piped cards, or a file) or an in-progress position, on all
//! cores (PLAN.md Phase 4). A win is verified by the reference engine before it is written. A deal that isn't
//! won gets a dossier; `--escalate` keeps trying with larger limits and fresh random orders first (§5 stages 1–2).

use crate::dossier;
use crate::read_input;
use accordion_core::deck_input::parse_position;
use accordion_core::fast::Position;
use accordion_core::notation::CardMove;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Outcome, SolveRecord};
use accordion_solver::{Config, Endgame, Report, Shared, Verdict, portfolio};
use clap::Args;
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const EXAMPLES: &str = "\
Examples:
  acc solve --seed 42
  acc solve --seed 42 --out win.json        save the winning line in a file
  acc solve --file deck.txt --escalate      a deck from a file, trying harder if it isn't won";

#[derive(Args)]
#[command(after_help = EXAMPLES)]
pub struct SolveArgs {
    #[command(flatten)]
    source: SolveSource,
    /// Positions to search in total, across all threads, before giving up (each --escalate stage multiplies it by 10).
    #[arg(long, default_value_t = 200_000_000, value_parser = crate::numbers::parse_count)]
    max_nodes: u64,
    /// Threads to use (0 = every core).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Seed for the solver's move-order randomness, so a run can be repeated exactly.
    #[arg(long, default_value_t = 1)]
    rng_seed: u64,
    /// Write the solve file here instead of standard output.
    #[arg(long)]
    out: Option<PathBuf>,
    /// If not won, try again with 10x and then 100x the limit and fresh random orders before giving up.
    #[arg(long)]
    escalate: bool,
    /// Where a deal that isn't won gets its dossier.
    #[arg(long, default_value = "output/hard-deals")]
    dossiers: PathBuf,
    /// Solver settings: the name of a JSON file, or the JSON itself; --max-nodes and --rng-seed override theirs.
    #[arg(long)]
    config: Option<String>,
    /// Directory holding endgame tables (`acc endgame build`).
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Endgame tables to use: a pile count, `off`, or `auto` (the largest found in --endgame-dir, up to 10 piles).
    #[arg(long, default_value = "auto")]
    endgame: String,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
pub struct SolveSource {
    /// The deal for this seed.
    #[arg(long)]
    seed: Option<u64>,
    /// A deck as text: 52 cards like "KS 7H 10d A♣", separated by spaces or commas.
    #[arg(long)]
    cards: Option<String>,
    /// Read the deck from a file, or "-" for standard input.
    #[arg(long)]
    file: Option<PathBuf>,
    /// A game in progress: piles separated by "|", each listed bottom to top (all 52 cards).
    #[arg(long)]
    position: Option<String>,
}

/// Runs the portfolio, printing progress to standard error twice a second once it has run for a moment.
fn run_stage(
    root: &Position,
    config: &Config,
    threads: usize,
    endgame: Option<Arc<Endgame>>,
    label: &str,
) -> (Report, f64) {
    let shared = Shared::default();
    let start = Instant::now();
    let mut printed = false;
    let report = std::thread::scope(|scope| {
        let worker = scope.spawn(|| portfolio(root, config, threads, 18, endgame, &shared));
        let mut next = Duration::from_millis(500);
        while !worker.is_finished() {
            std::thread::sleep(Duration::from_millis(10));
            if start.elapsed() >= next {
                next += Duration::from_millis(500);
                printed = true;
                let best = shared.best_piles.load(Ordering::Relaxed);
                eprint!(
                    "\r  {label}: {:>6.1} s · {:>14} positions · best so far {} piles   ",
                    start.elapsed().as_secs_f64(),
                    shared.nodes.load(Ordering::Relaxed),
                    if best == usize::MAX { "-".to_string() } else { best.to_string() }
                );
            }
        }
        worker.join().expect("solver panicked")
    });
    if printed {
        eprintln!();
    }
    (report, start.elapsed().as_secs_f64())
}

fn piles(n: usize) -> String {
    format!("{n} pile{}", if n == 1 { "" } else { "s" })
}

/// What to solve: a deal (with its seed, if it came from one), or a position in progress (no deal).
pub struct Source {
    pub deal: Option<Deal>,
    pub seed: Option<u64>,
    /// The game to start from: the deal's opening, or the position.
    pub game: Game,
}

impl SolveSource {
    pub fn load(&self) -> Result<Source, String> {
        let (deal, seed, game) = match (&self.seed, &self.cards, &self.file, &self.position) {
            (Some(seed), ..) => (Some(Deal::from_seed(*seed)), Some(*seed), None),
            (_, Some(text), ..) => {
                (Some(Deal::parse(text).map_err(|e| format!("invalid deck:\n{e}"))?), None, None)
            }
            (_, _, Some(path), _) => (
                Some(Deal::parse(&read_input(path)?).map_err(|e| format!("invalid deck:\n{e}"))?),
                None,
                None,
            ),
            (_, _, _, Some(text)) => (
                None,
                None,
                Some(Game::from_piles(parse_position(text).map_err(|e| format!("invalid position:\n{e}"))?)),
            ),
            _ => unreachable!("clap requires one source"),
        };
        let game = game.unwrap_or_else(|| Game::new(deal.as_ref().expect("deal or position")));
        Ok(Source { deal, seed, game })
    }
}

pub fn run(args: &SolveArgs) -> Result<bool, String> {
    let (text, won) = solve_text(args)?;
    match &args.out {
        Some(path) => {
            std::fs::write(path, text + "\n").map_err(|e| format!("writing {}: {e}", path.display()))?
        }
        None => {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{text}").map_err(|e| e.to_string())?;
        }
    }
    Ok(won)
}

/// Solves as [`run`] does, and gives back the solve file it writes, and whether the deal was won.
pub fn solve_text(args: &SolveArgs) -> Result<(String, bool), String> {
    let Source { deal, seed, game: start_game } = args.source.load()?;
    let root = Position::from_tops(&start_game.tops());
    let threads = if args.threads == 0 {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        args.threads
    };
    let stages: &[(u64, &str)] = if args.escalate {
        &[(1, "solve"), (10, "escalate-1"), (100, "escalate-2")]
    } else {
        &[(1, "solve")]
    };
    let mut base: Config = match &args.config {
        Some(text) => crate::json_setting("--config", text)?,
        None => Config::default(),
    };
    // A single solve takes milliseconds; loading the 1.2 GB 11-pile table would take longer than that, so `auto`
    // stops at 10 piles (68 MB) here.
    crate::runner::choose_endgame(&mut base, &args.endgame, &args.endgame_dir, 10)?;
    let endgame = crate::runner::load_endgame(&args.endgame_dir, base.search.endgame_piles)?;
    eprintln!("{} piles, {threads} threads, up to {} positions", root.len(), args.max_nodes);

    let mut last = None;
    for (i, &(factor, stage)) in stages.iter().enumerate() {
        let config = Config {
            max_nodes: (args.max_nodes.saturating_mul(factor) / threads as u64).max(1),
            seed: args.rng_seed.wrapping_add(1_000_003 * i as u64),
            ..base
        };
        let (report, seconds) = run_stage(&root, &config, threads, endgame.clone(), stage);
        if let Some(deal) = &deal
            && !matches!(report.verdict, Verdict::Win(_))
        {
            let attempt = dossier::Attempt { stage, config: &config, threads, report: &report, seconds };
            let path = dossier::record(&args.dossiers, deal, seed, &attempt)?;
            dossier::write_index(&args.dossiers)?;
            eprintln!("  not won; dossier: {}", path.display());
        }
        let done = !matches!(report.verdict, Verdict::Unknown);
        last = Some((report, seconds));
        if done {
            break;
        }
    }
    let (report, seconds) = last.expect("at least one stage");
    let (line, won) = match &report.verdict {
        Verdict::Win(line) => (line.clone(), true),
        _ => (report.best_line.clone(), false),
    };
    let what = match report.verdict {
        Verdict::Win(_) => "won".to_string(),
        Verdict::Exhausted => {
            "no win found; every line was searched, but this is not the double-checked proof. For that, run: acc prove"
                .to_string()
        }
        Verdict::Unknown => {
            format!("not won within the limit; best line leaves {}", piles(report.best_piles))
        }
    };
    eprintln!("{what} ({} positions, {} attempts, {seconds:.2} s)", report.nodes, report.attempts);

    let text = match &deal {
        Some(deal) => {
            let outcome = if won { Outcome::Win } else { Outcome::Unknown };
            let mut record =
                SolveRecord::new(deal, seed, outcome, &line).map_err(|e| format!("internal error: {e}"))?;
            record.stats = json!({
                "solver": format!("acc {}", env!("CARGO_PKG_VERSION")),
                "nodes": report.nodes,
                "attempts": report.attempts,
                "threads": threads,
                "seconds": (seconds * 1000.0).round() / 1000.0,
                "search": match report.verdict { Verdict::Win(_) => "win", Verdict::Exhausted => "exhausted", Verdict::Unknown => "limit" },
            })
            .as_object()
            .cloned();
            let checked = verify(&record)
                .map_err(|e| format!("internal error: the solver's line failed verification: {e}"))?;
            eprintln!(
                "verified by the reference engine: {} moves, {} left",
                checked.moves,
                piles(checked.piles_left)
            );
            record.to_json()
        }
        None => {
            // A position: replay the line on the reference engine from the given piles.
            let mut g = start_game.clone();
            let mut moves = Vec::new();
            for &mv in &line {
                let (mover, target) = g.cards_of(mv);
                g.apply(mv).map_err(|e| format!("internal error: solver move rejected: {e}"))?;
                moves.push(CardMove { mover, target }.to_string());
            }
            if won && !g.is_won() {
                return Err("internal error: the solver's line does not end in one pile".into());
            }
            eprintln!(
                "verified by the reference engine: {} moves, {} left",
                moves.len(),
                piles(g.pile_count())
            );
            serde_json::to_string_pretty(
                &json!({ "result": if won { "win" } else { "unknown" }, "moves": moves }),
            )
            .expect("serializable")
        }
    };
    Ok((text, won))
}
