//! `acc prove`: exhaustive search, to prove a deal or position lost (PLAN.md §5 stage 3). Two independent runs, each
//! with an exact set of dead positions, must both finish without a win before a loss is claimed: a plain one (rules
//! only, fixed order) and one with everything the solver uses (random order, dead-end tests, sleep sets, endgame
//! tables). A proven loss is written as a `loss` record and marks the deal's dossier `loss (proven)`. Exhaustive
//! search of a whole random deal is far out of reach; this is for small or nearly stuck positions.

use crate::dossier;
use crate::runner;
use crate::solve::{SolveSource, Source};
use accordion_core::fast::Position;
use accordion_core::notation::CardMove;
use accordion_core::verify::verify;
use accordion_core::{Move, Outcome, SolveRecord};
use accordion_solver::{Endgame, Proof, ProofReport, ProveOptions, prove};
use clap::Args;
use serde_json::json;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Args)]
pub struct ProveArgs {
    #[command(flatten)]
    source: SolveSource,
    /// Positions each of the two runs may generate before giving up.
    #[arg(long, default_value_t = 200_000_000)]
    max_positions: u64,
    /// Seed for the independent run's move order.
    #[arg(long, default_value_t = 1)]
    rng_seed: u64,
    /// Write the result (a `loss` or `win` solve record, for a deal) here instead of standard output.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where the deal's dossier is.
    #[arg(long, default_value = "hard-deals")]
    dossiers: PathBuf,
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Endgame tables for the independent run: a pile count, `off`, or `auto` (the largest found, up to 11).
    #[arg(long, default_value = "auto")]
    endgame: String,
}

fn summary(r: &ProofReport, seconds: f64) -> serde_json::Value {
    let result = match r.proof {
        Proof::Won(_) => "win",
        Proof::Lost => "lost",
        Proof::Unfinished => "unfinished",
    };
    json!({ "result": result, "positions": r.positions, "dead_positions": r.dead, "seconds": (seconds * 1000.0).round() / 1000.0 })
}

pub fn run(args: &ProveArgs) -> Result<bool, String> {
    let Source { deal, seed, game } = args.source.load()?;
    let root = Position::from_tops(&game.tops());
    let mut config = accordion_solver::Config::default();
    runner::choose_endgame(&mut config, &args.endgame, &args.endgame_dir, 11)?;
    let tables: Option<Endgame> = match config.search.endgame_piles {
        0 => None,
        k => Some(Endgame::load(&args.endgame_dir, k)?),
    };
    eprintln!(
        "{} piles; exhaustive search, two independent runs of up to {} positions",
        root.len(),
        args.max_positions
    );

    let t = Instant::now();
    let plain = prove(&root, &ProveOptions::plain(args.max_positions), None);
    let plain_s = t.elapsed().as_secs_f64();
    eprintln!(
        "  plain run:       {} after {} positions ({} dead) in {plain_s:.2} s",
        kind(&plain),
        plain.positions,
        plain.dead
    );
    let t = Instant::now();
    let independent =
        prove(&root, &ProveOptions::independent(args.rng_seed, args.max_positions), tables.as_ref());
    let ind_s = t.elapsed().as_secs_f64();
    eprintln!(
        "  independent run: {} after {} positions ({} dead) in {ind_s:.2} s",
        kind(&independent),
        independent.positions,
        independent.dead
    );

    let lost = plain.proof == Proof::Lost && independent.proof == Proof::Lost;
    let won = [&plain, &independent].into_iter().find_map(|r| match &r.proof {
        Proof::Won(line) => Some(line.clone()),
        _ => None,
    });
    // A finished run that found no win contradicts one that did: that is a bug, never a result.
    if won.is_some() && (plain.proof == Proof::Lost || independent.proof == Proof::Lost) {
        return Err(
            "the two runs DISAGREE (one won, one proved a loss): this is a bug; nothing was written".into()
        );
    }
    let best = if plain.best_piles <= independent.best_piles { &plain } else { &independent };
    let proof = json!({
        "solver": format!("acc {}", env!("CARGO_PKG_VERSION")),
        "plain": summary(&plain, plain_s),
        "independent": { "move_order_seed": args.rng_seed, "endgame_piles": config.search.endgame_piles, "run": summary(&independent, ind_s) },
    });
    let verdict = if lost {
        "proven lost: both exhaustive runs finished without a win"
    } else if won.is_some() {
        "won (so not a loss)"
    } else {
        "not decided: a run hit its limit"
    };
    eprintln!("{verdict}");

    let text = match &deal {
        Some(deal) => {
            // A deal that turned out winnable has nothing to record; a loss or an unfinished proof does.
            if won.is_none() {
                let path = dossier::record_proof(
                    &args.dossiers,
                    deal,
                    seed,
                    lost,
                    (best.best_piles, &best.best_line),
                    proof.clone(),
                )?;
                dossier::write_index(&args.dossiers)?;
                eprintln!("dossier: {}", path.display());
            }
            let (outcome, line): (Outcome, &[Move]) = match (&won, lost) {
                (Some(line), _) => (Outcome::Win, line),
                (None, true) => (Outcome::Loss, &best.best_line),
                (None, false) => (Outcome::Unknown, &best.best_line),
            };
            let mut record =
                SolveRecord::new(deal, seed, outcome, line).map_err(|e| format!("internal error: {e}"))?;
            record.stats = json!({ "proof": proof }).as_object().cloned();
            verify(&record).map_err(|e| format!("internal error: the line failed verification: {e}"))?;
            record.to_json()
        }
        None => {
            let mut g = game.clone();
            let line = won.as_deref().unwrap_or(&best.best_line);
            let mut moves = Vec::new();
            for &mv in line {
                let (mover, target) = g.cards_of(mv);
                g.apply(mv).map_err(|e| format!("internal error: move rejected: {e}"))?;
                moves.push(CardMove { mover, target }.to_string());
            }
            let result = if lost {
                "loss"
            } else if won.is_some() {
                "win"
            } else {
                "unknown"
            };
            serde_json::to_string_pretty(&json!({ "result": result, "moves": moves, "proof": proof }))
                .expect("serializable")
        }
    };
    match &args.out {
        Some(path) => {
            std::fs::write(path, text + "\n").map_err(|e| format!("writing {}: {e}", path.display()))?
        }
        None => println!("{text}"),
    }
    Ok(lost || won.is_some())
}

fn kind(r: &ProofReport) -> &'static str {
    match r.proof {
        Proof::Won(_) => "won",
        Proof::Lost => "lost",
        Proof::Unfinished => "unfinished",
    }
}
