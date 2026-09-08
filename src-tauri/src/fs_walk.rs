//! Runs diagnostics independently of filesystem calls blocked by macOS TCC.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, OnceLock,
    },
    time::{Duration, Instant},
};

const SLOW_WALK: Duration = Duration::from_secs(5);
enum Event {
    Started(u64, Instant, &'static str, PathBuf),
    Finished(u64),
}

fn monitor() -> &'static mpsc::Sender<Event> {
    static MONITOR: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
    MONITOR.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("folder-access-watchdog".into())
            .spawn(move || monitor_walks(rx, |operation, dir| {
                log::warn!(
                    "{operation}: filesystem walk of {} has taken over 5 s; macOS Files and Folders permission may be waiting for a response. Check the permission prompt or System Settings > Privacy & Security > Files and Folders.",
                    dir.display()
                );
            }))
        {
            log::warn!("Could not start folder access watchdog: {e}");
        }
        tx
    })
}

fn monitor_walks(rx: mpsc::Receiver<Event>, mut warn: impl FnMut(&str, &Path)) {
    let mut pending = HashMap::new();
    let mut warn_if_slow = |at: Instant, op: &str, dir: &Path| {
        if at.elapsed() < SLOW_WALK {
            return false;
        }
        warn(op, dir);
        true
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Started(id, at, op, dir)) => {
                pending.insert(id, (at, op, dir));
            }
            Ok(Event::Finished(id)) => {
                if let Some((at, op, dir)) = pending.remove(&id) {
                    warn_if_slow(at, op, &dir);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // Remove warned entries: one warning per walk, even if blocked forever.
        pending.retain(|_, (at, op, dir)| !warn_if_slow(*at, op, dir));
    }
}

pub(crate) struct Watch(u64);
impl Watch {
    pub(crate) fn new(operation: &'static str, dir: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let _ = monitor().send(Event::Started(id, Instant::now(), operation, dir.to_owned()));
        Self(id)
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        let _ = monitor().send(Event::Finished(self.0));
    }
}

pub(crate) fn read_dir(dir: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(dir).map_err(|e| {
        #[cfg(target_os = "macos")]
        if matches!(e.raw_os_error(), Some(1 | 13)) {
            log::warn!(
                "Folder access denied for {}: {e}; enable DropBeam in System Settings > Privacy & Security > Files and Folders (Downloads/Desktop access).",
                dir.display()
            );
        }
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watchdog_warns_while_walk_is_still_blocked() {
        let (tx, rx) = mpsc::channel();
        let (warnings, observed) = mpsc::channel();
        let worker = std::thread::spawn(move || monitor_walks(rx, |op, dir| {
            warnings.send((op.to_owned(), dir.to_owned())).unwrap();
        }));
        tx.send(Event::Started(1, Instant::now() - SLOW_WALK, "scan", "Downloads".into())).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            ("scan".into(), PathBuf::from("Downloads")));
        // No completion was required for the warning. Completion must not repeat it.
        tx.send(Event::Finished(1)).unwrap();
        tx.send(Event::Started(2, Instant::now(), "fast", "Desktop".into())).unwrap();
        tx.send(Event::Finished(2)).unwrap();
        drop(tx);
        worker.join().unwrap();
        assert!(observed.try_recv().is_err());
    }
}
