//! Hard-deal dossiers (PLAN.md §5). Every deal the solver hasn't won gets `<dir>/<id>.json` with everything needed
//! to reproduce and continue the work, and `<dir>/INDEX.md` lists them all. Running a deal again appends to its
//! history instead of starting over; a later win marks it solved (hard) and keeps the record of what was tried.

use accordion_core::fast::Position;
use accordion_core::notation::CardMove;
use accordion_core::solve_file::Rules;
use accordion_core::{Deal, Game, Move};
use accordion_solver::{Config, Report, Verdict};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One solver run on a deal, as recorded in its history.
pub struct Attempt<'a> {
    /// `batch`, `solve`, `escalate-1`, …
    pub stage: &'a str,
    /// Per-thread settings (recorded in full); thread `i` used move-order seed `config.seed + i`.
    pub config: &'a Config,
    pub threads: usize,
    pub report: &'a Report,
    pub seconds: f64,
}

/// `seed-<n>` for seeded deals, `deck-<16 hex>` (FNV-1a of the canonical cards) for entered decks.
pub fn id(deal: &Deal, seed: Option<u64>) -> String {
    match seed {
        Some(s) => format!("seed-{s}"),
        None => {
            let hash = deal
                .to_string()
                .bytes()
                .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
            format!("deck-{hash:016x}")
        }
    }
}

/// A line from the deal's start in card notation.
pub fn line_text(deal: &Deal, line: &[Move]) -> Vec<String> {
    let mut game = Game::new(deal);
    line.iter()
        .map(|&mv| {
            let (mover, target) = game.cards_of(mv);
            game.apply(mv).expect("solver lines are legal");
            CardMove { mover, target }.to_string()
        })
        .collect()
}

/// UTC timestamp, `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // Days to civil date (H. Hinnant's algorithm).
    let z = secs.div_euclid(86_400) + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let t = secs.rem_euclid(86_400);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", t / 3600, t % 3600 / 60, t % 60)
}

fn status(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Win(_) => "solved (hard)",
        Verdict::Exhausted => "exhausted",
        Verdict::Unknown => "unknown",
    }
}

/// Creates or updates the dossier for `deal` with one more attempt. Returns its path.
pub fn record(dir: &Path, deal: &Deal, seed: Option<u64>, attempt: &Attempt) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", id(deal, seed)));
    let mut d: Value = if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        let start = Position::from_deal(deal);
        let mut deal_json = json!({ "cards": deal.to_string() });
        if let Some(s) = seed {
            deal_json["seed"] = json!(s);
        }
        json!({
            "format": "accordion-dossier/1",
            "id": id(deal, seed),
            "deal": deal_json,
            "rules": Rules::default(),
            "status": "unknown",
            "features": {
                "initial_legal_moves": start.move_count(),
                "piles_with_a_move": (start.slides() | start.leaps()).count_ones(),
            },
            "best": { "piles_left": 52, "moves": [] },
            "history": [],
        })
    };
    let r = attempt.report;
    if d["status"] != "solved (hard)" {
        d["status"] = json!(status(&r.verdict));
    }
    if (r.best_piles as u64) < d["best"]["piles_left"].as_u64().unwrap_or(52) {
        d["best"] = json!({ "piles_left": r.best_piles, "moves": line_text(deal, &r.best_line) });
    }
    if let Verdict::Win(line) = &r.verdict {
        d["solution"] = json!(line_text(deal, line));
    }
    let c = attempt.config;
    let entry = json!({
        "date": now_utc(),
        "stage": attempt.stage,
        "result": match r.verdict { Verdict::Win(_) => "win", Verdict::Exhausted => "exhausted", Verdict::Unknown => "unknown" },
        "solver": format!("acc {}", env!("CARGO_PKG_VERSION")),
        "threads": attempt.threads,
        "max_nodes_per_thread": c.max_nodes,
        "move_order_seed": c.seed,
        "config": c,
        "nodes": r.nodes,
        "attempts": r.attempts,
        "table_hits": r.stats.table_hits,
        "dead_end_cuts": r.stats.dead_end_cuts,
        "seconds": (attempt.seconds * 1000.0).round() / 1000.0,
    });
    d["history"].as_array_mut().ok_or("dossier history is not a list")?.push(entry);
    let text = serde_json::to_string_pretty(&d).expect("serializable") + "\n";
    std::fs::write(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// Rewrites `<dir>/INDEX.md` from every dossier in `dir`.
pub fn write_index(dir: &Path) -> Result<(), String> {
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let Ok(d) = std::fs::read_to_string(&path).map(|t| serde_json::from_str::<Value>(&t)) else {
                    continue;
                };
                let Ok(d) = d else { continue };
                let history = d["history"].as_array().cloned().unwrap_or_default();
                let stages: Vec<&str> = history.iter().filter_map(|h| h["stage"].as_str()).collect();
                let nodes: u64 = history.iter().filter_map(|h| h["nodes"].as_u64()).sum();
                let last =
                    history.last().and_then(|h| h["date"].as_str()).unwrap_or("").get(..10).unwrap_or("");
                rows.push(format!(
                    "| [{id}]({id}.json) | {} | {} | {} | {} | {last} |",
                    d["status"].as_str().unwrap_or("?"),
                    stages.join(", "),
                    d["best"]["piles_left"],
                    nodes,
                    id = d["id"].as_str().unwrap_or("?"),
                ));
            }
        }
    }
    rows.sort();
    let mut text = String::from(
        "# Hard deals\n\nEvery deal the solver has not won within its cap gets a dossier here (`<id>.json`) and a row below.\n\
         See PLAN.md §5 for the dossier format and the escalation stages. `acc` rewrites this file after every run.\n\n\
         | Deal | Status | Stages run | Best (piles left) | Positions searched | Last attempt |\n|---|---|---|---|---|---|\n",
    );
    if rows.is_empty() {
        text.push_str("\nNone yet: every deal attempted so far has been won.\n");
    } else {
        text.push_str(&(rows.join("\n") + "\n"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    std::fs::write(dir.join("INDEX.md"), text).map_err(|e| format!("writing {}: {e}", dir.display()))
}
