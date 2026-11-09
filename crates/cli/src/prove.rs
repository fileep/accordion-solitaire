//! `acc prove`: exhaustive search, to prove a deal or position lost (PLAN.md §5 stage 3). Two independent runs, each
//! with an exact set of dead positions, must both finish without a win before a loss is claimed: one with everything
//! the solver uses (random order, dead-end tests, sleep sets, endgame tables), which goes first because it finishes
//! soonest, and a plain one (rules only, fixed order) to confirm it. A proven loss is written as a `loss` record and
//! marks the deal's dossier `loss (proven)`. `acc batch` proves the deals it doesn't win the same way
//! ([`prove_both`]). Exhaustive search of a typical deal is far out of reach; a lost deal can be small.

use crate::dossier;
use crate::runner;
use crate::solve::{SolveSource, Source};
use accordion_core::fast::Position;
use accordion_core::notation::CardMove;
use accordion_core::verify::verify;
use accordion_core::{Deal, Move, Outcome, SolveRecord};
use accordion_solver::{Endgame, Proof, ProofReport, ProveOptions, prove};
use clap::Args;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Instant;

/// Positions each run may generate unless told otherwise: about half an hour of one core apiece.
pub const DEFAULT_POSITIONS: u64 = 10_000_000_000;

const EXAMPLES: &str = "\
Examples:
  acc prove --seed 1005616798
  acc prove --seed 1005616798 --max-positions 2b --memory 4";

#[derive(Args)]
#[command(after_help = EXAMPLES)]
pub struct ProveArgs {
    #[command(flatten)]
    source: SolveSource,
    /// Positions each of the two runs may generate before giving up.
    #[arg(long, default_value_t = DEFAULT_POSITIONS, value_parser = crate::numbers::parse_count)]
    max_positions: u64,
    /// Memory each run's set of dead positions may take, in GB; past it, positions are forgotten and searched again.
    #[arg(long, default_value_t = 8)]
    memory: usize,
    /// Seed for the independent run's move order.
    #[arg(long, default_value_t = 1)]
    rng_seed: u64,
    /// The independent run stores positions of up to this many piles (at most 16) by suit/rank pattern, so positions
    /// that are the same game with suits and ranks renamed are searched once (0: by cards only).
    #[arg(long, default_value_t = 16)]
    pattern_piles: usize,
    /// Write the result (a `loss` or `win` solve record, for a deal) here instead of standard output.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where the deal's dossier is.
    #[arg(long, default_value = "output/hard-deals")]
    dossiers: PathBuf,
    #[arg(long, default_value = "endgame")]
    endgame_dir: PathBuf,
    /// Endgame tables for the independent run: a pile count, `off`, or `auto` (the largest found, up to 11).
    #[arg(long, default_value = "auto")]
    endgame: String,
}

/// How far each run may go.
#[derive(Clone, Copy)]
pub struct Limits {
    pub positions: u64,
    pub bytes: usize,
    /// The independent run's move-order seed.
    pub rng_seed: u64,
    /// The independent run's [`ProveOptions::pattern_piles`].
    pub pattern_piles: usize,
}

/// What proving found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// Both runs searched every line: no win.
    Lost,
    /// A winning line from the root.
    Won(Vec<Move>),
    /// A run hit its limit first.
    Undecided,
}

/// Both runs of a proof and what they found.
pub struct Proving {
    pub independent: (ProofReport, f64),
    /// Run only when the independent run finished without a win.
    pub plain: Option<(ProofReport, f64)>,
    pub finding: Finding,
    /// The runs as recorded in dossiers and solve records.
    pub json: Value,
}

impl Proving {
    /// Fewest piles either run reached, and the line there: the win itself, if there is one.
    pub fn best(&self) -> (usize, &[Move]) {
        if let Finding::Won(line) = &self.finding {
            return (1, line);
        }
        let runs = std::iter::once(&self.independent.0).chain(self.plain.as_ref().map(|(r, _)| r));
        let best = runs.min_by_key(|r| r.best_piles).expect("the independent run");
        (best.best_piles, &best.best_line)
    }

    /// The deal's solve record (a win, a loss with the line that gets furthest, or unknown), checked by the
    /// reference engine.
    pub fn record(&self, deal: &Deal, seed: Option<u64>) -> Result<SolveRecord, String> {
        let (outcome, line) = match &self.finding {
            Finding::Won(line) => (Outcome::Win, line.as_slice()),
            Finding::Lost => (Outcome::Loss, self.best().1),
            Finding::Undecided => (Outcome::Unknown, self.best().1),
        };
        let mut record =
            SolveRecord::new(deal, seed, outcome, line).map_err(|e| format!("internal error: {e}"))?;
        record.stats = json!({ "proof": self.json }).as_object().cloned();
        verify(&record).map_err(|e| format!("the line failed verification: {e}"))?;
        Ok(record)
    }
}

fn summary(r: &ProofReport, seconds: f64) -> Value {
    json!({
        "result": kind(r),
        "positions": r.positions,
        "dead_positions": r.dead,
        "evicted_positions": r.evicted,
        "set_bytes": r.set_bytes,
        "seconds": (seconds * 1000.0).round() / 1000.0,
    })
}

fn kind(r: &ProofReport) -> &'static str {
    match r.proof {
        Proof::Won(_) => "won",
        Proof::Lost => "lost",
        Proof::Unfinished => "unfinished",
    }
}

fn timed(root: &Position, opts: &ProveOptions, tables: Option<&Endgame>) -> (ProofReport, f64) {
    let t = Instant::now();
    let report = prove(root, opts, tables);
    (report, t.elapsed().as_secs_f64())
}

/// Proves `root` lost if it can: the independent run first, then the plain run if that found no win. `say` gets a
/// line for each run. Two finished runs that disagree are a bug, never a result: that is the error.
pub fn prove_both(
    root: &Position,
    limits: &Limits,
    tables: Option<&Endgame>,
    mut say: impl FnMut(String),
) -> Result<Proving, String> {
    let line = |name: &str, (r, s): &(ProofReport, f64)| {
        let evicted =
            if r.evicted > 0 { format!(", {} forgotten to fit in memory", r.evicted) } else { String::new() };
        format!(
            "{name} {} after {} positions ({} dead, {} MB{evicted}) in {s:.2} s",
            kind(r),
            r.positions,
            r.dead,
            r.set_bytes >> 20
        )
    };
    let opts = ProveOptions {
        max_bytes: limits.bytes,
        pattern_piles: limits.pattern_piles,
        ..ProveOptions::independent(limits.rng_seed, limits.positions)
    };
    let independent = timed(root, &opts, tables);
    say(line("independent run:", &independent));
    let plain = (independent.0.proof == Proof::Lost).then(|| {
        let opts = ProveOptions { max_bytes: limits.bytes, ..ProveOptions::plain(limits.positions) };
        let plain = timed(root, &opts, None);
        say(line("plain run:      ", &plain));
        plain
    });
    let finding = match (&independent.0.proof, plain.as_ref().map(|p| &p.0.proof)) {
        (Proof::Won(line), _) => Finding::Won(line.clone()),
        (Proof::Lost, Some(Proof::Lost)) => Finding::Lost,
        (Proof::Lost, Some(Proof::Won(_))) => {
            return Err("the two runs DISAGREE (one proved a loss, one won): this is a bug".into());
        }
        _ => Finding::Undecided,
    };
    let json = json!({
        "solver": format!("acc {}", env!("CARGO_PKG_VERSION")),
        "max_positions": limits.positions,
        "independent": {
            "move_order_seed": limits.rng_seed,
            "pattern_piles": limits.pattern_piles,
            "endgame_piles": tables.map_or(0, Endgame::max_piles),
            "run": summary(&independent.0, independent.1),
        },
        "plain": plain.as_ref().map(|(r, s)| summary(r, *s)),
    });
    Ok(Proving { independent, plain, finding, json })
}

pub fn run(args: &ProveArgs) -> Result<bool, String> {
    crate::numbers::check_gigabytes("--memory", args.memory as u64)?;
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
    let limits = Limits {
        positions: args.max_positions,
        bytes: args.memory << 30,
        rng_seed: args.rng_seed,
        pattern_piles: args.pattern_piles,
    };
    let proving = prove_both(&root, &limits, tables.as_ref(), |line| eprintln!("  {line}"))
        .map_err(|e| format!("{e}; nothing was written"))?;
    eprintln!(
        "{}",
        match proving.finding {
            Finding::Lost => "proven lost: both exhaustive runs finished without a win",
            Finding::Won(_) => "won (so not a loss)",
            Finding::Undecided => "not decided: a run hit its limit",
        }
    );

    let text = match &deal {
        Some(deal) => {
            // A deal that turned out winnable has nothing to record; a loss or an unfinished proof does.
            if !matches!(proving.finding, Finding::Won(_)) {
                let path = dossier::record_proof(
                    &args.dossiers,
                    deal,
                    seed,
                    &proving.finding,
                    proving.best(),
                    proving.json.clone(),
                )?;
                dossier::write_index(&args.dossiers)?;
                eprintln!("dossier: {}", path.display());
            }
            proving.record(deal, seed).map_err(|e| format!("internal error: {e}"))?.to_json()
        }
        None => {
            let mut g = game.clone();
            let line = match &proving.finding {
                Finding::Won(line) => line.as_slice(),
                _ => proving.best().1,
            };
            let mut moves = Vec::new();
            for &mv in line {
                let (mover, target) = g.cards_of(mv);
                g.apply(mv).map_err(|e| format!("internal error: move rejected: {e}"))?;
                moves.push(CardMove { mover, target }.to_string());
            }
            let result = match proving.finding {
                Finding::Lost => "loss",
                Finding::Won(_) => "win",
                Finding::Undecided => "unknown",
            };
            serde_json::to_string_pretty(&json!({ "result": result, "moves": moves, "proof": proving.json }))
                .expect("serializable")
        }
    };
    match &args.out {
        Some(path) => {
            std::fs::write(path, text + "\n").map_err(|e| format!("writing {}: {e}", path.display()))?
        }
        None => println!("{text}"),
    }
    Ok(proving.finding != Finding::Undecided)
}
