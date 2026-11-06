//! `acc batch` interrupted every way it can be, against the same run left alone. Without a GPU every deal's solve is
//! deterministic, so what comes out must be the same to the byte, but for times.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const ACC: &str = env!("CARGO_BIN_EXE_acc");

/// A directory for one test, empty, removed when the test passes.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("acc-pause-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn acc(dir: &Path) -> Command {
    let mut command = Command::new(ACC);
    command.current_dir(dir).args(["batch", "--no-log"]);
    command
}

/// A run's settings: the CPU alone, in a few seconds.
fn run_args(from: u64, count: u64) -> Vec<String> {
    let mut args: Vec<String> = ["--gpu", "off", "--endgame", "off", "--threads", "4"].map(String::from).to_vec();
    args.extend(["--from".to_string(), from.to_string(), "--count".to_string(), count.to_string()]);
    args
}

fn with(mut args: Vec<String>, more: &[&str]) -> Vec<String> {
    args.extend(more.iter().map(|s| s.to_string()));
    args
}

fn spawn(dir: &Path, args: &[String]) -> Child {
    acc(dir).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
}

fn resume(dir: &Path, checkpoint: &str, more: &[&str]) -> Output {
    acc(dir).args(["--resume", checkpoint]).args(more).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Waits for `path` to appear, while `child` runs.
fn wait_for(path: &Path, child: &mut Child) {
    let start = Instant::now();
    while !path.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("it ended with {status} before {} appeared", path.display());
        }
        assert!(start.elapsed() < Duration::from_secs(120), "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The report's lines that don't depend on time, by their label.
fn report(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter(|l| !l.starts_with("ms/deal") && !l.starts_with("throughput"))
        .map(|l| (l[..17.min(l.len())].trim().to_string(), l.to_string()))
        .collect()
}

/// The solve records, without their times.
fn records(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| {
            let mut record: Value = serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l}"));
            record["stats"]["us"] = Value::Null;
            record
        })
        .collect()
}

/// How far a checkpoint had got.
fn done_in(path: &Path) -> u64 {
    let checkpoint: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    checkpoint["standing"]["done"].as_u64().unwrap()
}

fn succeeded(output: &Output) -> String {
    assert!(output.status.success(), "{:?}\n{}", output.status, text(&output.stderr));
    text(&output.stdout)
}

/// The same run left alone: its report and records.
fn left_alone(dir: &Dir, args: &[String]) -> (BTreeMap<String, String>, Vec<Value>) {
    let out = "alone.jsonl";
    let output = acc(&dir.0)
        .args(with(args.to_vec(), &["--no-checkpoint", "--dossiers", "alone-dossiers", "--out", out]))
        .output()
        .unwrap();
    (report(&succeeded(&output)), records(&dir.file(out)))
}

const INTERRUPTED: &[&str] = &["--dossiers", "dossiers", "--out", "out.jsonl", "--checkpoint", "cp.json"];

#[test]
fn a_pause_changes_nothing() {
    let dir = Dir::new("pause");
    let args = run_args(1_000, 60_000);
    let (report_alone, records_alone) = left_alone(&dir, &args);
    assert!(report_alone["deals"].contains("60000"));
    assert!(report_alone["distinct games"].contains("all 60000 deals differ"));

    // Pause over and over, with a file, until the run ends.
    let args = with(args, INTERRUPTED);
    let (checkpoint, pause) = (dir.file("cp.json"), dir.file("cp.pause"));
    let mut child = spawn(&dir.0, &args);
    let mut stops = Vec::new();
    let mut stdout;
    loop {
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(&pause, "").unwrap();
        let output = child.wait_with_output().unwrap();
        stdout = succeeded(&output);
        if !stdout.is_empty() {
            break;
        }
        assert!(text(&output.stderr).contains("--resume"), "{}", text(&output.stderr));
        let done = done_in(&checkpoint);
        assert!(stops.last().is_none_or(|&last| last < done), "no further than {done}: {stops:?}");
        stops.push(done);
        child = acc(&dir.0).args(["--resume", "cp.json"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    }
    assert!(stops.len() >= 2, "it should have paused at least twice, not at {stops:?}");
    assert_eq!(report(&stdout), report_alone, "after pausing at {stops:?}");
    assert_eq!(records(&dir.file("out.jsonl")), records_alone);
    // Done: the report stays, and nothing else (but the last pause file, if the run ended before it came).
    assert!(!checkpoint.exists() && !dir.file("cp.costs").exists());
    assert_eq!(report(&std::fs::read_to_string(dir.file("cp.report.txt")).unwrap()), report_alone);
    assert!(resume(&dir.0, "cp.json", &[]).status.code() != Some(0), "there is nothing left to resume");
}

#[test]
fn a_crash_loses_only_the_last_checkpoint() {
    let dir = Dir::new("crash");
    let args = run_args(5_000, 60_000);
    let (report_alone, records_alone) = left_alone(&dir, &args);

    let args = with(args, INTERRUPTED);
    let args = with(args, &["--checkpoint-every", "1"]);
    let checkpoint = dir.file("cp.json");
    let mut child = spawn(&dir.0, &args);
    wait_for(&checkpoint, &mut child);
    // On past the checkpoint, so that records and costs have been written that it doesn't count.
    std::thread::sleep(Duration::from_millis(300));
    child.kill().unwrap();
    child.wait().unwrap();
    let done = done_in(&checkpoint);
    assert!(done > 0 && done < 60_000, "done {done}");
    assert!(acc(&dir.0).args(&args).output().unwrap().status.code() != Some(0), "a second run must not overwrite it");

    let stdout = succeeded(&resume(&dir.0, "cp.json", &[]));
    assert_eq!(report(&stdout), report_alone);
    assert_eq!(records(&dir.file("out.jsonl")), records_alone);
}

#[test]
fn proofs_in_hand_are_proved_again() {
    let dir = Dir::new("proofs");
    // Few positions to win in, so that many deals go on to proofs, which are quick with so few positions of their own
    // but slower than the deals.
    let limits = ["--max-nodes", "300", "--prove", "3000"];
    let args = with(run_args(0, 3_000), &limits);
    let (report_alone, _) = left_alone(&dir, &args);
    let not_won: u64 = report_alone["not won"].split_whitespace().nth(2).unwrap().parse().unwrap();
    assert!(not_won > 10, "{}", report_alone["not won"]);

    // The first pause while deals are being solved; then until the end, including while the last proofs finish.
    let args = with(args, INTERRUPTED);
    let mut child = spawn(&dir.0, &args);
    let mut pending_seen = false;
    let mut stdout;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let _ = std::fs::write(dir.file("cp.pause"), "");
        let output = child.wait_with_output().unwrap();
        stdout = succeeded(&output);
        if !stdout.is_empty() {
            break;
        }
        let checkpoint: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.file("cp.json")).unwrap()).unwrap();
        pending_seen |= checkpoint["proofs_pending"].as_array().is_some_and(|p| !p.is_empty());
        child = acc(&dir.0).args(["--resume", "cp.json"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    }
    assert!(pending_seen, "no proofs were in hand at a pause: this test should be made harder");
    assert_eq!(report(&stdout), report_alone);
}

#[test]
fn the_check_for_repeats_works_from_disk_and_can_be_skipped() {
    let dir = Dir::new("disk");
    let args = run_args(9_000, 5_000);
    let (report_alone, _) = left_alone(&dir, &args);
    let on_disk = with(args.clone(), &["--no-checkpoint", "--dossiers", "d", "--games-memory", "0"]);
    let output = acc(&dir.0).args(&on_disk).output().unwrap();
    assert_eq!(report(&succeeded(&output))["distinct games"], report_alone["distinct games"]);
    let skipped = with(args, &["--no-checkpoint", "--dossiers", "d", "--no-games-check"]);
    let output = acc(&dir.0).args(&skipped).output().unwrap();
    assert!(report(&succeeded(&output))["distinct games"].contains("not checked"));
}

#[test]
fn a_run_carries_on_only_with_its_own_settings() {
    let dir = Dir::new("settings");
    let output = acc(&dir.0).args(["--resume", "cp.json", "--count", "5"]).output().unwrap();
    assert!(!output.status.success() && text(&output.stderr).contains("--count"), "{}", text(&output.stderr));
    let output = resume(&dir.0, "nothing-here.json", &[]);
    assert!(!output.status.success() && text(&output.stderr).contains("nothing-here"), "{}", text(&output.stderr));
}

#[test]
fn a_short_run_leaves_nothing_behind() {
    let dir = Dir::new("short");
    let output = acc(&dir.0).args(run_args(0, 200)).args(["--dossiers", "d"]).output().unwrap();
    succeeded(&output);
    assert!(!dir.file("checkpoints").exists(), "{:?}", std::fs::read_dir(&dir.0).unwrap().collect::<Vec<_>>());
}

/// Ctrl+Break, which is all that one process can send another on Windows, asks for a pause, and a second one stops the
/// run at once. It needs a console to send them through: without one, there is nothing to test.
#[cfg(windows)]
#[test]
fn ctrl_break_pauses_and_a_second_one_stops() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    const CTRL_BREAK_EVENT: u32 = 1;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
    }
    let dir = Dir::new("ctrl");
    let args = with(run_args(0, 5_000_000), &["--dossiers", "d", "--checkpoint", "cp.json"]);
    let start = || {
        acc(&dir.0)
            .args(&args)
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let send = |child: &Child| {
        // SAFETY: an event and a process group's id, which is its first process's.
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) != 0 }
    };

    let child = start();
    std::thread::sleep(Duration::from_secs(2));
    if !send(&child) {
        drop(child);
        eprintln!("skipped: no console");
        return;
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let done = done_in(&dir.file("cp.json"));
    assert!(done > 0 && done < 5_000_000, "done {done}");
    let _ = std::fs::remove_file(dir.file("cp.json"));

    // Two in a row stop it, with no time to pause: deals this easy take a few milliseconds to finish.
    let child = start();
    std::thread::sleep(Duration::from_secs(2));
    assert!(send(&child) && send(&child));
    let start = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{}", text(&output.stderr));
    assert!(start.elapsed() < Duration::from_secs(5));
}
