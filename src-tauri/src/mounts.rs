//! What's plugged in or mounted right now, for the "Add a location" wizard's
//! first step. A non-technical host shouldn't have to know what
//! `/run/user/1000/gvfs/sftp:host=buddy-files` means: the wizard lists the NAS
//! shares and external disks this device can already see and lets them pick one.
//!
//! Read-only and cheap: one read of /proc/mounts (Linux), one directory
//! listing of /Volumes (macOS) or one GetLogicalDrives (Windows), plus one
//! statvfs / GetDiskFreeSpaceExW per candidate. Nothing here
//! grants access to anything — saving the location still goes through
//! `locations::save`, which validates the root and writes the mount marker.
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountCandidate {
    /// Plain-language name for the choice — the mount's own folder name
    /// ("buddy-nas", "Backup Drive"). Never the device string: that can carry a
    /// user name or host credentials.
    pub label: String,
    pub path: String,
    /// The filesystem type as the OS reports it ("fuse.sshfs", "smbfs", "exfat").
    /// Shown only as a small hint under the label.
    pub fstype: String,
    /// "network" — a NAS / network drive; "removable" — an external or extra
    /// disk; "fixed" — another internal drive (Windows `D:`). The wizard labels
    /// network mounts "Network drive".
    pub kind: String,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

/// Network filesystems: a NAS, wherever it happens to be mounted.
const NETWORK: &[&str] = &["cifs", "smb3", "smbfs", "smb", "nfs", "nfs4", "afpfs", "webdav",
    "davfs", "davfs2", "fuse.davfs", "fuse.sshfs", "sshfs", "fuse.rclone", "rclone", "fuse.gvfsd-fuse"];
/// Ordinary filesystems that only count as a removable/extra disk when they are
/// mounted somewhere a person plugs things in, never when they are the system.
const REMOVABLE: &[&str] = &["exfat", "vfat", "msdos", "ntfs", "ntfs3", "fuseblk", "ext4", "ext3",
    "btrfs", "xfs", "hfsplus", "apfs", "udf", "iso9660"];
/// Where a removable disk shows up on Linux desktops.
const MEDIA_ROOTS: &[&str] = &["/media", "/mnt", "/run/media"];
/// Mountpoints that are the operating system, not a place for files.
const SYSTEM_ROOTS: &[&str] = &["/proc", "/sys", "/dev", "/boot", "/var", "/usr", "/etc", "/tmp",
    "/snap", "/System", "/private", "/opt", "/lib", "/bin", "/sbin", "/nix", "/run/lock",
    "/run/user/0", "/run/snapd", "/run/credentials"];

fn under(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}
fn label_for(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// /proc/mounts escapes space, tab, newline and backslash as octal. Decoded
/// byte-wise so a share named "Caf\u{e9}" survives intact.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1..i + 4].iter().all(|b| b.is_ascii_digit() && *b < b'8') {
            out.push((bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0'));
            i += 4;
        } else { out.push(bytes[i]); i += 1; }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse /proc/mounts into the choices worth offering. `home` keeps a disk
/// mounted inside the host's own home directory (a common NAS habit) while
/// still excluding the system. Pure: the caller adds free/total bytes.
pub fn parse_proc_mounts(text: &str, home: Option<&Path>) -> Vec<MountCandidate> {
    let home = home.map(|h| h.to_string_lossy().into_owned());
    let mut out: Vec<MountCandidate> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_device), Some(point), Some(fstype)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let point = unescape(point);
        let fstype = unescape(fstype);
        if point == "/" || !point.starts_with('/') { continue; }
        if SYSTEM_ROOTS.iter().any(|root| under(&point, root)) { continue; }
        let network = NETWORK.contains(&fstype.as_str()) || fstype.starts_with("fuse.sshfs") || fstype.starts_with("fuse.rclone");
        let removable = REMOVABLE.contains(&fstype.as_str())
            && (MEDIA_ROOTS.iter().any(|root| under(&point, root))
                || home.as_deref().is_some_and(|h| under(&point, h) && point != h));
        if !network && !removable { continue; }
        // A later mount on the same point hides the earlier one.
        out.retain(|m| m.path != point);
        out.push(MountCandidate { label: label_for(&point), path: point,
            kind: if network { "network" } else { "removable" }.into(), fstype, free_bytes: None, total_bytes: None });
    }
    out
}

#[cfg(target_os = "macos")]
fn volumes() -> Vec<MountCandidate> {
    use std::os::unix::fs::MetadataExt;
    let boot = std::fs::metadata("/").map(|m| m.dev()).ok();
    let Ok(entries) = std::fs::read_dir("/Volumes") else { return vec![] };
    let mut out = vec![];
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') { continue; }
        // "Macintosh HD" is a symlink to /: the boot volume is never a choice.
        let Ok(link) = entry.path().symlink_metadata() else { continue };
        if link.file_type().is_symlink() { continue; }
        let Ok(meta) = entry.path().metadata() else { continue };
        if !meta.is_dir() || Some(meta.dev()) == boot { continue; }
        let path = entry.path().to_string_lossy().into_owned();
        let (fstype, local) = describe(&entry.path());
        out.push(MountCandidate { label: name, path, fstype,
            kind: if local { "removable" } else { "network" }.into(), free_bytes: None, total_bytes: None });
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// macOS: the volume's filesystem type, and whether it is a local disk (as
/// opposed to a mounted share). One statfs.
#[cfg(target_os = "macos")]
fn describe(path: &Path) -> (String, bool) {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return ("unknown".into(), true) };
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 { return ("unknown".into(), true); }
    let stat = unsafe { stat.assume_init() };
    let name: Vec<u8> = stat.f_fstypename.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    (String::from_utf8_lossy(&name).into_owned(), stat.f_flags & libc::MNT_LOCAL as u32 != 0)
}

/// A GNOME gvfs share is one directory per connection, named with its transport
/// and options: `sftp:host=buddy-files,user=penis`. Show the host.
fn pretty_gvfs(name: &str) -> String {
    let options = name.split_once(':').map(|(_, rest)| rest).unwrap_or(name);
    let value = |key: &str| options.split(',').find_map(|o| o.strip_prefix(key)).filter(|v| !v.is_empty());
    match (value("share="), value("host=").or_else(|| value("server="))) {
        (Some(share), Some(host)) => format!("{share} on {host}"),
        (None, Some(host)) => host.to_string(),
        _ => name.to_string(),
    }
}
/// /run/user/N/gvfs is a container of shares, not a share: offer its children.
#[cfg(target_os = "linux")]
fn expand_gvfs(list: Vec<MountCandidate>) -> Vec<MountCandidate> {
    let mut out = Vec::new();
    for m in list {
        if !m.fstype.contains("gvfsd-fuse") { out.push(m); continue; }
        let Ok(entries) = std::fs::read_dir(&m.path) else { continue };
        for entry in entries.flatten().take(20) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() { continue; }
            out.push(MountCandidate { label: pretty_gvfs(&name), path: entry.path().to_string_lossy().into_owned(),
                fstype: m.fstype.clone(), kind: m.kind.clone(), free_bytes: None, total_bytes: None });
        }
    }
    out
}

/// One Windows drive letter as a wizard choice: "Storage (D:)". The system
/// drive is never offered (Locations refuses it), nor are CD drives, RAM disks
/// or letters with no medium. Pure: the caller asks Windows for the facts.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn describe_drive(letter: char, drive_type: u32, label: &str, fstype: &str, system_drive: &str) -> Option<MountCandidate> {
    if !letter.is_ascii_alphabetic() || system_drive.trim_end_matches(['\\', ':']).eq_ignore_ascii_case(&letter.to_string()) { return None; }
    // GetDriveTypeW: 2 removable, 3 fixed, 4 remote.
    let (kind, fallback) = match drive_type { 2 => ("removable", "USB Drive"), 3 => ("fixed", "Local Disk"), 4 => ("network", "Network Drive"), _ => return None };
    let letter = letter.to_ascii_uppercase();
    let name = if label.trim().is_empty() { fallback } else { label.trim() };
    Some(MountCandidate { label: format!("{name} ({letter}:)"), path: format!("{letter}:\\"), fstype: fstype.into(),
        kind: kind.into(), free_bytes: None, total_bytes: None })
}
#[cfg(windows)]
fn drives() -> Vec<MountCandidate> {
    use windows::{core::PCWSTR, Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW}};
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let mask = unsafe { GetLogicalDrives() };
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    (0..26u8).filter(|i| mask & (1 << i) != 0).filter_map(|i| {
        let letter = (b'A' + i) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
        let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
        if !matches!(kind, 2..=4) { return None; }
        let (mut label, mut fstype) = ([0u16; 261], [0u16; 261]);
        // A card reader with no card or an offline network drive: skip quietly.
        unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut label), None, None, None, Some(&mut fstype)) }.ok()?;
        describe_drive(letter, kind, &text(&label), &text(&fstype), &system)
    }).collect()
}

fn candidates() -> Vec<MountCandidate> {
    #[cfg(target_os = "macos")] let mut list = volumes();
    #[cfg(target_os = "linux")]
    let mut list = expand_gvfs(parse_proc_mounts(&std::fs::read_to_string("/proc/mounts").unwrap_or_default(),
        std::env::var_os("HOME").as_deref().map(Path::new)));
    #[cfg(windows)] let mut list = drives();
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))] let mut list: Vec<MountCandidate> = vec![];
    list.truncate(50);
    for m in &mut list {
        if let Some((free, total)) = crate::locations::volume_bytes(Path::new(&m.path)) {
            m.free_bytes = Some(free); m.total_bytes = Some(total);
        }
    }
    list
}

/// Step 1 of the wizard: the network shares and external disks this device can
/// see. Empty is fine — the wizard always offers "Choose another folder…".
/// Async + blocking pool: a sync command runs on the MAIN thread, and statfs on a
/// disconnected network share can block indefinitely — freezing the whole UI.
#[tauri::command]
pub async fn list_mount_candidates() -> Vec<MountCandidate> {
    tauri::async_runtime::spawn_blocking(candidates).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0
penis@buddy:/mnt/shares /home/penis/buddy-nas fuse.sshfs rw,nosuid,nodev,relatime,user_id=1000 0 0
//buddy/media /mnt/media\040drive cifs rw,relatime 0 0
/dev/sdb1 /media/penis/Backup exfat rw,relatime 0 0
/dev/sdc1 /srv/scratch ext4 rw,relatime 0 0
tmpfs /run/user/1000 tmpfs rw,nosuid,nodev,relatime 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw,nosuid,nodev,relatime,user_id=1000 0 0
"#;

    /// The wizard's first step must show the NAS and the plugged-in disk, and
    /// nothing a person would never share (the system, /boot, tmpfs).
    #[test]
    fn proc_mounts_offers_nas_shares_and_removable_disks_only() {
        let found = parse_proc_mounts(SAMPLE, Some(Path::new("/home/penis")));
        let paths: Vec<_> = found.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, vec!["/home/penis/buddy-nas", "/mnt/media drive", "/media/penis/Backup",
            "/run/user/1000/gvfs"], "{found:#?}");
        let nas = &found[0];
        assert_eq!((nas.label.as_str(), nas.kind.as_str(), nas.fstype.as_str()), ("buddy-nas", "network", "fuse.sshfs"));
        // The octal escape for a space is decoded, and the label is the folder
        // name — never the device string, which can carry a user or a host.
        assert_eq!((found[1].label.as_str(), found[1].kind.as_str()), ("media drive", "network"));
        assert_eq!((found[2].label.as_str(), found[2].kind.as_str()), ("Backup", "removable"));
        assert!(found.iter().all(|m| m.free_bytes.is_none()), "sizes are added by the caller");
    }

    /// The root filesystem, system mounts and an ordinary disk mounted outside
    /// /media, /mnt or the host's home are never offered.
    #[test]
    fn proc_mounts_never_offers_the_system_or_an_unrelated_disk() {
        let found = parse_proc_mounts(SAMPLE, Some(Path::new("/home/penis")));
        for path in ["/", "/sys", "/proc", "/boot/efi", "/srv/scratch", "/run/user/1000"] {
            assert!(!found.iter().any(|m| m.path == path), "{path} must not be offered: {found:#?}");
        }
        // Without a home directory the NAS under it drops out, the rest stays.
        let no_home = parse_proc_mounts(SAMPLE, None);
        assert!(no_home.iter().any(|m| m.path == "/home/penis/buddy-nas"), "an sshfs share is a NAS wherever it is mounted");
        assert!(!no_home.iter().any(|m| m.path == "/srv/scratch"));
        // A later mount over the same point replaces the earlier entry.
        let stacked = parse_proc_mounts("//a/b /mnt/share cifs rw 0 0\n//c/d /mnt/share nfs4 rw 0 0\n", None);
        assert_eq!(stacked.len(), 1);
        assert_eq!(stacked[0].fstype, "nfs4");
    }

    /// Windows: the owner's data drive reads "Storage (D:)"; the system drive,
    /// CD drives and unknown types are never offered.
    #[test]
    fn windows_drives_offer_data_disks_but_never_the_system_drive() {
        let d = describe_drive('d', 3, "Storage", "NTFS", "C:").unwrap();
        assert_eq!((d.label.as_str(), d.path.as_str(), d.kind.as_str(), d.fstype.as_str()), ("Storage (D:)", "D:\\", "fixed", "NTFS"));
        assert_eq!(describe_drive('E', 2, "  ", "exFAT", "C:").unwrap().label, "USB Drive (E:)");
        assert_eq!(describe_drive('Z', 4, "", "NTFS", "C:").unwrap().kind, "network");
        assert!(describe_drive('C', 3, "Windows", "NTFS", "C:").is_none());
        assert!(describe_drive('c', 3, "Windows", "NTFS", "C:\\").is_none());
        for kind in [0, 1, 5, 6] { assert!(describe_drive('F', kind, "x", "x", "C:").is_none()); }
    }

    /// A GNOME gvfs share reads as its host, not as its connection string.
    #[test]
    fn gvfs_share_names_read_as_the_machine_they_come_from() {
        assert_eq!(pretty_gvfs("sftp:host=buddy-files,user=penis"), "buddy-files");
        assert_eq!(pretty_gvfs("smb-share:server=buddy,share=media"), "media on buddy");
        assert_eq!(pretty_gvfs("Backup"), "Backup");
    }
}
