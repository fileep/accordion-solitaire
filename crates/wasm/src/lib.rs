//! WebAssembly bindings for the visualizer. They expose the same `accordion-core` code the solver uses,
//! so the browser never carries its own copy of the rules: the page keeps only the deal and a list of moves
//! (compact bytes, `pile << 1 | leap`), and asks the engine for everything else. Structured results cross the
//! boundary as JSON strings; seeds are `BigInt` on the JavaScript side so all 64 bits survive.

use accordion_core::deck_input::parse_position;
use accordion_core::fast::Position;
use accordion_core::game::Game;
use accordion_core::notation::{CardMove, decode_compact};
use accordion_core::solve_file::{Outcome, SolveRecord, parse_records};
use accordion_core::verify::verify;
use accordion_core::{Deal, Move};
use accordion_solver::{Config, Gates, Search, Verdict};
use serde_json::{Map, Value, json};
use wasm_bindgen::prelude::*;

/// Solves from the position after `moves` on the deal `cards`, with the same solver code as `acc solve` (one
/// thread; run it in a Web Worker). `max_nodes` caps the search; `seed` picks the move-order randomness.
/// Returns `{"result": "win", "moves": [bytes from that position], "nodes", "attempts"}`, or
/// `{"result": "exhausted" | "unknown", "nodes", "attempts", "bestPiles"}`, or `{"error"}`.
/// The page verifies a win with `exportRecord` before showing it.
#[wasm_bindgen]
pub fn solve(cards: &str, moves: &[u8], max_nodes: f64, seed: f64) -> String {
    let deal = match Deal::parse(cards) {
        Ok(d) => d,
        Err(e) => return json!({ "error": format!("deal: {e}") }).to_string(),
    };
    let mut game = Game::new(&deal);
    for (i, &byte) in moves.iter().enumerate() {
        if let Err(e) = game.apply(Move::from_byte(byte)) {
            return json!({ "error": format!("move {}: {e}", i + 1) }).to_string();
        }
    }
    let config = Config { max_nodes: max_nodes.max(1.0) as u64, seed: seed as u64, ..Config::default() };
    let mut search = Search::new(16, Gates::default());
    let report = accordion_solver::solve(&Position::from_tops(&game.tops()), &config, &mut search, None);
    let common =
        json!({ "nodes": report.nodes, "attempts": report.attempts, "bestPiles": report.best_piles });
    let mut out = common.as_object().cloned().unwrap_or_default();
    match report.verdict {
        Verdict::Win(line) => {
            out.insert("result".into(), json!("win"));
            out.insert("moves".into(), json!(line.iter().map(|m| m.to_byte()).collect::<Vec<u8>>()));
        }
        Verdict::Exhausted => {
            out.insert("result".into(), json!("exhausted"));
        }
        Verdict::Unknown => {
            out.insert("result".into(), json!("unknown"));
        }
    }
    Value::Object(out).to_string()
}

/// Engine version, shown in the visualizer so it's clear which rules build is running.
#[wasm_bindgen(js_name = engineVersion)]
pub fn engine_version() -> String {
    accordion_core::VERSION.to_string()
}

/// The deal for `seed`, as canonical card text.
#[wasm_bindgen(js_name = dealFromSeed)]
pub fn deal_from_seed(seed: u64) -> String {
    Deal::from_seed(seed).to_string()
}

/// Checks typed or pasted deck text: `{"ok": "<canonical cards>"}` or `{"issues": [...], "message": "..."}`.
#[wasm_bindgen(js_name = parseDeck)]
pub fn parse_deck(text: &str) -> String {
    match Deal::parse(text) {
        Ok(deal) => json!({ "ok": deal.to_string() }),
        Err(e) => json!({ "issues": e.issues, "message": e.to_string() }),
    }
    .to_string()
}

/// Checks a game-in-progress position (piles separated by `|`, bottom to top):
/// `{"ok": [["AC", ...], ...]}` or `{"issues": [...], "message": "..."}`.
#[wasm_bindgen(js_name = parsePosition)]
pub fn parse_position_text(text: &str) -> String {
    match parse_position(text) {
        Ok(piles) => json!({ "ok": piles }),
        Err(e) => json!({ "issues": e.issues, "message": e.to_string() }),
    }
    .to_string()
}

/// Verifies a solve file (`.json` or `.jsonl` text). One entry per record:
/// `{"line", "ok": true, "result", "moves", "piles_left"}` or `{"line", "ok": false, "error", "move"}`.
#[wasm_bindgen(js_name = verifySolveFile)]
pub fn verify_solve_file(text: &str) -> String {
    let results: Vec<Value> = parse_records(text)
        .into_iter()
        .map(|(line, parsed)| {
            let checked = parsed.map_err(|e| (None, e)).and_then(|r| verify(&r).map_err(|e| (e.move_number, e.to_string())));
            match checked {
                Ok(v) => json!({ "line": line, "ok": true, "result": v.result, "moves": v.moves, "piles_left": v.piles_left }),
                Err((move_number, error)) => json!({ "line": line, "ok": false, "error": error, "move": move_number }),
            }
        })
        .collect();
    Value::Array(results).to_string()
}

fn state_json(game: &Game) -> Value {
    let legal: Vec<Value> = game
        .legal_moves()
        .into_iter()
        .map(|mv| {
            let (mover, target) = game.cards_of(mv);
            json!({ "pile": mv.pile, "leap": mv.leap, "byte": mv.to_byte(), "mover": mover, "target": target })
        })
        .collect();
    json!({ "piles": game.piles(), "legal": legal, "won": game.is_won() })
}

/// The position after playing `moves` from the deal `cards`:
/// `{"ok": {"piles": [[bottom..top], ...], "legal": [{"pile", "leap", "byte", "mover", "target"}], "won"}}`, or
/// `{"error", "move"}` naming the first illegal move (1-based).
#[wasm_bindgen]
pub fn replay(cards: &str, moves: &[u8]) -> String {
    let deal = match Deal::parse(cards) {
        Ok(d) => d,
        Err(e) => return json!({ "error": format!("deal: {e}"), "move": Value::Null }).to_string(),
    };
    let mut game = Game::new(&deal);
    for (i, &byte) in moves.iter().enumerate() {
        if let Err(e) = game.apply(Move::from_byte(byte)) {
            return json!({ "error": e.to_string(), "move": i + 1 }).to_string();
        }
    }
    json!({ "ok": state_json(&game) }).to_string()
}

/// Opens a solve file for replay. One entry per record: its deal, claimed result and verification outcome, and
/// its moves as compact bytes up to (not including) the first illegal one, so the page never animates a bad move:
/// `[{"line", "seed"?, "cards"?, "result"?, "moves": [byte], "texts": [..], "verified", "error"?, "errorMove"?}]`.
/// Seeds are decimal strings.
#[wasm_bindgen(js_name = openSolveFile)]
pub fn open_solve_file(text: &str) -> String {
    let entries: Vec<Value> = parse_records(text)
        .into_iter()
        .map(|(line, parsed)| {
            let mut e = Map::new();
            e.insert("line".into(), json!(line));
            let record = match parsed {
                Ok(r) => r,
                Err(err) => {
                    e.insert("verified".into(), json!(false));
                    e.insert("error".into(), json!(format!("not a valid solve record: {err}")));
                    e.insert("moves".into(), json!([]));
                    return Value::Object(e);
                }
            };
            if let Some(seed) = record.deal.seed {
                e.insert("seed".into(), json!(seed.to_string()));
            }
            e.insert("cards".into(), json!(record.deal.cards));
            e.insert("result".into(), json!(record.result));
            match verify(&record) {
                Ok(_) => {
                    e.insert("verified".into(), json!(true));
                }
                Err(err) => {
                    e.insert("verified".into(), json!(false));
                    e.insert("error".into(), json!(err.to_string()));
                    e.insert("errorMove".into(), json!(err.move_number));
                }
            }
            // The playable prefix: every move up to the first one the rules reject.
            let (mut bytes, mut texts) = (Vec::new(), Vec::new());
            if let Ok(deal) = Deal::parse(&record.deal.cards) {
                let mut game = Game::new(&deal);
                let compact = record.moves_b64.as_deref().map(decode_compact).and_then(Result::ok);
                let resolved: Vec<Option<Move>> = if record.moves.is_empty() {
                    compact.unwrap_or_default().into_iter().map(Some).collect()
                } else {
                    record.moves.iter().map(|_| None).collect()
                };
                for (i, slot) in resolved.into_iter().enumerate() {
                    let mv = match slot {
                        Some(mv) => game.check(mv).ok().map(|()| mv),
                        None => CardMove::parse(&record.moves[i])
                            .ok()
                            .and_then(|cm| game.move_between(cm.mover, cm.target).ok()),
                    };
                    let Some(mv) = mv else { break };
                    let (mover, target) = game.cards_of(mv);
                    texts.push(CardMove { mover, target }.to_string());
                    bytes.push(mv.to_byte());
                    game.apply(mv).expect("checked above");
                }
            }
            e.insert("moves".into(), json!(bytes));
            e.insert("texts".into(), json!(texts));
            Value::Object(e)
        })
        .collect();
    Value::Array(entries).to_string()
}

/// A solve-file record (pretty JSON) for a game played in the visualizer, verified before it is handed out:
/// `{"ok": "<json>"}` or `{"error"}`. The result is `win` when the moves end in one pile, else `unknown`.
#[wasm_bindgen(js_name = exportRecord)]
pub fn export_record(cards: &str, seed: Option<u64>, moves: &[u8]) -> String {
    let built = Deal::parse(cards).map_err(|e| format!("deal: {e}")).and_then(|deal| {
        let moves: Vec<Move> = moves.iter().map(|&b| Move::from_byte(b)).collect();
        let mut game = Game::new(&deal);
        for &mv in &moves {
            game.apply(mv).map_err(|e| e.to_string())?;
        }
        let result = if game.is_won() { Outcome::Win } else { Outcome::Unknown };
        let mut record = SolveRecord::new(&deal, seed, result, &moves).map_err(|e| e.to_string())?;
        let mut stats = Map::new();
        stats.insert("source".into(), json!(format!("visualizer, engine {}", accordion_core::VERSION)));
        record.stats = Some(stats);
        verify(&record).map_err(|e| e.to_string())?;
        Ok(record.to_json())
    });
    match built {
        Ok(text) => json!({ "ok": text }),
        Err(error) => json!({ "error": error }),
    }
    .to_string()
}
