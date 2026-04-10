mod config;
mod db;

use config::FileConfig;
use db::{ColumnFilter, ColumnInfo, DbState, QueryRequest, QueryResult, SchemaEntry, SqlResult, StrErr, TableInfo};
#[cfg(debug_assertions)]
use db::BenchmarkResult;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Set the main window title to "<filename> - dblitz v<version>" when a file is
/// open, or just "dblitz v<version>" when none is. Appends " DEV" in debug
/// builds. Multiple instances showing different files are then distinguishable
/// from the taskbar / Alt-Tab list.
fn update_window_title(app: &AppHandle, file: Option<&str>) {
    let version = app.package_info().version.to_string();
    let suffix = if cfg!(debug_assertions) { " DEV" } else { "" };
    let title = match file {
        Some(name) => format!("{name} - dblitz v{version}{suffix}"),
        None => format!("dblitz v{version}{suffix}"),
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&title);
    }
}

/// Extract the file name (with extension) from a full path, falling back to
/// the original path string if it has no parseable basename.
fn basename(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
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
        update_window_title(&app, Some(basename(&path)));
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
fn export_to_xlsx(app: tauri::AppHandle, headers: Vec<String>, rows: Vec<Vec<String>>) -> Result<String, String> {
    let path = db::export_to_xlsx(&headers, &rows)?;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    set_app_user_model_id();

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
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // args[0] is the exe, args[1..] may contain file paths
            if let Some(path) = args.get(1) {
                let _ = app.emit("open-file", path.clone());
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
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
