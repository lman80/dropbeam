//! Open in place (docs/locations-v2.md): a client-side block cache over the
//! host's `locations.read`, and the 127.0.0.1 range server that in-app
//! previews, players (AVPlayer, <video>) and the drive (drive.rs) read through.
//!
//! Blocks are 1 MiB files on disk under the app cache dir, keyed by
//! (device, location, path, etag) so a changed file can never serve stale
//! bytes, evicted least-recently-used beyond the size setting (default 10 GB).
//! Concurrent readers of one block share one request; sequential reads grow a
//! read-ahead window; misses become several range requests at once, each on
//! its own QUIC stream of the friend's existing connection. Which path that
//! connection uses (Local / Direct / Relay) is iroh's selector, unchanged.
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap, VecDeque}, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};
use tokio::sync::{mpsc, watch, Semaphore};

pub const BLOCK: u64 = 1 << 20;
/// Blocks per range request (4 MiB): small enough that several run at once on a
/// seek, big enough that a sequential read is a handful of streams in flight.
const REQUEST_BLOCKS: u64 = 4;
/// Range requests in flight per device.
const PARALLEL: usize = 8;
const READAHEAD_START: u64 = 2;
const READAHEAD_MAX: u64 = 64;
pub const DEFAULT_CACHE_BYTES: u64 = 10_000_000_000;
const STAT_TTL: Duration = Duration::from_secs(10);

/// One file in a friend's Location.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileRef { pub endpoint: String, pub location_id: String, pub rel_path: String }

#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stat { pub is_dir: bool, pub size: u64, pub modified: u64, pub etag: String }

/// One page-merged listing entry (locations.ls).
#[derive(Clone, Debug, serde::Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry { pub name: String, pub is_dir: bool, pub size: u64, pub modified: u64 }

/// The other device, as the engine and the drive see it. The real one is
/// `IrohRemote`; tests use an in-memory host.
#[async_trait::async_trait]
pub trait Remote: Send + Sync + 'static {
    async fn stat(&self, f: &FileRef) -> Result<Stat>;
    /// The bytes of `[offset, offset+len)` as consecutive chunks of any size.
    async fn read(&self, f: &FileRef, offset: u64, len: u64, etag: &str) -> Result<mpsc::Receiver<Result<Bytes>>>;
    /// Any other Locations request (list, ls, mkdir, rename, trash, thumb).
    async fn rpc(&self, endpoint: &str, request: Value) -> Result<Value>;
    /// Every friend device that may share Locations: (friend id, endpoint, name).
    fn devices(&self) -> Vec<(String, String, String)>;
    /// Publish a local file into `rel_dir` through the verified upload path.
    async fn upload(&self, endpoint: &str, location_id: &str, rel_dir: &str, file: &Path, replace: bool) -> Result<String>;
    /// ("local" | "direct" | "relay" | "connecting", round-trip ms) right now.
    fn path(&self, endpoint: &str) -> Option<(String, Option<u64>)>;
}

type BlockId = (String, u64);

/// Least-recently-used index of the blocks on disk.
#[derive(Default)]
struct Lru { map: HashMap<BlockId, (u64, u64)>, order: BTreeMap<u64, BlockId>, total: u64, tick: u64 }
impl Lru {
    fn touch(&mut self, id: &BlockId) -> bool {
        let Some((_, tick)) = self.map.get_mut(id) else { return false; };
        self.order.remove(tick);
        self.tick += 1; *tick = self.tick;
        self.order.insert(self.tick, id.clone());
        true
    }
    fn insert(&mut self, id: BlockId, size: u64) {
        if self.touch(&id) { return; }
        self.tick += 1;
        self.order.insert(self.tick, id.clone());
        self.map.insert(id, (size, self.tick));
        self.total += size;
    }
    fn remove(&mut self, id: &BlockId) -> bool {
        let Some((size, tick)) = self.map.remove(id) else { return false; };
        self.order.remove(&tick); self.total -= size; true
    }
    /// Oldest blocks to delete so the total fits `max`.
    fn evict(&mut self, max: u64) -> Vec<BlockId> {
        let mut out = Vec::new();
        while self.total > max {
            let Some((_, id)) = self.order.pop_first() else { break; };
            if let Some((size, _)) = self.map.remove(&id) { self.total -= size; }
            out.push(id);
        }
        out
    }
    fn remove_key(&mut self, key: &str) -> Vec<BlockId> {
        let ids: Vec<_> = self.map.keys().filter(|(k, _)| k == key).cloned().collect();
        ids.into_iter().filter(|id| self.remove(id)).collect()
    }
}

#[derive(Default)]
struct Seq { next: u64, window: u64 }
#[derive(Default)]
struct Meter { samples: VecDeque<(Instant, u64)>, total: u64 }
impl Meter {
    fn add(&mut self, n: u64) {
        let now = Instant::now();
        self.samples.push_back((now, n)); self.total += n;
        while self.samples.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(3)) { self.samples.pop_front(); }
    }
    fn rate(&self) -> u64 {
        let now = Instant::now();
        let recent: u64 = self.samples.iter().filter(|(t, _)| now.duration_since(*t) <= Duration::from_secs(2)).map(|(_, n)| n).sum();
        recent / 2
    }
}

/// What the UI shows while a file streams: the path badge and live speed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStatus { pub path: String, pub rtt_ms: Option<u64>, pub bytes_per_sec: u64, pub received: u64, pub cached_bytes: u64, pub cache_limit: u64 }

pub struct Engine {
    remote: Arc<dyn Remote>,
    dir: PathBuf,
    max_bytes: AtomicU64,
    lru: Mutex<Lru>,
    inflight: Mutex<HashMap<BlockId, watch::Receiver<Option<Result<(), String>>>>>,
    stats: Mutex<HashMap<FileRef, (Stat, Instant)>>,
    seq: Mutex<HashMap<String, Seq>>,
    limits: Mutex<HashMap<String, Arc<Semaphore>>>,
    meters: Mutex<HashMap<String, Meter>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|p| p.into_inner()) }
pub(crate) fn is_changed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<crate::locations::LocationError>() == Some(&crate::locations::LocationError::Changed)
}
fn file_key(f: &FileRef, etag: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(format!("{}\0{}\0{}\0{etag}", f.endpoint, f.location_id, f.rel_path)))[..32].to_string()
}

impl Engine {
    pub fn new(remote: Arc<dyn Remote>, dir: PathBuf, max_bytes: u64) -> Arc<Self> {
        let engine = Arc::new(Self { remote, dir, max_bytes: AtomicU64::new(max_bytes), lru: Default::default(), inflight: Default::default(),
            stats: Default::default(), seq: Default::default(), limits: Default::default(), meters: Default::default() });
        engine.scan();
        engine
    }
    pub fn remote(&self) -> &Arc<dyn Remote> { &self.remote }
    /// Rebuild the index from the blocks a previous run left, oldest first.
    fn scan(&self) {
        let mut found = Vec::new();
        for shard in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            for e in std::fs::read_dir(shard.path()).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let parsed = name.rsplit_once('-').and_then(|(k, i)| Some((k.to_string(), i.parse::<u64>().ok()?)));
                match (parsed, e.metadata()) {
                    (Some(id), Ok(m)) if m.is_file() && id.0.len() == 32 => found.push((m.modified().ok(), id, m.len())),
                    _ => { let _ = std::fs::remove_file(e.path()); }
                }
            }
        }
        found.sort_by_key(|(t, _, _)| *t);
        let mut lru = lock(&self.lru);
        for (_, id, size) in found { lru.insert(id, size); }
        let max = self.max_bytes.load(Ordering::Relaxed);
        for id in lru.evict(max) { let _ = std::fs::remove_file(self.block_path(&id)); }
    }
    fn block_path(&self, (key, idx): &BlockId) -> PathBuf { self.dir.join(&key[..2]).join(format!("{key}-{idx}")) }
    pub fn set_limit(&self, bytes: u64) {
        self.max_bytes.store(bytes, Ordering::Relaxed);
        let evicted = lock(&self.lru).evict(bytes);
        for id in evicted { let _ = std::fs::remove_file(self.block_path(&id)); }
    }
    pub fn cached_bytes(&self) -> u64 { lock(&self.lru).total }
    pub fn clear(&self) {
        let evicted = lock(&self.lru).evict(0);
        for id in evicted { let _ = std::fs::remove_file(self.block_path(&id)); }
    }
    fn limiter(&self, endpoint: &str) -> Arc<Semaphore> {
        lock(&self.limits).entry(endpoint.into()).or_insert_with(|| Arc::new(Semaphore::new(PARALLEL))).clone()
    }
    pub fn status(&self, endpoint: &str) -> StreamStatus {
        let (rate, received) = lock(&self.meters).get(endpoint).map(|m| (m.rate(), m.total)).unwrap_or((0, 0));
        let (path, rtt_ms) = self.remote.path(endpoint).unwrap_or(("connecting".into(), None));
        StreamStatus { path, rtt_ms, bytes_per_sec: rate, received, cached_bytes: self.cached_bytes(), cache_limit: self.max_bytes.load(Ordering::Relaxed) }
    }
    /// Size, date and etag, cached briefly. A new etag drops the old blocks.
    pub async fn stat(&self, f: &FileRef) -> Result<Stat> {
        if let Some((stat, at)) = lock(&self.stats).get(f) { if at.elapsed() < STAT_TTL { return Ok(stat.clone()); } }
        let stat = self.remote.stat(f).await?;
        let old = lock(&self.stats).insert(f.clone(), (stat.clone(), Instant::now()));
        if let Some((old, _)) = old { if old.etag != stat.etag { self.purge(f, &old.etag); } }
        Ok(stat)
    }
    pub fn forget(&self, f: &FileRef) { lock(&self.stats).remove(f); }
    fn purge(&self, f: &FileRef, etag: &str) {
        let key = file_key(f, etag);
        let removed = lock(&self.lru).remove_key(&key);
        for id in removed { let _ = std::fs::remove_file(self.block_path(&id)); }
        lock(&self.seq).remove(&key);
    }
    /// Up to `len` bytes at `offset` (fewer only at the end of the file).
    pub async fn read(self: &Arc<Self>, f: &FileRef, offset: u64, len: u64) -> Result<Bytes> {
        let stat = self.stat(f).await?;
        match self.read_at(f, &stat, offset, len).await {
            Err(e) if is_changed(&e) => {
                // The host's copy changed: forget everything about the old one.
                self.forget(f); self.purge(f, &stat.etag);
                let stat = self.stat(f).await?;
                self.read_at(f, &stat, offset, len).await
            }
            other => other,
        }
    }
    async fn read_at(self: &Arc<Self>, f: &FileRef, stat: &Stat, offset: u64, len: u64) -> Result<Bytes> {
        anyhow::ensure!(!stat.is_dir, "This is a folder");
        if offset >= stat.size || len == 0 { return Ok(Bytes::new()); }
        let end = (offset + len).min(stat.size);
        let key = file_key(f, &stat.etag);
        let (first, last) = (offset / BLOCK, (end - 1) / BLOCK);
        self.read_ahead(f, stat, &key, offset, end);
        for _ in 0..2 {
            self.ensure(f, stat, &key, first, last).await?;
            let mut out = bytes::BytesMut::with_capacity((end - offset) as usize);
            let mut missing = false;
            for idx in first..=last {
                match tokio::fs::read(self.block_path(&(key.clone(), idx))).await {
                    Ok(block) => {
                        let start = offset.saturating_sub(idx * BLOCK) as usize;
                        let stop = ((end - idx * BLOCK).min(block.len() as u64)) as usize;
                        anyhow::ensure!(start <= stop, "Cached block is short");
                        out.extend_from_slice(&block[start..stop]);
                    }
                    // Evicted between the check and the read: fetch again (once).
                    Err(_) => { lock(&self.lru).remove(&(key.clone(), idx)); missing = true; break; }
                }
            }
            if !missing { return Ok(out.freeze()); }
        }
        bail!("Couldn’t keep this file in the cache; make the cache bigger in Settings")
    }
    /// Sequential access doubles the window (2 → 64 MiB); a seek resets it.
    fn read_ahead(self: &Arc<Self>, f: &FileRef, stat: &Stat, key: &str, offset: u64, end: u64) {
        let window = {
            let mut seq = lock(&self.seq);
            let s = seq.entry(key.into()).or_default();
            // Re-reading what was already read (players do) is not progress.
            if s.next > 0 && end <= s.next && offset + BLOCK >= s.next { return; }
            let sequential = s.next > 0 && offset <= s.next + BLOCK && offset + BLOCK >= s.next;
            s.window = if sequential { (s.window * 2).clamp(READAHEAD_START, READAHEAD_MAX) } else if offset == 0 { READAHEAD_START } else { 0 };
            s.next = end;
            // Never read ahead more than a quarter of the cache, or prefetched
            // blocks would evict the ones being read.
            s.window.min(self.max_bytes.load(Ordering::Relaxed) / BLOCK / 4)
        };
        if window == 0 { return; }
        let blocks = stat.size.div_ceil(BLOCK);
        let from = (end - 1) / BLOCK + 1;
        let to = (from + window).min(blocks);
        if from >= to { return; }
        let (engine, f, stat, key) = (self.clone(), f.clone(), stat.clone(), key.to_string());
        tokio::spawn(async move { let _ = engine.ensure(&f, &stat, &key, from, to - 1).await; });
    }
    /// Make blocks `first..=last` present: cached, joined in flight, or fetched.
    async fn ensure(self: &Arc<Self>, f: &FileRef, stat: &Stat, key: &str, first: u64, last: u64) -> Result<()> {
        let mut waits = Vec::new();
        let mut missing: Vec<(u64, watch::Sender<Option<Result<(), String>>>)> = Vec::new();
        {
            let mut lru = lock(&self.lru);
            let mut inflight = lock(&self.inflight);
            for idx in first..=last {
                let id = (key.to_string(), idx);
                if lru.touch(&id) { continue; }
                if let Some(rx) = inflight.get(&id) { waits.push(rx.clone()); continue; }
                let (tx, rx) = watch::channel(None);
                inflight.insert(id, rx.clone());
                waits.push(rx); missing.push((idx, tx));
            }
        }
        // Contiguous runs of at most REQUEST_BLOCKS, each its own request.
        let mut runs: Vec<Vec<(u64, watch::Sender<Option<Result<(), String>>>)>> = Vec::new();
        for item in missing {
            match runs.last_mut() {
                Some(run) if run.last().is_some_and(|(i, _)| *i + 1 == item.0) && (run.len() as u64) < REQUEST_BLOCKS => run.push(item),
                _ => runs.push(vec![item]),
            }
        }
        for run in runs {
            // Spawned, so a reader that gives up (a player seeking away) never
            // strands the blocks others are waiting for.
            let (engine, f, etag, key, size) = (self.clone(), f.clone(), stat.etag.clone(), key.to_string(), stat.size);
            tokio::spawn(async move { engine.fetch(f, etag, key, size, run).await });
        }
        for mut rx in waits {
            let result = loop {
                if let Some(r) = rx.borrow().clone() { break r; }
                if rx.changed().await.is_err() { break rx.borrow().clone().unwrap_or(Err("Read was interrupted".into())); }
            };
            if let Err(e) = result {
                if e == crate::locations::LocationError::Changed.to_string() { return Err(crate::locations::LocationError::Changed.into()); }
                bail!(e);
            }
        }
        Ok(())
    }
    async fn fetch(self: Arc<Self>, f: FileRef, etag: String, key: String, size: u64, run: Vec<(u64, watch::Sender<Option<Result<(), String>>>)>) {
        let first = run[0].0;
        let offset = first * BLOCK;
        let len = (run.len() as u64 * BLOCK).min(size - offset);
        let mut next = 0usize;
        let result: Result<()> = async {
            let _permit = self.limiter(&f.endpoint).acquire_owned().await?;
            let mut rx = self.remote.read(&f, offset, len, &etag).await?;
            let mut buf = bytes::BytesMut::new();
            let mut received = 0u64;
            while let Some(chunk) = rx.recv().await {
                let chunk = chunk?;
                received += chunk.len() as u64;
                lock(&self.meters).entry(f.endpoint.clone()).or_default().add(chunk.len() as u64);
                buf.extend_from_slice(&chunk);
                while next < run.len() {
                    let idx = run[next].0;
                    let want = (size - idx * BLOCK).min(BLOCK) as usize;
                    if buf.len() < want { break; }
                    let block = buf.split_to(want).freeze();
                    self.store((key.clone(), idx), block).await?;
                    let _ = run[next].1.send(Some(Ok(())));
                    next += 1;
                }
            }
            anyhow::ensure!(next == run.len() && received == len, "The connection dropped. Try again.");
            Ok(())
        }.await;
        let failure = result.err().map(|e| if is_changed(&e) { crate::locations::LocationError::Changed.to_string() } else { format!("{e:#}") });
        let mut inflight = lock(&self.inflight);
        for (i, (idx, tx)) in run.iter().enumerate() {
            inflight.remove(&(key.clone(), *idx));
            if i >= next { let _ = tx.send(Some(Err(failure.clone().unwrap_or_else(|| "Read was interrupted".into())))); }
        }
    }
    async fn store(&self, id: BlockId, block: Bytes) -> Result<()> {
        let path = self.block_path(&id);
        let tmp = path.with_extension(format!("tmp{}", rand::random::<u32>()));
        tokio::fs::create_dir_all(path.parent().context("block dir")?).await?;
        tokio::fs::write(&tmp, &block).await?;
        tokio::fs::rename(&tmp, &path).await?;
        let evicted = {
            let mut lru = lock(&self.lru);
            lru.insert(id.clone(), block.len() as u64);
            let max = self.max_bytes.load(Ordering::Relaxed);
            lru.evict(max).into_iter().filter(|e| e != &id).collect::<Vec<_>>()
        };
        for id in evicted { let _ = tokio::fs::remove_file(self.block_path(&id)).await; }
        Ok(())
    }
    /// The whole file into `dest` (for "Open with" and Quick Look), through the
    /// cache, many blocks in flight. Leaves no partial file behind on error.
    pub async fn fetch_to_file(self: &Arc<Self>, f: &FileRef, dest: &Path) -> Result<Stat> {
        use tokio::io::AsyncWriteExt;
        let stat = self.stat(f).await?;
        if let Some(parent) = dest.parent() { tokio::fs::create_dir_all(parent).await?; }
        let tmp = dest.with_extension("dropbeam-part");
        let result: Result<()> = async {
            let mut out = tokio::fs::File::create(&tmp).await?;
            // Never ask for more at once than the cache can hold.
            let step = (PARALLEL as u64 * REQUEST_BLOCKS * BLOCK).min(self.max_bytes.load(Ordering::Relaxed) / 2 / BLOCK * BLOCK).max(BLOCK);
            let mut pos = 0;
            while pos < stat.size {
                let bytes = self.read(f, pos, step).await?;
                anyhow::ensure!(!bytes.is_empty(), "The file got shorter while it was being read");
                out.write_all(&bytes).await?;
                pos += bytes.len() as u64;
            }
            out.flush().await?;
            tokio::fs::rename(&tmp, dest).await?;
            Ok(())
        }.await;
        if result.is_err() { let _ = tokio::fs::remove_file(&tmp).await; }
        result.map(|_| stat)
    }
    /// Every entry of a folder (all `locations.ls` pages, name order).
    pub async fn list(&self, endpoint: &str, location_id: &str, rel_path: &str) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut request = json!({"kind": "locations.ls", "locations_v": crate::locations::VERSION, "id": location_id, "rel_path": rel_path, "sort": "name", "page_size": crate::locations::PAGE_SIZE});
            if let Some(c) = &cursor { request["cursor"] = json!(c); }
            let page = rpc_retrying(&*self.remote, endpoint, request).await?;
            entries.extend(serde_json::from_value::<Vec<Entry>>(page["entries"].clone())?);
            anyhow::ensure!(entries.len() <= 100_000, "Folder listing exceeds 100000 entries");
            match page["nextCursor"].as_str() { Some(next) if page["hasMore"] == true => cursor = Some(next.into()), _ => break }
        }
        Ok(entries)
    }
}

/// A host answers "rate limit" when a Finder window lists many folders at once;
/// wait and ask again rather than showing an error.
pub async fn rpc_retrying(remote: &dyn Remote, endpoint: &str, request: Value) -> Result<Value> {
    let mut wait = Duration::from_millis(120);
    for _ in 0..12 {
        match remote.rpc(endpoint, request.clone()).await {
            Err(e) if format!("{e}").contains("rate limit") => { tokio::time::sleep(wait).await; wait = (wait * 2).min(Duration::from_secs(1)); }
            other => return other,
        }
    }
    remote.rpc(endpoint, request).await
}

// ── The real remote: the friend's connection ─────────────────────────────────

pub(crate) const NEEDS_STREAM_UPDATE: &str = "This device needs a DropBeam update to open files in place.";

pub struct IrohRemote {
    iroh: Arc<crate::iroh_net::IrohState>,
    config: PathBuf,
    app: OnceLock<tauri::AppHandle>,
    sessions: Mutex<HashMap<String, (iroh::endpoint::Connection, u64)>>,
}
impl IrohRemote {
    pub fn new(iroh: Arc<crate::iroh_net::IrohState>, config: PathBuf) -> Arc<Self> {
        Arc::new(Self { iroh, config, app: OnceLock::new(), sessions: Default::default() })
    }
    pub fn set_app(&self, app: tauri::AppHandle) { let _ = self.app.set(app); }
    async fn session(&self, endpoint: &str, stale: Option<usize>) -> Result<(iroh::endpoint::Connection, u64)> {
        if stale.is_none() {
            if let Some((conn, max)) = lock(&self.sessions).get(endpoint) {
                if conn.close_reason().is_none() { return Ok((conn.clone(), *max)); }
            }
        }
        let session = crate::iroh_net::stream_connection(&self.iroh, endpoint, stale).await?;
        lock(&self.sessions).insert(endpoint.into(), session.clone());
        Ok(session)
    }
    /// Run `op` on the cached session; on a dropped stream, redial once.
    async fn on_session<T, F, Fut>(&self, endpoint: &str, v2: bool, op: F) -> Result<T>
    where F: Fn(iroh::endpoint::Connection) -> Fut, Fut: std::future::Future<Output = Result<T>> {
        let (conn, max) = self.session(endpoint, None).await?;
        anyhow::ensure!(!v2 || max >= crate::locations::STREAM_VERSION, NEEDS_STREAM_UPDATE);
        match op(conn.clone()).await {
            Err(e) if format!("{e}") == crate::iroh_net::LOCATIONS_DROPPED || conn.close_reason().is_some() => {
                lock(&self.sessions).remove(endpoint);
                let (conn, _) = self.session(endpoint, Some(conn.stable_id())).await?;
                op(conn).await
            }
            other => other,
        }
    }
}
fn v2(kind: &str, f: &FileRef) -> Value {
    json!({"kind": kind, "locations_v": crate::locations::STREAM_VERSION, "id": f.location_id, "rel_path": f.rel_path})
}
#[async_trait::async_trait]
impl Remote for IrohRemote {
    async fn stat(&self, f: &FileRef) -> Result<Stat> {
        let request = v2("locations.stat", f);
        let data = self.on_session(&f.endpoint, true, |conn| { let request = request.clone(); async move {
            crate::iroh_net::location_rpc_on(&conn, &request, Duration::from_secs(45)).await } }).await?;
        Ok(serde_json::from_value(data)?)
    }
    async fn read(&self, f: &FileRef, offset: u64, len: u64, etag: &str) -> Result<mpsc::Receiver<Result<Bytes>>> {
        let mut request = v2("locations.read", f);
        request["offset"] = json!(offset); request["len"] = json!(len); request["etag"] = json!(etag);
        let (header, recv) = self.on_session(&f.endpoint, true, |conn| { let request = request.clone(); async move {
            crate::iroh_net::location_read_on(&conn, &request).await } }).await?;
        chunked(&header, recv, len)
    }
    async fn rpc(&self, endpoint: &str, request: Value) -> Result<Value> {
        let v2 = request["locations_v"].as_u64() == Some(crate::locations::STREAM_VERSION);
        let timeout = Duration::from_secs(if request["kind"] == "locations.download" { 1800 } else { 45 });
        self.on_session(endpoint, v2, |conn| { let request = request.clone(); async move {
            crate::iroh_net::location_rpc_on(&conn, &request, timeout).await } }).await
    }
    fn devices(&self) -> Vec<(String, String, String)> {
        crate::friends::load(&self.config).into_iter()
            .filter_map(|f| Some((f.id, f.endpoint_id?, f.name))).collect()
    }
    async fn upload(&self, endpoint: &str, location_id: &str, rel_dir: &str, file: &Path, replace: bool) -> Result<String> {
        let app = self.app.get().context("DropBeam is still starting")?.clone();
        let friend = crate::friends::load(&self.config).into_iter().find(|f| f.endpoint_id.as_deref() == Some(endpoint)).context("Friend not found")?;
        use tauri::Manager;
        let state = app.state::<Arc<crate::AppState>>().inner().clone();
        let iroh = app.state::<Arc<crate::iroh_net::IrohState>>().inner().clone();
        let target = crate::locations::Target { location_id: location_id.into(), rel_path: rel_dir.into() };
        let update = crate::commands::start_location_upload_replacing(app, state, iroh, friend.id, target, vec![file.to_string_lossy().into_owned()], replace)
            .await.map_err(anyhow::Error::msg)?;
        Ok(update.id)
    }
    fn path(&self, endpoint: &str) -> Option<(String, Option<u64>)> {
        let conn = lock(&self.sessions).get(endpoint).map(|(c, _)| c.clone())
            .or_else(|| self.iroh.cached_friend_conn(endpoint, None))?;
        let detail = crate::iroh_net::conn_detail(&conn);
        Some((detail.path, detail.rtt_ms))
    }
}

/// The body of a `locations.read` as 1 MiB chunks. The host's header must
/// promise exactly the requested length (a file that shrank fails the read).
pub(crate) fn chunked(header: &Value, mut recv: iroh::endpoint::RecvStream, len: u64) -> Result<mpsc::Receiver<Result<Bytes>>> {
    let total = header["len"].as_u64().context("Bad read header")?;
    anyhow::ensure!(total == len, "The file got shorter while it was being read");
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        let mut left = total;
        while left > 0 {
            let mut buf = vec![0u8; left.min(BLOCK) as usize];
            let chunk = match tokio::time::timeout(Duration::from_secs(60), recv.read_exact(&mut buf)).await {
                Ok(Ok(())) => Ok(Bytes::from(buf)),
                Ok(Err(e)) => Err(anyhow::anyhow!("The connection dropped. Try again. ({e})")),
                Err(_) => Err(anyhow::anyhow!(crate::iroh_net::LOCATIONS_SLOW)),
            };
            let failed = chunk.is_err();
            if !failed { left -= left.min(BLOCK); }
            if tx.send(chunk).await.is_err() || failed { return; }
        }
    });
    Ok(rx)
}

// ── The engine this app runs, and the 127.0.0.1 range server ────────────────

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
static SERVER: OnceLock<Server> = OnceLock::new();
static REMOTE: OnceLock<Arc<IrohRemote>> = OnceLock::new();

/// Settings → Drives, persisted beside the other config (drive.json).
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DriveSettings { pub enabled: bool, pub auto_mount: bool, pub cache_gb: u64 }
impl Default for DriveSettings { fn default() -> Self { Self { enabled: true, auto_mount: true, cache_gb: DEFAULT_CACHE_BYTES / 1_000_000_000 } } }
pub fn load_settings(config: &Path) -> DriveSettings {
    match crate::settings::read_json_store::<DriveSettings>(&config.join("drive.json")) {
        crate::settings::StoreRead::Loaded(s) => s,
        _ => DriveSettings::default(),
    }
}
pub fn save_settings(config: &Path, s: &DriveSettings) -> Result<()> {
    crate::settings::write_atomic(&config.join("drive.json"), &serde_json::to_vec_pretty(s)?)?;
    if let Some(engine) = ENGINE.get() { engine.set_limit(s.cache_gb.clamp(1, 2000) * 1_000_000_000); }
    Ok(())
}

/// Start the engine and the range server (idempotent). Called from setup.
pub fn init(config: &Path, cache: &Path, iroh: Arc<crate::iroh_net::IrohState>, app: tauri::AppHandle) {
    let remote = REMOTE.get_or_init(|| IrohRemote::new(iroh, config.to_path_buf()));
    remote.set_app(app);
    let limit = load_settings(config).cache_gb.clamp(1, 2000) * 1_000_000_000;
    let dir = cache.join("stream-blocks");
    let _ = std::fs::create_dir_all(&dir);
    let engine = ENGINE.get_or_init(|| Engine::new(remote.clone(), dir, limit)).clone();
    let config = config.to_path_buf();
    if SERVER.get().is_none() {
        let resolve: Resolver = Arc::new(move |friend: &str| crate::friends::get(&config, friend).and_then(|f| f.endpoint_id));
        tauri::async_runtime::spawn(async move {
            match Server::start(engine, resolve).await {
                Ok(server) => { log::info!("open in place: serving on 127.0.0.1:{}", server.port); let _ = SERVER.set(server); }
                Err(e) => log::warn!("open in place: couldn't start the local server: {e:#}"),
            }
        });
    }
}
pub fn engine() -> Result<Arc<Engine>> { ENGINE.get().cloned().context("DropBeam is still starting") }
pub async fn server() -> Result<&'static Server> {
    for _ in 0..50 { if let Some(s) = SERVER.get() { return Ok(s); } tokio::time::sleep(Duration::from_millis(100)).await; }
    bail!("DropBeam is still starting")
}

pub type Resolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
type Body = http_body_util::combinators::BoxBody<Bytes, std::io::Error>;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub type Dav = Arc<dyn Fn(hyper::Request<hyper::body::Incoming>) -> std::pin::Pin<Box<dyn std::future::Future<Output = hyper::Response<Body>> + Send>> + Send + Sync>;

pub struct Server {
    pub port: u16,
    pub token: String,
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    dav: Arc<Mutex<Option<Dav>>>,
}
impl Server {
    pub async fn start(engine: Arc<Engine>, resolve: Resolver) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let token = hex::encode(rand::random::<[u8; 24]>());
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        let dav: Arc<Mutex<Option<Dav>>> = Default::default();
        let ctx = Arc::new(Ctx { engine, resolve, token: token.clone(), #[cfg(not(any(target_os = "ios", target_os = "android")))] dav: dav.clone() });
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { tokio::time::sleep(Duration::from_millis(50)).await; continue; };
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |req| { let ctx = ctx.clone(); async move { Ok::<_, std::convert::Infallible>(ctx.handle(req).await) } });
                    let _ = hyper::server::conn::http1::Builder::new().keep_alive(true)
                        .serve_connection(hyper_util::rt::TokioIo::new(socket), service).await;
                });
            }
        });
        Ok(Self { port, token, #[cfg(not(any(target_os = "ios", target_os = "android")))] dav })
    }
    /// The streaming URL of one file. Unguessable (per-run token) and 127.0.0.1-only.
    pub fn file_url(&self, friend_id: &str, location_id: &str, rel_path: &str) -> String {
        const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
        let enc = |s: &str| percent_encoding::utf8_percent_encode(s, SEGMENT).to_string();
        let rel: Vec<String> = rel_path.split('/').filter(|p| !p.is_empty()).map(enc).collect();
        format!("http://127.0.0.1:{}/{}/f/{}/{}/{}", self.port, self.token, enc(friend_id), enc(location_id), rel.join("/"))
    }
    pub fn dav_url(&self) -> String { format!("http://127.0.0.1:{}/{}/dav/", self.port, self.token) }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub fn set_dav(&self, dav: Option<Dav>) { *lock(&self.dav) = dav; }
}

struct Ctx {
    engine: Arc<Engine>, resolve: Resolver, token: String,
    #[cfg(not(any(target_os = "ios", target_os = "android")))] dav: Arc<Mutex<Option<Dav>>>,
}
fn full(status: u16, text: &str) -> hyper::Response<Body> {
    use http_body_util::BodyExt;
    hyper::Response::builder().status(status).header("Content-Type", "text/plain; charset=utf-8").header("Cache-Control", "no-store")
        .body(http_body_util::Full::new(Bytes::from(text.to_string())).map_err(|e| match e {}).boxed()).unwrap()
}
impl Ctx {
    async fn handle(&self, req: hyper::Request<hyper::body::Incoming>) -> hyper::Response<Body> {
        // Only this machine, only with the token (a web page can't guess it, and
        // a rebinding DNS name never matches the Host check).
        let host_ok = req.headers().get("host").and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.starts_with("127.0.0.1:") || h.starts_with("localhost:") || h == "127.0.0.1" || h == "localhost");
        let path = req.uri().path().to_string();
        let mut parts = path.trim_start_matches('/').splitn(3, '/');
        let token = parts.next().unwrap_or("");
        if !host_ok || !constant_eq(token.as_bytes(), self.token.as_bytes()) { return full(404, "Not found"); }
        match parts.next() {
            Some("f") => self.serve_file(&req, parts.next().unwrap_or("")).await,
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            Some("dav") => {
                let dav = lock(&self.dav).clone();
                match dav { Some(dav) => dav(req).await, None => full(404, "The drive is off") }
            }
            _ => full(404, "Not found"),
        }
    }
    async fn serve_file(&self, req: &hyper::Request<hyper::body::Incoming>, rest: &str) -> hyper::Response<Body> {
        use http_body_util::BodyExt;
        let origin = req.headers().get("origin").and_then(|o| o.to_str().ok()).filter(|o| allowed_origin(o)).map(str::to_string);
        let with_cors = |mut b: hyper::http::response::Builder| {
            if let Some(o) = &origin {
                b = b.header("Access-Control-Allow-Origin", o.as_str()).header("Vary", "Origin")
                    .header("Access-Control-Expose-Headers", "Content-Range, Content-Length, Accept-Ranges, ETag");
            }
            b
        };
        if req.method() == hyper::Method::OPTIONS {
            return with_cors(hyper::Response::builder().status(204)).header("Access-Control-Allow-Methods", "GET, HEAD")
                .header("Access-Control-Allow-Headers", "Range").body(http_body_util::Empty::new().map_err(|e| match e {}).boxed()).unwrap();
        }
        if req.method() != hyper::Method::GET && req.method() != hyper::Method::HEAD { return full(405, "Method not allowed"); }
        let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8().map(|c| c.into_owned()).ok();
        let mut segs = rest.splitn(3, '/');
        let (Some(friend), Some(location)) = (segs.next().and_then(decode), segs.next().and_then(decode)) else { return full(404, "Not found"); };
        let rel: Option<Vec<String>> = segs.next().unwrap_or("").split('/').filter(|s| !s.is_empty()).map(decode).collect();
        let Some(rel) = rel else { return full(400, "Bad path"); };
        let rel = rel.join("/");
        if crate::locations::relative(&rel).is_err() { return full(400, "Bad path"); }
        let Some(endpoint) = (self.resolve)(&friend) else { return full(404, "Friend not found"); };
        let f = FileRef { endpoint, location_id: location, rel_path: rel.clone() };
        let stat = match self.engine.stat(&f).await {
            Ok(s) if !s.is_dir => s,
            Ok(_) => return full(404, "This is a folder"),
            Err(e) => return full(502, &format!("{e:#}")),
        };
        let len = stat.size;
        let mime = mime(&rel);
        let mut b = with_cors(hyper::Response::builder())
            .header("Accept-Ranges", "bytes").header("Content-Type", mime).header("ETag", format!("\"{}\"", stat.etag))
            .header("Cache-Control", "private, max-age=0").header("X-Content-Type-Options", "nosniff");
        // Text and unknown types never run as a page (a friend's .html is shown as text).
        if mime.starts_with("text/") || mime == "application/octet-stream" { b = b.header("Content-Security-Policy", "sandbox"); }
        let (start, end) = match req.headers().get("range").and_then(|r| r.to_str().ok()).map(|r| parse_range(r, len)) {
            None | Some(Range::Ignore) => (0, len),
            Some(Range::Bytes(s, e)) => { b = b.status(206).header("Content-Range", format!("bytes {s}-{}/{len}", e - 1)); (s, e) }
            Some(Range::Unsatisfiable) => return with_cors(hyper::Response::builder()).status(416).header("Content-Range", format!("bytes */{len}"))
                .body(http_body_util::Empty::new().map_err(|e| match e {}).boxed()).unwrap(),
        };
        b = b.header("Content-Length", end - start);
        if req.method() == hyper::Method::HEAD || start == end {
            return b.body(http_body_util::Empty::new().map_err(|e| match e {}).boxed()).unwrap();
        }
        let (tx, rx) = mpsc::channel::<Result<hyper::body::Frame<Bytes>, std::io::Error>>(2);
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let mut pos = start;
            while pos < end {
                let stop = end.min((pos / BLOCK + 1) * BLOCK);
                match engine.read(&f, pos, stop - pos).await {
                    Ok(bytes) if !bytes.is_empty() => { pos += bytes.len() as u64; if tx.send(Ok(hyper::body::Frame::data(bytes))).await.is_err() { return; } }
                    Ok(_) => { let _ = tx.send(Err(std::io::Error::other("The file got shorter while it was being read"))).await; return; }
                    Err(e) => { log::info!("open in place: read failed: {e:#}"); let _ = tx.send(Err(std::io::Error::other(format!("{e:#}")))).await; return; }
                }
            }
        });
        let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
        b.body(http_body_util::StreamBody::new(stream).boxed()).unwrap()
    }
}
fn constant_eq(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }
/// The app's own webview origins (macOS/iOS, Windows, Linux, dev server).
fn allowed_origin(o: &str) -> bool {
    matches!(o, "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost" | "http://localhost:5173")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Range { Bytes(u64, u64), Unsatisfiable, Ignore }
/// One `bytes=` range → `[start, end)`. Several ranges are answered with the
/// whole file (allowed by RFC 9110); anything malformed is ignored the same way.
pub fn parse_range(value: &str, len: u64) -> Range {
    let Some(spec) = value.trim().strip_prefix("bytes=") else { return Range::Ignore; };
    if spec.contains(',') { return Range::Ignore; }
    let Some((a, b)) = spec.trim().split_once('-') else { return Range::Ignore; };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let Ok(n) = b.parse::<u64>() else { return Range::Ignore; };
        if n == 0 || len == 0 { return Range::Unsatisfiable; }
        return Range::Bytes(len - n.min(len), len);
    }
    let Ok(start) = a.parse::<u64>() else { return Range::Ignore; };
    let end = if b.is_empty() { len } else { match b.parse::<u64>() { Ok(e) if e >= start => (e + 1).min(len), _ => return Range::Ignore } };
    if start >= len { return Range::Unsatisfiable; }
    Range::Bytes(start, end)
}
/// Content-Type by extension. Active web content is served as plain text.
pub fn mime(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg", "png" => "image/png", "gif" => "image/gif", "webp" => "image/webp",
        "heic" => "image/heic", "heif" => "image/heif", "avif" => "image/avif", "bmp" => "image/bmp", "tif" | "tiff" => "image/tiff",
        "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm", "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo", "ogv" => "video/ogg", "ts" | "m2ts" => "video/mp2t", "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg", "m4a" => "audio/mp4", "aac" => "audio/aac", "wav" => "audio/wav", "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg", "opus" => "audio/opus", "aiff" | "aif" => "audio/aiff",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" | "csv" | "tsv" | "json" | "xml" | "yaml" | "yml" | "ini" | "conf" | "srt" | "vtt"
        | "html" | "htm" | "svg" | "js" | "css" | "rs" | "py" | "sh" | "ts.txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
/// What the in-app viewer can show for a name.
pub fn preview_kind(name: &str) -> &'static str {
    let m = mime(name);
    if m.starts_with("image/") && !matches!(m, "image/heic" | "image/heif" | "image/tiff") { "image" }
    else if m.starts_with("video/") { "video" } else if m.starts_with("audio/") { "audio" }
    else if m == "application/pdf" { "pdf" } else if m.starts_with("text/") { "text" } else { "other" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// An in-memory host: files by path, a call counter, optional "changed".
    pub(crate) struct FakeRemote { pub files: Mutex<HashMap<String, (Vec<u8>, String)>>, pub reads: AtomicUsize, pub delay: Duration }
    impl FakeRemote {
        pub fn new() -> Arc<Self> { Arc::new(Self { files: Default::default(), reads: AtomicUsize::new(0), delay: Duration::from_millis(20) }) }
        pub fn put(&self, path: &str, bytes: Vec<u8>, etag: &str) { lock(&self.files).insert(path.into(), (bytes, etag.into())); }
    }
    #[async_trait::async_trait]
    impl Remote for FakeRemote {
        async fn stat(&self, f: &FileRef) -> Result<Stat> {
            let files = lock(&self.files);
            let (b, etag) = files.get(&f.rel_path).context("Not found")?;
            Ok(Stat { is_dir: false, size: b.len() as u64, modified: 1, etag: etag.clone() })
        }
        async fn read(&self, f: &FileRef, offset: u64, len: u64, etag: &str) -> Result<mpsc::Receiver<Result<Bytes>>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let (bytes, current) = lock(&self.files).get(&f.rel_path).cloned().context("Not found")?;
            if current != etag { return Err(crate::locations::LocationError::Changed.into()); }
            let (tx, rx) = mpsc::channel(64);
            let slice = bytes[offset as usize..(offset + len) as usize].to_vec();
            for chunk in slice.chunks(300_000) { tx.send(Ok(Bytes::copy_from_slice(chunk))).await.unwrap(); }
            Ok(rx)
        }
        async fn rpc(&self, _: &str, _: Value) -> Result<Value> { bail!("unsupported") }
        fn devices(&self) -> Vec<(String, String, String)> { vec![] }
        async fn upload(&self, _: &str, _: &str, _: &str, _: &Path, _: bool) -> Result<String> { bail!("unsupported") }
        fn path(&self, _: &str) -> Option<(String, Option<u64>)> { Some(("local".into(), Some(1))) }
    }
    fn data(n: usize) -> Vec<u8> { (0..n).map(|i| (i * 7 + i / 977) as u8).collect() }
    fn file(path: &str) -> FileRef { FileRef { endpoint: "host".into(), location_id: "nas".into(), rel_path: path.into() } }
    fn temp() -> PathBuf { let d = std::env::temp_dir().join(format!("dropbeam-stream-{}", uuid::Uuid::new_v4())); std::fs::create_dir_all(&d).unwrap(); d }

    #[test]
    fn range_headers_parse_like_rfc_9110() {
        assert_eq!(parse_range("bytes=0-99", 1000), Range::Bytes(0, 100));
        assert_eq!(parse_range("bytes=100-", 1000), Range::Bytes(100, 1000));
        assert_eq!(parse_range("bytes=-200", 1000), Range::Bytes(800, 1000));
        assert_eq!(parse_range("bytes=-5000", 1000), Range::Bytes(0, 1000));
        assert_eq!(parse_range("bytes=990-5000", 1000), Range::Bytes(990, 1000));
        assert_eq!(parse_range("bytes=1000-", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Range::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-2", 1000), Range::Ignore);
        assert_eq!(parse_range("bytes=0-1,5-9", 1000), Range::Ignore);
        assert_eq!(parse_range("items=0-1", 1000), Range::Ignore);
        assert_eq!(parse_range("bytes=0-1", 0), Range::Unsatisfiable);
        assert_eq!(mime("Movie.MKV"), "video/x-matroska");
        assert_eq!(mime("page.html"), "text/plain; charset=utf-8");
        assert_eq!(preview_kind("a.pdf"), "pdf");
        assert_eq!(preview_kind("b.heic"), "other");
    }

    #[test]
    fn lru_evicts_oldest_first_and_touch_protects() {
        let mut lru = Lru::default();
        for i in 0..5 { lru.insert(("k".into(), i), 10); }
        assert!(lru.touch(&("k".into(), 0)));
        let gone = lru.evict(30);
        assert_eq!(gone, vec![("k".to_string(), 1), ("k".to_string(), 2)]);
        assert_eq!(lru.total, 30);
        assert_eq!(lru.remove_key("k").len(), 3);
        assert_eq!(lru.total, 0);
    }

    #[tokio::test]
    async fn reads_match_the_source_across_blocks_and_coalesce_concurrent_misses() {
        let remote = FakeRemote::new();
        let bytes = data(5 * BLOCK as usize + 12345);
        remote.put("a.bin", bytes.clone(), "e1");
        let engine = Engine::new(remote.clone(), temp(), 1 << 30);
        let f = file("a.bin");
        // Eight readers of the same middle range at once: one request per block run.
        let mut tasks = Vec::new();
        for _ in 0..8 { let (e, f) = (engine.clone(), f.clone()); tasks.push(tokio::spawn(async move { e.read(&f, BLOCK + 10, BLOCK).await.unwrap() })); }
        for t in tasks { assert_eq!(&t.await.unwrap()[..], &bytes[BLOCK as usize + 10..2 * BLOCK as usize + 10]); }
        let after_first = remote.reads.load(Ordering::SeqCst);
        assert!(after_first <= 2, "coalesced into {after_first} requests");
        // Random reads anywhere, including the short last block.
        for (off, len) in [(0, 10), (5 * BLOCK, 99_999), (3 * BLOCK - 1, 2), (5 * BLOCK + 12340, 100)] {
            let got = engine.read(&f, off, len).await.unwrap();
            let end = ((off + len) as usize).min(bytes.len());
            assert_eq!(&got[..], &bytes[off as usize..end]);
        }
        assert!(engine.read(&f, bytes.len() as u64, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sequential_reads_grow_read_ahead_and_seeks_reset_it() {
        let remote = FakeRemote::new();
        remote.put("v.mp4", data(40 * BLOCK as usize), "e1");
        let engine = Engine::new(remote.clone(), temp(), 1 << 30);
        let f = file("v.mp4");
        for i in 0..4 { engine.read(&f, i * BLOCK, BLOCK).await.unwrap(); }
        tokio::time::sleep(Duration::from_millis(300)).await;
        // After four sequential blocks the window reached 16: blocks well
        // beyond what was asked for are already on disk.
        let key = file_key(&f, "e1");
        assert!(lock(&engine.lru).map.contains_key(&(key.clone(), 12)));
        assert_eq!(lock(&engine.seq).get(&key).unwrap().window, 16);
        engine.read(&f, 30 * BLOCK + 5, 10).await.unwrap();
        assert_eq!(lock(&engine.seq).get(&key).unwrap().window, 0);
    }

    #[tokio::test]
    async fn a_changed_file_is_never_served_from_old_blocks() {
        let remote = FakeRemote::new();
        remote.put("doc.txt", vec![b'a'; 3000], "v1");
        let engine = Engine::new(remote.clone(), temp(), 1 << 30);
        let f = file("doc.txt");
        assert_eq!(&engine.read(&f, 0, 4).await.unwrap()[..], b"aaaa");
        remote.put("doc.txt", vec![b'b'; 2000], "v2");
        // The cached stat still says v1: the host answers "changed", the engine
        // re-stats, drops v1's blocks and reads v2.
        engine.forget(&f);
        lock(&engine.stats).insert(f.clone(), (Stat { is_dir: false, size: 3000, modified: 1, etag: "v1".into() }, Instant::now()));
        let key_v1 = file_key(&f, "v1");
        lock(&engine.lru).remove(&(key_v1.clone(), 0));
        assert_eq!(&engine.read(&f, 0, 4).await.unwrap()[..], b"bbbb");
        assert!(lock(&engine.lru).map.keys().all(|(k, _)| k != &key_v1));
        assert_eq!(engine.stat(&f).await.unwrap().size, 2000);
    }

    #[tokio::test]
    async fn the_cache_stays_within_its_size_and_survives_a_restart() {
        let remote = FakeRemote::new();
        let bytes = data(6 * BLOCK as usize);
        remote.put("big", bytes.clone(), "e");
        let dir = temp();
        let engine = Engine::new(remote.clone(), dir.clone(), 3 * BLOCK);
        let f = file("big");
        for i in 0..6 { engine.read(&f, i * BLOCK, BLOCK).await.unwrap(); }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(engine.cached_bytes() <= 3 * BLOCK);
        let on_disk: u64 = walk(&dir).iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
        assert!(on_disk <= 3 * BLOCK, "{on_disk}");
        let reopened = Engine::new(remote.clone(), dir.clone(), 3 * BLOCK);
        assert_eq!(reopened.cached_bytes(), engine.cached_bytes());
        assert_eq!(&reopened.read(&f, 5 * BLOCK, 10).await.unwrap()[..], &bytes[5 * BLOCK as usize..5 * BLOCK as usize + 10]);
        reopened.set_limit(BLOCK);
        assert!(reopened.cached_bytes() <= BLOCK);
        let out = dir.join("whole.bin");
        reopened.fetch_to_file(&f, &out).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), bytes);
    }
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() { out.extend(walk(&e.path())); } else if e.file_name().to_string_lossy().contains('-') { out.push(e.path()); }
        }
        out
    }

    #[tokio::test]
    async fn range_server_answers_get_head_and_ranges_only_with_the_token() {
        let remote = FakeRemote::new();
        let bytes = data(3 * BLOCK as usize + 7);
        remote.put("clips/movie.mp4", bytes.clone(), "e");
        let engine = Engine::new(remote.clone(), temp(), 1 << 30);
        let server = Server::start(engine, Arc::new(|f: &str| (f == "friend").then(|| "host".to_string()))).await.unwrap();
        let url = server.file_url("friend", "nas", "clips/movie.mp4");
        assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/f/friend/nas/clips/movie.mp4"));
        let client = reqwest::Client::new();
        let all = client.get(&url).send().await.unwrap();
        assert_eq!(all.status(), 200);
        assert_eq!(all.headers()["content-type"], "video/mp4");
        assert_eq!(all.headers()["accept-ranges"], "bytes");
        assert_eq!(all.bytes().await.unwrap().as_ref(), &bytes[..]);
        let part = client.get(&url).header("Range", "bytes=1048570-1048590").send().await.unwrap();
        assert_eq!(part.status(), 206);
        assert_eq!(part.headers()["content-range"], format!("bytes 1048570-1048590/{}", bytes.len()));
        assert_eq!(part.bytes().await.unwrap().as_ref(), &bytes[1048570..1048591]);
        let head = client.head(&url).send().await.unwrap();
        assert_eq!(head.headers()["content-length"], bytes.len().to_string());
        let past = client.get(&url).header("Range", format!("bytes={}-", bytes.len())).send().await.unwrap();
        assert_eq!(past.status(), 416);
        let wrong = url.replace(&server.token, &"0".repeat(48));
        assert_eq!(client.get(&wrong).send().await.unwrap().status(), 404);
        let escape = url.replace("clips/movie.mp4", "..%2Fsecret");
        assert_eq!(client.get(&escape).send().await.unwrap().status(), 400);
    }
}
