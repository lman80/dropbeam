//! Explicitly shared, non-mirroring folders. Native no-replace or exclusive
//! reservation publication; see docs/locations-phase1.md for the external-writer
//! race that ordinary rename cannot eliminate in reservation mode.
//! Unix filesystem access is descriptor-relative, O_NOFOLLOW and same-device.
//! Canonical paths are checked as well; an unavailable mount fails closed.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::{Component, Path, PathBuf}, sync::{Arc, Mutex, LazyLock}, collections::HashMap, time::{Duration, Instant}};

pub const VERSION: u64 = 1;
pub const PAGE_SIZE: usize = 500;
static CONFIG_LOCK: Mutex<()> = Mutex::new(());
// Settings revocation invalidates upload checks immediately, even within the
// one-second recheck interval. External config/friend changes use that interval.
#[derive(Default)]
struct Operation { mutex: Mutex<()>, revision: std::sync::atomic::AtomicU64 }
// Only ACL rechecks and namespace mutations take this lock; never byte copying or ls.
static OPERATIONS: LazyLock<Mutex<HashMap<(PathBuf, String), std::sync::Weak<Operation>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn operation(config: &Path, id: &str) -> Arc<Operation> {
    let mut locks = OPERATIONS.lock().unwrap_or_else(|p| p.into_inner());
    locks.retain(|_, v| v.strong_count() > 0);
    locks.entry((config.into(), id.into())).or_default().upgrade().unwrap_or_else(|| {
        let lock = Arc::new(Operation::default());
        locks.insert((config.into(), id.into()), Arc::downgrade(&lock)); lock
    })
}
pub fn default_byte_cap() -> u64 { 20_000_000_000 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationError { Quota, UnsafeRoot, MirrorOverlap, MountChanged, Permission, Busy, RateLimit }
impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Quota => "Location byte cap exceeded; reduce the selection or ask the host to raise the cap",
            Self::UnsafeRoot => "Unsafe location root: contains protected home, system or app configuration data",
            Self::MirrorOverlap => "Location overlaps a paired mirror folder",
            Self::MountChanged => "Location mount identity or marker changed; restore the original mount",
            Self::Permission => "Location permission denied",
            Self::Busy => "At most 2 location transfers per friend; retry after a transfer finishes",
            Self::RateLimit => "Location request rate limit (10/s); retry shortly",
        })
    }
}
impl std::error::Error for LocationError {}
fn yes() -> bool { true }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rights {
    #[serde(default = "yes")] pub upload: bool,
    #[serde(default)] pub manage: bool,
}
impl Default for Rights { fn default() -> Self { Self { upload: true, manage: false } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)] pub friend_ids: Vec<String>,
    #[serde(default)] pub rights: Rights,
    #[serde(default = "default_byte_cap")] pub byte_cap: u64,
    #[serde(default)] pub device: Option<u64>,
    #[serde(default)] pub marker: Option<String>,
    #[serde(default)] pub safe_publish: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target { pub location_id: String, pub rel_path: String }
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry { pub name: String, pub is_dir: bool, pub size: u64, pub modified: u64 }

pub fn load(config: &Path) -> Result<Vec<Location>> {
    match fs::read(config.join("locations.json")) {
        Ok(b) => Ok(serde_json::from_slice(&b).context("Cannot read locations.json; existing configuration was preserved")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}
pub fn hosted(config: &Path) -> Result<Vec<Location>> {
    let mut list = load(config)?;
    for l in &mut list {
        match Root::for_location(l) {
            #[cfg(unix)] Ok(root) => l.safe_publish = Some(if root.native { "native" } else { "reservation" }.into()),
            #[cfg(not(unix))] Ok(_) => {},
            Err(e) => l.safe_publish = Some(format!("unavailable ({e})")),
        }
    }
    Ok(list)
}

pub fn save(config: &Path, location: Option<Location>, remove: Option<&str>) -> Result<Vec<Location>> {
    let id = location.as_ref().map(|l| l.id.as_str()).or(remove).unwrap_or("");
    let lock = operation(config, id);
    let _op = lock.mutex.lock().unwrap_or_else(|p| p.into_inner());
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut list = load(config)?;
    if let Some(mut l) = location {
        ensure!(!l.name.trim().is_empty() && l.name.len() <= 160, "Enter a name (up to 160 bytes)");
        ensure!(l.friend_ids.len() <= 500, "Too many friends");
        let canonical = fs::canonicalize(&l.path)?;
        validate_root(config, &canonical)?;
        ensure!(l.byte_cap > 0, "Byte cap must be positive");
        let root = if let Some(old) = list.iter_mut().find(|old| old.id == l.id && Path::new(&old.path) == canonical && old.marker.is_some()) {
            let root = Root::for_location(old)?;
            l.device = old.device; l.marker = old.marker.clone();
            root
        } else { l.device = None; l.marker = None; Root::open(&canonical)? };
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            l.device = Some(root.dir.metadata()?.dev());
            if l.marker.is_none() { l.marker = Some(root.create_marker()?); }
            l.safe_publish = Some(if root.native { "native" } else { "reservation" }.into());
        }
        l.path = root.path.to_string_lossy().into_owned();
        l.name = l.name.trim().into();
        let friends = crate::friends::load(config);
        ensure!(l.friend_ids.iter().all(|id| friends.iter().any(|f| &f.id == id)), "Unknown friend");
        if l.id.is_empty() { l.id = uuid::Uuid::new_v4().to_string(); }
        if let Some(old) = list.iter_mut().find(|old| old.id == l.id) { *old = l; }
        else { ensure!(list.len() < 100, "At most 100 locations"); list.push(l); }
    }
    let stopped = remove.and_then(|id| list.iter().find(|l| l.id == id).cloned());
    if let Some(id) = remove { list.retain(|l| l.id != id); }
    fs::create_dir_all(config)?;
    let tmp = config.join(format!(".locations-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(&list)?)?; f.sync_all()?;
        fs::rename(&tmp, config.join("locations.json"))?; Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result?;
    lock.revision.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // Stop sharing must still work when the mount is offline. If present, move
    // the verified marker to recoverable trash; never unlink it from the NAS.
    #[cfg(unix)] if let Some(l) = stopped {
        if let Err(e) = Root::retire_marker(&l) { log::warn!("Stopped location {} but could not retire mount marker: {e:#}", l.id); }
    }
    Ok(list)
}

// Paired folders can be temporarily absent; resolve their existing ancestor so
// aliases such as /var vs /private/var cannot bypass overlap protection.
fn canonical_missing(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = fs::canonicalize(path) { return Ok(path); }
    let parent = path.parent().context("Cannot resolve paired mirror folder")?;
    let name = path.file_name().context("Cannot resolve paired mirror folder")?;
    Ok(canonical_missing(parent)?.join(name))
}
fn validate_root(config: &Path, root: &Path) -> Result<()> {
    let canonical_root = fs::canonicalize(root)?;
    let root = canonical_root.as_path();
    let config = fs::canonicalize(config)?;
    let mut protected = vec![config.clone(), PathBuf::from("/")];
    if let Some(home) = std::env::var_os("HOME") {
        let home = fs::canonicalize(home)?;
        #[cfg(target_os = "macos")] {
            let library = canonical_missing(&home.join("Library"))?;
            ensure!(!root.starts_with(&library), LocationError::UnsafeRoot);
            protected.push(library);
        }
        protected.push(home);
    }
    ensure!(!protected.iter().any(|p| p.starts_with(root)) && !root.starts_with(&config), LocationError::UnsafeRoot);
    for pair in crate::pairing::load(&config) {
        // All paired folders are excluded, including ones whose mirror option may change.
        let folder = canonical_missing(Path::new(&pair.folder))?;
        ensure!(!root.starts_with(&folder) && !folder.starts_with(root), LocationError::MirrorOverlap);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum Access { Read, Upload, Manage }
pub fn authorize(config: &Path, endpoint: &str, id: &str, access: Access) -> Result<(Location, String)> {
    // The authenticated transport identity is the ONLY authority. Never accept a
    // friend id, fromName, or endpoint_id supplied in a request as authentication.
    let friend = crate::friends::load(config).into_iter()
        .find(|f| f.endpoint_id.as_deref() == Some(endpoint)).context("Location access denied")?;
    let location = load(config)?.into_iter().find(|l| l.id == id && l.friend_ids.contains(&friend.id))
        .context("Location access denied")?;
    ensure!(match access { Access::Read => true, Access::Upload => location.rights.upload, Access::Manage => location.rights.manage }, LocationError::Permission);
    Ok((location, friend.id))
}
pub fn shared(config: &Path, endpoint: &str) -> Result<Value> {
    let friend = crate::friends::load(config).into_iter()
        .find(|f| f.endpoint_id.as_deref() == Some(endpoint)).context("Location access denied")?;
    Ok(json!(load(config)?.iter().filter(|l| l.friend_ids.contains(&friend.id))
        .map(|l| json!({"id": l.id, "name": l.name, "rights": l.rights})).collect::<Vec<_>>()))
}

/// Validate before normalizing: Path::components would silently remove `.`.
/// Unix permits colon and backslash in filenames; Windows does not.
pub fn relative(raw: &str) -> Result<PathBuf> {
    ensure!(raw.len() <= 4096 && !raw.contains('\0'), "Invalid relative path");
    #[cfg(windows)] ensure!(!raw.contains(['\\', ':']), "Invalid relative path");
    ensure!(!raw.starts_with('/'), "Absolute paths are not allowed");
    let mut out = PathBuf::new();
    for part in raw.split('/') {
        ensure!(part != "..", "Parent traversal is not allowed");
        if part.is_empty() || part == "." { continue; }
        ensure!(!part.to_ascii_lowercase().starts_with(".dropbeam-"), "Reserved location path");
        ensure!(part.len() <= 255, "File name too long");
        out.push(part);
    }
    ensure!(out.components().all(|c| matches!(c, Component::Normal(_))), "Invalid relative path");
    Ok(out)
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> { v[key].as_str().context(format!("Missing {key}")) }

pub struct Root {
    path: PathBuf,
    #[cfg(unix)] dir: fs::File,
    #[cfg(unix)] native: bool,
}
impl Root {
    pub fn for_location(l: &mut Location) -> Result<Self> {
        // Verify the marker before probing, so a missing mount receives no writes.
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            let dir = Self::verified_dir(l)?;
            // st_dev changes on network remounts. The marker is the persisted
            // identity; device numbers pin only this open session.
            l.device = Some(dir.metadata()?.dev());
            let native = unix::cached_probe(&dir)?;
            return Ok(Self { path: fs::canonicalize(&l.path)?, dir, native });
        }
        #[cfg(not(unix))] Self::open(Path::new(&l.path))
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).context("Location unavailable. Check that the NAS is mounted")?;
        ensure!(path.is_dir(), "Location must be a folder");
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            let dir = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path)?;
            let native = unix::cached_probe(&dir)?;
            Ok(Self { path, dir, native })
        }
        #[cfg(not(unix))] { let _ = path; bail!("Hosting Locations requires Linux or macOS in Phase 1") }
    }
    #[cfg(unix)]
    fn verified_dir(l: &Location) -> Result<fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        let marker = l.marker.as_ref().context("Location mount marker missing; save it in Settings first")?;
        ensure!(marker.strip_prefix(".dropbeam-mount-").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()), "Invalid mount marker");
        let dir = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&l.path).context("Location mount unavailable")?;
        unix::verify_marker(&dir, marker)?;
        Ok(dir)
    }
    fn recheck(&self, l: &Location) -> Result<()> {
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            let current = Self::verified_dir(l)?.metadata()?;
            let pinned = self.dir.metadata()?;
            ensure!(current.dev() == pinned.dev() && current.ino() == pinned.ino(), LocationError::MountChanged);
        }
        ensure!(fs::canonicalize(&l.path)? == self.path, LocationError::MountChanged);
        Ok(())
    }
    #[cfg(unix)]
    fn retire_marker(l: &Location) -> Result<()> {
        let dir = Self::verified_dir(l)?;
        let root = Self { path: PathBuf::from(&l.path), native: unix::cached_probe(&dir)?, dir };
        root.trash_name(&root.dir, std::ffi::OsStr::new(l.marker.as_ref().unwrap()))?;
        Ok(())
    }
    pub fn resolve(&self, raw: &str) -> Result<PathBuf> {
        let rel = relative(raw)?;
        let resolved = fs::canonicalize(self.path.join(&rel))?;
        ensure!(resolved.starts_with(&self.path), "Path escapes the location");
        // Strict policy: refuse symlinks even when they currently resolve inside.
        // Descriptor walks below prevent a later swap from redirecting a syscall.
        #[cfg(unix)] { let _ = self.open_rel(&rel, false)?; }
        Ok(resolved)
    }
}

#[cfg(test)]
thread_local! { pub(crate) static FORCE_HARD_LINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// Publish a private staging file without replacing an occupied destination.
/// Native exclusive rename, then hard-link publication, then logged reservation fallback.
pub(crate) fn publish_noreplace(source: &Path, destination: &Path) -> Result<()> {
    publish_noreplace_owned(source, destination, None)
}
pub(crate) fn publish_noreplace_owned(source: &Path, destination: &Path, expected: Option<crate::iroh_net::receive_stage::Identity>) -> Result<()> {
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        let open = |p: &Path| fs::OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(p);
        let from = open(source.parent().context("staging parent missing")?)?;
        let to = open(destination.parent().context("destination parent missing")?)?;
        unix::publish(&from, source.file_name().context("staging name missing")?,
            &to, destination.file_name().context("destination name missing")?, expected)
    }
    #[cfg(not(unix))] {
        // A new hard link is atomic and never replaces an existing directory entry.
        let file = fs::File::open(source)?;
        let identity = crate::iroh_net::receive_stage::Identity::of(&file)?;
        ensure!(expected.is_none_or(|id| id == identity), "Receive stage identity changed before publication");
        fs::hard_link(source, destination)?;
        if let Err(e) = identity.remove(source) { log::warn!("Published hard link; stage cleanup deferred: {e:#}"); }
        Ok(())
    }
}

/// Only call for registered ordinary receive directories, never NAS roots.
pub(crate) fn gc_receive_probes(config: &Path, dir: &Path) {
    // A user may later share a previously registered Downloads directory.
    // Registration is never permission to sweep a Location or its descendants.
    if !receive_sweep_allowed(config, dir) { return; }
    #[cfg(unix)] unix::gc_probes(dir, std::time::SystemTime::now());
}
pub(crate) fn receive_sweep_allowed(config: &Path, dir: &Path) -> bool {
    let Ok(locations) = load(config) else { return false; };
    let Ok(path) = fs::canonicalize(dir) else { return false; };
    for l in locations {
        let Ok(root) = canonical_missing(Path::new(&l.path)) else { return false; };
        if path.starts_with(root) { return false; }
    }
    true
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{ffi::{CStr, CString}, os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::MetadataExt}}};
    fn c(s: &std::ffi::OsStr) -> Result<CString> { Ok(CString::new(s.as_bytes())?) }
    fn file(fd: i32) -> Result<fs::File> {
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
    fn io(rc: i32) -> Result<()> { if rc < 0 { Err(std::io::Error::last_os_error().into()) } else { Ok(()) } }
    // Pin cached descriptors to prevent inode reuse. A remount's new device key
    // gets a fresh probe. Roots retain their capability even after LRU eviction.
    pub(super) struct Capability { _dir: fs::File, native: bool, used: Instant }
    static PROBES: LazyLock<Mutex<HashMap<(u64, u64), Capability>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    #[cfg(test)] thread_local! {
        pub(super) static PROBE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        pub(super) static PROBE_ERRNO: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    }
    #[cfg(test)] pub(super) fn hold_probe_cache() -> std::sync::MutexGuard<'static, HashMap<(u64, u64), Capability>> {
        PROBES.lock().unwrap_or_else(|p| p.into_inner())
    }
    #[cfg(test)] pub(super) fn forget_probe(path: &Path) {
        let m = fs::metadata(path).unwrap();
        PROBES.lock().unwrap_or_else(|p| p.into_inner()).remove(&(m.dev(), m.ino()));
    }
    pub(super) fn cached_probe(dir: &fs::File) -> Result<bool> {
        let meta = dir.metadata()?;
        let key = (meta.dev(), meta.ino());
        if let Some(cap) = PROBES.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&key) { cap.used = Instant::now(); return Ok(cap.native); }
        // NEVER hold the cache lock across filesystem calls: a probe on a NAS
        // mount that is asleep, or an open() that macOS parks behind a pending
        // privacy prompt, would otherwise stall every receive in the process
        // (they all pass through here). Two racing probes of one directory are
        // harmless; the second result simply wins.
        let native = probe(dir)?;
        let mut cache = PROBES.lock().unwrap_or_else(|p| p.into_inner());
        if cache.len() >= 128 && !cache.contains_key(&key) {
            if let Some(key) = cache.iter().min_by_key(|(_, c)| c.used).map(|(k, _)| *k) { cache.remove(&key); }
        }
        cache.insert(key, Capability { _dir: dir.try_clone()?, native, used: Instant::now() });
        Ok(native)
    }
    fn native_rename(from: &fs::File, old: &std::ffi::OsStr, to: &fs::File, new: &std::ffi::OsStr) -> Result<()> {
        let old = c(old)?; let new = c(new)?;
        #[cfg(target_os = "linux")]
        let rc = unsafe { libc::syscall(libc::SYS_renameat2, from.as_raw_fd(), old.as_ptr(), to.as_raw_fd(), new.as_ptr(), libc::RENAME_NOREPLACE) as i32 };
        #[cfg(target_os = "macos")]
        let rc = unsafe { libc::renameatx_np(from.as_raw_fd(), old.as_ptr(), to.as_raw_fd(), new.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        { bail!("Atomic no-replace rename is unavailable on this platform"); }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        io(rc).context("Could not move item without replacing data (the filesystem must support atomic no-replace rename)")
    }
    pub(super) fn probe(dir: &fs::File) -> Result<bool> {
        #[cfg(test)] PROBE_CALLS.with(|n| n.set(n.get() + 1));
        let a = std::ffi::OsString::from(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4()));
        let b = std::ffi::OsString::from(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4()));
        let reservation = probe_reserve(dir, &a);
        // Read-only/permission-denied shares still support reads. Unknown
        // errnos also select the conservative reservation capability.
        let reservation = match reservation { Ok(f) => f, Err(e) => { log::debug!("No native publication probe: {e:#}"); return Ok(false); } };
        let result = native_rename(dir, &a, dir, &b);
        for name in [&a, &b] {
            if same_reservation(dir, name, &reservation, false).unwrap_or(false) { let _ = unlink(dir, name, false); }
        }
        Ok(result.is_ok())
    }
    fn probe_reserve(dir: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File> {
        #[cfg(test)] if let Some(errno) = PROBE_ERRNO.with(|e| e.get()) { return Err(std::io::Error::from_raw_os_error(errno).into()); }
        reserve(dir, name, false)
    }
    pub(super) fn gc_probes(dir: &Path, now: std::time::SystemTime) {
        // Only generated, empty regular files older than a day are probe litter.
        // The age check alone keeps an in-flight probe (seconds old) safe, so no
        // lock is taken here: this runs from startup sweeps and receive paths on
        // directories that may block (sleeping NAS mounts, macOS privacy prompts),
        // and a global lock held across those calls stalled every receive.
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(parent) = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(dir) else { return; };
        let _ = names(&parent, |name| {
            let valid = name.to_str().and_then(|n| n.strip_prefix(".dropbeam-probe-")).is_some_and(|id| uuid::Uuid::parse_str(id).is_ok());
            if !valid { return Ok(true); }
            if let Ok(f) = child(&parent, name, false) {
                if let Ok(m) = f.metadata() {
                    let old = m.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > Duration::from_secs(24 * 3600));
                    if m.is_file() && old && same_reservation(&parent, name, &f, false).unwrap_or(false) { let _ = unlink(&parent, name, false); }
                }
            }
            Ok(true)
        });
    }
    fn unlink(parent: &fs::File, name: &std::ffi::OsStr, directory: bool) -> Result<()> {
        io(unsafe { libc::unlinkat(parent.as_raw_fd(), c(name)?.as_ptr(), if directory { libc::AT_REMOVEDIR } else { 0 }) })
    }
    fn unlink_source(parent: &fs::File, name: &std::ffi::OsStr, source: &fs::File) -> Result<()> {
        let current = child(parent, name, false)?;
        let a = current.metadata()?; let b = source.metadata()?;
        ensure!(a.dev() == b.dev() && a.ino() == b.ino(), "Stage name changed after hard-link publication");
        unlink(parent, name, false)
    }
    #[cfg(test)]
    mod stage_cleanup_tests {
        use super::*;
        #[test]
        fn hard_link_cleanup_leaves_a_replaced_stage_name_untouched() {
            let dir = std::env::temp_dir().join(format!("dropbeam-link-cleanup-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("stage"); fs::write(&path, b"body").unwrap();
            let parent = fs::File::open(&dir).unwrap();
            let name = std::ffi::OsStr::new("stage");
            let source = child(&parent, name, false).unwrap();
            fs::hard_link(&path, dir.join("landed")).unwrap();
            fs::remove_file(&path).unwrap(); fs::write(&path, b"replacement").unwrap();
            assert!(unlink_source(&parent, name, &source).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"replacement");
            assert_eq!(fs::read(dir.join("landed")).unwrap(), b"body");
            fs::remove_dir_all(dir).unwrap();
        }
    }
    fn mode(parent: &fs::File, directory: bool) -> Result<libc::mode_t> {
        Ok((parent.metadata()?.mode() & if directory { 0o777 } else { 0o666 }) as libc::mode_t)
    }
    fn reserve(parent: &fs::File, name: &std::ffi::OsStr, directory: bool) -> Result<fs::File> {
        if directory { mkdir(parent, name)?; return child(parent, name, true); }
        file(unsafe { libc::openat(parent.as_raw_fd(), c(name)?.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode(parent, false)? as libc::c_uint) })
    }
    fn same_reservation(parent: &fs::File, name: &std::ffi::OsStr, reservation: &fs::File, directory: bool) -> Result<bool> {
        let current = child(parent, name, directory)?;
        let a = current.metadata()?; let b = reservation.metadata()?;
        let mut empty = a.len() == 0;
        if directory { empty = true; names(&current, |_| { empty = false; Ok(false) })?; }
        Ok(a.dev() == b.dev() && a.ino() == b.ino() && empty)
    }
    fn unsupported(e: &anyhow::Error) -> bool {
        e.downcast_ref::<std::io::Error>().and_then(|e| e.raw_os_error())
            .is_some_and(|n| [libc::ENOSYS, libc::ENOTSUP, libc::EOPNOTSUPP, libc::EINVAL].contains(&n))
    }
    // Ordinary receives always attempt the real operation, regardless of a
    // cached probe result (permissions during a probe can be transient).
    pub(super) fn publish(from: &fs::File, old: &std::ffi::OsStr, to: &fs::File, new: &std::ffi::OsStr, expected: Option<crate::iroh_net::receive_stage::Identity>) -> Result<()> {
        if let Some(expected) = expected {
            ensure!(crate::iroh_net::receive_stage::Identity::of(&child(from, old, false)?)? == expected,
                "Receive stage identity changed before publication");
        }
        #[cfg(test)] if FORCE_HARD_LINK.with(|flag| flag.get()) { return rename_owned(false, from, old, to, new, expected); }
        match native_rename(from, old, to, new) {
            Ok(()) => Ok(()),
            Err(e) if unsupported(&e) => rename_owned(false, from, old, to, new, expected),
            Err(e) => Err(e),
        }
    }
    pub(super) fn rename(native: bool, from: &fs::File, old: &std::ffi::OsStr, to: &fs::File, new: &std::ffi::OsStr) -> Result<()> {
        rename_owned(native, from, old, to, new, None)
    }
    fn rename_owned(native: bool, from: &fs::File, old: &std::ffi::OsStr, to: &fs::File, new: &std::ffi::OsStr, expected: Option<crate::iroh_net::receive_stage::Identity>) -> Result<()> {
        if native { return native_rename(from, old, to, new); }
        let source = child(from, old, false)?;
        if let Some(expected) = expected {
            ensure!(crate::iroh_net::receive_stage::Identity::of(&source)? == expected, "Receive stage identity changed before publication");
        }
        let directory = source.metadata()?.is_dir();
        if !directory {
            match io(unsafe { libc::linkat(from.as_raw_fd(), c(old)?.as_ptr(), to.as_raw_fd(), c(new)?.as_ptr(), 0) }) {
                Ok(()) => {
                    // Publication has succeeded. Cleanup failure must not cause
                    // the caller to publish another copy on retry.
                    if let Err(e) = unlink_source(from, old, &source) { log::warn!("Published hard link; stage cleanup deferred: {e:#}"); }
                    return Ok(());
                }
                Err(e) if unsupported(&e) => {},
                Err(e) => return Err(e),
            }
        }
        log::warn!("Publication using reservation fallback: atomic exclusive rename and hard-link publication unavailable; concurrent external writers are not protected");
        let reservation = reserve(to, new, directory).context("Destination already exists or cannot be reserved; nothing was replaced")?;
        let result = (|| {
            ensure!(same_reservation(to, new, &reservation, directory)?, "Destination reservation changed; publication refused");
            io(unsafe { libc::renameat(from.as_raw_fd(), c(old)?.as_ptr(), to.as_raw_fd(), c(new)?.as_ptr()) })
                .context("Reservation publish failed; filesystem may not allow renaming a directory onto an empty reserved directory")
        })();
        if result.is_err() && same_reservation(to, new, &reservation, directory).unwrap_or(false) { let _ = unlink(to, new, directory); }
        result
    }
    pub(super) fn verify_marker(dir: &fs::File, marker: &str) -> Result<()> {
        use std::io::Read;
        let mut f = child(dir, std::ffi::OsStr::new(marker), false).context(LocationError::MountChanged)?;
        let mut value = String::new(); (&mut f).take(256).read_to_string(&mut value)?;
        ensure!(value == marker, LocationError::MountChanged); Ok(())
    }
    pub(super) fn mkdir(parent: &fs::File, name: &std::ffi::OsStr) -> Result<()> {
        let name = c(name)?;
        io(unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), mode(parent, true)?) })
    }
    fn child(parent: &fs::File, name: &std::ffi::OsStr, directory: bool) -> Result<fs::File> {
        let name = c(name)?;
        // O_NONBLOCK prevents FIFO/device names from hanging an endpoint worker.
        let f = file(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | if directory { libc::O_DIRECTORY } else { 0 }) })?;
        let m = f.metadata()?;
        ensure!(m.dev() == parent.metadata()?.dev(), "Crossing into another mount is not allowed");
        ensure!(m.is_dir() || m.is_file(), "Only regular files and folders are supported");
        Ok(f)
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory { fn drop(&mut self) { unsafe { libc::closedir(self.0); } } }
    pub(super) fn names(dir: &fs::File, mut visit: impl FnMut(&std::ffi::OsStr) -> Result<bool>) -> Result<()> {
        // Open a fresh description so listing never shares the root fd's offset.
        let fd = unsafe { libc::openat(dir.as_raw_fd(), c(std::ffi::OsStr::new("."))?.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        ensure!(fd >= 0, "Cannot open directory");
        let ptr = unsafe { libc::fdopendir(fd) };
        if ptr.is_null() { unsafe { libc::close(fd); } bail!("Cannot list directory"); }
        let d = Directory(ptr);
        loop {
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            unsafe { *errno = 0; }
            let ent = unsafe { libc::readdir(d.0) };
            if ent.is_null() {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                if unsafe { *errno } != 0 { return Err(std::io::Error::last_os_error().into()); }
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." { continue; }
            if !visit(std::ffi::OsStr::from_bytes(bytes))? { break; }
        }
        Ok(())
    }
    impl Root {
        pub(super) fn create_marker(&self) -> Result<String> {
            use std::io::Write;
            let marker = format!(".dropbeam-mount-{}", uuid::Uuid::new_v4());
            let mut f = reserve(&self.dir, std::ffi::OsStr::new(&marker), false)?;
            f.write_all(marker.as_bytes())?; f.sync_all()?; Ok(marker)
        }
        pub(super) fn open_rel(&self, rel: &Path, directory: bool) -> Result<fs::File> {
            let mut dir = self.dir.try_clone()?;
            let parts: Vec<_> = rel.components().collect();
            for (i, part) in parts.iter().enumerate() {
                ensure!(matches!(part, Component::Normal(_)), "Invalid path component");
                dir = child(&dir, part.as_os_str(), directory || i + 1 < parts.len())?;
            }
            Ok(dir)
        }
        pub(super) fn parent(&self, raw: &str) -> Result<(fs::File, std::ffi::OsString)> {
            let rel = relative(raw)?;
            let name = rel.file_name().context("The location root cannot be modified")?.to_owned();
            let parent = rel.parent().unwrap_or(Path::new(""));
            self.resolve(&parent.to_string_lossy())?;
            Ok((self.open_rel(parent, true)?, name))
        }
        pub fn listing(&self, raw: &str) -> Result<Vec<Entry>> {
            self.resolve(raw)?;
            let dir = self.open_rel(&relative(raw)?, true)?;
            let mut entries = Vec::new();
            names(&dir, |name| {
                let Some(name_str) = name.to_str() else { return Ok(true); };
                if relative(name_str).is_err() { return Ok(true); }
                let Ok(f) = child(&dir, name, false) else { return Ok(true); };
                let m = f.metadata()?;
                ensure!(entries.len() < 100_000, "Folder listing exceeds 100000 entries");
                entries.push(Entry { name: name_str.into(), is_dir: m.is_dir(), size: if m.is_file() { m.len() } else { 0 }, modified: m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0) });
                Ok(true)
            })?;
            Ok(entries)
        }
        pub fn new_folder(&self, raw: &str) -> Result<()> {
            let (parent, name) = self.parent(raw)?;
            mkdir(&parent, &name)
        }

        pub fn rename_item(&self, raw: &str, to: &str) -> Result<()> {
            self.resolve(raw)?;
            let (parent, name) = self.parent(raw)?;
            let (new_parent, new_name) = self.parent(to)?;
            rename(self.native, &parent, &name, &new_parent, &new_name)
        }
        pub fn trash(&self, raw: &str) -> Result<String> {
            self.resolve(raw)?;
            let (parent, name) = self.parent(raw)?;
            self.trash_name(&parent, &name)
        }
        pub(super) fn trash_name(&self, parent: &fs::File, name: &std::ffi::OsStr) -> Result<String> {
            let trash = std::ffi::OsStr::new(".dropbeam-trash");
            if let Err(e) = mkdir(&self.dir, trash) {
                if child(&self.dir, trash, true).is_err() { return Err(e); }
            }
            let dir = child(&self.dir, trash, true)?;
            let timestamp = format!("{}-{}", crate::chat::now_ms(), uuid::Uuid::new_v4());
            mkdir(&dir, std::ffi::OsStr::new(&timestamp))?;
            let bucket = child(&dir, std::ffi::OsStr::new(&timestamp), true)?;
            if let Err(e) = rename(self.native, parent, name, &bucket, name) {
                let _ = unlink(&dir, std::ffi::OsStr::new(&timestamp), true); return Err(e);
            }
            Ok(format!(".dropbeam-trash/{timestamp}/{}", name.to_string_lossy()))
        }
        pub fn ensure_dirs(&self, raw: &str) -> Result<()> {
            let mut dir = self.dir.try_clone()?;
            for part in relative(raw)?.components() {
                let name = part.as_os_str();
                match child(&dir, name, true) {
                    Ok(next) => dir = next,
                    Err(_) => { mkdir(&dir, name)?; dir = child(&dir, name, true)?; }
                }
            }
            Ok(())
        }
        /// Copy from private transfer staging, hash-check, fsync, then publish
        /// using the probed native/reservation capability. Failed copies never publish.
        pub fn land(&self, raw: &str, staged: &Path, digest: &str, cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(u64), publish: impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>) -> Result<PathBuf> {
            let expected = fs::metadata(staged)?.len();
            let rel = relative(raw)?;
            publish(&mut || self.ensure_dirs(&rel.parent().unwrap_or(Path::new("")).to_string_lossy()))?;
            let (parent, name) = self.parent(raw)?;
            if let Ok(mut existing) = child(&parent, &name, false) {
                ensure!(existing.metadata()?.is_file(), "An item with this name already exists");
                if existing.metadata()?.len() == 0 && expected > 0 {
                    let m = existing.metadata()?;
                    log::warn!("Location upload blocked by empty destination {raw:?} dev={} ino={}; possible leftover reservation, ownership cannot be proven; preserved for owner review", m.dev(), m.ino());
                    bail!("An empty destination already exists (possibly a leftover reservation); ownership cannot be proven. Ask the owner to move it to trash before retrying");
                }
                use sha2::{Digest, Sha256}; use std::io::Read;
                let mut source = fs::File::open(staged)?;
                ensure!(source.metadata()?.len() == existing.metadata()?.len(), "A different file with this name already exists; rename your upload first");
                let mut hash = Sha256::new(); let mut b = vec![0; 1 << 20]; let mut other = vec![0; 1 << 20]; let mut done = 0;
                loop {
                    ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
                    let n = source.read(&mut b)?; if n == 0 { break; }
                    existing.read_exact(&mut other[..n])?;
                    ensure!(b[..n] == other[..n], "A different file with this name already exists; rename your upload first");
                    hash.update(&b[..n]); done += n as u64; progress(done);
                }
                ensure!(hex::encode(hash.finalize()) == digest, "SHA-256 mismatch");
                publish(&mut || Ok(()))?;
                return Ok(self.path.join(rel));
            }
            let tmp = std::ffi::OsString::from(format!(".dropbeam-upload-{}.part", uuid::Uuid::new_v4()));
            let mut f = file(unsafe { libc::openat(parent.as_raw_fd(), c(&tmp)?.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode(&parent, false)? as libc::c_uint) })?;
            let result = (|| -> Result<()> {
                use sha2::{Digest, Sha256}; use std::io::{Read, Write};
                let mut src = fs::File::open(staged)?;
                let mut hash = Sha256::new(); let mut buf = vec![0; 1 << 20];
                let mut done = 0;
                loop {
                    ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
                    let n = src.read(&mut buf)?; if n == 0 { break; }
                    ensure!(done + n as u64 <= expected, LocationError::Quota);
                    hash.update(&buf[..n]); f.write_all(&buf[..n])?; done += n as u64; progress(done);
                }
                ensure!(hex::encode(hash.finalize()) == digest, "SHA-256 mismatch; upload was not published");
                f.sync_all()?;
                ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
                publish(&mut || rename(self.native, &parent, &tmp, &parent, &name))
            })();
            if result.is_err() { let _ = unsafe { libc::unlinkat(parent.as_raw_fd(), c(&tmp)?.as_ptr(), 0) }; }
            result?; Ok(self.path.join(rel))
        }
        pub(super) fn selection_bytes(&self, raw: &str, budget: &mut Budget, depth: usize) -> Result<()> {
            ensure!(depth < 64 && budget.entries > 0, "Selection too large or deeply nested"); budget.entries -= 1;
            let Ok(source) = self.open_rel(&relative(raw)?, false) else { return Ok(()); };
            if source.metadata()?.is_file() { budget.consume(source.metadata()?.len())?; }
            else { names(&source, |name| {
                let Some(name) = name.to_str() else { return Ok(true); };
                if name.to_ascii_lowercase().starts_with(".dropbeam-") { return Ok(true); }
                self.selection_bytes(&format!("{raw}/{name}"), budget, depth + 1)?; Ok(true)
            })?; }
            Ok(())
        }
        pub fn snapshot(&self, raw: &str, dest: &Path, budget: &mut Budget, depth: usize, skipped: &mut Vec<String>) -> Result<bool> {
            ensure!(depth < 64 && budget.entries > 0, "Selection too large or deeply nested"); budget.entries -= 1;
            let rel = relative(raw)?;
            let mut source = match self.open_rel(&rel, false) {
                Ok(f) => f,
                Err(e) => { skipped.push(format!("{raw}: {}", crate::telemetry::redact_paths_only(&format!("{e:#}")))); return Ok(false); }
            };
            if source.metadata()?.is_file() {
                use std::io::{Read, Write};
                use std::os::unix::fs::OpenOptionsExt;
                check_space(dest.parent().context("Missing snapshot parent")?, source.metadata()?.len())?;
                budget.check(source.metadata()?.len())?;
                let mut out = match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(dest) {
                    Ok(f) => f,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => { skipped.push(format!("{raw}: staged name collides with another selected item")); return Ok(false); }
                    Err(e) => return Err(e.into()),
                };
                let mut buf = vec![0; 1 << 20];
                loop { let n = source.read(&mut buf)?; if n == 0 { break; } budget.consume(n as u64)?; out.write_all(&buf[..n])?; }
                out.sync_all()?;
            } else {
                // Exclusive mkdir prevents case/normalization twins from being
                // silently merged in the staging filesystem.
                use std::os::unix::fs::DirBuilderExt;
                match fs::DirBuilder::new().mode(0o700).create(dest) {
                    Ok(()) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => { skipped.push(format!("{raw}: staged name collides with another selected item")); return Ok(false); }
                    Err(e) => return Err(e.into()),
                }
                names(&source, |name| {
                    let Some(name) = name.to_str() else { skipped.push(format!("{raw}: non-Unicode filename")); return Ok(true); };
                    if name.to_ascii_lowercase().starts_with(".dropbeam-") { return Ok(true); }
                    let next = if raw.is_empty() { name.to_string() } else { format!("{raw}/{name}") };
                    self.snapshot(&next, &dest.join(name), budget, depth + 1, skipped)?;
                    Ok(true)
                })?;
            }
            Ok(true)
        }

    }
}

// Fail closed on platforms without descriptor-relative filesystem protection.
#[cfg(not(unix))]
impl Root {
    pub fn listing(&self, _: &str) -> Result<Vec<Entry>> { bail!("Hosting unavailable") }
    pub fn new_folder(&self, _: &str) -> Result<()> { bail!("Hosting unavailable") }
    pub fn rename_item(&self, _: &str, _: &str) -> Result<()> { bail!("Hosting unavailable") }
    pub fn trash(&self, _: &str) -> Result<String> { bail!("Hosting unavailable") }
    pub fn ensure_dirs(&self, _: &str) -> Result<()> { bail!("Hosting unavailable") }
    pub fn land(&self, _: &str, _: &Path, _: &str, _: &std::sync::atomic::AtomicBool, _: impl Fn(u64), _: impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>) -> Result<PathBuf> { bail!("Hosting unavailable") }
    fn selection_bytes(&self, _: &str, _: &mut Budget, _: usize) -> Result<()> { bail!("Hosting unavailable") }
    pub fn snapshot(&self, _: &str, _: &Path, _: &mut Budget, _: usize, _: &mut Vec<String>) -> Result<bool> { bail!("Hosting unavailable") }
}

pub fn dispatch(config: &Path, endpoint: &str, request: &Value) -> Result<Value> {
    dispatch_at(config, endpoint, request, Instant::now())
}
fn dispatch_at(config: &Path, endpoint: &str, request: &Value, now: Instant) -> Result<Value> {
    ensure!(request["locations_v"].as_u64() == Some(VERSION), "Unsupported Locations capability");
    let kind = text(request, "kind")?;
    if kind == "locations.list" { return shared(config, endpoint); }
    let access = if kind == "locations.ls" { Access::Read } else { Access::Manage };
    let (mut l, friend) = authorize(config, endpoint, text(request, "id")?, access)?;
    limit_request(config, &friend, if matches!(access, Access::Read) { "ls" } else { "manage" }, now)?;
    let lock = operation(config, text(request, "id")?);
    let _op = if matches!(access, Access::Manage) { Some(lock.mutex.lock().unwrap_or_else(|p| p.into_inner())) } else { None };
    // A settings save may have won the lock after the admission check.
    if matches!(access, Access::Manage) { l = authorize(config, endpoint, &l.id, access)?.0; }
    let raw = text(request, "rel_path")?;
    if matches!(access, Access::Manage) { log::info!("locations manage attempt friend={friend} location={} operation={kind} path={raw:?}", l.id); }
    validate_root(config, Path::new(&l.path))?;
    let root = Root::for_location(&mut l)?;
    let result = match kind {
        "locations.ls" => cached_listing(config, &friend, &l.id, &root, raw, request),
        "locations.mkdir" => root.new_folder(raw).map(|_| json!({})),
        "locations.rename" => root.rename_item(raw, text(request, "to")?).map(|_| json!({})),
        "locations.trash" => root.trash(raw).map(|p| json!({"trashPath": p})),
        _ => bail!("Unknown location operation"),
    };
    if matches!(access, Access::Manage) {
        log::info!("locations manage result friend={friend} location={} operation={kind} success={} destination={:?}", l.id, result.is_ok(), request.get("to"));
        if result.is_ok() {
            let mut activity = ACTIVITY.lock().unwrap_or_else(|p| p.into_inner());
            let rows = activity.entry(config.into()).or_default();
            rows.push_back(json!({"friendId": friend, "locationId": l.id, "operation": kind, "item": raw, "to": request["to"], "at": crate::chat::now_ms()}));
            while rows.len() > 50 { rows.pop_front(); }
        }
    }
    result
}

static ACTIVITY: LazyLock<Mutex<HashMap<PathBuf, std::collections::VecDeque<Value>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
pub fn activity(config: &Path) -> Vec<Value> {
    ACTIVITY.lock().unwrap_or_else(|p| p.into_inner()).get(config).map(|v| v.iter().rev().cloned().collect()).unwrap_or_default()
}

pub struct Budget { remaining: u64, entries: usize }
impl Budget {
    fn check(&self, bytes: u64) -> Result<()> { ensure!(bytes <= self.remaining, LocationError::Quota); Ok(()) }
    fn consume(&mut self, bytes: u64) -> Result<()> { self.check(bytes)?; self.remaining -= bytes; Ok(()) }
}
// Include retained retry bytes when admitting a stage. The legacy receiver can
// briefly hold an old same-size file plus the new copy for deduplication.
fn stage_bytes(path: &Path, entries: &mut usize) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(path)? {
        ensure!(*entries > 0, "Private stage has too many entries"); *entries -= 1;
        let entry = entry?; let meta = fs::symlink_metadata(entry.path())?;
        ensure!(!meta.file_type().is_symlink(), "Private stage contains a symlink");
        let n = if meta.is_dir() { stage_bytes(&entry.path(), entries)? } else { meta.len() };
        bytes = bytes.checked_add(n).context("Stage byte count overflow")?;
    }
    Ok(bytes)
}
fn check_space(path: &Path, bytes: u64) -> Result<()> {
    #[cfg(unix)] {
        use std::{os::unix::ffi::OsStrExt, ffi::CString};
        let path = CString::new(path.as_os_str().as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        ensure!(unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0, "Cannot determine free space for location transfer");
        let stat = unsafe { stat.assume_init() };
        let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
        ensure!(free >= bytes.saturating_add(64 * 1024 * 1024), "Insufficient free space for location transfer (64 MiB headroom required)");
    }
    Ok(())
}
static FRIEND_TRANSFERS: LazyLock<Mutex<HashMap<(PathBuf, String), usize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
struct FriendLease((PathBuf, String));
impl FriendLease {
    fn acquire(config: &Path, friend: &str) -> Result<Self> {
        let key = (config.into(), friend.into());
        let mut active = FRIEND_TRANSFERS.lock().unwrap_or_else(|p| p.into_inner());
        let n = active.entry(key.clone()).or_default();
        ensure!(*n < 2, LocationError::Busy); *n += 1; Ok(Self(key))
    }
}
impl Drop for FriendLease { fn drop(&mut self) {
    let mut active = FRIEND_TRANSFERS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(n) = active.get_mut(&self.0) { *n -= 1; if *n == 0 { active.remove(&self.0); } }
} }
struct Listing { config: PathBuf, friend: String, location: String, path: String, created: Instant, entries: Arc<Vec<Entry>>, offset: usize }
static LISTINGS: LazyLock<Mutex<HashMap<String, Listing>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
struct TokenBucket { updated: Instant, tokens: f64 }
impl TokenBucket {
    fn take(&mut self, now: Instant) -> Result<()> {
        self.tokens = (self.tokens + now.saturating_duration_since(self.updated).as_secs_f64() * 10.0).min(10.0);
        // Concurrent requests can acquire the mutex out of timestamp order.
        self.updated = self.updated.max(now);
        ensure!(self.tokens >= 1.0, LocationError::RateLimit);
        self.tokens -= 1.0; Ok(())
    }
}
static REQUEST_RATE: LazyLock<Mutex<HashMap<(PathBuf, String, &'static str), TokenBucket>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
fn limit_request(config: &Path, friend: &str, kind: &'static str, now: Instant) -> Result<()> {
    let mut rate = REQUEST_RATE.lock().unwrap_or_else(|p| p.into_inner());
    rate.retain(|_, b| now.saturating_duration_since(b.updated) < Duration::from_secs(120));
    rate.entry((config.into(), friend.into(), kind)).or_insert(TokenBucket { updated: now, tokens: 10.0 }).take(now)
}
const MAX_CACHED_ENTRIES: usize = 200_000;
fn cached_entries(cache: &HashMap<String, Listing>) -> usize {
    let mut snapshots = std::collections::HashSet::new();
    cache.values().filter(|l| snapshots.insert(Arc::as_ptr(&l.entries))).map(|l| l.entries.len()).sum()
}
fn make_listing_room(cache: &mut HashMap<String, Listing>, entries: &Arc<Vec<Entry>>) {
    loop {
        let extra = if cache.values().any(|l| Arc::ptr_eq(&l.entries, entries)) { 0 } else { entries.len() };
        if cache.len() < 126 && cached_entries(cache) + extra <= MAX_CACHED_ENTRIES { break; }
        if let Some(key) = cache.iter().min_by_key(|(_, v)| v.created).map(|(k, _)| k.clone()) { cache.remove(&key); }
        else { break; }
    }
}
fn cached_listing(config: &Path, friend: &str, location: &str, root: &Root, raw: &str, request: &Value) -> Result<Value> {
    let size = request["page_size"].as_u64().unwrap_or(PAGE_SIZE as u64).clamp(1, PAGE_SIZE as u64) as usize;
    let mut cache = LISTINGS.lock().unwrap_or_else(|p| p.into_inner());
    cache.retain(|_, v| v.created.elapsed() < Duration::from_secs(120));
    let (entries, offset, created) = if let Some(cursor) = request["cursor"].as_str() {
        let l = cache.get(cursor).context("Listing cursor expired; refresh the folder")?;
        ensure!(l.config == config && l.friend == friend && l.location == location && l.path == raw, "Listing cursor access denied");
        (l.entries.clone(), l.offset, l.created)
    } else {
        drop(cache);
        let mut entries = root.listing(raw)?;
        let sort = request["sort"].as_str().unwrap_or("name");
        let query = request["query"].as_str().unwrap_or("").to_lowercase();
        entries.retain(|e| e.name.to_lowercase().contains(&query));
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| match sort { "size" => b.size.cmp(&a.size), "modified" => b.modified.cmp(&a.modified), _ => std::cmp::Ordering::Equal }).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
        cache = LISTINGS.lock().unwrap_or_else(|p| p.into_inner());
        (Arc::new(entries), 0, Instant::now())
    };
    let end = (offset + size).min(entries.len());
    // Cache the first page as well, so Previous returns the same snapshot.
    make_listing_room(&mut cache, &entries);
    let cursor = uuid::Uuid::new_v4().to_string();
    cache.insert(cursor.clone(), Listing { config: config.into(), friend: friend.into(), location: location.into(), path: raw.into(), created, entries: entries.clone(), offset });
    let next = if end < entries.len() {
        let key = uuid::Uuid::new_v4().to_string();
        cache.insert(key.clone(), Listing { config: config.into(), friend: friend.into(), location: location.into(), path: raw.into(), created, entries: entries.clone(), offset: end }); Some(key)
    } else { None };
    Ok(json!({"entries": &entries[offset..end], "hasMore": next.is_some(), "cursor": cursor, "nextCursor": next, "total": entries.len()}))
}

/// Private staging avoids ever giving the legacy receive engine a NAS path.
/// Stable across retries, including manual retries, and scoped by authenticated
/// endpoint + location + destination + content manifest. A per-target lease prevents concurrent use.
pub struct Upload {
    config: PathBuf, endpoint: String, target: Target, pub staging: PathBuf,
    pub destination: PathBuf, _lease: UploadLease, _friend: FriendLease, byte_cap: u64,
    root: Root, location: Location,
}
static UPLOADS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
struct UploadLease(PathBuf);
impl Drop for UploadLease { fn drop(&mut self) { UPLOADS.lock().unwrap_or_else(|p| p.into_inner()).retain(|p| p != &self.0); } }
impl Upload {
    pub fn prepare(config: &Path, endpoint: &str, header: &Value) -> Result<Self> {
        ensure!(header["locations_v"].as_u64() == Some(VERSION), "Unsupported Locations capability");
        let target: Target = serde_json::from_value(header["location"].clone())?;
        let (mut location, friend) = authorize(config, endpoint, &target.location_id, Access::Upload)?;
        let friend_lease = FriendLease::acquire(config, &friend)?;
        validate_root(config, Path::new(&location.path))?;
        let root = Root::for_location(&mut location)?;
        let destination = root.resolve(&target.rel_path)?;
        ensure!(destination.is_dir(), "Upload destination must be a folder");
        let items = header["items"].as_array().context("Missing file manifest")?;
        ensure!(items.len() <= 100_000, "Too many files");
        let dirs = header["dirs"].as_array().context("Missing directory manifest")?;
        ensure!(dirs.len() <= 100_000, "Too many directories");
        let manifest_total = items.iter().try_fold(0u64, |total, item| -> Result<u64> {
            total.checked_add(item["size"].as_u64().context("Invalid file size")?).context("Manifest size overflow")
        })?;
        ensure!(header["total"].as_u64() == Some(manifest_total), "Manifest byte total mismatch");
        Budget { remaining: location.byte_cap, entries: 0 }.check(manifest_total)?;
        check_space(config, manifest_total)?;
        check_space(&root.path, manifest_total)?;
        let mut seen = std::collections::HashSet::new();
        for item in items {
            let name = text(item, "name")?;
            ensure!(seen.insert(relative(name)?), "Duplicate file path");
            ensure!(!relative(name)?.as_os_str().is_empty(), "Empty file name");
            let digest = text(item, "sha256")?;
            ensure!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()), "Missing SHA-256");
        }
        for d in dirs { relative(d.as_str().context("Invalid directory")?)?; }
        use sha2::{Digest, Sha256};
        let id = text(header, "location_transfer")?;
        ensure!(!id.is_empty() && id.len() <= 100, "Invalid transfer id");
        let key = hex::encode(Sha256::digest(serde_json::to_vec(&(endpoint, &target, items, dirs))?));
        let staging = config.join("location-transfers").join(key);
        let mut leases = UPLOADS.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(!leases.contains(&staging), "This location upload is already in progress; retry shortly");
        leases.push(staging.clone());
        drop(leases);
        let lease = UploadLease(staging.clone());
        private_directory(&staging)?;
        let retained = stage_bytes(&staging, &mut 200_000)?;
        Budget { remaining: location.byte_cap, entries: 0 }.check(retained.checked_add(manifest_total).context("Stage byte count overflow")?)?;
        Ok(Self { config: config.into(), endpoint: endpoint.into(), target, destination, staging, _lease: lease, _friend: friend_lease, byte_cap: location.byte_cap, root, location })
    }
    pub fn finish(&self, header: &Value, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
        self.finish_progress(header, paths, &std::sync::atomic::AtomicBool::new(false), |_| {})
    }
    pub fn finish_progress(&self, header: &Value, paths: Vec<PathBuf>, cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(u64)) -> Result<Vec<PathBuf>> {
        let root = &self.root;
        let l = &self.location;
        let friend = &self._friend.0.1;
        let items = header["items"].as_array().context("Missing manifest")?;
        ensure!(items.len() == paths.len(), "Incomplete upload");
        let checked = std::cell::Cell::new(None::<(Instant, u64)>);
        let lock = operation(&self.config, &self.target.location_id);
        let publish = |action: &mut dyn FnMut() -> Result<()>| -> Result<()> {
            loop {
                let revision = lock.revision.load(std::sync::atomic::Ordering::SeqCst);
                if checked.get().is_none_or(|(at, rev)| at.elapsed() >= Duration::from_secs(1) || rev != revision) {
                    // Authenticate before waiting for the publication lock. No
                    // filesystem capability probes occur after prepare().
                    let (current, _) = authorize(&self.config, &self.endpoint, &self.target.location_id, Access::Upload)?;
                    ensure!(current.path == l.path && current.marker == l.marker, "Location changed during upload");
                    validate_root(&self.config, Path::new(&current.path))?;
                    root.recheck(&current)?;
                    ensure!(root.resolve(&self.target.rel_path)? == self.destination, "Location changed during upload");
                    Budget { remaining: current.byte_cap, entries: 0 }.check(header["total"].as_u64().context("Missing total")?)?;
                    checked.set(Some((Instant::now(), revision)));
                }
                let _guard = lock.mutex.lock().unwrap_or_else(|p| p.into_inner());
                if lock.revision.load(std::sync::atomic::Ordering::SeqCst) != revision { continue; }
                if checked.get().is_some_and(|(at, _)| at.elapsed() >= Duration::from_secs(1)) { continue; }
                return action();
            }
        };
        publish(&mut || Ok(()))?;
        let actual = paths.iter().try_fold(0u64, |n, p| -> Result<u64> { n.checked_add(fs::metadata(p)?.len()).context("Stage byte count overflow") })?;
        Budget { remaining: self.byte_cap, entries: 0 }.check(actual)?;
        check_space(&root.path, actual)?;
        let mut out = Vec::new();
        let mut base = 0;
        for (item, staged) in items.iter().zip(paths) {
            let raw = format!("{}/{}", self.target.rel_path, text(item, "name")?).trim_start_matches('/').to_string();
            ensure!(fs::metadata(&staged)?.len() == item["size"].as_u64().context("Invalid file size")?, "Staged file size differs from manifest");
            let landed = root.land(&raw, &staged, text(item, "sha256")?, cancel, |n| progress(base + n), &publish)?;
            log::info!("locations upload friend={friend} location={} path={raw:?}", l.id);
            out.push(landed);
            base += item["size"].as_u64().context("Invalid file size")?;
        }
        ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
        for dir in header["dirs"].as_array().into_iter().flatten() {
            let raw = format!("{}/{}", self.target.rel_path, dir.as_str().context("Invalid directory")?).trim_start_matches('/').to_string();
            publish(&mut || root.ensure_dirs(&raw))?;
        }
        let _ = fs::remove_dir_all(&self.staging);
        Ok(out)
    }
}

/// Only our private cache directories are eligible. Never traverse or GC NAS trash.
pub fn gc(config: &Path) -> Result<()> {
    let active = UPLOADS.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for category in ["location-snapshots", "location-transfers"] {
        let parent = config.join(category);
        if !parent.exists() { continue; }
        if !fs::symlink_metadata(&parent).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) { continue; }
        let entries = match fs::read_dir(&parent) { Ok(entries) => entries, Err(e) => { log::warn!("Location cache GC {parent:?}: {e}"); continue; } };
        for entry in entries {
            let entry = match entry { Ok(entry) => entry, Err(e) => { log::warn!("Location cache GC entry: {e}"); continue; } };
            let path = entry.path();
            if active.contains(&path) { continue; }
            let name = entry.file_name().to_string_lossy().into_owned();
            let generated = if category == "location-snapshots" { uuid::Uuid::parse_str(&name).is_ok() } else { name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) };
            if !generated { continue; }
            // Claim the orphan briefly: a retry may have acquired it since the
            // snapshot. Hold a lease, never UPLOADS, across filesystem deletion.
            let _lease = {
                let mut leases = UPLOADS.lock().unwrap_or_else(|p| p.into_inner());
                if leases.contains(&path) { continue; }
                leases.push(path.clone()); UploadLease(path.clone())
            };
            let result = (|| -> Result<()> {
                let kind = entry.file_type()?;
                if kind.is_dir() { fs::remove_dir_all(&path)?; }
                else if kind.is_symlink() { fs::remove_file(&path)?; }
                Ok(())
            })();
            if let Err(e) = result { log::warn!("Location cache GC {path:?}: {e:#}"); }
        }
    }
    Ok(())
}
pub fn spawn_gc(config: PathBuf) {
    tauri::async_runtime::spawn(async move {
        loop {
            let cfg = config.clone();
            let _ = tokio::task::spawn_blocking(move || { if let Err(e) = gc(&cfg) { log::warn!("Location cache GC: {e:#}"); } }).await;
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    });
}

#[cfg(test)]
pub fn sha256(path: &Path) -> Result<String> {
    sha256_progress(path, &std::sync::atomic::AtomicBool::new(false), |_| {})
}
pub fn sha256_progress(path: &Path, cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(u64)) -> Result<String> {
    use sha2::{Digest, Sha256}; use std::io::Read;
    let mut hash = Sha256::new(); let mut f = fs::File::open(path)?; let mut b = vec![0; 1 << 20]; let mut done = 0;
    loop {
        ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
        let n = f.read(&mut b)?; if n == 0 { break; }
        hash.update(&b[..n]); done += n as u64; progress(done);
    }
    Ok(hex::encode(hash.finalize()))
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new(); builder.recursive(true);
    #[cfg(unix)] {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?; Ok(())
}

/// Keep snapshots alive throughout normal friend-send retries, then remove only
/// this private generated directory. Never delete anything from a location.
pub struct Snapshot { pub directory: PathBuf, pub paths: Vec<String>, pub skipped: Vec<String>, _friend: FriendLease, _lease: UploadLease, config: PathBuf, endpoint: String, location_id: String, root_path: String }
impl Snapshot {
    pub fn check_access(&self, endpoint: &str) -> Result<()> {
        ensure!(endpoint == self.endpoint, "Download recipient changed");
        let (mut l, _) = authorize(&self.config, endpoint, &self.location_id, Access::Read)?;
        ensure!(l.path == self.root_path, "Location changed during download");
        validate_root(&self.config, Path::new(&l.path))?;
        let _ = Root::for_location(&mut l)?;
        Ok(())
    }
}
fn remove_private_async(path: PathBuf) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() { runtime.spawn_blocking(move || { let _ = fs::remove_dir_all(path); }); }
    else { std::thread::spawn(move || { let _ = fs::remove_dir_all(path); }); }
}
impl Drop for Snapshot { fn drop(&mut self) { remove_private_async(self.directory.clone()); } }
pub fn download_snapshot(config: &Path, endpoint: &str, request: &Value) -> Result<Snapshot> {
    download_snapshot_at(config, endpoint, request, Instant::now())
}
fn download_snapshot_at(config: &Path, endpoint: &str, request: &Value, now: Instant) -> Result<Snapshot> {
    ensure!(request["locations_v"].as_u64() == Some(VERSION), "Unsupported Locations capability");
    let (mut l, friend) = authorize(config, endpoint, text(request, "id")?, Access::Read)?;
    limit_request(config, &friend, "download", now)?;
    let friend_lease = FriendLease::acquire(config, &friend)?;
    validate_root(config, Path::new(&l.path))?;
    let root = Root::for_location(&mut l)?;
    let paths = request["paths"].as_array().context("Missing selection")?;
    ensure!(!paths.is_empty() && paths.len() <= 500, "Select 1–500 items");
    let mut preflight = Budget { remaining: l.byte_cap, entries: 100_000 };
    for raw in paths { root.selection_bytes(raw.as_str().context("Invalid path")?, &mut preflight, 0)?; }
    check_space(config, l.byte_cap - preflight.remaining)?;
    let directory = config.join("location-snapshots").join(uuid::Uuid::new_v4().to_string());
    let lease = {
        let mut active = UPLOADS.lock().unwrap_or_else(|p| p.into_inner());
        active.push(directory.clone()); UploadLease(directory.clone())
    };
    private_directory(&directory)?;
    let mut snapshot = Snapshot { directory, paths: vec![], skipped: vec![], _friend: friend_lease, _lease: lease, config: config.into(), endpoint: endpoint.into(), location_id: l.id.clone(), root_path: l.path.clone() };
    let mut budget = Budget { remaining: l.byte_cap, entries: 100_000 };
    for raw in paths {
        let raw = raw.as_str().context("Invalid path")?;
        let rel = relative(raw)?;
        let leaf = rel.file_name().context("Select items inside the location")?;
        let dest = snapshot.directory.join(leaf);
        if root.snapshot(raw, &dest, &mut budget, 0, &mut snapshot.skipped)? { snapshot.paths.push(dest.to_string_lossy().into_owned()); }
    }
    // Recheck after a potentially long snapshot, before releasing any bytes.
    let (current, _) = authorize(config, endpoint, &l.id, Access::Read)?;
    ensure!(current.path == l.path, "Location changed during download preparation");
    ensure!(current.marker == l.marker, LocationError::MountChanged);
    root.recheck(&current)?;
    Ok(snapshot)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    /// A sleeping NAS mount or a macOS privacy prompt can park a probe or a
    /// sweep inside a filesystem call for minutes. Neither may hold the shared
    /// probe cache while it waits, or every receive in the process stalls.
    #[test]
    fn probe_sweep_and_probe_never_wait_on_the_probe_cache() {
        let dir = std::env::temp_dir().join(format!("dropbeam-probe-lock-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4()));
        fs::write(&stale, b"").unwrap();
        let held = unix::hold_probe_cache();
        let sweep_dir = dir.clone();
        let sweep = std::thread::spawn(move || unix::gc_probes(&sweep_dir, std::time::SystemTime::now() + Duration::from_secs(48 * 3600)));
        let probe_dir = dir.clone();
        let probe = std::thread::spawn(move || {
            use std::os::unix::fs::OpenOptionsExt;
            let f = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY).open(&probe_dir).unwrap();
            unix::probe(&f).is_ok()
        });
        let started = Instant::now();
        while !(sweep.is_finished() && probe.is_finished()) {
            assert!(started.elapsed() < Duration::from_secs(10), "probe work waited on the probe cache lock");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(held);
        assert!(!stale.exists(), "stale probe litter must still be collected without the lock");
        assert!(probe.join().unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
    struct Fixture { dir: PathBuf, config: PathBuf, root: PathBuf, friend: String, location: Location }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("dropbeam-location-test-{}", uuid::Uuid::new_v4()));
            let root = dir.join("NAS"); let config = dir.join("config");
            fs::create_dir_all(&root).unwrap(); fs::create_dir_all(&config).unwrap();
            let friend = crate::friends::upsert_by_endpoint(&config, "owner-device", "Friend").id;
            let location = Location { id: "nas".into(), name: "Family NAS".into(), path: root.to_string_lossy().into_owned(), friend_ids: vec![friend.clone()], rights: Rights { upload: true, manage: true }, byte_cap: default_byte_cap(), device: None, marker: None, safe_publish: None };
            save(&config, Some(location.clone()), None).unwrap();
            Self { dir, config, root, friend, location }
        }
        fn req(&self, kind: &str, rel: &str) -> Value { json!({"kind": format!("locations.{kind}"), "locations_v": VERSION, "id": "nas", "rel_path": rel, "page": 0}) }
        fn upload(&self, name: &str, bytes: &[u8]) -> (Upload, Value, PathBuf) {
            let src = self.dir.join(format!("source-{}", uuid::Uuid::new_v4())); fs::write(&src, bytes).unwrap();
            let header = json!({"kind":"files", "locations_v": VERSION, "location": {"location_id":"nas", "rel_path":""}, "location_transfer":uuid::Uuid::new_v4().to_string(), "items":[{"name":name,"size":bytes.len(),"sha256":sha256(&src).unwrap()}], "dirs":[], "total":bytes.len()});
            let upload = Upload::prepare(&self.config, "owner-device", &header).unwrap();
            (upload, header, src)
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.dir); } }
    #[test]
    fn root_binding_rejects_parent_absolute_reserved_and_symlink_escapes() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new(); let root = Root::open(&f.root).unwrap();
        fs::write(f.root.join("旅行 🏡.txt"), b"unicode").unwrap();
        assert_eq!(root.resolve("./旅行 🏡.txt").unwrap(), fs::canonicalize(f.root.join("旅行 🏡.txt")).unwrap());
        for raw in ["../secret", "a/../b", "/tmp/secret", "//server/share", ".dropbeam-trash/x", "nested/.DROPBEAM-upload-x", "x\0y"] {
            let expected = if raw.contains("..") { "Parent traversal" } else if raw.starts_with('/') { "Absolute paths" } else if raw.contains('\0') { "Invalid relative path" } else { "Reserved location path" };
            error_message(relative(raw), expected);
        }
        let outside = f.dir.join("outside"); fs::create_dir(&outside).unwrap(); fs::write(outside.join("secret"), b"keep").unwrap();
        symlink(&outside, f.root.join("escape")).unwrap();
        error_message(root.resolve("escape/secret"), "Path escapes the location");
        error_message(root.new_folder("escape/new"), "Path escapes the location");
        error_message(root.rename_item("旅行 🏡.txt", "escape/new"), "Path escapes the location");
        error_message(root.trash("escape"), "Path escapes the location");
        symlink(&outside, f.root.join(".dropbeam-trash")).unwrap();
        io_kind(root.trash("旅行 🏡.txt"), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(outside.join("secret")).unwrap(), b"keep");
        assert!(f.root.join("旅行 🏡.txt").exists());
    }
    #[test]
    fn trash_moves_files_and_folders_without_deleting_or_overwriting() {
        let f = Fixture::new(); let root = Root::open(&f.root).unwrap();
        root.new_folder("work").unwrap(); fs::write(f.root.join("work/important"), b"family data").unwrap();
        let trash = root.trash("work").unwrap();
        assert!(!f.root.join("work").exists());
        assert_eq!(fs::read(f.root.join(&trash).join("important")).unwrap(), b"family data");
        assert!(trash.starts_with(".dropbeam-trash/"));
        error_message(root.trash(""), "The location root cannot be modified");
        error_message(root.resolve(&trash), "Reserved location path");
        assert!(root.listing("").unwrap().is_empty());
        root.new_folder("a").unwrap(); root.new_folder("b").unwrap();
        fs::write(f.root.join("b/keep"), b"keep").unwrap();
        io_kind(root.rename_item("a", "b"), std::io::ErrorKind::AlreadyExists);
        assert!(f.root.join("a").is_dir()); assert_eq!(fs::read(f.root.join("b/keep")).unwrap(), b"keep");
        root.rename_item("a", "renamed").unwrap(); assert!(f.root.join("renamed").is_dir());
    }
    #[test]
    fn every_operation_uses_authenticated_friend_and_current_rights() {
        let mut f = Fixture::new();
        error_message(authorize(&f.config, "stranger", "nas", Access::Read), "Location access denied");
        assert_eq!(authorize(&f.config, "owner-device", "nas", Access::Manage).unwrap().1, f.friend);
        f.location.rights = Rights { upload: false, manage: false };
        save(&f.config, Some(f.location.clone()), None).unwrap();
        assert!(authorize(&f.config, "owner-device", "nas", Access::Read).is_ok());
        for access in [Access::Upload, Access::Manage] { error_kind(authorize(&f.config, "owner-device", "nas", access), LocationError::Permission); }
        for kind in ["mkdir", "rename", "trash"] { error_kind(dispatch(&f.config, "owner-device", &f.req(kind,"x")), LocationError::Permission); }
        for kind in ["ls", "mkdir", "rename", "trash"] {
            let mut req = f.req(kind,"x"); req["friend_id"] = json!(f.friend); req["endpoint_id"] = json!("owner-device");
            error_message(dispatch(&f.config, "spoof", &req), "Location access denied");
        }
        f.location.friend_ids.clear(); save(&f.config, Some(f.location.clone()), None).unwrap();
        assert!(shared(&f.config, "owner-device").unwrap().as_array().unwrap().is_empty());
        error_message(authorize(&f.config, "owner-device", "nas", Access::Read), "Location access denied");
        let defaults: Location = serde_json::from_value(json!({"id":"x", "name":"x", "path":"/x"})).unwrap();
        assert!(defaults.friend_ids.is_empty()); assert!(defaults.rights.upload); assert!(!defaults.rights.manage);
    }
    #[test]
    fn upload_verifies_hash_revocation_and_never_clobbers_existing_data() {
        let mut f = Fixture::new();
        fs::write(f.root.join("existing"), b"precious").unwrap();
        let (u, h, src) = f.upload("existing", b"replacement");
        error_message(u.finish(&h, vec![src]), "A different file with this name already exists");
        assert_eq!(fs::read(f.root.join("existing")).unwrap(), b"precious"); drop(u);
        let (u, h, src) = f.upload("nested/文件.txt", b"hello NAS");
        assert!(!f.root.join("nested/文件.txt").exists());
        u.finish(&h, vec![src]).unwrap(); assert_eq!(fs::read(f.root.join("nested/文件.txt")).unwrap(), b"hello NAS"); drop(u);
        let (u, h, src) = f.upload("bad-hash", b"good"); fs::write(&src, b"evil").unwrap();
        error_message(u.finish(&h, vec![src]), "SHA-256 mismatch"); assert!(!f.root.join("bad-hash").exists()); drop(u);
        let (u, h, src) = f.upload("revoked", b"hello");
        f.location.rights.upload = false; save(&f.config, Some(f.location.clone()), None).unwrap();
        error_kind(u.finish(&h, vec![src]), LocationError::Permission); assert!(!f.root.join("revoked").exists());
    }
    #[test]
    fn listings_are_paginated_and_snapshots_refuse_symlinks() {
        let f = Fixture::new();
        for i in 0..1001 { fs::write(f.root.join(format!("item-{i}")), b"x").unwrap(); }
        let mut names = std::collections::HashSet::new();
        let mut request = f.req("ls", "");
        for page in 0..3 {
            let value = dispatch(&f.config, "owner-device", &request).unwrap();
            request["cursor"] = value["nextCursor"].clone();
            let entries = value["entries"].as_array().unwrap(); assert!(entries.len() <= PAGE_SIZE);
            assert_eq!(value["hasMore"], page < 2);
            for e in entries { assert!(names.insert(e["name"].as_str().unwrap().to_owned())); }
        }
        assert_eq!(names.len(), 1001);
        let req = json!({"locations_v":VERSION,"id":"nas","paths":["item-1"]});
        let snapshot = download_snapshot(&f.config, "owner-device", &req).unwrap();
        assert_eq!(fs::read(&snapshot.paths[0]).unwrap(), b"x");
        let dir = snapshot.directory.clone(); drop(snapshot);
        for _ in 0..100 { if !dir.exists() { break; } std::thread::sleep(Duration::from_millis(10)); }
        assert!(!dir.exists());
        std::os::unix::fs::symlink(f.dir.join("config"), f.root.join("link")).unwrap();
        let skipped = download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":["link"]})).unwrap();
        assert!(skipped.paths.is_empty()); assert!(skipped.skipped[0].starts_with("link:"));
    }
    #[test]
    fn canceled_publication_leaves_no_visible_file_and_manual_retry_reuses_staging() {
        let f = Fixture::new();
        let (upload, header, source) = f.upload("canceled.bin", &vec![42; 2 << 20]);
        let staging = upload.staging.clone();
        error_message(Upload::prepare(&f.config, "owner-device", &header), "already in progress");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        error_message(upload.finish_progress(&header, vec![source.clone()], &cancel, |_| { cancel.store(true, std::sync::atomic::Ordering::SeqCst); }), "canceled");
        assert!(!f.root.join("canceled.bin").exists());
        drop(upload);
        let mut retry = header.clone(); retry["location_transfer"] = json!(uuid::Uuid::new_v4().to_string());
        let upload = Upload::prepare(&f.config, "owner-device", &retry).unwrap();
        assert_eq!(upload.staging, staging);
        upload.finish(&retry, vec![source]).unwrap();
        assert_eq!(fs::metadata(f.root.join("canceled.bin")).unwrap().len(), 2 << 20);
    }
    #[test]
    fn corrupt_configuration_is_never_replaced_by_empty_defaults() {
        let f = Fixture::new(); fs::write(f.config.join("locations.json"), b"{broken").unwrap();
        error_message(save(&f.config, Some(f.location.clone()), None), "Cannot read locations.json");
        assert_eq!(fs::read(f.config.join("locations.json")).unwrap(), b"{broken");
    }    fn error_kind<T>(result: Result<T>, expected: LocationError) {
        let error = match result { Ok(_) => panic!("expected {expected:?}"), Err(e) => e };
        assert_eq!(error.downcast_ref::<LocationError>(), Some(&expected), "{error:#}");
    }
    fn error_message<T>(result: Result<T>, expected: &str) {
        let error = match result { Ok(_) => panic!("expected {expected}"), Err(e) => e };
        assert!(format!("{error:#}").contains(expected), "expected {expected:?}, got {error:#}");
    }
    fn io_kind<T>(result: Result<T>, expected: std::io::ErrorKind) {
        let error = match result { Ok(_) => panic!("expected {expected:?}"), Err(e) => e };
        assert_eq!(error.downcast_ref::<std::io::Error>().map(|e| e.kind()), Some(expected), "{error:#}");
    }
    #[test]
    fn reservation_publish_preserves_existing_files_directories_and_symlinks() {
        let f = Fixture::new(); let mut root = Root::open(&f.root).unwrap(); root.native = false;
        fs::write(f.root.join("source"), b"source").unwrap();
        fs::write(f.root.join("occupied"), b"precious").unwrap();
        io_kind(root.rename_item("source", "occupied"), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(f.root.join("occupied")).unwrap(), b"precious");
        std::os::unix::fs::symlink(f.root.join("occupied"), f.root.join("link")).unwrap();
        io_kind(root.rename_item("source", "link"), std::io::ErrorKind::AlreadyExists);
        let staged_link = f.dir.join("link-staged"); fs::write(&staged_link, b"replacement").unwrap();
        assert!(root.land("link", &staged_link, &sha256(&staged_link).unwrap(), &std::sync::atomic::AtomicBool::new(false), |_| {}, |action| action()).is_err());
        assert_eq!(fs::read(f.root.join("occupied")).unwrap(), b"precious");
        assert!(fs::symlink_metadata(f.root.join("link")).unwrap().is_symlink());
        root.rename_item("source", "renamed").unwrap();
        root.new_folder("folder").unwrap(); root.new_folder("taken").unwrap();
        io_kind(root.rename_item("folder", "taken"), std::io::ErrorKind::AlreadyExists);
        root.rename_item("folder", "moved").unwrap();
        let trash = root.trash("renamed").unwrap();
        assert_eq!(fs::read(f.root.join(trash)).unwrap(), b"source");
        let staged = f.dir.join("staged"); fs::write(&staged, b"bytes").unwrap();
        root.land("uploaded", &staged, &sha256(&staged).unwrap(), &std::sync::atomic::AtomicBool::new(false), |_| {}, |action| action()).unwrap();
        assert_eq!(fs::read(f.root.join("uploaded")).unwrap(), b"bytes");
    }
    #[test]
    fn byte_caps_refuse_uploads_snapshots_and_growing_sources() {
        let mut f = Fixture::new(); f.location.byte_cap = 3;
        save(&f.config, Some(f.location.clone()), None).unwrap();
        fs::write(f.root.join("four"), b"1234").unwrap();
        error_kind(download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":["four"]})), LocationError::Quota);
        let header = json!({"locations_v":VERSION,"location":{"location_id":"nas","rel_path":""},"items":[{"name":"four","size":4}],"dirs":[],"total":4});
        error_kind(Upload::prepare(&f.config, "owner-device", &header), LocationError::Quota);
        let mut budget = Budget { remaining: 3, entries: 1 };
        budget.consume(2).unwrap(); error_kind(budget.consume(2), LocationError::Quota);
        assert!(!f.config.join("location-snapshots").exists());
        error_message(check_space(&f.config, u64::MAX), "Insufficient free space");
    }
    #[test]
    fn blocklisted_roots_and_mirror_overlaps_are_refused() {
        let f = Fixture::new();
        for path in [PathBuf::from("/"), f.dir.clone(), f.config.clone(), fs::canonicalize(std::env::var_os("HOME").unwrap()).unwrap()] {
            error_kind(validate_root(&f.config, &path), LocationError::UnsafeRoot);
        }
        let pair = crate::pairing::create(&f.config, f.root.to_string_lossy().into_owned(), "host".into(), true, String::new(), true, Some("host".into())).unwrap();
        assert!(!pair.0.id.is_empty());
        fs::create_dir(f.root.join("child")).unwrap();
        for path in [&f.root, &f.root.join("child")] { error_kind(validate_root(&f.config, path), LocationError::MirrorOverlap); }
        // Ancestor case without intersecting app config.
        let outer = f.dir.join("outer"); fs::create_dir_all(outer.join("mirror")).unwrap();
        crate::pairing::create(&f.config, outer.join("mirror").to_string_lossy().into_owned(), "host".into(), true, String::new(), true, Some("host".into())).unwrap();
        error_kind(validate_root(&f.config, &outer), LocationError::MirrorOverlap);
        error_kind(dispatch(&f.config, "owner-device", &f.req("ls", "")), LocationError::MirrorOverlap);
    }
    #[test]
    fn mount_marker_accepts_and_restamps_remount_but_rejects_missing_or_replaced_marker() {
        let f = Fixture::new(); let mut l = load(&f.config).unwrap().remove(0);
        let device = l.device.unwrap();
        l.device = Some(device.wrapping_add(1));
        // Simulate a saved st_dev from a previous network mount session.
        fs::write(f.config.join("locations.json"), serde_json::to_vec(&vec![l.clone()]).unwrap()).unwrap();
        Root::for_location(&mut l).unwrap();
        assert_eq!(l.device, Some(device));
        let saved = save(&f.config, Some(f.location.clone()), None).unwrap();
        assert_eq!(saved[0].device, Some(device));
        assert_eq!(saved[0].marker, l.marker);
        l = load(&f.config).unwrap().remove(0);
        let marker = f.root.join(l.marker.as_ref().unwrap());
        fs::write(&marker, b"different").unwrap();
        error_kind(Root::for_location(&mut l), LocationError::MountChanged);
        fs::remove_file(marker).unwrap();
        for kind in ["ls", "mkdir", "rename", "trash"] { error_kind(dispatch(&f.config, "owner-device", &f.req(kind, "")), LocationError::MountChanged); }
        error_kind(save(&f.config, Some(f.location.clone()), None), LocationError::MountChanged);
    }
    #[test]
    fn created_share_files_and_directories_inherit_usable_permissions() {
        use std::os::unix::fs::{PermissionsExt, MetadataExt};
        let f = Fixture::new(); fs::set_permissions(&f.root, fs::Permissions::from_mode(0o755)).unwrap();
        let (upload, header, source) = f.upload("folder/file", b"contents");
        upload.finish(&header, vec![source]).unwrap();
        assert_eq!(fs::metadata(f.root.join("folder")).unwrap().mode() & 0o777, 0o755);
        assert_eq!(fs::metadata(f.root.join("folder/file")).unwrap().mode() & 0o777, 0o644);
        let (upload, _, _) = f.upload("another", b"private");
        assert_eq!(fs::metadata(&upload.staging).unwrap().mode() & 0o777, 0o700);
    }
    #[test]
    fn copying_does_not_block_browsing_or_settings_and_revocation_stops_publish() {
        let mut f = Fixture::new();
        let (upload, header, source) = f.upload("large", &vec![1; 2 << 20]);
        let (started, copying) = std::sync::mpsc::channel();
        let (resume, resumed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let once = std::sync::atomic::AtomicBool::new(false);
            upload.finish_progress(&header, vec![source], &std::sync::atomic::AtomicBool::new(false), |_| {
                if !once.swap(true, std::sync::atomic::Ordering::SeqCst) { started.send(()).unwrap(); resumed.recv_timeout(Duration::from_secs(5)).unwrap(); }
            })
        });
        copying.recv_timeout(Duration::from_secs(5)).unwrap();
        let before = Instant::now();
        dispatch(&f.config, "owner-device", &f.req("ls", "")).unwrap();
        f.location.rights.upload = false;
        save(&f.config, Some(f.location.clone()), None).unwrap();
        assert!(before.elapsed() < Duration::from_secs(3));
        resume.send(()).unwrap(); error_kind(worker.join().unwrap(), LocationError::Permission);
        assert!(!f.root.join("large").exists());
        let a = operation(&f.config, "a"); let b = operation(&f.config, "b");
        let _held = a.mutex.lock().unwrap(); assert!(b.mutex.try_lock().is_ok());
        // Even a held publish lock must not block ls on the same location.
        let lock = operation(&f.config, "nas"); let _held = lock.mutex.lock().unwrap();
        dispatch(&f.config, "owner-device", &f.req("ls", "")).unwrap();
    }
    #[test]
    fn cached_listing_is_sorted_stable_scoped_and_rate_limited() {
        let f = Fixture::new(); let now = Instant::now();
        fs::write(f.root.join("z"), b"123").unwrap(); fs::write(f.root.join("a"), b"1").unwrap();
        let mut request = f.req("ls", ""); request["page_size"] = json!(1); request["sort"] = json!("size");
        let first = dispatch_at(&f.config, "owner-device", &request, now).unwrap();
        assert_eq!(first["entries"][0]["name"], "z");
        fs::write(f.root.join("new"), b"12345").unwrap();
        request["cursor"] = first["nextCursor"].clone();
        assert_eq!(dispatch_at(&f.config, "owner-device", &request, now).unwrap()["entries"][0]["name"], "a");
        let mut wrong = request.clone(); wrong["rel_path"] = json!("other");
        error_message(dispatch_at(&f.config, "owner-device", &wrong, now), "Listing cursor access denied");
        request["cursor"] = json!("expired");
        error_message(dispatch_at(&f.config, "owner-device", &request, now), "Listing cursor expired");
        for _ in 0..6 { dispatch_at(&f.config, "owner-device", &f.req("ls", ""), now).unwrap(); }
        error_kind(dispatch_at(&f.config, "owner-device", &f.req("ls", ""), now), LocationError::RateLimit);
    }
    #[test]
    fn transfer_concurrency_and_gc_protect_active_stages_and_nas() {
        let f = Fixture::new();
        let (a, _, _) = f.upload("a", b"a"); let (b, _, _) = f.upload("b", b"b");
        error_kind(FriendLease::acquire(&f.config, &f.friend), LocationError::Busy);
        gc(&f.config).unwrap(); assert!(a.staging.exists()); assert!(b.staging.exists());
        let orphan = a.staging.clone(); drop(a); gc(&f.config).unwrap();
        assert!(!orphan.exists()); assert!(b.staging.exists());
        let root = Root::open(&f.root).unwrap(); root.new_folder("keep").unwrap();
        let trash = root.trash("keep").unwrap(); gc(&f.config).unwrap(); assert!(f.root.join(trash).is_dir());
    }
    #[test]
    fn unix_colons_backslashes_and_supported_siblings_survive_snapshot() {
        let f = Fixture::new(); fs::create_dir(f.root.join("folder")).unwrap();
        for name in ["a:b", "a\\b"] { relative(name).unwrap(); fs::write(f.root.join("folder").join(name), b"data").unwrap(); }
        std::os::unix::fs::symlink(&f.config, f.root.join("folder/link")).unwrap();
        let root = Root::open(&f.root).unwrap();
        let listing = root.listing("folder").unwrap();
        assert!(listing.iter().any(|e| e.name == "a:b")); assert_eq!(listing.len(), 2);
        let snapshot = download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":["folder"]})).unwrap();
        assert_eq!(snapshot.skipped.len(), 1);
        for name in ["a:b", "a\\b"] { assert_eq!(fs::read(Path::new(&snapshot.paths[0]).join(name)).unwrap(), b"data"); }
    }

    #[test]
    fn retry_admission_counts_retained_stage_bytes_and_activity_is_bounded() {
        let mut f = Fixture::new();
        let (upload, header, _) = f.upload("file", b"1234");
        let staging = upload.staging.clone(); drop(upload);
        fs::write(staging.join("retained"), b"1234").unwrap();
        f.location.byte_cap = 7; save(&f.config, Some(f.location.clone()), None).unwrap();
        error_kind(Upload::prepare(&f.config, "owner-device", &header), LocationError::Quota);
        let now = Instant::now();
        for i in 0..55 { dispatch_at(&f.config, "owner-device", &f.req("mkdir", &format!("folder-{i}")), now + Duration::from_secs(i)).unwrap(); }
        let rows = activity(&f.config); assert_eq!(rows.len(), 50);
        assert_eq!(rows[0]["friendId"], f.friend); assert_eq!(rows[0]["item"], "folder-54");
    }

    #[test]
    fn upload_session_never_reprobes_for_files_or_directories() {
        let f = Fixture::new();
        let src = f.dir.join("source"); fs::write(&src, b"data").unwrap();
        let items: Vec<_> = (0..32).map(|i| json!({"name":format!("folder-{i}/file"),"size":4,"sha256":sha256(&src).unwrap()})).collect();
        let dirs: Vec<_> = (0..32).map(|i| format!("empty-{i}/nested")).collect();
        let header = json!({"locations_v":VERSION,"location":{"location_id":"nas","rel_path":""},"location_transfer":"session","items":items,"dirs":dirs,"total":128});
        unix::forget_probe(&f.root);
        let before = unix::PROBE_CALLS.with(|n| n.get());
        let upload = Upload::prepare(&f.config, "owner-device", &header).unwrap();
        assert_eq!(unix::PROBE_CALLS.with(|n| n.get()) - before, 1);
        // Eviction must not matter to a verified upload's retained Root.
        unix::forget_probe(&f.root);
        let before = unix::PROBE_CALLS.with(|n| n.get());
        let paths = upload.finish(&header, vec![src; 32]).unwrap();
        assert_eq!(unix::PROBE_CALLS.with(|n| n.get()), before);
        assert_eq!(paths.len(), 32);
        assert!(paths.iter().all(|p| p.starts_with(fs::canonicalize(&f.root).unwrap())));
        assert!(f.root.join("empty-31/nested").is_dir());
    }

    #[test]
    fn read_only_and_unknown_probe_errors_allow_browse_download_and_resave() {
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { unix::PROBE_ERRNO.with(|e| e.set(None)); } }
        let _reset = Reset;
        for errno in [libc::EROFS, libc::EACCES, libc::EPERM, libc::EIO] {
            unix::PROBE_ERRNO.with(|e| e.set(None));
            let f = Fixture::new(); fs::write(f.root.join("readable"), b"data").unwrap();
            unix::forget_probe(&f.root);
            unix::PROBE_ERRNO.with(|e| e.set(Some(errno)));
            let mut l = load(&f.config).unwrap().remove(0);
            let root = Root::for_location(&mut l).unwrap(); assert!(!root.native);
            assert_eq!(dispatch(&f.config, "owner-device", &f.req("ls", "")).unwrap()["entries"][0]["name"], "readable");
            let snapshot = download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":["readable"]})).unwrap();
            assert_eq!(fs::read(&snapshot.paths[0]).unwrap(), b"data");
            assert_eq!(save(&f.config, Some(l), None).unwrap()[0].safe_publish.as_deref(), Some("reservation"));
        }
    }

    #[test]
    fn download_staging_collisions_skip_files_and_directories_without_aborting() {
        let f = Fixture::new();
        for parent in ["left", "right"] {
            fs::create_dir_all(f.root.join(parent).join("folder")).unwrap();
            fs::write(f.root.join(parent).join("same"), parent).unwrap();
        }
        fs::write(f.root.join("good"), b"keep").unwrap();
        let snapshot = download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":["left/same","right/same","left/folder","right/folder","good"]})).unwrap();
        assert_eq!(snapshot.paths.len(), 3); assert_eq!(snapshot.skipped.len(), 2);
        assert_eq!(fs::read(snapshot.directory.join("same")).unwrap(), b"left");
        assert_eq!(fs::read(snapshot.directory.join("good")).unwrap(), b"keep");
        assert!(snapshot.skipped.iter().all(|s| s.contains("staged name collides")));
    }

    #[test]
    fn download_case_and_unicode_twins_follow_staging_filesystem_semantics() {
        let f = Fixture::new();
        fs::create_dir(f.root.join("left")).unwrap(); fs::create_dir(f.root.join("right")).unwrap();
        for (a, b) in [("Case", "case"), ("é", "e\u{301}")] {
            fs::write(f.root.join("left").join(a), b"left").unwrap();
            fs::write(f.root.join("right").join(b), b"right").unwrap();
            let snapshot = download_snapshot(&f.config, "owner-device", &json!({"locations_v":VERSION,"id":"nas","paths":[format!("left/{a}"),format!("right/{b}")]})).unwrap();
            assert_eq!(snapshot.paths.len() + snapshot.skipped.len(), 2);
            assert_eq!(fs::read(snapshot.directory.join(a)).unwrap(), b"left");
            #[cfg(target_os = "macos")]
            if snapshot.directory.join(a) == snapshot.directory.join(b) || fs::canonicalize(snapshot.directory.join(a)).unwrap() == fs::canonicalize(snapshot.directory.join(b)).unwrap() {
                assert_eq!(snapshot.skipped.len(), 1);
            }
        }
    }

    #[test]
    fn ordinary_publish_uses_native_operation_without_probes_and_sweeps_only_old_probe_files() {
        let f = Fixture::new();
        let receive = f.dir.join("Downloads"); fs::create_dir(&receive).unwrap();
        let before = unix::PROBE_CALLS.with(|n| n.get());
        for i in 0..5 {
            let src = f.dir.join(format!("source-{i}")); fs::write(&src, b"payload").unwrap();
            publish_noreplace(&src, &receive.join(format!("file-{i}"))).unwrap();
        }
        assert_eq!(unix::PROBE_CALLS.with(|n| n.get()) - before, 0);
        let second = f.dir.join("Downloads2"); fs::create_dir(&second).unwrap();
        let src = f.dir.join("last"); fs::write(&src, b"payload").unwrap();
        publish_noreplace(&src, &second.join("file")).unwrap();
        assert_eq!(unix::PROBE_CALLS.with(|n| n.get()) - before, 0);
        let stray = receive.join(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4())); fs::write(&stray, b"").unwrap();
        let data = receive.join(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4())); fs::write(&data, b"keep").unwrap();
        let link = receive.join(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4())); std::os::unix::fs::symlink(&stray, &link).unwrap();
        let invalid = receive.join(".dropbeam-probe-owner-file"); fs::write(&invalid, b"").unwrap();
        gc_receive_probes(&f.config, &receive); assert!(stray.exists());
        unix::gc_probes(&receive, std::time::SystemTime::now() + Duration::from_secs(25 * 3600));
        assert!(!stray.exists()); assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(fs::read(data).unwrap(), b"keep"); assert!(invalid.exists());
        let nested = f.root.join("nested"); fs::create_dir(&nested).unwrap();
        for dir in [&f.root, &nested] {
            let protected = dir.join(format!(".dropbeam-probe-{}", uuid::Uuid::new_v4()));
            let file = fs::File::create(&protected).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(25 * 3600))).unwrap();
            gc_receive_probes(&f.config, dir); assert!(protected.exists());
        }
    }

    #[test]
    fn token_bucket_refills_continuously_with_injected_clock() {
        let now = Instant::now(); let mut bucket = TokenBucket { updated: now, tokens: 10.0 };
        for _ in 0..10 { bucket.take(now).unwrap(); }
        error_kind(bucket.take(now), LocationError::RateLimit);
        error_kind(bucket.take(now + Duration::from_millis(50)), LocationError::RateLimit);
        bucket.take(now + Duration::from_millis(100)).unwrap();
        error_kind(bucket.take(now + Duration::from_millis(100)), LocationError::RateLimit);
        for _ in 0..10 { bucket.take(now + Duration::from_secs(10)).unwrap(); }
        error_kind(bucket.take(now + Duration::from_secs(10)), LocationError::RateLimit);
        error_kind(bucket.take(now), LocationError::RateLimit);
        // An old timestamp must not mint tokens for the next concurrent request.
        error_kind(bucket.take(now + Duration::from_secs(10)), LocationError::RateLimit);
    }

    #[test]
    fn all_expensive_rpc_routes_enforce_scoped_rate_limits_before_filesystem_work() {
        let f = Fixture::new(); let now = Instant::now();
        for kind in ["mkdir", "rename", "trash", "ls"] {
            let lane = if kind == "ls" { "ls" } else { "manage" };
            REQUEST_RATE.lock().unwrap().remove(&(f.config.clone(), f.friend.clone(), lane));
            for _ in 0..10 { limit_request(&f.config, &f.friend, lane, now).unwrap(); }
            error_kind(dispatch_at(&f.config, "owner-device", &f.req(kind, "missing"), now), LocationError::RateLimit);
            error_message(dispatch_at(&f.config, "stranger", &f.req(kind, "missing"), now), "Location access denied");
        }
        let request = json!({"locations_v":VERSION,"id":"nas","paths":["missing"]});
        for _ in 0..10 { limit_request(&f.config, &f.friend, "download", now).unwrap(); }
        error_kind(download_snapshot_at(&f.config, "owner-device", &request, now), LocationError::RateLimit);
        assert!(!f.config.join("location-snapshots").exists());
        let later = now + Duration::from_secs(1);
        dispatch_at(&f.config, "owner-device", &f.req("ls", ""), later).unwrap();
        download_snapshot_at(&f.config, "owner-device", &request, later).unwrap();
    }

    #[test]
    fn cursor_entry_budget_counts_shared_snapshots_once_and_evicts_whole_data() {
        let mut cache = HashMap::new(); let now = Instant::now();
        let entries = Arc::new(vec![Entry { name:"x".into(), is_dir:false, size:0, modified:0 }; 100_000]);
        let row = |entries: Arc<Vec<Entry>>, created| Listing { config:PathBuf::new(), friend:String::new(), location:String::new(), path:String::new(), created, entries, offset:0 };
        cache.insert("a".into(), row(entries.clone(), now)); cache.insert("b".into(), row(entries.clone(), now));
        assert_eq!(cached_entries(&cache), 100_000);
        let second = Arc::new((*entries).clone()); cache.insert("c".into(), row(second, now + Duration::from_secs(1)));
        let third = Arc::new((*entries).clone()); make_listing_room(&mut cache, &third);
        cache.insert("d".into(), row(third, now + Duration::from_secs(2)));
        assert!(cached_entries(&cache) <= MAX_CACHED_ENTRIES); assert!(!cache.contains_key("a") && !cache.contains_key("b"));
    }

    #[test]
    fn stop_sharing_retires_only_verified_marker_to_trash() {
        let f = Fixture::new(); let l = load(&f.config).unwrap().remove(0); let marker = l.marker.unwrap();
        fs::write(f.root.join("keep"), b"precious").unwrap();
        assert!(save(&f.config, None, Some("nas")).unwrap().is_empty());
        assert!(!f.root.join(&marker).exists()); assert_eq!(fs::read(f.root.join("keep")).unwrap(), b"precious");
        let buckets: Vec<_> = fs::read_dir(f.root.join(".dropbeam-trash")).unwrap().flatten().collect();
        assert!(buckets.iter().any(|e| fs::read_to_string(e.path().join(&marker)).ok().as_deref() == Some(&marker)));
    }

    #[test]
    fn session_recheck_rejects_a_replaced_root_even_with_copied_marker() {
        let f = Fixture::new(); let mut l = load(&f.config).unwrap().remove(0);
        let root = Root::for_location(&mut l).unwrap();
        fs::rename(&f.root, f.dir.join("old-NAS")).unwrap(); fs::create_dir(&f.root).unwrap();
        let marker = l.marker.as_ref().unwrap(); fs::write(f.root.join(marker), marker).unwrap();
        error_kind(root.recheck(&l), LocationError::MountChanged);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn library_descendants_are_protected() {
        let f = Fixture::new(); let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        for path in [home.join("Library"), home.join("Library/Application Support")] {
            error_kind(validate_root(&f.config, &path), LocationError::UnsafeRoot);
        }
    }

}
