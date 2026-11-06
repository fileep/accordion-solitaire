//! `acc batch`'s progress line and speed log. Once a second the line shows deals done and left, the rate and the
//! time to go (on a terminal it updates in place; otherwise a line a minute), and the log gets a CSV row of how fast
//! the run is going, to look back on later: thermal throttling, a stalled GPU, CPU workers falling behind it.

use crate::runner::Progress;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The progress line's rate is over the last this many seconds.
const WINDOW: usize = 10;
/// Width of the progress bar.
const BAR: usize = 24;
/// Without a terminal, a progress line this often (seconds).
const PLAIN_EVERY: f64 = 60.0;

pub struct Meter {
    total: usize,
    /// Where this session of the run started.
    first: Progress,
    start: Instant,
    /// The last few samples, oldest first: (seconds since the start, progress).
    recent: VecDeque<(f64, Progress)>,
    log: Option<(PathBuf, File)>,
    terminal: bool,
    /// Length of the last line printed in place, to blank what a shorter one leaves.
    width: usize,
    /// When the last line was printed without a terminal.
    printed: f64,
}

impl Meter {
    /// A meter for `total` deals, from where the run stands in `first`. With `log_dir`, the speed log goes in a new
    /// file there, headed by `about` (the run's settings).
    pub fn new(total: usize, first: Progress, log_dir: Option<&Path>, about: &str) -> Result<Meter, String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let log = match log_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
                let path = dir.join(format!("batch-{}Z.csv", utc_stamp(now)));
                let mut file =
                    File::create(&path).map_err(|e| format!("creating {}: {e}", path.display()))?;
                writeln!(file, "# {about}, started {}Z", utc_stamp(now))
                    .and_then(|()| {
                        writeln!(
                            file,
                            "unix_time,elapsed_s,done,left,deals_per_s,gpu_wins_per_s,gpu_mpositions_per_s,gpu_backlog"
                        )
                    })
                    .map_err(|e| format!("writing {}: {e}", path.display()))?;
                eprintln!("speed log: {}", path.display());
                Some((path, file))
            }
            None => None,
        };
        Ok(Meter {
            total,
            first,
            start: Instant::now(),
            recent: VecDeque::from([(0.0, first)]),
            log,
            terminal: std::io::stderr().is_terminal(),
            width: 0,
            printed: 0.0,
        })
    }

    /// Takes a sample: a log row, and the progress line when it is due.
    pub fn update(&mut self, now: &Progress) {
        let t = self.start.elapsed().as_secs_f64();
        let &(t0, last) = self.recent.back().expect("never empty");
        let dt = (t - t0).max(1e-3);
        let per_s = |a: u64, b: u64| a.saturating_sub(b) as f64 / dt;
        let row = format!(
            "{:.3},{t:.1},{},{},{:.0},{:.0},{:.1},{}",
            SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()),
            now.done,
            self.total - now.done,
            per_s(now.done as u64, last.done as u64),
            per_s(now.gpu_won, last.gpu_won),
            per_s(now.gpu_positions, last.gpu_positions) / 1e6,
            now.backlog
        );
        if let Some((path, file)) = &mut self.log
            && let Err(e) = writeln!(file, "{row}")
        {
            eprintln!("\nspeed log: writing {}: {e}; no more rows", path.display());
            self.log = None;
        }
        self.recent.push_back((t, *now));
        if self.recent.len() > WINDOW + 1 {
            self.recent.pop_front();
        }
        let finished = now.done == self.total;
        if self.terminal {
            let line = self.line(t, now.done, finished);
            eprint!("\r{line:<w$}", w = self.width);
            self.width = line.chars().count();
        } else if finished || t - self.printed >= PLAIN_EVERY {
            self.printed = t;
            eprintln!("{}", self.line(t, now.done, finished));
        }
    }

    fn line(&self, t: f64, done: usize, finished: bool) -> String {
        let &(t0, first) = self.recent.front().expect("never empty");
        let rate = done.saturating_sub(first.done) as f64 / (t - t0).max(1e-3);
        let filled = (BAR * done / self.total.max(1)).min(BAR);
        let left = self.total - done;
        let to_go = if finished {
            format!("finished in {}", duration(t))
        } else if rate > 0.0 {
            format!("{} to go", duration(left as f64 / rate))
        } else {
            "time to go unknown".into()
        };
        format!(
            "  {}{} {:5.1}%  {} done · {} left · {} deals/s (avg {}) · {to_go}",
            "█".repeat(filled),
            "░".repeat(BAR - filled),
            100.0 * done as f64 / self.total.max(1) as f64,
            thousands(done as u64),
            thousands(left as u64),
            thousands(rate.round() as u64),
            thousands((done.saturating_sub(self.first.done) as f64 / t.max(1e-3)).round() as u64),
        )
    }
}

/// `n` with commas between thousands.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Seconds as `2h 05m`, `6m 14s` or `42s`.
fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s / 60 % 60),
    }
}

/// Seconds since 1970 as a UTC date and time, `YYYYMMDD-HHMMSS` (Howard Hinnant's days-to-civil).
fn utc_stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_times_read_well() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(12_345_678), "12,345,678");
        assert_eq!(duration(42.4), "42s");
        assert_eq!(duration(374.0), "6m 14s");
        assert_eq!(duration(7_500.0), "2h 05m");
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_700_000_000), "20231114-221320");
        assert_eq!(utc_stamp(1_709_164_800), "20240229-000000");
    }
}
