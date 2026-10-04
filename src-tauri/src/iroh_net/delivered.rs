//! Per-sender delivery ledger (audit S7): which files in this device's receive
//! folder were delivered BY WHICH SENDER, under which sent name.
//!
//! `files.stat` (resume skip) and `files.verify` (Verify copy) used to stat or
//! hash ANY name under Downloads for ANY friend, which let a friend probe for
//! (and fingerprint) files nobody ever sent them. They now answer only for
//! paths this exact sender delivered, looked up by the name they sent — which
//! also makes Verify find the copy wherever it actually landed (a "Save to"
//! folder, iOS Documents, a "name (1)" collision sibling).
//!
//! In memory, persisted (debounced, off the async threads) to
//! `delivered.json`; bounded per sender, per device and by age. Losing the
//! newest entries in a crash only costs a re-send (the receiver's
//! `identical_landed` dedup reuses the existing copy) or an "unverified" row.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Entries kept per sender — comfortably above the biggest folder sends seen
/// in the field (~15k files), so a whole folder stays resumable/verifiable.
const PER_SENDER: usize = 50_000;
/// Distinct senders remembered (least recently active dropped first).
const MAX_SENDERS: usize = 200;
/// Entries older than this are forgotten (Verify copy is a "just sent" action).
const MAX_AGE_MS: u64 = 60 * 24 * 3600 * 1000;
const FILE: &str = "delivered.json";

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct Entry {
    /// The item name exactly as the sender put it in the push header.
    name: String,
    /// Where it landed on this device.
    path: PathBuf,
    /// When it landed (ms since epoch).
    at: u64,
}

#[derive(Default)]
struct Ledger {
    loaded: bool,
    senders: HashMap<String, VecDeque<Entry>>,
    dirty: bool,
    saving: bool,
}

static LEDGERS: std::sync::LazyLock<Mutex<HashMap<PathBuf, Ledger>>> = std::sync::LazyLock::new(Default::default);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn load_into(config: &Path, ledger: &mut Ledger) {
    if ledger.loaded { return; }
    ledger.loaded = true;
    let Ok(bytes) = std::fs::read(config.join(FILE)) else { return };
    let Ok(map) = serde_json::from_slice::<HashMap<String, VecDeque<Entry>>>(&bytes) else {
        log::warn!("delivered ledger unreadable; starting empty");
        return;
    };
    // Entries recorded since start (before the lazy load) win over disk.
    for (sender, entries) in map {
        let slot = ledger.senders.entry(sender).or_default();
        let fresh = std::mem::take(slot);
        *slot = entries;
        slot.extend(fresh);
    }
    prune(&mut ledger.senders, now_ms());
}

fn prune(senders: &mut HashMap<String, VecDeque<Entry>>, now: u64) {
    for entries in senders.values_mut() {
        entries.retain(|e| now.saturating_sub(e.at) < MAX_AGE_MS);
        while entries.len() > PER_SENDER { entries.pop_front(); }
    }
    senders.retain(|_, e| !e.is_empty());
    if senders.len() > MAX_SENDERS {
        let mut by_age: Vec<(u64, String)> = senders.iter()
            .map(|(s, e)| (e.back().map_or(0, |x| x.at), s.clone())).collect();
        by_age.sort();
        for (_, s) in by_age.into_iter().take(senders.len() - MAX_SENDERS) { senders.remove(&s); }
    }
}

/// Persist soon, on a plain thread (never on a tokio worker), coalescing bursts.
fn schedule_save(config: &Path, ledger: &mut Ledger) {
    ledger.dirty = true;
    if ledger.saving { return; }
    ledger.saving = true;
    let config = config.to_path_buf();
    let _ = std::thread::Builder::new().name("delivered-save".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        save_now(&config);
    });
}

fn save_now(config: &Path) {
    let snapshot = {
        let mut all = LEDGERS.lock().unwrap_or_else(|p| p.into_inner());
        let Some(ledger) = all.get_mut(config) else { return };
        // Merge what is on disk first, so a save never drops older deliveries
        // that this run simply had not needed to read yet.
        load_into(config, ledger);
        ledger.saving = false;
        if !ledger.dirty { return; }
        ledger.dirty = false;
        serde_json::to_vec(&ledger.senders)
    };
    let Ok(bytes) = snapshot else { return };
    let tmp = config.join(format!("{FILE}.tmp"));
    if std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, config.join(FILE))).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// `sender` (endpoint id) delivered the item it called `name`; it is at `path`.
pub(crate) fn record(config: &Path, sender: &str, name: &str, path: &Path) {
    if sender.is_empty() || name.is_empty() { return; }
    let mut all = LEDGERS.lock().unwrap_or_else(|p| p.into_inner());
    let ledger = all.entry(config.to_path_buf()).or_default();
    let entries = ledger.senders.entry(sender.to_owned()).or_default();
    // Re-delivery of the same file: refresh it (only the recent tail is
    // checked — scanning 50k entries per landed file was quadratic).
    let tail = entries.len().saturating_sub(256);
    if let Some(i) = entries.range(tail..).position(|e| e.name == name && e.path == path) { entries.remove(tail + i); }
    entries.push_back(Entry { name: name.to_owned(), path: path.to_path_buf(), at: now_ms() });
    while entries.len() > PER_SENDER { entries.pop_front(); }
    if ledger.senders.len() > MAX_SENDERS { prune(&mut ledger.senders, now_ms()); }
    schedule_save(config, ledger);
}

/// Where `sender`'s item `name` landed, newest delivery first. Empty = this
/// sender never delivered anything under that name here.
pub(crate) fn lookup(config: &Path, sender: &str, name: &str) -> Vec<PathBuf> {
    let mut all = LEDGERS.lock().unwrap_or_else(|p| p.into_inner());
    let ledger = all.entry(config.to_path_buf()).or_default();
    load_into(config, ledger);
    ledger.senders.get(sender).map(|entries| entries.iter().rev()
        .filter(|e| e.name == name).map(|e| e.path.clone()).collect()).unwrap_or_default()
}

/// Write any pending entries now (tests; orderly shutdown).
#[allow(dead_code)]
pub(crate) fn flush(config: &Path) {
    {
        let mut all = LEDGERS.lock().unwrap_or_else(|p| p.into_inner());
        let ledger = all.entry(config.to_path_buf()).or_default();
        load_into(config, ledger);
        ledger.dirty = true;
    }
    save_now(config);
}

#[cfg(test)]
pub(crate) fn forget_in_memory(config: &Path) {
    LEDGERS.lock().unwrap().remove(config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_is_per_sender_newest_first_and_persists() {
        let dir = std::env::temp_dir().join(format!("dropbeam-delivered-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        record(&dir, "alice", "a.txt", Path::new("/d/a.txt"));
        record(&dir, "alice", "a.txt", Path::new("/d/a (1).txt"));
        record(&dir, "bob", "b.txt", Path::new("/d/b.txt"));
        assert_eq!(lookup(&dir, "alice", "a.txt"), vec![PathBuf::from("/d/a (1).txt"), PathBuf::from("/d/a.txt")]);
        assert!(lookup(&dir, "bob", "a.txt").is_empty(), "another sender's delivery is invisible");
        assert!(lookup(&dir, "alice", "b.txt").is_empty());
        flush(&dir);
        forget_in_memory(&dir);
        assert_eq!(lookup(&dir, "alice", "a.txt").len(), 2, "survives a restart");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ledger_is_bounded() {
        let mut senders: HashMap<String, VecDeque<Entry>> = HashMap::new();
        let now = now_ms();
        senders.insert("old".into(), VecDeque::from(vec![Entry { name: "x".into(), path: "/x".into(), at: now - MAX_AGE_MS - 1 }]));
        let many: VecDeque<Entry> = (0..PER_SENDER + 10).map(|i| Entry { name: format!("{i}"), path: "/p".into(), at: now }).collect();
        senders.insert("big".into(), many);
        prune(&mut senders, now);
        assert!(!senders.contains_key("old"));
        assert_eq!(senders["big"].len(), PER_SENDER);
        assert_eq!(senders["big"].front().unwrap().name, "10", "oldest dropped first");
    }
}
