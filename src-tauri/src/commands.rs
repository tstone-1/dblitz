//! The `#[tauri::command]` functions the webview invokes. Each one picks the
//! SQLite or the Parquet backend and keeps no logic of its own beyond that.

use crate::config::{self, FileConfig};
use crate::db::{
    self, ColumnFilter, ColumnInfo, DbState, ErrCtx, QueryRequest, QueryResult, SchemaEntry,
    SqlResult, StrErr, TableInfo,
};
use crate::launch::{launch_argument, PendingOpen};
#[cfg(target_os = "macos")]
use crate::platform::macos::add_to_recent_docs;
#[cfg(windows)]
use crate::platform::windows::{add_to_recent_docs, set_window_db_marker};
use crate::pq;
use crate::updates::UpdateStatus;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// Set the main window title to `"<filename> - dblitz v<version>"` when a file
/// is open, or just `"dblitz v<version>"` when none is. Appends `" DEV"` in
/// debug builds. Uses the filename (not the full path) for a cleaner title bar
/// — the full path is shown in the toolbar instead.
///
/// Cosmetic only - split out from [`set_window_db_marker`] (the Win32 property
/// [`try_activate_existing`](crate::platform::windows::try_activate_existing) depends on for duplicate detection) so a
/// title-only refresh can never accidentally drop that marker. Callers that
/// change which file is open must call both.
pub(crate) fn update_window_title(app: &AppHandle, path: Option<&str>) {
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

#[tauri::command(async)]
pub(crate) fn close_database(app: AppHandle, state: State<'_, Arc<DbState>>) {
    db::close_database(&state);
    update_window_title(&app, None);
    #[cfg(windows)]
    set_window_db_marker(&app, None);
}

#[tauri::command]
pub(crate) fn cancel_queries(state: State<'_, Arc<DbState>>) {
    db::cancel_queries(&state);
}

/// Percent done of a view still being prepared, polled by Browse Data while a
/// page request is pending. Only a Parquet sort reports one; everything else
/// is `None`. Sync on purpose, like `cancel_queries`: it takes no connection
/// lock, so it answers while the query it describes holds one.
#[tauri::command]
pub(crate) fn view_progress(state: State<'_, Arc<DbState>>) -> Option<f64> {
    state.parquet_session().and_then(|pq| pq.view_progress())
}

#[tauri::command(async)]
pub(crate) fn open_database(
    app: AppHandle,
    state: State<'_, Arc<DbState>>,
    path: String,
) -> Result<Vec<TableInfo>, String> {
    // Parquet by content (`PAR1`), CSV and JSON by extension; see
    // `pq::FileFormat::of`. Everything else goes to SQLite, which reports a
    // non-database itself.
    let result = if pq::FileFormat::of(&path).is_some() {
        db::open_parquet(&state, &path)
    } else {
        db::open_database(&state, &path)
    }
    .err_ctx(&format!("opening {path}"));
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
pub(crate) fn get_recent_files() -> Vec<config::RecentFile> {
    config::get_recent_files_enriched()
}

#[tauri::command(async)]
pub(crate) fn clear_recent_files() -> Result<(), String> {
    config::clear_recent_files()
}

/// Returns the configured Excel-export folder, or `None` when exports go to the
/// OS temp directory (the default).
#[tauri::command(async)]
pub(crate) fn get_export_dir() -> Option<String> {
    config::get_export_dir()
}

/// Sets the Excel-export folder. Pass `null`/empty to reset to the temp-dir default.
#[tauri::command(async)]
pub(crate) fn set_export_dir(dir: Option<String>) -> Result<(), String> {
    config::set_export_dir(dir)
}

/// The database to open at startup: an OS document-open request that arrived
/// before the webview subscribed (see [`PendingOpen`]), else the launch
/// argument. The page calls this once on mount, *after* registering its
/// `open-file` listener — which is what makes the ready flag mean what it says.
#[tauri::command]
pub(crate) fn get_initial_file(pending: State<'_, PendingOpen>) -> Option<String> {
    pending.take_initial(launch_argument())
}

/// This launch's version transition plus whether this install can replace
/// itself. Resolved once during `setup` and handed out unchanged — see
/// [`crate::updates::UpdateStatus`] and the `setup` hook in `lib.rs` for why it cannot be
/// recomputed on demand.
#[tauri::command]
pub(crate) fn update_status(status: State<'_, UpdateStatus>) -> UpdateStatus {
    status.inner().clone()
}

/// Whether the automatic post-launch update check is enabled.
#[tauri::command(async)]
pub(crate) fn get_check_for_updates_on_startup() -> bool {
    config::get_check_for_updates_on_startup()
}

/// Persist the automatic-update-check opt-out.
#[tauri::command(async)]
pub(crate) fn set_check_for_updates_on_startup(enabled: bool) -> Result<(), String> {
    config::set_check_for_updates_on_startup(enabled)
}

/// Whether dblitz is in Explorer's "Open with" list for CSV and JSON files;
/// `None` where the setting does not exist (everywhere but Windows).
#[tauri::command(async)]
pub(crate) fn get_open_with_registration() -> Option<bool> {
    #[cfg(windows)]
    return Some(crate::platform::open_with::is_registered());
    #[cfg(not(windows))]
    None
}

/// Add dblitz to, or remove it from, "Open with" for CSV and JSON files.
#[tauri::command(async)]
pub(crate) fn set_open_with_registration(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    return crate::platform::open_with::set_registered(enabled);
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("\"Open with\" registration exists only on Windows".into())
    }
}

#[tauri::command(async)]
pub(crate) fn get_columns(
    state: State<'_, Arc<DbState>>,
    table: String,
) -> Result<Vec<ColumnInfo>, String> {
    match state.parquet_session() {
        Some(pq) => pq.get_columns(&table),
        None => db::get_columns(&state, &table),
    }
    .err_ctx(&format!("loading columns for table \"{table}\""))
}

#[tauri::command(async)]
pub(crate) fn get_schema(state: State<'_, Arc<DbState>>) -> Result<Vec<SchemaEntry>, String> {
    match state.parquet_session() {
        Some(pq) => Ok(pq.get_schema()),
        None => db::get_schema(&state),
    }
    .err_ctx("loading the database schema")
}

#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub(crate) fn query_table(
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
    match state.parquet_session() {
        Some(session) => pq::query_table(&session, &req, state.generation()),
        None => db::query_table(&state, &req),
    }
    .err_ctx(&format!("querying table \"{}\"", req.table))
}

#[tauri::command(async)]
pub(crate) fn count_rows(
    state: State<'_, Arc<DbState>>,
    table: String,
    filters: Vec<ColumnFilter>,
    global_filter: String,
) -> Result<i64, String> {
    match state.parquet_session() {
        Some(pq) => pq.count_rows(&table, &filters, &global_filter, state.generation()),
        None => db::count_rows(&state, &table, &filters, &global_filter),
    }
    .err_ctx(&format!("counting rows in table \"{table}\""))
}

#[tauri::command(async)]
pub(crate) fn execute_sql(state: State<'_, Arc<DbState>>, sql: String) -> SqlResult {
    match state.parquet_session() {
        Some(pq) => pq.execute_sql(&sql, state.generation()),
        None => db::execute_sql(&state, &sql),
    }
}

/// The SQL editor's live check: every gate `execute_sql` applies, without
/// running the statement. `None` means "no verdict right now" (nothing open, or
/// the SQL tab is busy) and the editor shows nothing rather than guessing.
#[tauri::command(async)]
pub(crate) fn validate_sql(state: State<'_, Arc<DbState>>, sql: String) -> Option<db::SqlCheck> {
    match state.parquet_session() {
        Some(pq) => pq.validate_sql(&sql),
        None => db::validate_sql(&state, &sql),
    }
}

/// Which SQL engine answers the SQL tab: `"duckdb"` for a Parquet file,
/// `"sqlite"` otherwise. The editor picks its function list and dialect by it.
#[tauri::command]
pub(crate) fn sql_engine(state: State<'_, Arc<DbState>>) -> &'static str {
    if state.parquet_session().is_some() {
        "duckdb"
    } else {
        "sqlite"
    }
}

#[tauri::command(async)]
pub(crate) fn export_to_xlsx(
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
#[tauri::command]
pub(crate) fn toggle_devtools(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_devtools_open() {
            window.close_devtools();
        } else {
            window.open_devtools();
        }
    }
}

#[tauri::command(async)]
pub(crate) fn load_view_config(state: State<'_, Arc<DbState>>) -> FileConfig {
    let path = state.current_path.lock();
    match path.as_ref() {
        Some(p) => config::load_config(p),
        None => FileConfig::default(),
    }
}

/// Persist a view config against the database the CALLER names, not against
/// whatever happens to be open when this runs.
///
/// `#[tauri::command(async)]` means this executes on Tauri's threadpool at a
/// time the frontend does not control, while `state.current_path` is mutable
/// and changes on every open. Selecting the destination from it made the
/// destination depend on scheduling: a save queued for database A that ran
/// after the user opened database B wrote A's settings under B's key, silently
/// replacing them. `path` closes that by carrying the answer with the request.
///
/// Deliberately does NOT also require `path` to still be the open database. A
/// save legitimately outlives its session - the user changes a filter and
/// immediately opens another file - and rejecting it there would throw away a
/// change they made while showing them a save-failed notice for doing nothing
/// wrong. Writing it to the file it belongs to is both lossless and correct.
///
/// That is safe because `path` is a lookup KEY, not a destination: the config
/// lands at `<app-config-dir>/<sha256(path)[..16]>.json` (see
/// `ConfigStore::config_path_for_db`), so no value of `path` can direct a write
/// outside dblitz's own config directory.
#[tauri::command(async)]
pub(crate) fn save_view_config(config: FileConfig, path: String) -> Result<(), String> {
    if path.is_empty() {
        return Err("Cannot save view config without a database path".to_string());
    }
    config::save_config(&path, &config).err_ctx(&format!("saving view config for {path}"))
}
