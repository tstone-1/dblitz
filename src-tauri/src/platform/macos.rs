//! macOS-only glue. The whole module is `cfg(target_os = "macos")`.

use tauri::AppHandle;
use tracing::warn;

/// macOS counterpart to the Windows [`add_to_recent_docs`]: register the opened
/// database with the OS so it appears in the Dock right-click menu and the app's
/// "Open Recent" menu, via `NSDocumentController::noteNewRecentDocumentURL:`.
///
/// `NSDocumentController` is main-thread-only, so the AppKit call is dispatched
/// onto the main thread through the Tauri app handle. Best-effort — any failure
/// (dispatch error, off-main-thread marker) is logged and swallowed, matching
/// the Windows path which also ignores errors.
pub(crate) fn add_to_recent_docs(app: &AppHandle, path: &str) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSDocumentController;
    use objc2_foundation::{NSString, NSURL};

    let path = path.to_string();
    let dispatched = app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            // run_on_main_thread guarantees the main thread; this should never fire.
            warn!("add_to_recent_docs: not on main thread, skipping");
            return;
        };
        let ns_path = NSString::from_str(&path);
        let url = NSURL::fileURLWithPath(&ns_path);
        let controller = NSDocumentController::sharedDocumentController(mtm);
        controller.noteNewRecentDocumentURL(&url);
    });
    if let Err(e) = dispatched {
        warn!(error = %e, "Failed to register recent document with the macOS Dock");
    }
}
