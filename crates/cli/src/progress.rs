//! `acc batch`'s progress line and speed log. Once a second the line shows deals done and left, the rate, the time to
//! go and how many deals have failed so far (on a terminal it updates in place; otherwise a line a minute), and the log
//! gets a CSV row of how fast the run is going, to look back on later: thermal throttling, a stalled GPU, CPU workers
//! falling behind it.

use crate::runner::Progress;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The progress line's rate is over the last this many seconds.
const WINDOW: usize = 10;
/// Width of the progress bar.
const BAR: usize = 16;
/// Without a terminal, a progress line this often (seconds).
const PLAIN_EVERY: f64 = 60.0;
/// From a run this long the line's counts are short (`50.0T`): with every digit it no longer fits on a terminal, and one that
/// wraps can't be rewritten in place.
const SHORT_COUNTS_FROM: usize = 1_000_000_000_000;
/// From this long, times are in days.
const DAYS_FROM: u64 = 48 * 3600;

/// What has gone wrong, or not been won, so far: the figures behind the progress line's "lost" and its flags. They count
/// the deals handed on in seed order (the ones a checkpoint counts) and what the prover has finished of those, so a deal
/// the solver didn't win shows as "proving" until its proof is done, as "lost" once it is proven lost, and never as won.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Trouble {
    /// Deals the solver didn't win. Each goes to the prover, if there is one.
    pub not_won: u64,
    /// Proven lost: two exhaustive runs agreed.
    pub lost: u64,
    /// Sent to the prover, not yet decided.
    pub proving: u64,
    /// The proof hit its limit, so the deal is neither won nor proven lost.
    pub undecided: u64,
    /// Winning lines that failed verification (a bug), on the GPU or the CPU.
    pub unverified: u64,
    /// Proofs that failed (a bug).
    pub proof_failed: u64,
    /// Whether there is a prover (`--prove` above 0). Without, no deal can be proven lost.
    pub proving_on: bool,
}

impl Trouble {
    /// The line's part about them: `1,040 lost`, then whatever isn't zero of the rest.
    pub fn text(&self) -> String {
        let mut out = if self.proving_on {
            format!("{} lost", thousands(self.lost))
        } else {
            format!("{} not won, no proofs", thousands(self.not_won))
        };
        for (n, what) in [
            (self.proving, "proving"),
            (self.undecided, "undecided"),
            (self.unverified, "UNVERIFIED"),
            (self.proof_failed, "PROOF FAILED"),
        ] {
            if n > 0 {
                out += &format!(", {} {what}", thousands(n));
            }
        }
        out
    }
}

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
    pub fn update(&mut self, now: &Progress, trouble: &Trouble) {
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
            let line = self.line(t, now.done, finished, trouble);
            eprint!("\r{line:<w$}", w = self.width);
            self.width = line.chars().count();
        } else if finished || t - self.printed >= PLAIN_EVERY {
            self.printed = t;
            eprintln!("{}", self.line(t, now.done, finished, trouble));
        }
    }

    fn line(&self, t: f64, done: usize, finished: bool, trouble: &Trouble) -> String {
        let &(t0, first) = self.recent.front().expect("never empty");
        let rate = done.saturating_sub(first.done) as f64 / (t - t0).max(1e-3);
        let short = self.total >= SHORT_COUNTS_FROM;
        // A half-length bar is what brings the longest of these under 120 columns.
        let bar = if short { BAR / 2 } else { BAR };
        let filled = ((bar as u128 * done as u128 / self.total.max(1) as u128) as usize).min(bar);
        let left = self.total - done;
        let to_go = if finished {
            format!("finished in {}", duration(t))
        } else if rate > 0.0 {
            format!("{} to go", duration(left as f64 / rate))
        } else {
            "time to go unknown".into()
        };
        let count = |n: usize| if short { short_count(n as u64) } else { thousands(n as u64) };
        // Long runs creep: a tenth of one percent of 100 trillion deals is 5 hours.
        let percent = 100.0 * done as f64 / self.total.max(1) as f64;
        let percent = if short { format!("{percent:6.2}") } else { format!("{percent:5.1}") };
        format!(
            "  {}{} {percent}%  {} done · {} left · {} deals/s (avg {}) · {to_go} · {}",
            "█".repeat(filled),
            "░".repeat(bar - filled),
            count(done),
            count(left),
            speed(rate),
            speed(done.saturating_sub(self.first.done) as f64 / t.max(1e-3)),
            trouble.text(),
        )
    }
}

/// Deals a second as `5.90M` from a million up, else with commas.
fn speed(per_s: f64) -> String {
    if per_s >= 1e6 { format!("{:.2}M", per_s / 1e6) } else { thousands(per_s.round() as u64) }
}

/// `n` with commas between thousands.
pub fn thousands(n: u64) -> String {
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

/// `n` in three figures, as `50.0T`, `1.25B` or `999M`; under a million, with commas.
fn short_count(n: u64) -> String {
    let (value, unit) = match n {
        0..1_000_000 => return thousands(n),
        1_000_000..1_000_000_000 => (n as f64 / 1e6, "M"),
        1_000_000_000..1_000_000_000_000 => (n as f64 / 1e9, "B"),
        _ => (n as f64 / 1e12, "T"),
    };
    let decimals = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.decimals$}{unit}")
}

/// Seconds as `175d 09h`, `2h 05m`, `6m 14s` or `42s`; in days past 48 hours.
pub fn duration(secs: f64) -> String {
    let s = secs.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {:02}s", s / 60, s % 60),
        3600..DAYS_FROM => format!("{}h {:02}m", s / 3600, s / 60 % 60),
        _ => format!("{}d {:02}h", s / 86_400, s / 3600 % 24),
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
        assert_eq!(speed(5_904_000.0), "5.90M");
        assert_eq!(speed(850_400.0), "850,400");
        assert_eq!(duration(42.4), "42s");
        assert_eq!(duration(374.0), "6m 14s");
        assert_eq!(duration(7_500.0), "2h 05m");
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_700_000_000), "20231114-221320");
        assert_eq!(utc_stamp(1_709_164_800), "20240229-000000");
    }

    #[test]
    fn long_runs_read_short() {
        assert_eq!(short_count(999_999), "999,999");
        assert_eq!(short_count(1_000_000), "1.00M");
        assert_eq!(short_count(999_000_000), "999M");
        assert_eq!(short_count(1_250_000_000), "1.25B");
        assert_eq!(short_count(50_000_000_000_000), "50.0T");
        assert_eq!(short_count(100_000_000_000_000), "100T");
        assert_eq!(short_count(1_234_000_000_000), "1.23T");
        assert_eq!(duration(47.0 * 3600.0 + 59.0 * 60.0), "47h 59m");
        assert_eq!(duration(48.0 * 3600.0), "2d 00h");
        assert_eq!(duration(4_209.0 * 3600.0 + 52.0 * 60.0), "175d 09h");
    }

    fn meter(total: usize) -> Meter {
        let first = Progress { done: 0, gpu_won: 0, gpu_positions: 0, backlog: 0, gpu_failed: 0 };
        Meter::new(total, first, None, "").unwrap()
    }

    #[test]
    fn the_line_of_a_100_trillion_deal_run_fits_a_terminal() {
        let meter = meter(100_000_000_000_000);
        let trouble = Trouble { lost: 37_000, proving: 1, proving_on: true, ..Trouble::default() };
        // Halfway, at 5 million deals a second.
        let line = meter.line(1e7, 50_000_000_000_000, false, &trouble);
        assert!(line.contains(" 50.00%  50.0T done · 50.0T left · "), "{line}");
        assert!(line.chars().count() < 120, "{} columns: {line}", line.chars().count());
    }

    #[test]
    fn the_line_of_an_ordinary_run_is_as_it_was() {
        let meter = meter(2_000_000);
        let trouble = Trouble { proving_on: true, ..Trouble::default() };
        let line = meter.line(1.0, 500_000, false, &trouble);
        assert!(
            line.starts_with(&format!(
                "  {}{}  25.0%  500,000 done · 1,500,000 left · ",
                "█".repeat(4),
                "░".repeat(12)
            )),
            "{line}"
        );
    }

    #[test]
    fn the_bar_does_not_overflow_near_the_last_seed() {
        let total = usize::MAX;
        let line = meter(total).line(1.0, total / 2 + 1, false, &Trouble::default());
        assert_eq!(line.matches('█').count(), 4, "{line}");
    }

    #[test]
    fn the_failures_show_and_are_never_hidden() {
        let none = Trouble { proving_on: true, ..Trouble::default() };
        assert_eq!(none.text(), "0 lost");
        let some = Trouble {
            not_won: 1_044,
            lost: 1_040,
            proving: 1,
            undecided: 2,
            unverified: 3,
            proof_failed: 4,
            proving_on: true,
        };
        assert_eq!(some.text(), "1,040 lost, 1 proving, 2 undecided, 3 UNVERIFIED, 4 PROOF FAILED");
        // With no prover nothing is proven lost, and the deals not won say so.
        assert_eq!(Trouble { not_won: 7, ..Trouble::default() }.text(), "7 not won, no proofs");
    }
}
