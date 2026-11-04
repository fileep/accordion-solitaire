//! Compliance gates (PLAN.md §6.3, Phases 4–5): the solver never reports a win on the unwinnable golden deals and
//! positions, wins the winnable controls, and every win it reports passes the reference verifier. Each check runs
//! with the default strategy (ladder + sleep sets), with endgame tables, and with the Phase 4 solver; and with every
//! fast stage forced to fail, the thorough search must still win every deal.

use accordion_core::deck_input::parse_position;
use accordion_core::fast::Position;
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Deal, Game, Outcome, SolveRecord};
use accordion_solver::{Config, Endgame, Gates, Ladder, Search, Stage, Verdict, solve};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

fn testdata(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

fn config(seed: u64) -> Config {
    Config { max_nodes: 20_000_000, seed, ..Config::default() }
}

/// Endgame tables up to 8 piles (built once, in about a second).
fn tables() -> Arc<Endgame> {
    static TABLES: OnceLock<Arc<Endgame>> = OnceLock::new();
    TABLES.get_or_init(|| Arc::new(Endgame::build(8, 4, |_, _, _| {}))).clone()
}

/// The strategies every gate runs under: the default, the default with endgame tables, and Phase 4's.
fn strategies(seed: u64) -> Vec<(&'static str, Config, Search)> {
    let mut with_tables = config(seed);
    with_tables.search.endgame_piles = 8;
    let mut tabled = Search::with_options(16, with_tables.search);
    tabled.set_endgame(Some(tables()));
    vec![
        ("default", config(seed), Search::new(16, Gates::default())),
        ("endgame tables", with_tables, tabled),
        (
            "phase 4",
            Config { max_nodes: 20_000_000, seed, ..Config::phase4() },
            Search::new(16, Gates::default()),
        ),
    ]
}

#[test]
fn golden_positions_get_the_right_verdict() {
    let golden: serde_json::Value = serde_json::from_str(&testdata("exhaustive.json")).unwrap();
    for case in golden["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let piles = parse_position(case["position"].as_str().unwrap()).unwrap();
        let game = Game::from_piles(piles);
        for (strategy, cfg, mut search) in strategies(7) {
            let report = solve(&Position::from_tops(&game.tops()), &cfg, &mut search, None);
            match (&report.verdict, case["winnable"].as_bool().unwrap()) {
                (Verdict::Win(line), true) => {
                    let mut g = game.clone();
                    for &mv in line {
                        g.apply(mv)
                            .unwrap_or_else(|e| panic!("{name} ({strategy}): solver move rejected: {e}"));
                    }
                    assert!(g.is_won(), "{name} ({strategy}): the line does not end in one pile");
                }
                (Verdict::Exhausted, false) => {}
                (v, w) => panic!("{name} ({strategy}): verdict {v:?}, but winnable = {w}"),
            }
        }
    }
}

#[test]
fn unwinnable_golden_deals_are_never_won() {
    for file in ["unwinnable_no_moves.json", "unwinnable_dead_end.json"] {
        let (_, record) = parse_records(&testdata(&format!("games/{file}"))).remove(0);
        let deal = Deal::parse(&record.unwrap().deal.cards).unwrap();
        for (strategy, cfg, mut search) in strategies(1) {
            let report = solve(&Position::from_deal(&deal), &cfg, &mut search, None);
            assert_eq!(report.verdict, Verdict::Exhausted, "{file} ({strategy})");
            assert!(report.best_piles > 1, "{file} ({strategy})");
        }
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

#[test]
fn the_thorough_search_wins_when_every_fast_stage_fails() {
    // A ladder whose searches get no budget can't win anything, so every deal must fall through to the thorough
    // stage and still be won (PLAN.md Phase 5: every strategy falls back to thorough searching).
    let starved = Ladder { budget: 0, tries: 8, ..Ladder::default() };
    let mut search = Search::new(16, Gates::default());
    for seed in 0..100 {
        search.clear_table();
        let deal = Deal::from_seed(seed);
        let cfg = Config { ladder: Some(starved), ..config(seed) };
        let report = solve(&Position::from_deal(&deal), &cfg, &mut search, None);
        let Verdict::Win(line) = report.verdict else { panic!("seed {seed}: {:?}", report.verdict) };
        assert_eq!(report.stage, Stage::Thorough, "seed {seed}");
        let record = SolveRecord::new(&deal, Some(seed), Outcome::Win, &line).unwrap();
        assert_eq!(verify(&record).map(|v| v.moves), Ok(51), "seed {seed}");
    }
}
