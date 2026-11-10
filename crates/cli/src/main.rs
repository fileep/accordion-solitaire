//! `acc`: deal, verify, solve and batch-run Accordion games (docs/FORMATS.md for the file formats), and the commands in
//! front of those for running it without knowing them ([`front`]).

use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Outcome, SolveRecord};
use clap::{Args, Parser, Subcommand};
use serde::de::DeserializeOwned;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The commands, by what they are for, as `acc help` lists them (in a macro, so that the help can be put together from it
/// when the program is built). Each visible command must be here, which a test checks.
macro_rules! commands {
    () => {
        "\
Start here:
  doctor    Check that this computer is ready, in a few seconds
  run       Play a range of deals and count how many are won
  status    See how the runs started here are going
  pause     Pause a run that is going in another window
  resume    Carry on with a paused run
  results   See what a run has found, finished or not

One deal:
  deal      Show the cards of a numbered deal, or check a deck you typed
  show      Show a deal and a way to win it, move by move
  solve     Find a winning line for one deal, and print it as a solve file
  prove     Prove that a deal can't be won
  verify    Check solve files, replaying every move under the rules

Advanced:
  batch     The engine behind acc run, with all its settings
  settings  Print the solver's settings, as a file to edit
  endgame   Build the tables of small endings that make solving a little faster
  selftest  Check this build in depth: the fast solver against a slow, simple one

For developers: acc bench and acc experiment (docs/COMMANDS.md)."
    };
}

const HELP: &str = concat!(
    "{about}\n\n{usage-heading} {usage}\n\n",
    commands!(),
    "\n\nOptions:\n{options}\n\n",
    "acc help <command> shows more about one. The documentation starts at docs/GETTING_STARTED.md."
);

/// What bare `acc` says.
const START: &str = "\
Accordion is a solitaire game. The 52 cards are dealt face up in a row, and a pile may be moved
onto the pile next to it, or onto the one three to its left, when their top cards have the same
suit or the same rank. It is won when all the cards are in one pile. acc plays numbered deals,
finds a way to win each one it can, and counts how many it won.

To begin, type these one at a time:
  acc doctor                   check that this computer is ready (a few seconds)
  acc run --quick              play 10,000 deals and see how many are won
  acc show --seed 1 --solve    see one deal won, move by move";

#[derive(Parser)]
#[command(
    name = "acc",
    bin_name = "acc",
    version,
    about = "acc: Accordion solitaire, played and counted deal by deal",
    help_template = HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

const DEAL_EXAMPLES: &str = "\
Examples:
  acc deal --seed 42                       the cards of deal 42
  acc deal --file deck.txt                 check that a deck in a file is 52 different cards
  acc deal --seed 42 --json                as a solve-file record";

const VERIFY_EXAMPLES: &str = "\
Examples:
  acc verify win.json                      a solve file from acc solve --out
  acc verify wins.jsonl                    every line saved by acc run --save-wins
  acc verify output/hard-deals/proofs      every loss record in a folder";

const SELFTEST_EXAMPLES: &str = "\
Examples:
  acc selftest                             every check, at full size (some minutes)
  acc selftest --playouts 10k --positions 10k --residuals 1k --proofs 300
                                           fewer cases, as acc doctor --full does";

#[derive(Subcommand)]
enum Command {
    /// Check that this computer is ready: build, CPU, memory, disk, GPU, battery, and a few seconds of solving.
    Doctor(front::doctor::DoctorArgs),
    /// Play a range of deals and count how many are won, pausing and carrying on as needed.
    Run(front::run::RunArgs),
    /// See how the runs started in this folder are going.
    Status(front::status::StatusArgs),
    /// Pause a run that is going in another window (Ctrl+C in its own window does the same).
    Pause(front::status::PauseArgs),
    /// Carry on with a paused run.
    Resume(front::run::ResumeArgs),
    /// See what a run has found: its report, or the win rate so far.
    Results(front::status::ResultsArgs),
    /// Show the cards of a numbered deal, or check a deck you typed or pasted.
    #[command(after_help = DEAL_EXAMPLES)]
    Deal {
        #[command(flatten)]
        source: DeckSource,
        /// Print a solve-file record (result "unknown", no moves) instead of the bare cards.
        #[arg(long)]
        json: bool,
    },
    /// Show a deal laid out to read, and a way to win it move by move.
    Show(front::show::ShowArgs),
    /// Find a winning line for one deal, and print it as a verified solve file.
    Solve(solve::SolveArgs),
    /// Prove that a deal can't be won, by two separate complete searches (nearly stuck deals only).
    Prove(prove::ProveArgs),
    /// Check solve files by replaying every move under the rules: files, or folders of them.
    #[command(after_help = VERIFY_EXAMPLES)]
    Verify {
        /// Solve files (.json, .jsonl), folders of them, or "-" for standard input.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Only print failures and the summary.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Solve a range of numbered deals, then report how many were won and how fast: the engine behind acc run.
    Batch(Box<batch::BatchArgs>),
    /// Print the solver's settings (or the GPU's), as a file to edit for acc run --settings.
    Settings(front::show::SettingsArgs),
    /// Build the tables of small endings that make solving faster.
    Endgame(endgame::EndgameArgs),
    /// Check this build: the fast solver against a slow, simple version of the same rules.
    #[command(after_help = SELFTEST_EXAMPLES)]
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
    #[command(hide = true)]
    Bench(bench::BenchArgs),
    /// Compare solver settings on a fixed set of deals (for developers).
    #[command(hide = true)]
    Experiment(experiment::ExperimentArgs),
}

mod batch;
mod bench;
mod dossier;
mod endgame;
mod experiment;
mod front;
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

/// The deal of a dossier (`accordion-dossier/1`), the notes kept on a deal that wasn't won.
pub struct DossierDeal {
    pub id: String,
    pub cards: String,
    pub seed: Option<u64>,
}

/// The deal in `text`, if it is a dossier.
pub fn dossier_of(text: &str) -> Option<DossierDeal> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if !value["format"].as_str()?.starts_with("accordion-dossier") {
        return None;
    }
    Some(DossierDeal {
        id: value["id"].as_str()?.to_string(),
        cards: value["deal"]["cards"].as_str()?.to_string(),
        seed: value["deal"]["seed"].as_u64(),
    })
}

/// The files `paths` stand for: a folder for the solve files in it (not in the folders inside it).
fn files_of(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for path in paths {
        if !path.is_dir() {
            files.push(path.clone());
            continue;
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json" || x == "jsonl"))
            .collect();
        if found.is_empty() {
            return Err(format!("{}: there are no .json or .jsonl files in this folder", path.display()));
        }
        found.sort();
        files.extend(found);
    }
    Ok(files)
}

fn verify_command(paths: &[PathBuf], quiet: bool) -> Result<bool, String> {
    let files = files_of(paths)?;
    let several = files.len() > 1;
    let (mut all, mut all_failed) = (0, 0);
    for path in &files {
        if several {
            println!("{}:", path.display());
        }
        let text = read_input(path)?;
        if let Some(dossier) = dossier_of(&text) {
            let dir = path.parent().unwrap_or(Path::new(""));
            let record = dir.join("proofs").join(format!("{}-loss.json", dossier.id));
            println!("this is a dossier: the notes kept on a deal that wasn't won, not a solve file");
            if record.exists() {
                println!("its loss record is {}: acc verify {0} checks that", record.display());
            } else {
                println!("it has no loss record in {} (not proven lost there)", dir.join("proofs").display());
            }
            continue;
        }
        let records = parse_records(&text);
        if records.is_empty() {
            if !several {
                return Err(format!("{}: no records found", path.display()));
            }
            println!("FAILED: no records found");
            all_failed += 1;
            continue;
        }
        let mut failed = 0;
        for (line, parsed) in &records {
            let outcome =
                parsed.as_ref().map_err(|e| format!("not a valid solve record: {e}")).and_then(|record| {
                    let seed = record.deal.seed.map(|s| format!("seed {s}  ")).unwrap_or_default();
                    verify(record).map(|v| (seed.clone(), v, record)).map_err(|e| format!("{seed}{e}"))
                });
            match outcome {
                Ok((seed, v, record)) if !quiet => {
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
                    if v.result == Outcome::Loss {
                        let deal = match record.deal.seed {
                            Some(s) => format!("--seed {s}"),
                            None => format!("--cards \"{}\"", record.deal.cards),
                        };
                        println!("line {line}  to re-check this, run: acc prove {deal}");
                    }
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
        all += n;
        all_failed += failed;
    }
    if several {
        println!("in all: {} files, {all} records, {all_failed} failed", files.len());
    }
    Ok(all_failed == 0)
}

fn exit_code(result: Result<u8, String>) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("acc: {message}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // On Linux, this may start acc again with a copy of NVRTC on the loader's path; before anything else is done.
    accordion_gpu::restart_with_bundled_nvrtc();
    let Some(command) = cli.command else {
        println!(
            "acc {}: Accordion solitaire, played and counted deal by deal.\n",
            env!("CARGO_PKG_VERSION")
        );
        println!(
            "{START}\n\n{}\n\nacc help <command> shows more about one. The documentation starts at docs/GETTING_STARTED.md.",
            commands!()
        );
        return ExitCode::SUCCESS;
    };
    // The commands that see a run through give back its exit code.
    let command = match command {
        Command::Run(args) => return exit_code(front::run::run(&args)),
        Command::Resume(args) => return exit_code(front::run::resume(&args)),
        other => other,
    };
    let result = match command {
        Command::Doctor(args) => front::doctor::run(&args),
        Command::Status(args) => front::status::status(&args),
        Command::Pause(args) => front::status::pause(&args),
        Command::Results(args) => front::status::results(&args),
        Command::Show(args) => front::show::show(&args),
        Command::Settings(args) => front::show::settings(&args),
        Command::Deal { source, json } => deal_command(source, json).map(|()| true),
        Command::Verify { paths, quiet } => verify_command(&paths, quiet),
        Command::Selftest { playouts, positions, residuals, proofs, endgame_dir } => {
            selftest::run(playouts, positions, residuals, proofs, &endgame_dir)
        }
        Command::Bench(args) => bench::run(&args).map(|()| true),
        Command::Solve(args) => solve::run(&args),
        Command::Batch(args) => batch::run(&args),
        Command::Experiment(args) => experiment::run(&args),
        Command::Endgame(args) => endgame::run(&args).map(|()| true),
        Command::Prove(args) => prove::run(&args),
        Command::Run(_) | Command::Resume(_) => unreachable!("handled above"),
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
    use clap::CommandFactory;

    #[test]
    fn the_help_lists_every_command() {
        let listed = commands!();
        for command in Cli::command().get_subcommands() {
            let name = command.get_name();
            if command.is_hide_set() {
                assert!(listed.contains(&format!("acc {name}")), "{name} on the developers' line");
            } else {
                assert!(listed.contains(&format!("\n  {name} ")), "{name} in the list");
                assert!(command.get_after_help().is_some(), "{name} has examples");
            }
        }
        let names: Vec<String> = Cli::command().get_subcommands().map(|c| c.get_name().to_string()).collect();
        for line in listed.lines().filter(|l| l.starts_with("  ")) {
            let name = line.split_whitespace().next().unwrap();
            assert!(names.iter().any(|n| n == name), "{name} is listed but isn't a command");
        }
    }

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
