//! Keep the computer from idle-sleeping while files are moving. A big send left
//! to finish ("I'll go make tea") used to stop when the Mac dozed off after a
//! few idle minutes. While on, a small helper process holds a "don't idle-sleep"
//! assertion; it watches our pid, so it can never outlive DropBeam. Closing a
//! laptop lid still sleeps — that's the person's choice, not ours to override.
//!
//! Windows has no helper process: a parked thread holds
//! `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`; the hold is tied
//! to that thread, so it ends when the thread is told to stop (or DropBeam exits).

use std::process::Child;
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// What currently holds the system awake.
enum Hold {
    Helper(Child),
    #[allow(dead_code)] // only built on Windows
    Thread(std::sync::mpsc::Sender<()>),
}

impl Hold {
    fn exited(&mut self) -> bool {
        match self {
            Hold::Helper(c) => matches!(c.try_wait(), Ok(Some(_))),
            Hold::Thread(_) => false,
        }
    }
    fn release(self) {
        match self {
            Hold::Helper(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            // Dropping the sender wakes the thread, which clears its hold and exits.
            Hold::Thread(tx) => drop(tx),
        }
    }
}

static HOLDER: Mutex<Option<Hold>> = Mutex::new(None);

#[cfg(windows)]
fn spawn() -> Option<Hold> {
    use windows::Win32::System::Power::{SetThreadExecutionState, EXECUTION_STATE, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("keep-awake".into())
        .spawn(move || {
            unsafe { SetThreadExecutionState(EXECUTION_STATE(ES_CONTINUOUS.0 | ES_SYSTEM_REQUIRED.0)) };
            let _ = rx.recv(); // returns when the sender is dropped
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        })
        .map_err(|e| log::info!("keep-awake: thread unavailable: {e}"))
        .ok()?;
    Some(Hold::Thread(tx))
}

#[cfg(not(windows))]
fn spawn() -> Option<Hold> {
    let pid = std::process::id().to_string();
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/caffeinate");
        c.args(["-i", "-w", &pid]);
        c
    } else if cfg!(target_os = "linux") {
        let mut c = Command::new("systemd-inhibit");
        c.args([
            "--what=idle:sleep",
            "--who=DropBeam",
            "--why=Sending or receiving files",
            "--mode=block",
            "tail",
            &format!("--pid={pid}"),
            "-f",
            "/dev/null",
        ]);
        c
    } else {
        return None;
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map_err(|e| log::info!("keep-awake: helper unavailable: {e}")).ok().map(Hold::Helper)
}

/// Turn the hold on or off. Idempotent; best-effort (no helper = no hold).
pub fn set(on: bool) {
    let mut held = HOLDER.lock().unwrap_or_else(|p| p.into_inner());
    // A helper that already exited (killed by the user, etc.) no longer holds.
    if held.as_mut().is_some_and(Hold::exited) {
        *held = None;
    }
    match (on, held.is_some()) {
        (true, false) => {
            *held = spawn();
            if held.is_some() {
                log::info!("keep-awake: on (transfer in progress)");
            }
        }
        (false, true) => {
            if let Some(hold) = held.take() {
                hold.release();
            }
            log::info!("keep-awake: off");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn toggling_is_idempotent_and_releases() {
        super::set(true);
        super::set(true);
        super::set(false);
        super::set(false);
        assert!(super::HOLDER.lock().unwrap().is_none());
    }
}
