//! "Show Locations as a drive" (docs/locations-v2.md, Phase 4). One virtual
//! tree, `/<device>/<location>/…`, over every friend Location this device may
//! use, backed by the stream engine (stream.rs) for reads and by the existing
//! verified Locations upload for writes.
//!
//! - macOS: an in-process NFSv3 server (`nfsserve`) on 127.0.0.1, mounted
//!   without admin rights by `mount_nfs` at `~/DropBeam Drives`.
//! - Linux: WebDAV on the stream server (127.0.0.1, per-run token), mounted
//!   with `gio mount` so it shows in Files.
//! - Windows: the same WebDAV mapped to a drive letter with `net use`
//!   (WebClient), with a one-click fix for a stopped service / 50 MB limit.
//!
//! Writes never touch the host directly. A file created or changed through the
//! drive is written to a private staging file here; once it has been idle for
//! `COMMIT_IDLE` (NFSv3 has no close, and nfsserve answers every WRITE as
//! FILE_SYNC so clients send no COMMIT) or when a WebDAV PUT ends, it is
//! uploaded through the normal Locations upload: hashed, verified and published
//! with no-replace (an edit of an existing file replaces it and the previous
//! version goes to the location's recoverable trash). Until the host lists it,
//! the drive shows the local copy. New folders, renames and deletes are
//! `locations.mkdir/rename/trash` and need the "rename & delete" right; without
//! the upload right a location is read-only.
use crate::stream::{Engine, Entry, FileRef};
use anyhow::{Context, Result};
use bytes::Bytes;
use serde::Serialize;
use serde_json::json;
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

pub const MOUNT_NAME: &str = "DropBeam Drives";
const COMMIT_IDLE: Duration = Duration::from_secs(2);
const LIST_TTL: Duration = Duration::from_secs(5);
const HOSTS_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail { NotFound, Denied, ReadOnly, Exists, IsDir, Invalid, Io(String) }
impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        let m = format!("{e:#}");
        if m.contains("Not found") || m.contains("No such file") { Fail::NotFound }
        else if m.contains("permission denied") || m.contains("access denied") { Fail::Denied }
        else if m.contains("exists") || m.contains("nothing was replaced") { Fail::Exists }
        else { Fail::Io(m) }
    }
}
impl From<std::io::Error> for Fail { fn from(e: std::io::Error) -> Self { if e.kind() == std::io::ErrorKind::NotFound { Fail::NotFound } else { Fail::Io(e.to_string()) } } }

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Node { pub id: u64, pub name: String, pub is_dir: bool, pub size: u64, pub modified: u64, pub writable: bool }

#[derive(Clone, Debug)]
struct Loc { name: String, id: String, upload: bool, manage: bool }
#[derive(Clone, Debug)]
struct Host { name: String, friend_id: String, endpoint: String, locations: Vec<Loc> }
enum Place { Root, Host(Host), Loc { host: Host, loc: Loc, rel: String } }

/// Folder names a person sees: no separators, never empty, unique.
fn display_name(raw: &str, taken: &[String]) -> String {
    let base: String = raw.trim().chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '-' } else { c }).collect();
    let base = if base.is_empty() || base == "." || base == ".." { "Untitled".to_string() } else { base };
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) { name = format!("{base} ({n})"); n += 1; }
    name
}
/// Files a desktop writes for itself (Finder metadata, Office locks…): kept on
/// this device for the session, never uploaded to a friend's folder.
pub fn local_only(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with("._") || name == ".localized" || name.starts_with("~$")
        || name.eq_ignore_ascii_case("Thumbs.db") || name.eq_ignore_ascii_case("desktop.ini") || name.starts_with(".~lock.")
}
fn split(path: &str) -> (&str, &str) { path.rsplit_once('/').unwrap_or(("", path)) }
fn join(dir: &str, name: &str) -> String { if dir.is_empty() { name.into() } else { format!("{dir}/{name}") } }

/// Stable file ids (NFS file handles) for virtual paths, persisted so a mount
/// keeps the same ids across launches.
#[derive(Default, Serialize, serde::Deserialize)]
struct IdFile { next: u64, paths: HashMap<String, u64> }
struct Ids { file: PathBuf, by_path: HashMap<String, u64>, by_id: HashMap<u64, String>, next: u64, dirty: bool }
impl Ids {
    fn load(file: PathBuf) -> Self {
        let saved: IdFile = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let by_id = saved.paths.iter().map(|(p, id)| (*id, p.clone())).collect();
        Self { file, next: saved.next.max(2), by_path: saved.paths, by_id, dirty: false }
    }
    fn id(&mut self, path: &str) -> u64 {
        if path.is_empty() { return 1; }
        if let Some(id) = self.by_path.get(path) { return *id; }
        let id = self.next; self.next += 1; self.dirty = true;
        self.by_path.insert(path.into(), id); self.by_id.insert(id, path.into());
        id
    }
    fn path(&self, id: u64) -> Option<String> { if id == 1 { Some(String::new()) } else { self.by_id.get(&id).cloned() } }
    /// Moving an item keeps its id (and its children's), so open files survive.
    fn rename(&mut self, from: &str, to: &str) {
        let prefix = format!("{from}/");
        let moved: Vec<(String, u64)> = self.by_path.iter().filter(|(p, _)| *p == from || p.starts_with(&prefix)).map(|(p, id)| (p.clone(), *id)).collect();
        for (old, id) in moved {
            let new = format!("{to}{}", &old[from.len()..]);
            self.by_path.remove(&old);
            if let Some(stale) = self.by_path.insert(new.clone(), id) { self.by_id.remove(&stale); }
            self.by_id.insert(id, new);
        }
        self.dirty = true;
    }
    fn save(&mut self) {
        if !self.dirty { return; }
        self.dirty = false;
        let body = serde_json::to_vec(&IdFile { next: self.next, paths: self.by_path.clone() }).unwrap_or_default();
        if let Err(e) = crate::settings::write_atomic(&self.file, &body) { log::warn!("drive: couldn't save file ids: {e}"); }
    }
}

/// A file written through the drive, held here until the host has it.
#[derive(Debug)]
struct Staged {
    file: PathBuf, endpoint: String, location_id: String, rel_dir: String, name: String,
    replace: bool, local_only: bool, dirty: bool, size: u64, modified: u64,
    last_write: Instant, uploading: Option<Instant>, error: Option<String>,
}

pub struct Tree {
    engine: Arc<Engine>,
    staging: PathBuf,
    ids: Mutex<Ids>,
    hosts: Mutex<(Option<Instant>, Vec<Host>)>,
    refreshing: AtomicBool,
    dirs: Mutex<HashMap<String, (Instant, Arc<Vec<Entry>>)>>,
    staged: Mutex<HashMap<String, Staged>>,
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|p| p.into_inner()) }
fn now_ms() -> u64 { crate::chat::now_ms() }

impl Tree {
    pub fn new(engine: Arc<Engine>, config: &Path, staging: PathBuf) -> Arc<Self> {
        // Staging from a previous run whose upload never finished is not
        // silently re-sent: it was already handed to the transfer engine
        // (whose own retry covers it) or the app quit mid-edit.
        let _ = std::fs::remove_dir_all(&staging);
        Arc::new(Self { engine, staging, ids: Mutex::new(Ids::load(config.join("drive-ids.json"))), hosts: Mutex::new((None, vec![])),
            refreshing: AtomicBool::new(false), dirs: Default::default(), staged: Default::default() })
    }
    pub fn id(&self, path: &str) -> u64 { lock(&self.ids).id(path) }
    pub fn path_of(&self, id: u64) -> Option<String> { lock(&self.ids).path(id) }
    /// (devices, locations) currently visible.
    pub fn counts(&self) -> (usize, usize) {
        let hosts = lock(&self.hosts);
        (hosts.1.len(), hosts.1.iter().map(|h| h.locations.len()).sum())
    }
    pub fn pending_uploads(&self) -> usize { lock(&self.staged).values().filter(|s| !s.local_only).count() }
    pub fn upload_errors(&self) -> Vec<String> { lock(&self.staged).values().filter_map(|s| s.error.clone()).collect() }

    /// Ask every friend device which Locations it shares with us. A device
    /// that doesn't answer keeps what it shared last time.
    pub async fn refresh_hosts(&self) {
        if self.refreshing.swap(true, Ordering::SeqCst) { return; }
        let remote = self.engine.remote().clone();
        let devices = remote.devices();
        let asks = devices.iter().map(|(friend, endpoint, name)| {
            let remote = remote.clone();
            let (friend, endpoint, name) = (friend.clone(), endpoint.clone(), name.clone());
            async move {
                let reply = tokio::time::timeout(Duration::from_secs(12),
                    remote.rpc(&endpoint, json!({"kind": "locations.list", "locations_v": crate::locations::VERSION}))).await;
                (friend, endpoint, name, reply.ok().and_then(|r| r.ok()))
            }
        });
        let answers = futures_util::future::join_all(asks).await;
        let previous = lock(&self.hosts).1.clone();
        let mut hosts: Vec<Host> = Vec::new();
        for (friend_id, endpoint, name, reply) in answers {
            let locations: Vec<(String, String, bool, bool)> = match reply {
                Some(list) => list.as_array().into_iter().flatten().filter_map(|l| Some((
                    l["name"].as_str()?.to_string(), l["id"].as_str()?.to_string(),
                    l["rights"]["upload"].as_bool().unwrap_or(false), l["rights"]["manage"].as_bool().unwrap_or(false)))).collect(),
                None => match previous.iter().find(|h| h.endpoint == endpoint) {
                    Some(h) => h.locations.iter().map(|l| (l.name.clone(), l.id.clone(), l.upload, l.manage)).collect(),
                    None => continue,
                },
            };
            if locations.is_empty() { continue; }
            let taken: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
            let mut locs: Vec<Loc> = Vec::new();
            for (n, id, upload, manage) in locations {
                let names: Vec<String> = locs.iter().map(|l| l.name.clone()).collect();
                locs.push(Loc { name: display_name(&n, &names), id, upload, manage });
            }
            // Keep a device's folder name stable when it already had one.
            let name = previous.iter().find(|h| h.endpoint == endpoint).map(|h| h.name.clone())
                .filter(|n| !taken.iter().any(|t| t.eq_ignore_ascii_case(n))).unwrap_or_else(|| display_name(&name, &taken));
            hosts.push(Host { name, friend_id, endpoint, locations: locs });
        }
        hosts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        *lock(&self.hosts) = (Some(Instant::now()), hosts);
        self.refreshing.store(false, Ordering::SeqCst);
    }
    async fn hosts(self: &Arc<Self>) -> Vec<Host> {
        let (at, hosts) = lock(&self.hosts).clone();
        match at {
            None => { self.refresh_hosts().await; lock(&self.hosts).1.clone() }
            Some(at) if at.elapsed() > HOSTS_TTL => {
                let tree = self.clone();
                tokio::spawn(async move { tree.refresh_hosts().await });
                hosts
            }
            Some(_) => hosts,
        }
    }
    async fn place(self: &Arc<Self>, path: &str) -> Result<Place, Fail> {
        if path.is_empty() { return Ok(Place::Root); }
        let mut parts = path.splitn(3, '/');
        let hosts = self.hosts().await;
        let first = parts.next().unwrap_or("");
        let host = hosts.into_iter().find(|h| h.name == first).ok_or(Fail::NotFound)?;
        let Some(loc_name) = parts.next() else { return Ok(Place::Host(host)); };
        let loc = host.locations.iter().find(|l| l.name == loc_name).cloned().ok_or(Fail::NotFound)?;
        let rel = parts.next().unwrap_or("").to_string();
        if !rel.is_empty() && crate::locations::relative(&rel).is_err() { return Err(Fail::Invalid); }
        Ok(Place::Loc { host, loc, rel })
    }
    fn file(host: &Host, loc: &Loc, rel: &str) -> FileRef { FileRef { endpoint: host.endpoint.clone(), location_id: loc.id.clone(), rel_path: rel.into() } }
    /// A folder's entries on the host, cached briefly (Finder asks a lot).
    async fn listing(&self, vdir: &str, host: &Host, loc: &Loc, rel: &str) -> Result<Arc<Vec<Entry>>, Fail> {
        if let Some((at, entries)) = lock(&self.dirs).get(vdir) { if at.elapsed() < LIST_TTL { return Ok(entries.clone()); } }
        let entries = Arc::new(self.engine.list(&host.endpoint, &loc.id, rel).await?);
        let mut dirs = lock(&self.dirs);
        if dirs.len() > 2000 { dirs.retain(|_, (at, _)| at.elapsed() < LIST_TTL); }
        dirs.insert(vdir.into(), (Instant::now(), entries.clone()));
        Ok(entries)
    }
    fn invalidate(&self, vdir: &str) { lock(&self.dirs).remove(vdir); }
    fn dir_node(&self, path: &str, writable: bool) -> Node {
        Node { id: self.id(path), name: split(path).1.into(), is_dir: true, size: 0, modified: 0, writable }
    }
    fn staged_node(&self, path: &str) -> Option<Node> {
        let staged = lock(&self.staged);
        let s = staged.get(path)?;
        Some(Node { id: self.id(path), name: s.name.clone(), is_dir: false, size: s.size, modified: s.modified, writable: true })
    }
    pub async fn attr(self: &Arc<Self>, path: &str) -> Result<Node, Fail> {
        if let Some(node) = self.staged_node(path) { return Ok(node); }
        let place = self.place(path).await.map_err(|f| if f == Fail::Invalid { Fail::NotFound } else { f })?;
        match place {
            Place::Root | Place::Host(_) => Ok(self.dir_node(path, false)),
            Place::Loc { loc, rel, .. } if rel.is_empty() => Ok(self.dir_node(path, loc.upload)),
            Place::Loc { host, loc, rel } => {
                let (vdir, name) = split(path);
                let (rel_dir, _) = split(&rel);
                let entries = self.listing(vdir, &host, &loc, rel_dir).await?;
                let e = entries.iter().find(|e| e.name == name).ok_or(Fail::NotFound)?;
                Ok(Node { id: self.id(path), name: e.name.clone(), is_dir: e.is_dir, size: e.size, modified: e.modified, writable: loc.upload })
            }
        }
    }
    pub async fn list(self: &Arc<Self>, path: &str) -> Result<Vec<Node>, Fail> {
        let mut nodes = match self.place(path).await? {
            Place::Root => self.hosts().await.iter().map(|h| self.dir_node(&h.name, false)).collect(),
            Place::Host(h) => h.locations.iter().map(|l| self.dir_node(&join(path, &l.name), l.upload)).collect(),
            Place::Loc { host, loc, rel } => {
                let entries = self.listing(path, &host, &loc, &rel).await?;
                entries.iter().map(|e| Node { id: self.id(&join(path, &e.name)), name: e.name.clone(), is_dir: e.is_dir, size: e.size, modified: e.modified, writable: loc.upload }).collect::<Vec<_>>()
            }
        };
        // Files written here that the host doesn't list yet.
        let pending: Vec<String> = lock(&self.staged).keys().filter(|p| split(p).0 == path).cloned().collect();
        for p in pending {
            if let Some(node) = self.staged_node(&p) {
                match nodes.iter_mut().find(|n| n.name == node.name) { Some(n) => *n = node, None => nodes.push(node) }
            }
        }
        Ok(nodes)
    }
    pub async fn read(self: &Arc<Self>, path: &str, offset: u64, len: u64) -> Result<Bytes, Fail> {
        let staged = lock(&self.staged).get(path).map(|s| s.file.clone());
        if let Some(file) = staged {
            return tokio::task::spawn_blocking(move || -> Result<Bytes, Fail> {
                let f = std::fs::File::open(&file)?;
                let size = f.metadata()?.len();
                if offset >= size { return Ok(Bytes::new()); }
                let mut buf = vec![0u8; len.min(size - offset) as usize];
                crate::locations::read_exact_at(&f, &mut buf, offset)?;
                Ok(Bytes::from(buf))
            }).await.map_err(|e| Fail::Io(e.to_string()))?;
        }
        match self.place(path).await? {
            Place::Loc { host, loc, rel } if !rel.is_empty() => Ok(self.engine.read(&Self::file(&host, &loc, &rel), offset, len).await?),
            _ => Err(Fail::IsDir),
        }
    }
    /// The location a new or changed file lands in, checked for the upload right.
    async fn writable(self: &Arc<Self>, path: &str) -> Result<(Host, Loc, String), Fail> {
        // Device and location folders are the drive's own; nothing is created there.
        if path.splitn(3, '/').count() < 3 { return Err(Fail::ReadOnly); }
        match self.place(path).await? {
            Place::Loc { host, loc, rel } if !rel.is_empty() => {
                if !loc.upload && !local_only(split(path).1) { return Err(Fail::ReadOnly); }
                Ok((host, loc, rel))
            }
            Place::Loc { .. } | Place::Host(_) | Place::Root => Err(Fail::ReadOnly),
        }
    }
    fn stage(&self, path: &str, host: &Host, loc: &Loc, rel: &str, replace: bool) -> Result<(), Fail> {
        let name = split(path).1.to_string();
        let dir = self.staging.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(&name);
        std::fs::File::create(&file)?;
        lock(&self.staged).insert(path.into(), Staged { file, endpoint: host.endpoint.clone(), location_id: loc.id.clone(),
            rel_dir: split(rel).0.into(), local_only: local_only(&name), name, replace, dirty: true, size: 0, modified: now_ms(),
            last_write: Instant::now(), uploading: None, error: None });
        Ok(())
    }
    /// Create (or truncate) a file. An existing file on the host is replaced
    /// on commit; its previous version goes to the location's trash.
    pub async fn create(self: &Arc<Self>, path: &str, exclusive: bool) -> Result<Node, Fail> {
        let (host, loc, rel) = self.writable(path).await?;
        let existing = match self.attr(path).await { Ok(n) => Some(n), Err(Fail::NotFound) => None, Err(e) => return Err(e) };
        if let Some(n) = &existing { if exclusive { return Err(Fail::Exists); } if n.is_dir { return Err(Fail::IsDir); } }
        if crate::locations::relative(split(path).1).is_err() { return Err(Fail::Invalid); }
        let was_staged = lock(&self.staged).contains_key(path);
        if was_staged { self.set_size(path, 0).await?; } else { self.stage(path, &host, &loc, &rel, existing.is_some())?; }
        self.attr(path).await
    }
    /// Make `path` a local staging copy (fetching the host's bytes) before an edit.
    async fn materialize(self: &Arc<Self>, path: &str) -> Result<PathBuf, Fail> {
        {
            let mut staged = lock(&self.staged);
            if let Some(s) = staged.get_mut(path) {
                if s.uploading.is_none() { return Ok(s.file.clone()); }
                // Being uploaded right now: edit a fresh copy, upload it next.
                let dir = self.staging.join(uuid::Uuid::new_v4().to_string());
                std::fs::create_dir_all(&dir)?;
                let copy = dir.join(&s.name);
                std::fs::copy(&s.file, &copy)?;
                s.file = copy.clone(); s.uploading = None; s.replace = true;
                return Ok(copy);
            }
        }
        let (host, loc, rel) = self.writable(path).await?;
        let node = self.attr(path).await?;
        if node.is_dir { return Err(Fail::IsDir); }
        self.stage(path, &host, &loc, &rel, true)?;
        let file = lock(&self.staged).get(path).map(|s| s.file.clone()).ok_or(Fail::NotFound)?;
        if node.size > 0 {
            if let Err(e) = self.engine.fetch_to_file(&Self::file(&host, &loc, &rel), &file).await {
                lock(&self.staged).remove(path);
                return Err(e.into());
            }
        }
        if let Some(s) = lock(&self.staged).get_mut(path) { s.size = node.size; }
        Ok(file)
    }
    pub async fn write(self: &Arc<Self>, path: &str, offset: u64, data: &[u8]) -> Result<Node, Fail> {
        let file = self.materialize(path).await?;
        let data = data.to_vec();
        let size = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
            let f = std::fs::OpenOptions::new().write(true).open(&file)?;
            #[cfg(unix)] { use std::os::unix::fs::FileExt; f.write_all_at(&data, offset)?; }
            #[cfg(windows)] { use std::os::windows::fs::FileExt; let mut done = 0; while done < data.len() { done += f.seek_write(&data[done..], offset + done as u64)?; } }
            Ok(f.metadata()?.len())
        }).await.map_err(|e| Fail::Io(e.to_string()))??;
        self.touched(path, size);
        self.attr(path).await
    }
    pub async fn set_size(self: &Arc<Self>, path: &str, size: u64) -> Result<Node, Fail> {
        let node = self.attr(path).await?;
        if node.is_dir { return Err(Fail::IsDir); }
        let staged = lock(&self.staged).contains_key(path);
        if !staged && node.size == size { return Ok(node); }
        let file = if !staged && size == 0 {
            let (host, loc, rel) = self.writable(path).await?;
            self.stage(path, &host, &loc, &rel, true)?;
            lock(&self.staged).get(path).map(|s| s.file.clone()).ok_or(Fail::NotFound)?
        } else { self.materialize(path).await? };
        std::fs::OpenOptions::new().write(true).open(&file)?.set_len(size)?;
        self.touched(path, size);
        self.attr(path).await
    }
    fn touched(&self, path: &str, size: u64) {
        if let Some(s) = lock(&self.staged).get_mut(path) {
            s.size = size; s.modified = now_ms(); s.last_write = Instant::now(); s.dirty = true; s.error = None;
        }
    }
    pub async fn mkdir(self: &Arc<Self>, path: &str) -> Result<Node, Fail> {
        let Place::Loc { host, loc, rel } = self.place(path).await? else { return Err(Fail::ReadOnly); };
        if rel.is_empty() { return Err(Fail::Exists); }
        if !loc.manage { return Err(if loc.upload { Fail::Denied } else { Fail::ReadOnly }); }
        self.engine.remote().rpc(&host.endpoint, json!({"kind": "locations.mkdir", "locations_v": crate::locations::VERSION, "id": loc.id, "rel_path": rel})).await?;
        self.invalidate(split(path).0);
        Ok(self.dir_node(path, true))
    }
    pub async fn remove(self: &Arc<Self>, path: &str) -> Result<(), Fail> {
        let removed = lock(&self.staged).remove(path);
        if let Some(s) = removed {
            if s.uploading.is_none() { if let Some(dir) = s.file.parent() { let _ = std::fs::remove_dir_all(dir); } }
            if s.local_only || !s.replace { return Ok(()); }
        }
        let Place::Loc { host, loc, rel } = self.place(path).await? else { return Err(Fail::ReadOnly); };
        if rel.is_empty() { return Err(Fail::ReadOnly); }
        if !loc.manage { return Err(if loc.upload { Fail::Denied } else { Fail::ReadOnly }); }
        self.engine.remote().rpc(&host.endpoint, json!({"kind": "locations.trash", "locations_v": crate::locations::VERSION, "id": loc.id, "rel_path": rel})).await?;
        self.invalidate(split(path).0); self.invalidate(path);
        Ok(())
    }
    pub async fn rename(self: &Arc<Self>, from: &str, to: &str) -> Result<(), Fail> {
        if from == to { return Ok(()); }
        let (Place::Loc { host, loc, rel }, Place::Loc { host: to_host, loc: to_loc, rel: to_rel }) = (self.place(from).await?, self.place(to).await?) else { return Err(Fail::ReadOnly); };
        if host.endpoint != to_host.endpoint || loc.id != to_loc.id { return Err(Fail::Invalid); }
        if rel.is_empty() || to_rel.is_empty() { return Err(Fail::ReadOnly); }
        let (to_dir, to_name) = split(to);
        if crate::locations::relative(to_name).is_err() { return Err(Fail::Invalid); }
        // A file still staged here (an app's "save to temp, then rename over"):
        // rename the local copy; it uploads under the new name.
        let staged = lock(&self.staged).remove(from);
        if let Some(mut s) = staged {
            let target_exists = self.attr(to).await.is_ok();
            let file = s.file.with_file_name(to_name);
            if s.uploading.is_some() { std::fs::copy(&s.file, &file)?; } else { std::fs::rename(&s.file, &file)?; }
            s.file = file; s.name = to_name.into(); s.rel_dir = split(&to_rel).0.into();
            s.local_only = local_only(to_name); s.replace = s.replace || target_exists; s.uploading = None; s.dirty = true; s.last_write = Instant::now();
            if !s.local_only && !loc.upload { lock(&self.staged).insert(from.into(), s); return Err(Fail::ReadOnly); }
            lock(&self.staged).insert(to.into(), s);
            lock(&self.ids).rename(from, to);
            return Ok(());
        }
        if !loc.manage { return Err(if loc.upload { Fail::Denied } else { Fail::ReadOnly }); }
        self.engine.remote().rpc(&host.endpoint, json!({"kind": "locations.rename", "locations_v": crate::locations::VERSION, "id": loc.id, "rel_path": rel, "to": to_rel})).await?;
        lock(&self.ids).rename(from, to);
        self.invalidate(split(from).0); self.invalidate(to_dir); self.invalidate(from);
        Ok(())
    }
    /// Upload a staged file now (end of a WebDAV PUT).
    pub async fn commit(self: &Arc<Self>, path: &str) {
        if let Some(s) = lock(&self.staged).get_mut(path) { s.last_write = Instant::now() - COMMIT_IDLE; }
        self.tick().await;
    }
    /// Every second: upload files idle for COMMIT_IDLE, forget the ones the
    /// host now lists, save file ids.
    pub async fn tick(self: &Arc<Self>) {
        let mut start = Vec::new();
        let mut check = Vec::new();
        {
            let mut staged = lock(&self.staged);
            for (path, s) in staged.iter_mut() {
                if s.local_only { continue; }
                if s.dirty && s.uploading.is_none() && s.last_write.elapsed() >= COMMIT_IDLE {
                    s.uploading = Some(Instant::now()); s.dirty = false;
                    start.push((path.clone(), s.endpoint.clone(), s.location_id.clone(), s.rel_dir.clone(), s.file.clone(), s.replace));
                } else if !s.dirty && s.uploading.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                    check.push((path.clone(), s.size, s.uploading.unwrap()));
                }
            }
        }
        for (path, endpoint, location, rel_dir, file, replace) in start {
            let tree = self.clone();
            tokio::spawn(async move {
                log::info!("drive: uploading a saved file ({} bytes) to a location", std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0));
                if let Err(e) = tree.engine.remote().upload(&endpoint, &location, &rel_dir, &file, replace).await {
                    log::warn!("drive: upload couldn't start: {e:#}");
                    if let Some(s) = lock(&tree.staged).get_mut(&path) { s.uploading = None; s.dirty = true; s.last_write = Instant::now() + Duration::from_secs(10); s.error = Some(format!("{e:#}")); }
                }
            });
        }
        for (path, size, started) in check {
            self.invalidate(split(&path).0);
            let landed = matches!(self.attr_remote(&path).await, Ok(n) if n.size == size);
            if landed || started.elapsed() > Duration::from_secs(6 * 3600) {
                let mut staged = lock(&self.staged);
                if staged.get(&path).is_some_and(|s| !s.dirty && s.uploading == Some(started)) {
                    if let Some(s) = staged.remove(&path) { if let Some(dir) = s.file.parent() { let _ = std::fs::remove_dir_all(dir); } }
                }
            }
        }
        lock(&self.ids).save();
    }
    /// What the host lists for `path`, ignoring the local copy.
    async fn attr_remote(self: &Arc<Self>, path: &str) -> Result<Node, Fail> {
        let Place::Loc { host, loc, rel } = self.place(path).await? else { return Err(Fail::NotFound); };
        let (vdir, name) = split(path);
        let entries = self.listing(vdir, &host, &loc, split(&rel).0).await?;
        let e = entries.iter().find(|e| e.name == name).ok_or(Fail::NotFound)?;
        Ok(Node { id: self.id(path), name: name.into(), is_dir: e.is_dir, size: e.size, modified: e.modified, writable: loc.upload })
    }
    /// The drive path of a Location item, for "Show in Finder/Explorer/Files".
    pub fn virtual_path(&self, friend_id: &str, location_id: &str, rel: &str) -> Option<String> {
        let hosts = lock(&self.hosts);
        let host = hosts.1.iter().find(|h| h.friend_id == friend_id)?;
        let loc = host.locations.iter().find(|l| l.id == location_id)?;
        let base = format!("{}/{}", host.name, loc.name);
        Some(if rel.trim_matches('/').is_empty() { base } else { format!("{base}/{}", rel.trim_matches('/')) })
    }
}

// ── macOS: NFSv3 ────────────────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod nfs {
    use super::*;
    use nfsserve::{nfs::*, vfs::{DirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities}};
    pub struct Nfs { pub tree: Arc<Tree>, uid: u32, gid: u32 }
    impl Nfs { pub fn new(tree: Arc<Tree>) -> Self { Self { tree, uid: unsafe { libc::getuid() }, gid: unsafe { libc::getgid() } } } }
    fn status(f: Fail) -> nfsstat3 {
        match f {
            Fail::NotFound => nfsstat3::NFS3ERR_NOENT, Fail::Denied => nfsstat3::NFS3ERR_ACCES, Fail::ReadOnly => nfsstat3::NFS3ERR_ROFS,
            Fail::Exists => nfsstat3::NFS3ERR_EXIST, Fail::IsDir => nfsstat3::NFS3ERR_ISDIR, Fail::Invalid => nfsstat3::NFS3ERR_INVAL,
            Fail::Io(m) => { log::info!("drive: {m}"); nfsstat3::NFS3ERR_IO }
        }
    }
    fn time(ms: u64) -> nfstime3 { nfstime3 { seconds: (ms / 1000) as u32, nseconds: ((ms % 1000) * 1_000_000) as u32 } }
    fn name(f: &filename3) -> Result<String, nfsstat3> { String::from_utf8(f.0.clone()).map_err(|_| nfsstat3::NFS3ERR_INVAL) }
    impl Nfs {
        fn fattr(&self, n: &Node) -> fattr3 {
            let mode = match (n.is_dir, n.writable) { (true, true) => 0o755, (true, false) => 0o555, (false, true) => 0o644, (false, false) => 0o444 };
            fattr3 { ftype: if n.is_dir { ftype3::NF3DIR } else { ftype3::NF3REG }, mode, nlink: if n.is_dir { 2 } else { 1 },
                uid: self.uid, gid: self.gid, size: n.size, used: n.size, rdev: specdata3::default(), fsid: 0x44_42, fileid: n.id,
                atime: time(n.modified), mtime: time(n.modified), ctime: time(n.modified) }
        }
        fn path(&self, id: fileid3) -> Result<String, nfsstat3> { self.tree.path_of(id).ok_or(nfsstat3::NFS3ERR_STALE) }
        fn child(&self, dir: fileid3, f: &filename3) -> Result<String, nfsstat3> {
            let dir = self.path(dir)?;
            let n = name(f)?;
            if n.is_empty() || n.contains('/') || n == "." || n == ".." { return Err(nfsstat3::NFS3ERR_INVAL); }
            Ok(join(&dir, &n))
        }
    }
    #[async_trait::async_trait]
    impl NFSFileSystem for Nfs {
        fn capabilities(&self) -> VFSCapabilities { VFSCapabilities::ReadWrite }
        fn root_dir(&self) -> fileid3 { 1 }
        async fn lookup(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
            let dir = self.path(dirid)?;
            match name(filename)?.as_str() {
                "." => return Ok(dirid),
                ".." => return Ok(self.tree.id(split(&dir).0)),
                _ => {}
            }
            let path = self.child(dirid, filename)?;
            self.tree.attr(&path).await.map(|n| n.id).map_err(status)
        }
        async fn getattr(&self, id: fileid3) -> Result<fattr3, nfsstat3> {
            let path = self.path(id)?;
            self.tree.attr(&path).await.map(|n| self.fattr(&n)).map_err(status)
        }
        async fn setattr(&self, id: fileid3, setattr: sattr3) -> Result<fattr3, nfsstat3> {
            let path = self.path(id)?;
            let node = match setattr.size { set_size3::size(size) => self.tree.set_size(&path, size).await, set_size3::Void => self.tree.attr(&path).await };
            node.map(|n| self.fattr(&n)).map_err(status)
        }
        async fn read(&self, id: fileid3, offset: u64, count: u32) -> Result<(Vec<u8>, bool), nfsstat3> {
            let path = self.path(id)?;
            let node = self.tree.attr(&path).await.map_err(status)?;
            let bytes = self.tree.read(&path, offset, count as u64).await.map_err(status)?;
            let eof = offset + bytes.len() as u64 >= node.size;
            Ok((bytes.to_vec(), eof))
        }
        async fn write(&self, id: fileid3, offset: u64, data: &[u8]) -> Result<fattr3, nfsstat3> {
            let path = self.path(id)?;
            self.tree.write(&path, offset, data).await.map(|n| self.fattr(&n)).map_err(status)
        }
        async fn create(&self, dirid: fileid3, filename: &filename3, _: sattr3) -> Result<(fileid3, fattr3), nfsstat3> {
            let path = self.child(dirid, filename)?;
            self.tree.create(&path, false).await.map(|n| (n.id, self.fattr(&n))).map_err(status)
        }
        async fn create_exclusive(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
            let path = self.child(dirid, filename)?;
            self.tree.create(&path, true).await.map(|n| n.id).map_err(status)
        }
        async fn mkdir(&self, dirid: fileid3, dirname: &filename3) -> Result<(fileid3, fattr3), nfsstat3> {
            let path = self.child(dirid, dirname)?;
            self.tree.mkdir(&path).await.map(|n| (n.id, self.fattr(&n))).map_err(status)
        }
        async fn remove(&self, dirid: fileid3, filename: &filename3) -> Result<(), nfsstat3> {
            let path = self.child(dirid, filename)?;
            self.tree.remove(&path).await.map_err(status)
        }
        async fn rename(&self, from_dirid: fileid3, from: &filename3, to_dirid: fileid3, to: &filename3) -> Result<(), nfsstat3> {
            let (from, to) = (self.child(from_dirid, from)?, self.child(to_dirid, to)?);
            self.tree.rename(&from, &to).await.map_err(|f| if f == Fail::Invalid { nfsstat3::NFS3ERR_XDEV } else { status(f) })
        }
        async fn readdir(&self, dirid: fileid3, start_after: fileid3, max_entries: usize) -> Result<ReadDirResult, nfsstat3> {
            let path = self.path(dirid)?;
            let mut nodes = self.tree.list(&path).await.map_err(status)?;
            nodes.sort_by_key(|n| n.id);
            let skip = if start_after == 0 { 0 } else { nodes.iter().position(|n| n.id > start_after).unwrap_or(nodes.len()) };
            let entries: Vec<DirEntry> = nodes[skip..].iter().take(max_entries)
                .map(|n| DirEntry { fileid: n.id, name: nfsstring(n.name.clone().into_bytes()), attr: self.fattr(n) }).collect();
            let end = skip + entries.len() >= nodes.len();
            Ok(ReadDirResult { entries, end })
        }
        async fn symlink(&self, _: fileid3, _: &filename3, _: &nfspath3, _: &sattr3) -> Result<(fileid3, fattr3), nfsstat3> { Err(nfsstat3::NFS3ERR_NOTSUPP) }
        async fn readlink(&self, _: fileid3) -> Result<nfspath3, nfsstat3> { Err(nfsstat3::NFS3ERR_NOTSUPP) }
        async fn fsinfo(&self, root: fileid3) -> Result<fsinfo3, nfsstat3> {
            let attr = match self.getattr(root).await { Ok(a) => post_op_attr::attributes(a), Err(_) => post_op_attr::Void };
            Ok(fsinfo3 { obj_attributes: attr, rtmax: 1 << 20, rtpref: 1 << 20, rtmult: 1 << 20, wtmax: 1 << 20, wtpref: 1 << 20, wtmult: 1 << 20,
                dtpref: 1 << 20, maxfilesize: 1 << 50, time_delta: nfstime3 { seconds: 0, nseconds: 1_000_000 }, properties: FSF_HOMOGENEOUS | FSF_CANSETTIME })
        }
    }
}

// ── Linux / Windows: WebDAV on the stream server ─────────────────────────────
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod dav {
    use super::*;
    use dav_server::{davpath::DavPath, fs::*};
    use futures_util::FutureExt;
    #[derive(Clone)]
    pub struct DavFs { pub tree: Arc<Tree> }
    fn fs_error(f: Fail) -> FsError {
        match f {
            Fail::NotFound => FsError::NotFound, Fail::Exists => FsError::Exists, Fail::Invalid => FsError::IsRemote,
            Fail::Denied | Fail::ReadOnly | Fail::IsDir => FsError::Forbidden,
            Fail::Io(m) => { log::info!("drive: {m}"); FsError::GeneralFailure }
        }
    }
    fn vpath(p: &DavPath) -> String { String::from_utf8_lossy(p.as_bytes()).trim_matches('/').to_string() }
    #[derive(Debug, Clone)]
    struct Meta(Node);
    impl DavMetaData for Meta {
        fn len(&self) -> u64 { self.0.size }
        fn modified(&self) -> FsResult<std::time::SystemTime> { Ok(std::time::UNIX_EPOCH + Duration::from_millis(self.0.modified)) }
        fn is_dir(&self) -> bool { self.0.is_dir }
    }
    struct DirEnt(Node);
    impl DavDirEntry for DirEnt {
        fn name(&self) -> Vec<u8> { self.0.name.clone().into_bytes() }
        fn metadata(&self) -> FsFuture<'_, Box<dyn DavMetaData>> { let m = Meta(self.0.clone()); async move { Ok(Box::new(m) as Box<dyn DavMetaData>) }.boxed() }
    }
    struct Handle { tree: Arc<Tree>, path: String, pos: u64, wrote: bool }
    impl std::fmt::Debug for Handle { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "drive file") } }
    impl DavFile for Handle {
        fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
            async move { self.tree.attr(&self.path).await.map(|n| Box::new(Meta(n)) as Box<dyn DavMetaData>).map_err(fs_error) }.boxed()
        }
        fn write_buf(&mut self, mut buf: Box<dyn bytes::Buf + Send>) -> FsFuture<'_, ()> {
            let bytes = buf.copy_to_bytes(buf.remaining());
            self.write_bytes(bytes)
        }
        fn write_bytes(&mut self, buf: Bytes) -> FsFuture<'_, ()> {
            async move {
                self.tree.write(&self.path, self.pos, &buf).await.map_err(fs_error)?;
                self.pos += buf.len() as u64; self.wrote = true; Ok(())
            }.boxed()
        }
        fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
            async move {
                let b = self.tree.read(&self.path, self.pos, count as u64).await.map_err(fs_error)?;
                self.pos += b.len() as u64; Ok(b)
            }.boxed()
        }
        fn seek(&mut self, pos: std::io::SeekFrom) -> FsFuture<'_, u64> {
            async move {
                self.pos = match pos {
                    std::io::SeekFrom::Start(n) => n,
                    std::io::SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
                    std::io::SeekFrom::End(d) => self.tree.attr(&self.path).await.map_err(fs_error)?.size.saturating_add_signed(d),
                };
                Ok(self.pos)
            }.boxed()
        }
        fn flush(&mut self) -> FsFuture<'_, ()> {
            async move { if self.wrote { self.tree.commit(&self.path).await; } Ok(()) }.boxed()
        }
    }
    impl DavFileSystem for DavFs {
        fn open<'a>(&'a self, path: &'a DavPath, options: OpenOptions) -> FsFuture<'a, Box<dyn DavFile>> {
            async move {
                let p = vpath(path);
                if options.write {
                    let exists = self.tree.attr(&p).await.is_ok();
                    if options.create_new && exists { return Err(FsError::Exists); }
                    if options.truncate || !exists { self.tree.create(&p, false).await.map_err(fs_error)?; }
                } else {
                    let n = self.tree.attr(&p).await.map_err(fs_error)?;
                    if n.is_dir { return Err(FsError::Forbidden); }
                }
                Ok(Box::new(Handle { tree: self.tree.clone(), path: p, pos: 0, wrote: options.write && options.truncate }) as Box<dyn DavFile>)
            }.boxed()
        }
        fn read_dir<'a>(&'a self, path: &'a DavPath, _: ReadDirMeta) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
            async move {
                let nodes = self.tree.list(&vpath(path)).await.map_err(fs_error)?;
                let items: Vec<FsResult<Box<dyn DavDirEntry>>> = nodes.into_iter().map(|n| Ok(Box::new(DirEnt(n)) as Box<dyn DavDirEntry>)).collect();
                Ok(Box::pin(futures_util::stream::iter(items)) as FsStream<Box<dyn DavDirEntry>>)
            }.boxed()
        }
        fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
            async move { self.tree.attr(&vpath(path)).await.map(|n| Box::new(Meta(n)) as Box<dyn DavMetaData>).map_err(fs_error) }.boxed()
        }
        fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            async move { self.tree.mkdir(&vpath(path)).await.map(|_| ()).map_err(fs_error) }.boxed()
        }
        fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            async move { self.tree.remove(&vpath(path)).await.map_err(fs_error) }.boxed()
        }
        fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            async move { self.tree.remove(&vpath(path)).await.map_err(fs_error) }.boxed()
        }
        fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
            async move { self.tree.rename(&vpath(from), &vpath(to)).await.map_err(fs_error) }.boxed()
        }
    }
    pub fn handler(tree: Arc<Tree>, token: &str) -> crate::stream::Dav {
        let handler = dav_server::DavHandler::builder().filesystem(Box::new(DavFs { tree })).locksystem(dav_server::fakels::FakeLs::new())
            .strip_prefix(format!("/{token}/dav")).build_handler();
        Arc::new(move |req| {
            let handler = handler.clone();
            Box::pin(async move {
                use http_body_util::BodyExt;
                handler.handle(req).await.map(|b| b.boxed_unsync())
            })
        })
    }
}

// ── Mounting ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveStatus {
    pub supported: bool,
    pub settings: crate::stream::DriveSettings,
    pub mounted: bool,
    pub busy: bool,
    pub path: Option<String>,
    pub error: Option<String>,
    pub devices: usize,
    pub locations: usize,
    pub pending_uploads: usize,
    pub upload_errors: Vec<String>,
    pub cached_bytes: u64,
    /// The same tree over WebDAV (127.0.0.1 only), for any WebDAV client.
    pub dav_url: Option<String>,
    /// Windows only: why the WebDAV client can't map large files, in words.
    pub windows_fix: Option<String>,
}

struct Mounted { path: PathBuf, #[cfg(target_os = "macos")] server: tokio::task::JoinHandle<()>, #[cfg(not(target_os = "macos"))] uri: String }
pub struct Drive { tree: Arc<Tree>, config: PathBuf, mounted: tokio::sync::Mutex<Option<Mounted>>, state: Mutex<(bool, Option<String>)> }
static DRIVE: OnceLock<Arc<Drive>> = OnceLock::new();
pub fn drive() -> Result<Arc<Drive>> { DRIVE.get().cloned().context("DropBeam is still starting") }

pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux", windows));

/// Start the drive (after stream::init): file ids, the commit ticker, stale
/// mount recovery, and the auto-mount when there is anything to show.
pub fn init(config: &Path, cache: &Path) {
    let Ok(engine) = crate::stream::engine() else { return; };
    let tree = Tree::new(engine, config, cache.join("drive-staging"));
    let drive = Arc::new(Drive { tree: tree.clone(), config: config.to_path_buf(), mounted: Default::default(), state: Mutex::new((false, None)) });
    if DRIVE.set(drive.clone()).is_err() { return; }
    // The WebDAV view is always there (127.0.0.1 + token); Linux and Windows mount it.
    let dav_tree = tree.clone();
    tauri::async_runtime::spawn(async move {
        if let Ok(server) = crate::stream::server().await { server.set_dav(Some(dav::handler(dav_tree, &server.token))); }
    });
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut since_hosts = 0u32;
        loop {
            tick.tick().await;
            tree.tick().await;
            since_hosts += 1;
            if since_hosts >= HOSTS_TTL.as_secs() as u32 { since_hosts = 0; tree.refresh_hosts().await; }
        }
    });
    if !SUPPORTED { return; }
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(recover_stale).await;
        let settings = crate::stream::load_settings(&drive.config);
        if !(settings.enabled && settings.auto_mount) { return; }
        // Let the network come up, then mount only when there's something to show.
        for wait in [8u64, 30, 120] {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            drive.tree.refresh_hosts().await;
            if drive.tree.counts().1 > 0 {
                if let Err(e) = drive.mount().await { log::warn!("drive: auto-mount failed: {e:#}"); }
                return;
            }
        }
    });
}

impl Drive {
    pub fn tree(&self) -> &Arc<Tree> { &self.tree }
    pub async fn status(&self) -> DriveStatus {
        let path = self.mounted.lock().await.as_ref().map(|m| m.path.to_string_lossy().into_owned());
        let (busy, error) = lock(&self.state).clone();
        let (devices, locations) = self.tree.counts();
        DriveStatus { supported: SUPPORTED, settings: crate::stream::load_settings(&self.config), mounted: path.is_some(), busy, path, error,
            devices, locations, pending_uploads: self.tree.pending_uploads(), upload_errors: self.tree.upload_errors(),
            cached_bytes: crate::stream::engine().map(|e| e.cached_bytes()).unwrap_or(0),
            dav_url: if SUPPORTED { crate::stream::server().await.ok().map(|s| s.dav_url()) } else { None }, windows_fix: windows_fix_needed() }
    }
    pub async fn mount(&self) -> Result<PathBuf> {
        anyhow::ensure!(SUPPORTED, "Showing Locations as a drive isn't available on this device");
        let mut mounted = self.mounted.lock().await;
        if let Some(m) = mounted.as_ref() { return Ok(m.path.clone()); }
        *lock(&self.state) = (true, None);
        let result = self.mount_inner().await;
        match result {
            Ok(m) => { let p = m.path.clone(); *mounted = Some(m); *lock(&self.state) = (false, None); log::info!("drive: mounted"); Ok(p) }
            Err(e) => { *lock(&self.state) = (false, Some(format!("{e:#}"))); Err(e) }
        }
    }
    pub async fn unmount(&self) -> Result<()> {
        let mut mounted = self.mounted.lock().await;
        let Some(m) = mounted.take() else { return Ok(()); };
        let result = unmount_os(&m).await;
        if result.is_err() { *mounted = Some(m); return result; }
        #[cfg(target_os = "macos")] m.server.abort();
        *lock(&self.state) = (false, None);
        Ok(())
    }
    pub async fn mount_path(&self) -> Option<PathBuf> { self.mounted.lock().await.as_ref().map(|m| m.path.clone()) }

    #[cfg(target_os = "macos")]
    async fn mount_inner(&self) -> Result<Mounted> {
        use nfsserve::tcp::{NFSTcp, NFSTcpListener};
        let path = mount_point()?;
        tokio::task::spawn_blocking(recover_stale).await?;
        std::fs::create_dir_all(&path)?;
        let leftovers: Vec<_> = std::fs::read_dir(&path)?.flatten().filter(|e| e.file_name() != ".DS_Store").collect();
        anyhow::ensure!(leftovers.is_empty(), "The folder {} isn't empty, so the drive can't appear there. Move its contents and try again.", path.display());
        let listener = NFSTcpListener::bind("127.0.0.1:0", nfs::Nfs::new(self.tree.clone())).await?;
        let port = listener.get_listen_port();
        let server = tokio::spawn(async move { if let Err(e) = listener.handle_forever().await { log::warn!("drive: NFS server stopped: {e}"); } });
        // Options as hf-mount / xet use them, plus a soft mount that fails fast
        // (and is dropped after `deadtimeout`) if DropBeam stops answering.
        let options = format!("locallocks,vers=3,tcp,port={port},mountport={port},soft,intr,timeo=300,retrans=2,deadtimeout=60,rsize=1048576,wsize=1048576,actimeo=5");
        let target = path.clone();
        let out = tokio::time::timeout(Duration::from_secs(40), tokio::task::spawn_blocking(move || {
            std::process::Command::new("/sbin/mount_nfs").args(["-o", &options, "127.0.0.1:/"]).arg(&target).output()
        })).await;
        let out = match out { Ok(Ok(Ok(o))) => o, other => { server.abort(); anyhow::bail!("Couldn’t mount the drive ({other:?})"); } };
        if !out.status.success() {
            server.abort();
            anyhow::bail!("Couldn’t mount the drive: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(Mounted { path, server })
    }
    #[cfg(target_os = "linux")]
    async fn mount_inner(&self) -> Result<Mounted> {
        let server = crate::stream::server().await?;
        let uri = format!("dav://127.0.0.1:{}/{}/dav/", server.port, server.token);
        let run = uri.clone();
        let out = tokio::time::timeout(Duration::from_secs(40), tokio::task::spawn_blocking(move || {
            std::process::Command::new("gio").args(["mount", &run]).output()
        })).await.context("Mounting took too long")??
            .map_err(|e| anyhow::anyhow!("Couldn’t run gio to mount the drive ({e}). Install gvfs (gvfs-backends) and try again."))?;
        anyhow::ensure!(out.status.success(), "Couldn’t mount the drive: {}", String::from_utf8_lossy(&out.stderr).trim());
        let uid = unsafe { libc::getuid() };
        let path = PathBuf::from(format!("/run/user/{uid}/gvfs/dav:host=127.0.0.1,port={},ssl=false,prefix=%2F{}%2Fdav", server.port, server.token));
        Ok(Mounted { path, uri })
    }
    #[cfg(windows)]
    async fn mount_inner(&self) -> Result<Mounted> {
        let server = crate::stream::server().await?;
        let unc = windows_unc(server.port, &server.token);
        let letter = tokio::task::spawn_blocking(free_letter).await?.context("No free drive letter")?;
        let (l, u) = (letter.clone(), unc.clone());
        let out = tokio::time::timeout(Duration::from_secs(60), tokio::task::spawn_blocking(move || {
            hidden("net").args(["use", &l, &u, "/persistent:no"]).output()
        })).await.context("Mapping the drive took too long")???;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let hint = windows_fix_needed().map(|f| format!(" {f}")).unwrap_or_default();
            anyhow::bail!("Windows couldn’t map the drive: {why}.{hint}");
        }
        Ok(Mounted { path: PathBuf::from(format!("{letter}\\")), uri: letter })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    async fn mount_inner(&self) -> Result<Mounted> { anyhow::bail!("Showing Locations as a drive isn't available on this device") }
}

#[cfg(target_os = "macos")]
fn mount_point() -> Result<PathBuf> { Ok(PathBuf::from(std::env::var_os("HOME").context("No home folder")?).join(MOUNT_NAME)) }

/// A mount left behind by a crash (the server it pointed at is gone).
fn recover_stale() {
    #[cfg(target_os = "macos")] {
        let Ok(mp) = mount_point() else { return; };
        let Ok(out) = std::process::Command::new("/sbin/mount").output() else { return; };
        let needle = format!(" on {} (nfs", mp.display());
        if String::from_utf8_lossy(&out.stdout).lines().any(|l| l.starts_with("127.0.0.1:") && l.contains(&needle)) {
            log::info!("drive: removing a mount left by an earlier run");
            let ok = std::process::Command::new("/sbin/umount").arg("-f").arg(&mp).status().is_ok_and(|s| s.success());
            if !ok { let _ = std::process::Command::new("/usr/sbin/diskutil").args(["unmount", "force"]).arg(&mp).status(); }
        }
    }
    #[cfg(target_os = "linux")] {
        let Ok(out) = std::process::Command::new("gio").args(["mount", "-l"]).output() else { return; };
        for uri in stale_dav_uris(&String::from_utf8_lossy(&out.stdout)) {
            log::info!("drive: removing a mount left by an earlier run");
            let _ = std::process::Command::new("gio").args(["mount", "-u", &uri]).status();
        }
    }
    #[cfg(windows)] {
        let Ok(out) = hidden("net").arg("use").output() else { return; };
        for letter in stale_net_letters(&String::from_utf8_lossy(&out.stdout)) {
            log::info!("drive: removing a mapping left by an earlier run");
            let _ = hidden("net").args(["use", &letter, "/delete", "/y"]).status();
        }
    }
}
/// `gio mount -l` lines naming our localhost WebDAV mounts.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn stale_dav_uris(listing: &str) -> Vec<String> {
    listing.split_whitespace().filter(|w| w.starts_with("dav://127.0.0.1:") && w.contains("/dav")).map(str::to_string).collect()
}
/// `net use` rows mapping a drive letter to our localhost WebDAV share.
#[cfg_attr(not(windows), allow(dead_code))]
fn stale_net_letters(listing: &str) -> Vec<String> {
    listing.lines().filter(|l| l.contains("\\\\127.0.0.1@")).filter_map(|l| l.split_whitespace().find(|w| w.len() == 2 && w.ends_with(':')).map(str::to_string)).collect()
}
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_unc(port: u16, token: &str) -> String { format!("\\\\127.0.0.1@{port}\\{token}\\dav") }

async fn unmount_os(m: &Mounted) -> Result<()> {
    #[cfg(target_os = "macos")] {
        let path = m.path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if std::process::Command::new("/sbin/umount").arg(&path).status().is_ok_and(|s| s.success()) { return Ok(()); }
            let out = std::process::Command::new("/usr/sbin/diskutil").args(["unmount", "force"]).arg(&path).output()?;
            anyhow::ensure!(out.status.success(), "The drive is in use. Close files opened from it and try again.");
            Ok(())
        }).await?
    }
    #[cfg(target_os = "linux")] {
        let uri = m.uri.clone();
        let out = tokio::task::spawn_blocking(move || std::process::Command::new("gio").args(["mount", "-u", &uri]).output()).await??;
        anyhow::ensure!(out.status.success(), "Couldn’t remove the drive: {}", String::from_utf8_lossy(&out.stderr).trim());
        Ok(())
    }
    #[cfg(windows)] {
        let letter = m.uri.clone();
        let out = tokio::task::spawn_blocking(move || hidden("net").args(["use", &letter, "/delete", "/y"]).output()).await??;
        anyhow::ensure!(out.status.success(), "The drive is in use. Close files opened from it and try again.");
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))] { let _ = m; Ok(()) }
}

/// Unmount on quit (best effort, bounded so quitting never hangs).
pub fn shutdown_blocking() {
    let Some(drive) = DRIVE.get().cloned() else { return; };
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = done.clone();
    tauri::async_runtime::spawn(async move { let _ = drive.unmount().await; flag.store(true, Ordering::SeqCst); });
    let start = Instant::now();
    while !done.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(50)); }
}

#[cfg(windows)]
fn hidden(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(program);
    c.creation_flags(0x0800_0000);
    c
}
#[cfg(windows)]
fn free_letter() -> Option<String> {
    ('D'..='Z').rev().map(|c| format!("{c}:")).find(|l| !Path::new(&format!("{l}\\")).exists())
}
/// Windows' WebDAV client (WebClient) must be running, and by default refuses
/// files over 50 MB. Returns what's wrong in words, or None when it's fine.
#[cfg(windows)]
pub fn windows_fix_needed() -> Option<String> {
    let service = hidden("sc").args(["qc", "WebClient"]).output().ok()?;
    let text = String::from_utf8_lossy(&service.stdout).to_string();
    if !service.status.success() || text.is_empty() { return Some("The Windows WebDAV client (WebClient) isn’t installed. Turn on “WebDAV Redirector” in Windows Features.".into()); }
    if text.contains("DISABLED") { return Some("The Windows WebDAV client (WebClient) is turned off.".into()); }
    let limit = hidden("reg").args(["query", r"HKLM\SYSTEM\CurrentControlSet\Services\WebClient\Parameters", "/v", "FileSizeLimitInBytes"]).output().ok()?;
    let value = String::from_utf8_lossy(&limit.stdout).split_whitespace().last().and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(50_000_000);
    (value < 4_000_000_000).then(|| "Windows limits files on this kind of drive to 50 MB.".into())
}
#[cfg(not(windows))]
pub fn windows_fix_needed() -> Option<String> { None }
/// One elevated step (Windows asks for permission): start WebClient
/// automatically and raise its file size limit to the 4 GB maximum.
#[cfg(windows)]
pub fn windows_fix() -> Result<()> {
    let script = r"Set-Service WebClient -StartupType Automatic; Set-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Services\WebClient\Parameters' -Name FileSizeLimitInBytes -Value 4294967295 -Type DWord; Restart-Service WebClient -Force";
    let outer = format!("Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','{}'", script.replace('\'', "''"));
    let status = hidden("powershell").args(["-NoProfile", "-Command", &outer]).status()?;
    anyhow::ensure!(status.success(), "Windows didn’t allow the change");
    Ok(())
}
#[cfg(not(windows))]
pub fn windows_fix() -> Result<()> { anyhow::bail!("Only needed on Windows") }

// ── Commands ────────────────────────────────────────────────────────────────

fn text(e: anyhow::Error) -> String { format!("{e:#}") }

/// Is the drive shown, where, and what's waiting to upload.
#[tauri::command]
pub async fn drive_status() -> Result<DriveStatus, String> {
    Ok(drive().map_err(text)?.status().await)
}
/// Show Locations as a drive now (Finder / Files / Explorer). Returns its path.
#[tauri::command]
pub async fn drive_mount() -> Result<String, String> {
    let d = drive().map_err(text)?;
    d.tree.refresh_hosts().await;
    d.mount().await.map(|p| p.to_string_lossy().into_owned()).map_err(text)
}
#[tauri::command]
pub async fn drive_unmount() -> Result<(), String> {
    drive().map_err(text)?.unmount().await.map_err(text)
}
/// Ask friends again which Locations they share (the drive's top folders).
#[tauri::command]
pub async fn drive_refresh() -> Result<DriveStatus, String> {
    let d = drive().map_err(text)?;
    d.tree.refresh_hosts().await;
    Ok(d.status().await)
}
/// Settings → Drives. Turning the drive off removes it; turning it on shows it.
#[tauri::command]
pub async fn drive_settings_set(settings: crate::stream::DriveSettings) -> Result<DriveStatus, String> {
    let d = drive().map_err(text)?;
    let before = crate::stream::load_settings(&d.config);
    crate::stream::save_settings(&d.config, &settings).map_err(text)?;
    if before.enabled && !settings.enabled { d.unmount().await.map_err(text)?; }
    if !before.enabled && settings.enabled && SUPPORTED {
        d.tree.refresh_hosts().await;
        if let Err(e) = d.mount().await { log::warn!("drive: mount after turning on failed: {e:#}"); }
    }
    Ok(d.status().await)
}
/// "Open drive" / "Show in Finder/Explorer/Files": mounts if needed, then opens
/// the drive (or reveals one Location item in it).
#[tauri::command]
pub async fn drive_open(app: tauri::AppHandle, friend_id: Option<String>, location_id: Option<String>, rel_path: Option<String>) -> Result<String, String> {
    use tauri_plugin_opener::OpenerExt;
    let d = drive().map_err(text)?;
    d.tree.refresh_hosts().await;
    let mount = d.mount().await.map_err(text)?;
    let target = match (friend_id, location_id) {
        (Some(f), Some(l)) => {
            let rel = d.tree.virtual_path(&f, &l, rel_path.as_deref().unwrap_or("")).ok_or("This location isn’t on the drive yet. Try again in a moment.")?;
            rel.split('/').fold(mount.clone(), |p, part| p.join(part))
        }
        _ => mount.clone(),
    };
    let is_dir = tokio::fs::metadata(&target).await.map(|m| m.is_dir()).unwrap_or(true);
    if is_dir { app.opener().open_path(target.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())?; }
    else { app.opener().reveal_item_in_dir(&target).map_err(|e| e.to_string())?; }
    Ok(target.to_string_lossy().into_owned())
}
/// Windows: allow the drive's large files (asks for administrator permission).
#[tauri::command]
pub async fn drive_fix_windows() -> Result<DriveStatus, String> {
    tokio::task::spawn_blocking(windows_fix).await.map_err(|e| e.to_string())?.map_err(text)?;
    drive_status().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{Remote, Stat};
    use tokio::sync::mpsc;

    /// A friend device with one Location held in memory.
    struct Host { files: Mutex<HashMap<String, Vec<u8>>>, dirs: Mutex<Vec<String>>, uploads: Mutex<Vec<(String, bool)>>, manage: bool }
    #[async_trait::async_trait]
    impl Remote for Host {
        async fn stat(&self, f: &FileRef) -> Result<Stat> {
            let files = lock(&self.files);
            let b = files.get(&f.rel_path).context("Not found")?;
            Ok(Stat { is_dir: false, size: b.len() as u64, modified: 5, etag: format!("{}", b.len()) })
        }
        async fn read(&self, f: &FileRef, offset: u64, len: u64, _: &str) -> Result<mpsc::Receiver<Result<Bytes>>> {
            let b = lock(&self.files).get(&f.rel_path).cloned().context("Not found")?;
            let (tx, rx) = mpsc::channel(4);
            tx.send(Ok(Bytes::copy_from_slice(&b[offset as usize..(offset + len) as usize]))).await.unwrap();
            Ok(rx)
        }
        async fn rpc(&self, _: &str, r: serde_json::Value) -> Result<serde_json::Value> {
            let rel = r["rel_path"].as_str().unwrap_or("").to_string();
            match r["kind"].as_str().unwrap() {
                "locations.list" => Ok(json!([{"id": "nas", "name": "Media/Drive", "rights": {"upload": true, "manage": self.manage}}])),
                "locations.ls" => {
                    let prefix = if rel.is_empty() { String::new() } else { format!("{rel}/") };
                    let mut entries: Vec<_> = lock(&self.files).iter().filter_map(|(p, b)| p.strip_prefix(&prefix).filter(|n| !n.contains('/')).map(|n| json!({"name": n, "isDir": false, "size": b.len(), "modified": 5}))).collect();
                    entries.extend(lock(&self.dirs).iter().filter_map(|d| d.strip_prefix(&prefix).filter(|n| !n.contains('/')).map(|n| json!({"name": n, "isDir": true, "size": 0, "modified": 5}))));
                    Ok(json!({"entries": entries, "hasMore": false}))
                }
                "locations.mkdir" => { anyhow::ensure!(self.manage, "Location permission denied"); lock(&self.dirs).push(rel); Ok(json!({})) }
                "locations.trash" => { anyhow::ensure!(self.manage, "Location permission denied"); lock(&self.files).remove(&rel).context("Not found")?; Ok(json!({})) }
                "locations.rename" => {
                    anyhow::ensure!(self.manage, "Location permission denied");
                    let b = lock(&self.files).remove(&rel).context("Not found")?;
                    lock(&self.files).insert(r["to"].as_str().unwrap().into(), b); Ok(json!({}))
                }
                k => anyhow::bail!("unexpected {k}"),
            }
        }
        fn devices(&self) -> Vec<(String, String, String)> { vec![("f1".into(), "ep1".into(), "Ashton's PC".into()), ("f2".into(), "ep2".into(), "Ashton's PC".into())] }
        async fn upload(&self, endpoint: &str, _: &str, rel_dir: &str, file: &Path, replace: bool) -> Result<String> {
            if endpoint != "ep1" { anyhow::bail!("offline"); }
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            lock(&self.uploads).push((join(rel_dir, &name), replace));
            lock(&self.files).insert(join(rel_dir, &name), std::fs::read(file)?);
            Ok("t".into())
        }
        fn path(&self, _: &str) -> Option<(String, Option<u64>)> { None }
    }
    fn setup(manage: bool) -> (Arc<Tree>, Arc<Host>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dropbeam-drive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let host = Arc::new(Host { files: Mutex::new(HashMap::from([("movie.mp4".to_string(), vec![9u8; 3_000_000]), ("docs/a.txt".to_string(), b"hello".to_vec())])), dirs: Mutex::new(vec!["docs".into()]), uploads: Default::default(), manage });
        let engine = Engine::new(host.clone(), dir.join("blocks"), 1 << 30);
        (Tree::new(engine, &dir, dir.join("staging")), host, dir)
    }

    #[tokio::test]
    async fn the_tree_maps_devices_and_locations_to_unique_safe_folders() {
        let (tree, _, dir) = setup(true);
        let root: Vec<String> = tree.list("").await.unwrap().into_iter().map(|n| n.name).collect();
        assert_eq!(root, vec!["Ashton's PC", "Ashton's PC (2)"]);
        let locs = tree.list("Ashton's PC").await.unwrap();
        assert_eq!(locs[0].name, "Media-Drive");
        let mut names: Vec<_> = tree.list("Ashton's PC/Media-Drive").await.unwrap().into_iter().map(|n| (n.name, n.is_dir)).collect();
        names.sort();
        assert_eq!(names, vec![("docs".to_string(), true), ("movie.mp4".to_string(), false)]);
        let movie = tree.attr("Ashton's PC/Media-Drive/movie.mp4").await.unwrap();
        assert_eq!(movie.size, 3_000_000);
        assert_eq!(&tree.read("Ashton's PC/Media-Drive/docs/a.txt", 1, 3).await.unwrap()[..], b"ell");
        assert_eq!(tree.read("Ashton's PC/Media-Drive/movie.mp4", 2_999_990, 100).await.unwrap().len(), 10);
        assert_eq!(tree.attr("Ashton's PC/Nope").await, Err(Fail::NotFound));
        assert_eq!(tree.attr("Ashton's PC/Media-Drive/../../etc").await, Err(Fail::NotFound));
        assert_eq!(tree.virtual_path("f1", "nas", "docs/a.txt").as_deref(), Some("Ashton's PC/Media-Drive/docs/a.txt"));
        // File ids are stable, survive a restart and follow a rename.
        let id = movie.id;
        tree.tick().await;
        let again = Ids::load(dir.join("drive-ids.json"));
        assert_eq!(again.by_path.get("Ashton's PC/Media-Drive/movie.mp4"), Some(&id));
        tree.rename("Ashton's PC/Media-Drive/movie.mp4", "Ashton's PC/Media-Drive/film.mp4").await.unwrap();
        assert_eq!(tree.attr("Ashton's PC/Media-Drive/film.mp4").await.unwrap().id, id);
    }

    #[tokio::test]
    async fn writes_stage_locally_then_upload_through_the_verified_path() {
        let (tree, host, _) = setup(false);
        let p = "Ashton's PC/Media-Drive/docs/new.txt";
        tree.create(p, true).await.unwrap();
        tree.write(p, 0, b"hello ").await.unwrap();
        tree.write(p, 6, b"world").await.unwrap();
        assert_eq!(tree.attr(p).await.unwrap().size, 11);
        assert_eq!(&tree.read(p, 0, 100).await.unwrap()[..], b"hello world");
        assert!(tree.list("Ashton's PC/Media-Drive/docs").await.unwrap().iter().any(|n| n.name == "new.txt"));
        // Nothing reaches the host until the file has been idle.
        tree.tick().await;
        assert!(lock(&host.uploads).is_empty());
        tokio::time::sleep(COMMIT_IDLE + Duration::from_millis(100)).await;
        tree.tick().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(lock(&host.uploads).clone(), vec![("docs/new.txt".to_string(), false)]);
        assert_eq!(lock(&host.files).get("docs/new.txt").unwrap(), b"hello world");
        // Editing an existing file replaces it (its old version goes to trash on the host).
        let a = "Ashton's PC/Media-Drive/docs/a.txt";
        tree.write(a, 5, b"!").await.unwrap();
        assert_eq!(&tree.read(a, 0, 10).await.unwrap()[..], b"hello!");
        tree.commit(a).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(lock(&host.uploads).contains(&("docs/a.txt".to_string(), true)));
        // Finder's own files never leave this device.
        tree.create("Ashton's PC/Media-Drive/.DS_Store", false).await.unwrap();
        tree.commit("Ashton's PC/Media-Drive/.DS_Store").await;
        assert!(!lock(&host.files).contains_key(".DS_Store"));
        // Without "rename & delete": no folders, no deletes, no renames on the host.
        assert_eq!(tree.mkdir("Ashton's PC/Media-Drive/new").await, Err(Fail::Denied));
        assert_eq!(tree.remove("Ashton's PC/Media-Drive/movie.mp4").await, Err(Fail::Denied));
        assert_eq!(tree.rename("Ashton's PC/Media-Drive/movie.mp4", "Ashton's PC/Media-Drive/m.mp4").await, Err(Fail::Denied));
        // Nothing can be created at the top levels.
        assert_eq!(tree.create("Ashton's PC/x.txt", false).await, Err(Fail::ReadOnly));
        assert_eq!(tree.create("Ashton's PC/Media-Drive/.dropbeam-trash", false).await, Err(Fail::Invalid));
        // An upload that can't start stays local and is retried.
        tree.create("Ashton's PC (2)/Media-Drive/b.txt", false).await.unwrap();
        tree.commit("Ashton's PC (2)/Media-Drive/b.txt").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(tree.pending_uploads() >= 1);
        assert!(tree.upload_errors().iter().any(|e| e.contains("offline")));
    }

    #[tokio::test]
    async fn manage_rights_map_to_mkdir_trash_and_rename_on_the_host() {
        let (tree, host, _) = setup(true);
        tree.mkdir("Ashton's PC/Media-Drive/Photos").await.unwrap();
        assert!(lock(&host.dirs).contains(&"Photos".to_string()));
        tree.rename("Ashton's PC/Media-Drive/docs/a.txt", "Ashton's PC/Media-Drive/docs/b.txt").await.unwrap();
        assert!(lock(&host.files).contains_key("docs/b.txt"));
        tree.remove("Ashton's PC/Media-Drive/docs/b.txt").await.unwrap();
        assert!(!lock(&host.files).contains_key("docs/b.txt"));
        // Save-to-temp-then-rename: the temp file uploads under the final name.
        let tmp = "Ashton's PC/Media-Drive/movie.mp4.sb-123";
        tree.create(tmp, true).await.unwrap();
        tree.write(tmp, 0, b"new cut").await.unwrap();
        tree.rename(tmp, "Ashton's PC/Media-Drive/movie.mp4").await.unwrap();
        tree.commit("Ashton's PC/Media-Drive/movie.mp4").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(lock(&host.uploads).contains(&("movie.mp4".to_string(), true)));
        assert_eq!(lock(&host.files).get("movie.mp4").unwrap(), b"new cut");
        // Renames across locations or devices are refused (NFS answers EXDEV).
        assert_eq!(tree.rename("Ashton's PC/Media-Drive/movie.mp4", "Ashton's PC (2)/Media-Drive/movie.mp4").await, Err(Fail::Invalid));
    }

    /// The WebDAV view Linux and Windows mount: PROPFIND, ranged GET, PUT.
    #[tokio::test]
    async fn webdav_lists_reads_ranges_and_uploads_on_put() {
        let (tree, host, _) = setup(false);
        let engine = crate::stream::Engine::new(tree.engine.remote().clone(), std::env::temp_dir().join(format!("dropbeam-dav-{}", uuid::Uuid::new_v4())), 1 << 30);
        let server = crate::stream::Server::start(engine, Arc::new(|_: &str| None)).await.unwrap();
        server.set_dav(Some(dav::handler(tree.clone(), &server.token)));
        let base = server.dav_url();
        let client = reqwest::Client::new();
        let propfind = reqwest::Method::from_bytes(b"PROPFIND").unwrap();
        let listing = client.request(propfind.clone(), format!("{base}Ashton%27s%20PC/Media-Drive/")).header("Depth", "1").send().await.unwrap();
        assert_eq!(listing.status(), 207);
        let xml = listing.text().await.unwrap();
        assert!(xml.contains("movie.mp4") && xml.contains("docs"), "{xml}");
        let part = client.get(format!("{base}Ashton%27s%20PC/Media-Drive/movie.mp4")).header("Range", "bytes=10-19").send().await.unwrap();
        assert_eq!(part.status(), 206);
        assert_eq!(part.bytes().await.unwrap().len(), 10);
        let put = client.put(format!("{base}Ashton%27s%20PC/Media-Drive/docs/put.txt")).body("over webdav").send().await.unwrap();
        assert!(put.status().is_success(), "{}", put.status());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(lock(&host.files).get("docs/put.txt").map(|b| b.as_slice()), Some(&b"over webdav"[..]));
        // Without the token nothing is served.
        let wrong = base.replace(&server.token, "nope");
        assert_eq!(client.request(propfind, wrong).header("Depth", "1").send().await.unwrap().status(), 404);
    }

    #[test]
    fn names_and_leftover_mounts_parse() {
        assert_eq!(display_name("a/b:c", &[]), "a-b-c");
        assert_eq!(display_name("", &[]), "Untitled");
        assert_eq!(display_name("NAS", &["nas".into()]), "NAS (2)");
        assert!(local_only("._movie.mp4") && local_only(".DS_Store") && local_only("~$report.docx") && !local_only("report.docx"));
        assert_eq!(stale_dav_uris("Mount(0): 127.0.0.1 -> dav://127.0.0.1:41234/abc/dav/\n  Type: GDaemonMount"), vec!["dav://127.0.0.1:41234/abc/dav/"]);
        let net = "OK           Z:        \\\\127.0.0.1@51234\\abc\\dav   Web Client Network\nOK  Y: \\\\nas\\share  Microsoft Windows Network";
        assert_eq!(stale_net_letters(net), vec!["Z:"]);
        assert_eq!(windows_unc(5000, "tok"), "\\\\127.0.0.1@5000\\tok\\dav");
    }
}
