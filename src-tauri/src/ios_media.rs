//! Native iOS media UI. UIKit work stays on the main thread; provider copies
//! happen inside their completion callbacks, before Apple's temporary URLs expire.
use block2::RcBlock;
use objc2::{class, define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2::rc::{Allocated, Retained};
use std::cell::RefCell;
use objc2::runtime::{AnyObject, NSObject};
use objc2_foundation::{NSArray, NSError, NSItemProvider, NSData, NSString, NSURL};
use std::sync::{Arc, Mutex};

#[path = "ios_media_format.rs"]
mod media_format;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

thread_local! { static PICKER: RefCell<Option<Retained<PickerDelegate>>> = const { RefCell::new(None) }; }

static PICK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

type PickReply = oneshot::Sender<Result<Vec<String>, String>>;
// PHPicker's delegate is weak: retain it until the async command finishes.
struct PickerIvars { reply: Mutex<Option<PickReply>>, providers: RefCell<Vec<Retained<NSItemProvider>>> }
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DropBeamPhotoPickerDelegate"]
    #[ivars = PickerIvars]
    struct PickerDelegate;
    impl PickerDelegate {
        #[unsafe(method(picker:didFinishPicking:))]
        fn finished(&self, picker: &AnyObject, results: &NSArray<AnyObject>) {
            log::info!("photo picker: selected {} assets; preparing providers", results.len());
            unsafe { let _: () = msg_send![picker, dismissViewControllerAnimated: true, completion: std::ptr::null::<AnyObject>()]; }
            let Some(reply) = self.ivars().reply.lock().unwrap().take() else { return };
            let pending: Vec<_> = results.iter().map(|r| {
                let provider: Retained<NSItemProvider> = unsafe { msg_send![&*r, itemProvider] };
                let pending = copy_asset(&provider);
                self.ivars().providers.borrow_mut().push(provider);
                pending
            }).collect();
            tauri::async_runtime::spawn(async move {
                let mut paths = Vec::new();
                let mut failure = None;
                // Await in selection order; only Rust channels cross threads.
                for pending in pending {
                    match tokio::time::timeout(std::time::Duration::from_secs(120), pending).await
                        .map_err(|_| "The photo could not be downloaded in time. Check your connection, open it in Photos to download it, and try again.".to_string())
                        .and_then(|r| r.map_err(|_| "Photo provider stopped responding.".to_string())).and_then(|r| r) {
                        Ok(path) => paths.push(path),
                        Err(error) => { failure = Some(error); }
                    }
                }
                if let Some(error) = failure {
                    log::info!("photo picker: failed: {error}");
                    for path in paths { let _ = std::fs::remove_file(path); }
                    let _ = reply.send(Err(error));
                } else {
                    log::info!("photo picker: prepared {} files", paths.len());
                    let _ = reply.send(Ok(paths));
                }
            });
        }
    }
);

#[link(name = "PhotosUI", kind = "framework")]
unsafe extern "C" {}

// Present above SwiftUI sheets/viewers while refusing an already active system picker.
unsafe fn presenter() -> Result<Retained<AnyObject>, String> {
    let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
    let windows: Retained<NSArray<AnyObject>> = msg_send![&*app, windows];
    for window in windows.iter() {
        let key: bool = msg_send![&*window, isKeyWindow];
        if !key { continue; }
        let root: Option<Retained<AnyObject>> = msg_send![&*window, rootViewController];
        if let Some(mut root) = root {
            loop {
                let dismissing: bool = msg_send![&*root, isBeingDismissed];
                let presenting: bool = msg_send![&*root, isBeingPresented];
                let transition: Option<Retained<AnyObject>> = msg_send![&*root, transitionCoordinator];
                if dismissing || presenting || transition.is_some() { return Err("A screen is still closing. Please try again.".into()); }
                let presented: Option<Retained<AnyObject>> = msg_send![&*root, presentedViewController];
                match presented { Some(next) => root = next, None => break }
            }
            let dismissing: bool = msg_send![&*root, isBeingDismissed];
            let photo: bool = msg_send![&*root, isKindOfClass: class!(PHPickerViewController)];
            let document: bool = msg_send![&*root, isKindOfClass: class!(UIDocumentPickerViewController)];
            let sharing: bool = msg_send![&*root, isKindOfClass: class!(UIActivityViewController)];
            let alert: bool = msg_send![&*root, isKindOfClass: class!(UIAlertController)];
            if dismissing || photo || document || sharing || alert { return Err("Finish the current picker or share sheet first.".into()); }
            return Ok(root);
        }
    }
    Err("No active iOS window.".into())
}

type AssetReply = Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>;

fn finish_asset(reply: &AssetReply, result: Result<String, String>) {
    if let Some(tx) = reply.lock().unwrap_or_else(|e| e.into_inner()).take() {
        // A timed-out request no longer owns its eventual provider output.
        if let Err(Ok(path)) = tx.send(result) { let _ = std::fs::remove_file(path); }
    }
}

fn copy_asset(provider: &NSItemProvider) -> oneshot::Receiver<Result<String, String>> {
    let (tx, rx) = oneshot::channel();
    let image = NSString::from_str("public.image");
    let movie = NSString::from_str("public.movie");
    let is_image = provider.hasItemConformingToTypeIdentifier(&image);
    let ty = if is_image { image } else if provider.hasItemConformingToTypeIdentifier(&movie) { movie }
        else { let _ = tx.send(Err("The selected asset has no image or video representation.".into())); return rx; };
    let reply = Arc::new(Mutex::new(Some(tx)));
    // Keep the provider alive through both completion blocks, including iCloud downloads.
    let Some(owned) = (unsafe { Retained::retain(provider as *const NSItemProvider as *mut NSItemProvider) }) else {
        finish_asset(&reply, Err("The photo provider is unavailable.".into())); return rx;
    };
    log::info!("photo provider: requesting file representation ({ty})");
    let block = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
        let result = (|| {
            if let Some(error) = unsafe { error.as_ref() } { return Err(error.localizedDescription().to_string()); }
            let url = unsafe { url.as_ref() }.ok_or("Could not load the selected photo or video.")?;
            let source = url.path().ok_or("Photo provider returned no file path.")?.to_string();
            let source = std::path::Path::new(&source);
            let dir = asset_directory()?;
            let dest = dir.join(source.file_name().ok_or("Photo has no filename.")?);
            std::fs::copy(source, &dest).map_err(|e| format!("Could not copy the selected photo: {e}"))?;
            log::info!("photo provider: copied file representation");
            Ok(dest.to_string_lossy().into_owned())
        })();
        match result {
            Err(error) if is_image => {
                log::info!("photo provider: file representation failed: {error}; trying image data");
                load_image_data(&owned, reply.clone(), false, error);
            }
            result => finish_asset(&reply, result),
        }
    });
    unsafe { provider.loadFileRepresentationForTypeIdentifier_completionHandler(&ty, &block); }
    rx
}

fn asset_directory() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("dropbeam-picked").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not prepare storage for the photo: {e}"))?;
    Ok(dir)
}

fn load_image_data(provider: &NSItemProvider, reply: AssetReply, jpeg: bool, previous: String) {
    let identifier = if jpeg { "public.jpeg" } else { "public.image" };
    let ty = NSString::from_str(identifier);
    let Some(owned) = (unsafe { Retained::retain(provider as *const NSItemProvider as *mut NSItemProvider) }) else {
        finish_asset(&reply, Err(previous)); return;
    };
    log::info!("photo provider: requesting data representation ({identifier})");
    let block = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let result = (|| {
            if let Some(error) = unsafe { error.as_ref() } { return Err(error.localizedDescription().to_string()); }
            let data = unsafe { data.as_ref() }.ok_or("Photo provider returned no image data.")?;
            let bytes = data.to_vec();
            if bytes.is_empty() { return Err("Photo provider returned an empty image.".into()); }
            let extension = media_format::image_extension(&bytes, identifier);
            let dest = asset_directory()?.join(format!("photo.{extension}"));
            std::fs::write(&dest, &bytes).map_err(|e| format!("Could not save the selected photo: {e}"))?;
            log::info!("photo provider: wrote {} bytes ({extension})", bytes.len());
            Ok(dest.to_string_lossy().into_owned())
        })();
        match result {
            Err(error) if !jpeg => {
                log::info!("photo provider: image data failed: {error}; trying JPEG data");
                load_image_data(&owned, reply.clone(), true, format!("{previous}; {error}"));
            }
            Err(error) => finish_asset(&reply, Err(format!("Could not prepare this photo: {previous}; {error}. If it is in iCloud, connect to the internet or download it in Photos first."))),
            result => finish_asset(&reply, result),
        }
    });
    unsafe { provider.loadDataRepresentationForTypeIdentifier_completionHandler(&ty, &block); }
}

#[tauri::command]
pub async fn pick_photos(app: AppHandle) -> Result<Vec<String>, String> {
    let _guard = PICK_LOCK.try_lock().map_err(|_| "A photo selection is already in progress.".to_string())?;
    let (reply, result) = oneshot::channel();
    
    log::info!("photo picker: requested");
    let pending_reply = Arc::new(Mutex::new(Some(reply)));
    // Retry presentation on the main thread without ever blocking UIKit animations.
    for attempt in 0..=10 {
        let reply = pending_reply.clone();
        let (tx, ready) = oneshot::channel();
        app.run_on_main_thread(move || unsafe {
            let setup = (|| {
                let root = presenter()?;
                if PICKER.with(|p| p.borrow().is_some()) { return Err("A photo selection is already in progress.".into()); }
                let this = PickerDelegate::alloc().set_ivars(PickerIvars {
                    reply: Mutex::new(reply.lock().unwrap_or_else(|e| e.into_inner()).take()),
                    providers: RefCell::new(Vec::new())
                });
                let delegate: Retained<PickerDelegate> = msg_send![super(this), init];
                let config: Retained<AnyObject> = msg_send![class!(PHPickerConfiguration), new];
                let _: () = msg_send![&*config, setSelectionLimit: 0isize];
                // PHPickerConfigurationAssetRepresentationModeCurrent = 1.
                // Preserve HEIC bytes; the transfer engine does not need JPEG conversion.
                let _: () = msg_send![&*config, setPreferredAssetRepresentationMode: 1isize];
                let images: Retained<AnyObject> = msg_send![class!(PHPickerFilter), imagesFilter];
                let videos: Retained<AnyObject> = msg_send![class!(PHPickerFilter), videosFilter];
                let filters = NSArray::from_retained_slice(&[images, videos]);
                let filter: Retained<AnyObject> = msg_send![class!(PHPickerFilter), anyFilterMatchingSubfilters: &*filters];
                let _: () = msg_send![&*config, setFilter: &*filter];
                let picker: Allocated<AnyObject> = msg_send![class!(PHPickerViewController), alloc];
                let picker: Retained<AnyObject> = msg_send![picker, initWithConfiguration: &*config];
                let _: () = msg_send![&*picker, setDelegate: &*delegate];
                let _: () = msg_send![&*picker, setModalInPresentation: true];
                let _: () = msg_send![&*root, presentViewController: &*picker, animated: true, completion: std::ptr::null::<AnyObject>()];
                PICKER.with(|p| *p.borrow_mut() = Some(delegate));
                log::info!("photo picker: presented with current representation");
                Ok::<_, String>(())
            })();
            let _ = tx.send(setup);
        }).map_err(|e| e.to_string())?;
        match ready.await.map_err(|e| e.to_string())? {
            Ok(()) => break,
            Err(error) if attempt < 10 => {
                log::info!("photo picker: waiting for presenter ({attempt}): {error}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => { log::info!("photo picker: presentation failed: {error}"); return Err(error); }
        }
    }
    let picked = result.await.map_err(|e| e.to_string())?;
    app.run_on_main_thread(move || { PICKER.with(|p| p.borrow_mut().take()); }).map_err(|e| e.to_string())?;
    picked
}

#[tauri::command]
pub async fn share_files(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() { return Err("No files to share.".into()); }
    // Received files, imported picks and sent-chat media all live in our sandbox.
    // Canonicalize both sides so a symlink cannot share a file outside it.
    let documents = app.path().document_dir().map_err(|e| e.to_string())?;
    let sandbox = documents.parent().ok_or("No app sandbox")?.canonicalize().map_err(|e| e.to_string())?;
    let paths: Vec<_> = paths.into_iter().map(|path| {
        let path = std::path::PathBuf::from(path).canonicalize().map_err(|e| e.to_string())?;
        if !path.starts_with(&sandbox) || !path.is_file() { return Err("This received file is unavailable for sharing.".to_string()); }
        Ok(path)
    }).collect::<Result<_, String>>()?;
    let (tx, rx) = oneshot::channel();
    app.run_on_main_thread(move || unsafe {
        let result = (|| {
            let root = presenter()?;
            let urls: Vec<_> = paths.iter().map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()))).collect();
            let items = NSArray::from_retained_slice(&urls);
            let sheet: Allocated<AnyObject> = msg_send![class!(UIActivityViewController), alloc];
            let sheet: Retained<AnyObject> = msg_send![sheet, initWithActivityItems: &*items, applicationActivities: std::ptr::null::<AnyObject>()];
            // Required on iPad. Anchor to the root view and suppress arrows.
            let popover: Option<Retained<AnyObject>> = msg_send![&*sheet, popoverPresentationController];
            if let Some(popover) = popover {
                let view: Retained<AnyObject> = msg_send![&*root, view];
                let _: () = msg_send![&*popover, setSourceView: &*view];
                let _: () = msg_send![&*popover, setPermittedArrowDirections: 0usize];
            }
            let _: () = msg_send![&*root, presentViewController: &*sheet, animated: true, completion: std::ptr::null::<AnyObject>()];
            Ok(())
        })();
        let _ = tx.send(result);
    }).map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}
