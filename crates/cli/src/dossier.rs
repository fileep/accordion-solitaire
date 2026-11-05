//! Hard-deal dossiers (PLAN.md §5). Every deal the solver hasn't won gets `<dir>/<id>.json` with everything needed
//! to reproduce and continue the work, and `<dir>/INDEX.md` lists them all. Running a deal again appends to its
//! history instead of starting over; a later win marks it solved (hard) and keeps the record of what was tried.

use crate::prove::Finding;
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

/// The dossier for `deal` in `dir`, read from disk or new.
fn load_or_new(dir: &Path, deal: &Deal, seed: Option<u64>) -> Result<(PathBuf, Value), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", id(deal, seed)));
    let d: Value = if path.exists() {
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
    Ok((path, d))
}

fn save(path: &Path, d: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(d).expect("serializable") + "\n";
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Records an exhaustive proof (`acc prove`, or `acc batch` for a deal it didn't win) in the deal's dossier. A proven
/// loss sets the status to `loss (proven)` and a win to `solved (hard)`; an undecided proof only adds to the history.
pub fn record_proof(
    dir: &Path,
    deal: &Deal,
    seed: Option<u64>,
    finding: &Finding,
    best: (usize, &[Move]),
    entry: Value,
) -> Result<PathBuf, String> {
    let (path, mut d) = load_or_new(dir, deal, seed)?;
    match finding {
        Finding::Lost => d["status"] = json!("loss (proven)"),
        Finding::Won(line) => {
            d["status"] = json!("solved (hard)");
            d["solution"] = json!(line_text(deal, line));
        }
        Finding::Undecided => {}
    }
    if (best.0 as u64) < d["best"]["piles_left"].as_u64().unwrap_or(52) {
        d["best"] = json!({ "piles_left": best.0, "moves": line_text(deal, best.1) });
    }
    let mut entry = entry;
    entry["date"] = json!(now_utc());
    entry["stage"] = json!("prove");
    d["history"].as_array_mut().ok_or("dossier history is not a list")?.push(entry);
    save(&path, &d)?;
    Ok(path)
}

/// Creates or updates the dossier for `deal` with one more attempt. Returns its path.
pub fn record(dir: &Path, deal: &Deal, seed: Option<u64>, attempt: &Attempt) -> Result<PathBuf, String> {
    let (path, mut d) = load_or_new(dir, deal, seed)?;
    let r = attempt.report;
    if d["status"] != "solved (hard)" && d["status"] != "loss (proven)" {
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
    save(&path, &d)?;
    Ok(path)
}

/// One deal of a run's hardest, for the gallery: its line and what it cost.
pub struct GalleryEntry<'a> {
    pub deal: Deal,
    pub seed: u64,
    pub report: &'a Report,
}

/// Replaces `<dir>/gallery/` with solve files for `entries` (hardest first), each loadable in the visualizer, and
/// rewrites the index. `run` says which run they came from.
pub fn write_gallery(dir: &Path, entries: &[GalleryEntry], run: &str) -> Result<(), String> {
    let gallery = dir.join("gallery");
    std::fs::create_dir_all(&gallery).map_err(|e| format!("creating {}: {e}", gallery.display()))?;
    if let Ok(old) = std::fs::read_dir(&gallery) {
        for f in old.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if name.starts_with("seed-") && name.ends_with(".json") {
                std::fs::remove_file(f.path())
                    .map_err(|e| format!("removing {}: {e}", f.path().display()))?;
            }
        }
    }
    for (rank, e) in entries.iter().enumerate() {
        let Verdict::Win(line) = &e.report.verdict else { continue };
        let mut record =
            accordion_core::SolveRecord::new(&e.deal, Some(e.seed), accordion_core::Outcome::Win, line)
                .map_err(|err| format!("seed {}: {err}", e.seed))?;
        record.stats = json!({
            "rank": rank + 1,
            "positions": e.report.nodes,
            "attempts": e.report.attempts,
            "stage": e.report.stage.to_string(),
            "run": run,
            "solver": format!("acc {}", env!("CARGO_PKG_VERSION")),
        })
        .as_object()
        .cloned();
        accordion_core::verify::verify(&record).map_err(|err| format!("seed {}: {err}", e.seed))?;
        let path = gallery.join(format!("seed-{}.json", e.seed));
        std::fs::write(&path, record.to_json() + "\n")
            .map_err(|err| format!("writing {}: {err}", path.display()))?;
    }
    write_index(dir)
}

/// Rewrites `<dir>/INDEX.md` from every dossier in `dir` and every solve file in `<dir>/gallery/`.
pub fn write_index(dir: &Path) -> Result<(), String> {
    let read =
        |path: &Path| std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let mut rows = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(d) = read(&path) else { continue };
            let history = d["history"].as_array().cloned().unwrap_or_default();
            let stages: Vec<&str> = history.iter().filter_map(|h| h["stage"].as_str()).collect();
            // Solver runs record `nodes`; proof runs record each exhaustive run's positions.
            let nodes: u64 = history
                .iter()
                .map(|h| {
                    h["nodes"].as_u64().unwrap_or(0)
                        + h["plain"]["positions"].as_u64().unwrap_or(0)
                        + h["independent"]["run"]["positions"].as_u64().unwrap_or(0)
                })
                .sum();
            let last = history.last().and_then(|h| h["date"].as_str()).unwrap_or("").get(..10).unwrap_or("");
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
    rows.sort();
    let mut gallery = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("gallery")) {
        for entry in entries.flatten() {
            if let Some(r) = read(&entry.path()) {
                gallery.push(r);
            }
        }
    }
    gallery.sort_by_key(|r| r["stats"]["rank"].as_u64().unwrap_or(u64::MAX));
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
    if !gallery.is_empty() {
        text.push_str(&format!(
            "\n## Gallery: the hardest deals won\n\nThe deals that took the most positions in {}. Each file is a verified \
             solve file: open it in the visualizer to replay the win.\n\n| Rank | Deal | Positions | Won in stage |\n|---|---|---|---|\n",
            gallery[0]["stats"]["run"].as_str().unwrap_or("the last gallery run")
        ));
        for r in &gallery {
            let seed = &r["deal"]["seed"];
            text.push_str(&format!(
                "| {} | [seed {seed}](gallery/seed-{seed}.json) | {} | {} |\n",
                r["stats"]["rank"],
                r["stats"]["positions"],
                r["stats"]["stage"].as_str().unwrap_or("?"),
            ));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    std::fs::write(dir.join("INDEX.md"), text).map_err(|e| format!("writing {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::fast::Position;
    use accordion_solver::{Config, Search, solve};

    #[test]
    fn proofs_and_the_gallery_reach_the_index() {
        let dir = std::env::temp_dir().join(format!("acc-dossier-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let deal = Deal::from_seed(3);
        let entry = json!({ "plain": { "positions": 10 }, "independent": { "run": { "positions": 5 } } });
        record_proof(&dir, &deal, Some(3), &Finding::Lost, (40, &[]), entry).unwrap();
        let d: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("seed-3.json")).unwrap()).unwrap();
        assert_eq!(d["status"], "loss (proven)");
        assert_eq!(d["best"]["piles_left"], 40);

        let mut search = Search::new(14, accordion_solver::Gates::default());
        let reports: Vec<_> = [5u64, 6]
            .iter()
            .map(|&s| {
                (
                    s,
                    solve(
                        &Position::from_deal(&Deal::from_seed(s)),
                        &Config { seed: s, ..Config::default() },
                        &mut search,
                        None,
                    ),
                )
            })
            .collect();
        let entries: Vec<GalleryEntry> = reports
            .iter()
            .map(|(s, r)| GalleryEntry { deal: Deal::from_seed(*s), seed: *s, report: r })
            .collect();
        write_gallery(&dir, &entries, "a test run").unwrap();
        let index = std::fs::read_to_string(dir.join("INDEX.md")).unwrap();
        assert!(index.contains("| [seed-3](seed-3.json) | loss (proven) | prove | 40 | 15 |"), "{index}");
        assert!(index.contains("[seed 5](gallery/seed-5.json)") && index.contains("a test run"), "{index}");
        let record = std::fs::read_to_string(dir.join("gallery/seed-6.json")).unwrap();
        let (_, parsed) = accordion_core::solve_file::parse_records(&record).remove(0);
        assert!(accordion_core::verify::verify(&parsed.unwrap()).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
