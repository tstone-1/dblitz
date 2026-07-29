mod config;
mod db;
mod updates;

use config::FileConfig;
#[cfg(debug_assertions)]
use db::BenchmarkResult;
use db::{
    ColumnFilter, ColumnInfo, DbState, ErrCtx, QueryRequest, QueryResult, SchemaEntry, SqlResult,
    StrErr, TableInfo,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tracing::warn;
use updates::UpdateStatus;

/// Compute a 64-bit hash of the lowercased path for cross-process duplicate
/// detection via Win32 window properties.
///
/// `DefaultHasher::new` is seeded with fixed keys (unlike `RandomState`), so the
/// value is stable across processes — which is the whole point, since one
/// instance writes it and another reads it. Rust does not promise the algorithm
/// is stable across *compiler* versions, so two dblitz builds from different
/// toolchains may not recognise each other; that degrades to opening a second
/// window, never to opening the wrong one.
#[cfg(windows)]
fn path_hash(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_ascii_lowercase().hash(&mut hasher);
    hasher.finish()
}

/// Set the main window title to `"<filename> - dblitz v<version>"` when a file
/// is open, or just `"dblitz v<version>"` when none is. Appends `" DEV"` in
/// debug builds. Uses the filename (not the full path) for a cleaner title bar
/// — the full path is shown in the toolbar instead.
///
/// Cosmetic only - split out from [`set_window_db_marker`] (the Win32 property
/// [`try_activate_existing`] depends on for duplicate detection) so a
/// title-only refresh can never accidentally drop that marker. Callers that
/// change which file is open must call both.
fn update_window_title(app: &AppHandle, path: Option<&str>) {
    let version = app.package_info().version.to_string();
    let suffix = if cfg!(debug_assertions) { " DEV" } else { "" };
    let title = match path {
        Some(p) => {
            let name = std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(p);
            format!("{name} - dblitz v{version}{suffix}")
        }
        None => format!("dblitz v{version}{suffix}"),
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&title);
    }
}

/// Set the Win32 window property (`dblitz_db_path`) containing a hash of the
/// full path, used by [`try_activate_existing`] for duplicate detection.
/// Kept separate from [`update_window_title`]: the title is purely cosmetic,
/// but this marker is load-bearing state another process reads to find this
/// window, so callers that change which file is open must always set both
/// together rather than risk a title-only path leaving it stale.
#[cfg(windows)]
fn set_window_db_marker(app: &AppHandle, path: Option<&str>) {
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(hwnd) = window.hwnd() {
            use windows::core::w;
            use windows::Win32::Foundation::{HANDLE, HWND};
            use windows::Win32::UI::WindowsAndMessaging::SetPropW;
            // Tauri's windows-rs is pinned a version behind ours, so its
            // HWND type is structurally identical but type-distinct. Both
            // versions define HWND as `struct HWND(pub *mut c_void)`, so a
            // pointer-equivalent rewrap is sound.
            let hwnd = HWND(hwnd.0);
            let hash = path.map(path_hash).unwrap_or(0);
            unsafe {
                let _ = SetPropW(hwnd, w!("dblitz_db_path"), Some(HANDLE(hash as *mut _)));
            }
        }
    }
}

#[cfg(windows)]
fn set_app_user_model_id() {
    use windows::core::w;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("com.tstone.dblitz"));
    }
}

#[cfg(windows)]
fn add_to_recent_docs(path: &str) {
    use windows::Win32::UI::Shell::{SHAddToRecentDocs, SHARD_PATHW};
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(wide.as_ptr() as *const _));
    }
}

/// macOS counterpart to the Windows [`add_to_recent_docs`]: register the opened
/// database with the OS so it appears in the Dock right-click menu and the app's
/// "Open Recent" menu, via `NSDocumentController::noteNewRecentDocumentURL:`.
///
/// `NSDocumentController` is main-thread-only, so the AppKit call is dispatched
/// onto the main thread through the Tauri app handle. Best-effort — any failure
/// (dispatch error, off-main-thread marker) is logged and swallowed, matching
/// the Windows path which also ignores errors.
#[cfg(target_os = "macos")]
fn add_to_recent_docs(app: &AppHandle, path: &str) {
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

#[tauri::command(async)]
fn close_database(app: AppHandle, state: State<'_, Arc<DbState>>) {
    db::close_database(&state);
    update_window_title(&app, None);
    #[cfg(windows)]
    set_window_db_marker(&app, None);
}

#[tauri::command]
fn cancel_queries(state: State<'_, Arc<DbState>>) {
    db::cancel_queries(&state);
}

#[tauri::command(async)]
fn open_database(
    app: AppHandle,
    state: State<'_, Arc<DbState>>,
    path: String,
) -> Result<Vec<TableInfo>, String> {
    let result = db::open_database(&state, &path).err_ctx(&format!("opening {path}"));
    if result.is_ok() {
        #[cfg(windows)]
        add_to_recent_docs(&path);
        #[cfg(target_os = "macos")]
        add_to_recent_docs(&app, &path);
        config::push_recent_file(&path);
        update_window_title(&app, Some(&path));
        #[cfg(windows)]
        set_window_db_marker(&app, Some(&path));
    }
    result
}

/// Returns the recent-files list with each entry enriched by its per-DB
/// window marker (tint + label). Return shape changed in 26.4.6+ from
/// `Vec<String>` to `Vec<RecentFile>`; callers must deserialize as objects.
#[tauri::command(async)]
fn get_recent_files() -> Vec<config::RecentFile> {
    config::get_recent_files_enriched()
}

#[tauri::command(async)]
fn clear_recent_files() -> Result<(), String> {
    config::clear_recent_files()
}

/// Returns the configured Excel-export folder, or `None` when exports go to the
/// OS temp directory (the default).
#[tauri::command(async)]
fn get_export_dir() -> Option<String> {
    config::get_export_dir()
}

/// Sets the Excel-export folder. Pass `null`/empty to reset to the temp-dir default.
#[tauri::command(async)]
fn set_export_dir(dir: Option<String>) -> Result<(), String> {
    config::set_export_dir(dir)
}

/// Slot for a database path the OS asked us to open before the webview was in
/// any position to hear about it.
///
/// macOS never puts a document open in argv: Finder double-clicks, Dock "Open
/// Recent" entries and `open -a dblitz file.db` all arrive as Apple events,
/// surfaced by Tauri as `RunEvent::Opened`. Those can land at any point in the
/// process lifetime, including before the webview has loaded — and an emitted
/// event with no listener is not queued, it is dropped, which reads to the user
/// as "double-clicking a database opens an empty dblitz".
///
/// So the handler picks exactly one of two delivery routes, and `webview_ready`
/// decides which:
///   - not ready yet: stash the path here; [`get_initial_file`] hands it over
///     when the page mounts.
///   - ready: emit `open-file`, which the page is listening for by then.
///
/// Choosing one route rather than doing both is what stops a path that arrives
/// mid-mount from being opened twice.
#[derive(Default)]
struct PendingOpen(parking_lot::Mutex<PendingOpenState>);

#[derive(Default)]
struct PendingOpenState {
    path: Option<String>,
    /// Set by [`get_initial_file`], which the page calls *after* registering its
    /// `open-file` listener — so this is a proxy for "an emit would be heard".
    webview_ready: bool,
}

impl PendingOpen {
    /// Record an OS open request. Returns `Some(path)` when the caller should
    /// emit it to the webview, `None` when it has been stashed instead.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn deliver(&self, path: String) -> Option<String> {
        let mut state = self.0.lock();
        if state.webview_ready {
            Some(path)
        } else {
            state.path = Some(path);
            None
        }
    }

    /// Drain the stash and mark the webview ready, falling back to the launch
    /// argument. `argv_path` is passed in rather than read here so the decision
    /// is testable without touching the process environment.
    fn take_initial(&self, argv_path: Option<String>) -> Option<String> {
        let mut state = self.0.lock();
        state.webview_ready = true;
        state.path.take().or(argv_path)
    }
}

/// Convert a `file://` URL as delivered by the OS into a plain filesystem path.
///
/// `RunEvent::Opened` carries URLs, not paths, so a database in
/// `~/My Databases/` arrives as `file:///Users/x/My%20Databases/db.sqlite`.
/// Handing that string to SQLite would look for a file whose name literally
/// contains "%20". Returns `None` for anything that is not a local file URL (a
/// custom scheme, a remote authority) or that is malformed, so the caller can
/// log and skip instead of opening something unintended.
///
/// Compiled on every platform even though only the macOS arm of the run-event
/// handler calls it: that keeps it under `cargo test` on the machines this is
/// actually developed on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn file_url_to_path(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    // "file:///path" leaves the authority empty; "file://localhost/path" is the
    // one non-empty authority RFC 8089 still defines as meaning this machine.
    // Anything else names a remote host and is not ours to open.
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    if !(authority.is_empty() || authority.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    // An unencoded '?' or '#' is a URL delimiter, not part of the path — a real
    // one in a filename would have arrived percent-encoded.
    let path = path.split(['?', '#']).next()?;
    let decoded = percent_decode(path)?;
    // An embedded NUL would be silently truncated by the C APIs downstream,
    // turning "open A\0B" into "open A".
    if decoded.is_empty() || decoded.contains('\0') {
        return None;
    }
    Some(decoded)
}

/// Percent-decode a URL path. `None` on a malformed escape or on bytes that do
/// not form valid UTF-8 — both mean we cannot tell which file was meant, which
/// is worth reporting rather than guessing at.
///
/// Deliberately does *not* treat '+' as a space: that is form encoding, and a
/// filename may legitimately contain a plus.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn percent_decode(input: &str) -> Option<String> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Route one OS document-open request to whichever half of [`PendingOpen`] can
/// actually reach the user: emit it if the webview is listening, stash it if it
/// is not. A URL that is not a local file is logged and dropped.
///
/// Split out of the `RunEvent::Opened` arm rather than written inline so that it
/// compiles on Windows and Linux too: the `cfg` gate up there is unavoidable
/// (Tauri gates the variant itself), and everything left inside it is one
/// destructuring line. This is the part with types to get wrong, and it is
/// type-checked by every platform's build instead of only by the one nobody
/// develops on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn handle_open_request(app: &AppHandle, url: &str) {
    use tauri::Emitter;
    let Some(path) = file_url_to_path(url) else {
        warn!(url, "Ignoring an open request that is not a local file URL");
        return;
    };
    let Some(path) = app.state::<PendingOpen>().deliver(path) else {
        return;
    };
    if let Err(e) = app.emit("open-file", path) {
        warn!(error = %e, "Failed to hand an open request to the webview");
    }
}

/// The database to open at startup: an OS document-open request that arrived
/// before the webview subscribed (see [`PendingOpen`]), else the launch
/// argument. The page calls this once on mount, *after* registering its
/// `open-file` listener — which is what makes the ready flag mean what it says.
#[tauri::command]
fn get_initial_file(pending: State<'_, PendingOpen>) -> Option<String> {
    pending.take_initial(std::env::args().nth(1))
}

/// This launch's version transition plus whether this install can replace
/// itself. Resolved once during `setup` and handed out unchanged — see
/// [`updates::UpdateStatus`] and the `setup` hook below for why it cannot be
/// recomputed on demand.
#[tauri::command]
fn update_status(status: State<'_, UpdateStatus>) -> UpdateStatus {
    status.inner().clone()
}

/// Whether the automatic post-launch update check is enabled.
#[tauri::command(async)]
fn get_check_for_updates_on_startup() -> bool {
    config::get_check_for_updates_on_startup()
}

/// Persist the automatic-update-check opt-out.
#[tauri::command(async)]
fn set_check_for_updates_on_startup(enabled: bool) -> Result<(), String> {
    config::set_check_for_updates_on_startup(enabled)
}

#[tauri::command(async)]
fn get_tables(state: State<'_, Arc<DbState>>) -> Result<Vec<TableInfo>, String> {
    db::get_tables(&state).err_ctx("loading the table list")
}

#[tauri::command(async)]
fn get_columns(state: State<'_, Arc<DbState>>, table: String) -> Result<Vec<ColumnInfo>, String> {
    db::get_columns(&state, &table).err_ctx(&format!("loading columns for table \"{table}\""))
}

#[tauri::command(async)]
fn get_schema(state: State<'_, Arc<DbState>>) -> Result<Vec<SchemaEntry>, String> {
    db::get_schema(&state).err_ctx("loading the database schema")
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
fn query_table(
    state: State<'_, Arc<DbState>>,
    table: String,
    offset: i64,
    limit: i64,
    filters: Vec<ColumnFilter>,
    global_filter: String,
    sort_column: Option<String>,
    sort_asc: bool,
) -> Result<QueryResult, String> {
    let req = QueryRequest {
        table,
        offset,
        limit,
        filters,
        global_filter,
        sort_column,
        sort_asc,
    };
    db::query_table(&state, &req).err_ctx(&format!("querying table \"{}\"", req.table))
}

#[tauri::command(async)]
fn count_rows(
    state: State<'_, Arc<DbState>>,
    table: String,
    filters: Vec<ColumnFilter>,
    global_filter: String,
) -> Result<i64, String> {
    db::count_rows(&state, &table, &filters, &global_filter)
        .err_ctx(&format!("counting rows in table \"{table}\""))
}

#[tauri::command(async)]
fn execute_sql(state: State<'_, Arc<DbState>>, sql: String) -> SqlResult {
    db::execute_sql(&state, &sql)
}

#[tauri::command(async)]
fn export_to_xlsx(
    app: tauri::AppHandle,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    column_types: Option<Vec<String>>,
) -> Result<String, String> {
    let types = column_types.unwrap_or_default();
    let dest_dir = config::resolve_export_dir();
    let path =
        db::export_to_xlsx(&headers, &rows, &types, &dest_dir).err_ctx("exporting to Excel")?;
    // Open with default application via opener plugin (safe, cross-platform)
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(&path, None::<&str>)
        .str_err()
        .err_ctx(&format!("opening the exported file {path}"))?;
    Ok(path)
}

#[cfg(debug_assertions)]
#[tauri::command(async)]
fn benchmark_query(
    state: State<'_, Arc<DbState>>,
    table: String,
    chunk_size: i64,
) -> Result<Vec<BenchmarkResult>, String> {
    db::benchmark_query(&state, &table, chunk_size)
}

#[cfg(debug_assertions)]
#[tauri::command]
fn toggle_devtools(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_devtools_open() {
            window.close_devtools();
        } else {
            window.open_devtools();
        }
    }
}

#[tauri::command]
fn get_current_path(state: State<'_, Arc<DbState>>) -> Option<String> {
    state.current_path.lock().clone()
}

#[tauri::command(async)]
fn load_view_config(state: State<'_, Arc<DbState>>) -> FileConfig {
    let path = state.current_path.lock();
    match path.as_ref() {
        Some(p) => config::load_config(p),
        None => FileConfig::default(),
    }
}

#[tauri::command(async)]
fn save_view_config(state: State<'_, Arc<DbState>>, config: FileConfig) -> Result<(), String> {
    let path = state.current_path.lock();
    match path.as_ref() {
        Some(p) => config::save_config(p, &config).err_ctx(&format!("saving view config for {p}")),
        None => Err("No database open".to_string()),
    }
}

/// Search for an existing dblitz window that has the same file open by
/// comparing the `dblitz_db_path` window property (a 64-bit hash of the
/// full, lowercased path set by [`set_window_db_marker`]).
///
/// If found, restore (un-minimise) and surface it, returning `true` so
/// the caller can exit early.
///
/// There is a narrow race between when an instance launches and when it
/// finishes loading its database (at which point the property is set). If the
/// same file is double-clicked twice within milliseconds the second instance
/// may not find the first. Acceptable in practice.
#[cfg(windows)]
fn try_activate_existing(path: &str) -> bool {
    use windows::core::{w, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Ctx {
        target_hash: u64,
        found: HWND,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        let prop = GetPropW(hwnd, w!("dblitz_db_path"));
        if !prop.is_invalid() && prop.0 as u64 == ctx.target_hash {
            ctx.found = hwnd;
            return BOOL(0); // stop enumerating
        }
        BOOL(1)
    }

    let mut ctx = Ctx {
        target_hash: path_hash(path),
        found: HWND::default(),
    };

    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
        if ctx.found.0.is_null() {
            return false;
        }
        let hwnd = ctx.found;
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
        // `SetForegroundWindow` is a request, not a command: Windows' foreground
        // lock denies it whenever the caller doesn't already own the foreground,
        // which is the normal case here — we were launched by Explorer, or a
        // full-screen app has focus. Measured on this machine with a full-screen
        // game focused: the call returned and the foreground never moved, so the
        // second launch exited 0 with no window, no message, nothing. That reads
        // as "dblitz refuses to open this file".
        //
        // So always follow up with a flash. It is deliberately unconditional:
        // there is no reliable way to ask whether the activation took.
        // `SetForegroundWindow`'s return value can be nonzero while it only
        // flashed the taskbar, and testing `GetForegroundWindow() != hwnd` right
        // after is racy in the other direction — measured reporting the *old*
        // foreground immediately after an activation that did succeed, which
        // would make the failure branch fire on the happy path. FLASHW_TIMERNOFG
        // makes the unconditional call self-correcting: it flashes only until
        // the window reaches the foreground, so a successful activation stops it
        // on its own and a denied one keeps flashing until the user looks.
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        let _ = FlashWindowEx(&info);
        eprintln!("dblitz: {path} is already open; raised the existing window");
        true
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    set_app_user_model_id();

    // If launched with a file already open in another instance, activate
    // that window instead of opening a duplicate.
    #[cfg(windows)]
    if let Some(path) = std::env::args().nth(1) {
        if try_activate_existing(&path) {
            return;
        }
    }

    let db_state = Arc::new(DbState::new());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("dblitz_lib=info".parse().unwrap()),
        )
        .try_init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // The updater does its HTTP work in Rust (reqwest), not the webview, so
        // the `connect-src` CSP in tauri.conf.json deliberately says nothing
        // about GitHub — adding it there would be cargo-culting.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(db_state)
        .manage(PendingOpen::default())
        .invoke_handler(tauri::generate_handler![
            close_database,
            cancel_queries,
            open_database,
            get_tables,
            get_columns,
            get_schema,
            query_table,
            count_rows,
            execute_sql,
            export_to_xlsx,
            #[cfg(debug_assertions)]
            benchmark_query,
            #[cfg(debug_assertions)]
            toggle_devtools,
            get_current_path,
            load_view_config,
            save_view_config,
            get_initial_file,
            get_recent_files,
            clear_recent_files,
            get_export_dir,
            set_export_dir,
            update_status,
            get_check_for_updates_on_startup,
            set_check_for_updates_on_startup,
        ])
        .setup(|app| {
            update_window_title(app.handle(), None);

            // Resolve the version transition once, here, and stash it as app
            // state. `record_run_version` is destructive by design: it returns
            // the previous version and immediately overwrites it, so the
            // "did we just update?" answer only exists at this moment.
            let current_version = app.package_info().version.to_string();
            let previous_version =
                config::record_run_version(&current_version).unwrap_or_else(|e| {
                    // A failed write is not worth blocking startup over. The
                    // only consequence is the "updated to vX" notice appearing
                    // again on the next launch.
                    warn!(error = %e, "Failed to record run version");
                    None
                });
            app.manage(UpdateStatus::new(
                previous_version,
                current_version,
                cfg!(target_os = "linux"),
                // AppImage exports this; a .deb/.rpm install does not, and the
                // Tauri updater cannot replace those in place.
                std::env::var("APPIMAGE").ok().as_deref(),
            ));
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // `build` + `run(callback)` rather than `run(context)` purely so there is a
    // run-event callback to hang the macOS document-open handling off; the two
    // are otherwise the same call.
    app.run(|_app_handle, _event| {
        // macOS delivers document opens as Apple events, never as argv: Finder
        // double-clicks, the Dock's "Open Recent" list (which
        // `add_to_recent_docs` populates) and `open -a dblitz file.db` all land
        // here. Without this arm every one of those launches shows an empty
        // window and no error at all. Tauri cfg-gates the `Opened` variant
        // itself to macOS/iOS, so the match arm has to be gated too or the
        // Windows and Linux builds stop compiling.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &_event {
            for url in urls {
                handle_open_request(_app_handle, url.as_str());
            }
        }
    });
}

/// Everything about the OS "open this document" path that can be checked
/// without an OS: the URL-to-path conversion and the two-route delivery
/// decision. The event plumbing itself (`RunEvent::Opened` firing at all, the
/// webview receiving `open-file`) is macOS-only and is NOT covered here — see
/// the comments in [`PendingOpen`] and the run-event handler.
#[cfg(test)]
mod open_request_tests {
    use super::{file_url_to_path, PendingOpen};

    #[test]
    fn file_url_percent_escapes_are_decoded() {
        // The case that actually bites: a space in a folder name. Left encoded,
        // SQLite looks for a file whose name contains a literal "%20".
        assert_eq!(
            file_url_to_path("file:///Users/alice/My%20Databases/db.sqlite").as_deref(),
            Some("/Users/alice/My Databases/db.sqlite")
        );
        // Multi-byte UTF-8 arrives as one escape per byte.
        assert_eq!(
            file_url_to_path("file:///Users/alice/Gr%C3%B6%C3%9Fe.sqlite").as_deref(),
            Some("/Users/alice/Größe.sqlite")
        );
        // Uppercase hex digits are equally legal.
        assert_eq!(
            file_url_to_path("file:///tmp/a%2Db.sqlite").as_deref(),
            Some("/tmp/a-b.sqlite")
        );
    }

    #[test]
    fn file_url_without_escapes_passes_through() {
        assert_eq!(
            file_url_to_path("file:///Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
    }

    #[test]
    fn plus_is_not_decoded_as_a_space() {
        // '+' means space in form encoding only. A filename may contain one,
        // and rewriting it produces a path that does not exist.
        assert_eq!(
            file_url_to_path("file:///tmp/c++notes.sqlite").as_deref(),
            Some("/tmp/c++notes.sqlite")
        );
    }

    #[test]
    fn localhost_authority_is_local_but_a_remote_host_is_not() {
        assert_eq!(
            file_url_to_path("file://localhost/Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
        assert_eq!(file_url_to_path("file://server/share/db.sqlite"), None);
    }

    #[test]
    fn non_file_urls_are_rejected() {
        // The handler skips these rather than opening something unintended;
        // returning a path here would mean handing SQLite an https URL.
        assert_eq!(file_url_to_path("https://example.com/db.sqlite"), None);
        assert_eq!(file_url_to_path("dblitz://open/db.sqlite"), None);
        assert_eq!(file_url_to_path("/Users/alice/db.sqlite"), None);
    }

    #[test]
    fn scheme_matching_is_case_insensitive() {
        assert_eq!(
            file_url_to_path("FILE:///Users/alice/db.sqlite").as_deref(),
            Some("/Users/alice/db.sqlite")
        );
    }

    #[test]
    fn malformed_or_unopenable_urls_are_rejected() {
        // A truncated escape and a non-hex escape: we cannot tell what file was
        // meant, so say so instead of guessing.
        assert_eq!(file_url_to_path("file:///tmp/a%2"), None);
        assert_eq!(file_url_to_path("file:///tmp/a%zz.sqlite"), None);
        // Invalid UTF-8 (a lone continuation byte).
        assert_eq!(file_url_to_path("file:///tmp/%FF.sqlite"), None);
        // An embedded NUL would be truncated silently downstream.
        assert_eq!(file_url_to_path("file:///tmp/a%00b.sqlite"), None);
        // No path component at all.
        assert_eq!(file_url_to_path("file://localhost"), None);
    }

    #[test]
    fn query_and_fragment_are_not_part_of_the_path() {
        assert_eq!(
            file_url_to_path("file:///tmp/db.sqlite?x=1").as_deref(),
            Some("/tmp/db.sqlite")
        );
        assert_eq!(
            file_url_to_path("file:///tmp/db.sqlite#frag").as_deref(),
            Some("/tmp/db.sqlite")
        );
    }

    #[test]
    fn an_open_before_the_webview_is_ready_is_stashed_not_emitted() {
        // The cold-launch case. Emitting here would reach nobody and the launch
        // would show an empty window.
        let pending = PendingOpen::default();
        assert_eq!(pending.deliver("/tmp/db.sqlite".to_string()), None);
        assert_eq!(
            pending.take_initial(None).as_deref(),
            Some("/tmp/db.sqlite")
        );
    }

    #[test]
    fn an_open_after_the_webview_is_ready_is_emitted_not_stashed() {
        // The app-already-running case: the page is listening, so the path goes
        // out as an event. It must NOT also be stashed - a later
        // `get_initial_file` would then open the same file a second time.
        let pending = PendingOpen::default();
        pending.take_initial(None);
        assert_eq!(
            pending.deliver("/tmp/db.sqlite".to_string()),
            Some("/tmp/db.sqlite".to_string())
        );
        assert_eq!(pending.take_initial(None), None);
    }

    #[test]
    fn a_stashed_path_wins_over_the_launch_argument() {
        // Both can be present on a cold launch. The OS event names the document
        // the user actually double-clicked; argv on macOS is the bundle's own
        // arguments and can be stale or irrelevant.
        let pending = PendingOpen::default();
        pending.deliver("/tmp/opened.sqlite".to_string());
        assert_eq!(
            pending
                .take_initial(Some("/tmp/argv.sqlite".to_string()))
                .as_deref(),
            Some("/tmp/opened.sqlite")
        );
    }

    #[test]
    fn the_launch_argument_is_used_when_nothing_was_stashed() {
        // Windows and Linux only ever take this route.
        let pending = PendingOpen::default();
        assert_eq!(
            pending
                .take_initial(Some("/tmp/argv.sqlite".to_string()))
                .as_deref(),
            Some("/tmp/argv.sqlite")
        );
        assert_eq!(pending.take_initial(None), None);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::path_hash;

    /// Explorer, the recent-files list and the file dialog can all hand back
    /// the same file with different casing, and NTFS treats those as one file.
    /// If the hash disagreed, double-clicking a database already open under a
    /// different casing would open a second window onto the same snapshot.
    #[test]
    fn path_hash_ignores_case() {
        assert_eq!(
            path_hash(r"C:\Users\alice\Data\inventory.sqlite"),
            path_hash(r"c:\users\ALICE\data\INVENTORY.SQLITE")
        );
    }

    /// The marker is keyed on the *full* path, deliberately: same-named
    /// databases in different directories are different files and must each get
    /// their own window. Hashing only the filename would collapse them.
    #[test]
    fn path_hash_distinguishes_directories() {
        assert_ne!(
            path_hash(r"C:\a\inventory.sqlite"),
            path_hash(r"C:\b\inventory.sqlite")
        );
    }

    /// Two different files must not collide into "already open" — that would
    /// make the second one silently un-openable.
    #[test]
    fn path_hash_distinguishes_filenames() {
        assert_ne!(path_hash(r"C:\a\one.sqlite"), path_hash(r"C:\a\two.sqlite"));
    }

    /// A `HANDLE` of 0 is what [`set_window_db_marker`] writes for "no file
    /// open", and `GetPropW` returns 0 for a window that has no such property
    /// at all. A path hashing to 0 would therefore match every non-dblitz
    /// window on the desktop. Not provable in general, but pin the paths this
    /// app actually sees so a hash swap that lands on 0 for a plausible input
    /// is caught here rather than in the wild.
    #[test]
    fn path_hash_is_nonzero_for_real_paths() {
        for p in [
            r"C:\db.sqlite",
            r"C:\Users\alice\Data\inventory.sqlite",
            r"\\server\share\db.sqlite",
            "/Users/alice/db.sqlite",
        ] {
            assert_ne!(path_hash(p), 0, "{p} hashed to the 'no file open' marker");
        }
    }
}
