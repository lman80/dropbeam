//! Windows host side of Locations: the same pinned, handle-relative contract as
//! `mod unix` (docs/locations-phase1.md, "Windows hosts"). Every component is
//! opened with NtCreateFile relative to its parent's handle and
//! FILE_OPEN_REPARSE_POINT; any reparse point (symlink, junction, mount point,
//! cloud placeholder) is refused like a Unix symlink, as is crossing to another
//! volume serial. Identity is volume serial + file id instead of st_dev/st_ino.
//! Renames are FileRenameInformation with ReplaceIfExists = FALSE relative to the
//! pinned destination handle, issued on the source's own handle: native
//! no-replace on NTFS, ReFS, FAT, exFAT and SMB, so Windows needs neither a
//! capability probe nor the reservation fallback. Deletes go through a handle
//! whose identity was checked, never a name lookup.
use super::*;
use std::ffi::{OsStr, OsString};
use std::os::windows::{ffi::{OsStrExt, OsStringExt}, fs::OpenOptionsExt, io::{AsRawHandle, FromRawHandle}};
use windows::Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem::{NtCreateFile, NtSetInformationFile, FileRenameInformation,
    FILE_RENAME_INFORMATION, NTCREATEFILE_CREATE_DISPOSITION, NTCREATEFILE_CREATE_OPTIONS, FILE_OPEN, FILE_CREATE,
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT}};
use windows::Win32::{Foundation::{HANDLE, NTSTATUS, UNICODE_STRING, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, ERROR_NO_MORE_FILES},
    System::IO::IO_STATUS_BLOCK, Storage::FileSystem::{GetFileInformationByHandle, GetFileInformationByHandleEx, SetFileInformationByHandle,
    BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FILE_ATTRIBUTE_TAG_INFO, FILE_FULL_DIR_INFO, FILE_BASIC_INFO, FILE_DISPOSITION_INFO,
    FILE_DISPOSITION_INFO_EX, FILE_DISPOSITION_INFO_EX_FLAGS, FILE_NAME_INFO, FileNameInfo, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FileIdInfo, FileAttributeTagInfo,
    FileFullDirectoryInfo, FileFullDirectoryRestartInfo, FileBasicInfo, FileDispositionInfo, FileDispositionInfoEx,
    FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE, DELETE, SYNCHRONIZE,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY, FILE_TRAVERSE, FILE_READ_ATTRIBUTES,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT}};

fn handle(f: &fs::File) -> HANDLE { HANDLE(f.as_raw_handle()) }
/// HRESULT_FROM_WIN32(code) → the Win32 code, so ERROR_ALREADY_EXISTS reads as
/// io::ErrorKind::AlreadyExists (same decoding as `windows_move_noreplace`).
fn io_err(e: windows::core::Error) -> std::io::Error {
    let hr = e.code().0 as u32;
    std::io::Error::from_raw_os_error(if hr & 0xFFFF_0000 == 0x8007_0000 { (hr & 0xFFFF) as i32 } else { hr as i32 })
}
fn nt_err(status: NTSTATUS) -> std::io::Error { std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32) }

/// Volume serial + 128-bit file id (ReFS needs the wide id); falls back to the
/// 64-bit file index where FileIdInfo is unsupported (FAT, some SMB servers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Id { pub volume: u64, file: [u8; 16] }
pub(super) fn id(f: &fs::File) -> Result<Id> {
    let mut info = FILE_ID_INFO::default();
    if unsafe { GetFileInformationByHandleEx(handle(f), FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(), std::mem::size_of::<FILE_ID_INFO>() as u32) }.is_ok() {
        return Ok(Id { volume: info.VolumeSerialNumber, file: info.FileId.Identifier });
    }
    let mut b = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(handle(f), &mut b) }.map_err(io_err)?;
    let mut file = [0; 16]; file[..8].copy_from_slice(&((b.nFileIndexHigh as u64) << 32 | b.nFileIndexLow as u64).to_le_bytes());
    Ok(Id { volume: b.dwVolumeSerialNumber as u64, file })
}
fn attributes(f: &fs::File) -> Result<u32> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    unsafe { GetFileInformationByHandleEx(handle(f), FileAttributeTagInfo, (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) }.map_err(io_err)?;
    Ok(info.FileAttributes)
}
const PROTECTED: u32 = FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0;
/// Hidden+system entries are the OS's own ($RECYCLE.BIN, System Volume
/// Information, desktop.ini): Explorer hides them and friends never see them.
pub(super) fn protected_entry(attributes: u32) -> bool { attributes & PROTECTED == PROTECTED }
fn reserved(name: &OsStr) -> bool { name.to_string_lossy().to_ascii_lowercase().starts_with(".dropbeam-") }
/// Control data (.dropbeam-*) is created hidden, like a dotfile on Unix.
fn hidden(name: &OsStr) -> u32 {
    if reserved(name) { FILE_ATTRIBUTE_HIDDEN.0 } else { FILE_ATTRIBUTE_NORMAL.0 }
}
/// The name the object behind `f` really has (its last path component) — the
/// long name even when it was opened by its 8.3 short name. None if the file
/// system won't say.
fn real_name(f: &fs::File) -> Option<OsString> {
    let mut cap = 1024usize;
    for _ in 0..2 {
        let mut buf = vec![0u64; (std::mem::size_of::<FILE_NAME_INFO>() + cap).div_ceil(8)];
        let size = (buf.len() * 8) as u32;
        let got = unsafe { GetFileInformationByHandleEx(handle(f), FileNameInfo, buf.as_mut_ptr().cast(), size) };
        let info = buf.as_ptr().cast::<FILE_NAME_INFO>();
        let len = unsafe { (*info).FileNameLength } as usize;
        if got.is_ok() && len <= size as usize {
            let all = unsafe { std::slice::from_raw_parts(std::ptr::addr_of!((*info).FileName).cast::<u16>(), len / 2) };
            return Some(OsString::from_wide(all.rsplit(|&c| c == '\\' as u16).next().unwrap_or(all)));
        }
        if len <= cap || len > 1 << 17 { return None; }
        cap = len;
    }
    None
}
/// "DROPBE~1", "DROPBE~12.JSO": the shape of an 8.3 short name (only used when
/// the real name can't be read).
pub(super) fn short_alias(name: &str) -> bool {
    let (base, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let Some((_, digits)) = base.rsplit_once('~') else { return false };
    base.len() <= 8 && ext.len() <= 3 && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// NtCreateFile relative to `parent` (an empty name reopens `parent` itself as a
/// fresh file object). Never follows a reparse point: FILE_OPEN_REPARSE_POINT
/// opens the link itself, which `checked` then refuses.
fn nt_open(parent: &fs::File, name: &OsStr, access: u32, disposition: NTCREATEFILE_CREATE_DISPOSITION, options: u32, attributes: u32) -> std::io::Result<fs::File> {
    let mut wide: Vec<u16> = name.encode_wide().collect();
    // One component only: a separator would let NT walk several components at
    // once, a colon would open an alternate data stream.
    if wide.iter().any(|&c| c == '\\' as u16 || c == '/' as u16 || c == ':' as u16 || c == 0) || wide.len() > 32_767 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file name"));
    }
    let bytes = (wide.len() * 2) as u16;
    let unicode = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: windows::core::PWSTR(wide.as_mut_ptr()) };
    let object = OBJECT_ATTRIBUTES { Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32, RootDirectory: handle(parent),
        ObjectName: &unicode, Attributes: OBJ_CASE_INSENSITIVE, ..Default::default() };
    let mut out = HANDLE(std::ptr::null_mut());
    let mut io = IO_STATUS_BLOCK::default();
    let status = unsafe { NtCreateFile(&mut out, FILE_ACCESS_RIGHTS(access | SYNCHRONIZE.0), &object, &mut io, None,
        FILE_FLAGS_AND_ATTRIBUTES(attributes), FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, disposition,
        NTCREATEFILE_CREATE_OPTIONS(options | FILE_SYNCHRONOUS_IO_NONALERT.0 | FILE_OPEN_REPARSE_POINT.0), None, 0) };
    if status.0 < 0 { return Err(nt_err(status)); }
    let f = unsafe { fs::File::from_raw_handle(out.0) };
    // NT also resolves 8.3 short names ("DROPBE~1"): unless control data was
    // asked for by its own name, what opened must not be control data.
    if disposition == FILE_OPEN && !name.is_empty() && !reserved(name) {
        let alias = match real_name(&f) { Some(real) => reserved(&real), None => short_alias(&name.to_string_lossy()) };
        if alias { return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Reserved location path")); }
    }
    Ok(f)
}
/// The Unix `child` checks: no reparse point, same volume as the parent, and
/// no protected OS entry.
fn checked(f: fs::File, parent: &fs::File) -> Result<fs::File> {
    let attributes = attributes(&f)?;
    ensure!(attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0, "Links, junctions and mount points are not supported");
    ensure!(!protected_entry(attributes), "Protected system items are not shared");
    ensure!(id(&f)?.volume == id(parent)?.volume, "Crossing into another mount is not allowed");
    Ok(f)
}
pub(super) fn child(parent: &fs::File, name: &OsStr, directory: bool) -> Result<fs::File> {
    let access = FILE_GENERIC_READ.0 | if directory { FILE_TRAVERSE.0 } else { 0 };
    checked(nt_open(parent, name, access, FILE_OPEN, if directory { FILE_DIRECTORY_FILE.0 } else { 0 }, 0)?, parent)
}
/// Open for rename/delete. DELETE access is what the operation runs on.
fn movable(parent: &fs::File, name: &OsStr) -> Result<fs::File> {
    checked(nt_open(parent, name, DELETE.0 | FILE_READ_ATTRIBUTES.0, FILE_OPEN, 0, 0)?, parent)
}
/// Never opens the entry's data, so a link or a locked file still reads as
/// occupied (and is therefore skipped, never published over).
fn exists_at(parent: &fs::File, name: &OsStr) -> Result<bool> {
    match nt_open(parent, name, FILE_READ_ATTRIBUTES.0, FILE_OPEN, 0, 0) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
/// Exclusive create (FILE_CREATE fails on ANY existing entry, link or not).
pub(super) fn mkdir(parent: &fs::File, name: &OsStr) -> Result<()> {
    nt_open(parent, name, FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0, FILE_CREATE, FILE_DIRECTORY_FILE.0, hidden(name))?; Ok(())
}
fn create(parent: &fs::File, name: &OsStr) -> Result<fs::File> {
    Ok(nt_open(parent, name, FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0 | DELETE.0, FILE_CREATE, FILE_NON_DIRECTORY_FILE.0, hidden(name))?)
}
/// Rename the object behind `source` (opened with DELETE) to `to`\`new`.
/// ReplaceIfExists = FALSE: an occupied name fails with AlreadyExists.
fn rename_handle(source: &fs::File, to: &fs::File, new: &OsStr) -> Result<()> {
    let wide: Vec<u16> = new.encode_wide().collect();
    ensure!(!wide.is_empty() && !wide.iter().any(|&c| c == '\\' as u16 || c == '/' as u16 || c == ':' as u16), "Invalid file name");
    let header = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let length = (header + wide.len() * 2).max(std::mem::size_of::<FILE_RENAME_INFORMATION>());
    let mut buffer = vec![0u64; length.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    unsafe {
        (*info).Anonymous.Flags = 0; // ReplaceIfExists = FALSE
        (*info).RootDirectory = handle(to);
        (*info).FileNameLength = (wide.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), wide.len());
    }
    let mut io = IO_STATUS_BLOCK::default();
    let status = unsafe { NtSetInformationFile(handle(source), &mut io, info.cast(), length as u32, FileRenameInformation) };
    if status.0 < 0 { return Err(nt_err(status)).context("Could not move item without replacing data"); }
    Ok(())
}
pub(super) fn rename(from: &fs::File, old: &OsStr, to: &fs::File, new: &OsStr) -> Result<()> {
    rename_handle(&movable(from, old)?, to, new)
}
/// Delete through a handle (POSIX semantics where supported, so the name is
/// gone immediately even if an indexer holds the file open).
fn delete(f: &fs::File) -> Result<()> {
    let ex = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_INFO_EX_FLAGS(FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0) };
    if unsafe { SetFileInformationByHandle(handle(f), FileDispositionInfoEx, (&ex as *const FILE_DISPOSITION_INFO_EX).cast(), std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32) }.is_ok() { return Ok(()); }
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe { SetFileInformationByHandle(handle(f), FileDispositionInfo, (&info as *const FILE_DISPOSITION_INFO).cast(), std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) }.map_err(io_err)?;
    Ok(())
}
/// Remove one entry by name. `directory = false` never removes a real folder
/// (it may remove a link, which is only the link). Folders must be empty.
fn unlink(parent: &fs::File, name: &OsStr, directory: bool) -> Result<()> {
    let f = nt_open(parent, name, DELETE.0 | FILE_READ_ATTRIBUTES.0, FILE_OPEN, if directory { FILE_DIRECTORY_FILE.0 } else { 0 }, 0)?;
    let attributes = attributes(&f)?;
    ensure!(directory || attributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0, "Is a directory");
    delete(&f)
}
/// A published upload must not stay hidden like its temp name was.
fn unhide(f: &fs::File) {
    let info = FILE_BASIC_INFO { FileAttributes: FILE_ATTRIBUTE_NORMAL.0, ..Default::default() };
    if let Err(e) = unsafe { SetFileInformationByHandle(handle(f), FileBasicInfo, (&info as *const FILE_BASIC_INFO).cast(), std::mem::size_of::<FILE_BASIC_INFO>() as u32) } {
        log::warn!("Published upload may remain hidden: {e}");
    }
}

pub(super) struct Item { pub name: OsString, pub attributes: u32, pub size: u64, pub modified_ms: u64 }
/// Enumerate through the handle (FileFullDirectoryInfo), on a fresh file object
/// so a duplicated root handle never shares its enumeration cursor.
pub(super) fn entries(dir: &fs::File, mut visit: impl FnMut(Item) -> Result<bool>) -> Result<()> {
    let fresh = match nt_open(dir, OsStr::new(""), FILE_LIST_DIRECTORY.0 | FILE_READ_ATTRIBUTES.0, FILE_OPEN, FILE_DIRECTORY_FILE.0, 0) {
        Ok(fresh) => fresh,
        // Some redirectors refuse a nameless relative open; the restart class
        // below still starts the scan from the top.
        Err(e) => { log::debug!("Directory reopen failed ({e}); listing through a duplicate handle"); dir.try_clone()? }
    };
    let mut buffer = vec![0u64; 8192];
    let mut class = FileFullDirectoryRestartInfo;
    loop {
        if let Err(e) = unsafe { GetFileInformationByHandleEx(handle(&fresh), class, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32) } {
            let e = io_err(e);
            if e.raw_os_error() == Some(ERROR_NO_MORE_FILES.0 as i32) { return Ok(()); }
            return Err(e.into());
        }
        class = FileFullDirectoryInfo;
        let mut offset = 0usize;
        loop {
            // Entries are 8-byte aligned within the buffer, which is u64-backed.
            let info = unsafe { buffer.as_ptr().cast::<u8>().add(offset).cast::<FILE_FULL_DIR_INFO>() };
            let (next, attributes, size, written, length) = unsafe { ((*info).NextEntryOffset, (*info).FileAttributes, (*info).EndOfFile, (*info).LastWriteTime, (*info).FileNameLength) };
            let name = OsString::from_wide(unsafe { std::slice::from_raw_parts(std::ptr::addr_of!((*info).FileName).cast::<u16>(), length as usize / 2) });
            if name != "." && name != ".." {
                // FILETIME: 100 ns ticks since 1601.
                let modified_ms = (written.max(0) as u64).saturating_sub(116_444_736_000_000_000) / 10_000;
                if !visit(Item { name, attributes, size: size.max(0) as u64, modified_ms })? { return Ok(()); }
            }
            if next == 0 { break; }
            offset += next as usize;
        }
    }
}
pub(super) fn names(dir: &fs::File, mut visit: impl FnMut(&OsStr) -> Result<bool>) -> Result<()> {
    entries(dir, |item| visit(&item.name))
}
pub(super) fn open_root(path: &Path) -> Result<fs::File> {
    let dir = fs::OpenOptions::new().access_mode(FILE_GENERIC_READ.0 | FILE_TRAVERSE.0).share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0).open(path)?;
    let attributes = attributes(&dir)?;
    ensure!(attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0, "Location must be a folder");
    ensure!(attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0, "Links, junctions and mount points are not supported");
    Ok(dir)
}
pub(super) fn verify_marker(dir: &fs::File, marker: &str) -> Result<()> {
    use std::io::Read;
    let mut f = child(dir, OsStr::new(marker), false).context(LocationError::MountChanged)?;
    let mut value = String::new(); (&mut f).take(256).read_to_string(&mut value)?;
    ensure!(value == marker, LocationError::MountChanged); Ok(())
}

/// Names Windows (or its Win32 path layer) refuses or silently rewrites:
/// device names with any extension, trailing dots/spaces, reserved characters.
pub(super) fn valid_name(part: &str) -> Result<()> {
    ensure!(!part.chars().any(|c| (c as u32) < 0x20 || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')), "Invalid file name on Windows (contains a reserved character)");
    ensure!(!part.ends_with(['.', ' ']), "Invalid file name on Windows (ends with a dot or space)");
    let base = part.split('.').next().unwrap_or(part).trim_end_matches(' ').to_uppercase();
    let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|p| base.strip_prefix(p).is_some_and(|n| matches!(n, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")));
    ensure!(!device, "Invalid file name on Windows (reserved device name)");
    Ok(())
}
/// `\\?\D:\x` → `D:\x` (and `\\?\UNC\s\x` → `\\s\x`) when every component
/// survives Win32 normalization unchanged, so stored paths, the UI and Explorer
/// agree. std re-adds the prefix for long paths on its own.
pub(super) fn simplify(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else { return path; };
    let candidate = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") { format!(r"\\{rest}") }
        else if let Some(rest) = text.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' { return path; }
            rest.to_string()
        } else { return path; };
    let tail = match candidate.strip_prefix(r"\\") { Some(unc) => unc.splitn(3, '\\').nth(2).unwrap_or(""), None => candidate.get(3..).unwrap_or("") };
    if tail.split('\\').filter(|p| !p.is_empty()).all(|p| valid_name(p).is_ok()) { PathBuf::from(candidate) } else { path }
}
/// Component-wise, case-insensitive `a.starts_with(b)` (NTFS names are
/// case-insensitive; canonical paths should agree on case, this doesn't rely on it).
pub(super) fn starts_with(a: &Path, b: &Path) -> bool {
    let key = |p: &Path| p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    let (a, b) = (key(a), key(b));
    a.len() >= b.len() && a[..b.len()] == b[..]
}
/// (refused when the root equals or contains one, refused when the root is
/// inside one). A whole non-system drive such as `D:\` is allowed.
pub(super) fn protected() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let resolve = |p: PathBuf| canonical(&p).unwrap_or(p);
    let drive = std::env::var("SystemDrive").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "C:".into());
    let mut inside = vec![var("SystemRoot").or_else(|| var("windir")).unwrap_or_else(|| PathBuf::from(format!(r"{drive}\Windows")))];
    inside.extend(["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "APPDATA", "LOCALAPPDATA"].iter().filter_map(|k| var(k)));
    let profile = var("USERPROFILE");
    if let Some(p) = &profile { inside.push(p.join("AppData")); }
    let inside: Vec<PathBuf> = inside.into_iter().map(resolve).collect();
    let mut ancestors = inside.clone();
    ancestors.push(resolve(PathBuf::from(format!(r"{drive}\"))));
    ancestors.extend(profile.map(resolve));
    (ancestors, inside)
}

impl Root {
    pub(super) fn verified_dir(l: &Location) -> Result<fs::File> {
        let marker = l.marker.as_ref().context("Location mount marker missing; save it in Settings first")?;
        ensure!(marker.strip_prefix(".dropbeam-mount-").is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()), "Invalid mount marker");
        let dir = open_root(Path::new(&l.path)).context("Location mount unavailable")?;
        verify_marker(&dir, marker)?;
        Ok(dir)
    }
    pub(super) fn retire_marker(l: &Location) -> Result<()> {
        let dir = Self::verified_dir(l)?;
        let root = Self { path: PathBuf::from(&l.path), dir, native: true };
        root.trash_name(&root.dir, OsStr::new(l.marker.as_ref().unwrap()))?;
        Ok(())
    }
    pub(super) fn create_marker(&self) -> Result<String> {
        use std::io::Write;
        let marker = format!(".dropbeam-mount-{}", uuid::Uuid::new_v4());
        let mut f = create(&self.dir, OsStr::new(&marker))?;
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
    pub(super) fn parent(&self, raw: &str) -> Result<(fs::File, OsString)> {
        let rel = relative(raw)?;
        let name = rel.file_name().context("The location root cannot be modified")?.to_owned();
        let parent = rel.parent().unwrap_or(Path::new(""));
        self.resolve(&slash(parent))?;
        Ok((self.open_rel(parent, true)?, name))
    }
    pub fn stat_entry(&self, raw: &str) -> Result<Option<(bool, u64)>> {
        let rel = relative(raw)?;
        let Ok(file) = self.open_rel(&rel, false) else { return Ok(None); };
        let meta = file.metadata()?;
        Ok(Some((meta.is_dir(), if meta.is_file() { meta.len() } else { 0 })))
    }
    /// Handle-relative twin of the Unix `stat_many`: one parent walk, one open per leaf.
    pub fn stat_many(&self, raw_parent: &str, leaves: &[String]) -> Result<Vec<Option<(bool, u64, u64)>>> {
        let rel = relative(raw_parent)?;
        let Ok(dir) = self.open_rel(&rel, true) else { return Ok(vec![None; leaves.len()]); };
        Ok(leaves.iter().map(|leaf| {
            let file = child(&dir, OsStr::new(leaf), false).ok()?;
            let meta = file.metadata().ok()?;
            Some((meta.is_dir(), if meta.is_file() { meta.len() } else { 0 }, crate::iroh_net::mtime_secs(&meta)))
        }).collect())
    }
    pub fn hash_entry(&self, raw: &str, cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(u64)) -> Result<Option<String>> {
        let rel = relative(raw)?;
        let Ok(file) = self.open_rel(&rel, false) else { return Ok(None); };
        if !file.metadata().map(|m| m.is_file()).unwrap_or(false) { return Ok(None); }
        match sha256_file_progress(file, cancel, progress) {
            Ok(digest) => Ok(Some(digest)),
            Err(e) if e.to_string().contains("canceled") => Err(e),
            Err(_) => Ok(None),
        }
    }
    /// Metadata comes from the enumeration itself: no per-entry open, so a file
    /// another program holds exclusively is still listed.
    pub fn listing(&self, raw: &str) -> Result<Vec<Entry>> {
        self.resolve(raw)?;
        let dir = self.open_rel(&relative(raw)?, true)?;
        let mut out = Vec::new();
        entries(&dir, |item| {
            let Some(name) = item.name.to_str() else { return Ok(true); };
            if relative(name).is_err() || item.attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || protected_entry(item.attributes) { return Ok(true); }
            ensure!(out.len() < 100_000, "Folder listing exceeds 100000 entries");
            let is_dir = item.attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
            out.push(Entry { name: name.into(), is_dir, size: if is_dir { 0 } else { item.size }, modified: item.modified_ms });
            Ok(true)
        })?;
        Ok(out)
    }
    pub fn new_folder(&self, raw: &str) -> Result<()> {
        let (parent, name) = self.parent(raw)?;
        mkdir(&parent, &name)
    }
    pub fn rename_item(&self, raw: &str, to: &str) -> Result<()> {
        self.resolve(raw)?;
        let (parent, name) = self.parent(raw)?;
        let (new_parent, new_name) = self.parent(to)?;
        rename(&parent, &name, &new_parent, &new_name)
    }
    pub fn trash(&self, raw: &str) -> Result<String> {
        self.resolve(raw)?;
        let (parent, name) = self.parent(raw)?;
        self.trash_name(&parent, &name)
    }
    pub(super) fn trash_name(&self, parent: &fs::File, name: &OsStr) -> Result<String> {
        let trash = OsStr::new(".dropbeam-trash");
        if let Err(e) = mkdir(&self.dir, trash) {
            if child(&self.dir, trash, true).is_err() { return Err(e); }
        }
        let dir = child(&self.dir, trash, true)?;
        let timestamp = format!("{}-{}", crate::chat::now_ms(), uuid::Uuid::new_v4());
        mkdir(&dir, OsStr::new(&timestamp))?;
        let bucket = child(&dir, OsStr::new(&timestamp), true)?;
        if let Err(e) = rename(parent, name, &bucket, name) {
            drop(bucket); let _ = unlink(&dir, OsStr::new(&timestamp), true); return Err(e);
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
    pub fn create_stage(&self, key: &str) -> Result<PathBuf> {
        ensure!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()), "Invalid stage key");
        let name = OsStr::new(".dropbeam-staging");
        if let Err(e) = mkdir(&self.dir, name) {
            if child(&self.dir, name, true).is_err() { return Err(e); }
        }
        let parent = child(&self.dir, name, true)?;
        let key_name = OsStr::new(key);
        if let Err(e) = mkdir(&parent, key_name) {
            if child(&parent, key_name, true).is_err() { return Err(e); }
        }
        child(&parent, key_name, true)?;
        Ok(self.path.join(name).join(key))
    }
    pub fn remove_stage(&self, key: &str) -> Result<()> {
        ensure!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()), "Invalid stage key");
        fn remove(parent: &fs::File, name: &OsStr) -> Result<()> {
            if let Ok(dir) = child(parent, name, true) {
                names(&dir, |entry| { remove(&dir, entry)?; Ok(true) })?;
                drop(dir);
                unlink(parent, name, true)
            } else { unlink(parent, name, false) }
        }
        let parent = child(&self.dir, OsStr::new(".dropbeam-staging"), true)?;
        remove(&parent, OsStr::new(key))?;
        drop(parent);
        let _ = unlink(&self.dir, OsStr::new(".dropbeam-staging"), true);
        Ok(())
    }
    pub fn gc_stages(&self) -> Result<()> {
        let Ok(parent) = child(&self.dir, OsStr::new(".dropbeam-staging"), true) else { return Ok(()); };
        names(&parent, |name| {
            let Some(key) = name.to_str() else { return Ok(true); };
            if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) { return Ok(true); }
            let path = self.path.join(".dropbeam-staging").join(key);
            let _lease = {
                let mut leases = UPLOADS.lock().unwrap_or_else(|p| p.into_inner());
                if leases.contains(&path) { return Ok(true); }
                leases.push(path.clone()); UploadLease(path)
            };
            if !stage_expired(&self.path.join(".dropbeam-staging").join(key)) { return Ok(true); }
            if let Err(e) = self.remove_stage(key) { log::warn!("Location staging GC: {e:#}"); }
            Ok(true)
        })
    }
    /// True when `existing` is byte-for-byte the staged upload (see Unix twin).
    fn identical(mut existing: fs::File, staged: &Path, digest: &str, cancel: &std::sync::atomic::AtomicBool, progress: &impl Fn(u64)) -> Result<bool> {
        use sha2::{Digest, Sha256}; use std::io::Read;
        let mut source = fs::File::open(staged)?;
        if source.metadata()?.len() != existing.metadata()?.len() { return Ok(false); }
        let mut hash = Sha256::new(); let mut b = vec![0; 1 << 20]; let mut other = vec![0; 1 << 20]; let mut done = 0;
        loop {
            ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
            let n = source.read(&mut b)?; if n == 0 { break; }
            existing.read_exact(&mut other[..n])?;
            if b[..n] != other[..n] { return Ok(false); }
            hash.update(&b[..n]); done += n as u64; progress(done);
        }
        ensure!(hex::encode(hash.finalize()) == digest, "Upload not verified: SHA-256 verification mismatch");
        Ok(true)
    }
    /// The integrity receipt already verified the streamed v2 payload: publish the
    /// staged file itself, renamed through its own (identity-checked) handle.
    pub fn land_verified(&self, raw: &str, staged: &Path, digest: &str, replace: bool, cancel: &std::sync::atomic::AtomicBool,
        progress: impl Fn(u64), publish: impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>) -> Result<Landed> {
        let rel = staged.strip_prefix(self.path.join(".dropbeam-staging"))
            .context("Upload not verified: stage is outside location staging")?;
        ensure!(rel.components().count() >= 2 && rel.components().all(|c| matches!(c, Component::Normal(_))),
            "Upload not verified: invalid stage path");
        let source_rel = staged.strip_prefix(&self.path)?;
        let from = self.open_rel(source_rel.parent().context("Missing stage parent")?, true)?;
        let stage_name = staged.file_name().context("Missing stage name")?;
        let source = checked(nt_open(&from, stage_name, FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0 | DELETE.0, FILE_OPEN, FILE_NON_DIRECTORY_FILE.0, 0)?, &from)?;
        ensure!(source.metadata()?.is_file(), "Invalid stage type");
        source.sync_all()?;
        let size = source.metadata()?.len();
        progress(size);
        ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
        let rel = relative(raw)?;
        publish(&mut || self.ensure_dirs(&slash(rel.parent().unwrap_or(Path::new("")))))?;
        let (parent, name) = self.parent(raw)?;
        let identity = crate::iroh_net::receive_stage::Identity::of(&source)?;
        let same_existing = |candidate: &OsStr, existing_id: crate::iroh_net::receive_stage::Identity| -> Result<()> {
            ensure!(crate::iroh_net::receive_stage::Identity::of(&child(&parent, candidate, false)?)? == existing_id, "Destination changed during verification");
            Ok(())
        };
        if replace {
            if exists_at(&parent, &name)? {
                let existing = child(&parent, &name, false).context("An item with this name already exists")?;
                let meta = existing.metadata()?;
                ensure!(meta.is_file(), "An item with this name already exists");
                if meta.len() == size {
                    let existing_id = crate::iroh_net::receive_stage::Identity::of(&existing)?;
                    if sha256_file_progress(existing, cancel, &progress)? == digest {
                        publish(&mut || same_existing(&name, existing_id))?;
                        return Ok(Landed { path: self.path.join(&rel), conflict: false, replaced: false });
                    }
                }
            }
            let replaced = std::cell::Cell::new(false);
            publish(&mut || {
                replaced.set(false);
                if exists_at(&parent, &name)? { self.trash_name(&parent, &name)?; replaced.set(true); }
                ensure!(crate::iroh_net::receive_stage::Identity::of(&source)? == identity, "Receive stage identity changed before publication");
                rename_handle(&source, &parent, &name)
            })?;
            return Ok(Landed { path: self.path.join(&rel), conflict: false, replaced: replaced.get() });
        }
        for index in 0..LAND_NAME_LIMIT {
            let candidate = sibling_name(&name, index);
            let landed = self.path.join(sibling_rel(&rel, &candidate)?);
            if exists_at(&parent, &candidate)? {
                let existing = match child(&parent, &candidate, false) {
                    Ok(existing) => existing,
                    Err(e) if index == 0 => return Err(e).context("An item with this name already exists"),
                    Err(_) => continue,
                };
                let meta = existing.metadata()?;
                ensure!(meta.is_file() || index > 0, "An item with this name already exists");
                if !meta.is_file() || meta.len() != size { continue; }
                let existing_id = crate::iroh_net::receive_stage::Identity::of(&existing)?;
                if sha256_file_progress(existing, cancel, &progress)? != digest { continue; }
                publish(&mut || same_existing(&candidate, existing_id))?;
                return Ok(Landed { path: landed, conflict: index > 0, replaced: false });
            }
            match publish(&mut || rename_handle(&source, &parent, &candidate)) {
                Ok(()) => return Ok(Landed { path: landed, conflict: index > 0, replaced: false }),
                Err(e) if already_exists(&e) => continue,
                Err(e) => return Err(e),
            }
        }
        bail!("Upload destination name space exhausted ({LAND_NAME_LIMIT} candidates)")
    }
    /// Copy from private transfer staging into an exclusive hidden temp file,
    /// hash-check, flush, then rename that handle into place without replacing.
    pub fn land(&self, raw: &str, staged: &Path, digest: &str, replace: bool, cancel: &std::sync::atomic::AtomicBool, progress: impl Fn(u64), publish: impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()>) -> Result<Landed> {
        let expected = fs::metadata(staged)?.len();
        let rel = relative(raw)?;
        publish(&mut || self.ensure_dirs(&slash(rel.parent().unwrap_or(Path::new("")))))?;
        let (parent, name) = self.parent(raw)?;
        let mut index = 0usize;
        let mut replacing = false;
        loop {
            ensure!(index < LAND_NAME_LIMIT, "Upload destination name space exhausted ({LAND_NAME_LIMIT} candidates)");
            let candidate = sibling_name(&name, index);
            let landed_rel = sibling_rel(&rel, &candidate)?;
            if !exists_at(&parent, &candidate)? { break; }
            let existing = match child(&parent, &candidate, false) {
                Ok(existing) => existing,
                Err(e) if index == 0 => return Err(e).context("An item with this name already exists"),
                Err(_) => { index += 1; continue; }
            };
            let meta = existing.metadata()?;
            ensure!(meta.is_file() || index > 0, "An item with this name already exists");
            if replace && index == 0 && meta.is_file() {
                if Self::identical(existing, staged, digest, cancel, &progress)? {
                    publish(&mut || Ok(()))?;
                    return Ok(Landed { path: self.path.join(landed_rel), conflict: false, replaced: false });
                }
                replacing = true; break;
            }
            if index == 0 && meta.is_file() && meta.len() == 0 && expected > 0 {
                log::warn!("Location upload blocked by empty destination {raw:?} id={:?}; possible leftover reservation, ownership cannot be proven; preserved for owner review", id(&existing).ok());
                bail!("An empty destination already exists (possibly a leftover reservation); ownership cannot be proven. Ask the owner to move it to trash before retrying");
            }
            if meta.is_file() && Self::identical(existing, staged, digest, cancel, &progress)? {
                publish(&mut || Ok(()))?;
                return Ok(Landed { path: self.path.join(landed_rel), conflict: index > 0, replaced: false });
            }
            index += 1;
        }
        let tmp = OsString::from(format!(".dropbeam-upload-{}.part", uuid::Uuid::new_v4()));
        let mut f = create(&parent, &tmp)?;
        let result = (|| -> Result<PathBuf> {
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
            ensure!(hex::encode(hash.finalize()) == digest, "Upload not verified: SHA-256 verification mismatch; upload was not published");
            f.sync_all()?;
            ensure!(!cancel.load(std::sync::atomic::Ordering::SeqCst), "canceled");
            if replacing {
                publish(&mut || {
                    if exists_at(&parent, &name)? { self.trash_name(&parent, &name)?; }
                    rename_handle(&f, &parent, &name)
                })?;
                return Ok(self.path.join(sibling_rel(&rel, &name)?));
            }
            loop {
                ensure!(index < LAND_NAME_LIMIT, "Upload destination name space exhausted ({LAND_NAME_LIMIT} candidates)");
                let candidate = sibling_name(&name, index);
                let landed_rel = sibling_rel(&rel, &candidate)?;
                match publish(&mut || rename_handle(&f, &parent, &candidate)) {
                    Ok(()) => return Ok(self.path.join(landed_rel)),
                    Err(e) if already_exists(&e) => index += 1,
                    Err(e) => return Err(e),
                }
            }
        })();
        // The temp file is removed through our own handle: never a name lookup.
        if result.is_err() { let _ = delete(&f); } else { unhide(&f); }
        drop(f);
        Ok(Landed { path: result?, conflict: index > 0 && !replacing, replaced: replacing })
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
    pub fn select(&self, raw: &str, wire: &str, budget: &mut Budget, depth: usize,
        skipped: &mut Vec<String>, items: &mut Vec<(PathBuf, String, u64, u64)>, dirs: &mut Vec<String>) -> Result<()> {
        ensure!(depth < 64 && budget.entries > 0, "Selection too large or deeply nested"); budget.entries -= 1;
        let rel = relative(raw)?;
        let source = match self.open_rel(&rel, false) {
            Ok(f) => f,
            Err(e) => { skipped.push(format!("{raw}: {}", crate::telemetry::redact_paths_only(&format!("{e:#}")))); return Ok(()); }
        };
        let meta = source.metadata()?;
        if meta.is_file() {
            budget.consume(meta.len())?;
            items.push((self.path.join(rel), wire.into(), meta.len(),
                meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)));
        } else {
            dirs.push(wire.into());
            // Protected OS entries (desktop.ini, $RECYCLE.BIN) are left out silently,
            // as Explorer hides them; links are skipped and reported like on Unix.
            let mut children = Vec::new();
            entries(&source, |item| { if !protected_entry(item.attributes) { children.push(item.name); } Ok(true) })?;
            for name in children {
                let Some(name) = name.to_str() else { skipped.push(format!("{raw}: non-Unicode filename")); continue; };
                if name.to_ascii_lowercase().starts_with(".dropbeam-") { continue; }
                self.select(&format!("{raw}/{name}"), &format!("{wire}/{name}"), budget, depth + 1, skipped, items, dirs)?;
            }
        }
        Ok(())
    }
}
