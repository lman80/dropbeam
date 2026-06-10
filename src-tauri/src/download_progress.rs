//! Native download progress in the macOS Dock's Downloads stack for files we
//! receive — the same mechanism a web browser uses. We publish an `NSProgress`
//! marked as a *file download* pointed at the destination file; macOS then shows
//! a live progress ring on it in the Downloads stack as bytes arrive.
//!
//! Pure objc2 `msg_send` against `NSProgress` / `NSURL` (no typed bindings, so we
//! don't depend on optional objc2-foundation features). The published progress is
//! thread-safe in AppKit, so the transfer task drives it directly via a raw
//! pointer wrapped to be `Send`. Off macOS the whole thing is a no-op.

#[cfg(target_os = "macos")]
mod imp {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    /// A published `NSProgress` we own (+1 retain), driven from the transfer task.
    pub struct DownloadProgress(Ptr);

    // NSProgress reporting is thread-safe; carry the pointer across `.await`s.
    struct Ptr(*mut AnyObject);
    unsafe impl Send for Ptr {}

    // These NSProgress constants' string VALUES equal their symbol names, so we
    // can use them directly without linking the Foundation symbols.
    const KIND_FILE: &str = "NSProgressKindFile";
    const OP_KIND_KEY: &str = "NSProgressFileOperationKindKey";
    const OP_DOWNLOADING: &str = "NSProgressFileOperationKindDownloading";
    const FILE_URL_KEY: &str = "NSProgressFileURLKey";

    impl DownloadProgress {
        /// Start showing download progress on `path` (the file's expected final
        /// location, e.g. in Downloads) of `total` bytes. `None` if it can't be
        /// set up — callers just ignore the absence.
        pub fn begin(path: &str, total: u64) -> Option<Self> {
            if total == 0 || path.is_empty() {
                return None;
            }
            autoreleasepool(|_| unsafe {
                let path_ns = NSString::from_str(path);
                let url: *mut AnyObject =
                    msg_send![class!(NSURL), fileURLWithPath: &*path_ns];
                if url.is_null() {
                    return None;
                }
                let alloc: *mut AnyObject = msg_send![class!(NSProgress), alloc];
                let nil: *const AnyObject = std::ptr::null();
                let progress: *mut AnyObject =
                    msg_send![alloc, initWithParent: nil, userInfo: nil];
                if progress.is_null() {
                    return None;
                }
                let _: () =
                    msg_send![progress, setKind: &*NSString::from_str(KIND_FILE)];
                let _: () = msg_send![progress, setTotalUnitCount: total as i64];
                let _: () = msg_send![
                    progress,
                    setUserInfoObject: &*NSString::from_str(OP_DOWNLOADING),
                    forKey: &*NSString::from_str(OP_KIND_KEY)
                ];
                let _: () = msg_send![
                    progress,
                    setUserInfoObject: url,
                    forKey: &*NSString::from_str(FILE_URL_KEY)
                ];
                let _: () = msg_send![progress, publish];
                Some(DownloadProgress(Ptr(progress)))
            })
        }

        /// Update bytes delivered so far.
        pub fn set(&self, done: u64) {
            unsafe {
                let _: () = msg_send![self.0 .0, setCompletedUnitCount: done as i64];
            }
        }
    }

    impl Drop for DownloadProgress {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0 .0, unpublish];
                let _: () = msg_send![self.0 .0, release];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub struct DownloadProgress;
    impl DownloadProgress {
        pub fn begin(_path: &str, _total: u64) -> Option<Self> {
            None
        }
        pub fn set(&self, _done: u64) {}
    }
}

pub use imp::DownloadProgress;
