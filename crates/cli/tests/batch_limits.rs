//! What `acc batch` and the commands beside it accept and refuse, what they say, and what a very long run keeps. All of it
//! on the CPU, in folders of its own.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const ACC: &str = env!("CARGO_BIN_EXE_acc");
/// A checkpoint and its file of costs, paused partway through seeds 0..3000, as saved by the program before it capped the
/// costs (commit dbf57d4, format 3): the real thing, not this program's output with fields taken out. See tests/data/README.md.
const OLD_CHECKPOINT: &str = include_str!("data/v3-checkpoint.json");
const OLD_COSTS: &[u8] = include_bytes!("data/v3-checkpoint.costs");

/// A directory for one test, empty, removed when the test passes.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("acc-limits-test-{}-{name}", std::process::id()));
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

fn acc(dir: &Dir, args: &[&str]) -> Output {
    Command::new(ACC).current_dir(&dir.0).args(args).output().unwrap()
}

/// A command line for `acc batch` on the CPU alone, with `more`.
fn batch_args<'a>(more: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["batch", "--no-log", "--gpu", "off", "--endgame", "off", "--threads", "2"];
    args.extend(more);
    args
}

fn batch(dir: &Dir, more: &[&str]) -> Output {
    acc(dir, &batch_args(more))
}

fn resume_args<'a>(more: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["batch", "--no-log", "--threads", "2", "--resume", "cp.json"];
    args.extend(more);
    args
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn succeeded(output: &Output) -> String {
    assert!(output.status.success(), "{:?}\n{}", output.status, text(&output.stderr));
    text(&output.stdout)
}

fn refused(output: &Output, says: &str) {
    assert!(!output.status.success(), "it should have been refused: {}", text(&output.stdout));
    assert!(text(&output.stderr).contains(says), "wanted {says:?} in: {}", text(&output.stderr));
}

/// The report's lines that don't depend on the time or the commit, by their label.
fn report(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter(|l| !l.starts_with("ms/deal") && !l.starts_with("throughput") && !l.starts_with("commits"))
        .map(|l| (l[..17.min(l.len())].trim().to_string(), l.to_string()))
        .collect()
}

/// The word after `label` in `line`.
fn after<'a>(line: &'a str, label: &str) -> &'a str {
    let mut words = line.split_whitespace();
    words.find(|w| *w == label).unwrap_or_else(|| panic!("no {label} in {line}"));
    words.next().unwrap()
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Puts the old checkpoint here, as if the run had started in this folder.
fn old_checkpoint(dir: &Dir) -> Value {
    let mut checkpoint: Value = serde_json::from_str(OLD_CHECKPOINT).unwrap();
    checkpoint["dir"] = Value::String(dir.0.to_str().unwrap().to_string());
    std::fs::write(dir.file("cp.json"), serde_json::to_string_pretty(&checkpoint).unwrap()).unwrap();
    std::fs::write(dir.file("cp.costs"), OLD_COSTS).unwrap();
    checkpoint
}

/// The same run as the old checkpoint's, left alone: its report.
fn old_run_left_alone(dir: &Dir, checkpoint: &Value) -> BTreeMap<String, String> {
    let count = checkpoint["args"]["count"].as_u64().unwrap().to_string();
    report(&succeeded(&batch(
        dir,
        &["--from", "0", "--count", &count, "--dossiers", "alone", "--no-checkpoint"],
    )))
}

#[test]
fn a_checkpoint_from_before_the_cap_carries_on() {
    let dir = Dir::new("old");
    let checkpoint = old_checkpoint(&dir);
    // That this is what it is meant to be.
    assert_eq!(checkpoint["version"], 3);
    let tally = checkpoint["tally"].as_object().unwrap();
    for new in ["all_cpu_deals", "total_micros", "most_nodes", "most_micros"] {
        assert!(!tally.contains_key(new), "{new} is in the old checkpoint");
    }
    let done = checkpoint["standing"]["done"].as_u64().unwrap();
    assert!(done > 0 && done < 3_000, "done {done}");
    assert_eq!(tally["cpu_deals"].as_u64().unwrap(), done);
    assert_eq!(OLD_COSTS.len() as u64, done * 16);

    let alone = old_run_left_alone(&dir, &checkpoint);
    let stdout = succeeded(&acc(&dir, &resume_args(&[])));
    assert_eq!(report(&stdout), alone);
    assert!(!dir.file("cp.json").exists() && !dir.file("cp.costs").exists(), "it cleans up");
}

#[test]
fn an_old_checkpoint_over_the_cap_is_cut_to_it_and_its_totals_stay_exact() {
    let dir = Dir::new("old-over");
    let checkpoint = old_checkpoint(&dir);
    let alone = old_run_left_alone(&dir, &checkpoint);
    let output = Command::new(ACC)
        .current_dir(&dir.0)
        .args(resume_args(&[]))
        .env("ACC_COST_SAMPLE_MOST", "50")
        .output()
        .unwrap();
    let capped = report(&succeeded(&output));
    let (line, expected) = (&capped["positions/deal"], &alone["positions/deal"]);
    assert_eq!(after(line, "mean"), after(expected, "mean"));
    assert_eq!(after(line, "max"), after(expected, "max"));
    assert!(capped["cost sample"].contains("first 50 of the 3000"), "{}", capped["cost sample"]);
    for label in ["deals", "won", "not won", "proofs", "failed to verify", "distinct games", "win rate"] {
        assert_eq!(capped[label], alone[label]);
    }
}

#[test]
fn a_save_that_fails_after_the_cut_leaves_the_old_checkpoint_whole() {
    let dir = Dir::new("old-over-failed");
    let checkpoint = old_checkpoint(&dir);
    let alone = old_run_left_alone(&dir, &checkpoint);
    let done = checkpoint["standing"]["done"].as_u64().unwrap();
    let capped = |more: &[&str]| {
        let mut command = Command::new(ACC);
        command.current_dir(&dir.0).args(resume_args(more)).env("ACC_COST_SAMPLE_MOST", "50");
        command.output().unwrap()
    };
    // The new checkpoint can't be written: a folder is where it goes first.
    std::fs::create_dir(dir.file("cp.tmp")).unwrap();
    let until = (done + 1).to_string();
    let output = capped(&["--until", &until]);
    assert!(!output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("note: this run's file of costs has"), "{}", text(&output.stderr));
    assert!(
        std::fs::read(dir.file("cp.costs")).unwrap() == OLD_COSTS,
        "the costs must still be the checkpoint's"
    );
    assert_eq!(json(&dir.file("cp.json"))["tally"]["cpu_deals"].as_u64().unwrap(), done);

    // So it carries on from there, its totals as exact as ever.
    std::fs::remove_dir(dir.file("cp.tmp")).unwrap();
    let capped = report(&succeeded(&capped(&[])));
    let (line, expected) = (&capped["positions/deal"], &alone["positions/deal"]);
    assert_eq!(after(line, "mean"), after(expected, "mean"));
    assert_eq!(capped["win rate"], alone["win rate"]);
}

#[test]
fn a_pause_file_from_before_the_run_is_ignored_and_one_made_as_it_starts_is_kept() {
    let dir = Dir::new("pause-file");
    // Left by a session that has ended: this run goes to its end.
    std::fs::write(dir.file("cp.pause"), "").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let args = batch_args(&["--count", "200", "--dossiers", "d", "--checkpoint", "cp.json"]);
    assert!(report(&succeeded(&acc(&dir, &args))).contains_key("win rate"));
    assert!(!dir.file("cp.pause").exists());

    // Made once the run has its lock, perhaps before it watches for one (acc pause, say): it pauses. Not at once, as
    // a file system may give a file made in the same moment a time a little before it.
    let args = batch_args(&["--count", "1m", "--dossiers", "d", "--checkpoint", "cp.json"]);
    let mut child = spawn(&dir, &args);
    wait_for(&dir.file("cp.lock"), &mut child);
    std::thread::sleep(Duration::from_millis(100));
    std::fs::write(dir.file("cp.pause"), "").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success() && output.stdout.is_empty(), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("paused after"), "{}", text(&output.stderr));
}

fn spawn(dir: &Dir, args: &[&str]) -> Child {
    Command::new(ACC)
        .current_dir(&dir.0)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
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

#[test]
fn the_costs_kept_stop_at_the_cap_and_a_resume_keeps_them_stopped() {
    let dir = Dir::new("cap");
    let args =
        batch_args(&["--count", "1m", "--dossiers", "d", "--checkpoint", "cp.json", "--until", "2000"]);
    let output = Command::new(ACC)
        .current_dir(&dir.0)
        .args(&args)
        .env("ACC_COST_SAMPLE_MOST", "500")
        .output()
        .unwrap();
    succeeded(&output);
    let saved = json(&dir.file("cp.json"));
    let done = saved["standing"]["done"].as_u64().unwrap();
    assert!((2_000..1_000_000).contains(&done), "done {done}");
    assert_eq!(saved["tally"]["cpu_deals"].as_u64().unwrap(), 500);
    assert_eq!(saved["tally"]["all_cpu_deals"].as_u64().unwrap(), done);
    assert_eq!(std::fs::metadata(dir.file("cp.costs")).unwrap().len(), 500 * 16);
    assert_eq!(json(&dir.file("cp.status.json"))["deals_on_cpu"].as_u64().unwrap(), done);
    let total_micros = saved["tally"]["total_micros"].as_u64().unwrap();
    assert!(total_micros > 0);

    // A higher cap doesn't reopen it: what is kept has to stay the first deals.
    let until = (done + 1).to_string();
    let output = Command::new(ACC)
        .current_dir(&dir.0)
        .args(resume_args(&["--until", &until]))
        .env("ACC_COST_SAMPLE_MOST", "5000")
        .output()
        .unwrap();
    succeeded(&output);
    let saved = json(&dir.file("cp.json"));
    let later = saved["standing"]["done"].as_u64().unwrap();
    assert!(later > done, "from {done} to {later}");
    assert_eq!(saved["tally"]["cpu_deals"].as_u64().unwrap(), 500);
    assert_eq!(saved["tally"]["all_cpu_deals"].as_u64().unwrap(), later);
    assert!(saved["tally"]["total_micros"].as_u64().unwrap() > total_micros);
    assert_eq!(std::fs::metadata(dir.file("cp.costs")).unwrap().len(), 500 * 16);
}

#[test]
fn over_the_cap_the_report_says_so_and_only_the_percentiles_change() {
    let dir = Dir::new("report");
    let args = ["--count", "4000", "--dossiers", "d", "--no-checkpoint"];
    let alone = report(&succeeded(&batch(&dir, &args)));
    assert!(!alone.contains_key("cost sample"));

    // A cap the run doesn't reach changes nothing.
    let with_cap = |cap: &str| {
        let output = Command::new(ACC)
            .current_dir(&dir.0)
            .args(batch_args(&args))
            .env("ACC_COST_SAMPLE_MOST", cap)
            .output();
        report(&succeeded(&output.unwrap()))
    };
    assert_eq!(with_cap("4000"), alone);

    let capped = with_cap("3999");
    assert!(capped["cost sample"].contains("first 3999 of the 4000"), "{}", capped["cost sample"]);
    let (line, expected) = (&capped["positions/deal"], &alone["positions/deal"]);
    assert_eq!(after(line, "mean"), after(expected, "mean"));
    assert_eq!(after(line, "max"), after(expected, "max"));
    assert_eq!(capped.len(), alone.len() + 1);
}

#[test]
fn two_processes_cannot_run_the_same_checkpoint() {
    let dir = Dir::new("lock");
    let args = batch_args(&[
        "--count",
        "1t",
        "--dossiers",
        "d",
        "--checkpoint",
        "cp.json",
        "--checkpoint-every",
        "1",
    ]);
    let mut first = spawn(&dir, &args);
    wait_for(&dir.file("cp.json"), &mut first);
    const SAYS: &str = "this run is already going in another window";
    refused(&acc(&dir, &args), SAYS);
    refused(&acc(&dir, &resume_args(&[])), SAYS);
    // What they were refused must not have touched the run.
    assert!(first.try_wait().unwrap().is_none(), "the first run stopped");
    assert!(dir.file("cp.lock").exists());
    std::fs::write(dir.file("cp.pause"), "").unwrap();
    succeeded(&first.wait_with_output().unwrap());
    assert!(!dir.file("cp.lock").exists(), "a pause gives the lock up");

    // With nobody holding it, a lock file left by a crash is no obstacle.
    std::fs::write(dir.file("cp.lock"), "").unwrap();
    succeeded(&acc(&dir, &resume_args(&["--until", "1"])));
}

#[test]
fn a_short_run_with_a_checkpoint_leaves_nothing_at_all() {
    let dir = Dir::new("short");
    succeeded(&batch(&dir, &["--count", "100", "--dossiers", "d", "--checkpoint", "a/b/cp.json"]));
    assert!(!dir.file("a").exists(), "folders made for the run should go with it");
    // But not a folder that was there before.
    std::fs::create_dir(dir.file("kept")).unwrap();
    succeeded(&batch(&dir, &["--count", "100", "--dossiers", "d", "--checkpoint", "kept/cp.json"]));
    assert_eq!(std::fs::read_dir(dir.file("kept")).unwrap().count(), 0);
}

#[test]
fn counts_and_limits_that_make_no_sense_are_refused() {
    let dir = Dir::new("refused");
    let no_file = ["--dossiers", "d"];
    let with = |more: &[&str]| {
        let mut args = no_file.to_vec();
        args.extend(more);
        batch(&dir, &args)
    };
    refused(&with(&["--count", "0"]), "--count");
    refused(&with(&["--from", "18446744073709551615", "--count", "2"]), "past the last seed");
    refused(&with(&["--from", "2", "--count", "18446744073709551615"]), "past the last seed");
    for bad in ["--stop-after=NaN", "--stop-after=inf", "--stop-after=-1", "--stop-after=1e300"] {
        refused(&with(&[bad]), "--stop-after");
    }
    refused(&with(&["--games-memory", "17179869184"]), "--games-memory");
    refused(&with(&["--prove-memory", "17179869184"]), "--prove-memory");
    refused(&acc(&dir, &["prove", "--seed", "0", "--memory", "17179869184", "--endgame", "off"]), "--memory");
    let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
    assert!(left.is_empty(), "a refused run should touch nothing: {left:?}");

    // The very last seed there is can be asked for.
    let stdout = succeeded(&with(&["--from", "18446744073709551615", "--count", "1", "--no-checkpoint"]));
    assert!(stdout.contains("seeds 18446744073709551615..18446744073709551615"), "{stdout}");
}

#[test]
fn numbers_can_be_written_the_short_way() {
    let dir = Dir::new("numbers");
    let stdout =
        succeeded(&batch(&dir, &["--from", "1m", "--count", "2k", "--no-checkpoint", "--dossiers", "d"]));
    assert!(stdout.contains("deals            2000 (seeds 1000000..1001999)"), "{stdout}");
    refused(&batch(&dir, &["--count", "1,000"]), "expected a whole number such as 5000, 2m, 1.5b or 100t");
    let quick = ["--endgame", "off", "--dossiers", "d", "--seed", "1"];
    let one = |command: &str, limit: &str, value: &str| {
        let mut args = vec![command, limit, value];
        args.extend(quick);
        acc(&dir, &args)
    };
    succeeded(&one("solve", "--max-nodes", "1e7"));
    assert_ne!(one("solve", "--max-nodes", "1.5").status.code(), Some(0));
    assert_ne!(one("prove", "--max-positions", "1k").status.code(), Some(2), "not a usage error");
    refused(&one("prove", "--max-positions", "1.5"), "expected a whole number");
    succeeded(&acc(
        &dir,
        &["selftest", "--playouts", "1k", "--positions", "1e2", "--residuals", "0", "--proofs", "0"],
    ));
    succeeded(&acc(&dir, &["bench", "--deals", "1", "--depths", "40", "--budget", "1k"]));
}

#[test]
fn help_is_grouped_short_or_complete() {
    let dir = Dir::new("help");
    let long = succeeded(&acc(&dir, &["batch", "--help"]));
    for heading in [
        "Which deals:",
        "Speed and machine:",
        "Stopping and carrying on:",
        "Output:",
        "Proving losses:",
        "Repeated-game check:",
        "Research:",
        "Examples:",
    ] {
        assert!(long.contains(heading), "{heading} in {long}");
    }
    for flag in [
        "--table",
        "--games-disk",
        "--checkpoint-every",
        "--prove-memory",
        "--count",
        "[possible values: auto, on, off]",
    ] {
        assert!(long.contains(flag), "{flag} in {long}");
    }
    for hidden in ["--gpu-kernel", "--gpu-define", "--plant-repeats"] {
        assert!(!long.contains(hidden), "{hidden} in {long}");
    }
    let short = succeeded(&acc(&dir, &["batch", "-h"]));
    let flags = short.lines().filter(|l| l.trim_start().starts_with("--")).count();
    assert!((6..=10).contains(&flags), "{flags} flags in {short}");
    assert!(short.contains("--count") && short.contains("--resume") && !short.contains("--table"), "{short}");

    for command in ["solve", "prove"] {
        assert!(succeeded(&acc(&dir, &[command, "--help"])).contains("Examples:"), "{command}");
    }
    for command in ["deal", "solve"] {
        let help = succeeded(&acc(&dir, &[command, "--help"]));
        assert!(help.contains("A♣") && !help.contains("♣A"), "{help}");
    }
    let top = succeeded(&acc(&dir, &["--help"]));
    assert!(!top.contains("PLAN.md") && !top.contains("lockstep"), "{top}");
    refused(&acc(&dir, &["batch", "--gpu", "maybe"]), "maybe");
}

#[test]
fn the_rule_on_what_may_be_given_with_resume_stands() {
    let dir = Dir::new("resume-rule");
    old_checkpoint(&dir);
    for not_allowed in ["--count", "--from", "--max-nodes", "--prove", "--gallery", "--games-disk"] {
        refused(&acc(&dir, &resume_args(&[not_allowed, "5"])), not_allowed);
    }
    for not_allowed in [["--gpu", "off"], ["--endgame", "off"], ["--dossiers", "d"], ["--config", "x"]] {
        refused(&acc(&dir, &resume_args(&not_allowed)), not_allowed[0]);
    }
    let allowed = [
        "--checkpoint-every",
        "60",
        "--log-dir",
        "logs",
        "--games-memory",
        "1",
        "--keep-free",
        "0.001",
        "--until",
        "1",
        "--stop-after",
        "1",
    ];
    succeeded(&acc(&dir, &resume_args(&allowed)));
}

const LADDER: &str = r#"{"ladder": {"start": 30}}"#;

#[test]
fn settings_can_come_from_a_file_in_any_of_windows_encodings() {
    let dir = Dir::new("config");
    let line = |config: &str| {
        let args = ["--count", "300", "--no-checkpoint", "--dossiers", "d", "--config", config];
        report(&succeeded(&batch(&dir, &args)))["positions/deal"].clone()
    };
    let inline = line(LADDER);
    let default = report(&succeeded(&batch(&dir, &["--count", "300", "--no-checkpoint", "--dossiers", "d"])))
        ["positions/deal"]
        .clone();
    assert_ne!(inline, default, "the setting should have made a difference");

    let utf16 = |to_bytes: fn(u16) -> [u8; 2], bom: [u8; 2]| {
        let mut bytes = bom.to_vec();
        bytes.extend(LADDER.encode_utf16().flat_map(to_bytes));
        bytes
    };
    let files: [(&str, Vec<u8>); 4] = [
        ("plain.json", LADDER.as_bytes().to_vec()),
        ("utf8.json", [&[0xEF, 0xBB, 0xBF], LADDER.as_bytes()].concat()),
        ("utf16le.json", utf16(u16::to_le_bytes, [0xFF, 0xFE])),
        ("utf16be.json", utf16(u16::to_be_bytes, [0xFE, 0xFF])),
    ];
    for (name, bytes) in files {
        std::fs::write(dir.file(name), bytes).unwrap();
        assert_eq!(line(name), inline, "{name}");
    }
    let gpu = ["--count", "10", "--no-checkpoint", "--dossiers", "d", "--gpu-config", "plain.json"];
    refused(&batch(&dir, &gpu), "unknown field");
    succeeded(&batch(&dir, &["--count", "10", "--no-checkpoint", "--dossiers", "d", "--gpu-config", "{}"]));

    refused(&batch(&dir, &["--config", "not-there.json"]), "not-there.json");
    const QUOTES: &str = "your shell removed the quotes; put the JSON in a file and pass its name";
    refused(&batch(&dir, &["--config", "{ladder: {start: 30}}"]), QUOTES);
    let bad = batch(&dir, &["--config", r#"{"ladder": 3"#]);
    assert!(!text(&bad.stderr).contains(QUOTES), "{}", text(&bad.stderr));

    let solved = ["solve", "--seed", "1", "--endgame", "off", "--dossiers", "d", "--config", "utf16le.json"];
    succeeded(&acc(&dir, &solved));
    std::fs::write(dir.file("variant.json"), r#"{"name": "from a file"}"#).unwrap();
    let experiment =
        ["experiment", "--set", "0:5", "--threads", "2", "--out", "e", "--config", "variant.json"];
    assert!(succeeded(&acc(&dir, &experiment)).contains("from a file"));
}

#[test]
fn what_is_not_won_goes_outside_the_tracked_record_by_default() {
    let dir = Dir::new("output");
    let args = ["--count", "2000", "--max-nodes", "300", "--prove", "0", "--no-checkpoint"];
    succeeded(&batch(&dir, &args));
    assert!(dir.file("output/hard-deals/INDEX.md").exists());
    assert!(!dir.file("hard-deals").exists());

    let solve =
        acc(&dir, &["solve", "--seed", "77", "--max-nodes", "1", "--threads", "1", "--endgame", "off"]);
    assert!(text(&solve.stderr).contains("output"), "{}", text(&solve.stderr));
    assert!(dir.file("output/hard-deals/seed-77.json").exists());

    acc(&dir, &["prove", "--seed", "78", "--max-positions", "10", "--endgame", "off"]);
    assert!(dir.file("output/hard-deals/seed-78.json").exists());
    assert!(!dir.file("hard-deals").exists());

    let experiment = ["experiment", "--set", "0:5", "--threads", "2", "--config", r#"{"name": "a"}"#];
    succeeded(&acc(&dir, &experiment));
    assert!(dir.file("output/experiments/summary.jsonl").exists());
    assert!(!dir.file("research").exists());
}

#[test]
fn the_status_file_says_what_a_front_end_needs() {
    let dir = Dir::new("status");
    let args = [
        "--count",
        "1m",
        "--dossiers",
        "here",
        "--checkpoint",
        "cp.json",
        "--checkpoint-every",
        "40",
        "--until",
        "1000",
    ];
    succeeded(&batch(&dir, &args));
    let status = json(&dir.file("cp.status.json"));
    let (done, not_won) = (status["done"].as_u64().unwrap(), status["not_won_by_solver"].as_u64().unwrap());
    assert_eq!(status["won"].as_u64().unwrap(), done - not_won);
    assert_eq!(status["dossiers"], "here");
    assert_eq!(status["save_every_s"].as_u64().unwrap(), 40);
    assert_eq!(status["checkpoint"], "cp.json");
    // All that was there before stays.
    for old in [
        "state",
        "updated",
        "from",
        "count",
        "done",
        "left",
        "percent",
        "lost",
        "proving",
        "deals_on_cpu",
        "disk_free_gb",
    ] {
        assert!(status.get(old).is_some(), "{old}");
    }

    let until = (done + 1).to_string();
    succeeded(&acc(&dir, &resume_args(&["--checkpoint-every", "7", "--until", &until])));
    assert_eq!(json(&dir.file("cp.status.json"))["save_every_s"].as_u64().unwrap(), 7);
}

#[test]
fn the_full_check_on_disk_is_left_for_a_sample_when_it_would_not_fit_in_memory() {
    let dir = Dir::new("plan");
    let args = [
        "--count",
        "1t",
        "--games-memory",
        "0",
        "--games-disk",
        "10000",
        "--dossiers",
        "d",
        "--checkpoint",
        "cp.json",
        "--until",
        "1",
    ];
    let output = batch(&dir, &args);
    succeeded(&output);
    let said = text(&output.stderr);
    assert!(said.contains("would want about") && said.contains("a sample"), "{said}");
    assert!(json(&dir.file("cp.json"))["games"].get("Sampled").is_some());
}

#[test]
fn checkpoints_that_cannot_be_used_say_what_to_do() {
    let dir = Dir::new("errors");
    old_checkpoint(&dir);
    std::fs::remove_file(dir.file("cp.costs")).unwrap();
    refused(&acc(&dir, &resume_args(&[])), "cp.*");

    std::fs::write(dir.file("cp.json"), r#"{"version": 3}"#).unwrap();
    refused(&acc(&dir, &resume_args(&[])), "different version of acc");

    let mut checkpoint: Value = serde_json::from_str(OLD_CHECKPOINT).unwrap();
    checkpoint["version"] = 99.into();
    std::fs::write(dir.file("cp.json"), checkpoint.to_string()).unwrap();
    refused(&acc(&dir, &resume_args(&[])), "different version of acc");

    let args = batch_args(&["--count", "3000", "--dossiers", "d", "--checkpoint", "cp.json"]);
    refused(&acc(&dir, &args), "cp.*");
    assert!(!dir.file("cp.lock").exists());
}
