//! "Verify copy" — a full SHA-256 comparison of a finished SEND against the copy
//! that actually landed on the peer (a friend's download folder, or a Location
//! host's shared folder).
//!
//! The transport lives in `iroh_net` (`files.verify` / `locations.verify`); this
//! module owns the parts that are pure: what a card needs in order to be
//! re-checked, how the work is chunked, and how two digests become a verdict.

use std::ops::Range;
use std::path::PathBuf;

use serde::Serialize;

/// At most this many files ride in one request frame — the same bound
/// `files.stat` / `locations.stat` use, so a reply frame stays small and a
/// 17,000-file folder becomes 18 round trips instead of 17,000.
pub const MAX_ITEMS: usize = 1000;

/// How many finished sends stay verifiable. A card the user can still see is
/// almost always one of the last few, and the record is tiny (paths + names).
const MAX_RECORDS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyState {
    Running,
    Done,
    Failed,
    Canceled,
}

/// What the card shows while (and after) a verification runs. Rides the SAME
/// `transfer://update` event as everything else, on the transfer's own id.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub state: VerifyState,
    /// Files whose verdict is settled so far.
    pub checked: u64,
    pub total: u64,
    /// Bytes hashed, averaged over the two sides: both devices read every byte,
    /// so the mean is a true fraction of the whole job while `bytes_total` stays
    /// the folder size the user recognises.
    pub bytes_hashed: u64,
    pub bytes_total: u64,
    pub mismatched: Vec<String>,
    pub missing: Vec<String>,
    pub error: Option<String>,
}

impl VerifyReport {
    pub fn running(total: u64, bytes_total: u64) -> Self {
        VerifyReport {
            state: VerifyState::Running,
            checked: 0,
            total,
            bytes_hashed: 0,
            bytes_total,
            mismatched: vec![],
            missing: vec![],
            error: None,
        }
    }
}

/// Everything a finished send needs to be re-checked later: the same files and
/// the same destination the send itself used (the engine-side twin of the
/// frontend's retry payload, captured where the send is planned so a folder is
/// never re-walked into a different item list).
#[derive(Debug, Clone)]
pub struct VerifyRecord {
    pub endpoint_id: String,
    /// `Some` for a Location/NAS upload, `None` for a plain friend send.
    pub target: Option<crate::locations::Target>,
    /// (local source path, peer-relative name, size in bytes)
    pub items: Vec<(PathBuf, String, u64)>,
}

impl VerifyRecord {
    pub fn bytes_total(&self) -> u64 {
        self.items.iter().map(|i| i.2).sum()
    }
}

/// Keep the newest `MAX_RECORDS` entries, oldest evicted first.
pub fn remember<T>(records: &mut Vec<(String, T)>, id: &str, value: T) {
    records.retain(|(existing, _)| existing != id);
    if records.len() >= MAX_RECORDS {
        records.remove(0);
    }
    records.push((id.to_string(), value));
}

/// One file's verdict inputs: what we hashed locally, what the peer reported for
/// the requested name, and (Locations only) what it reported for the `name (2)`
/// sibling a host publishes when the original name already held other content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRow {
    pub name: String,
    pub local: Option<String>,
    pub remote: Option<String>,
    pub alt: Option<String>,
}

impl VerifyRow {
    pub fn new(name: &str, local: Option<String>, remote: Option<String>) -> Self {
        VerifyRow { name: name.to_string(), local, remote, alt: None }
    }
    /// A digest we hold locally that the peer reported identically (under either
    /// the requested name or its `(2)` sibling).
    pub fn identical(&self) -> bool {
        self.local.is_some() && (self.local == self.remote || self.local == self.alt)
    }
}

/// (mismatched, missing) — the whole verdict, as a pure function of the digests.
///
/// * the peer has no copy under either name → **missing**
/// * the peer has a copy whose bytes differ → **mismatched**
/// * the source can no longer be read → **mismatched**: we cannot claim identical
///   for a file we never hashed.
pub fn compare(rows: &[VerifyRow]) -> (Vec<String>, Vec<String>) {
    let mut mismatched = Vec::new();
    let mut missing = Vec::new();
    for row in rows {
        if row.identical() {
            continue;
        }
        if row.remote.is_none() && row.alt.is_none() {
            missing.push(row.name.clone());
        } else {
            mismatched.push(row.name.clone());
        }
    }
    (mismatched, missing)
}

/// Rows that a `(2)`-sibling probe could still rescue: everything not yet proven
/// identical, before any `alt` digest has been fetched.
pub fn unresolved(rows: &[VerifyRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.alt.is_none() && !row.identical())
        .map(|(i, _)| i)
        .collect()
}

/// Split `len` items into request-sized ranges.
pub fn chunks(len: usize) -> Vec<Range<usize>> {
    (0..len)
        .step_by(MAX_ITEMS)
        .map(|start| start..(start + MAX_ITEMS).min(len))
        .collect()
}

/// The `name (2).ext` sibling a Location host publishes beside an existing file
/// of the same name — matching `locations::sibling_name(name, 1)`.
pub fn alt_name(name: &str) -> String {
    let path = std::path::Path::new(name);
    let parent = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let leaf = format!("{stem} (2){ext}");
    if parent.is_empty() { leaf } else { format!("{parent}/{leaf}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Option<String> {
        Some(format!("{byte:02x}").repeat(32))
    }

    #[test]
    fn compare_sorts_every_outcome() {
        let rows = vec![
            VerifyRow::new("same", digest(1), digest(1)),
            VerifyRow::new("changed", digest(1), digest(2)),
            VerifyRow::new("gone", digest(1), None),
            // Source unreadable now: we cannot certify it, and it is not "missing
            // on the peer" either.
            VerifyRow::new("unreadable-source", None, digest(2)),
            VerifyRow::new("both-gone", None, None),
        ];
        let (mismatched, missing) = compare(&rows);
        assert_eq!(mismatched, vec!["changed", "unreadable-source"]);
        assert_eq!(missing, vec!["gone", "both-gone"]);
    }

    #[test]
    fn a_conflict_sibling_counts_as_identical() {
        let mut row = VerifyRow::new("clips/a.mp4", digest(1), digest(2));
        assert!(!row.identical());
        assert_eq!(unresolved(std::slice::from_ref(&row)), vec![0]);
        row.alt = digest(1);
        assert!(row.identical());
        assert_eq!(compare(&[row]), (vec![], vec![]));
    }

    #[test]
    fn unresolved_skips_matches_and_already_probed_rows() {
        let rows = vec![
            VerifyRow::new("ok", digest(1), digest(1)),
            VerifyRow::new("bad", digest(1), digest(2)),
            VerifyRow { alt: digest(3), ..VerifyRow::new("probed", digest(1), digest(2)) },
            VerifyRow::new("absent", digest(1), None),
        ];
        assert_eq!(unresolved(&rows), vec![1, 3]);
    }

    #[test]
    fn chunking_never_exceeds_the_frame_bound() {
        assert!(chunks(0).is_empty());
        assert_eq!(chunks(3), vec![0..3]);
        assert_eq!(chunks(MAX_ITEMS), vec![0..MAX_ITEMS]);
        assert_eq!(chunks(MAX_ITEMS + 1), vec![0..MAX_ITEMS, MAX_ITEMS..MAX_ITEMS + 1]);
        let big = chunks(17_587);
        assert_eq!(big.len(), 18);
        assert!(big.iter().all(|r| r.len() <= MAX_ITEMS));
        assert_eq!(big.iter().map(|r| r.len()).sum::<usize>(), 17_587);
        assert_eq!(big.last().unwrap().end, 17_587);
    }

    #[test]
    fn alt_names_match_the_hosts_sibling_naming() {
        assert_eq!(alt_name("a.mp4"), "a (2).mp4");
        assert_eq!(alt_name("clips/a.mp4"), "clips/a (2).mp4");
        assert_eq!(alt_name("folder/data"), "folder/data (2)");
        assert_eq!(alt_name("archive.tar.gz"), "archive.tar (2).gz");
    }

    #[test]
    fn records_are_bounded_and_replace_by_id() {
        let mut records: Vec<(String, u32)> = Vec::new();
        for i in 0..(MAX_RECORDS as u32 + 10) {
            remember(&mut records, &i.to_string(), i);
        }
        assert_eq!(records.len(), MAX_RECORDS);
        assert_eq!(records[0].0, "10");
        remember(&mut records, "10", 99);
        assert_eq!(records.len(), MAX_RECORDS);
        assert_eq!(records.last().unwrap(), &("10".to_string(), 99));
    }
}
