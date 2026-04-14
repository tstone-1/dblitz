mod config;
mod db;

use config::FileConfig;
use db::{ColumnFilter, ColumnInfo, DbState, QueryRequest, QueryResult, SchemaEntry, SqlResult, StrErr, TableInfo};
#[cfg(debug_assertions)]
use db::BenchmarkResult;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// Compute a 64-bit hash of the lowercased path for cross-process duplicate
/// detection via Win32 window properties.
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
/// Also sets a Win32 window property (`dblitz_db_path`) containing a hash of
/// the full path, used by [`try_activate_existing`] for duplicate detection.
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
        #[cfg(windows)]
        {
            use windows::core::w;
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::UI::WindowsAndMessaging::SetPropW;
            let hwnd = window.hwnd().expect("main window hwnd");
            let hash = path.map(path_hash).unwrap_or(0);
            unsafe {
                let _ = SetPropW(hwnd, w!("dblitz_db_path"), Some(HANDLE(hash as *mut _)));
            }
        }
    }
}

#[cfg(windows)]
fn set_app_user_model_id() {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::w;
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(w!("com.dblitz.app"));
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

#[tauri::command]
fn close_database(app: AppHandle, state: State<'_, Arc<DbState>>) {
    db::close_database(&state);
    update_window_title(&app, None);
}

#[tauri::command]
fn open_database(
    app: AppHandle,
    state: State<'_, Arc<DbState>>,
    path: String,
) -> Result<Vec<TableInfo>, String> {
    let result = db::open_database(&state, &path);
    if result.is_ok() {
        #[cfg(windows)]
        add_to_recent_docs(&path);
        config::push_recent_file(&path);
        update_window_title(&app, Some(&path));
    }
    result
}

#[tauri::command]
fn get_recent_files() -> Vec<String> {
    config::get_recent_files()
}

#[tauri::command]
fn clear_recent_files() -> Result<(), String> {
    config::clear_recent_files()
}

#[tauri::command]
fn get_initial_file() -> Option<String> {
    std::env::args().nth(1)
}

#[tauri::command]
fn get_tables(state: State<'_, Arc<DbState>>) -> Result<Vec<TableInfo>, String> {
    db::get_tables(&state)
}

#[tauri::command]
fn get_columns(state: State<'_, Arc<DbState>>, table: String) -> Result<Vec<ColumnInfo>, String> {
    db::get_columns(&state, &table)
}

#[tauri::command]
fn get_schema(state: State<'_, Arc<DbState>>) -> Result<Vec<SchemaEntry>, String> {
    db::get_schema(&state)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
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
        table, offset, limit, filters, global_filter, sort_column, sort_asc,
    };
    db::query_table(&state, &req)
}

#[tauri::command]
fn count_rows(
    state: State<'_, Arc<DbState>>,
    table: String,
    filters: Vec<ColumnFilter>,
    global_filter: String,
) -> Result<i64, String> {
    db::count_rows(&state, &table, &filters, &global_filter)
}

#[tauri::command]
fn execute_sql(state: State<'_, Arc<DbState>>, sql: String) -> SqlResult {
    db::execute_sql(&state, &sql)
}

#[tauri::command]
fn export_to_xlsx(
    app: tauri::AppHandle,
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    column_types: Option<Vec<String>>,
) -> Result<String, String> {
    let types = column_types.unwrap_or_default();
    let path = db::export_to_xlsx(&headers, &rows, &types)?;
    // Open with default application via opener plugin (safe, cross-platform)
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(&path, None::<&str>).str_err()?;
    Ok(path)
}

#[cfg(debug_assertions)]
#[tauri::command]
fn benchmark_query(
    state: State<'_, Arc<DbState>>,
    table: String,
    chunk_size: i64,
) -> Result<Vec<BenchmarkResult>, String> {
    db::benchmark_query(&state, &table, chunk_size)
}

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

#[tauri::command]
fn load_view_config(state: State<'_, Arc<DbState>>) -> FileConfig {
    let path = state.current_path.lock();
    match path.as_ref() {
        Some(p) => config::load_config(p),
        None => FileConfig::default(),
    }
}

#[tauri::command]
fn save_view_config(
    state: State<'_, Arc<DbState>>,
    config: FileConfig,
) -> Result<(), String> {
    let path = state.current_path.lock();
    match path.as_ref() {
        Some(p) => config::save_config(p, &config),
        None => Err("No database open".to_string()),
    }
}

/// Search for an existing dblitz window that has the same file open by
/// comparing the `dblitz_db_path` window property (a 64-bit hash of the
/// full, lowercased path set by [`update_window_title`]).
///
/// If found, restore (un-minimise) and activate it, returning `true` so
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
        if !ctx.found.0.is_null() {
            if IsIconic(ctx.found).as_bool() {
                let _ = ShowWindow(ctx.found, SW_RESTORE);
            }
            let _ = SetForegroundWindow(ctx.found);
            eprintln!("dblitz: activated existing window for {path}");
            return true;
        }
    }
    false
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

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(db_state)
        .invoke_handler(tauri::generate_handler![
            close_database,
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
            toggle_devtools,
            get_current_path,
            load_view_config,
            save_view_config,
            get_initial_file,
            get_recent_files,
            clear_recent_files,
        ])
        .setup(|app| {
            update_window_title(app.handle(), None);
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
