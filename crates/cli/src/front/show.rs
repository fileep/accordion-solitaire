//! `acc show`: a deal laid out to read, with a winning line in words when there is one; and `acc settings`: the settings
//! the solver and the GPU use unless told otherwise, as a file to edit.

use crate::read_input;
use crate::solve::SolveArgs;
use accordion_core::notation::{CardMove, decode_compact};
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Move, Outcome, SolveRecord};
use clap::{Args, Parser};
use std::path::PathBuf;

const SHOW_EXAMPLES: &str = "\
Examples:
  acc show --seed 42                 deal number 42
  acc show --seed 42 --solve         and a way to win it, move by move
  acc show win.json                  a solve file (acc solve --out win.json writes one)
  acc show --cards \"KS 7H 10d A♣ ...\" --solve
                                     a deck you typed: all 52 cards, separated by spaces or commas";

const SETTINGS_EXAMPLES: &str = "\
Examples:
  acc settings                                   print them
  acc settings --out my-settings.json            save them in a file to edit
  acc run --settings my-settings.json            then run with the file";

/// The rules, as one line says them.
const RULES: &str = "Rules: all 52 cards dealt face up in a row; a pile moves onto the pile next to it, or the one three \
                     to its left, when their top cards match in suit or rank; won when one pile is left.";

#[derive(Args)]
#[command(after_help = SHOW_EXAMPLES)]
pub struct ShowArgs {
    /// Deal number N.
    #[arg(long, conflicts_with_all = ["cards", "file"])]
    seed: Option<u64>,
    /// A deck as text: 52 cards like "KS 7H 10d A♣", separated by spaces or commas.
    #[arg(long, conflicts_with = "file")]
    cards: Option<String>,
    /// A solve file (with a line to show) or a file holding a deck.
    #[arg(required_unless_present_any = ["seed", "cards"])]
    file: Option<PathBuf>,
    /// Find a winning line first, as acc solve does.
    #[arg(long)]
    solve: bool,
    /// Threads for --solve (0 = every one).
    #[arg(long, default_value_t = 0)]
    threads: usize,
}

#[derive(Args)]
#[command(after_help = SETTINGS_EXAMPLES)]
pub struct SettingsArgs {
    /// The GPU's settings instead of the solver's.
    #[arg(long)]
    gpu: bool,
    /// Write them to this file instead of printing them.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
}

/// What to show: a deal, and a line from its start if there is one.
struct Shown {
    deal: Deal,
    seed: Option<u64>,
    line: Option<(Outcome, Vec<Move>)>,
}

/// `acc solve`'s arguments, for running it from here.
#[derive(Parser)]
struct Solve {
    #[command(flatten)]
    args: SolveArgs,
}

pub fn show(args: &ShowArgs) -> Result<bool, String> {
    let mut shown = match (&args.seed, &args.cards, &args.file) {
        (Some(seed), ..) => Shown { deal: Deal::from_seed(*seed), seed: Some(*seed), line: None },
        (_, Some(cards), _) => Shown {
            deal: Deal::parse(cards).map_err(|e| format!("invalid deck:\n{e}"))?,
            seed: None,
            line: None,
        },
        (_, _, Some(path)) => from_file(path)?,
        _ => unreachable!("clap requires a deal"),
    };
    if args.solve {
        let mut words = vec!["solve".to_string(), "--threads".into(), args.threads.to_string()];
        match shown.seed {
            Some(seed) => words.extend(["--seed".into(), seed.to_string()]),
            None => words.extend(["--cards".into(), shown.deal.to_string()]),
        }
        let solve = Solve::try_parse_from(&words).map_err(|e| e.to_string())?;
        let (text, _) = crate::solve::solve_text(&solve.args)?;
        let record = parse_records(&text).into_iter().next().and_then(|(_, r)| r.ok());
        let record = record.ok_or("internal error: acc solve's record couldn't be read back")?;
        shown.line = Some((record.result, moves_of(&record)?));
    }
    print(&shown);
    Ok(true)
}

/// A solve file's first record, a dossier's deal, or a deck.
fn from_file(path: &std::path::Path) -> Result<Shown, String> {
    let text = read_input(path)?;
    let records = parse_records(&text);
    if let Some((_, Ok(record))) = records.first() {
        verify(record).map_err(|e| format!("{}: {e}", path.display()))?;
        let deal = Deal::parse(&record.deal.cards).map_err(|e| format!("{}: {e}", path.display()))?;
        if records.len() > 1 {
            eprintln!("note: {} has {} records; this is the first", path.display(), records.len());
        }
        return Ok(Shown { deal, seed: record.deal.seed, line: Some((record.result, moves_of(record)?)) });
    }
    if let Some(dossier) = crate::dossier_of(&text) {
        eprintln!(
            "note: {} is a dossier, the notes kept on a deal that wasn't won; this is its deal",
            path.display()
        );
        let deal = Deal::parse(&dossier.cards).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(Shown { deal, seed: dossier.seed, line: None });
    }
    let deal =
        Deal::parse(&text).map_err(|e| format!("{}: not a solve file, nor a deck:\n{e}", path.display()))?;
    Ok(Shown { deal, seed: None, line: None })
}

/// A record's moves, which [`verify`] has checked, as moves.
fn moves_of(record: &SolveRecord) -> Result<Vec<Move>, String> {
    if record.moves.is_empty()
        && let Some(compact) = &record.moves_b64
    {
        return decode_compact(compact);
    }
    let deal = Deal::parse(&record.deal.cards).map_err(|e| e.to_string())?;
    let mut game = Game::new(&deal);
    record
        .moves
        .iter()
        .map(|text| {
            let card_move = CardMove::parse(text)?;
            let mv =
                game.move_between(card_move.mover, card_move.target).map_err(|e| format!("{text}: {e}"))?;
            game.apply(mv).map_err(|e| e.to_string())?;
            Ok(mv)
        })
        .collect()
}

fn print(shown: &Shown) {
    let which = shown.seed.map_or("A deck".to_string(), |seed| format!("Deal {seed}"));
    println!("{which}. {RULES}\n");
    let cards: Vec<String> = shown.deal.to_string().split_whitespace().map(String::from).collect();
    for (row, chunk) in cards.chunks(13).enumerate() {
        println!("  {:>2}-{:<2}  {}", row * 13 + 1, row * 13 + 13, chunk.join(" "));
    }
    let Some((outcome, line)) = &shown.line else {
        let again = match shown.seed {
            Some(seed) => format!("acc show --seed {seed} --solve"),
            None => "the same command with --solve".into(),
        };
        println!("\n{again} finds a way to win it.");
        return;
    };
    let mut game = Game::new(&shown.deal);
    println!(
        "\n{}",
        match outcome {
            Outcome::Win => format!("A winning line, {} moves, every one checked by the rules:", line.len()),
            Outcome::Loss => "Proven impossible. The furthest line found:".to_string(),
            Outcome::Unknown => "Not won within the limit. The furthest line found:".to_string(),
        }
    );
    for (i, &mv) in line.iter().enumerate() {
        let (mover, target) = game.cards_of(mv);
        if game.apply(mv).is_err() {
            break;
        }
        let left = game.pile_count();
        println!(
            "  {:>2}. {mover} onto {target}, {left} pile{} left",
            i + 1,
            if left == 1 { "" } else { "s" }
        );
    }
    if *outcome != Outcome::Win {
        println!(
            "\nacc prove{} tries to prove that no line wins.",
            shown.seed.map_or(String::new(), |s| format!(" --seed {s}"))
        );
    }
}

pub fn settings(args: &SettingsArgs) -> Result<bool, String> {
    let text = if args.gpu {
        serde_json::to_string_pretty(&accordion_gpu::Settings::default())
    } else {
        serde_json::to_string_pretty(&accordion_solver::Config::default())
    }
    .expect("serializable")
        + "\n";
    if !args.gpu {
        eprintln!("note: runs take the positions limit, max_nodes, from --max-nodes, whatever the file says");
    }
    match &args.out {
        None => print!("{text}"),
        Some(path) if path.exists() => {
            return Err(format!(
                "{} is already there: give another name, or delete it first",
                path.display()
            ));
        }
        Some(path) => {
            std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
            let flag = if args.gpu { "--gpu-settings" } else { "--settings" };
            println!(
                "Wrote {}. Change what you like in it, then: acc run {flag} {}\n(Fields you take out keep their defaults; docs/COMMANDS.md says what each does.)",
                path.display(),
                path.display()
            );
        }
    }
    Ok(true)
}
