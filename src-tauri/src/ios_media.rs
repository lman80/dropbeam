//! Native iOS media UI. UIKit work stays on the main thread; provider copies
//! happen inside their completion callbacks, before Apple's temporary URLs expire.
use block2::RcBlock;
use objc2::{class, define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2::rc::{Allocated, Retained};
use std::cell::RefCell;
use objc2::runtime::{AnyObject, NSObject};
use objc2_foundation::{NSArray, NSError, NSItemProvider, NSString, NSURL};
use std::sync::Mutex;
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
                    match pending.await.unwrap_or_else(|_| Err("Photo provider stopped responding.".into())) {
                        Ok(path) => paths.push(path),
                        Err(error) => { failure = Some(error); }
                    }
                }
                if let Some(error) = failure {
                    for path in paths { let _ = std::fs::remove_file(path); }
                    let _ = reply.send(Err(error));
                } else {
                    let _ = reply.send(Ok(paths));
                }
            });
        }
    }
);

#[link(name = "PhotosUI", kind = "framework")]
unsafe extern "C" {}

// Tauri owns a single foreground window. Refuse to stack native presentations.
unsafe fn presenter() -> Result<Retained<AnyObject>, String> {
    let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
    let windows: Retained<NSArray<AnyObject>> = msg_send![&*app, windows];
    for window in windows.iter() {
        let key: bool = msg_send![&*window, isKeyWindow];
        if !key { continue; }
        let root: Option<Retained<AnyObject>> = msg_send![&*window, rootViewController];
        if let Some(root) = root {
            let presented: Option<Retained<AnyObject>> = msg_send![&*root, presentedViewController];
            if presented.is_some() { return Err("Close the current sheet first.".into()); }
            return Ok(root);
        }
    }
    Err("No active iOS window.".into())
}

fn copy_asset(provider: &NSItemProvider) -> oneshot::Receiver<Result<String, String>> {
    let (tx, rx) = oneshot::channel();
    let Some(ty) = provider.registeredTypeIdentifiers().firstObject() else {
        let _ = tx.send(Err("The selected asset has no file representation.".into()));
        return rx;
    };
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
        let result = (|| {
            if let Some(error) = unsafe { error.as_ref() } { return Err(error.localizedDescription().to_string()); }
            let url = unsafe { url.as_ref() }.ok_or("Could not load the selected photo or video.")?;
            let source = url.path().ok_or("Photo provider returned no file path.")?.to_string();
            let source = std::path::Path::new(&source);
            let dir = std::env::temp_dir().join("dropbeam-picked").join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let dest = dir.join(source.file_name().ok_or("Photo has no filename.")?);
            std::fs::copy(source, &dest).map_err(|e| e.to_string())?;
            Ok(dest.to_string_lossy().into_owned())
        })();
        if let Some(tx) = tx.lock().unwrap().take() { let _ = tx.send(result); }
    });
    unsafe { provider.loadFileRepresentationForTypeIdentifier_completionHandler(&ty, &block); }
    rx
}

#[tauri::command]
pub async fn pick_photos(app: AppHandle) -> Result<Vec<String>, String> {
    let _guard = PICK_LOCK.try_lock().map_err(|_| "A photo selection is already in progress.".to_string())?;
    let (reply, result) = oneshot::channel();
    
    app.run_on_main_thread(move || unsafe {
        if PICKER.with(|p| p.borrow().is_some()) {
            let _ = reply.send(Err("A photo selection is already in progress.".into()));
            return;
        }
        let this = PickerDelegate::alloc().set_ivars(PickerIvars { reply: Mutex::new(Some(reply)), providers: RefCell::new(Vec::new()) });
        let delegate: Retained<PickerDelegate> = msg_send![super(this), init];
        let setup = (|| {
            let root = presenter()?;
            let config: Retained<AnyObject> = msg_send![class!(PHPickerConfiguration), new];
            let _: () = msg_send![&*config, setSelectionLimit: 0isize];
            let images: Retained<AnyObject> = msg_send![class!(PHPickerFilter), imagesFilter];
            let videos: Retained<AnyObject> = msg_send![class!(PHPickerFilter), videosFilter];
            let filters = NSArray::from_retained_slice(&[images, videos]);
            let filter: Retained<AnyObject> = msg_send![class!(PHPickerFilter), anyFilterMatchingSubfilters: &*filters];
            let _: () = msg_send![&*config, setFilter: &*filter];
            let picker: Allocated<AnyObject> = msg_send![class!(PHPickerViewController), alloc];
            let picker: Retained<AnyObject> = msg_send![picker, initWithConfiguration: &*config];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];
            // Prevent swipe dismissal bypassing the delegate's cancel callback.
            let _: () = msg_send![&*picker, setModalInPresentation: true];
            let _: () = msg_send![&*root, presentViewController: &*picker, animated: true, completion: std::ptr::null::<AnyObject>()];
            Ok::<_, String>(())
        })();
        if let Err(error) = setup {
            if let Some(reply) = delegate.ivars().reply.lock().unwrap().take() { let _ = reply.send(Err(error)); }
        }
        PICKER.with(|p| *p.borrow_mut() = Some(delegate));
    }).map_err(|e| e.to_string())?;
    let picked = result.await.map_err(|e| e.to_string())?;
    app.run_on_main_thread(move || { PICKER.with(|p| p.borrow_mut().take()); }).map_err(|e| e.to_string())?;
    picked
}

#[tauri::command]
pub async fn share_files(app: AppHandle, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() { return Err("No files to share.".into()); }
    // Only share existing files from this app's Documents directory.
    let documents = app.path().document_dir().map_err(|e| e.to_string())?.canonicalize().map_err(|e| e.to_string())?;
    let paths: Vec<_> = paths.into_iter().map(|path| {
        let path = std::path::PathBuf::from(path).canonicalize().map_err(|e| e.to_string())?;
        if !path.starts_with(&documents) || !path.is_file() { return Err("This received file is unavailable for sharing.".to_string()); }
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
