//! Turning transfer failures into what actually happened (audit T6, T16).
//!
//! * A deliberate stop is carried in the QUIC CONNECTION_CLOSE reason
//!   ("canceled" / "paused"). The far side reads it from the connection
//!   itself — noq's error Display ("connection lost") drops the reason, which
//!   is why a cancel used to show up over there as "Failed: connection lost".
//! * Everything users see is a plain sentence; internal staging paths
//!   (`.dropbeam-recv-…`) never leak.
//! * A FAT32 destination can't hold a file of 4 GiB or more: that is checked
//!   before a byte moves, not after 4 GB of it.
use super::*;

/// The close reason our cancel puts on the connection (every shipped build
/// closes with exactly this on cancel AND pause).
pub(crate) const CLOSE_CANCELED: &[u8] = b"canceled";
/// New builds close a pause with this, so the far side can say "paused".
pub(crate) const CLOSE_PAUSED: &[u8] = b"paused";
/// Internal marker error: the OTHER side stopped this transfer on purpose.
pub(crate) const PEER_CANCELED: &str = "canceled by the other device";
pub(crate) const PEER_PAUSED: &str = "paused by the other device (canceled)";

/// Did the peer close `conn` deliberately (cancel or pause)?
pub(crate) fn peer_stopped(conn: &Connection) -> Option<CancelReason> {
    match conn.close_reason() {
        Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) => reason_of(&close.reason),
        _ => None,
    }
}

fn reason_of(reason: &[u8]) -> Option<CancelReason> {
    if reason == CLOSE_CANCELED { Some(CancelReason::Cancel) } else if reason == CLOSE_PAUSED { Some(CancelReason::Pause) } else { None }
}

/// The same question asked of an error alone (no connection at hand): walk the
/// whole chain, including errors wrapped inside `io::Error`.
pub(crate) fn error_peer_stopped(e: &anyhow::Error) -> Option<CancelReason> {
    use iroh::endpoint::{ConnectionError, ReadError, WriteError};
    fn conn_reason(c: &ConnectionError) -> Option<CancelReason> {
        match c { ConnectionError::ApplicationClosed(close) => reason_of(&close.reason), _ => None }
    }
    fn any(err: &(dyn std::error::Error + 'static)) -> Option<CancelReason> {
        if let Some(c) = err.downcast_ref::<ConnectionError>() { return conn_reason(c); }
        if let Some(ReadError::ConnectionLost(c)) = err.downcast_ref::<ReadError>() { return conn_reason(c); }
        if let Some(WriteError::ConnectionLost(c)) = err.downcast_ref::<WriteError>() { return conn_reason(c); }
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            if let Some(inner) = io.get_ref() { return any(inner); }
        }
        None
    }
    e.chain().find_map(any)
}

/// The close reason for a local stop of `reason`.
pub(crate) fn close_reason_for(reason: CancelReason) -> &'static [u8] {
    match reason { CancelReason::Cancel => CLOSE_CANCELED, CancelReason::Pause => CLOSE_PAUSED }
}

/// The marker error for a peer's deliberate stop.
pub(crate) fn peer_stop_error(reason: CancelReason) -> anyhow::Error {
    anyhow::anyhow!(match reason { CancelReason::Cancel => PEER_CANCELED, CancelReason::Pause => PEER_PAUSED })
}

/// "Canceled by Alex" / "Alex paused it" for the far side's card.
pub(crate) fn peer_stop_detail(err: &str, who: Option<&str>) -> Option<String> {
    let who = who.filter(|w| !w.trim().is_empty());
    if err.contains(PEER_PAUSED) {
        Some(match who { Some(w) => format!("{w} paused the transfer"), None => "The other device paused the transfer".into() })
    } else if err.contains(PEER_CANCELED) {
        Some(match who { Some(w) => format!("Canceled by {w}"), None => "Canceled by the other device".into() })
    } else { None }
}

/// Every common failure as one plain sentence. Unknown errors pass through
/// with internal staging paths removed.
pub(crate) fn friendly(dir: Direction, err: &str) -> String {
    // A Quick Send refusal is already a sentence for people.
    if let Some(text) = err.split_once(super::quick::REFUSED).map(|(_, t)| t) {
        return text.to_string();
    }
    let lower = err.to_ascii_lowercase();
    let receiving = matches!(dir, Direction::Receive);
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    // Already a sentence written for people: keep it.
    if has(&["their disk is full", "couldn't reach", "no direct connection", "lost the direct connection",
        "this link", "needs an update", "already running", "declined", "verification failed"]) {
        // The receiver's own disk-full sentence: say it like the sender's
        // branch below (no "receiver: " plumbing tag on the card).
        if !receiving && lower.contains("their disk is full") {
            return "The recipient's disk is full — once they free up space, retry and it picks up where it stopped".into();
        }
        return strip_internal_paths(err.strip_prefix("receiver: ").unwrap_or(err));
    }
    if has(&["no space left on device", "os error 28", "not enough space on the disk", "os error 112"]) {
        return if receiving { "This device's disk is full — free up space, then retry and it picks up where it stopped".into() }
            else { "The recipient's disk is full — once they free up space, retry and it picks up where it stopped".into() };
    }
    if has(&["file too large", "os error 27", "efbig", "fat32"]) {
        return if receiving { "This file is 4 GB or larger, and the drive it's saving to (FAT32) can't hold files that big — choose a folder on another drive".into() }
            else { "The file is 4 GB or larger and the recipient's drive (FAT32) can't hold files that big".into() };
    }
    if has(&["permission denied", "os error 13", "access is denied", "os error 5)", "read-only file system", "os error 30"]) {
        return if receiving { "DropBeam isn't allowed to save into that folder — pick a different folder in Settings".into() }
            else if lower.starts_with("receiver:") { "The recipient's DropBeam isn't allowed to save into its download folder — they can pick another one in Settings".into() }
            else { "DropBeam couldn't read one of the files — check it still exists and you can open it".into() };
    }
    if has(&["frame too large", "frame exceeds", "header too large", "message too large"]) {
        return "This transfer has more files than the other device's DropBeam can take at once — ask them to update DropBeam, then retry".into();
    }
    if has(&["invalid ticket", "parse ticket", "bad ticket", "invalid code"]) {
        return "That code or link isn't valid — check you copied all of it".into();
    }
    if has(&["no pending send for token", "link has expired"]) {
        return "This link has expired or was already used — ask for a new one".into();
    }
    if has(&["inactivity timeout", "stalled", "no data for", "receiver stopped", "progress output stalled"]) {
        return "The transfer stalled — no data moved for a while. Retry and it picks up where it stopped".into();
    }
    if has(&["stream ended", "finished early", "stream reset", "before the recipient confirmed", "interrupted before"]) {
        return "The connection dropped before the transfer finished — retry and it picks up where it stopped".into();
    }
    if has(&["connection lost", "timed out", "reset by peer", "closed by peer", "aborted by peer", "connection refused",
        "dial ticket", "network is unreachable", "host is unreachable", "locally closed", "connection closed"]) {
        return "The connection to the other device was lost — check both devices are online, then retry".into();
    }
    strip_internal_paths(err)
}

/// Never show a staging path: `/x/y/.dropbeam-recv-<uuid>.part` → "a temporary file".
pub(crate) fn strip_internal_paths(err: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"(?:[A-Za-z]:)?[^\s"']*[\\/]?\.dropbeam-[^\s"']*"#).unwrap());
    re.replace_all(err, "a temporary file").into_owned()
}

/// The largest file the filesystem holding `dir` can store, when it has a
/// small hard limit (FAT32: 4 GiB − 1). `None` = no limit worth checking.
pub(crate) fn max_file_size(dir: &Path) -> Option<u64> {
    const FAT32_MAX: u64 = u32::MAX as u64;
    let existing = dir.ancestors().find(|a| a.exists())?;
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let c = CString::new(existing.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 { return None; }
        let stat = unsafe { stat.assume_init() };
        let name: Vec<u8> = stat.f_fstypename.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        return (name == b"msdos").then_some(FAT32_MAX);
    }
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        const MSDOS_SUPER_MAGIC: i64 = 0x4d44;
        let c = CString::new(existing.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 { return None; }
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)]
        return (stat.f_type as i64 == MSDOS_SUPER_MAGIC).then_some(FAT32_MAX);
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
        let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut root = [0u16; 512];
        unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.ok()?;
        let mut fs_name = [0u16; 64];
        unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut fs_name)) }.ok()?;
        let len = fs_name.iter().position(|&c| c == 0).unwrap_or(fs_name.len());
        let name = String::from_utf16_lossy(&fs_name[..len]).to_ascii_uppercase();
        return name.starts_with("FAT").then_some(FAT32_MAX);
    }
    #[allow(unreachable_code)]
    { let _ = existing; None }
}

/// Refuse a receive up front when a file can't fit the destination filesystem.
pub(crate) fn check_fits(dir: &Path, header: &serde_json::Value) -> Result<()> {
    let Some(limit) = max_file_size(dir) else { return Ok(()) };
    let biggest = header["items"].as_array().into_iter().flatten()
        .filter_map(|i| i["size"].as_u64()).max().unwrap_or(0);
    anyhow::ensure!(biggest <= limit, "file too large for a FAT32 drive");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_disk_full_reads_as_a_sentence_on_the_sender() {
        let e = "receiver: their disk is full — once they free up space, retry and it picks up where it stopped";
        assert_eq!(friendly(Direction::Send, e), "The recipient's disk is full — once they free up space, retry and it picks up where it stopped");
    }

    #[test]
    fn t16_common_failures_read_as_sentences() {
        let r = Direction::Receive;
        let s = Direction::Send;
        for (dir, raw, expect) in [
            (s, "stream ended before /Users/a/Downloads/.dropbeam-recv-1234.part finished", "connection dropped"),
            (r, "frame too large: 2000000 > 1048576", "more files"),
            (s, "connection lost", "connection to the other device was lost"),
            (r, "read error: connection lost", "connection to the other device was lost"),
            (r, "invalid ticket", "isn't valid"),
            (s, "inactivity timeout", "stalled"),
            (s, "timed out", "connection to the other device was lost"),
            (r, "File too large (os error 27)", "FAT32"),
            (r, "Permission denied (os error 13)", "isn't allowed to save"),
            (r, "no pending send for token", "expired"),
            (r, "write: No space left on device (os error 28)", "This device's disk is full"),
            (s, "receiver: No space left on device (os error 28)", "recipient's disk is full"),
        ] {
            let got = friendly(dir, raw);
            assert!(got.contains(expect), "{raw:?} → {got:?}");
            assert!(!got.contains(".dropbeam-"), "{got}");
        }
        assert!(friendly(s, "their disk is full — once").starts_with("The recipient's disk is full"));
        let odd = friendly(r, "weird failure at C:\\Users\\x\\.dropbeam-recv-ab.part now");
        assert!(!odd.contains(".dropbeam") && odd.contains("a temporary file"), "{odd}");
    }

    #[test]
    fn t6_peer_stop_markers_and_detail() {
        assert_eq!(reason_of(b"canceled"), Some(CancelReason::Cancel));
        assert_eq!(reason_of(b"paused"), Some(CancelReason::Pause));
        assert_eq!(reason_of(b"done"), None);
        let e = peer_stop_error(CancelReason::Cancel);
        assert!(e.to_string().contains("canceled"), "the marker must classify as a cancel everywhere");
        assert_eq!(peer_stop_detail(&e.to_string(), Some("Alex")).as_deref(), Some("Canceled by Alex"));
        let p = peer_stop_error(CancelReason::Pause).to_string();
        assert!(p.contains("canceled"));
        assert_eq!(peer_stop_detail(&p, None).as_deref(), Some("The other device paused the transfer"));
    }
}
