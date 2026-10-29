mod commands;
mod config;
/// Database access. Public only so the benchmark examples under
/// `src-tauri/examples/` can measure the shipped query paths instead of
/// re-implementing them - see [`db::bench_api`]. Everything the app itself uses
/// goes through the `#[tauri::command]` wrappers in `commands.rs`.
pub mod db;
mod launch;
mod platform;
mod pq;
mod updates;

use commands::update_window_title;
use db::DbState;
#[cfg(windows)]
use launch::launch_argument;
use launch::PendingOpen;
#[cfg(windows)]
use platform::windows::{set_app_user_model_id, try_activate_existing, windows_install_provenance};
use std::sync::Arc;
use tauri::Manager;
use tracing::warn;
use updates::{InstallProvenance, UpdateStatus};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    set_app_user_model_id();

    // If launched with a file already open in another instance, activate
    // that window instead of opening a duplicate.
    //
    // Absolutized first, and that is load-bearing rather than tidy: the marker
    // the running instance published was hashed from the absolutized path (the
    // one `get_initial_file` hands the webview), so hashing the raw `argv[1]`
    // compared two different spellings of the same file. `dblitz inventory.db`
    // from the directory holding it therefore never matched, and opened a
    // second window on a file already open - the exact case this check exists
    // for, and the one a user is most likely to reach from a terminal.
    #[cfg(windows)]
    if let Some(path) = launch_argument() {
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
            commands::close_database,
            commands::cancel_queries,
            commands::view_progress,
            commands::open_database,
            commands::get_columns,
            commands::get_schema,
            commands::query_table,
            commands::count_rows,
            commands::execute_sql,
            commands::validate_sql,
            commands::sql_engine,
            commands::export_to_xlsx,
            #[cfg(debug_assertions)]
            commands::toggle_devtools,
            commands::load_view_config,
            commands::save_view_config,
            commands::get_initial_file,
            commands::get_recent_files,
            commands::clear_recent_files,
            commands::get_export_dir,
            commands::set_export_dir,
            commands::update_status,
            commands::get_check_for_updates_on_startup,
            commands::set_check_for_updates_on_startup,
            commands::get_open_with_registration,
            commands::set_open_with_registration,
        ])
        .setup(|app| {
            update_window_title(app.handle(), None);
            // Sort caches and spill files a crashed Parquet session left behind.
            std::thread::spawn(pq::sweep_stale_cache);

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
            // Which artifact is this? `updates.rs` owns the policy; resolving
            // its input needs the OS, the environment and the registry, so it
            // happens here.
            // Each arm is annotated with the type on purpose. Without it the
            // Linux build never names `InstallProvenance` — both of the arms
            // that would are `cfg`-ed out — so the import at the top of this
            // file becomes an unused import, which `-D warnings` rejects. That
            // is invisible to a Windows-only local gate and only surfaces on
            // the Linux CI leg.
            #[cfg(target_os = "linux")]
            // AppImage exports this; a .deb/.rpm install does not, and the
            // Tauri updater cannot replace those in place.
            let provenance: InstallProvenance =
                updates::linux_provenance(std::env::var("APPIMAGE").ok().as_deref());
            #[cfg(windows)]
            let provenance: InstallProvenance = windows_install_provenance();
            #[cfg(target_os = "macos")]
            let provenance: InstallProvenance = InstallProvenance::MacOsBundle;

            app.manage(UpdateStatus::new(
                previous_version,
                current_version,
                provenance,
            ));
            // Skipped under WebDriver: tauri-driver exports TAURI_AUTOMATION
            // when it launches the app (that's how wry knows to put the
            // webview into automation mode), and the smoke test drives a
            // debug-profile build -- an inspector window popping up alongside
            // the automated one is at best noise and at worst steals the
            // session's window. The variable is already load-bearing for wry,
            // so keying on it adds no new configuration surface.
            #[cfg(debug_assertions)]
            if std::env::var_os("TAURI_AUTOMATION").is_none() {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
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
                launch::handle_open_request(_app_handle, url.as_str());
            }
        }
    });
}
