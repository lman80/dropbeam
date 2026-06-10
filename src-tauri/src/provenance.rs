//! Stamp the SENDER onto a received file as a macOS extended attribute
//! (`com.dropbeam.from`), so provenance travels WITH the file. This is the data
//! layer the Finder Sync extension reads to badge files with who they came from
//! (and it already shows up to anything that reads xattrs). Best-effort and
//! no-op off macOS — a failure here never affects the transfer.

#[cfg(target_os = "macos")]
pub fn set_sender(path: &std::path::Path, sender: &str) {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;

    let sender = sender.trim();
    if sender.is_empty() {
        return;
    }
    extern "C" {
        fn setxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const c_void,
            size: usize,
            position: u32,
            options: c_int,
        ) -> c_int;
    }
    let (Ok(cpath), Ok(cname)) = (
        CString::new(path.as_os_str().as_bytes()),
        CString::new("com.dropbeam.from"),
    ) else {
        return;
    };
    let val = sender.as_bytes();
    // options=0 (create or replace), position=0.
    unsafe {
        setxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            val.as_ptr() as *const c_void,
            val.len(),
            0,
            0,
        );
    }
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn get_sender(path: &std::path::Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn getxattr(
            path: *const c_char,
            name: *const c_char,
            value: *mut c_void,
            size: usize,
            position: u32,
            options: c_int,
        ) -> isize;
    }
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    let cname = CString::new("com.dropbeam.from").ok()?;
    let mut buf = vec![0u8; 512];
    let n = unsafe {
        getxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut c_void,
            buf.len(),
            0,
            0,
        )
    };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn set_sender(_path: &std::path::Path, _sender: &str) {}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn sender_xattr_round_trips() {
        let p = std::env::temp_dir().join(format!("dropbeam-prov-{}.txt", std::process::id()));
        std::fs::write(&p, b"hi").unwrap();
        set_sender(&p, "Ashton");
        assert_eq!(get_sender(&p).as_deref(), Some("Ashton"));
        // Empty sender is a no-op (doesn't clobber).
        set_sender(&p, "  ");
        assert_eq!(get_sender(&p).as_deref(), Some("Ashton"));
        let _ = std::fs::remove_file(&p);
    }
}
