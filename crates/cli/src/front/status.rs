//! `acc status`, `acc pause` and `acc results`: what the runs started here are doing and have found, from the files they
//! keep for that (their status files, logs of events and reports), and a pause asked of one by the file it watches for.

use super::{Run, State, ago, figures, find, in_short, locked, nothing_here, runs_here};
use crate::progress::{duration, thousands};
use crate::runner::wilson;
use clap::Args;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const STATUS_EXAMPLES: &str = "\
Examples:
  acc status                     every run started in this folder
  acc status batch-0-1000000     one of them in full";

const PAUSE_EXAMPLES: &str = "\
Examples:
  acc pause                      the run going here (Ctrl+C in its own window does the same)
  acc pause batch-0-1000000      this one, when several are going";

const RESULTS_EXAMPLES: &str = "\
Examples:
  acc results                    the run here: its report, or the win rate so far
  acc results batch-0-1000000    this one, when there are several";

/// How long `acc pause` waits for the run to take its request.
const PAUSE_WAIT: Duration = Duration::from_secs(30);
/// Proven losses listed by name, at most.
const LOSSES_SHOWN: usize = 50;

#[derive(Args)]
#[command(after_help = STATUS_EXAMPLES)]
pub struct StatusArgs {
    /// Which run, as the list names them (default: all of them, or the one there is in full).
    name: Option<String>,
}

#[derive(Args)]
#[command(after_help = PAUSE_EXAMPLES)]
pub struct PauseArgs {
    /// Which run, when several are going.
    name: Option<String>,
}

#[derive(Args)]
#[command(after_help = RESULTS_EXAMPLES)]
pub struct ResultsArgs {
    /// Which run, when there are several.
    name: Option<String>,
}

/// A run's state in a few words.
fn state_text(run: &Run) -> String {
    match run.state() {
        State::Live if run.word() == "finishing" => {
            "finishing: every deal is done, the last checks are running".into()
        }
        State::Live => "going".into(),
        State::Paused => match run.word() {
            "paused: no room" => "paused: the disk was nearly full".into(),
            "paused: the GPU failed" => "paused: the GPU failed".into(),
            _ => "paused".into(),
        },
        State::Stopped(age) => format!(
            "stopped unexpectedly{}",
            age.map_or(String::new(), |a| format!(" (last seen {})", ago(a)))
        ),
        State::Finished => "finished".into(),
        State::Gone => "gone: no checkpoint or report left".into(),
    }
}

/// What to type next for a run.
fn next_text(run: &Run) -> String {
    let name = &run.name;
    match run.state() {
        State::Live => format!("acc pause {name} pauses it; acc results {name} shows the win rate so far"),
        State::Paused | State::Stopped(_) => {
            format!("acc resume {name} carries on; acc results {name} shows the win rate so far")
        }
        State::Finished => format!("acc results {name} shows what it found"),
        State::Gone => "nothing: its files can be deleted".into(),
    }
}

pub fn status(args: &StatusArgs) -> Result<bool, String> {
    let runs = runs_here();
    if runs.is_empty() {
        println!("{}.", nothing_here());
        return Ok(true);
    }
    match (&args.name, runs.as_slice()) {
        (Some(name), _) => show_one(find(&runs, name)?),
        (None, [run]) => show_one(run),
        (None, runs) => {
            for run in runs {
                println!("{:<28} {:<10} {}", run.name, short_state(run), run.done_text());
            }
            println!("\nacc status <name> shows one in full; acc results <name> what it found.");
        }
    }
    Ok(true)
}

fn short_state(run: &Run) -> &'static str {
    match run.state() {
        State::Live if run.word() == "finishing" => "finishing",
        State::Live => "going",
        State::Paused => "paused",
        State::Stopped(_) => "stopped",
        State::Finished => "finished",
        State::Gone => "gone",
    }
}

fn show_one(run: &Run) {
    let state = run.state();
    println!("{} ({})", run.name, run.seeds());
    println!("  state       {}", state_text(run));
    if let Some(saved) = run.saved() {
        println!("  saved       {saved}");
    }
    if run.status.is_null() {
        println!("  done        {}", run.done_text());
    } else {
        let (done, count) = (run.num("done"), run.num("count"));
        println!("  done        {}, {} left", run.done_text(), thousands(count.saturating_sub(done)));
        let session = run.status["deals_per_s_this_session"].as_f64().unwrap_or(0.0);
        let overall = run.status["deals_per_s_overall"].as_f64().unwrap_or(0.0);
        let to_go = run.status["hours_left"]
            .as_f64()
            .filter(|_| state == State::Live)
            .map_or(String::new(), |h| format!("; about {} to go at that speed", duration(h * 3600.0)));
        let when = if state == State::Live { "" } else { " when last going" };
        println!(
            "  speed       {} deals a second{when} ({} over the whole run){to_go}",
            thousands(session.round() as u64),
            thousands(overall.round() as u64)
        );
        let so_far = if state == State::Finished { "" } else { " so far" };
        println!("  won         {}{so_far}", thousands(run.won()));
        for (key, what) in [
            ("lost", "proven impossible"),
            ("proving", "being proved"),
            ("undecided", "not decided either way"),
            ("unverified", "FAILED VERIFICATION"),
            ("proof_failed", "PROOFS FAILED"),
        ] {
            let n = run.num(key);
            if n > 0 || matches!(key, "lost" | "proving" | "undecided") {
                println!("  {:<11} {} {what}", key.replace('_', " "), thousands(n));
            }
        }
        if let Some(free) = run.status["disk_free_gb"].as_f64() {
            println!("  disk free   {free:.0} GB");
        }
    }
    println!("  next        {}", next_text(run));
}

pub fn pause(args: &PauseArgs) -> Result<bool, String> {
    let runs = runs_here();
    let run = match &args.name {
        Some(name) => {
            let run = find(&runs, name)?;
            if run.state() != State::Live {
                return Err(format!(
                    "{} isn't going ({}), so there is nothing to pause",
                    run.name,
                    state_text(run)
                ));
            }
            run
        }
        None => {
            let live: Vec<&Run> = runs.iter().filter(|r| r.state() == State::Live).collect();
            match live.as_slice() {
                [run] => *run,
                [] if runs.is_empty() => return Err(nothing_here()),
                [] => return Err("no run is going here: acc status lists the runs here".into()),
                several => {
                    let names: Vec<String> =
                        several.iter().map(|r| format!("  {:<28} {}", r.name, r.done_text())).collect();
                    return Err(format!(
                        "{} runs are going here; say which, as in acc pause {}:\n{}",
                        several.len(),
                        several[0].name,
                        names.join("\n")
                    ));
                }
            }
        }
    };
    let file = run.file("pause");
    std::fs::write(&file, "").map_err(|e| format!("creating {}: {e}", file.display()))?;
    println!("Asked {} to pause ({}).", run.name, file.display());
    // A run that holds its lock can be seen to stop. One that doesn't (an older acc) can only be seen to take the request.
    let with_lock = locked(&run.name);
    let (start, mut asked, mut taken) = (Instant::now(), Instant::now(), false);
    while start.elapsed() < PAUSE_WAIT {
        if !file.exists() {
            taken = true;
            if !with_lock || !locked(&run.name) {
                break;
            }
            // A run still starting up deletes a pause file it finds, as one left from before: so it is asked again.
            if asked.elapsed() >= Duration::from_secs(2) {
                let _ = std::fs::write(&file, "");
                asked = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let paused = with_lock && !locked(&run.name);
    // What a run that stopped, or one done asking, leaves of the request.
    if paused || !taken {
        let _ = std::fs::remove_file(&file);
    }
    match (paused, taken) {
        (true, _) => println!(
            "It has paused, at {}. acc resume {} carries on.",
            Run::new(&run.name).done_text(),
            run.name
        ),
        (false, true) if with_lock => println!(
            "It took the request, but is still finishing what it had in hand after {} seconds (the last proofs, say); its own window says when it has paused.",
            PAUSE_WAIT.as_secs()
        ),
        (false, true) => println!(
            "It took the request, and is finishing the deals it has in hand; its own window says when it has paused. acc resume {} carries on.",
            run.name
        ),
        (false, false) => {
            return Err(format!(
                "{} didn't take the request within {} seconds, so it was taken back. The run may have stopped already: acc status shows it",
                run.name,
                PAUSE_WAIT.as_secs()
            ));
        }
    }
    Ok(true)
}

pub fn results(args: &ResultsArgs) -> Result<bool, String> {
    let runs = runs_here();
    let run = match (&args.name, runs.as_slice()) {
        (Some(name), _) => find(&runs, name)?,
        (None, [run]) => run,
        (None, []) => return Err(nothing_here()),
        (None, several) => {
            let names: Vec<String> =
                several.iter().map(|r| format!("  {:<28} {}", r.name, state_text(r))).collect();
            return Err(format!(
                "there are {} runs here; say which, as in acc results {}:\n{}",
                several.len(),
                several[0].name,
                names.join("\n")
            ));
        }
    };
    let report = std::fs::read_to_string(run.file("report.txt")).ok();
    match (run.state(), report) {
        (State::Finished, Some(report)) => {
            let short = figures(&report).map_or(String::new(), |f| format!(" {}", in_short(&f)));
            println!(
                "{} is finished.{short} Its report, from {}:\n",
                run.name,
                run.file("report.txt").display()
            );
            print!("{report}");
        }
        (State::Finished, None) => {
            return Err(format!(
                "{} is finished, but its report, {}, isn't there",
                run.name,
                run.file("report.txt").display()
            ));
        }
        (_, _) if run.status.is_null() => {
            println!("{} has only just started: nothing is saved yet. Try again in a minute.", run.name);
            return Ok(true);
        }
        _ => provisional(run),
    }
    let losses = proven_lost(run);
    if !losses.is_empty() {
        println!("\nProven impossible ({}):", thousands(losses.len() as u64));
        let dossiers = dossiers(run);
        for seed in losses.iter().take(LOSSES_SHOWN) {
            let record = dossiers
                .as_ref()
                .map(|d| d.join("proofs").join(format!("seed-{seed}-loss.json")))
                .filter(|p| p.exists())
                .map_or(String::new(), |p| p.display().to_string());
            println!("  seed {seed:<22} {record}");
        }
        if losses.len() > LOSSES_SHOWN {
            println!("  and {} more, in {}", losses.len() - LOSSES_SHOWN, run.file("events.log").display());
        }
        println!("acc verify checks a loss record; acc prove --seed N proves it again from scratch.");
    }
    Ok(true)
}

/// The win rate so far of a run that hasn't finished, by `acc batch`'s rule: the deals not decided yet count as lost for
/// the lower bound, and as won for the upper.
fn provisional(run: &Run) {
    let n = run.num("done");
    let won = run.won();
    let (lost, proving, undecided) = (run.num("lost"), run.num("proving"), run.num("undecided"));
    let proof_failed = run.num("proof_failed");
    // Deals the solver didn't win that went to no prover (with proofs off) are undecided too.
    let proved =
        lost + run.status["proofs"]["won"].as_u64().unwrap_or(0) + undecided + proof_failed + proving;
    let unproven = run.num("not_won_by_solver").saturating_sub(proved);
    println!(
        "{} isn't finished: it is {}, at {}. These figures are as of its last save{}.",
        run.name,
        state_text(run),
        run.done_text(),
        run.saved().map_or(String::new(), |s| format!(", {s}"))
    );
    if n == 0 {
        println!("No deals are done yet.");
        return;
    }
    let (lo, _) = wilson(won, n);
    let (_, hi) = wilson((won + undecided + proving + proof_failed + unproven).min(n), n);
    println!("  won          {} of {} deals", thousands(won), thousands(n));
    println!(
        "  win rate     {:.6}% so far; with 95% confidence between {:.6}% and {:.6}%",
        100.0 * won as f64 / n as f64,
        100.0 * lo,
        100.0 * hi
    );
    for (count, what, meaning) in [
        (lost, "lost", "proven impossible: two separate complete searches found no way to win them"),
        (proving, "proving", "not won by the solver, and being checked by a complete search now"),
        (undecided, "undecided", "the complete search reached its limit, so it is not known either way"),
        (unproven, "not won", "not won by the solver, and not proved either way (proofs were off)"),
        (
            run.num("unverified"),
            "UNVERIFIED",
            "a winning line failed the check by the rules: a bug worth reporting",
        ),
        (proof_failed, "PROOF FAILED", "the two complete searches disagreed: a bug worth reporting"),
    ] {
        if count > 0 {
            println!("  {what:<12} {}: {meaning}", thousands(count));
        }
    }
    println!(
        "The deals not decided yet count as lost for the lower figure and as won for the higher. The check that no two \
         deals are the same game is made at the end of the run."
    );
    println!("{}", next_text(run));
}

/// The seeds proven lost so far, from the run's log of events (`seed N: proven lost`).
fn proven_lost(run: &Run) -> Vec<u64> {
    let Ok(log) = std::fs::read_to_string(run.file("events.log")) else { return Vec::new() };
    let mut seeds: Vec<u64> = log
        .lines()
        .filter_map(|line| {
            let rest = &line[line.find("seed ")? + 5..];
            let (seed, what) = rest.split_once(": ")?;
            (what == "proven lost").then(|| seed.parse().ok()).flatten()
        })
        .collect();
    seeds.sort_unstable();
    seeds.dedup();
    seeds
}

/// Where the run's dossiers go: as its status file says, or for one from before it said, the default of the time.
fn dossiers(run: &Run) -> Option<PathBuf> {
    match &run.status["dossiers"] {
        serde_json::Value::String(dir) => Some(PathBuf::from(dir)),
        serde_json::Value::Null if run.status.get("dossiers").is_some() => None,
        _ => ["output/hard-deals", "hard-deals"]
            .iter()
            .map(PathBuf::from)
            .find(|d| Path::new(d).join("proofs").is_dir()),
    }
}
