//! The front end: `acc run`, `resume`, `status`, `pause`, `results`, `doctor`, `show`, `settings`, `verify` and the help.
//! Each test in a folder of its own, on the CPU with 2 threads, and with a battery the test sets (`ACC_FAKE_BATTERY`), so
//! that the computer's own can't pause a run.

use serde_json::Value;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const ACC: &str = env!("CARGO_BIN_EXE_acc");

/// The commands `acc help` lists.
const COMMANDS: [&str; 15] = [
    "doctor", "run", "status", "pause", "resume", "results", "deal", "show", "solve", "prove", "verify",
    "batch", "settings", "endgame", "selftest",
];

/// A directory for one test, empty but for the battery, removed when the test passes.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("acc-front-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = Dir(dir);
        dir.battery("none");
        dir
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// What the fake battery says: `<percent> <1 or 0 for the charger>`, `none` or `unreadable`.
    fn battery(&self, text: &str) {
        std::fs::write(self.file("battery.txt"), text).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(ACC);
        command.current_dir(&self.0).env("ACC_FAKE_BATTERY", self.file("battery.txt")).args(args);
        command
    }

    fn acc(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Starts acc with its output going to files, `<name>.out` and `<name>.err`, which [`finished`] reads.
    fn spawn(&self, name: &str, args: &[&str]) -> Child {
        let out = File::create(self.file(&format!("{name}.out"))).unwrap();
        let err = File::create(self.file(&format!("{name}.err"))).unwrap();
        self.command(args).stdin(Stdio::null()).stdout(out).stderr(err).spawn().unwrap()
    }

    /// Waits for a child started by [`spawn`], and what it said.
    fn finished(&self, name: &str, mut child: Child) -> (bool, String, String) {
        let status = child.wait().unwrap();
        let read = |ending: &str| std::fs::read_to_string(self.file(&format!("{name}.{ending}"))).unwrap();
        (status.success(), read("out"), read("err"))
    }

    /// The names of what is in the folder, but the battery.
    fn left(&self) -> Vec<String> {
        let mut left: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "battery.txt")
            .collect();
        left.sort();
        left
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{:?}\n{}\n{}",
        output.status,
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

fn refused(output: &Output, says: &str) {
    assert!(!output.status.success(), "it should have been refused: {}", text(&output.stdout));
    assert!(text(&output.stderr).contains(says), "wanted {says:?} in: {}", text(&output.stderr));
}

fn contains_all(text: &str, words: &[&str]) {
    for w in words {
        assert!(text.contains(w), "wanted {w:?} in:\n{text}");
    }
}

/// Whether a process holds the lock at `path`.
fn held(path: &Path) -> bool {
    OpenOptions::new()
        .read(true)
        .open(path)
        .is_ok_and(|f| matches!(f.try_lock(), Err(TryLockError::WouldBlock)))
}

/// The state a status file gives, or nothing before there is one.
fn state(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["state"].as_str().map(String::from))
        .unwrap_or_default()
}

/// Waits until `done`, while `child` runs.
fn wait_until(child: &mut Child, what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("it ended with {status} before {what}");
        }
        assert!(start.elapsed() < Duration::from_secs(120), "no {what} in two minutes");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A command line with the memory flags left out, which depend on the computer's memory.
fn without_memory(line: &str) -> String {
    let mut words = Vec::new();
    let mut skip = false;
    for word in line.split_whitespace() {
        if std::mem::take(&mut skip) {
            continue;
        }
        if word == "--games-memory" || word == "--prove-memory" {
            skip = true;
            continue;
        }
        words.push(word);
    }
    words.join(" ")
}

#[test]
fn the_help_and_every_command_with_no_run_here() {
    let dir = Dir::new("empty");
    let start = succeeded(&dir.acc(&[]));
    contains_all(&start, &["Accordion is a solitaire game", "acc doctor", "acc run --quick", "Start here:"]);
    for help in [&["--help"][..], &["help"], &["-h"]] {
        let shown = succeeded(&dir.acc(help));
        for command in COMMANDS {
            assert!(shown.contains(&format!("\n  {command} ")), "{command} in acc {help:?}:\n{shown}");
        }
        contains_all(
            &shown,
            &["acc bench and acc experiment", "docs/GETTING_STARTED.md", "Usage: acc [COMMAND]"],
        );
    }
    for command in COMMANDS.iter().chain(&["bench", "experiment"]) {
        assert!(succeeded(&dir.acc(&[command, "--help"])).contains("Examples:"), "{command}");
    }

    let status = succeeded(&dir.acc(&["status"]));
    contains_all(&status, &["no runs found in", "checkpoints", "from that same folder"]);
    for command in ["pause", "resume", "results"] {
        refused(&dir.acc(&[command]), "no runs found");
    }

    let doctor = succeeded(&dir.acc(&["doctor"]));
    contains_all(
        &doctor,
        &["PASS  build", "PASS  folder", "PASS  battery   none", "PASS  solving   100 of 100 deals won"],
    );
    assert!(!doctor.contains("FAIL"), "{doctor}");

    let shown = succeeded(&dir.acc(&["show", "--seed", "42"]));
    contains_all(&shown, &["Deal 42. Rules:", "   1-13  2S 2C KS", "acc show --seed 42 --solve"]);
    let solved = succeeded(&dir.acc(&["show", "--seed", "1", "--solve", "--threads", "2"]));
    contains_all(&solved, &["A winning line, 51 moves", "\n   1. ", ", 51 piles left\n", ", 1 pile left\n"]);

    let config: Value = serde_json::from_str(&succeeded(&dir.acc(&["settings"]))).unwrap();
    assert!(config["ladder"].is_object() && config["max_nodes"].is_u64(), "{config}");
    let gpu: Value = serde_json::from_str(&succeeded(&dir.acc(&["settings", "--gpu"]))).unwrap();
    assert!(gpu["rungs"].is_array(), "{gpu}");
    assert!(succeeded(&dir.acc(&["settings", "--out", "s.json"])).contains("acc run --settings s.json"));
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.file("s.json")).unwrap()).unwrap();
    assert_eq!(saved, config);
    refused(&dir.acc(&["settings", "--out", "s.json"]), "already there");
    let line = succeeded(&dir.acc(&["run", "--settings", "s.json", "--threads", "2", "--dry-run"]));
    assert!(line.contains("--config s.json"), "{line}");
    // The settings as printed are taken back whole.
    let batch = ["batch", "--count", "20", "--threads", "2", "--gpu", "off", "--no-checkpoint", "--no-log"];
    succeeded(&dir.acc(&[&batch[..], &["--no-dossiers", "--endgame", "off", "--config", "s.json"]].concat()));
    assert_eq!(dir.left(), ["s.json"], "nothing else is written");
}

#[test]
fn a_dry_run_says_the_batch_command_and_touches_nothing() {
    let dir = Dir::new("dry");
    let line = |args: &[&str]| {
        let output = dir.acc(&[&["run", "--dry-run"][..], args].concat());
        without_memory(succeeded(&output).trim())
    };
    assert_eq!(line(&["--threads", "2"]), "acc batch --from 0 --count 1000000 --threads 2");
    assert_eq!(line(&["--quick"]), "acc batch --from 0 --count 10000");
    assert_eq!(line(&["--hours", "2"]), "acc batch --from 0 --count 100000000000000 --stop-after 2");
    assert_eq!(line(&["--overnight", "--deals", "5k"]), "acc batch --from 0 --count 5000 --stop-after 8");
    assert_eq!(
        line(&[
            "--from",
            "1.5b",
            "--deals",
            "2m",
            "--gpu",
            "off",
            "--no-proofs",
            "--threads",
            "3",
            "--save-wins",
            "w.jsonl"
        ]),
        "acc batch --from 1500000000 --count 2000000 --threads 3 --gpu off --out w.jsonl --prove 0"
    );
    let output = dir.acc(&["run", "--dry-run", "--deals", "2m", "--save-wins", "w.jsonl"]);
    assert!(text(&output.stderr).contains("about 700 MB in w.jsonl"), "{}", text(&output.stderr));
    let half = (std::thread::available_parallelism().unwrap().get() / 2).max(1);
    assert_eq!(line(&["--gentle"]), format!("acc batch --from 0 --count 1000000 --threads {half} --gpu off"));
    assert_eq!(
        line(&["--gentle", "--gpu", "auto"]),
        format!("acc batch --from 0 --count 1000000 --threads {half} --gpu auto")
    );

    refused(
        &dir.acc(&["run", "--deals", "1,000", "--dry-run"]),
        "expected a whole number such as 5000, 2m, 1.5b or 100t",
    );
    refused(&dir.acc(&["run", "--hours", "1", "--save-wins", "w.jsonl", "--dry-run"]), "--save-wins");
    refused(&dir.acc(&["run", "--battery-pause", "70", "--dry-run"]), "--battery-resume");
    refused(&dir.acc(&["run", "--deals", "0", "--dry-run"]), "at least 1");
    refused(
        &dir.acc(&["run", "--from", "18446744073709551615", "--deals", "2", "--dry-run"]),
        "past the last deal",
    );
    refused(&dir.acc(&["run", "--hours", "0", "--dry-run"]), "--hours");
    refused(&dir.acc(&["run", "--quick", "--deals", "5", "--dry-run"]), "--deals");
    assert!(dir.left().is_empty(), "{:?}", dir.left());
}

#[test]
fn a_quick_run_goes_from_start_to_finish() {
    let dir = Dir::new("quick");
    let output = dir.acc(&["run", "--quick", "--once", "--gpu", "off", "--threads", "2"]);
    let stdout = succeeded(&output);
    contains_all(
        &stdout,
        &[
            "deals            10000 (seeds 0..9999)",
            "In short: ",
            " of 10,000 deals won",
            "Next: acc run --from 10k --deals 10k",
        ],
    );
    let said = text(&output.stderr);
    assert!(said.contains("same as: acc batch --from 0 --count 10000 --threads 2 --gpu off"), "{said}");
    // A run this short leaves nothing to carry on, so nothing to report on either.
    assert!(!dir.file("checkpoints").exists());
    refused(&dir.acc(&["results"]), "no runs found");
}

#[test]
fn a_run_pauses_for_the_battery_and_by_hand_and_carries_on() {
    let dir = Dir::new("guard");
    dir.battery("90 1");
    let run = ["run", "--deals", "30k", "--once", "--gpu", "off", "--threads", "2"];
    let (lock, status) =
        (dir.file("checkpoints/batch-0-30000.lock"), dir.file("checkpoints/batch-0-30000.status.json"));
    let mut child = dir.spawn("run", &run);
    wait_until(&mut child, "the run started", || held(&lock));
    // While it goes, it can't be started a second time, nor carried on.
    refused(&dir.acc(&run), "already going in another window");
    refused(&dir.acc(&["resume", "--threads", "2"]), "already going");
    assert!(succeeded(&dir.acc(&["status"])).contains("state       going"));

    dir.battery("20 0");
    wait_until(&mut child, "a pause for the battery", || state(&status) == "paused" && !held(&lock));
    let shown = succeeded(&dir.acc(&["status"]));
    contains_all(
        &shown,
        &["batch-0-30000 (seeds 0 to 29,999)", "state       paused", "acc resume batch-0-30000"],
    );
    let results = succeeded(&dir.acc(&["results"]));
    contains_all(
        &results,
        &["batch-0-30000 isn't finished: it is paused", "win rate", "with 95% confidence"],
    );

    dir.battery("70 1");
    wait_until(&mut child, "the run carrying on", || held(&lock));
    let paused = succeeded(&dir.acc(&["pause"]));
    assert!(paused.contains("It has paused, at ") && paused.contains("acc resume batch-0-30000"), "{paused}");
    assert!(!dir.file("checkpoints/batch-0-30000.pause").exists());
    let (ok, _, said) = dir.finished("run", child);
    assert!(ok, "{said}");
    contains_all(
        &said,
        &[
            "same as: acc batch --from 0 --count 30000 --threads 2 --gpu off",
            "the battery is not on the charger, and down to 20%: pausing the run",
            "paused for the battery at",
            "battery 70%, on the charger: carrying on",
            "carrying on: acc batch --resume checkpoints/batch-0-30000.json --threads 2",
            "Paused at",
            "acc resume carries on",
        ],
    );

    let stdout = succeeded(&dir.acc(&["resume", "--once", "--threads", "2"]));
    contains_all(
        &stdout,
        &["deals            30000", "In short: 30,000 of 30,000 deals won", "acc results batch-0-30000"],
    );
    assert!(succeeded(&dir.acc(&["status"])).contains("state       finished"));
    let results = succeeded(&dir.acc(&["results"]));
    contains_all(
        &results,
        &["batch-0-30000 is finished. In short: 30,000 of 30,000", "deals            30000"],
    );
    refused(&dir.acc(&["resume", "--threads", "2"]), "nothing here is paused");

    // These deals have been played: starting them again takes --new, which keeps the old run's files.
    refused(&dir.acc(&["run", "--deals", "30k", "--threads", "2", "--dry-run"]), "played here already");
    let mut child =
        dir.spawn("again", &["run", "--deals", "30k", "--new", "--once", "--gpu", "off", "--threads", "2"]);
    wait_until(&mut child, "the new run started", || held(&lock));
    succeeded(&dir.acc(&["pause", "batch-0-30000"]));
    let (ok, _, said) = dir.finished("again", child);
    assert!(ok && said.contains("kept, as checkpoints/batch-0-30000.old-1.*"), "{said}");
    assert!(dir.file("checkpoints/batch-0-30000.old-1.report.txt").exists());
    let list = succeeded(&dir.acc(&["status"]));
    contains_all(&list, &["batch-0-30000 ", "paused", "batch-0-30000.old-1", "finished"]);
}

#[test]
fn a_run_for_some_hours_pauses_when_they_are_up() {
    let dir = Dir::new("hours");
    let output =
        dir.acc(&["run", "--deals", "1m", "--hours", "0.002", "--once", "--gpu", "off", "--threads", "2"]);
    succeeded(&output);
    contains_all(
        &text(&output.stderr),
        &[
            "--stop-after 0.002",
            "Leave this window open, keep the charger in and the lid open. Ctrl+C pauses safely.",
            "The 0.002 hours are up: the run is paused at",
            "acc results shows where it stands; acc resume carries on",
        ],
    );
    assert_eq!(state(&dir.file("checkpoints/batch-0-1000000.status.json")), "paused");
    assert!(succeeded(&dir.acc(&["results"])).contains("isn't finished"));
    // The same command carries on.
    let again = dir.acc(&[
        "run",
        "--deals",
        "1m",
        "--hours",
        "0.0005",
        "--once",
        "--gpu",
        "off",
        "--threads",
        "2",
        "--dry-run",
    ]);
    assert_eq!(
        without_memory(succeeded(&again).trim()),
        "acc batch --resume checkpoints/batch-0-1000000.json --threads 2 --stop-after 0.0005"
    );
    assert!(text(&again.stderr).contains("carrying on with it"), "{}", text(&again.stderr));
}

/// A status file as `acc batch` wrote it before it had `won`, `dossiers`, `save_every_s` and `checkpoint` (commit dbf57d4).
fn old_status(state: &str, updated: &str, from: u64, count: u64, done: u64) -> String {
    serde_json::json!({
        "state": state, "updated": updated, "from": from, "count": count, "done": done, "left": count - done,
        "percent": 100.0 * done as f64 / count as f64, "deals_per_s_this_session": 1000.0,
        "deals_per_s_overall": 900.0, "hours_left": 0.1, "hours_so_far": 0.2, "sessions": 2, "commit": "dbf57d4",
        "hardware": "2 threads", "lost": 1, "proving": 1, "undecided": 1, "unverified": 0, "proof_failed": 0,
        "not_won_by_solver": 4, "failed_verification": 0,
        "proofs": { "lost": 1, "won": 1, "undecided": 1, "failed": 0, "in_hand": 1 },
        "gpu": { "won": 0, "launches": 0, "attempts": 0 }, "deals_on_cpu": done, "games_log_gb": 0.0,
        "disk_free_gb": 100.0
    })
    .to_string()
}

#[test]
fn status_and_results_read_status_files_from_before_they_had_all_they_say_now() {
    let dir = Dir::new("old");
    std::fs::create_dir_all(dir.file("checkpoints")).unwrap();
    std::fs::create_dir_all(dir.file("hard-deals/proofs")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.file(name), text).unwrap();
    write(
        "checkpoints/batch-5-1000.status.json",
        &old_status("paused", "2026-11-01T10:00:00Z", 5, 1000, 600),
    );
    write("checkpoints/batch-5-1000.json", "{}");
    write(
        "checkpoints/batch-5-1000.events.log",
        "2026-11-01T09:00:00Z session 1 started at 0 deals\n2026-11-01T09:10:00Z seed 77: proven lost\n2026-11-01T09:20:00Z seed 12: undecided\n",
    );
    write("hard-deals/proofs/seed-77-loss.json", "");

    let shown = succeeded(&dir.acc(&["status"]));
    contains_all(
        &shown,
        &[
            "batch-5-1000 (seeds 5 to 1,004)",
            "state       paused",
            "600 of 1,000 deals",
            "won         597 so far",
            "lost        1 proven impossible",
        ],
    );
    let results = succeeded(&dir.acc(&["results"]));
    contains_all(
        &results,
        &[
            "won          597 of 600 deals",
            "lost         1: proven impossible",
            "proving      1:",
            "undecided    1:",
            "made at the end of the run",
            "Proven impossible (1):",
            "seed 77",
            "seed-77-loss.json",
        ],
    );
    assert!(!results.contains("seed 12 "), "{results}");

    // One that was going and has stopped, started in another folder; and one that is finished.
    write("checkpoints/batch-9-100.status.json", &old_status("running", "2026-01-01T00:00:00Z", 9, 100, 50));
    let elsewhere = std::env::temp_dir().display().to_string();
    write("checkpoints/batch-9-100.json", &serde_json::json!({ "dir": elsewhere }).to_string());
    write("checkpoints/batch-1-10.status.json", &old_status("finished", "2026-01-01T00:00:00Z", 1, 10, 10));
    write(
        "checkpoints/batch-1-10.report.txt",
        "deals            10 (seeds 1..10)\nwon              9 (every one verified)\nnot won          1 by the solver\nproofs           1 proven lost, 0 won, 0 undecided (two runs)\nfailed to verify 0\n",
    );
    let list = succeeded(&dir.acc(&["status"]));
    contains_all(&list, &["batch-1-10", "finished", "batch-5-1000", "paused", "batch-9-100", "stopped"]);
    assert!(succeeded(&dir.acc(&["status", "9-100"])).contains("stopped unexpectedly (last seen"));
    refused(&dir.acc(&["resume", "--threads", "2"]), "2 runs are paused here; say which");
    refused(
        &dir.acc(&["resume", "batch-9-100", "--threads", "2", "--dry-run"]),
        "can only carry on from there",
    );
    let line = succeeded(&dir.acc(&["resume", "batch-5-1000", "--threads", "2", "--dry-run"]));
    assert_eq!(without_memory(line.trim()), "acc batch --resume checkpoints/batch-5-1000.json --threads 2");
    let results = succeeded(&dir.acc(&["results", "batch-1-10"]));
    contains_all(
        &results,
        &["batch-1-10 is finished. In short: 9 of 10 deals won; 1 proven impossible.", "failed to verify 0"],
    );
    refused(&dir.acc(&["results"]), "say which");
    refused(&dir.acc(&["pause"]), "no run is going here");
    refused(&dir.acc(&["status", "batch-2-2"]), "no run named batch-2-2");
}

#[test]
fn verify_and_show_take_what_runs_leave() {
    let dir = Dir::new("files");
    let solve = ["solve", "--threads", "2", "--endgame", "off"];
    succeeded(&dir.acc(&[&solve[..], &["--seed", "1", "--out", "win.json"]].concat()));
    let shown = succeeded(&dir.acc(&["show", "win.json"]));
    contains_all(&shown, &["Deal 1. Rules:", "A winning line, 51 moves", "1 pile left"]);

    // A dossier, as a deal not won gets, and a loss record beside it, as a proof writes.
    dir.acc(&[&solve[..], &["--seed", "77", "--max-nodes", "1"]].concat());
    let dossier = dir.file("output/hard-deals/seed-77.json");
    assert!(dossier.exists());
    let mut record: Value =
        serde_json::from_str(&succeeded(&dir.acc(&["deal", "--seed", "77", "--json"]))).unwrap();
    record["result"] = "loss".into();
    std::fs::create_dir_all(dir.file("output/hard-deals/proofs")).unwrap();
    std::fs::write(dir.file("output/hard-deals/proofs/seed-77-loss.json"), record.to_string()).unwrap();

    let said = succeeded(&dir.acc(&["verify", "output/hard-deals/seed-77.json"]));
    contains_all(&said, &["this is a dossier", "seed-77-loss.json", "acc verify"]);
    let said = succeeded(&dir.acc(&["verify", "output/hard-deals/proofs"]));
    contains_all(&said, &["ok: loss claimed", "to re-check this, run: acc prove --seed 77"]);
    let said = succeeded(&dir.acc(&["verify", "output/hard-deals/proofs", "win.json"]));
    contains_all(&said, &["win.json:", "ok: win in 51 moves", "in all: 2 files, 2 records, 0 failed"]);
    // One file says what it always said.
    assert_eq!(
        succeeded(&dir.acc(&["verify", "win.json"])),
        "line 1  seed 1  ok: win in 51 moves\n1 record, 1 ok, 0 failed\n"
    );

    let output = dir.acc(&["show", "output/hard-deals/seed-77.json"]);
    assert!(succeeded(&output).contains("Deal 77. Rules:"));
    assert!(text(&output.stderr).contains("is a dossier"), "{}", text(&output.stderr));
    std::fs::create_dir(dir.file("empty")).unwrap();
    refused(&dir.acc(&["verify", "empty"]), "no .json or .jsonl files");
}

/// Ctrl+Break, which is all one process can send another on Windows, reaches the run, which pauses, and `acc run`, which
/// stays to say so rather than ending with it. It needs a console to send it through: without one, there is nothing to test.
#[cfg(windows)]
#[test]
fn ctrl_break_pauses_the_run_and_acc_run_stays_to_say_so() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    const CTRL_BREAK_EVENT: u32 = 1;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
    }
    let dir = Dir::new("ctrl");
    let (out, err) = (File::create(dir.file("run.out")).unwrap(), File::create(dir.file("run.err")).unwrap());
    let mut child = dir
        .command(&["run", "--deals", "1m", "--once", "--gpu", "off", "--threads", "2"])
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .unwrap();
    // Once the run says how to pause it, its own Ctrl+C handler is in place.
    let said = || std::fs::read_to_string(dir.file("run.err")).unwrap_or_default();
    wait_until(&mut child, "the run started", || said().contains("to pause, press Ctrl+C"));
    // SAFETY: an event and a process group's id, which is its first process's.
    if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } == 0 {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("skipped: no console");
        return;
    }
    let (ok, _, said) = dir.finished("run", child);
    assert!(ok, "{said}");
    // The run's own word for it, then this process's, which stayed.
    contains_all(
        &said,
        &["\npaused after ", "\ncarry on with: acc batch --resume", "Paused at", "acc resume carries on"],
    );
    assert_eq!(state(&dir.file("checkpoints/batch-0-1000000.status.json")), "paused");
}

#[test]
fn a_run_that_was_killed_is_seen_to_have_stopped_at_once() {
    let dir = Dir::new("killed");
    let args =
        ["batch", "--count", "1m", "--threads", "2", "--gpu", "off", "--no-log", "--checkpoint-every", "1"];
    let mut child = dir.spawn("batch", &args);
    let status = dir.file("checkpoints/batch-0-1000000.status.json");
    wait_until(&mut child, "a save", || state(&status) == "running");
    assert!(succeeded(&dir.acc(&["status"])).contains("state       going"));
    child.kill().unwrap();
    child.wait().unwrap();
    // Its status file is fresh and says running, but an acc that takes the lock is going only while it holds it.
    let shown = succeeded(&dir.acc(&["status"]));
    contains_all(
        &shown,
        &["state       stopped unexpectedly (last seen", "acc resume batch-0-1000000 carries on"],
    );
    let line = succeeded(&dir.acc(&["resume", "--threads", "2", "--dry-run"]));
    assert_eq!(
        without_memory(line.trim()),
        "acc batch --resume checkpoints/batch-0-1000000.json --threads 2"
    );
}
