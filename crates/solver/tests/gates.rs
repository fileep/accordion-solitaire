//! Phase 4 compliance gates (PLAN.md §6.3): the solver never reports a win on the unwinnable golden deals and
//! positions, wins the winnable controls, and every win it reports passes the reference verifier.

use accordion_core::deck_input::parse_position;
use accordion_core::fast::Position;
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Outcome, SolveRecord};
use accordion_solver::{Config, Gates, Search, Verdict, solve};
use std::path::PathBuf;

fn testdata(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

fn config(seed: u64) -> Config {
    Config { max_nodes: 20_000_000, seed, ..Config::default() }
}

#[test]
fn golden_positions_get_the_right_verdict() {
    let golden: serde_json::Value = serde_json::from_str(&testdata("exhaustive.json")).unwrap();
    for case in golden["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let piles = parse_position(case["position"].as_str().unwrap()).unwrap();
        let game = Game::from_piles(piles);
        let mut search = Search::new(16, Gates::default());
        let report = solve(&Position::from_tops(&game.tops()), &config(7), &mut search, None);
        match (&report.verdict, case["winnable"].as_bool().unwrap()) {
            (Verdict::Win(line), true) => {
                let mut g = game.clone();
                for &mv in line {
                    g.apply(mv).unwrap_or_else(|e| panic!("{name}: solver move rejected: {e}"));
                }
                assert!(g.is_won(), "{name}: the line does not end in one pile");
            }
            (Verdict::Exhausted, false) => {}
            (v, w) => panic!("{name}: verdict {v:?}, but winnable = {w}"),
        }
    }
}

#[test]
fn unwinnable_golden_deals_are_never_won() {
    for file in ["unwinnable_no_moves.json", "unwinnable_dead_end.json"] {
        let (_, record) = parse_records(&testdata(&format!("games/{file}"))).remove(0);
        let deal = Deal::parse(&record.unwrap().deal.cards).unwrap();
        let mut search = Search::new(16, Gates::default());
        let report = solve(&Position::from_deal(&deal), &config(1), &mut search, None);
        assert_eq!(report.verdict, Verdict::Exhausted, "{file}");
        assert!(report.best_piles > 1, "{file}");
    }
}

#[test]
fn first_hundred_deals_are_won_and_verify() {
    let mut search = Search::new(16, Gates::default());
    for seed in 0..100 {
        search.clear_table();
        let deal = Deal::from_seed(seed);
        let report = solve(&Position::from_deal(&deal), &config(seed), &mut search, None);
        let Verdict::Win(line) = report.verdict else { panic!("seed {seed}: {:?}", report.verdict) };
        let record = SolveRecord::new(&deal, Some(seed), Outcome::Win, &line).unwrap();
        assert_eq!(verify(&record).map(|v| v.moves), Ok(51), "seed {seed}");
    }
}
