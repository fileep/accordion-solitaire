//! WebAssembly bindings for the visualizer. They expose the same `accordion-core` code the solver uses,
//! so the browser never carries its own copy of the rules. Structured results cross the boundary as JSON
//! strings; seeds are `BigInt` on the JavaScript side so all 64 bits survive.

use accordion_core::Deal;
use accordion_core::deck_input::parse_position;
use accordion_core::solve_file::parse_records;
use accordion_core::verify::verify;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

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
        .map(|(line, parsed)| match parsed.map_err(|e| (None, e)).and_then(|r| verify(&r).map_err(|e| (e.move_number, e.to_string()))) {
            Ok(v) => json!({ "line": line, "ok": true, "result": v.result, "moves": v.moves, "piles_left": v.piles_left }),
            Err((move_number, error)) => json!({ "line": line, "ok": false, "error": error, "move": move_number }),
        })
        .collect();
    Value::Array(results).to_string()
}
