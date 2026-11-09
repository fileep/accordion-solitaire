//! `acc run` and `acc resume`: an `acc batch` with arguments made from a few plain ones, seen through to its end. The run
//! is started again when it stops for a reason a new process may get past (the GPU failing, a crash), as
//! `tools/long-run.ps1` does, and paused while the battery is low and started again once it has charged, as the battery
//! guard of the long runs here did. Ctrl+C reaches the run, which pauses, and this process stays to say so.

use super::machine::{self, Guard};
use super::{FOLDER, Run, State, figures, in_short, short};
use crate::numbers::parse_count;
use crate::progress::thousands;
use clap::Args;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUN_EXAMPLES: &str = "\
Examples:
  acc run --quick                    10,000 deals, to try it out
  acc run                            a million deals
  acc run --deals 50m --gentle       50 million, leaving the computer usable meanwhile
  acc run --overnight                as many as 8 hours allow; the same command carries on the next night
  acc run --from 1m --deals 1m       the next million deals after the first
  acc run --deals 1m --dry-run       show the acc batch command this stands for, and stop";

const RESUME_EXAMPLES: &str = "\
Examples:
  acc resume                         carry on with the paused run here
  acc resume batch-0-1000000         carry on with this one, when there are several
  acc resume --hours 2 --gentle      for two hours, leaving the computer usable";

/// Deals in a run with --hours and no --deals: more than any run gets through, so that the hours decide.
const OPEN_ENDED: u64 = 100_000_000_000_000;
/// A run's tries in a row without getting further, before giving up on it.
const TRIES: u32 = 6;
const WAIT_BEFORE_TRYING_AGAIN: Duration = Duration::from_secs(60);
/// How long a pause for the battery may take, under [`machine::CRITICAL`] percent, before the run is stopped at once.
const PAUSE_AT_MOST: Duration = Duration::from_secs(60);
/// How large a winning line is in `--save-wins`, about (`acc batch --out`).
const RECORD_BYTES: u64 = 350;
/// `acc batch`'s defaults for its memory, in GB, which `acc run` passes only when the computer has less.
const GAMES_MEMORY: u64 = 16;
const PROVE_MEMORY: u64 = 8;

/// What `acc run` and `acc resume` share: how much of the computer to use, and what to do when the run stops.
#[derive(Args, Clone)]
pub struct MachineArgs {
    /// Threads to use (default: every one; with --gentle, half).
    #[arg(long)]
    threads: Option<usize>,
    /// Leave the computer usable: half the threads and a lower priority, and a run that starts here leaves the GPU alone
    /// unless --gpu asks for it.
    #[arg(long)]
    gentle: bool,
    /// Don't start the run again by itself if it stops with an error.
    #[arg(long)]
    once: bool,
    /// Don't pause the run when the battery runs low.
    #[arg(long)]
    no_battery_guard: bool,
    /// Pause the run when the battery falls under this percentage.
    #[arg(long, default_value_t = 30, value_name = "PCT", value_parser = clap::value_parser!(u8).range(1..=99))]
    battery_pause: u8,
    /// After a pause for the battery, carry on once the charger is in and the battery is back to this percentage.
    #[arg(long, default_value_t = 60, value_name = "PCT", value_parser = clap::value_parser!(u8).range(1..=100))]
    battery_resume: u8,
}

#[derive(Args)]
#[command(after_help = RUN_EXAMPLES)]
pub struct RunArgs {
    /// How many deals to play. 5000, 2m, 1.5b and 100t all work. Default: a million, or with --hours as many as the hours
    /// allow.
    #[arg(long, value_parser = parse_count, conflicts_with = "quick")]
    deals: Option<u64>,
    /// Stop after this many hours: the run pauses, and carries on with the same command (or acc resume).
    #[arg(long, conflicts_with = "overnight")]
    hours: Option<f64>,
    /// The same as --hours 8.
    #[arg(long)]
    overnight: bool,
    /// A quick try: 10,000 deals.
    #[arg(long)]
    quick: bool,
    /// The first deal's number. Deals are numbered from 0; runs that start where the last one ended never repeat a deal.
    #[arg(long, default_value_t = 0, value_parser = parse_count)]
    from: u64,
    /// Use an NVIDIA card: auto (if there is one that works), on, or off.
    #[arg(long, value_parser = ["auto", "on", "off"])]
    gpu: Option<String>,
    /// Save every winning line in this file, to check again later with acc verify (about 350 bytes a deal).
    #[arg(long, value_name = "FILE")]
    save_wins: Option<PathBuf>,
    /// Solver settings from a file (acc settings writes one to edit).
    #[arg(long, value_name = "FILE")]
    settings: Option<PathBuf>,
    /// GPU settings from a file (acc settings --gpu writes one to edit).
    #[arg(long, value_name = "FILE")]
    gpu_settings: Option<PathBuf>,
    /// Don't try to prove that the deals not won can't be won.
    #[arg(long)]
    no_proofs: bool,
    /// Start these deals again, though a run of them is paused or finished here (its files are kept, renamed .old-1).
    #[arg(long)]
    new: bool,
    /// Print the acc batch command this stands for, and stop.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    machine: MachineArgs,
}

#[derive(Args)]
#[command(after_help = RESUME_EXAMPLES)]
pub struct ResumeArgs {
    /// Which run, as acc status lists them; needed only when several are paused here.
    name: Option<String>,
    /// Stop after this many hours: the run pauses again.
    #[arg(long, conflicts_with = "overnight")]
    hours: Option<f64>,
    /// The same as --hours 8.
    #[arg(long)]
    overnight: bool,
    /// Print the acc batch command this stands for, and stop.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    machine: MachineArgs,
}

/// A run to see through.
struct Plan {
    name: String,
    from: u64,
    count: u64,
    /// `acc batch`'s arguments to start it, or none if it can only carry on.
    start: Option<Vec<String>>,
    /// Those for each time it carries on, after `--resume <checkpoint>`.
    session: Vec<String>,
    hours: Option<f64>,
    save_wins: Option<PathBuf>,
}

impl Plan {
    fn checkpoint(&self) -> String {
        format!("{FOLDER}/{}.json", self.name)
    }

    /// The arguments for the next session, given the hours left of the run's.
    fn args(&self, hours_left: Option<f64>) -> Vec<String> {
        let mut args = match &self.start {
            Some(start) if !Path::new(&self.checkpoint()).exists() => start.clone(),
            _ => [&["batch".into(), "--resume".into(), self.checkpoint()], &self.session[..]].concat(),
        };
        if let Some(hours) = hours_left {
            // Never 0, which would pause before a deal is done.
            args.extend(["--stop-after".into(), hours_text(hours)]);
        }
        args
    }
}

/// An argument list as it would be typed: `acc batch --count 1000000`.
fn typed(args: &[String]) -> String {
    let words: Vec<String> = args
        .iter()
        .map(|a| if a.is_empty() || a.contains([' ', '\'']) { format!("\"{a}\"") } else { a.clone() })
        .collect();
    format!("acc {}", words.join(" "))
}

/// The flags of `acc batch` that say how much of the computer to use, as `machine` and its memory ask.
fn session_flags(machine: &MachineArgs, resuming: bool) -> Vec<String> {
    let mut flags = Vec::new();
    let threads = machine.threads.or(machine.gentle.then(|| (crate::runner::all_threads() / 2).max(1)));
    if let Some(threads) = threads {
        flags.extend(["--threads".into(), threads.to_string()]);
    }
    // Memory to suit the computer, where it has less than acc batch assumes. A run carrying on keeps its proofs' memory.
    if let Some(gb) = machine::ram_bytes().map(|b| b >> 30) {
        let games = (gb / 2).clamp(1, GAMES_MEMORY);
        if games < GAMES_MEMORY {
            flags.extend(["--games-memory".into(), games.to_string()]);
        }
        let prove = (gb / 4).clamp(1, PROVE_MEMORY);
        if prove < PROVE_MEMORY && !resuming {
            flags.extend(["--prove-memory".into(), prove.to_string()]);
        }
    }
    flags
}

fn hours_of(hours: Option<f64>, overnight: bool) -> Result<Option<f64>, String> {
    let hours = if overnight { Some(8.0) } else { hours };
    match hours {
        Some(h) if !(h.is_finite() && h > 0.0 && h < 1e6) => {
            Err(format!("--hours must be a number of hours above 0, not {h}"))
        }
        _ => Ok(hours),
    }
}

fn check_battery_marks(machine: &MachineArgs) -> Result<(), String> {
    if machine.battery_pause >= machine.battery_resume {
        return Err(format!(
            "--battery-resume ({}%) must be above --battery-pause ({}%)",
            machine.battery_resume, machine.battery_pause
        ));
    }
    Ok(())
}

/// Refuses to carry on with a run that started in another folder, whose paths are relative to that one. Only for a run
/// that isn't going: it reads the checkpoint.
fn started_here(run: &Run) -> Result<(), String> {
    let Ok(text) = std::fs::read_to_string(run.file("json")) else { return Ok(()) };
    let Some(dir) = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|c| c["dir"].as_str().map(PathBuf::from))
    else {
        return Ok(());
    };
    let here = std::env::current_dir().and_then(std::fs::canonicalize).ok();
    if dir.as_os_str().is_empty() || std::fs::canonicalize(&dir).ok() == here {
        return Ok(());
    }
    Err(format!(
        "{} started in {}, and can only carry on from there: go to that folder (cd \"{}\") and run this again{}",
        run.name,
        dir.display(),
        dir.display(),
        if dir.exists() { "" } else { ". It isn't there any more: if it was moved or renamed, put it back" }
    ))
}

/// Keeps the files of a run that is starting again: `batch-0-1000.json` becomes `batch-0-1000.old-1.json`, and so on.
fn keep_old(name: &str) -> Result<String, String> {
    let files: Vec<String> = std::fs::read_dir(FOLDER)
        .map_err(|e| format!("reading {FOLDER}: {e}"))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|f| f.starts_with(&format!("{name}.")) && !f.starts_with(&format!("{name}.old-")))
        .collect();
    let taken = |n: u32| {
        std::fs::read_dir(FOLDER)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}.old-{n}.")))
    };
    let n = (1..).find(|&n| !taken(n)).expect("a free number");
    for f in &files {
        let to = format!("{name}.old-{n}.{}", &f[name.len() + 1..]);
        std::fs::rename(Path::new(FOLDER).join(f), Path::new(FOLDER).join(&to))
            .map_err(|e| format!("renaming {FOLDER}/{f} to {to}: {e}"))?;
    }
    Ok(format!("{FOLDER}/{name}.old-{n}.*"))
}

pub fn run(args: &RunArgs) -> Result<u8, String> {
    check_battery_marks(&args.machine)?;
    let hours = hours_of(args.hours, args.overnight)?;
    let count = match (args.deals, args.quick, hours) {
        (Some(n), ..) => n,
        (None, true, _) => 10_000,
        (None, false, Some(_)) => OPEN_ENDED,
        (None, false, None) => 1_000_000,
    };
    if count == 0 {
        return Err("--deals must be at least 1".into());
    }
    if args.from.checked_add(count - 1).is_none() {
        return Err(format!(
            "--from {} with {} deals goes past the last deal there is, number {}",
            args.from,
            count,
            u64::MAX
        ));
    }
    if hours.is_some() && args.deals.is_none() && !args.quick && args.save_wins.is_some() {
        return Err(format!(
            "--save-wins can't go with --hours alone: the file grows by about {RECORD_BYTES} bytes a deal for as long as the run goes, and could fill the disk. Say how many deals with --deals"
        ));
    }
    let name = format!("batch-{}-{count}", args.from);
    let run = Run::new(&name);
    let exists = run.file("json").exists();
    let played = !exists && run.file("report.txt").exists();
    let state = run.state();
    if state == State::Live {
        return Err(format!(
            "this run is already going in another window: acc status shows how it is doing, and acc pause {name} pauses it"
        ));
    }
    let mut kept = None;
    if (exists || played) && args.new {
        if args.dry_run {
            eprintln!(
                "note: without --dry-run, the files of the earlier run of these deals would be kept, renamed .old-1"
            );
        } else {
            kept = Some(keep_old(&name)?);
        }
    } else if played {
        return Err(format!(
            "these deals have been played here already ({}): acc results {name} shows what they found. acc run --from {} plays the next ones; adding --new plays these again",
            run.file("report.txt").display(),
            short(args.from.saturating_add(count))
        ));
    }
    let resuming = exists && !args.new;
    let gpu = args.gpu.clone().or(args.machine.gentle.then(|| "off".to_string()));
    let start = if resuming {
        started_here(&run)?;
        let ignored: Vec<&str> = [
            (gpu.is_some(), "--gpu"),
            (args.settings.is_some(), "--settings"),
            (args.gpu_settings.is_some(), "--gpu-settings"),
            (args.no_proofs, "--no-proofs"),
            (args.save_wins.is_some(), "--save-wins"),
        ]
        .into_iter()
        .filter_map(|(given, flag)| given.then_some(flag))
        .collect();
        eprintln!(
            "A run of these deals is paused here, at {}: carrying on with it, with the settings it began with.",
            run.done_text()
        );
        if !ignored.is_empty() {
            eprintln!(
                "note: {} can't change partway through a run, so {} left as they were. To start again instead, add --new.",
                ignored.join(", "),
                if ignored.len() == 1 { "it is" } else { "they are" }
            );
        }
        None
    } else {
        let mut start: Vec<String> =
            ["batch", "--from", &args.from.to_string(), "--count", &count.to_string()]
                .map(String::from)
                .to_vec();
        start.extend(session_flags(&args.machine, false));
        if let Some(gpu) = &gpu {
            start.extend(["--gpu".into(), gpu.clone()]);
        }
        if let Some(file) = &args.save_wins {
            start.extend(["--out".into(), file.display().to_string()]);
        }
        if args.no_proofs {
            start.extend(["--prove".into(), "0".into()]);
        }
        if let Some(file) = &args.settings {
            start.extend(["--config".into(), file.display().to_string()]);
        }
        if let Some(file) = &args.gpu_settings {
            start.extend(["--gpu-config".into(), file.display().to_string()]);
        }
        Some(start)
    };
    let plan = Plan {
        name,
        from: args.from,
        count,
        start,
        session: session_flags(&args.machine, true),
        hours,
        save_wins: args.save_wins.clone(),
    };
    if let Some(kept) = kept {
        eprintln!("The earlier run of these deals is kept, as {kept}.");
    }
    see_through(&plan, &args.machine, args.dry_run)
}

pub fn resume(args: &ResumeArgs) -> Result<u8, String> {
    check_battery_marks(&args.machine)?;
    let hours = hours_of(args.hours, args.overnight)?;
    let runs = super::runs_here();
    let run = match &args.name {
        Some(name) => super::find(&runs, name)?,
        None => {
            let paused: Vec<&Run> =
                runs.iter().filter(|r| matches!(r.state(), State::Paused | State::Stopped(_))).collect();
            match paused.as_slice() {
                [run] => *run,
                [] if runs.iter().any(|r| r.state() == State::Live) => {
                    return Err(
                        "the run here is already going, and nothing else is paused: acc status shows it"
                            .into(),
                    );
                }
                [] => {
                    return Err(if runs.is_empty() {
                        super::nothing_here()
                    } else {
                        "nothing here is paused: acc status lists the runs here".into()
                    });
                }
                several => {
                    let names: Vec<String> =
                        several.iter().map(|r| format!("  {:<28} {}", r.name, r.done_text())).collect();
                    return Err(format!(
                        "{} runs are paused here; say which, as in acc resume {}:\n{}",
                        several.len(),
                        several[0].name,
                        names.join("\n")
                    ));
                }
            }
        }
    };
    match run.state() {
        State::Live => {
            return Err(format!(
                "{} is already going in another window: acc status shows how it is doing",
                run.name
            ));
        }
        State::Finished => {
            return Err(format!("{} is finished: acc results {} shows what it found", run.name, run.name));
        }
        State::Gone => return Err(format!("{} has no checkpoint to carry on from", run.name)),
        State::Stopped(age) => eprintln!(
            "{} stopped unexpectedly{}: carrying on from its last checkpoint.",
            run.name,
            age.map_or(String::new(), |a| format!(" (last seen {})", super::ago(a)))
        ),
        State::Paused => eprintln!("Carrying on with {}, at {}.", run.name, run.done_text()),
    }
    started_here(run)?;
    if args.machine.gentle && run.status["hardware"].as_str().is_some_and(|h| h.contains("GPU")) {
        eprintln!(
            "note: this run uses the GPU, which can't be turned off partway through; --gentle only lowers the rest"
        );
    }
    let (from, count) = run.range().unwrap_or_default();
    let plan = Plan {
        name: run.name.clone(),
        from,
        count,
        start: None,
        session: session_flags(&args.machine, true),
        hours,
        save_wins: None,
    };
    see_through(&plan, &args.machine, args.dry_run)
}

fn stamp() -> String {
    let now = crate::dossier::now_utc();
    format!("{} {} UTC", &now[..10], &now[11..19])
}

/// Says the command, then runs it until it is finished, paused or stopped for good, starting it again when it stops for a
/// reason a new process may get past. The exit code is the run's last.
fn see_through(plan: &Plan, machine: &MachineArgs, dry_run: bool) -> Result<u8, String> {
    let first = plan.args(plan.hours);
    if let Some(file) = &plan.save_wins
        && plan.start.is_some()
    {
        eprintln!(
            "The winning lines will take about {} in {}.",
            size(plan.count.saturating_mul(RECORD_BYTES)),
            file.display()
        );
    }
    if dry_run {
        println!("{}", typed(&first));
        return Ok(0);
    }
    eprintln!("same as: {}", typed(&first));
    if machine.gentle && !machine::lower_priority() {
        eprintln!("note: the system refused a lower priority; the run goes at the normal one");
    }
    if plan.hours.is_some() {
        eprintln!("Leave this window open, keep the charger in and the lid open. Ctrl+C pauses safely.");
    }
    machine::keep_through_ctrl_c();
    let checkpoint = PathBuf::from(plan.checkpoint());
    let began = Instant::now();
    let hours_left = || plan.hours.map(|h| h - began.elapsed().as_secs_f64() / 3600.0);
    let mut guard =
        (!machine.no_battery_guard).then(|| Guard::new(machine.battery_pause, machine.battery_resume));
    let every = if machine::battery_faked() { Duration::from_secs(1) } else { Duration::from_secs(30) };
    let run = || Run::new(&plan.name);
    let mut failures = 0;
    let mut reached = run().num("done");
    let mut sessions = 0;
    loop {
        if machine::stop_asked() {
            eprintln!("{} stopped before starting the run again. acc resume carries on.", stamp());
            return Ok(0);
        }
        if hours_left().is_some_and(|h| h <= 0.0) {
            return Ok(paused_text(plan, true));
        }
        // A battery that is low already waits for the charger, as after a pause.
        if let Some(g) = guard.as_mut()
            && let Some(why) = g.pause_for(machine::battery())
        {
            eprintln!(
                "{} {why}: waiting for the charger and {}% before {} the run",
                stamp(),
                g.resume_from,
                if sessions == 0 { "starting" } else { "carrying on with" }
            );
            match wait_for_charge(g, every, &hours_left) {
                Waited::Charged => continue,
                Waited::HoursUp => return Ok(paused_text(plan, true)),
                Waited::Stopped => return Ok(0),
            }
        }
        let args = plan.args(hours_left());
        if sessions > 0 {
            eprintln!("{} carrying on: {}", stamp(), typed(&args));
        }
        sessions += 1;
        let ended = session(&args, guard.as_mut(), &checkpoint.with_extension("pause"), every)?;
        let exists = checkpoint.exists();
        // However it ended, a run whose checkpoint has given way to its report is over, and must not start again.
        if !exists && figures(&ended.stdout).is_some() {
            return Ok(finished_text(plan, &ended.stdout, u8::from(ended.code != Some(0))));
        }
        if ended.for_battery && exists && !machine::stop_asked() {
            let g = guard.as_mut().expect("a pause for the battery comes from the guard");
            eprintln!(
                "{} paused for the battery at {} deals; it carries on once the charger is in and the battery is at {}%",
                stamp(),
                thousands(run().num("done")),
                g.resume_from
            );
            match wait_for_charge(g, every, &hours_left) {
                Waited::Charged => {
                    eprintln!("{} battery {}: carrying on", stamp(), machine::describe(machine::battery()));
                    continue;
                }
                Waited::HoursUp => return Ok(paused_text(plan, true)),
                Waited::Stopped => return Ok(0),
            }
        }
        let hours_up = hours_left().is_some_and(|h| h <= 1.0 / 3600.0);
        match ended.code {
            Some(0) if exists => return Ok(paused_text(plan, hours_up && !machine::stop_asked())),
            Some(0) => return Ok(finished_text(plan, &ended.stdout, 0)),
            Some(74) => {
                eprintln!(
                    "{} stopped: the disk is nearly full. Make room, then acc resume carries on; nothing is lost.",
                    stamp()
                );
                return Ok(74);
            }
            Some(130) => {
                eprintln!(
                    "{} stopped at once. Its last checkpoint, a minute old at most, stands: acc resume carries on.",
                    stamp()
                );
                return Ok(130);
            }
            Some(code @ (1 | 2)) => {
                eprintln!(
                    "{} acc batch stopped with an error, above. {}",
                    stamp(),
                    if exists {
                        "Nothing is lost: once it is put right, acc resume carries on."
                    } else {
                        "Once it is put right, the same command starts the run."
                    }
                );
                return Ok(code as u8);
            }
            code => {
                let code = code.unwrap_or(1);
                // What carries it on from here: nothing is saved before its first checkpoint.
                let again = if exists {
                    "Nothing is lost: acc resume carries on."
                } else {
                    "Nothing was saved yet: the same command starts it again."
                };
                if machine::stop_asked() {
                    eprintln!("{} stopped, with exit code {code}. {again}", stamp());
                    return Ok(exit_code(code));
                }
                let now = run().num("done");
                if now > reached {
                    failures = 0;
                }
                reached = now;
                failures += 1;
                eprintln!(
                    "{} acc batch stopped with exit code {code}{} at {} deals: try {failures} of {TRIES} without getting further",
                    stamp(),
                    if code == 75 { " (the GPU failed; a new start usually gets it back)" } else { "" },
                    thousands(now)
                );
                if machine.once {
                    eprintln!("Not starting it again (--once). {again}");
                    return Ok(exit_code(code));
                }
                if failures >= TRIES {
                    eprintln!(
                        "Giving up: it isn't getting any further. {again} {} says what happened.",
                        checkpoint.with_extension("events.log").display()
                    );
                    return Ok(exit_code(code));
                }
                eprintln!(
                    "Starting it again in {} seconds (Ctrl+C to stop here).",
                    WAIT_BEFORE_TRYING_AGAIN.as_secs()
                );
                let until = Instant::now() + WAIT_BEFORE_TRYING_AGAIN;
                while Instant::now() < until && !machine::stop_asked() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

/// Bytes as `350 MB` or `3.5 GB`.
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000_000_000 => format!("{} MB", bytes.div_ceil(1_000_000)),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

/// Hours as they would be typed: `8`, `0.5`.
fn hours_text(hours: f64) -> String {
    ((hours.max(1e-4) * 1e4).round() / 1e4).to_string()
}

/// An exit code as this process can give it back.
fn exit_code(code: i32) -> u8 {
    u8::try_from(code).ok().filter(|&c| c != 0).unwrap_or(1)
}

fn paused_text(plan: &Plan, hours_up: bool) -> u8 {
    let run = Run::new(&plan.name);
    let at = format!("{} of {} deals", thousands(run.num("done")), thousands(plan.count));
    if hours_up {
        let hours = plan.hours.unwrap_or_default();
        eprintln!(
            "{} The {} hour{} up: the run is paused at {at}. acc results shows where it stands; acc resume carries on.",
            stamp(),
            hours_text(hours),
            if hours == 1.0 { " is" } else { "s are" }
        );
    } else {
        eprintln!("{} Paused at {at}. acc resume carries on; acc results shows where it stands.", stamp());
    }
    0
}

fn finished_text(plan: &Plan, report: &str, code: u8) -> u8 {
    if let Some(f) = figures(report) {
        println!("\n{}", in_short(&f));
    }
    let report_file = Path::new(FOLDER).join(format!("{}.report.txt", plan.name));
    if report_file.exists() {
        println!(
            "The report is saved in {}, and acc results {} shows it again.",
            report_file.display(),
            plan.name
        );
    }
    if let Some(file) = &plan.save_wins {
        println!(
            "Every winning line is in {}: acc verify {} checks them all again.",
            file.display(),
            file.display()
        );
    }
    let next = plan.from.saturating_add(plan.count);
    if plan.count > 0 && next.checked_add(plan.count - 1).is_some() {
        println!(
            "Next: acc run --from {} --deals {} plays the next {} deals.",
            short(next),
            short(plan.count),
            thousands(plan.count)
        );
    }
    code
}

/// How a session of the run ended.
struct Ended {
    code: Option<i32>,
    /// What it printed on standard output: the report, when it finished.
    stdout: String,
    /// Whether the guard asked it to pause.
    for_battery: bool,
}

/// One `acc batch` process, from start to end, with the battery looked at every `every` while it goes.
fn session(
    args: &[String],
    mut guard: Option<&mut Guard>,
    pause_file: &Path,
    every: Duration,
) -> Result<Ended, String> {
    let exe = std::env::current_exe().map_err(|e| format!("finding acc itself: {e}"))?;
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting acc batch: {e}"))?;
    // Its report comes through here, to be said again in one line at the end.
    let stdout = child.stdout.take().expect("piped");
    let echo = std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}").and_then(|()| out.flush());
            text += &line;
            text.push('\n');
        }
        text
    });
    let mut next_look = Instant::now() + every;
    let (mut asked, mut asked_at, mut killed) = (false, None::<Instant>, false);
    let mut last_touch: Option<Instant> = None;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("waiting for acc batch: {e}"))? {
            break status;
        }
        // Ctrl+C reached the run too, unless it was sent to this process alone; and a run still starting up takes a pause
        // file for one left from before, and deletes it. So it is put there again, every second, while it is wanted.
        if (asked || machine::stop_asked())
            && last_touch.is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            let _ = std::fs::write(pause_file, "");
            last_touch = Some(Instant::now());
        }
        if let Some(g) = guard.as_deref_mut()
            && Instant::now() >= next_look
        {
            next_look = Instant::now() + every;
            let battery = machine::battery();
            match asked_at {
                None => {
                    if let Some(why) = g.pause_for(battery) {
                        eprintln!(
                            "\n{} {why}: pausing the run; it carries on once the charger is in and the battery is at {}%",
                            stamp(),
                            g.resume_from
                        );
                        asked = true;
                        asked_at = Some(Instant::now());
                    }
                }
                Some(at) if !killed && at.elapsed() >= PAUSE_AT_MOST && g.critical(battery) => {
                    eprintln!(
                        "\n{} the battery is at {} and the pause isn't done after {} s: stopping the run at once. Its last checkpoint, a minute old at most, stands.",
                        stamp(),
                        machine::describe(battery),
                        PAUSE_AT_MOST.as_secs()
                    );
                    let _ = child.kill();
                    killed = true;
                }
                Some(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = std::fs::remove_file(pause_file);
    let stdout = echo.join().unwrap_or_default();
    Ok(Ended { code: status.code(), stdout, for_battery: asked })
}

enum Waited {
    Charged,
    HoursUp,
    Stopped,
}

/// Waits until the battery may carry the run on, or the run's hours are up, or Ctrl+C.
fn wait_for_charge(guard: &Guard, every: Duration, hours_left: &dyn Fn() -> Option<f64>) -> Waited {
    let mut next_look = Instant::now() + every;
    let mut said = Instant::now();
    loop {
        if machine::stop_asked() {
            eprintln!("{} stopped waiting for the battery. acc resume carries on.", stamp());
            return Waited::Stopped;
        }
        if hours_left().is_some_and(|h| h <= 0.0) {
            return Waited::HoursUp;
        }
        if Instant::now() >= next_look {
            next_look = Instant::now() + every;
            let battery = machine::battery();
            if guard.may_resume(battery) {
                return Waited::Charged;
            }
            if said.elapsed() >= Duration::from_secs(600) {
                eprintln!("{} still waiting: battery {}", stamp(), machine::describe(battery));
                said = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_shown_as_they_would_be_typed() {
        let args: Vec<String> =
            ["batch", "--out", "my wins.jsonl", "--count", "5"].map(String::from).to_vec();
        assert_eq!(typed(&args), "acc batch --out \"my wins.jsonl\" --count 5");
    }

    #[test]
    fn hours_must_be_some() {
        assert_eq!(hours_of(None, true), Ok(Some(8.0)));
        assert_eq!(hours_of(Some(0.5), false), Ok(Some(0.5)));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(hours_of(Some(bad), false).is_err(), "{bad}");
        }
    }

    #[test]
    fn exit_codes_pass_through_or_become_1() {
        assert_eq!((exit_code(75), exit_code(-1073741819), exit_code(0), exit_code(300)), (75, 1, 1, 1));
    }
}
