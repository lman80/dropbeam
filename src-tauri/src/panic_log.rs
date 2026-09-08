//! Emergency diagnostics must not re-enter the logger or any application mutex.
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::mem::ManuallyDrop;
use std::path::Path;

struct Line {
    bytes: [u8; 4096],
    len: usize,
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        // Leave room for a newline, including for oversized panic payloads.
        let available = self.bytes.len().saturating_sub(self.len).saturating_sub(1);
        let mut end = text.len().min(available);
        while !text.is_char_boundary(end) { end = end.saturating_sub(1); }
        for byte in text.bytes().take(end) {
            if let Some(slot) = self.bytes.get_mut(self.len) {
                *slot = byte;
                self.len += 1;
            }
        }
        Ok(())
    }
}

pub(crate) fn install(log_dir: Option<&Path>) {
    // Open once before installing the hook. A separate append-only file avoids
    // the rotating logger's locks; telemetry already scans DropBeam*.log.
    let log = log_dir.and_then(|dir| {
        OpenOptions::new().create(true).append(true)
            .open(dir.join("DropBeam-panic.log")).ok()
    });
    // Borrow the OS stderr handle, bypassing Rust's stderr lock. ManuallyDrop
    // prevents us from closing a descriptor owned by the process.
    #[cfg(unix)]
    let stderr = {
        use std::os::fd::{AsRawFd, FromRawFd};
        ManuallyDrop::new(unsafe { File::from_raw_fd(std::io::stderr().as_raw_fd()) })
    };
    #[cfg(windows)]
    let stderr = {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        ManuallyDrop::new(unsafe { File::from_raw_handle(std::io::stderr().as_raw_handle()) })
    };
    std::panic::set_hook(Box::new(move |info| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            use std::fmt::Write as _;
            let mut line = Line { bytes: [0; 4096], len: 0 };
            let message = info.payload().downcast_ref::<&str>().copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("non-string panic payload");
            // Only built-in string/integer formatting, never payload Display/Debug
            // or PanicHookInfo's formatter. No heap formatting or logger callbacks.
            if let Some(at) = info.location() {
                let _ = write!(line, "[ERROR][app_lib] panicked at {}:{}:{}: ", at.file(), at.line(), at.column());
            } else {
                let _ = write!(line, "[ERROR][app_lib] panicked: ");
            }
            let _ = line.write_str(message);
            if let Some(slot) = line.bytes.get_mut(line.len) {
                *slot = b'\n';
                line.len += 1;
            }
            let bytes = line.bytes.get(..line.len).unwrap_or(b"panicked\n");
            // stderr first: even an unavailable/full diagnostics volume cannot
            // erase the original message. Ignore closed descriptors and I/O errors.
            let _ = (&*stderr).write(bytes);
            if let Some(file) = &log {
                let _ = (&*file).write(bytes);
            }
        }));
        // catch_unwind is only a last guard: Rust may abort on a nested panic
        // inside a hook before unwinding. The body itself must remain non-panicking.
        // Don't run a potentially panicking destructor on an unexpected payload.
        if let Err(payload) = result {
            std::mem::forget(payload);
        }
    }));
}

#[cfg(test)]
mod tests {
    #[test]
    fn hook_survives_with_logger_and_stderr_locks_held() {
        const CHILD: &str = "DROPBEAM_PANIC_HOOK_TEST";
        if let Some(dir) = std::env::var_os(CHILD) {
            struct BrokenLogger;
            impl log::Log for BrokenLogger {
                fn enabled(&self, _: &log::Metadata<'_>) -> bool { true }
                fn log(&self, _: &log::Record<'_>) { panic!("logger must not be called"); }
                fn flush(&self) {}
            }
            static LOGGER: BrokenLogger = BrokenLogger;
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(log::LevelFilter::Trace);
            super::install(Some(std::path::Path::new(&dir)));
            let held = std::sync::Mutex::new(());
            let _stderr = std::io::stderr().lock();
            assert!(std::panic::catch_unwind(|| {
                let _guard = held.lock().unwrap();
                panic!("original worker panic");
            }).is_err());
            assert!(std::panic::catch_unwind(|| { drop(held.lock().unwrap()); }).is_err());
            assert!(std::panic::catch_unwind(|| std::panic::panic_any(42u32)).is_err());
            let oversized = "x".repeat(10_000);
            assert!(std::panic::catch_unwind(|| std::panic::panic_any(oversized)).is_err());
            return;
        }
        let dir = std::env::temp_dir().join(format!("dropbeam-panic-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "panic_log::tests::hook_survives_with_logger_and_stderr_locks_held", "--nocapture"])
            .env(CHILD, &dir).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let disk = std::fs::read(dir.join("DropBeam-panic.log")).unwrap();
        assert_eq!(disk, output.stderr);
        let text = String::from_utf8_lossy(&disk);
        assert!(text.contains("original worker panic"));
        assert!(text.contains("PoisonError"));
        assert!(text.contains("non-string panic payload"));
        assert!(disk.ends_with(b"\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
