// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 AJ Media Studio (TetalCut, a fork of Concat)

//! The UIKit side of [`crate`]. Everything that touches a view controller
//! runs on the main thread (where the window's tap handlers run); the
//! picked files are copied on whichever queue the system answers on.

use std::cell::RefCell;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::{Block, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_foundation::{NSArray, NSString, NSURL};

// UIKit and Foundation come with the window; these three do not.
#[link(name = "PhotosUI", kind = "framework")]
unsafe extern "C" {}
#[link(name = "Photos", kind = "framework")]
unsafe extern "C" {}
#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {}
#[link(name = "AVFAudio", kind = "framework")]
unsafe extern "C" {}

/// Sound that plays with the ring/silent switch on silent, as a video
/// editor's must: the audio session's Playback category (the default,
/// SoloAmbient, is silenced by the switch, and cpal does not set one).
pub fn setup_audio() {
    let Ok(session) = class(c"AVAudioSession") else {
        log::warn!("no AVAudioSession: sound follows the silent switch");
        return;
    };
    unsafe {
        let shared: Option<Retained<AnyObject>> = msg_send![session, sharedInstance];
        let Some(shared) = shared else { return };
        let mut error: *mut AnyObject = std::ptr::null_mut();
        // the value of AVAudioSessionCategoryPlayback
        let set: bool = msg_send![&*shared, setCategory: &*ns("AVAudioSessionCategoryPlayback"), error: &mut error];
        if !set {
            log::warn!("could not set the audio session's category");
        }
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let active: bool = msg_send![&*shared, setActive: true, error: &mut error];
        if !active {
            log::warn!("could not activate the audio session");
        }
    }
}

/// Where a published export goes, as the export sheet names it.
pub const PHOTOS_FOLDER: &str = "Fotos";

/// What the window asked to be told when a pick is done.
type Picked = Box<dyn FnOnce(Vec<PathBuf>) + Send>;

/// The pick in flight.
static PENDING: Mutex<Option<Picked>> = Mutex::new(None);

thread_local! {
    /// The delegate of the picker on screen: a picker holds its delegate
    /// weakly, so something has to keep it alive until the pick ends.
    static KEEP: RefCell<Option<Retained<NSObject>>> = const { RefCell::new(None) };
}

/// Answers the pick in flight, once.
fn finish(paths: Vec<PathBuf>) {
    let pending = PENDING.lock().ok().and_then(|mut slot| slot.take());
    if let Some(pending) = pending {
        pending(paths);
    }
}

fn class(name: &CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| format!("{} is not available on this iPhone", name.to_string_lossy()))
}

fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

/// The window's Import on an iPhone: a sheet with Photos and Files, then
/// that picker. `on_picked` gets the copies in the app's media folder, or
/// nothing when the pick was cancelled.
pub fn pick(on_picked: Picked) {
    let previous = PENDING.lock().ok().and_then(|mut slot| slot.replace(on_picked));
    if let Some(previous) = previous {
        // a pick that was already up is told it got nothing
        previous(Vec::new());
    }
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("the media picker was asked for off the main thread");
        finish(Vec::new());
        return;
    };
    if let Err(error) = show_sheet(mtm) {
        log::error!("could not show the media picker: {error}");
        finish(Vec::new());
    }
}

/// The view controller on top: the one a sheet is presented from.
fn top_controller() -> Result<Retained<AnyObject>, String> {
    unsafe {
        let app: Option<Retained<AnyObject>> = msg_send![class(c"UIApplication")?, sharedApplication];
        let app = app.ok_or("no application")?;
        let scenes: Retained<AnyObject> = msg_send![&*app, connectedScenes];
        let scenes: Retained<NSArray<AnyObject>> = msg_send![&*scenes, allObjects];
        let window_scene = class(c"UIWindowScene")?;
        for scene in scenes.iter() {
            let is_window_scene: bool = msg_send![&*scene, isKindOfClass: window_scene];
            if !is_window_scene {
                continue;
            }
            let windows: Retained<NSArray<AnyObject>> = msg_send![&*scene, windows];
            let mut chosen: Option<Retained<AnyObject>> = None;
            for window in windows.iter() {
                let key: bool = msg_send![&*window, isKeyWindow];
                if key || chosen.is_none() {
                    chosen = Some(window);
                }
                if key {
                    break;
                }
            }
            let Some(window) = chosen else { continue };
            let root: Option<Retained<AnyObject>> = msg_send![&*window, rootViewController];
            let Some(mut top) = root else { continue };
            loop {
                let next: Option<Retained<AnyObject>> = msg_send![&*top, presentedViewController];
                match next {
                    Some(next) => top = next,
                    None => break,
                }
            }
            return Ok(top);
        }
        Err("no window to show the picker on".into())
    }
}

fn present(controller: &AnyObject) -> Result<(), String> {
    let presenter = top_controller()?;
    unsafe {
        let _: () = msg_send![&*presenter, presentViewController: controller, animated: true, completion: None::<&Block<dyn Fn()>>];
    }
    Ok(())
}

/// The sheet: Photos, Files or Cancel.
fn show_sheet(mtm: MainThreadMarker) -> Result<(), String> {
    let _ = mtm;
    unsafe {
        // UIAlertControllerStyleActionSheet
        let sheet: Retained<AnyObject> = msg_send![
            class(c"UIAlertController")?,
            alertControllerWithTitle: &*ns("Importar"),
            message: &*ns("¿De dónde quieres traer los videos o fotos?"),
            preferredStyle: 0isize
        ];
        let action = class(c"UIAlertAction")?;
        let photos = RcBlock::new(|_action: *mut AnyObject| {
            let result = MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string()).and_then(show_photos);
            if let Err(error) = result {
                log::error!("could not show Photos: {error}");
                finish(Vec::new());
            }
        });
        let files = RcBlock::new(|_action: *mut AnyObject| {
            let result = MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string()).and_then(show_files);
            if let Err(error) = result {
                log::error!("could not show Files: {error}");
                finish(Vec::new());
            }
        });
        let cancel = RcBlock::new(|_action: *mut AnyObject| finish(Vec::new()));
        // UIAlertActionStyleDefault 0, Cancel 1
        for (title, style, handler) in [("Fotos", 0isize, &photos), ("Archivos", 0, &files), ("Cancelar", 1, &cancel)] {
            let item: Retained<AnyObject> = msg_send![action, actionWithTitle: &*ns(title), style: style, handler: &**handler];
            let _: () = msg_send![&*sheet, addAction: &*item];
        }
        // on an iPad a sheet is a popover and needs something to point at
        let popover: Option<Retained<AnyObject>> = msg_send![&*sheet, popoverPresentationController];
        if let Some(popover) = popover {
            let presenter = top_controller()?;
            let view: Option<Retained<AnyObject>> = msg_send![&*presenter, view];
            if let Some(view) = view {
                let _: () = msg_send![&*popover, setSourceView: &*view];
            }
        }
        present(&sheet)
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TetalCutPhotosPickerDelegate"]
    struct PhotosDelegate;

    impl PhotosDelegate {
        /// PHPickerViewControllerDelegate: the pick is done (or cancelled).
        #[unsafe(method(picker:didFinishPicking:))]
        fn did_finish(&self, picker: &AnyObject, results: &NSArray<AnyObject>) {
            unsafe {
                let _: () = msg_send![picker, dismissViewControllerAnimated: true, completion: None::<&Block<dyn Fn()>>];
            }
            photos_picked(results);
            KEEP.with(|keep| keep.borrow_mut().take());
        }
    }
);

impl PhotosDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TetalCutFilesPickerDelegate"]
    struct FilesDelegate;

    impl FilesDelegate {
        /// UIDocumentPickerDelegate: files chosen (copies, `asCopy`).
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &AnyObject, urls: &NSArray<AnyObject>) {
            let mut paths = Vec::new();
            for url in urls.iter() {
                if let Some(path) = bring_in(&url, true) {
                    paths.push(path);
                }
            }
            finish(paths);
            KEEP.with(|keep| keep.borrow_mut().take());
        }

        /// UIDocumentPickerDelegate: cancelled.
        #[unsafe(method(documentPickerWasCancelled:))]
        fn was_cancelled(&self, _picker: &AnyObject) {
            finish(Vec::new());
            KEEP.with(|keep| keep.borrow_mut().take());
        }
    }
);

impl FilesDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Photos: every video and picture, as they are (no re-encoding: an
/// iPhone's HEVC stays HEVC, which the editor reads).
fn show_photos(mtm: MainThreadMarker) -> Result<(), String> {
    unsafe {
        let config: Retained<AnyObject> = msg_send![class(c"PHPickerConfiguration")?, new];
        // 0: as many as wanted
        let _: () = msg_send![&*config, setSelectionLimit: 0isize];
        let filter = class(c"PHPickerFilter")?;
        let videos: Retained<AnyObject> = msg_send![filter, videosFilter];
        let images: Retained<AnyObject> = msg_send![filter, imagesFilter];
        let both = NSArray::from_retained_slice(&[videos, images]);
        let any: Retained<AnyObject> = msg_send![filter, anyFilterMatchingSubfilters: &*both];
        let _: () = msg_send![&*config, setFilter: &*any];
        // PHPickerConfigurationAssetRepresentationModeCurrent
        let _: () = msg_send![&*config, setPreferredAssetRepresentationMode: 1isize];
        let picker: Allocated<AnyObject> = msg_send![class(c"PHPickerViewController")?, alloc];
        let picker: Retained<AnyObject> = msg_send![picker, initWithConfiguration: &*config];
        let delegate = PhotosDelegate::new(mtm);
        let _: () = msg_send![&*picker, setDelegate: &*delegate];
        KEEP.with(|keep| *keep.borrow_mut() = Some(Retained::into_super(delegate)));
        present(&picker)
    }
}

/// Files: videos, pictures and sound, copied for the app (`asCopy`).
fn show_files(mtm: MainThreadMarker) -> Result<(), String> {
    unsafe {
        let ut = class(c"UTType")?;
        let mut types: Vec<Retained<AnyObject>> = Vec::new();
        for id in ["public.movie", "public.image", "public.audio"] {
            let ty: Option<Retained<AnyObject>> = msg_send![ut, typeWithIdentifier: &*ns(id)];
            types.extend(ty);
        }
        let types = NSArray::from_retained_slice(&types);
        let picker: Allocated<AnyObject> = msg_send![class(c"UIDocumentPickerViewController")?, alloc];
        let picker: Retained<AnyObject> = msg_send![picker, initForOpeningContentTypes: &*types, asCopy: true];
        let _: () = msg_send![&*picker, setAllowsMultipleSelection: true];
        let delegate = FilesDelegate::new(mtm);
        let _: () = msg_send![&*picker, setDelegate: &*delegate];
        KEEP.with(|keep| *keep.borrow_mut() = Some(Retained::into_super(delegate)));
        present(&picker)
    }
}

/// What a set of Photos results is still waiting for.
struct Collect {
    left: usize,
    paths: Vec<PathBuf>,
}

/// Each result's file, copied in as the system hands it over (on its own
/// queue; the file is gone once the handler returns).
fn photos_picked(results: &NSArray<AnyObject>) {
    let count = results.count();
    if count == 0 {
        finish(Vec::new());
        return;
    }
    let state = Arc::new(Mutex::new(Collect { left: count, paths: Vec::new() }));
    let done_one = |state: &Arc<Mutex<Collect>>, path: Option<PathBuf>| {
        let finished = {
            let Ok(mut s) = state.lock() else { return };
            s.paths.extend(path);
            s.left = s.left.saturating_sub(1);
            (s.left == 0).then(|| std::mem::take(&mut s.paths))
        };
        if let Some(paths) = finished {
            finish(paths);
        }
    };
    for result in results.iter() {
        let provider: Retained<AnyObject> = unsafe { msg_send![&*result, itemProvider] };
        let movie: bool = unsafe { msg_send![&*provider, hasItemConformingToTypeIdentifier: &*ns("public.movie")] };
        let image: bool = unsafe { msg_send![&*provider, hasItemConformingToTypeIdentifier: &*ns("public.image")] };
        let ty = if movie {
            "public.movie"
        } else if image {
            "public.image"
        } else {
            done_one(&state, None);
            continue;
        };
        let st = state.clone();
        let handler = RcBlock::new(move |url: *mut AnyObject, _error: *mut AnyObject| {
            // SAFETY: the system hands a valid NSURL or nil
            let path = unsafe { url.as_ref() }.and_then(|url| bring_in(url, false));
            if path.is_none() {
                log::warn!("a picked item could not be copied in");
            }
            done_one(&st, path);
        });
        let _progress: Option<Retained<AnyObject>> =
            unsafe { msg_send![&*provider, loadFileRepresentationForTypeIdentifier: &*ns(ty), completionHandler: &*handler] };
    }
}

/// The app's media folder: `Documents/TetalCut Media` (visible in the
/// Files app, under TetalCut).
fn media_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join("Documents").join("TetalCut Media");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// A free name in `dir` for `name`: `clip.mov`, `clip 2.mov`, …
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..10_000).map(|n| dir.join(format!("{stem} {n}{ext}"))).find(|c| !c.exists()).unwrap_or(first)
}

/// Copies (or, for a file that is already the app's own copy, moves) the
/// file at `url` into the media folder.
fn bring_in(url: &AnyObject, may_move: bool) -> Option<PathBuf> {
    let path: Option<Retained<NSString>> = unsafe { msg_send![url, path] };
    let src = PathBuf::from(path?.to_string());
    let name = src.file_name()?.to_string_lossy().into_owned();
    let dest = free_name(&media_dir()?, &name);
    if may_move && std::fs::rename(&src, &dest).is_ok() {
        return Some(dest);
    }
    std::fs::copy(&src, &dest).ok()?;
    Some(dest)
}

/// Saves a finished export to Photos and removes the app's copy. Called
/// off the main thread (it waits for the permission and the save).
pub fn publish(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("the export's path is not text")?;
    let url = NSURL::fileURLWithPath(&ns(text));
    let library = class(c"PHPhotoLibrary")?;
    // PHAccessLevelAddOnly
    let mut status: isize = unsafe { msg_send![library, authorizationStatusForAccessLevel: 1isize] };
    if status == 0 {
        // not asked yet: ask, and wait for the answer
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = RcBlock::new(move |answer: isize| {
            let _ = tx.send(answer);
        });
        let _: () = unsafe { msg_send![library, requestAuthorizationForAccessLevel: 1isize, handler: &*handler] };
        status = rx.recv_timeout(Duration::from_secs(600)).unwrap_or(0);
    }
    // PHAuthorizationStatusAuthorized 3, Limited 4
    if status != 3 && status != 4 {
        return Err("TetalCut no tiene permiso para guardar en Fotos: actívalo en Ajustes ▸ TetalCut ▸ Fotos".into());
    }
    let image = matches!(
        path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "heic" | "gif" | "webp")
    );
    let shared: Retained<AnyObject> = unsafe { msg_send![library, sharedPhotoLibrary] };
    let file = url.clone();
    let change = RcBlock::new(move || {
        let Ok(request) = class(c"PHAssetChangeRequest") else { return };
        let _: Option<Retained<AnyObject>> = unsafe {
            if image {
                msg_send![request, creationRequestForAssetFromImageAtFileURL: &*file]
            } else {
                msg_send![request, creationRequestForAssetFromVideoAtFileURL: &*file]
            }
        };
    });
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let saved: bool = unsafe { msg_send![&*shared, performChangesAndWait: &*change, error: &mut error] };
    if !saved {
        let why = unsafe { error.as_ref() }
            .map(|e| {
                let text: Option<Retained<NSString>> = unsafe { msg_send![e, localizedDescription] };
                text.map(|t| t.to_string()).unwrap_or_default()
            })
            .unwrap_or_default();
        return Err(format!("no se pudo guardar en Fotos: {why}"));
    }
    let _ = std::fs::remove_file(path);
    Ok(PHOTOS_FOLDER.into())
}
