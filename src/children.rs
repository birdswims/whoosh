//! Helper processes started by Whoosh, such as `dns-sd`.
//!
//! The desktop app quits this process with SIGTERM. Tokio keeps that signal
//! from ending the process, and several helpers call `setsid`, so they are not
//! killed when their parent exits. Track them and stop them on shutdown.

#![allow(unsafe_code)]

use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

static PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static DEADLINE_ALLOWED: AtomicBool = AtomicBool::new(false);
static DEADLINE_DISARMED: AtomicBool = AtomicBool::new(false);
static DEADLINE: Once = Once::new();

pub(crate) struct TrackedChild {
    child: Child,
    pid: u32,
}

impl TrackedChild {
    pub(crate) fn new(child: Child) -> Self {
        let pid = child.id();
        PIDS.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(pid);
        Self { child, pid }
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        PIDS.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .retain(|pid| *pid != self.pid);
        // Already waited: `kill` refuses a reaped child, so a reused pid is safe.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl std::ops::Deref for TrackedChild {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for TrackedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}

/// Stops every helper still recorded. Already-exited pids are unchanged.
pub(crate) fn kill_all() {
    let pids: Vec<u32> = PIDS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    for pid in pids {
        kill_pid(pid);
    }
}

/// Lets a stuck `whoosh app` process exit after helpers are stopped.
///
/// Tests leave this off. `process::exit` would end the whole test binary.
pub(crate) fn allow_quit_deadline() {
    DEADLINE_ALLOWED.store(true, Ordering::SeqCst);
}

pub(crate) fn arm_quit_deadline() {
    if !DEADLINE_ALLOWED.load(Ordering::SeqCst) {
        return;
    }
    DEADLINE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("whoosh-quit".into())
            .spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                if DEADLINE_DISARMED.load(Ordering::SeqCst) {
                    return;
                }
                kill_all();
                std::process::exit(0);
            });
    });
}

pub(crate) fn disarm_quit_deadline() {
    DEADLINE_DISARMED.store(true, Ordering::SeqCst);
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    unsafe {
        const PROCESS_TERMINATE: u32 = 0x0001;
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn TerminateProcess(handle: *mut std::ffi::c_void, code: u32) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return;
        }
        let _ = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sleeper() -> Command {
        #[cfg(unix)]
        {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        }
    }

    fn quiet(mut command: Command) -> Command {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn drop_stops_the_helper() {
        let child = TrackedChild::new(quiet(sleeper()).spawn().unwrap());
        let pid = child.id();
        drop(child);
        assert!(
            !still_running(pid),
            "helper {pid} was still running after drop"
        );
    }

    #[test]
    fn tracked_pid_can_be_killed() {
        let mut child = TrackedChild::new(quiet(sleeper()).spawn().unwrap());
        let pid = child.id();
        assert!(PIDS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains(&pid));
        // Same signal `kill_all` sends. Calling `kill_all` here would also
        // stop helpers registered by tests running in parallel.
        kill_pid(pid);
        let status = child.wait().unwrap();
        assert!(!status.success(), "helper exited on its own: {status}");
        assert!(
            !still_running(pid),
            "helper {pid} was still running after it was killed"
        );
    }

    fn still_running(pid: u32) -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("tasklist");
            command.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
            let output = command.output().ok();
            output.is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
        }
    }
}
