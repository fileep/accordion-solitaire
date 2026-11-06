//! Asking a long `acc batch` to pause: Ctrl+C, or a file appearing beside its checkpoint (for a run with no console at
//! hand). A second Ctrl+C stops the process at once, which costs only the work since the last checkpoint.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

/// Set when a pause has been asked for ([`crate::runner::Options::pause`]).
pub static PAUSE: AtomicBool = AtomicBool::new(false);
static PRESSES: AtomicU32 = AtomicU32::new(0);

pub fn requested() -> bool {
    PAUSE.load(Ordering::Relaxed)
}

/// Ctrl+C was pressed. Atomics only, as this may run in a signal handler.
fn pressed() {
    if PRESSES.fetch_add(1, Ordering::Relaxed) == 0 {
        PAUSE.store(true, Ordering::Relaxed);
    } else {
        stop_now();
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn TerminateProcess(process: *mut std::ffi::c_void, code: u32) -> i32;
}

#[cfg(windows)]
fn stop_now() {
    // SAFETY: the current process's pseudo-handle, and an exit code.
    unsafe {
        TerminateProcess(GetCurrentProcess(), 130);
    }
}

/// Makes Ctrl+C ask for a pause.
#[cfg(windows)]
pub fn catch_ctrl_c() {
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    unsafe extern "system" fn handler(event: u32) -> i32 {
        if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
            pressed();
            1
        } else {
            0 // The window closing, or the computer shutting down: the last checkpoint stands.
        }
    }
    // SAFETY: registers a function that only touches atomics or ends the process, for as long as the process lives.
    if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
        eprintln!("note: Windows refused the Ctrl+C handler; Ctrl+C stops the run, at its last checkpoint");
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
    fn _exit(status: i32) -> !;
}

#[cfg(unix)]
fn stop_now() {
    // SAFETY: _exit, unlike process::exit, may be called in a signal handler.
    unsafe { _exit(130) }
}

/// Makes Ctrl+C (and SIGTERM) ask for a pause.
#[cfg(unix)]
pub fn catch_ctrl_c() {
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    extern "C" fn handler(_: i32) {
        pressed();
    }
    // SAFETY: registers a function that only touches atomics or ends the process.
    unsafe {
        signal(SIGINT, handler as extern "C" fn(i32) as usize);
        signal(SIGTERM, handler as extern "C" fn(i32) as usize);
    }
}

#[cfg(not(any(windows, unix)))]
fn stop_now() {
    std::process::exit(130);
}

#[cfg(not(any(windows, unix)))]
pub fn catch_ctrl_c() {}

/// On a thread of its own, watches for `file`, which asks for a pause (and is removed), and says when one is asked for.
pub fn watch(file: PathBuf) {
    std::thread::spawn(move || {
        loop {
            if file.exists() {
                let _ = std::fs::remove_file(&file);
                PAUSE.store(true, Ordering::Relaxed);
            }
            if requested() {
                eprintln!("\npausing: finishing what is in hand (a second Ctrl+C stops at once, at the last checkpoint)");
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}
