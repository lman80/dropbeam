//! integrity_v=1: SHA256("DropBeam integrity v1\0" || size:u64be ||
//! block_size:u64be || SHA256(block_0) || ... || SHA256(block_n)).
//! Blocks are fixed 4 MiB slices, last may be short; empty files have no leaves.
//! Fixed boundaries make the root independent of reads, stream order and resume.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const BLOCK: u64 = 4 * 1024 * 1024;
pub const ALGORITHM: &str = "SHA-256 / DropBeam blocks v1 (4 MiB)";
pub const FAILED: &str = "Verification failed — retry";
pub fn snapshot_leaves(leaves: &Leaves) -> BTreeMap<u64, [u8; 32]> { leaves.lock().unwrap().clone() }
pub type Leaves = Arc<Mutex<BTreeMap<u64, [u8; 32]>>>;

// Reporting is scoped to ONE transfer task, never to a thread or connection.
// Split batch sends share the scope; parallel workers receive explicit hash sinks.
tokio::task_local! {
    pub static REPORTS: Mutex<Vec<crate::models::FileIntegrity>>;
    pub static ITEM_OFFSET: u64;
    static REHASH: AtomicU64;
    static ACTIVITY_HOOK: Mutex<Option<Arc<dyn Fn(u64) + Send + Sync>>>;
}
pub fn item_offset() -> u64 { ITEM_OFFSET.try_with(|n| *n).unwrap_or(0) }
pub fn rehash_activity() -> u64 { REHASH.try_with(|n| n.load(Ordering::Relaxed)).unwrap_or(0) }
pub fn rehashed(n: u64) {
    let _ = REHASH.try_with(|a| a.fetch_add(n, Ordering::Relaxed));
    let _ = ACTIVITY_HOOK.try_with(|hook| { if let Some(hook) = hook.lock().unwrap().as_ref() { hook(n); } });
}
pub fn set_activity_hook(hook: Arc<dyn Fn(u64) + Send + Sync>) {
    let _ = ACTIVITY_HOOK.try_with(|h| *h.lock().unwrap() = Some(hook));
}
pub async fn scope<T>(future: impl std::future::Future<Output = T>) -> T {
    ACTIVITY_HOOK.scope(Mutex::new(None), REHASH.scope(AtomicU64::new(0), REPORTS.scope(Mutex::new(vec![]), future))).await
}
pub async fn ensure_scope<T>(future: impl std::future::Future<Output = T>) -> T {
    if REPORTS.try_with(|_| ()).is_ok() { future.await } else { scope(future).await }
}
pub fn reports() -> Vec<crate::models::FileIntegrity> {
    REPORTS.try_with(|r| r.lock().unwrap().clone()).unwrap_or_default()
}
pub fn record(rows: Vec<crate::models::FileIntegrity>) {
    let _ = REPORTS.try_with(|r| {
        let mut r = r.lock().unwrap();
        // A retry replaces only the same manifest item, including across splits.
        r.retain(|old| !rows.iter().any(|row| row.index == old.index && row.name == old.name));
        r.extend(rows);
    });
}
pub fn enabled(header: &serde_json::Value) -> bool { header["integrity_v"].as_u64() == Some(1) }
pub fn ready(header: &serde_json::Value, mut reply: serde_json::Value) -> serde_json::Value {
    if enabled(header) { reply["integrity_v"] = serde_json::json!(1); }
    reply
}

pub fn terminal(mut frame: serde_json::Value) -> serde_json::Value {
    let rows = reports();
    if !rows.is_empty() { frame["integrity"] = serde_json::json!(rows); }
    frame
}

// The read cap is 1 MiB. Leave ample room for terminal/protocol metadata.
pub const PAGE_BYTES: usize = 256 * 1024;
pub fn pages(kind: &str, field: &str, rows: impl serde::Serialize) -> Result<Vec<serde_json::Value>> {
    let rows = serde_json::to_value(rows)?.as_array().context("invalid integrity rows")?.clone();
    let mut pages = vec![];
    let mut page = vec![];
    let mut size = 128;
    for row in rows {
        let n = serde_json::to_vec(&row)?.len() + 1;
        anyhow::ensure!(n + 128 <= PAGE_BYTES, "integrity item exceeds page limit");
        if size + n > PAGE_BYTES {
            pages.push(serde_json::json!({"kind": kind, "integrity_v": 1, field: page, "more": true}));
            page = vec![]; size = 128;
        }
        page.push(row); size += n;
    }
    pages.push(serde_json::json!({"kind": kind, "integrity_v": 1, field: page, "more": false}));
    Ok(pages)
}
pub async fn send_hashes(send: &mut SendStream, hashes: &[FileHash]) -> Result<()> {
    let blocks: Vec<_> = hashes.iter().flat_map(|h| h.leaves.iter().map(move |(&offset, digest)|
        serde_json::json!({"index": h.index, "offset": offset, "digest": hex::encode(digest)}))).collect();
    if !blocks.is_empty() {
        for page in pages("integrity_blocks", "blocks", blocks)? { write_frame(send, &page).await?; rehashed(page["blocks"].as_array().unwrap().len() as u64); }
    }
    for page in pages("integrity", "files", hashes)? { write_frame(send, &page).await?; rehashed(page["files"].as_array().unwrap().len() as u64); }
    Ok(())
}
pub async fn write_terminal(send: &mut SendStream, frame: &serde_json::Value) -> Result<()> {
    let mut frame = frame.clone();
    if let Some(rows) = frame.as_object_mut().and_then(|f| f.remove("integrity")) {
        for page in pages("integrity_receipt", "integrity", rows)? { write_frame(send, &page).await?; }
        frame["integrity_done"] = serde_json::json!(true);
    }
    write_frame(send, &frame).await
}

#[cfg(test)]
tokio::task_local! { pub static STALL_BUDGET: Duration; }
pub fn stall_budget() -> Duration {
    #[cfg(test)] if let Ok(d) = STALL_BUDGET.try_with(|d| *d) { return d; }
    TRANSFER_STALL
}
pub struct Inactivity {
    last: tokio::time::Instant,
    work: u64,
}
impl Inactivity {
    pub fn new(activity: Option<&AtomicU64>) -> Self {
        Self { last: tokio::time::Instant::now(), work: activity.map(|a| a.load(Ordering::Relaxed)).unwrap_or(0).saturating_add(rehash_activity()) }
    }
    pub fn progress(&mut self) { self.last = tokio::time::Instant::now(); }
    pub fn check(&mut self, activity: Option<&AtomicU64>) -> Result<()> {
        let work = activity.map(|a| a.load(Ordering::Relaxed)).unwrap_or(0).saturating_add(rehash_activity());
        if work > self.work { self.work = work; self.progress(); }
        anyhow::ensure!(self.last.elapsed() < stall_budget(), "verification inactivity timeout");
        Ok(())
    }
}
async fn optional_frame(recv: &mut RecvStream) -> Result<Option<serde_json::Value>> {
    let mut len = [0; 4];
    let Some(n) = recv.read(&mut len).await? else { return Ok(None); };
    recv.read_exact(&mut len[n..]).await?;
    let n = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(n <= MAX_HEADER, "frame too large");
    let mut bytes = vec![0; n];
    recv.read_exact(&mut bytes).await?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}
async fn read_cancellable(recv: &mut RecvStream, cancel: &AtomicBool, deadline: &mut Inactivity) -> Result<Option<serde_json::Value>> {
    let frame = optional_frame(recv);
    tokio::pin!(frame);
    loop {
        tokio::select! {
            r = &mut frame => return r,
            _ = tokio::time::sleep(Duration::from_millis(20)) => {
                anyhow::ensure!(!cancel.load(Ordering::SeqCst), "canceled");
                deadline.check(None)?;
            },
        }
    }
}
#[derive(Debug)]
struct Mismatch { matched: Coverage }
impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(FAILED) }
}
impl std::error::Error for Mismatch {}
pub async fn verify_received(recv: &mut RecvStream, hashes: &[FileHash], cancel: &AtomicBool) -> Result<()> {
    verify_received_indexed(recv, hashes, cancel, true).await
}
pub async fn verify_received_indexed(recv: &mut RecvStream, hashes: &[FileHash], cancel: &AtomicBool, bind_index: bool) -> Result<()> {
    let mut remote: Vec<WireHash> = vec![];
    let mut blocks: BTreeMap<(u64, u64), [u8; 32]> = BTreeMap::new();
    let max_blocks: u64 = hashes.iter().map(|h| h.size.div_ceil(BLOCK)).sum();
    let mut blocks_complete = false;
    let mut deadline = Inactivity::new(None);
    loop {
        let Some(frame) = read_cancellable(recv, cancel, &mut deadline).await? else {
            // Body FIN won the capability race. With no extension started, both
            // sides complete Saved, unverified; truncated extensions are errors.
            anyhow::ensure!(remote.is_empty() && blocks.is_empty(), "incomplete integrity pages");
            return Ok(());
        };
        if remote.is_empty() && blocks.is_empty() && frame["kind"] == "integrity" && frame["verify"] == false { return Ok(()); }
        if frame["kind"] == "integrity_blocks" {
            anyhow::ensure!(enabled(&frame) && remote.is_empty() && !blocks_complete, "unexpected block continuation");
            let page = frame["blocks"].as_array().context("invalid block page")?;
            anyhow::ensure!(!page.is_empty(), "non-advancing integrity continuation");
            anyhow::ensure!(blocks.len() as u64 + page.len() as u64 <= max_blocks, "too many integrity blocks");
            for row in page {
                let index = row["index"].as_u64().context("missing block index")?;
                let offset = row["offset"].as_u64().context("missing block offset")?;
                let digest: [u8; 32] = hex::decode(row["digest"].as_str().context("missing block digest")?)?
                    .try_into().map_err(|_| anyhow::anyhow!("invalid block digest"))?;
                anyhow::ensure!(offset % BLOCK == 0 && blocks.insert((index, offset), digest).is_none(), "duplicate or unaligned block");
            }
            blocks_complete = frame["more"] != true;
            deadline.progress(); rehashed(page.len() as u64);
            continue;
        }
        anyhow::ensure!(blocks.is_empty() || blocks_complete, "incomplete block pages");
        anyhow::ensure!(frame["kind"] == "integrity" && enabled(&frame), "missing integrity manifest");
        let page: Vec<WireHash> = serde_json::from_value(frame["files"].clone())?;
        anyhow::ensure!(frame["more"] != true || !page.is_empty(), "non-advancing integrity continuation");
        anyhow::ensure!(remote.len() + page.len() <= hashes.len(), "invalid integrity manifest length");
        if !page.is_empty() { deadline.progress(); rehashed(page.len() as u64); }
        remote.extend(page);
        if frame["more"] != true { break; }
    }
    let (local, remote) = normalize_manifest(hashes, remote, bind_index)?;
    let rows = compare(&local, &remote)?;
    let mut matched = Coverage::default();
    for (a, b) in local.iter().zip(&remote) {
        let leaves: BTreeMap<_, _> = blocks.range((b.index, 0)..=(b.index, u64::MAX)).map(|((_, o), d)| (*o, *d)).collect();
        if !blocks.is_empty() {
            anyhow::ensure!(combine(b.size, &Arc::new(Mutex::new(leaves.clone())))? == b.digest, "block list does not match root");
            for (offset, digest) in leaves {
                if local.len() == 1 && a.leaves.get(&offset) == Some(&digest) { matched.insert(offset, (offset + BLOCK).min(a.size)); }
            }
        }
    }
    if !blocks.is_empty() { anyhow::ensure!(blocks.len() as u64 == max_blocks, "incomplete block list"); }
    let ok = rows.iter().all(|r| r.verified);
    record(rows);
    if !ok { return Err(Mismatch { matched }.into()); }
    Ok(())
}

pub async fn send_ack(send: &mut SendStream) -> Result<()> {
    write_frame(send, &serde_json::json!({"kind": "integrity_ack"})).await?;
    send.finish()?;
    Ok(())
}
/// Bytes are already saved. Only an application acknowledgement certifies that
/// the sender validated the receipt. Failure retains a Saved, unverified result.
pub fn unconfirmed() {
    let _ = REPORTS.try_with(|r| { for row in r.lock().unwrap().iter_mut() { row.acknowledged = false; } });
}
pub async fn receive_ack(recv: &mut RecvStream) {
    if reports().is_empty() { return; }
    let acknowledged = matches!(tokio::time::timeout(Duration::from_secs(10), read_frame(recv)).await,
        Ok(Ok(frame)) if frame["kind"] == "integrity_ack");
    let _ = REPORTS.try_with(|r| { for row in r.lock().unwrap().iter_mut() { row.acknowledged = acknowledged; } });
}

/// Validate the peer's receipt against OUR streaming hashes, not its claim alone.
pub fn receipt(frame: &serde_json::Value, hashes: &[FileHash]) -> Result<()> {
    if hashes.is_empty() && frame.get("integrity").is_none() { return Ok(()); }
    let rows: Vec<crate::models::FileIntegrity> = serde_json::from_value(frame["integrity"].clone())?;
    let wire: Vec<WireHash> = serde_json::from_value(frame["integrity"].clone())?;
    let (local, remote) = normalize_manifest(hashes, wire, true)?;
    let mut compared = compare(&local, &remote)?;
    for (row, expected) in rows.iter().zip(&compared) {
        anyhow::ensure!(row.algorithm == ALGORITHM && row.peer_digest == expected.digest && row.verified == expected.verified, "invalid integrity receipt");
    }
    let ok = compared.iter().all(|r| r.verified);
    for row in &mut compared { row.acknowledged = true; }
    record(compared);
    anyhow::ensure!(ok, "receiver: {FAILED}");
    Ok(())
}

pub async fn send_parallel<F: Fn(u64, u64)>(conn: &Connection, item: &(PathBuf, String, u64, u64),
    reply: &serde_json::Value, n: u64, cancel: &AtomicBool, pace: bool, progress: F, activity: &AtomicU64) -> Result<Vec<FileHash>> {
    let (base, legacy) = parse_resume_reply(Some(reply), item.2, n);
    if !enabled(reply) {
        send_ranges_parallel(conn, &item.0, item.2, base, &legacy, cancel, pace, progress).await?;
        return Ok(vec![]);
    }
    let ranges: Vec<(u64, u64)> = serde_json::from_value(reply["resume"]["have"].clone())?;
    let mut retained = Coverage::default();
    for (s, e) in ranges {
        anyhow::ensure!(s <= e && e <= item.2, "invalid integrity resume");
        retained.insert(s, e);
    }
    let aligned = aligned_coverage(&retained, item.2);
    anyhow::ensure!(aligned.ranges == retained.ranges, "unaligned integrity resume");
    let leaves = Leaves::default();
    let ranges = plan(&retained, item.2, n);
    // Open the first stream immediately, even when rechecking a huge resume.
    tokio::try_join!(
        hash_retained_progress(&item.0, &retained, leaves.clone(), cancel, |n| { activity.fetch_add(n, Ordering::SeqCst); }),
        send_ranges_hashed(conn, &item.0, item.2, retained.covered(), &ranges, cancel, pace, progress, Some(leaves.clone()))
    )?;
    Ok(vec![FileHash { sha256: None, leaves: snapshot_leaves(&leaves), index: item_offset(), name: item.1.clone(), size: item.2, digest: combine(item.2, &leaves)? }])
}

pub async fn receive_parallel<F: Fn(u64, u64)>(conn: &Connection, finalize: FinalizeDest, total: u64,
    part: PathBuf, resume: Option<ResumeCtx>, cov: Coverage, first: RecvStream, cancel: &AtomicBool,
    progress: F, header: &serde_json::Value, item_offset: u64, recv: &mut RecvStream) -> Result<PathBuf> {
    if !enabled(header) { return recv_file_resumable(conn, finalize, total, part, resume, cov, first, cancel, progress).await; }
    let leaves = Leaves::default();
    // Retain the partial and sidecar until BOTH hashing and verification finish.
    let retained = open_for_send(&part).await?;
    let progress = Mutex::new(progress);
    let landed = AtomicU64::new(cov.covered());
    let hashing = async {
        let result = hash_retained_file(retained, &cov, leaves.clone(), cancel, |n| {
            rehashed(n);
            progress.lock().unwrap()(landed.load(Ordering::Relaxed), total);
        }).await;
        if result.is_err() { cancel.store(true, Ordering::SeqCst); }
        result
    };
    let (hashed, received) = tokio::join!(
        hashing,
        recv_file_resumable_hashed(conn, FinalizeDest::Retain, total, part.clone(), resume.clone(), cov.clone(), first, cancel, |d, t| { landed.fetch_max(d, Ordering::Relaxed); progress.lock().unwrap()(d, t); }, Some(leaves.clone()))
    );
    hashed?;
    let path = received?;
    let hash = FileHash { sha256: None, leaves: snapshot_leaves(&leaves), index: received_item_index(item_offset, 0), name: header["items"][0]["name"].as_str().context("missing integrity name")?.into(), size: total, digest: combine(total, &leaves)? };
    // Revoke old coverage BEFORE verification. A failed invalidation save can
    // never leave a fully-covered corrupt sidecar available to the next resume.
    if let Some(rc) = &resume {
        revoke_sidecar(&rc.side)?;
    }
    if let Err(e) = verify_received_indexed(recv, &[hash], cancel, header.get("chatTransfer").is_some() || header.get("location_item_offset").is_some()).await {
        if let Some(rc) = &resume {
            let coverage = e.downcast_ref::<Mismatch>().map(|m| m.matched.clone()).unwrap_or_default();
            save_sidecar_checked(&rc.side, &PartialSidecar { v: 1, fp: rc.fp.clone(), total, coverage })
                .context("invalidated coverage could not be persisted; resume refused, next attempt starts clean")?;
        }
        return Err(e);
    }
    finalize_received(finalize, path, resume.as_ref())
}

pub struct Blocks {
    offset: u64,
    used: u64,
    hash: ring::digest::Context,
    leaves: Leaves,
}
impl Blocks {
    pub fn new(offset: u64, leaves: Leaves) -> Result<Self> {
        anyhow::ensure!(offset % BLOCK == 0, "unaligned integrity range");
        Ok(Self { offset, used: 0, hash: ring::digest::Context::new(&ring::digest::SHA256), leaves })
    }
    pub fn update(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = bytes.len().min((BLOCK - self.used) as usize);
            self.hash.update(&bytes[..n]);
            self.used += n as u64;
            bytes = &bytes[n..];
            if self.used == BLOCK { self.flush(); }
        }
    }
    fn flush(&mut self) {
        let hash = std::mem::replace(&mut self.hash, ring::digest::Context::new(&ring::digest::SHA256));
        self.leaves.lock().unwrap().insert(self.offset, hash.finish().as_ref().try_into().unwrap());
        self.offset += self.used;
        self.used = 0;
    }
    pub fn finish(mut self) { if self.used > 0 { self.flush(); } }
}
pub fn combine(size: u64, leaves: &Leaves) -> Result<String> {
    let leaves = leaves.lock().unwrap();
    anyhow::ensure!(leaves.len() as u64 == size.div_ceil(BLOCK), "incomplete integrity blocks");
    let mut hash = Sha256::new();
    hash.update(b"DropBeam integrity v1\0");
    hash.update(size.to_be_bytes());
    hash.update(BLOCK.to_be_bytes());
    for offset in (0..size).step_by(BLOCK as usize) {
        hash.update(leaves.get(&offset).context("missing integrity block")?);
    }
    Ok(hex::encode(hash.finalize()))
}

/// Only whole canonical blocks are retained; an incomplete block is retransmitted.
pub fn aligned_coverage(cov: &Coverage, total: u64) -> Coverage {
    let mut out = Coverage::default();
    for &(s, e) in &cov.ranges {
        let start = s.div_ceil(BLOCK) * BLOCK;
        let end = if e == total { e } else { e / BLOCK * BLOCK };
        if end > start { out.insert(start, end); }
    }
    out
}
pub fn plan(cov: &Coverage, total: u64, streams: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<_> = cov.missing(total).into_iter().map(|(s, e)| (s, e - s)).collect();
    while out.len() < streams as usize {
        let Some((i, &(start, len))) = out.iter().enumerate().max_by_key(|(_, (_, n))| *n) else { break; };
        let half = (len / 2) / BLOCK * BLOCK;
        if half == 0 { break; }
        out[i] = (start, half);
        out.insert(i + 1, (start + half, len - half));
    }
    // An empty stream signals that a fully retained partial can be finalized.
    if out.is_empty() { out.push((0, 0)); }
    out
}
// A resumed attempt must independently check the bytes that it skips sending.
#[cfg(test)]
tokio::task_local! { pub static HASH_DELAY: Duration; }

#[cfg(test)]
pub async fn hash_retained(path: &Path, cov: &Coverage, leaves: Leaves, cancel: &AtomicBool) -> Result<()> {
    hash_retained_progress(path, cov, leaves, cancel, |_| {}).await
}
pub async fn hash_retained_progress(path: &Path, cov: &Coverage, leaves: Leaves, cancel: &AtomicBool, activity: impl Fn(u64)) -> Result<()> {
    if cov.ranges.is_empty() { return Ok(()); }
    hash_retained_file(open_for_send(path).await?, cov, leaves, cancel, activity).await
}

async fn hash_retained_file(mut file: tokio::fs::File, cov: &Coverage, leaves: Leaves, cancel: &AtomicBool, activity: impl Fn(u64)) -> Result<()> {
    use tokio::io::AsyncSeekExt;
    let mut buf = vec![0; CHUNK];
    for &(start, end) in &cov.ranges {
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut blocks = Blocks::new(start, leaves.clone())?;
        let mut left = end - start;
        while left > 0 {
            anyhow::ensure!(!cancel.load(Ordering::SeqCst), "canceled");
            let want = left.min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..want]).await?;
            anyhow::ensure!(n > 0, "retained integrity range ended early");
            blocks.update(&buf[..n]);
            left -= n as u64;
            activity(n as u64);
            #[cfg(test)]
            if let Ok(delay) = HASH_DELAY.try_with(|d| *d) { tokio::time::sleep(delay).await; }
        }
        blocks.finish();
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct FileHash { #[serde(default)] pub sha256: Option<String>, pub index: u64, pub name: String, pub size: u64, pub digest: String, #[serde(skip)] pub leaves: BTreeMap<u64, [u8; 32]> }

// Missing is distinct from explicit zero (and null is invalid). Only a wholly
// indexless legacy list may acquire indices from the validated manifest order.
#[derive(serde::Deserialize)]
struct WireHash {
    #[serde(default, deserialize_with = "present_index")]
    index: Option<u64>,
    name: String,
    size: u64,
    digest: String,
    #[serde(default)]
    sha256: Option<String>,
}
fn present_index<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<u64>, D::Error> {
    <u64 as serde::Deserialize>::deserialize(d).map(Some)
}
fn normalize_manifest(hashes: &[FileHash], remote: Vec<WireHash>, bind_index: bool) -> Result<(Vec<FileHash>, Vec<FileHash>)> {
    anyhow::ensure!(hashes.len() == remote.len(), "invalid integrity manifest length");
    let indexless = remote.iter().all(|row| row.index.is_none());
    anyhow::ensure!(indexless || remote.iter().all(|row| row.index.is_some()), "mixed integrity index presence");
    let mut local = hashes.to_vec();
    let base = remote.first().and_then(|row| row.index).unwrap_or(0);
    let mut normalized = Vec::with_capacity(remote.len());
    for (i, (expected, row)) in local.iter_mut().zip(remote).enumerate() {
        anyhow::ensure!(expected.name == row.name && expected.size == row.size, "invalid integrity manifest order");
        let index = row.index.unwrap_or(expected.index);
        if !indexless && !bind_index {
            anyhow::ensure!(base.checked_add(i as u64) == Some(index), "non-contiguous integrity indices");
            expected.index = index;
        }
        anyhow::ensure!(index == expected.index, "invalid integrity manifest index");
        normalized.push(FileHash { sha256: row.sha256, index, name: row.name, size: row.size, digest: row.digest, leaves: Default::default() });
    }
    Ok((local, normalized))
}

pub fn compare(local: &[FileHash], remote: &[FileHash]) -> Result<Vec<crate::models::FileIntegrity>> {
    anyhow::ensure!(local.len() == remote.len(), "invalid integrity manifest length");
    local.iter().zip(remote).map(|(a, b)| {
        anyhow::ensure!(a.index == b.index && a.name == b.name && a.size == b.size && b.digest.len() == 64
            && b.digest.bytes().all(|c| c.is_ascii_hexdigit()), "invalid integrity manifest");
        let verified = a.digest == b.digest && (a.sha256.is_none() || b.sha256.is_none() || a.sha256 == b.sha256);
        if !verified {
            // Index/name/path are deliberately excluded. Digests and sizes suffice.
            log::warn!("INTEGRITY-MISMATCH size_local={} size_peer={} local={} peer={} algorithm={ALGORITHM}", a.size, b.size, a.digest, b.digest);
        }
        Ok(crate::models::FileIntegrity { sha256: b.sha256.clone().or_else(|| a.sha256.clone()), index: a.index, acknowledged: false, name: a.name.clone(), size: a.size, algorithm: ALGORITHM.into(),
            digest: a.digest.clone(), peer_digest: b.digest.clone(), verified })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_plain_sha256_manifest_roundtrip() {
        for sha256 in [None, Some("a".repeat(64))] {
            let hash = FileHash { sha256: sha256.clone(), index: 0, name: "file".into(), size: 0, digest: "b".repeat(64), leaves: Default::default() };
            let mut value = serde_json::to_value(&hash).unwrap();
            if sha256.is_none() { value.as_object_mut().unwrap().remove("sha256"); }
            assert_eq!(serde_json::from_value::<FileHash>(value.clone()).unwrap(), hash);
            assert_eq!(serde_json::from_value::<WireHash>(value).unwrap().sha256, sha256);
            let rows = compare(&[hash.clone()], &[hash]).unwrap();
            let mut value = serde_json::to_value(&rows[0]).unwrap();
            if sha256.is_none() { value.as_object_mut().unwrap().remove("sha256"); }
            assert_eq!(serde_json::from_value::<crate::models::FileIntegrity>(value).unwrap().sha256, sha256);
        }
    }

    #[tokio::test]
    async fn indexless_two_file_manifests_and_receipts_verify_in_manifest_order() {
        scope(async {
            let hashes: Vec<_> = (0..2).map(|i| FileHash { sha256: None, index: 7 + i, name: format!("{i}.bin"),
                size: i, digest: format!("{i}").repeat(64), leaves: Default::default() }).collect();
            let mut wire = serde_json::to_value(&hashes).unwrap();
            for row in wire.as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("index"); }
            for bind in [false, true] {
                let (local, remote) = normalize_manifest(&hashes, serde_json::from_value(wire.clone()).unwrap(), bind).unwrap();
                let rows = compare(&local, &remote).unwrap();
                assert!(rows.iter().all(|r| r.verified));
                assert_eq!(rows.iter().map(|r| r.index).collect::<Vec<_>>(), vec![7, 8]);
                let mut receipt_rows = serde_json::to_value(rows).unwrap();
                for row in receipt_rows.as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("index"); }
                receipt(&serde_json::json!({"integrity":receipt_rows}), &hashes).unwrap();
                assert!(reports().iter().all(|r| r.verified && r.acknowledged));
            }
            let mut reversed = wire.clone(); reversed.as_array_mut().unwrap().reverse();
            assert!(normalize_manifest(&hashes, serde_json::from_value(reversed).unwrap(), false).is_err());
            for indices in [serde_json::json!([0,0]), serde_json::json!([8,7]), serde_json::json!([7,9])] {
                let mut invalid = wire.clone();
                for (row, index) in invalid.as_array_mut().unwrap().iter_mut().zip(indices.as_array().unwrap()) { row["index"] = index.clone(); }
                for bind in [false, true] {
                    assert!(normalize_manifest(&hashes, serde_json::from_value(invalid.clone()).unwrap(), bind).is_err());
                }
            }
            let mut mixed = wire.clone(); mixed[0]["index"] = serde_json::json!(7);
            assert!(normalize_manifest(&hashes, serde_json::from_value(mixed).unwrap(), true).is_err());
            let mut null = wire; null[0]["index"] = serde_json::Value::Null;
            assert!(serde_json::from_value::<Vec<WireHash>>(null).is_err());
            let rows = compare(&hashes, &hashes).unwrap();
            let mut invalid_receipt = serde_json::to_value(rows).unwrap();
            invalid_receipt[1]["index"] = serde_json::json!(0);
            assert!(receipt(&serde_json::json!({"integrity":invalid_receipt}), &hashes).is_err());
        }).await;
    }

    #[test]
    fn combine_is_order_and_chunk_independent() {
        let bytes = vec![37u8; BLOCK as usize * 2 + 91];
        let a = Leaves::default();
        let mut stream = Blocks::new(0, a.clone()).unwrap();
        for chunk in bytes.chunks(7919) { stream.update(chunk); }
        stream.finish();
        let b = Leaves::default();
        for i in (0..3).rev() {
            let start = i * BLOCK as usize;
            let mut range = Blocks::new(start as u64, b.clone()).unwrap();
            range.update(&bytes[start..bytes.len().min(start + BLOCK as usize)]);
            range.finish();
        }
        assert_eq!(combine(bytes.len() as u64, &a).unwrap(), combine(bytes.len() as u64, &b).unwrap());
        b.lock().unwrap().get_mut(&0).unwrap()[0] ^= 1;
        assert_ne!(combine(bytes.len() as u64, &a).unwrap(), combine(bytes.len() as u64, &b).unwrap());
        assert!(combine(BLOCK, &Leaves::default()).is_err());
        assert_eq!(combine(0, &Leaves::default()).unwrap(), hex::encode(Sha256::digest([b"DropBeam integrity v1\0".as_slice(), &0u64.to_be_bytes(), &BLOCK.to_be_bytes()].concat())));
    }
    #[test]
    fn resume_alignment_covers_file() {
        let total = 5 * BLOCK + 31;
        let cov = aligned_coverage(&Coverage { ranges: vec![(1, BLOCK * 3 + 7), (BLOCK * 4, total)] }, total);
        assert_eq!(cov.ranges, vec![(BLOCK, BLOCK * 3), (BLOCK * 4, total)]);
        let mut all = cov.clone();
        for (start, len) in plan(&cov, total, 4) { assert_eq!(start % BLOCK, 0); all.insert(start, start + len); }
        assert_eq!(all.covered(), total);
    }

    #[test]
    fn roots_match_independent_sha256_and_bind_size() {
        for size in [1, BLOCK as usize, BLOCK as usize + 1] {
            let bytes = vec![0xa5; size];
            let leaves = Leaves::default();
            let mut blocks = Blocks::new(0, leaves.clone()).unwrap();
            blocks.update(&bytes);
            blocks.finish();
            let mut reference = Sha256::new();
            reference.update(b"DropBeam integrity v1\0");
            reference.update((size as u64).to_be_bytes());
            reference.update(BLOCK.to_be_bytes());
            for block in bytes.chunks(BLOCK as usize) { reference.update(Sha256::digest(block)); }
            assert_eq!(combine(size as u64, &leaves).unwrap(), hex::encode(reference.finalize()));
            if size > 1 { assert_ne!(combine(size as u64, &leaves).ok(), combine(size as u64 - 1, &leaves).ok()); }
        }
    }

    #[test]
    fn receipts_cannot_certify_missing_or_different_digests() {
        let local = FileHash { sha256: None, leaves: Default::default(), index: 0, name: "a".into(), size: 1, digest: "a".repeat(64) };
        assert!(receipt(&serde_json::json!({"ok": true}), &[local.clone()]).is_err());
        let mut rows = compare(&[local.clone()], &[local.clone()]).unwrap();
        assert!(receipt(&serde_json::json!({"integrity": rows}), &[local.clone()]).is_ok());
        rows[0].digest = "b".repeat(64); // A claimed check mark cannot hide a mismatch.
        assert!(receipt(&serde_json::json!({"integrity": rows}), &[local.clone()]).is_err());
        rows[0].verified = false;
        assert!(receipt(&serde_json::json!({"integrity": rows}), &[local]).unwrap_err().to_string().contains(FAILED));
    }

    #[tokio::test]
    async fn duplicate_names_keep_both_ordered_receipts() {
        scope(async {
            let hashes = vec![
                FileHash { sha256: None, leaves: Default::default(), index: 0, name: "same.bin".into(), size: 1, digest: "a".repeat(64) },
                FileHash { sha256: None, leaves: Default::default(), index: 1, name: "same.bin".into(), size: 2, digest: "b".repeat(64) },
            ];
            record(compare(&hashes, &hashes).unwrap());
            assert_eq!(reports().len(), 2);
            receipt(&terminal(serde_json::json!({"ok": true})), &hashes).unwrap();
            assert_eq!(reports().len(), 2);
        }).await;
    }
    #[tokio::test]
    async fn successive_splits_and_retry_preserve_duplicate_names() {
        scope(async {
            let first = FileHash { sha256: None, leaves: Default::default(), index: 0, name: "same.bin".into(), size: 1, digest: "a".repeat(64) };
            let second = FileHash { sha256: None, leaves: Default::default(), index: 1, name: "same.bin".into(), size: 2, digest: "b".repeat(64) };
            for hash in [&first, &second, &first] {
                let rows = compare(std::slice::from_ref(hash), std::slice::from_ref(hash)).unwrap();
                receipt(&serde_json::json!({"integrity": rows}), std::slice::from_ref(hash)).unwrap();
            }
            assert_eq!(reports().len(), 2);
            assert!(reports().iter().all(|r| r.verified && r.acknowledged));
            assert_eq!(reports().iter().map(|r| r.size).sum::<u64>(), 3);
        }).await;
    }

    #[test]
    fn five_thousand_file_pages_fit_read_cap_and_roundtrip() {
        let hashes: Vec<_> = (0..5000).map(|i| FileHash { sha256: None, leaves: Default::default(), index: i, name: format!("file-{i}.bin"), size: i, digest: "a".repeat(64) }).collect();
        let rows = compare(&hashes, &hashes).unwrap();
        for (kind, field, values) in [("integrity", "files", serde_json::to_value(&hashes).unwrap()),
            ("integrity_receipt", "integrity", serde_json::to_value(&rows).unwrap())] {
            let pages = pages(kind, field, values.clone()).unwrap();
            assert!(pages.len() > 1);
            let mut joined = vec![];
            for (i, page) in pages.iter().enumerate() {
                let encoded = serde_json::to_vec(page).unwrap();
                assert!(encoded.len() <= PAGE_BYTES && encoded.len() < MAX_HEADER);
                let decoded: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
                assert_eq!(decoded["more"], i + 1 < pages.len());
                joined.extend(decoded[field].as_array().unwrap().iter().cloned());
            }
            assert_eq!(serde_json::json!(joined), values);
        }
    }

}
