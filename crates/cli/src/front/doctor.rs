//! `acc doctor`: whether this computer is ready, a line for each thing a run needs, each with what to do about it. It
//! only looks, but for a few seconds of solving on the CPU; the GPU is only used when asked (`--gpu`).

use super::machine::{self, Battery};
use super::{State, figures, runs_here};
use crate::progress::thousands;
use accordion_gpu::Libraries;
use clap::Args;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

const EXAMPLES: &str = "\
Examples:
  acc doctor              a few seconds
  acc doctor --full       also the build's own checks in depth, a minute or two
  acc doctor --gpu        also try the NVIDIA card, with a few seconds of solving on it";

#[derive(Args)]
#[command(after_help = EXAMPLES)]
pub struct DoctorArgs {
    /// Also try the NVIDIA card: build the GPU code and solve 20,000 deals on it.
    #[arg(long)]
    gpu: bool,
    /// Also run the build's own checks (acc selftest) on fewer cases than it does by default.
    #[arg(long)]
    full: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Pass,
    Warn,
    Fail,
}

/// The lines so far, and the worst of them.
struct Report {
    worst: Level,
}

impl Report {
    /// One line, then what to do, indented.
    fn say(&mut self, level: Level, what: &str, text: &str, todo: &str) {
        self.worst = self.worst.max(level);
        let word = match level {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        };
        println!("{word}  {what:<9} {text}");
        for line in todo.lines().filter(|l| !l.is_empty()) {
            println!("                {line}");
        }
    }
}

pub fn run(args: &DoctorArgs) -> Result<bool, String> {
    let mut r = Report { worst: Level::Pass };
    build(&mut r);
    cpu(&mut r);
    memory(&mut r);
    disk(&mut r);
    tables(&mut r);
    gpu_libraries(&mut r);
    runs(&mut r);
    battery(&mut r);
    smoke_test(&mut r);
    if args.gpu {
        gpu_test(&mut r);
    }
    if args.full {
        full(&mut r)?;
    }
    println!();
    match r.worst {
        Level::Pass => println!("All is ready. acc run --quick plays 10,000 deals to try it out."),
        Level::Warn => println!("acc works here; the WARN lines say how it could work better."),
        Level::Fail => println!("Something must be put right first: see the FAIL lines."),
    }
    Ok(r.worst != Level::Fail)
}

fn build(r: &mut Report) {
    let commit = crate::experiment::commit();
    let commit = if commit.is_empty() { String::new() } else { format!(", source at commit {commit}") };
    if cfg!(debug_assertions) {
        r.say(
            Level::Warn,
            "build",
            &format!("acc {}, a debug build{commit}", env!("CARGO_PKG_VERSION")),
            "A debug build is many times slower. From the project folder: cargo install --path crates/cli --force",
        );
    } else {
        r.say(Level::Pass, "build", &format!("acc {}, release build{commit}", env!("CARGO_PKG_VERSION")), "");
    }
}

/// Instructions the solver uses where it has them, that this CPU has but the build may not use.
#[cfg(target_arch = "x86_64")]
fn unused_instructions() -> (Vec<&'static str>, Vec<&'static str>) {
    // A feature the build has is reported as present whatever the CPU, so this can only find what the build leaves out.
    let found = [
        ("AVX2", cfg!(target_feature = "avx2"), std::is_x86_feature_detected!("avx2")),
        ("BMI2", cfg!(target_feature = "bmi2"), std::is_x86_feature_detected!("bmi2")),
    ];
    let used = found.iter().filter(|f| f.1).map(|f| f.0).collect();
    let unused = found.iter().filter(|f| !f.1 && f.2).map(|f| f.0).collect();
    (used, unused)
}

#[cfg(not(target_arch = "x86_64"))]
fn unused_instructions() -> (Vec<&'static str>, Vec<&'static str>) {
    (Vec::new(), Vec::new())
}

fn cpu(r: &mut Report) {
    let threads = crate::runner::all_threads();
    let (used, unused) = unused_instructions();
    if unused.is_empty() {
        let using = if used.is_empty() {
            String::new()
        } else {
            format!("; the build uses its {}", used.join(" and "))
        };
        r.say(Level::Pass, "CPU", &format!("{threads} threads{using}"), "");
    } else {
        r.say(
            Level::Warn,
            "CPU",
            &format!(
                "{threads} threads; not built for this CPU: slower than it could be (it has {})",
                unused.join(" and ")
            ),
            "Build acc on this computer, from the project folder: cargo install --path crates/cli --force",
        );
    }
}

fn memory(r: &mut Report) {
    match machine::ram_bytes() {
        Some(bytes) if bytes < 8 << 30 => r.say(
            Level::Warn,
            "memory",
            &format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64),
            "Under 8 GB. acc run sizes what it takes to fit, but close other programs during long runs.",
        ),
        Some(bytes) => {
            r.say(Level::Pass, "memory", &format!("{:.0} GB", bytes as f64 / (1u64 << 30) as f64), "")
        }
        None => r.say(
            Level::Warn,
            "memory",
            "couldn't be read",
            "acc run then uses acc batch's defaults: 16 GB at most.",
        ),
    }
}

fn disk(r: &mut Report) {
    let here = std::env::current_dir().map_or_else(|_| ".".into(), |d| d.display().to_string());
    // The folder: acc keeps a run's files where it is started.
    let probe = Path::new(".").join(format!(".acc-doctor-{}.tmp", std::process::id()));
    let writable =
        std::fs::write(&probe, "a test file, deleted at once").and_then(|()| std::fs::remove_file(&probe));
    match writable {
        Ok(()) => r.say(Level::Pass, "folder", &format!("{here} can be written to"), ""),
        Err(e) => r.say(
            Level::Fail,
            "folder",
            &format!("{here} can't be written to ({e})"),
            "acc keeps a run's files in the folder it is started from: go to one you can write to, such as Documents.",
        ),
    }
    match crate::batch::free_bytes(Path::new(".")) {
        Some(free) if free < 5_000_000_000 => r.say(
            Level::Fail,
            "disk",
            &format!("{:.1} GB free", free as f64 / 1e9),
            "A run pauses when less than 5 GB is free here. Make room, or go to another disk.",
        ),
        Some(free) if free < 20_000_000_000 => r.say(
            Level::Warn,
            "disk",
            &format!("{:.0} GB free", free as f64 / 1e9),
            "Enough to start; long runs want more (docs/LONG_RUNS.md).",
        ),
        Some(free) => r.say(Level::Pass, "disk", &format!("{:.0} GB free", free as f64 / 1e9), ""),
        None => r.say(Level::Warn, "disk", "free space couldn't be read", ""),
    }
}

fn tables(r: &mut Report) {
    let piles = crate::runner::available_endgame(Path::new("endgame"));
    const BUILD: &str = "Optional: acc endgame build --piles 11 builds them in about 7 minutes (1.2 GB on disk), and they\n\
                         make solving about 1.2-1.3x faster.";
    match piles {
        0 => r.say(Level::Warn, "tables", "no endgame tables in ./endgame", BUILD),
        11.. => r.say(Level::Pass, "tables", &format!("endgame tables for up to {piles} piles"), ""),
        _ => r.say(Level::Pass, "tables", &format!("endgame tables for up to {piles} piles"), BUILD),
    }
}

/// How to add NVRTC, the step a fresh copy of the project lacks for the GPU.
#[cfg(windows)]
const NVRTC_STEPS: &str = "\
To add it (NVIDIA's own software, under NVIDIA's licence; a 76 MB download), in the project's folder run:
   powershell -ExecutionPolicy Bypass -File tools\\get-nvrtc.ps1
It downloads it from PyPI, checks it and unpacks it into .tools\\cuda-nvrtc-12.9.86, where acc finds it.
By hand instead:
1. Download nvidia_cuda_nvrtc_cu12-12.9.86-py3-none-win_amd64.whl from
   https://pypi.org/project/nvidia-cuda-nvrtc-cu12/12.9.86/#files
2. It is a zip file: rename it to end in .zip, and unpack it into a folder named .tools\\cuda-nvrtc-12.9.86
   in the folder you run acc from, so that .tools\\cuda-nvrtc-12.9.86\\nvidia\\cuda_nvrtc\\bin holds nvrtc64_120_0.dll.
Or, with the CUDA Toolkit 12 installed, set ACC_NVRTC_DIR to the folder holding its nvrtc64_120_0.dll.
Then run acc doctor again. docs/GPU_SETUP.md says more.";

#[cfg(not(windows))]
const NVRTC_STEPS: &str = "\
To add it (NVIDIA's own software, under NVIDIA's licence), not yet tried off Windows, in the project's folder run:
   sh tools/get-nvrtc.sh
It downloads it from PyPI, checks it and unpacks it into .tools/cuda-nvrtc-12.9.86, where acc finds it.
Or, with the CUDA Toolkit 12 installed, set ACC_NVRTC_DIR to the folder holding its libnvrtc.so.12.
Then run acc doctor again. docs/GPU_SETUP.md says more.";

fn gpu_libraries(r: &mut Report) {
    match accordion_gpu::diagnose() {
        Ok(Libraries::Found(version)) => r.say(
            Level::Pass,
            "GPU",
            &format!("NVIDIA driver (CUDA {}) and NVRTC found", Libraries::cuda(version)),
            "acc doctor --gpu tries the card itself.",
        ),
        Ok(Libraries::NoNvrtc(version)) => r.say(
            Level::Warn,
            "GPU",
            &format!(
                "NVIDIA driver (CUDA {}) found, but not NVRTC, the free compiler acc needs to use the card",
                Libraries::cuda(version)
            ),
            NVRTC_STEPS,
        ),
        Ok(Libraries::OldDriver(version)) => r.say(
            Level::Warn,
            "GPU",
            &format!("the NVIDIA driver supports CUDA {}; acc needs 12.9 or later", Libraries::cuda(version)),
            "Update the driver (https://www.nvidia.com/drivers), then run acc doctor again.",
        ),
        Ok(Libraries::NoDriver) => r.say(
            Level::Pass,
            "GPU",
            "no NVIDIA driver, so runs use the CPU alone",
            "That works. If this computer has an NVIDIA card, installing its driver makes runs much faster.",
        ),
        Err(e) => r.say(Level::Warn, "GPU", &format!("the NVIDIA driver didn't answer: {e}"), ""),
    }
}

fn runs(r: &mut Report) {
    let runs = runs_here();
    let count = |state: fn(&State) -> bool| runs.iter().filter(|run| state(&run.state())).count();
    let (live, paused) = (count(|s| *s == State::Live), count(|s| *s == State::Paused));
    let stopped = count(|s| matches!(s, State::Stopped(_)));
    let text = match (live, paused, stopped) {
        (0, 0, 0) => "none going or paused here".to_string(),
        _ => format!("{live} going, {paused} paused, {stopped} stopped unexpectedly here"),
    };
    if stopped > 0 {
        r.say(Level::Warn, "runs", &text, "acc status shows them; acc resume carries on with one.");
    } else {
        r.say(Level::Pass, "runs", &text, if live + paused > 0 { "acc status shows them." } else { "" });
    }
}

fn battery(r: &mut Report) {
    match machine::battery() {
        Battery::None if cfg!(target_os = "macos") => r.say(
            Level::Pass,
            "battery",
            "not read on macOS yet",
            "acc run can't pause for a low battery here: keep the charger in during long runs.",
        ),
        Battery::None => r.say(Level::Pass, "battery", "none", ""),
        Battery::Level { charger: true, percent } => {
            r.say(Level::Pass, "battery", &format!("{percent}%, on the charger"), "")
        }
        b @ Battery::Level { charger: false, .. } => r.say(
            Level::Warn,
            "battery",
            &machine::describe(b),
            "Plug the charger in before a long run; acc run pauses its run when the battery is under 30%.",
        ),
        Battery::Unreadable => r.say(Level::Warn, "battery", "there is one, but it can't be read", ""),
    }
}

/// `acc batch` on the CPU in a process of its own, with these more arguments, and what it printed, timed.
fn batch(more: &[&str]) -> Result<(String, f64), String> {
    let exe = std::env::current_exe().map_err(|e| format!("finding acc itself: {e}"))?;
    let start = Instant::now();
    let output = Command::new(exe)
        .args(["batch", "--threads", "2", "--no-checkpoint", "--no-log", "--no-dossiers", "--endgame", "off"])
        .args(["--prove", "0", "--no-games-check"])
        .args(more)
        .output()
        .map_err(|e| format!("starting acc batch: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{}{}", stderr.trim(), if stdout.is_empty() { "" } else { "\n" }) + &stdout);
    }
    Ok((stdout, start.elapsed().as_secs_f64()))
}

fn smoke_test(r: &mut Report) {
    match batch(&["--count", "100", "--gpu", "off"]) {
        Ok((report, seconds)) => match figures(&report) {
            Some(f) if f.failed == 0 && f.deals == 100 => r.say(
                Level::Pass,
                "solving",
                &format!(
                    "{} of 100 deals won on 2 threads, every win checked by the rules, in {seconds:.1} s",
                    f.won
                ),
                "",
            ),
            _ => r.say(Level::Fail, "solving", "a test of 100 deals went wrong", &(report + A_BUG)),
        },
        Err(e) => r.say(Level::Fail, "solving", "a test of 100 deals went wrong", &format!("{e}\n{A_BUG}")),
    }
}

const A_BUG: &str = "This is a bug, please report it with this output.";

fn gpu_test(r: &mut Report) {
    if runs_here().iter().any(|run| run.state() == State::Live) {
        r.say(
            Level::Warn,
            "GPU test",
            "a run is going here: the test shares the card with it for a few seconds, and slows it",
            "",
        );
    }
    match batch(&["--count", "20000", "--gpu", "on"]) {
        Ok((report, seconds)) => {
            let line = report.lines().find(|l| l.starts_with("GPU ")).map_or("", |l| l[4..].trim());
            match figures(&report) {
                Some(f) if f.failed == 0 => r.say(
                    Level::Pass,
                    "GPU test",
                    &format!(
                        "{} of {} deals won, in {seconds:.1} s with the card",
                        thousands(f.won),
                        thousands(f.deals)
                    ),
                    line,
                ),
                _ => r.say(Level::Fail, "GPU test", "a test of 20,000 deals went wrong", &(report + A_BUG)),
            }
        }
        Err(e) => r.say(
            Level::Warn,
            "GPU test",
            "the card couldn't be used",
            &format!("{e}\nRuns still work on the CPU: acc run --gpu off. docs/GPU_SETUP.md has more."),
        ),
    }
}

fn full(r: &mut Report) -> Result<(), String> {
    println!("\nThe build's own checks, on fewer cases than acc selftest's defaults:");
    let passed = crate::selftest::run(20_000, 20_000, 2_000, 300, Path::new("endgame"))?;
    println!();
    if passed {
        r.say(Level::Pass, "selftest", "every check agreed", "");
    } else {
        r.say(Level::Fail, "selftest", "a check failed, above", A_BUG);
    }
    Ok(())
}
