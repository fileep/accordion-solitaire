//! The CUDA rules code against the Rust engines, on the golden files and random deals (PLAN.md §6.3, Phases 5 and
//! 8). Random playouts record the GPU's legal moves, dead-end test and undo at every step, for comparison with the
//! reference engine (moves) and the fast engine (tests); ladder attempts must give the right verdict on the golden
//! positions and deals, and every line they return must replay. Skipped, with a note, on a machine without a
//! usable NVIDIA GPU.

use accordion_core::deck_input::parse_position;
use accordion_core::fast::{Position, connected};
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use accordion_core::{Card, Deal, Game, Move, Outcome, SolveRecord};
use accordion_gpu::{Attempts, Gpu, Settings, deal_codes, position_codes};
use std::path::PathBuf;
use std::sync::OnceLock;

fn testdata(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata").join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

/// The GPU, compiled once for every test; `None` (and a note) without one.
fn gpu() -> Option<&'static Gpu> {
    static GPU: OnceLock<Option<Gpu>> = OnceLock::new();
    GPU.get_or_init(|| {
        Gpu::new(Settings::default()).inspect_err(|e| eprintln!("skipping the GPU tests: {e}")).ok()
    })
    .as_ref()
}

/// Every golden deal: the solve files, batch.jsonl and the seeded-deal list, with their recorded result.
fn golden_deals() -> Vec<(String, Deal, Option<Outcome>)> {
    let mut deals = Vec::new();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/games");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        for (line, record) in parse_records(&text) {
            // Some golden files are deliberately broken; only their deals matter here.
            let Ok(record) = record else { continue };
            let Ok(deal) = Deal::parse(&record.deal.cards) else { continue };
            // The `bad_*` files pair a deal with a wrong line, so their recorded result says nothing.
            let result = (!name.starts_with("bad_")).then_some(record.result);
            deals.push((format!("{name}:{line}"), deal, result));
        }
    }
    let seeded: serde_json::Value = serde_json::from_str(&testdata("deals_by_seed.json")).unwrap();
    for d in seeded["deals"].as_array().unwrap() {
        let seed = d["seed"].as_u64().unwrap();
        deals.push((format!("seed {seed}"), Deal::from_seed(seed), None));
    }
    deals
}

/// The golden positions (exhaustive.json): name, top cards, winnable.
fn golden_positions() -> Vec<(String, Vec<Card>, bool)> {
    let golden: serde_json::Value = serde_json::from_str(&testdata("exhaustive.json")).unwrap();
    golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let piles = parse_position(case["position"].as_str().unwrap()).unwrap();
            let tops = Game::from_piles(piles).tops();
            (case["name"].as_str().unwrap().to_string(), tops, case["winnable"].as_bool().unwrap())
        })
        .collect()
}

fn codes(tops: &[Card]) -> [u8; 52] {
    position_codes(&tops.iter().map(|c| c.code()).collect::<Vec<_>>())
}

/// Replays a GPU playout on both engines, step by step.
fn check_playout(name: &str, tops: &[Card], p: &accordion_gpu::Playout) {
    let mut game = Game::from_piles(tops.iter().map(|&c| vec![c]).collect());
    let mut pos = Position::from_tops(tops);
    let steps = p.moves.len();
    assert_eq!(p.moves.last(), Some(&0xFF), "{name}: the playout did not end");
    for k in 0..steps {
        let (mut slides, mut leaps) = (0u64, 0u64);
        for mv in game.legal_moves() {
            *if mv.leap { &mut leaps } else { &mut slides } |= 1 << mv.pile;
        }
        assert_eq!(
            p.masks[k],
            (slides, leaps),
            "{name}, step {k}: legal moves differ from the reference engine"
        );
        assert_eq!(
            p.flags[k] & 1 != 0,
            connected(pos.present()),
            "{name}, step {k}: connectivity test differs"
        );
        assert_eq!(p.flags[k] & 2 != 0, pos.link_broken(), "{name}, step {k}: link-broken trigger differs");
        assert_eq!(
            p.flags[k] & 8 != 0,
            pos.connected_without_leftmost(),
            "{name}, step {k}: connectivity without the leftmost card differs"
        );
        assert_eq!(p.flags[k] & 4, 0, "{name}, step {k}: undo did not restore the position");
        if p.moves[k] == 0xFF {
            assert_eq!(k, steps - 1);
            assert_eq!(
                slides | leaps,
                0,
                "{name}, step {k}: the GPU found no move, the reference engine did"
            );
        } else {
            let mv = Move::from_byte(p.moves[k]);
            game.apply(mv).unwrap_or_else(|e| panic!("{name}, step {k}: GPU move rejected: {e}"));
            pos.apply(mv.pile as usize, mv.leap);
        }
    }
}

#[test]
fn playouts_match_the_rust_engines() {
    let Some(gpu) = gpu() else { return };
    let mut starts: Vec<(String, Vec<Card>)> =
        golden_deals().into_iter().map(|(name, deal, _)| (name, deal.cards().to_vec())).collect();
    starts.extend(golden_positions().into_iter().map(|(name, tops, _)| (name, tops)));
    // The golden starts many times over (different moves each time), then random deals.
    let mut all: Vec<(String, Vec<Card>)> = Vec::new();
    for rep in 0..64 {
        all.extend(starts.iter().map(|(name, tops)| (format!("{name} #{rep}"), tops.clone())));
    }
    all.extend(
        (0..20_000u64)
            .map(|s| (format!("seed {}", 5_000_000 + s), Deal::from_seed(5_000_000 + s).cards().to_vec())),
    );
    let positions: Vec<[u8; 52]> = all.iter().map(|(_, tops)| codes(tops)).collect();
    let playouts = gpu.playouts(&positions, 0xC0FFEE).unwrap();
    let mut steps = 0;
    for ((name, tops), p) in all.iter().zip(&playouts) {
        check_playout(name, tops, p);
        steps += p.moves.len();
    }
    assert!(steps > 20_000 * 30, "only {steps} steps");
}

#[test]
fn golden_positions_get_the_right_verdict() {
    let Some(gpu) = gpu() else { return };
    for (name, tops, winnable) in golden_positions()
        .into_iter()
        .flat_map(|g| [(g.clone(), false), (g, true)])
        .map(|((name, tops, winnable), leftmost)| {
            (format!("{name} (leftmost test {leftmost})"), tops, (winnable, leftmost))
        })
    {
        let (winnable, leftmost) = winnable;
        // Small positions: no prefix and no budget, so every try is a complete search. Large ones: long random
        // prefixes, which never find a win on an unwinnable position.
        let a = if tops.len() <= accordion_gpu::RESIDUAL {
            Attempts {
                tries: 8,
                prefix: 0,
                tries2: 0,
                prefix2: 0,
                budget: u32::MAX,
                conn_gate: 19,
                leftmost,
                seed: 1,
            }
        } else {
            assert!(!winnable, "{name}: no large winnable golden position to test");
            Attempts {
                tries: 4096,
                prefix: 48,
                tries2: 4096,
                prefix2: 40,
                budget: 100_000,
                conn_gate: 19,
                leftmost,
                seed: 1,
            }
        };
        let result = gpu.run(&[codes(&tops)], &a).unwrap();
        match (&result.lines[0], winnable) {
            (Some(line), true) => {
                let mut game = Game::from_piles(tops.iter().map(|&c| vec![c]).collect());
                for &b in line {
                    game.apply(Move::from_byte(b))
                        .unwrap_or_else(|e| panic!("{name}: GPU move rejected: {e}"));
                }
                assert!(game.is_won(), "{name}: the GPU line does not end in one pile");
            }
            (None, false) => assert!(result.tries[0] > 0, "{name}: no attempt ran"),
            (line, w) => panic!("{name}: GPU line {line:?}, but winnable = {w}"),
        }
    }
}

#[test]
fn golden_and_random_deals_are_won_with_lines_that_verify() {
    let Some(gpu) = gpu() else { return };
    let mut deals: Vec<(String, Deal, Option<Outcome>)> = golden_deals();
    deals.extend((0..4096u64).map(|s| (format!("seed {s}"), Deal::from_seed(s), None)));
    let positions: Vec<[u8; 52]> = deals.iter().map(|(_, deal, _)| deal_codes(deal)).collect();
    for leftmost in [false, true] {
        let result = gpu.run(&positions, &Attempts { leftmost, ..gpu.settings.attempts(0) }).unwrap();
        let won = check_deals(&deals, &result.lines);
        assert!(
            won as f64 > 0.98 * deals.len() as f64,
            "leftmost test {leftmost}: won {won} of {}",
            deals.len()
        );
    }
}

/// Every recorded win is won, no recorded loss is, and every line verifies; returns the wins.
fn check_deals(deals: &[(String, Deal, Option<Outcome>)], lines: &[Option<Vec<u8>>]) -> usize {
    let mut won = 0;
    for ((name, deal, recorded), line) in deals.iter().zip(lines) {
        match line {
            Some(line) => {
                assert_ne!(*recorded, Some(Outcome::Loss), "{name}: the GPU won an unwinnable deal");
                let moves: Vec<Move> = line.iter().map(|&b| Move::from_byte(b)).collect();
                let record = SolveRecord::new(deal, None, Outcome::Win, &moves)
                    .unwrap_or_else(|e| panic!("{name}: GPU line is not legal: {e}"));
                verify(&record).unwrap_or_else(|e| panic!("{name}: GPU line failed verification: {e}"));
                won += 1;
            }
            None => assert_ne!(*recorded, Some(Outcome::Win), "{name}: a winnable golden deal was not won"),
        }
    }
    won
}
