//! Golden tests: the files in testdata/ were produced by an independent Python implementation of
//! docs/FORMATS.md (testdata/tools/make_golden.py). The Rust code must agree with every one of them.

use accordion_core::deck_input::{DeckIssue, parse_position};
use accordion_core::fast::Position;
use accordion_core::selfcheck::explore_fast;
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Card, Deal, Game, Outcome};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

fn testdata(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

#[derive(Deserialize)]
struct SeedDeals {
    deals: Vec<SeedDeal>,
}

#[derive(Deserialize)]
struct SeedDeal {
    seed: u64,
    cards: String,
}

#[test]
fn seeded_deals_match_the_spec() {
    let golden: SeedDeals = serde_json::from_str(&testdata("deals_by_seed.json")).unwrap();
    assert!(golden.deals.len() >= 5);
    for d in golden.deals {
        assert_eq!(Deal::from_seed(d.seed).to_string(), d.cards, "seed {}", d.seed);
    }
}

#[derive(Deserialize)]
struct Manifest {
    games: Vec<GameFile>,
}

#[derive(Deserialize)]
struct GameFile {
    file: String,
    expect: Vec<Expect>,
}

#[derive(Deserialize)]
struct Expect {
    ok: bool,
    result: Option<Outcome>,
    moves: Option<usize>,
    piles_left: Option<usize>,
    #[serde(rename = "move")]
    move_number: Option<usize>,
}

#[test]
fn golden_games_verify_as_expected() {
    let manifest: Manifest = serde_json::from_str(&testdata("games/manifest.json")).unwrap();
    assert!(manifest.games.len() >= 10);
    for game in manifest.games {
        let records = parse_records(&testdata(&format!("games/{}", game.file)));
        assert_eq!(records.len(), game.expect.len(), "{}: record count", game.file);
        for ((line, parsed), expect) in records.iter().zip(&game.expect) {
            let label = format!("{} line {line}", game.file);
            let record = parsed.as_ref().unwrap_or_else(|e| panic!("{label}: {e}"));
            match (verify(record), expect.ok) {
                (Ok(v), true) => {
                    assert_eq!(Some(v.result), expect.result, "{label}");
                    assert_eq!(Some(v.moves), expect.moves, "{label}");
                    assert_eq!(Some(v.piles_left), expect.piles_left, "{label}");
                }
                (Err(e), false) => assert_eq!(e.move_number, expect.move_number, "{label}: {e}"),
                (Ok(v), false) => panic!("{label}: expected a failure, verified {v:?}"),
                (Err(e), true) => panic!("{label}: expected success, got {e}"),
            }
        }
    }
}

#[derive(Deserialize)]
struct Exhaustive {
    cases: Vec<ExhaustiveCase>,
}

#[derive(Deserialize)]
struct ExhaustiveCase {
    name: String,
    position: String,
    reachable: usize,
    winnable: bool,
}

/// Every distinct top-card sequence reachable from `start` with the reference engine (start included),
/// and whether any of them is a win. Legal moves depend only on top cards, so one game per sequence suffices.
fn explore(start: Game) -> (usize, bool) {
    let mut seen = HashSet::from([start.tops()]);
    let mut stack = vec![start];
    let mut winnable = false;
    while let Some(game) = stack.pop() {
        winnable |= game.is_won();
        for mv in game.legal_moves() {
            let mut next = game.clone();
            next.apply(mv).expect("legal_moves only returns legal moves");
            if seen.insert(next.tops()) {
                stack.push(next);
            }
        }
    }
    (seen.len(), winnable)
}

/// Includes deals that can't be won: the reference engine must find no win, and must agree with the Python
/// search on exactly how many positions exist, so neither side is missing or inventing moves.
#[test]
fn exhaustive_search_agrees_including_unwinnable_deals() {
    let golden: Exhaustive = serde_json::from_str(&testdata("exhaustive.json")).unwrap();
    assert!(golden.cases.iter().filter(|c| !c.winnable).count() >= 2, "need unwinnable examples");
    assert!(golden.cases.iter().any(|c| c.winnable), "need a winnable control");
    for case in golden.cases {
        let piles = parse_position(&case.position).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let game = Game::from_piles(piles);
        let fast = Position::from_tops(&game.tops());
        assert_eq!(explore(game), (case.reachable, case.winnable), "{} (reference engine)", case.name);
        assert_eq!(explore_fast(fast), (case.reachable, case.winnable), "{} (fast engine)", case.name);
    }
}

#[derive(Deserialize)]
struct InputCases {
    decks: Vec<InputCase<String>>,
    positions: Vec<InputCase<Vec<Vec<Card>>>>,
}

#[derive(Deserialize)]
struct InputCase<T> {
    name: String,
    input: String,
    ok: Option<T>,
    issues: Option<Vec<DeckIssue>>,
}

#[test]
fn deck_and_position_input_match_the_spec() {
    let cases: InputCases = serde_json::from_str(&testdata("deck_input.json")).unwrap();
    for case in &cases.decks {
        match (Deal::parse(&case.input), &case.ok, &case.issues) {
            (Ok(deal), Some(expected), None) => assert_eq!(&deal.to_string(), expected, "{}", case.name),
            (Err(e), None, Some(expected)) => assert_eq!(&e.issues, expected, "{}", case.name),
            (got, _, _) => panic!("{}: unexpected {got:?}", case.name),
        }
    }
    for case in &cases.positions {
        match (parse_position(&case.input), &case.ok, &case.issues) {
            (Ok(piles), Some(expected), None) => assert_eq!(&piles, expected, "{}", case.name),
            (Err(e), None, Some(expected)) => assert_eq!(&e.issues, expected, "{}", case.name),
            (got, _, _) => panic!("{}: unexpected {got:?}", case.name),
        }
    }
}
