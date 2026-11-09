//! `acc`: deal, verify, solve and batch-run Accordion games (docs/FORMATS.md for the file formats).

use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Outcome, SolveRecord};
use clap::{Args, Parser, Subcommand};
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "acc",
    version,
    about = "Accordion solitaire: solve deals, check solutions, and see how often the game can be won"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the cards of a numbered deal, or check a deck you typed or pasted.
    Deal {
        #[command(flatten)]
        source: DeckSource,
        /// Print a solve-file record (result "unknown", no moves) instead of the bare cards.
        #[arg(long)]
        json: bool,
    },
    /// Check the moves in a solve file by replaying them under the rules.
    Verify {
        path: PathBuf,
        /// Only print failures and the summary.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Check this build: the fast solver against a slow, simple version of the same rules.
    Selftest {
        /// Random games played with both engines in lockstep.
        #[arg(long, default_value_t = 100_000, value_parser = numbers::parse_count)]
        playouts: u64,
        /// Random small positions on which any dead-end test that fires must be confirmed by reference search.
        #[arg(long, default_value_t = 100_000, value_parser = numbers::parse_count)]
        positions: u64,
        /// Random residuals on which sleep sets and endgame tables must agree with plain search.
        #[arg(long, default_value_t = 100_000, value_parser = numbers::parse_count)]
        residuals: u64,
        /// Random positions on which the exhaustive prover must agree with a plain recursive search.
        #[arg(long, default_value_t = 3_000, value_parser = numbers::parse_count)]
        proofs: u64,
        /// Endgame tables to check (the largest found here; tables to 8 piles are built if there are none).
        #[arg(long, default_value = "endgame")]
        endgame_dir: PathBuf,
    },
    /// Measure how fast the search is on one thread (for developers).
    Bench(bench::BenchArgs),
    /// Find a winning line for one deal, and print it as a verified solve file.
    Solve(solve::SolveArgs),
    /// Solve a range of numbered deals, then report how many were won and how fast.
    Batch(Box<batch::BatchArgs>),
    /// Compare solver settings on a fixed set of deals (for developers).
    Experiment(experiment::ExperimentArgs),
    /// Build the tables of small endings that make solving faster.
    Endgame(endgame::EndgameArgs),
    /// Prove that a deal can't be won, by two separate complete searches (nearly stuck deals only).
    Prove(prove::ProveArgs),
}

mod batch;
mod bench;
mod dossier;
mod endgame;
mod experiment;
mod games;
mod numbers;
mod pause;
mod progress;
mod prove;
mod runner;
mod selftest;
mod solve;

#[derive(Args)]
#[group(required = true, multiple = false)]
struct DeckSource {
    /// Generate the deal for this seed.
    #[arg(long)]
    seed: Option<u64>,
    /// A deck as text: 52 cards like "KS 7H 10d A♣", separated by spaces or commas.
    #[arg(long)]
    cards: Option<String>,
    /// Read the deck from a file, or "-" for standard input.
    #[arg(long)]
    file: Option<PathBuf>,
}

/// Text from a file, or standard input for "-". Windows tools like to start a file with a byte order mark, and PowerShell 5.1
/// to write UTF-16, so those are read as well.
fn read_input(path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    if path.as_os_str() == "-" {
        std::io::stdin().read_to_end(&mut bytes).map_err(|e| format!("reading standard input: {e}"))?;
    } else {
        bytes = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    }
    decode(&bytes).ok_or_else(|| format!("reading {}: this is not text in UTF-8 or UTF-16", path.display()))
}

/// `bytes` as UTF-8, or as what a byte order mark at the start says.
fn decode(bytes: &[u8]) -> Option<String> {
    let utf16 = |rest: &[u8], word: fn([u8; 2]) -> u16| {
        let (pairs, odd) = rest.as_chunks::<2>();
        if odd.is_empty() {
            String::from_utf16(&pairs.iter().map(|&p| word(p)).collect::<Vec<_>>()).ok()
        } else {
            None
        }
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

/// A setting that is JSON: the JSON itself, or if it doesn't start with `{`, the name of a file holding it.
fn json_setting<T: DeserializeOwned>(flag: &str, value: &str) -> Result<T, String> {
    let inline = value.trim_start().starts_with('{');
    let text = if inline { value.to_string() } else { read_input(Path::new(value))? };
    serde_json::from_str(&text).map_err(|e| {
        if inline && !text.contains('"') {
            format!("{flag}: {e}: your shell removed the quotes; put the JSON in a file and pass its name")
        } else {
            format!("{flag}: {e}")
        }
    })
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
        Command::Selftest { playouts, positions, residuals, proofs, endgame_dir } => {
            selftest::run(playouts, positions, residuals, proofs, &endgame_dir)
        }
        Command::Bench(args) => bench::run(&args).map(|()| true),
        Command::Solve(args) => solve::run(&args),
        Command::Batch(args) => batch::run(&args),
        Command::Experiment(args) => experiment::run(&args),
        Command::Endgame(args) => endgame::run(&args).map(|()| true),
        Command::Prove(args) => prove::run(&args),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_read_with_or_without_a_byte_order_mark() {
        assert_eq!(decode(b"7H"), Some("7H".to_string()));
        assert_eq!(decode(b"\xEF\xBB\xBF7H"), Some("7H".to_string()));
        assert_eq!(decode(b"\xFF\xFE7\x00H\x00"), Some("7H".to_string()));
        assert_eq!(decode(b"\xFE\xFF\x007\x00H"), Some("7H".to_string()));
        assert_eq!(decode(b""), Some(String::new()));
        assert_eq!(decode(b"\xFF\xFE7"), None, "half a character");
        assert_eq!(decode(b"\xE9"), None, "neither");
    }

    #[test]
    fn the_cards_in_the_help_can_be_typed() {
        let mut deck: Vec<String> =
            Deal::from_seed(1).to_string().split_whitespace().map(String::from).collect();
        // The forms the help shows, in place of however the deal happens to write those four cards.
        for (plain, typed) in [("KS", "KS"), ("7H", "7H"), ("TD", "10d"), ("AC", "A♣")] {
            let card = deck.iter_mut().find(|c| *c == plain).unwrap_or_else(|| panic!("{plain}"));
            *card = typed.to_string();
        }
        assert_eq!(
            Deal::parse(&deck.join(" ")).expect("the help's forms").to_string(),
            Deal::from_seed(1).to_string()
        );
        assert!(Deal::parse(&deck.join(" ").replace("A♣", "♣A")).is_err());
    }
}
