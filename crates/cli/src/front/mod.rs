//! The front end: `acc run`, `resume`, `status`, `pause`, `results`, `doctor`, `show` and `settings`, for using the program
//! without knowing its engine. Every run is an `acc batch` with arguments made here, in a process of its own (batch ends its
//! process for some ways of stopping), and all that is said about a run comes from the files it keeps beside its checkpoint:
//! its status file, its log of events and its report. The checkpoint itself is only ever read when its run isn't going, as
//! a reader holding it open could make a save fail.

pub mod doctor;
pub mod machine;
pub mod run;
pub mod show;
pub mod status;

use crate::progress::thousands;
use serde_json::Value;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The folder of the runs started here: `acc batch`'s default for its checkpoints.
pub const FOLDER: &str = "checkpoints";
/// A status file this old, from an acc that takes no lock, means the run has stopped going through its deals: it is
/// written at every save while it does.
const STALE_AFTER_S: u64 = 5 * 60;

/// A run started in this folder, as its files show it.
pub struct Run {
    /// `batch-0-1000000`: what its files are named after, and how commands name it.
    pub name: String,
    /// Its status file, or `Null` before there is one (in a run's first minute).
    pub status: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// Going, in this window or another.
    Live,
    Paused,
    /// Its status says it was going, but it isn't: a crash, a power cut, a window closed. Seconds since it was last seen.
    Stopped(Option<u64>),
    /// Started by an older acc, which takes no lock, and last seen in its last checks, in which it can go for hours without
    /// a word: it may be going still, or have stopped. Seconds since it was last seen.
    Unsure(Option<u64>),
    Finished,
    /// Neither a checkpoint nor a report: nothing to carry on with or show.
    Gone,
}

impl Run {
    pub fn new(name: &str) -> Run {
        let status = std::fs::read_to_string(file(name, "status.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        Run { name: name.to_string(), status }
    }

    /// One of its files: `checkpoints/<name>.<ending>`.
    pub fn file(&self, ending: &str) -> PathBuf {
        file(&self.name, ending)
    }

    /// A count from the status file, 0 if it hasn't one.
    pub fn num(&self, key: &str) -> u64 {
        self.status[key].as_u64().unwrap_or(0)
    }

    pub fn word(&self) -> &str {
        self.status["state"].as_str().unwrap_or("")
    }

    /// Seconds since the status file was written.
    pub fn age_s(&self) -> Option<u64> {
        let updated = parse_utc(self.status["updated"].as_str()?)?;
        Some(now_s().saturating_sub(updated))
    }

    pub fn state(&self) -> State {
        if locked(&self.name) {
            return State::Live;
        }
        let has_checkpoint = self.file("json").exists();
        let age = self.age_s();
        match self.word() {
            "finished" => State::Finished,
            "" if self.status.is_null() && self.file("report.txt").exists() => State::Finished,
            // An acc that takes the lock says how often it saves, and for one the lock alone tells.
            w if has_checkpoint && self.by_older_acc() => self.older_state(w, age),
            w if w.starts_with("paused") && has_checkpoint => State::Paused,
            _ if has_checkpoint => State::Stopped(age),
            _ => State::Gone,
        }
    }

    /// Whether its status file was written by an older acc, which takes no lock, so that a second process could take up
    /// its checkpoint while it goes: what is said of it, and what is done with it, can go only by the times in its files.
    pub fn by_older_acc(&self) -> bool {
        !self.status.is_null() && self.status.get("save_every_s").is_none()
    }

    /// The state of a run of an older acc. It writes its status file at every save while it goes through its deals; in
    /// its last checks only when something has changed, which can be hours apart; and when it carries on, not before its
    /// first save, a minute in, though its log of events says at once that it has started.
    fn older_state(&self, word: &str, age: Option<u64>) -> State {
        let fresh = |age: Option<u64>| age.is_some_and(|a| a < STALE_AFTER_S);
        match word {
            "running" | "finishing" if fresh(age) => State::Live,
            "finishing" => State::Unsure(age),
            w if w.starts_with("paused") => match self.started_since() {
                Some(since) if age.is_none_or(|age| since < age) => {
                    if fresh(Some(since)) {
                        State::Live
                    } else {
                        State::Stopped(Some(since))
                    }
                }
                _ => State::Paused,
            },
            _ => State::Stopped(age),
        }
    }

    /// Seconds since the last session of the run started, as its log of events says. A session that started after the
    /// status file was last written has not saved yet: nearly every way a session ends, but a crash, writes it.
    fn started_since(&self) -> Option<u64> {
        let mut log = File::open(self.file("events.log")).ok()?;
        let length = log.metadata().ok()?.len();
        log.seek(SeekFrom::Start(length.saturating_sub(16 << 10))).ok()?;
        let mut tail = Vec::new();
        log.read_to_end(&mut tail).ok()?;
        let tail = String::from_utf8_lossy(&tail);
        let started = tail.lines().rev().find_map(|line| {
            let (when, what) = line.split_once(' ')?;
            (what.starts_with("session ") && what.contains(" started at ")).then_some(when)
        })?;
        parse_utc(started).map(|t| now_s().saturating_sub(t))
    }

    /// The deals won so far: the status file's count, or for one written before it had that, worked out as `acc batch`
    /// works it out at the end.
    pub fn won(&self) -> u64 {
        match self.status["won"].as_u64() {
            Some(won) => won,
            None => (self.num("done") + self.status["proofs"]["won"].as_u64().unwrap_or(0))
                .saturating_sub(self.num("not_won_by_solver") + self.num("failed_verification")),
        }
    }

    /// Its first seed and its count of deals: from the status file, or before there is one, from a name as `acc batch`
    /// gives it (`batch-<from>-<count>`).
    pub fn range(&self) -> Option<(u64, u64)> {
        if !self.status.is_null() {
            return Some((self.num("from"), self.num("count")));
        }
        let (from, count) = self.name.strip_prefix("batch-")?.split_once('-')?;
        Some((from.parse().ok()?, count.parse().ok()?))
    }

    /// Its seeds, as `seeds 0 to 999,999`.
    pub fn seeds(&self) -> String {
        match self.range() {
            Some((from, count)) if count > 0 => {
                format!("seeds {} to {}", thousands(from), thousands(from.saturating_add(count - 1)))
            }
            _ => "seeds unknown".into(),
        }
    }

    /// When the status file was written, as `2026-11-09 13:00:00 UTC (5 minutes ago)`.
    pub fn saved(&self) -> Option<String> {
        let updated = self.status["updated"].as_str()?;
        let age = self.age_s()?;
        Some(format!("{} UTC ({})", updated.trim_end_matches('Z').replacen('T', " ", 1), ago(age)))
    }

    /// Where it stands, in a few words.
    pub fn done_text(&self) -> String {
        let (done, count) = (self.num("done"), self.num("count"));
        if self.status.is_null() {
            return "just started: nothing is saved in its first minute".into();
        }
        format!(
            "{} of {} deals ({}%)",
            thousands(done),
            thousands(count),
            percent(100.0 * done as f64 / count.max(1) as f64)
        )
    }
}

fn file(name: &str, ending: &str) -> PathBuf {
    Path::new(FOLDER).join(format!("{name}.{ending}"))
}

/// A percentage to one decimal, but never 100.0 for what isn't all, nor 0.0 for what is some.
fn percent(p: f64) -> String {
    match p {
        p if p > 99.9 && p < 100.0 => "99.9".into(),
        p if p > 0.0 && p < 0.1 => "under 0.1".into(),
        p => format!("{p:.1}"),
    }
}

/// Whether a process holds the run's lock, which `acc batch` takes for as long as it has the checkpoint. A lock file that
/// nobody holds was left by a run that ended without tidying up, and means nothing.
pub fn locked(name: &str) -> bool {
    let Ok(lock) = OpenOptions::new().read(true).open(file(name, "lock")) else { return false };
    matches!(lock.try_lock(), Err(TryLockError::WouldBlock))
}

/// Every run that has left files in `checkpoints/` here, by name.
pub fn runs_here() -> Vec<Run> {
    let Ok(entries) = std::fs::read_dir(FOLDER) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let name = [".status.json", ".report.txt", ".lock"]
                .iter()
                .find_map(|ending| file.strip_suffix(ending))
                .or_else(|| file.strip_suffix(".json"))?
                .to_string();
            // The files of a run started again with acc run --new are kept, but are not a run here any more.
            if is_kept(&name) {
                return None;
            }
            // A lock file says nothing unless it is held: in a run's first minute, before there is a status file.
            (!file.ends_with(".lock") || locked(&name)).then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| Run::new(n)).collect()
}

/// Whether a name is that of a run's files kept by `acc run --new`: `batch-0-1000.old-1`.
pub fn is_kept(name: &str) -> bool {
    name.rsplit_once(".old-").is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The run that `name` names among `runs`: its name (`batch-0-1000000`), that without `batch-`, or one of its files.
pub fn find<'a>(runs: &'a [Run], name: &str) -> Result<&'a Run, String> {
    let given = Path::new(name).file_name().map_or(name.to_string(), |n| n.to_string_lossy().into_owned());
    let given = [".status.json", ".report.txt", ".events.log", ".json", ".lock", ".pause"]
        .iter()
        .find_map(|ending| given.strip_suffix(ending))
        .unwrap_or(&given)
        .to_string();
    runs.iter().find(|r| r.name == given || r.name == format!("batch-{given}")).ok_or_else(|| {
        if runs.is_empty() {
            format!("there is no run named {name} here: {}", nothing_here())
        } else {
            let names: Vec<&str> = runs.iter().map(|r| r.name.as_str()).collect();
            format!("there is no run named {name} here; there are: {}", names.join(", "))
        }
    })
}

/// What to say when there are no runs here.
pub fn nothing_here() -> String {
    let here = std::env::current_dir().map_or_else(|_| ".".into(), |d| d.display().to_string());
    format!(
        "no runs found in {}. A run keeps its files where it was started, so acc must be run from that same folder; and \
         a run that took under a minute keeps none: what it printed is all there is",
        Path::new(&here).join(FOLDER).display()
    )
}

/// Seconds since 1970.
pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DDTHH:MM:SSZ` (as [`crate::dossier::now_utc`] writes it) as seconds since 1970 (H. Hinnant's days from civil).
pub fn parse_utc(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let numbers =
        |s: &str, sep: char| s.split(sep).map(|n| n.parse::<i64>().ok()).collect::<Option<Vec<_>>>();
    let (date, time) = (numbers(date, '-')?, numbers(time, ':')?);
    let [y, m, d]: [i64; 3] = date.as_slice().try_into().ok()?;
    let [hh, mm, ss]: [i64; 3] = time.as_slice().try_into().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A time ago, in words: `5 minutes ago`.
pub fn ago(secs: u64) -> String {
    let (n, unit) = match secs {
        0..120 => (secs, "second"),
        120..7200 => (secs / 60, "minute"),
        7200..172_800 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// A count the way it can be typed back, as short as it goes exactly: `1m`, `2.5b`, `12345`.
pub fn short(n: u64) -> String {
    for (unit, size) in [("t", 1_000_000_000_000u64), ("b", 1_000_000_000), ("m", 1_000_000), ("k", 1_000)] {
        if n >= size {
            let whole = n / size;
            let rest = n % size;
            if rest == 0 {
                return format!("{whole}{unit}");
            }
            // One or two decimals, if that is exact.
            for places in [1u32, 2] {
                let step = size / 10u64.pow(places);
                if rest.is_multiple_of(step) {
                    return format!("{whole}.{:0w$}{unit}", rest / step, w = places as usize);
                }
            }
            break;
        }
    }
    n.to_string()
}

/// The figures of a report that its one plain line needs.
#[derive(Debug, Default, PartialEq)]
pub struct Figures {
    pub deals: u64,
    pub won: u64,
    pub not_won: u64,
    /// Proven lost and undecided, if there were proofs.
    pub proofs: Option<(u64, u64)>,
    pub proofs_failed: bool,
    pub failed: u64,
}

/// The figures in `acc batch`'s report, from its lines' labels.
pub fn figures(report: &str) -> Option<Figures> {
    let mut f = Figures::default();
    let mut seen = false;
    for line in report.lines() {
        let (label, rest) = line.split_at(17.min(line.len()));
        let first = || rest.split_whitespace().next().and_then(|w| w.parse::<u64>().ok());
        match label.trim() {
            "deals" => {
                f.deals = first()?;
                seen = true;
            }
            "won" => f.won = first()?,
            "not won" => f.not_won = first()?,
            "failed to verify" => f.failed = line["failed to verify".len()..].trim().parse().ok()?,
            "proofs" => {
                let words: Vec<&str> = rest.split_whitespace().collect();
                let lost = words.first()?.parse().ok()?;
                let undecided = words.iter().position(|w| *w == "undecided")?.checked_sub(1)?;
                f.proofs = Some((lost, words[undecided].parse().ok()?));
                f.proofs_failed = rest.contains("FAILED");
            }
            _ => {}
        }
    }
    seen.then_some(f)
}

/// `In short: 999,999 of 1,000,000 deals won; 1 proven impossible.`
pub fn in_short(f: &Figures) -> String {
    let mut text = format!("In short: {} of {} deals won", thousands(f.won), thousands(f.deals));
    match f.proofs {
        Some((lost, undecided)) => {
            text +=
                &format!("; {} proven impossible", if lost == 0 { "none".into() } else { thousands(lost) });
            if undecided > 0 {
                text += &format!(", {} not decided either way", thousands(undecided));
            }
        }
        None if f.not_won > 0 => {
            text +=
                &format!("; {} not won, and not proved either way (proofs were off)", thousands(f.not_won));
        }
        None => {}
    }
    if f.failed > 0 || f.proofs_failed {
        text += "; some checks FAILED, which would be a bug worth reporting (see the report above)";
    }
    text + "."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_back_as_written() {
        let now = now_s();
        let written = crate::dossier::now_utc();
        let read = parse_utc(&written).unwrap();
        assert!(read.abs_diff(now) <= 2, "{written}: {read} against {now}");
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T12:30:15Z"), Some(951_913_815));
        assert_eq!(parse_utc("2026-02-28T23:59:59Z"), Some(1_772_323_199));
        for bad in ["", "2026-11-09", "2026-11-09T13:00:00", "2026-13-01T00:00:00Z", "x-1-1T0:0:0Z"] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
        assert_eq!(ago(59), "59 seconds ago");
        assert_eq!(ago(300), "5 minutes ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }

    #[test]
    fn counts_are_written_as_they_can_be_typed() {
        for (n, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1k"),
            (10_000, "10k"),
            (1_000_000, "1m"),
            (2_500_000_000, "2.5b"),
            (1_250_000, "1.25m"),
            (1_234_567, "1234567"),
            (100_000_000_000_000, "100t"),
            (22_000_000_000, "22b"),
        ] {
            assert_eq!(short(n), text);
            assert_eq!(crate::numbers::parse_count(text), Ok(n), "{text} reads back");
        }
    }

    #[test]
    fn small_parts_are_not_shown_as_more_and_kept_runs_are_known() {
        assert_eq!(percent(100.0 * 3_556.0 / 1e14), "under 0.1");
        assert_eq!(
            (percent(0.0), percent(0.1), percent(50.0), percent(99.95)),
            ("0.0".into(), "0.1".into(), "50.0".into(), "99.9".into())
        );
        assert!(is_kept("batch-0-1000.old-1") && is_kept("batch-0-1000.old-12"));
        assert!(!is_kept("batch-0-1000") && !is_kept("my.old-run") && !is_kept("x.old-"));
    }

    const REPORT: &str = "\
deals            1000000 (seeds 0..999999)
won              999998 (every line replayed: by the reference rules, or a GPU line by the quick check and 1 in 256 by the reference engine too); by stage: ladder 999000, thorough 998
not won          2 by the solver (2 exhausted, 0 hit the limit of 50000000 positions)
proofs           1 proven lost, 0 won, 1 undecided (two exhaustive runs of up to 10000000000 positions each)
failed to verify 0
win rate         99.9998000000%   95% CI [x, y]
";

    #[test]
    fn the_report_comes_down_to_one_plain_line() {
        let f = figures(REPORT).unwrap();
        assert_eq!(
            f,
            Figures {
                deals: 1_000_000,
                won: 999_998,
                not_won: 2,
                proofs: Some((1, 1)),
                ..Figures::default()
            }
        );
        assert_eq!(
            in_short(&f),
            "In short: 999,998 of 1,000,000 deals won; 1 proven impossible, 1 not decided either way."
        );
        let no_proofs: String =
            REPORT.lines().filter(|l| !l.starts_with("proofs")).map(|l| l.to_string() + "\n").collect();
        assert!(in_short(&figures(&no_proofs).unwrap()).contains("2 not won, and not proved either way"));
        let failed = REPORT.replace("failed to verify 0", "failed to verify 3");
        assert!(in_short(&figures(&failed).unwrap()).contains("FAILED"));
        let all = "deals            10 (seeds 0..9)\nwon              10 (every one)\nnot won          0 by\nproofs           0 proven lost, 0 won, 0 undecided (x)\nfailed to verify 0\n";
        assert_eq!(in_short(&figures(all).unwrap()), "In short: 10 of 10 deals won; none proven impossible.");
        assert_eq!(figures("nothing like a report"), None);
    }
}
