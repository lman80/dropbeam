//! Files copied in Finder / Explorer / a Linux file manager (GitHub #37).
//!
//! A webview never sees a copied FILE — WKWebView hands a paste the file's icon
//! as an image and its name as text — so ⌘V/Ctrl+V asks the OS clipboard for
//! file references directly:
//!   • macOS: the general NSPasteboard (NSFilenamesPboardType, else one
//!     `public.file-url` per pasteboard item).
//!   • Windows: CF_HDROP.
//!   • Linux: `text/uri-list` through GTK.
//! Only paths that exist are returned; an empty list means "no files — handle
//! the paste normally".

/// Absolute paths of the files/folders on the clipboard, or empty. Sync on
/// purpose: Tauri runs non-async commands on the main thread, which AppKit's
/// pasteboard and GTK's clipboard both expect.
#[tauri::command]
pub fn clipboard_file_paths() -> Vec<String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let mut out: Vec<String> = Vec::new();
        for p in read() {
            if !p.is_empty() && std::path::Path::new(&p).exists() && !out.contains(&p) {
                out.push(p);
            }
        }
        if !out.is_empty() {
            log::info!("clipboard: {} copied file(s) on the clipboard", out.len());
        }
        out
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn read() -> Vec<String> {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSArray, NSString, NSURL};

    let pb = NSPasteboard::generalPasteboard();
    // Finder still writes the legacy list type for any copy (one or many files).
    let legacy = NSString::from_str("NSFilenamesPboardType");
    if let Some(plist) = pb.propertyListForType(&legacy) {
        if let Ok(arr) = plist.downcast::<NSArray<AnyObject>>() {
            let paths: Vec<String> = (0..arr.count())
                .filter_map(|i| arr.objectAtIndex(i).downcast::<NSString>().ok().map(|s| s.to_string()))
                .collect();
            if !paths.is_empty() {
                return paths;
            }
        }
    }
    // Modern writers: one file URL per pasteboard item.
    let file_url = NSString::from_str("public.file-url");
    let mut out = Vec::new();
    if let Some(items) = pb.pasteboardItems() {
        for item in items.iter() {
            let Some(s) = item.stringForType(&file_url) else { continue };
            let Some(url) = NSURL::URLWithString(&s) else { continue };
            if !url.isFileURL() {
                continue;
            }
            // Resolves /.file/id= reference URLs to a real path.
            let url = url.filePathURL().unwrap_or(url);
            if let Some(path) = url.path() {
                out.push(path.to_string());
            }
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn read() -> Vec<String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    const CF_HDROP: u32 = 15;
    let mut out = Vec::new();
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP).is_err() || OpenClipboard(None).is_err() {
            return out;
        }
        if let Ok(handle) = GetClipboardData(CF_HDROP) {
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..count {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                out.push(String::from_utf16_lossy(&buf[..got.min(len)]));
            }
        }
        let _ = CloseClipboard();
    }
    out
}

#[cfg(target_os = "linux")]
fn read() -> Vec<String> {
    let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    clipboard
        .wait_for_uris()
        .iter()
        .filter_map(|uri| tauri::Url::parse(uri.as_str()).ok()?.to_file_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}
