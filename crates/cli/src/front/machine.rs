//! What the front end asks of the computer: its memory, a lower priority, a Ctrl+C that leaves this process standing, and
//! the battery. Each through the system's own calls, as `pause.rs` does, and none of them changes a system setting.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The memory installed, in bytes, if it can tell.
#[cfg(windows)]
pub fn ram_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: a structure of the size its first field gives, which the call fills in.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0 && status.total_phys > 0).then_some(status.total_phys)
}

#[cfg(target_os = "linux")]
pub fn ram_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kb.checked_mul(1024)
}

#[cfg(target_os = "macos")]
pub fn ram_bytes() -> Option<u64> {
    let output = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn ram_bytes() -> Option<u64> {
    None
}

/// Lowers this process's priority, and so that of the runs it starts, which inherit it: other programs come first, and
/// the run takes what they leave. It changes nothing once this process ends.
#[cfg(windows)]
pub fn lower_priority() -> bool {
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x4000;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn SetPriorityClass(process: *mut std::ffi::c_void, class: u32) -> i32;
    }
    // SAFETY: the current process's pseudo-handle, and a priority class.
    unsafe { SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS) != 0 }
}

#[cfg(unix)]
pub fn lower_priority() -> bool {
    unsafe extern "C" {
        fn nice(increment: i32) -> i32;
    }
    // SAFETY: takes a number. It returns the new niceness, which may be -1 without an error; this asks for 10 more.
    unsafe { nice(10) != -1 }
}

#[cfg(not(any(windows, unix)))]
pub fn lower_priority() -> bool {
    false
}

/// Set once Ctrl+C has been pressed (or, off Windows, the process asked to stop).
static STOP: AtomicBool = AtomicBool::new(false);

pub fn stop_asked() -> bool {
    STOP.load(Ordering::Relaxed)
}

/// Makes Ctrl+C note that it was pressed rather than end this process, so that the run it started, which gets the same
/// Ctrl+C and pauses, has this process to report on it. Not by ignoring Ctrl+C: the run would inherit that, and never
/// see one.
#[cfg(windows)]
pub fn keep_through_ctrl_c() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    unsafe extern "system" fn handler(event: u32) -> i32 {
        const CTRL_C_EVENT: u32 = 0;
        const CTRL_BREAK_EVENT: u32 = 1;
        if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
            STOP.store(true, Ordering::Relaxed);
            1
        } else {
            0
        }
    }
    // SAFETY: registers a function that only stores to an atomic, for as long as the process lives.
    unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
}

#[cfg(unix)]
pub fn keep_through_ctrl_c() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    extern "C" fn handler(_: i32) {
        STOP.store(true, Ordering::Relaxed);
    }
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    // SAFETY: registers a function that only stores to an atomic. A handler, unlike an ignored signal, goes back to the
    // default in a program this one starts.
    unsafe {
        signal(SIGINT, handler as extern "C" fn(i32) as usize);
        signal(SIGTERM, handler as extern "C" fn(i32) as usize);
    }
}

#[cfg(not(any(windows, unix)))]
pub fn keep_through_ctrl_c() {}

/// The battery, as the guard sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Battery {
    /// A desktop, or a system whose battery isn't read (macOS, for now).
    None,
    /// There is one, but it can't be read just now.
    Unreadable,
    Level {
        percent: u8,
        charger: bool,
    },
}

/// The battery now. `ACC_FAKE_BATTERY` names a file to read instead, for testing: `<percent> <1 or 0 for the charger>`,
/// `unreadable` or `none`. A file that can't be read counts as a battery that can't.
pub fn battery() -> Battery {
    match std::env::var_os("ACC_FAKE_BATTERY") {
        Some(file) => fake_battery(Path::new(&file)),
        None => real_battery(),
    }
}

/// Whether the battery is faked, which the guard reads every second instead of every 30.
pub fn battery_faked() -> bool {
    std::env::var_os("ACC_FAKE_BATTERY").is_some()
}

fn fake_battery(file: &Path) -> Battery {
    let Ok(text) = std::fs::read_to_string(file) else { return Battery::Unreadable };
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["none"] => Battery::None,
        [percent, charger] => match (percent.parse::<u8>(), *charger) {
            (Ok(percent), "1" | "0") if percent <= 100 => {
                Battery::Level { percent, charger: *charger == "1" }
            }
            _ => Battery::Unreadable,
        },
        _ => Battery::Unreadable,
    }
}

#[cfg(windows)]
fn real_battery() -> Battery {
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }
    const NO_SYSTEM_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    let mut s = SystemPowerStatus::default();
    // SAFETY: a structure for the call to fill in.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
        return Battery::Unreadable;
    }
    if s.battery_flag != UNKNOWN && s.battery_flag & NO_SYSTEM_BATTERY != 0 {
        return Battery::None;
    }
    if s.battery_flag == UNKNOWN || s.battery_life_percent > 100 || s.ac_line_status == UNKNOWN {
        return Battery::Unreadable;
    }
    Battery::Level { percent: s.battery_life_percent, charger: s.ac_line_status == 1 }
}

#[cfg(target_os = "linux")]
fn real_battery() -> Battery {
    let Ok(supplies) = std::fs::read_dir("/sys/class/power_supply") else { return Battery::None };
    let (mut battery, mut charger) = (None, None);
    for supply in supplies.flatten() {
        let name = supply.file_name().to_string_lossy().into_owned();
        let read = |what: &str| std::fs::read_to_string(supply.path().join(what)).ok();
        if name.starts_with("BAT") && battery.is_none() {
            let percent = read("capacity").and_then(|t| t.trim().parse::<u8>().ok());
            let status = read("status").unwrap_or_default();
            battery = Some((percent, status.trim().to_string()));
        } else if name.starts_with("AC") {
            charger = charger.or(read("online").map(|t| t.trim() == "1"));
        }
    }
    let Some((percent, status)) = battery else { return Battery::None };
    let charging = matches!(status.as_str(), "Charging" | "Full" | "Not charging");
    match percent {
        Some(percent) if percent <= 100 => Battery::Level { percent, charger: charger.unwrap_or(charging) },
        _ => Battery::Unreadable,
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn real_battery() -> Battery {
    Battery::None
}

/// The battery's part in the guard: when to pause the run, and when it may carry on.
pub struct Guard {
    pub pause_below: u8,
    pub resume_from: u8,
    /// Whether the battery has been read: from then on, one that can't be counts as low.
    seen: bool,
}

/// Under this, a pause that drags on is cut short.
pub const CRITICAL: u8 = 10;
/// The marks `acc run` pauses at and carries on from, unless told others.
pub const BATTERY_PAUSE: u8 = 30;
pub const BATTERY_RESUME: u8 = 60;

impl Guard {
    pub fn new(pause_below: u8, resume_from: u8) -> Guard {
        Guard { pause_below, resume_from, seen: false }
    }

    /// Why the run should pause, if it should.
    pub fn pause_for(&mut self, battery: Battery) -> Option<String> {
        match battery {
            Battery::None => None,
            Battery::Unreadable if self.seen => Some("the battery can't be read any more".into()),
            Battery::Unreadable => None,
            Battery::Level { percent, charger } => {
                self.seen = true;
                (percent < self.pause_below).then(|| {
                    let on = if charger { "on the charger, but" } else { "not on the charger, and" };
                    format!("the battery is {on} down to {percent}%")
                })
            }
        }
    }

    /// Whether a run paused for the battery may carry on.
    pub fn may_resume(&self, battery: Battery) -> bool {
        match battery {
            Battery::None => true,
            Battery::Unreadable => false,
            Battery::Level { percent, charger } => charger && percent >= self.resume_from,
        }
    }

    /// Whether a pause that has taken a while should be cut short.
    pub fn critical(&self, battery: Battery) -> bool {
        match battery {
            Battery::None => false,
            Battery::Unreadable => self.seen,
            Battery::Level { percent, .. } => percent < CRITICAL,
        }
    }
}

/// The battery in a few words: `85%, on the charger`.
pub fn describe(battery: Battery) -> String {
    match battery {
        Battery::None => "none".into(),
        Battery::Unreadable => "can't be read".into(),
        Battery::Level { percent, charger } => {
            format!("{percent}%, {}", if charger { "on the charger" } else { "not on the charger" })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(percent: u8, charger: bool) -> Battery {
        Battery::Level { percent, charger }
    }

    #[test]
    fn the_guard_pauses_under_its_mark_and_carries_on_charged_and_on_the_charger() {
        let mut guard = Guard::new(30, 60);
        assert_eq!(guard.pause_for(Battery::Unreadable), None, "not yet seen: a desktop that can't tell");
        assert_eq!(guard.pause_for(level(31, false)), None);
        assert!(guard.pause_for(level(29, true)).is_some_and(|why| why.contains("29%")));
        assert!(guard.pause_for(Battery::Unreadable).is_some(), "seen, then lost: as if low");
        assert_eq!(guard.pause_for(Battery::None), None);

        assert!(!guard.may_resume(level(80, false)), "not on the charger");
        assert!(!guard.may_resume(level(59, true)));
        assert!(guard.may_resume(level(60, true)));
        assert!(!guard.may_resume(Battery::Unreadable));

        assert!(guard.critical(level(9, true)) && !guard.critical(level(10, false)));
        assert!(guard.critical(Battery::Unreadable));
        assert!(!Guard::new(30, 60).critical(Battery::Unreadable));
    }

    #[test]
    fn a_fake_battery_is_read_as_the_tests_write_it() {
        let file = std::env::temp_dir().join(format!("acc-fake-battery-{}", std::process::id()));
        for (text, expected) in [
            ("55 1\n", level(55, true)),
            ("7 0", level(7, false)),
            ("unreadable", Battery::Unreadable),
            ("none", Battery::None),
            ("101 1", Battery::Unreadable),
            ("50 yes", Battery::Unreadable),
        ] {
            std::fs::write(&file, text).unwrap();
            assert_eq!(fake_battery(&file), expected, "{text:?}");
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!(fake_battery(&file), Battery::Unreadable, "no file");
    }
}
