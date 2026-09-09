//! Ownership is persisted before body writes. An OS lock survives task moves and
//! distinguishes a crashed owner from an active receive in another process.
use super::*;
use std::{fs::{self, File, OpenOptions}, io::{Read, Write}, time::SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Identity { dev: u64, ino: u64 }
impl Identity {
    pub(crate) fn of(file: &File) -> Result<Self> {
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            let m = file.metadata()?;
            Ok(Self { dev: m.dev(), ino: m.ino() })
        }
        #[cfg(windows)] {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION}};
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?; }
            Ok(Self { dev: info.dwVolumeSerialNumber as u64, ino: (info.nFileIndexHigh as u64) << 32 | info.nFileIndexLow as u64 })
        }
    }
    pub(crate) fn matches(self, path: &Path) -> bool {
        open_regular(path).and_then(|f| Self::of(&f)).is_ok_and(|id| id == self)
    }
    pub(crate) fn remove(self, path: &Path) -> Result<bool> {
        // Never follow a symlink or unlink a new occupant at the staged name.
        if !self.matches(path) { return Ok(false); }
        fs::remove_file(path)?;
        Ok(true)
    }
}
fn open_regular(path: &Path) -> Result<File> {
    anyhow::ensure!(fs::symlink_metadata(path)?.is_file(), "not a regular stage file");
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    let file = options.open(path)?;
    anyhow::ensure!(file.metadata()?.is_file(), "not a regular stage file");
    Ok(file)
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Ownership {
    path: PathBuf,
    #[serde(flatten)] identity: Identity,
    size: u64,
    created: SystemTime,
    transfer_id: String,
}
fn sidecar(path: &Path) -> PathBuf { path.with_extension("owner.json") }
pub(super) fn is_receive_stage(name: &str) -> bool {
    // Include malformed/lookalike names so unknown files are logged too.
    name.starts_with(".dropbeam-recv-") && name.ends_with(".part")
}
pub(super) struct ReceiveStage {
    path: PathBuf,
    identity: Identity,
    // Pin the inode and retain its lock until cleanup/publication is finished.
    _file: File,
    registry: Option<(File, Identity)>,
    armed: bool,
}
impl ReceiveStage {
    pub(super) fn create(path: PathBuf, size: u64, transfer_id: &str) -> Result<(Self, File)> {
        // Keep the recovery path independent of the next process's working directory.
        let path = fs::canonicalize(path.parent().context("stage parent missing")?)?
            .join(path.file_name().context("stage name missing")?);
        // No guard exists until exclusive creation succeeds.
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
        let identity = Identity::of(&file)?;
        let mut stage = Self { path, identity, _file: file.try_clone()?, registry: None, armed: true };
        file.try_lock()?;
        let record = Ownership { path: stage.path.clone(), identity, size, created: SystemTime::now(), transfer_id: transfer_id.into() };
        let mut registry = OpenOptions::new().read(true).write(true).create_new(true).open(sidecar(&stage.path))?;
        stage.registry = Some((registry.try_clone()?, Identity::of(&registry)?));
        registry.write_all(&serde_json::to_vec(&record)?)?;
        registry.sync_all()?;
        if let Some(dir) = stage.path.parent() {
            #[cfg(unix)] File::open(dir)?.sync_all()?;
            note_partial_dir(dir);
        }
        Ok((stage, file))
    }
    pub(super) fn remove(&mut self) -> Result<()> {
        if self.armed {
            self.identity.remove(&self.path)?;
            self.armed = false;
            self.forget_registry();
        }
        Ok(())
    }
    pub(super) fn publish(&mut self, natural: &Path) -> Result<PathBuf> {
        let landed = publish_unique_owned(&self.path, natural, RECEIVE_NAME_LIMIT, Some(self.identity))?;
        self.published();
        Ok(landed)
    }
    fn published(&mut self) {
        // The publisher already consumed the staged NAME (including hard links).
        // If its unlink failed, retain the record for later recovery, never retry
        // that unlink from Drop.
        self.armed = false;
        if !self.identity.matches(&self.path) { self.forget_registry(); }
    }
    fn forget_registry(&mut self) {
        if let Some((_, id)) = &self.registry {
            if let Err(e) = id.remove(&sidecar(&self.path)) { log::warn!("Stage registry cleanup failed: {e:#}"); }
        }
        self.registry = None;
    }
}
impl Drop for ReceiveStage {
    fn drop(&mut self) {
        if self.armed {
            if let Err(e) = self.remove() { log::warn!("Stage cleanup deferred: {e:#}"); }
        }
    }
}
/// Return freed bytes. Unknown, changed, young, or locked stages are untouched.
pub(super) fn recover(path: &Path, now: SystemTime) -> u64 {
    let Some(parent) = path.parent().and_then(|p| fs::canonicalize(p).ok()) else { return 0; };
    let Some(name) = path.file_name() else { return 0; };
    let path = parent.join(name);
    let path = path.as_path();
    let read = (|| -> Result<_> {
        let mut registry = open_regular(&sidecar(path))?;
        let registry_id = Identity::of(&registry)?;
        let mut bytes = vec![];
        (&mut registry).take(64 * 1024).read_to_end(&mut bytes)?;
        let record: Ownership = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(record.path == path && !record.transfer_id.is_empty(), "invalid stage ownership");
        Ok((registry, registry_id, record))
    })();
    let Ok((_registry, registry_id, record)) = read else {
        log::warn!("Unregistered receive-stage-looking file left untouched: {}", path.display());
        return 0;
    };
    if !now.duration_since(record.created).is_ok_and(|age| age > TRANSFER_STALL) { return 0; }
    let Ok(file) = open_regular(path) else { return 0; };
    if Identity::of(&file).ok() != Some(record.identity) {
        log::warn!("Receive stage identity changed; left untouched: {}", path.display());
        return 0;
    }
    // flock/LockFileEx on the pinned inode also protects another process's stage.
    if file.try_lock().is_err() { return 0; }
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    match record.identity.remove(path) {
        Ok(true) => { let _ = registry_id.remove(&sidecar(path)); size }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("dropbeam-stage-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(".dropbeam-recv-{}.part", uuid::Uuid::new_v4()));
        (dir, path)
    }
    fn aged() -> SystemTime { SystemTime::now() + TRANSFER_STALL + Duration::from_secs(1) }
    #[test]
    fn recovery_requires_persisted_identity_age_and_inactive_owner() {
        let (dir, path) = fixture();
        fs::write(&path, b"user file").unwrap();
        assert_eq!(recover(&path, aged()), 0);
        assert_eq!(fs::read(&path).unwrap(), b"user file");
        fs::remove_file(&path).unwrap();
        let (mut stage, mut file) = ReceiveStage::create(path.clone(), 4, "owning-transfer").unwrap();
        file.write_all(b"body").unwrap();
        let record: Ownership = serde_json::from_slice(&fs::read(sidecar(&path)).unwrap()).unwrap();
        assert_eq!(record.path, fs::canonicalize(&path).unwrap()); assert_eq!(record.size, 4); assert_eq!(record.transfer_id, "owning-transfer");
        assert_eq!(record.identity, Identity::of(&file).unwrap());
        // Recovery uses a separately opened descriptor, as another process does.
        assert_eq!(recover(&path, aged()), 0); assert!(path.exists());
        stage.armed = false; drop(stage); drop(file); // simulate a crashed owner
        assert_eq!(recover(&path, SystemTime::now()), 0); assert!(path.exists());
        assert_eq!(recover(&path, aged()), 4); assert!(!path.exists());
        assert!(!sidecar(&path).exists());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn recovery_and_drop_never_remove_replacements_or_failed_create_occupants() {
        let (dir, path) = fixture();
        fs::write(&path, b"occupied").unwrap();
        assert!(ReceiveStage::create(path.clone(), 4, "failed-create").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"occupied");
        fs::remove_file(&path).unwrap();
        let (mut stage, file) = ReceiveStage::create(path.clone(), 4, "owner").unwrap();
        fs::rename(&path, dir.join("original")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert_eq!(recover(&path, aged()), 0);
        assert!(stage.publish(&dir.join("refused")).is_err());
        assert!(!dir.join("refused").exists());
        drop(stage); drop(file);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn hard_link_publication_and_explicit_removal_disarm_drop() {
        let (dir, path) = fixture();
        for hard_link in [false, true] {
            let (mut stage, mut file) = ReceiveStage::create(path.clone(), 4, "owner").unwrap();
            file.write_all(b"body").unwrap();
            if hard_link {
                crate::locations::FORCE_HARD_LINK.with(|flag| flag.set(true));
                let result = stage.publish(&dir.join("landed"));
                crate::locations::FORCE_HARD_LINK.with(|flag| flag.set(false));
                result.unwrap();
                assert_eq!(fs::read(dir.join("landed")).unwrap(), b"body");
            } else { stage.remove().unwrap(); }
            assert!(!path.exists());
            fs::write(&path, b"new occupant").unwrap();
            drop(stage); drop(file);
            assert_eq!(fs::read(&path).unwrap(), b"new occupant", "Drop must not unlink the staged name twice");
            fs::remove_file(&path).unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn locations_exclude_the_entire_sweep_including_registered_stages_and_partials() {
        let (dir, _) = fixture();
        let config = dir.join("config"); let root = config.join("folder-partials");
        fs::create_dir_all(&config).unwrap(); fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(config.join("locations.json"), serde_json::to_vec(&serde_json::json!([{
            "id":"shared", "name":"shared", "path":root, "friend_ids":[], "rights":{"upload":false,"manage":false}
        }])).unwrap()).unwrap();
        // Verify the fixture loads, rather than accidentally exercising fail-closed parsing.
        crate::locations::load(&config).unwrap();
        for parent in [&root, &root.join("nested")] {
            let path = parent.join(format!(".dropbeam-recv-{}.part", uuid::Uuid::new_v4()));
            let (mut stage, file) = ReceiveStage::create(path.clone(), 0, "owner").unwrap();
            let mut record: Ownership = serde_json::from_slice(&fs::read(sidecar(&path)).unwrap()).unwrap();
            record.created = SystemTime::now() - TRANSFER_STALL - Duration::from_secs(1);
            fs::write(sidecar(&path), serde_json::to_vec(&record).unwrap()).unwrap();
            stage.armed = false; drop(stage); drop(file);
            let partial = parent.join(".dropbeam-partial-test.part");
            let file = File::create(&partial).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(PARTIAL_TTL_SECS + 1))).unwrap();
            super::super::gc_stale_partials_at(parent, None, &config);
            assert_eq!(IrohState::default().clear_transfer_cache(&config), 0);
            assert!(path.exists()); assert!(partial.exists()); assert!(sidecar(&path).exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
