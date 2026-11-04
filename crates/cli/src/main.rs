//! `acc`: deal, verify, solve and batch-run Accordion games (docs/FORMATS.md for the file formats).

use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Outcome, SolveRecord};
use clap::{Args, Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "acc", version, about = "Accordion solitaire: deal, verify and (soon) solve games")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a deal from a seed, or check a deck you typed or pasted and print it in canonical form.
    Deal {
        #[command(flatten)]
        source: DeckSource,
        /// Print a solve-file record (result "unknown", no moves) instead of the bare cards.
        #[arg(long)]
        json: bool,
    },
    /// Replay every game in a solve file (.json or .jsonl) and check its moves and claimed result.
    Verify {
        path: PathBuf,
        /// Only print failures and the summary.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Check the fast engine against the reference engine: lockstep random playouts, then dead-end soundness.
    Selftest {
        /// Random games played with both engines in lockstep.
        #[arg(long, default_value_t = 100_000)]
        playouts: u64,
        /// Random small positions on which any dead-end test that fires must be confirmed by reference search.
        #[arg(long, default_value_t = 100_000)]
        positions: u64,
    },
    /// Measure search speed (single thread) on fixed residual positions, optionally sweeping the dead-end gates.
    Bench(bench::BenchArgs),
}

mod bench;
mod selftest;

#[derive(Args)]
#[group(required = true, multiple = false)]
struct DeckSource {
    /// Generate the deal for this seed.
    #[arg(long)]
    seed: Option<u64>,
    /// A deck as text: 52 cards like "KS 7H 10d ♣A", separated by spaces or commas.
    #[arg(long)]
    cards: Option<String>,
    /// Read the deck from a file, or "-" for standard input.
    #[arg(long)]
    file: Option<PathBuf>,
}

fn read_input(path: &PathBuf) -> Result<String, String> {
    if path.as_os_str() == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("reading standard input: {e}"))?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))
    }
}

fn deal_command(source: DeckSource, json: bool) -> Result<(), String> {
    let (deal, seed) = match (source.seed, source.cards, source.file) {
        (Some(seed), _, _) => (Deal::from_seed(seed), Some(seed)),
        (_, Some(text), _) => (Deal::parse(&text).map_err(|e| format!("invalid deck:\n{e}"))?, None),
        (_, _, Some(path)) => {
            (Deal::parse(&read_input(&path)?).map_err(|e| format!("invalid deck:\n{e}"))?, None)
        }
        (None, None, None) => unreachable!("clap requires one deck source"),
    };
    if json {
        let record = SolveRecord::new(&deal, seed, Outcome::Unknown, &[]).expect("no moves to check");
        println!("{}", record.to_json());
    } else {
        println!("{deal}");
    }
    Ok(())
}

fn verify_command(path: PathBuf, quiet: bool) -> Result<bool, String> {
    let text = read_input(&path)?;
    let records = parse_records(&text);
    if records.is_empty() {
        return Err(format!("{}: no records found", path.display()));
    }
    let mut failed = 0;
    for (line, parsed) in &records {
        let outcome =
            parsed.as_ref().map_err(|e| format!("not a valid solve record: {e}")).and_then(|record| {
                let seed = record.deal.seed.map(|s| format!("seed {s}  ")).unwrap_or_default();
                verify(record).map(|v| (seed.clone(), v)).map_err(|e| format!("{seed}{e}"))
            });
        match outcome {
            Ok((seed, v)) if !quiet => {
                let what = match v.result {
                    Outcome::Win => format!("win in {} moves", v.moves),
                    Outcome::Unknown => {
                        format!("not won yet: {} moves checked, {} piles left", v.moves, v.piles_left)
                    }
                    Outcome::Loss => format!(
                        "loss claimed ({} moves checked; a loss can't be confirmed by replay)",
                        v.moves
                    ),
                };
                println!("line {line}  {seed}ok: {what}");
            }
            Ok(_) => {}
            Err(message) => {
                failed += 1;
                println!("line {line}  FAILED: {message}");
            }
        }
    }
    let n = records.len();
    println!("{n} record{}, {} ok, {failed} failed", if n == 1 { "" } else { "s" }, n - failed);
    Ok(failed == 0)
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Deal { source, json } => deal_command(source, json).map(|()| true),
        Command::Verify { path, quiet } => verify_command(path, quiet),
        Command::Selftest { playouts, positions } => selftest::run(playouts, positions),
        Command::Bench(args) => bench::run(&args).map(|()| true),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("acc: {message}");
            ExitCode::FAILURE
        }
    }
}
