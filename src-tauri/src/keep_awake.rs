//! Keep the computer from idle-sleeping while files are moving. A big send left
//! to finish ("I'll go make tea") used to stop when the Mac dozed off after a
//! few idle minutes. While on, a small helper process holds a "don't idle-sleep"
//! assertion; it watches our pid, so it can never outlive DropBeam. Closing a
//! laptop lid still sleeps — that's the person's choice, not ours to override.

use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

static HOLDER: Mutex<Option<Child>> = Mutex::new(None);

fn spawn() -> Option<Child> {
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
        // Windows needs SetThreadExecutionState (a `windows` crate feature) — not yet.
        return None;
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map_err(|e| log::info!("keep-awake: helper unavailable: {e}")).ok()
}

/// Turn the hold on or off. Idempotent; best-effort (no helper = no hold).
pub fn set(on: bool) {
    let mut held = HOLDER.lock().unwrap_or_else(|p| p.into_inner());
    // A helper that already exited (killed by the user, etc.) no longer holds.
    if let Some(child) = held.as_mut() {
        if matches!(child.try_wait(), Ok(Some(_))) {
            *held = None;
        }
    }
    match (on, held.is_some()) {
        (true, false) => {
            *held = spawn();
            if held.is_some() {
                log::info!("keep-awake: on (transfer in progress)");
            }
        }
        (false, true) => {
            if let Some(mut child) = held.take() {
                let _ = child.kill();
                let _ = child.wait();
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
