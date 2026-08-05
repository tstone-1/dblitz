use crate::db::StrErr;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Maximum number of recently-opened databases tracked in app config.
const RECENT_FILES_MAX: usize = 10;

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct PinnedFilter {
    pub value: String,
    pub is_regex: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ViewConfig {
    pub hidden_columns: Vec<String>,
    pub column_colors: HashMap<String, String>,
    pub sort_column: Option<String>,
    pub sort_asc: bool,
    #[serde(default)]
    pub column_order: Vec<String>,
    #[serde(default)]
    pub pinned_filters: HashMap<String, PinnedFilter>,
    #[serde(default)]
    pub pinned_global_filter: Option<String>,
    #[serde(default)]
    pub column_widths: HashMap<String, u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct FileConfig {
    /// Per-table view configs, keyed by table name
    pub tables: HashMap<String, ViewConfig>,
    /// Optional CSS color string used to tint the toolbar so multiple
    /// open windows are visually distinguishable (e.g. PROD vs QA).
    #[serde(default)]
    pub tint: Option<String>,
    /// Optional short label shown as a pill next to the file path.
    #[serde(default)]
    pub label: Option<String>,
}

pub const TINT_PRESETS: &[&str] = &[
    "#d94040", "#e0a030", "#4aa84a", "#3080d0", "#8050c0", "#c04090",
];

/// Column-tint colors offered by the frontend's header-context-menu color
/// picker (`colorPresetsForTheme` in `src/lib/components/columnView.ts`).
/// That function returns a different palette per theme (plus a `""` "no
/// color" entry that the frontend deletes the map key for instead of
/// storing — see `setColumnColor` in BrowseData.svelte), so this allowlist
/// is the union of both the light- and dark-theme palettes: a config saved
/// while the app was in one theme must still validate after the user
/// switches to the other.
pub const COLUMN_COLOR_PRESETS: &[&str] = &[
    // Light theme
    "#fde8e8", "#e8fde8", "#e8e8fd", "#fdfde8", "#fde8fd", "#e8fdfd", "#f5eded", "#edf5ed",
    // Dark theme
    "#3b1c1c", "#1c3b1c", "#1c1c3b", "#3b3b1c", "#3b1c3b", "#1c3b3b", "#2d1f1f", "#1f2d1f",
];

/// Defensive backstop on the window-marker label length. The frontend input
/// already caps entry at `maxlength="12"` (see Toolbar.svelte), but a
/// hand-edited `config.json` bypasses that, and the label is rendered inline
/// in the toolbar where an unbounded string would visually break the layout.
/// Deliberately more generous than the UI's 12 chars -- this is a safety net
/// against corrupt/adversarial input, not a UX constraint.
const LABEL_MAX_LEN: usize = 64;

fn sanitize_tint(tint: Option<String>) -> Option<String> {
    tint.filter(|value| TINT_PRESETS.contains(&value.as_str()))
}

/// Drops any per-column tint that isn't one of the frontend's known preset
/// values, mirroring `sanitize_tint` above. Applied on both load and save
/// (via `sanitize_file_config`) so a hand-edited or otherwise adversarial
/// `column_colors` map never reaches the frontend, which uses these values
/// directly in inline `style` attributes.
fn sanitize_column_colors(colors: HashMap<String, String>) -> HashMap<String, String> {
    colors
        .into_iter()
        .filter(|(_, color)| COLUMN_COLOR_PRESETS.contains(&color.as_str()))
        .collect()
}

/// Truncates the window-marker label to [`LABEL_MAX_LEN`] chars (by char, not
/// byte, so this can't split a multi-byte UTF-8 sequence).
fn sanitize_label(label: Option<String>) -> Option<String> {
    label.map(|s| s.chars().take(LABEL_MAX_LEN).collect())
}

fn sanitize_file_config(mut config: FileConfig) -> FileConfig {
    config.tint = sanitize_tint(config.tint);
    config.label = sanitize_label(config.label);
    for view in config.tables.values_mut() {
        view.column_colors = sanitize_column_colors(std::mem::take(&mut view.column_colors));
    }
    config
}

fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| {
        warn!("OS config directory unavailable, falling back to current working directory");
        PathBuf::from(".")
    });
    base.join("dblitz")
}

/// Writes `contents` to `path` atomically: write to a sibling temp file in the
/// same directory, then `fs::rename` it over the target. A rename within one
/// directory is a single filesystem-metadata operation, so a crash or power
/// loss mid-write can never leave `path` holding a torn/partial JSON file --
/// readers see either the old complete file or the new complete file, never
/// something in between. `fs::rename` replaces an existing destination on
/// both Windows (`MoveFileExW` + `MOVEFILE_REPLACE_EXISTING`) and Unix, so
/// this works whether or not `path` already exists.
///
/// The temp filename embeds the PID and a nanosecond timestamp so concurrent
/// writers (e.g. two dblitz windows saving at the same moment) don't clobber
/// each other's in-flight temp file. The final rename itself is still a
/// plain last-writer-wins race across processes -- that's the existing,
/// accepted, documented best-effort behavior for the unlocked
/// read-modify-write cycle; this helper only removes the "torn file" failure
/// mode, not the race.
fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("config path {} has no parent directory", path.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(
        "{}.{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config"),
        std::process::id(),
        nanos
    );
    let tmp_path = dir.join(tmp_name);
    fs::write(&tmp_path, contents).map_err(|e| {
        // A failed write can still have created (and partially filled) the
        // temp file - a disk-full or quota error hits mid-write, not before
        // it. Returning without this cleanup stranded one uniquely-named
        // `.tmp` per attempt in the config dir, with nothing that ever
        // collects them. Same best-effort shape as the rename path below:
        // ignore a secondary failure so the original error is reported.
        let _ = fs::remove_file(&tmp_path);
        e.to_string()
    })?;
    fs::rename(&tmp_path, path).map_err(|e| {
        // Best-effort cleanup of the orphaned temp file; ignore a secondary
        // failure here so the original rename error is what gets reported.
        let _ = fs::remove_file(&tmp_path);
        format!(
            "failed to atomically replace {} via rename: {e}",
            path.display()
        )
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// App-level config (recent files, etc.) — separate from per-DB view config.
// The per-DB view config lives in one SHA-256-named file per database
// (<config_dir>/<hash>.json); the app config is a single shared
// <config_dir>/app.json.
// ─────────────────────────────────────────────────────────────────────────────

/// Every field is `#[serde(default)]` because [`ConfigStore::load_app_config`]
/// falls back to `AppConfig::default()` on *any* parse error — a field added
/// without a default would make every older `app.json` unparseable and silently
/// wipe the user's recent-files list.
///
/// `Default` is written by hand rather than derived: `check_for_updates_on_startup`
/// defaults to `true`, and a derived `Default` would give `false`. That would
/// diverge from the serde default, so a corrupt `app.json` would silently opt
/// the user out of update checks instead of leaving them on.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    /// Most-recently-opened databases, most recent first. Capped at RECENT_FILES_MAX.
    #[serde(default)]
    pub recent_files: Vec<String>,
    /// Directory into which "Open in Excel" exports are written. `None` means
    /// use the OS temp directory (the historical default).
    #[serde(default)]
    pub export_dir: Option<String>,
    /// Whether to check GitHub for a new release shortly after launch. On by
    /// default: an updater nobody opts into never reaches the users who most
    /// need it. The manual check in the toolbar's Settings dropdown works
    /// regardless of this flag.
    #[serde(default = "default_check_for_updates_on_startup")]
    pub check_for_updates_on_startup: bool,
    /// Version of the last build that ran. Written by Rust at startup, not by
    /// the frontend, so the UI can tell "we just updated" from "same build as
    /// before" and confirm the update landed. `None` on a first run.
    #[serde(default)]
    pub last_run_version: Option<String>,
}

fn default_check_for_updates_on_startup() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            recent_files: Vec::new(),
            export_dir: None,
            check_for_updates_on_startup: default_check_for_updates_on_startup(),
            last_run_version: None,
        }
    }
}

/// Public struct for the enriched recents list (path + window marker).
/// `rename_all = "camelCase"` is a no-op today (all fields are single words)
/// but locks in the JS-side field names so adding e.g. `last_opened` later
/// won't silently rename the serialized key.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecentFile {
    pub path: String,
    pub tint: Option<String>,
    pub label: Option<String>,
}

/// Normalize a path for case-insensitive dedup on Windows. On Unix, paths are
/// case-sensitive so the original is returned.
fn normalize_for_dedup(p: &str) -> String {
    if cfg!(windows) {
        p.replace('\\', "/").to_lowercase()
    } else {
        p.to_string()
    }
}

/// All config I/O, rooted at one base directory. Every operation is a method on
/// the store so it can be exercised against any root: production uses
/// [`ConfigStore::os_default`] (the real `<os-config-dir>/dblitz`), while unit
/// tests build one over a `tempfile::TempDir` and never touch the user's real
/// `~/.config/dblitz`. Carrying the base dir as state is exactly why this is a
/// store rather than a set of free functions that hardcode the OS dir — the
/// whole surface becomes testable by swapping one field. The public no-arg
/// wrappers below (the Tauri-command-facing API) delegate to the OS-default
/// store.
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    /// Store rooted at an explicit directory. Used by tests with a `TempDir`.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Production store: rooted at the real OS config directory.
    pub fn os_default() -> Self {
        Self::new(config_dir())
    }

    fn config_path_for_db(&self, db_path: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(db_path.as_bytes());
        let hash = hex::encode(hasher.finalize());
        self.dir.join(format!("{}.json", &hash[..16]))
    }

    pub fn load_config(&self, db_path: &str) -> FileConfig {
        let path = self.config_path_for_db(db_path);
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(s) => match serde_json::from_str(&s) {
                    Ok(config) => return sanitize_file_config(config),
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "Config file corrupted, using defaults")
                    }
                },
                Err(e) => warn!(path = %path.display(), error = %e, "Failed to read config file"),
            }
        }
        FileConfig::default()
    }

    pub fn save_config(&self, db_path: &str, config: &FileConfig) -> Result<(), String> {
        fs::create_dir_all(&self.dir).str_err()?;
        let path = self.config_path_for_db(db_path);
        let json = serde_json::to_string_pretty(&sanitize_file_config(config.clone())).str_err()?;
        atomic_write(&path, &json)?;
        info!(path = %path.display(), "Saved view config");
        Ok(())
    }

    fn app_config_path(&self) -> PathBuf {
        self.dir.join("app.json")
    }

    fn load_app_config(&self) -> AppConfig {
        let path = self.app_config_path();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(s) => match serde_json::from_str::<AppConfig>(&s) {
                    Ok(c) => return c,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "App config corrupted, using defaults")
                    }
                },
                Err(e) => warn!(path = %path.display(), error = %e, "Failed to read app config"),
            }
        }
        AppConfig::default()
    }

    fn save_app_config(&self, config: &AppConfig) -> Result<(), String> {
        fs::create_dir_all(&self.dir).str_err()?;
        let path = self.app_config_path();
        let json = serde_json::to_string_pretty(config).str_err()?;
        atomic_write(&path, &json)
    }

    /// Push a path to the front of the recent-files list. Dedupes against any
    /// existing entry (case-insensitive on Windows) and caps the list length.
    /// Failures are logged but not propagated — recents are best-effort.
    pub fn push_recent_file(&self, path: &str) {
        let mut config = self.load_app_config();
        let normalized = normalize_for_dedup(path);
        config
            .recent_files
            .retain(|p| normalize_for_dedup(p) != normalized);
        config.recent_files.insert(0, path.to_string());
        config.recent_files.truncate(RECENT_FILES_MAX);
        if let Err(e) = self.save_app_config(&config) {
            warn!(error = %e, "Failed to save recent files");
        }
    }

    /// Returns the recent-files list, lazily filtering out paths that no longer
    /// exist on disk. Stale entries stay in the saved config until the next push.
    pub fn get_recent_files(&self) -> Vec<String> {
        self.load_app_config()
            .recent_files
            .into_iter()
            // Defensive cap on the read path: enforces the contract even if
            // `app.json` was hand-edited to contain more than RECENT_FILES_MAX
            // entries. Take BEFORE filter so the cap applies to storage order,
            // not to the post-filter (existing-only) list.
            .take(RECENT_FILES_MAX)
            .filter(|p| std::path::Path::new(p).exists())
            .collect()
    }

    /// Wipe the recent-files list.
    pub fn clear_recent_files(&self) -> Result<(), String> {
        let mut config = self.load_app_config();
        config.recent_files.clear();
        self.save_app_config(&config)
    }

    /// Returns the configured Excel-export directory, or `None` when unset (the
    /// caller should then fall back to the OS temp dir — see [`ConfigStore::resolve_export_dir`]).
    pub fn get_export_dir(&self) -> Option<String> {
        self.load_app_config().export_dir
    }

    /// Persist the Excel-export directory. Pass `None` (or a blank string) to
    /// clear it back to the temp-dir default.
    pub fn set_export_dir(&self, export_dir: Option<String>) -> Result<(), String> {
        let mut config = self.load_app_config();
        // Treat blank/whitespace as "unset" so the UI can clear back to the
        // default by sending an empty string without a separate command.
        config.export_dir = export_dir.filter(|s| !s.trim().is_empty());
        self.save_app_config(&config)
    }

    /// Resolve the directory exports should be written to. Returns the
    /// configured directory when it is set and currently exists as a directory;
    /// otherwise falls back to the OS temp dir. The fallback keeps "Open in
    /// Excel" working if the chosen folder was deleted, renamed, or lives on an
    /// unmounted drive.
    pub fn resolve_export_dir(&self) -> PathBuf {
        match self.get_export_dir() {
            Some(dir) => {
                let p = PathBuf::from(&dir);
                if p.is_dir() {
                    p
                } else {
                    warn!(dir = %dir, "Configured export directory missing, using temp dir");
                    std::env::temp_dir()
                }
            }
            None => std::env::temp_dir(),
        }
    }

    /// Whether the startup update check is enabled (default `true`).
    pub fn get_check_for_updates_on_startup(&self) -> bool {
        self.load_app_config().check_for_updates_on_startup
    }

    /// Persist the startup-update-check opt-out.
    pub fn set_check_for_updates_on_startup(&self, enabled: bool) -> Result<(), String> {
        let mut config = self.load_app_config();
        config.check_for_updates_on_startup = enabled;
        self.save_app_config(&config)
    }

    /// Records `version` as the build that ran, returning the version it
    /// replaced. A `Some(previous)` differing from `version` means the app was
    /// updated since the last launch, which is what drives the one-time
    /// "updated to vX" notice. A first run returns `None`, so callers can tell
    /// a fresh install from an upgrade and stay quiet on the former.
    ///
    /// Must be called exactly once per launch, before anything else can read
    /// the value back — the write is what makes the *next* launch's answer
    /// correct, and it destroys this launch's answer in the process. That is
    /// why the result is cached in app state (see `lib.rs`) instead of being
    /// re-read on demand.
    pub fn record_run_version(&self, version: &str) -> Result<Option<String>, String> {
        let mut config = self.load_app_config();
        let previous = config.last_run_version.clone();
        if previous.as_deref() == Some(version) {
            // Unchanged: skip the write so an ordinary relaunch doesn't rewrite
            // app.json for nothing.
            return Ok(previous);
        }
        config.last_run_version = Some(version.to_string());
        self.save_app_config(&config)?;
        Ok(previous)
    }

    /// Same as [`ConfigStore::get_recent_files`], but also reads each file's
    /// per-DB config to attach the window marker (tint + label) so the recents
    /// dropdown can render them. Missing/corrupt per-DB configs silently yield
    /// `None` for both fields — a visible tint is a nice-to-have, not a
    /// correctness concern.
    pub fn get_recent_files_enriched(&self) -> Vec<RecentFile> {
        self.get_recent_files()
            .into_iter()
            .map(|path| {
                let cfg = self.load_config(&path);
                RecentFile {
                    path,
                    tint: cfg.tint,
                    label: cfg.label,
                }
            })
            .collect()
    }
}

// Thin Tauri-command-facing wrappers over the OS-default store. Their
// signatures are the app's stable config API (called from lib.rs commands);
// they exist only to bind the OS config dir so callers don't have to.

pub fn load_config(db_path: &str) -> FileConfig {
    ConfigStore::os_default().load_config(db_path)
}

pub fn save_config(db_path: &str, config: &FileConfig) -> Result<(), String> {
    ConfigStore::os_default().save_config(db_path, config)
}

pub fn push_recent_file(path: &str) {
    ConfigStore::os_default().push_recent_file(path);
}

pub fn clear_recent_files() -> Result<(), String> {
    ConfigStore::os_default().clear_recent_files()
}

pub fn get_export_dir() -> Option<String> {
    ConfigStore::os_default().get_export_dir()
}

pub fn set_export_dir(export_dir: Option<String>) -> Result<(), String> {
    ConfigStore::os_default().set_export_dir(export_dir)
}

pub fn resolve_export_dir() -> PathBuf {
    ConfigStore::os_default().resolve_export_dir()
}

pub fn get_recent_files_enriched() -> Vec<RecentFile> {
    ConfigStore::os_default().get_recent_files_enriched()
}

pub fn get_check_for_updates_on_startup() -> bool {
    ConfigStore::os_default().get_check_for_updates_on_startup()
}

pub fn set_check_for_updates_on_startup(enabled: bool) -> Result<(), String> {
    ConfigStore::os_default().set_check_for_updates_on_startup(enabled)
}

pub fn record_run_version(version: &str) -> Result<Option<String>, String> {
    ConfigStore::os_default().record_run_version(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    #[cfg(windows)]
    fn normalize_for_dedup_is_case_insensitive_on_windows() {
        // Same path written two different ways: backslashes vs slashes,
        // mixed case vs lowercase. Both must collapse to the same key.
        let a = normalize_for_dedup("C:\\Users\\Mail\\foo.db");
        let b = normalize_for_dedup("c:/users/mail/foo.db");
        assert_eq!(a, b, "Windows dedup must be case- and slash-insensitive");
    }

    #[test]
    fn push_recent_file_dedupes_existing_entry() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.push_recent_file("c:/foo/a.db");
        // Same path again with different casing.
        store.push_recent_file("C:/Foo/A.db");
        let config = store.load_app_config();
        if cfg!(windows) {
            // Windows: dedup applies, only the most-recent casing survives.
            assert_eq!(config.recent_files.len(), 1);
            assert_eq!(config.recent_files[0], "C:/Foo/A.db");
        } else {
            // Unix: case-sensitive, both entries kept, most-recent first.
            assert_eq!(config.recent_files.len(), 2);
            assert_eq!(config.recent_files[0], "C:/Foo/A.db");
            assert_eq!(config.recent_files[1], "c:/foo/a.db");
        }
    }

    #[test]
    fn push_recent_file_truncates_at_max() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        for i in 0..15 {
            store.push_recent_file(&format!("/tmp/file{i}.db"));
        }
        let config = store.load_app_config();
        assert_eq!(config.recent_files.len(), RECENT_FILES_MAX);
        // Most-recently-pushed entry sits at the front of the list.
        assert_eq!(config.recent_files[0], "/tmp/file14.db");
        // Pushes 0..=4 dropped off the tail, push 5 is now the oldest.
        assert_eq!(config.recent_files[RECENT_FILES_MAX - 1], "/tmp/file5.db");
    }

    #[test]
    fn get_recent_files_filters_missing_paths_lazily() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        let file_a = dir.path().join("a.db");
        let file_b = dir.path().join("b.db");
        std::fs::write(&file_a, b"").unwrap();
        std::fs::write(&file_b, b"").unwrap();
        store.push_recent_file(file_a.to_str().unwrap());
        store.push_recent_file(file_b.to_str().unwrap());
        // Delete one file out from under the recents list.
        std::fs::remove_file(&file_a).unwrap();
        // Read path filters dead entries — should only see B.
        let visible = store.get_recent_files();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0], file_b.to_str().unwrap());
        // But the saved file still contains both — filtering is lazy and
        // non-destructive, so a temporarily-unmounted drive doesn't wipe
        // the user's history.
        let saved = store.load_app_config();
        assert_eq!(saved.recent_files.len(), 2);
    }

    #[test]
    fn clear_recent_files_wipes_list() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.push_recent_file("/tmp/a.db");
        store.push_recent_file("/tmp/b.db");
        store.clear_recent_files().unwrap();
        let config = store.load_app_config();
        assert!(config.recent_files.is_empty());
    }

    #[test]
    fn get_recent_files_caps_at_max_even_if_storage_overflows() {
        let dir = TempDir::new().unwrap();
        // Hand-craft a config with more than RECENT_FILES_MAX entries to
        // simulate someone editing app.json directly. All paths point at
        // a real file so the existence filter doesn't drop them.
        let store = ConfigStore::new(dir.path().to_path_buf());
        let real = dir.path().join("real.db");
        std::fs::write(&real, b"").unwrap();
        let real_str = real.to_str().unwrap().to_string();
        let bloated = AppConfig {
            recent_files: (0..50).map(|_| real_str.clone()).collect(),
            ..AppConfig::default()
        };
        store.save_app_config(&bloated).unwrap();
        // Read path must enforce the cap even though storage overflowed.
        let visible = store.get_recent_files();
        assert_eq!(visible.len(), RECENT_FILES_MAX);
    }

    #[test]
    fn export_dir_round_trips_and_clears() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        // Default is unset.
        assert_eq!(store.get_export_dir(), None);
        // Set, then read back.
        store
            .set_export_dir(Some("C:/Exports".to_string()))
            .unwrap();
        assert_eq!(store.get_export_dir(), Some("C:/Exports".to_string()));
        // Blank string clears it back to the default.
        store.set_export_dir(Some("   ".to_string())).unwrap();
        assert_eq!(store.get_export_dir(), None);
        // Explicit None also clears.
        store
            .set_export_dir(Some("C:/Exports".to_string()))
            .unwrap();
        store.set_export_dir(None).unwrap();
        assert_eq!(store.get_export_dir(), None);
    }

    #[test]
    fn export_dir_does_not_disturb_recent_files() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.push_recent_file("/tmp/a.db");
        store
            .set_export_dir(Some("/tmp/exports".to_string()))
            .unwrap();
        let config = store.load_app_config();
        assert_eq!(config.recent_files, vec!["/tmp/a.db".to_string()]);
        assert_eq!(config.export_dir, Some("/tmp/exports".to_string()));
    }

    #[test]
    fn startup_update_check_defaults_on_and_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        // Default is on — an updater nobody opts into never reaches anyone.
        assert!(store.get_check_for_updates_on_startup());
        store.set_check_for_updates_on_startup(false).unwrap();
        assert!(!store.get_check_for_updates_on_startup());
        store.set_check_for_updates_on_startup(true).unwrap();
        assert!(store.get_check_for_updates_on_startup());
    }

    #[test]
    fn app_config_default_matches_its_serde_default() {
        // The load path falls back to `AppConfig::default()` on a parse error
        // while a *successful* parse of an older file uses the serde defaults.
        // If those two disagreed, a corrupt app.json would silently flip the
        // user's update-check preference. This is what a derived `Default`
        // would have broken.
        let from_empty_json: AppConfig = serde_json::from_str("{}").unwrap();
        let from_default = AppConfig::default();
        assert_eq!(
            from_empty_json.check_for_updates_on_startup,
            from_default.check_for_updates_on_startup
        );
        assert!(from_default.check_for_updates_on_startup);
    }

    #[test]
    fn pre_updater_app_config_still_parses_and_keeps_recents() {
        // Exactly what an app.json written before the updater shipped looks
        // like. Parsing must succeed: the fallback path would wipe recents.
        let legacy = r#"{"recent_files":["/tmp/a.db"],"export_dir":null}"#;
        let config: AppConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(config.recent_files, vec!["/tmp/a.db".to_string()]);
        assert!(config.check_for_updates_on_startup);
        assert_eq!(config.last_run_version, None);
    }

    #[test]
    fn record_run_version_reports_none_on_first_run() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        assert_eq!(store.record_run_version("26.7.6").unwrap(), None);
        // ...and persists it for the next launch to compare against.
        assert_eq!(
            store.load_app_config().last_run_version.as_deref(),
            Some("26.7.6")
        );
    }

    #[test]
    fn record_run_version_reports_the_version_it_replaced() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.record_run_version("26.7.5").unwrap();
        assert_eq!(
            store.record_run_version("26.7.6").unwrap().as_deref(),
            Some("26.7.5")
        );
    }

    #[test]
    fn record_run_version_is_a_no_op_on_an_unchanged_relaunch() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.record_run_version("26.7.6").unwrap();

        // Detect a rewrite by content rather than mtime: filesystem timestamp
        // resolution is coarse enough that two writes microseconds apart can
        // compare equal. Serde drops unknown keys on deserialize and does not
        // re-emit them, so this marker survives if and only if nothing saved.
        let path = store.app_config_path();
        let marked =
            fs::read_to_string(&path)
                .unwrap()
                .replacen('{', "{\n  \"__marker\": true,", 1);
        fs::write(&path, &marked).unwrap();

        // Same version again: reports itself, and must not rewrite the file.
        assert_eq!(
            store.record_run_version("26.7.6").unwrap().as_deref(),
            Some("26.7.6")
        );
        assert!(
            fs::read_to_string(&path).unwrap().contains("__marker"),
            "an unchanged relaunch must not rewrite app.json"
        );
    }

    #[test]
    fn record_run_version_does_not_disturb_other_app_config() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        store.push_recent_file("/tmp/a.db");
        store
            .set_export_dir(Some("/tmp/exports".to_string()))
            .unwrap();
        store.set_check_for_updates_on_startup(false).unwrap();
        store.record_run_version("26.7.6").unwrap();

        let config = store.load_app_config();
        assert_eq!(config.recent_files, vec!["/tmp/a.db".to_string()]);
        assert_eq!(config.export_dir, Some("/tmp/exports".to_string()));
        assert!(!config.check_for_updates_on_startup);
    }

    #[test]
    fn sanitize_file_config_drops_unknown_tint() {
        let config = FileConfig {
            tint: Some("background: red".to_string()),
            ..FileConfig::default()
        };

        assert_eq!(sanitize_file_config(config).tint, None);
    }

    #[test]
    fn sanitize_file_config_keeps_preset_tint() {
        let config = FileConfig {
            tint: Some("#3080d0".to_string()),
            ..FileConfig::default()
        };

        assert_eq!(
            sanitize_file_config(config).tint.as_deref(),
            Some("#3080d0")
        );
    }

    #[test]
    fn tint_presets_match_frontend_toolbar_utils() {
        let frontend = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/components/toolbarUtils.ts"),
        )
        .unwrap();
        let frontend_values: Vec<String> = frontend
            .lines()
            .filter_map(|line| {
                let value = line.split_once("value: \"")?.1.split_once('"')?.0;
                Some(value.to_string())
            })
            .collect();
        assert_eq!(
            TINT_PRESETS,
            frontend_values
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn column_color_presets_match_frontend_column_view() {
        // Deliberately does NOT edit columnView.ts -- just parses it as data,
        // the same lockstep pattern as `tint_presets_match_frontend_toolbar_utils`
        // above. Order doesn't matter here (unlike TINT_PRESETS, which drives an
        // ordered dropdown), so compare as sorted/deduped sets.
        let frontend = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/components/columnView.ts"),
        )
        .unwrap();
        let hex_re = regex::Regex::new(r"#[0-9a-fA-F]{6}").unwrap();
        let mut frontend_values: Vec<String> = hex_re
            .find_iter(&frontend)
            .map(|m| m.as_str().to_string())
            .collect();
        frontend_values.sort();
        frontend_values.dedup();

        let mut expected: Vec<String> =
            COLUMN_COLOR_PRESETS.iter().map(|s| s.to_string()).collect();
        expected.sort();
        expected.dedup();

        assert_eq!(
            expected, frontend_values,
            "COLUMN_COLOR_PRESETS in config.rs must match colorPresetsForTheme in columnView.ts"
        );
    }

    #[test]
    fn sanitize_file_config_drops_unknown_column_colors() {
        let mut view = ViewConfig::default();
        view.column_colors
            .insert("good".to_string(), "#fde8e8".to_string());
        view.column_colors
            .insert("bad".to_string(), "javascript:alert(1)".to_string());
        let mut config = FileConfig::default();
        config.tables.insert("t".to_string(), view);

        let sanitized = sanitize_file_config(config);
        let colors = &sanitized.tables["t"].column_colors;
        assert_eq!(colors.len(), 1);
        assert_eq!(colors.get("good"), Some(&"#fde8e8".to_string()));
    }

    #[test]
    fn sanitize_file_config_caps_label_length() {
        let long_label = "x".repeat(200);
        let config = FileConfig {
            label: Some(long_label),
            ..FileConfig::default()
        };

        let sanitized = sanitize_file_config(config);
        assert_eq!(sanitized.label.unwrap().chars().count(), LABEL_MAX_LEN);
    }

    #[test]
    fn sanitize_file_config_keeps_short_label() {
        let config = FileConfig {
            label: Some("PROD".to_string()),
            ..FileConfig::default()
        };

        assert_eq!(sanitize_file_config(config).label.as_deref(), Some("PROD"));
    }

    #[test]
    fn view_config_round_trips() {
        let dir = TempDir::new().unwrap();
        let db_path = "C:/data/prod.db";

        let mut view = ViewConfig::default();
        view.column_widths.insert("id".to_string(), 120);
        view.hidden_columns.push("internal_notes".to_string());
        view.column_colors
            .insert("id".to_string(), "#fde8e8".to_string());
        view.pinned_filters.insert(
            "id".to_string(),
            PinnedFilter {
                value: "42".to_string(),
                is_regex: false,
            },
        );
        let mut config = FileConfig {
            tint: Some("#3080d0".to_string()),
            label: Some("PROD".to_string()),
            ..FileConfig::default()
        };
        config.tables.insert("orders".to_string(), view);

        let store = ConfigStore::new(dir.path().to_path_buf());
        store.save_config(db_path, &config).unwrap();
        let loaded = store.load_config(db_path);

        assert_eq!(loaded, config);
    }

    #[test]
    fn corrupt_view_config_yields_defaults() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        let db_path = "C:/data/corrupt.db";
        fs::create_dir_all(dir.path()).unwrap();
        let path = store.config_path_for_db(db_path);
        fs::write(&path, "{bad json").unwrap();

        // Must not panic, and must fall back to defaults rather than
        // propagating the parse error.
        let loaded = store.load_config(db_path);
        assert_eq!(loaded, FileConfig::default());
    }

    #[test]
    fn config_path_for_db_uses_16_char_sha256_hex_filename() {
        // Pins the hashing scheme: sha256("test.db") truncated to its first
        // 16 hex chars, `.json` extension. Expected hash independently
        // verified via .NET's SHA256 outside this test.
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        let path = store.config_path_for_db("test.db");
        assert_eq!(path, dir.path().join("aec6124094934e4c.json"));
    }

    #[test]
    fn save_config_leaves_no_temp_files_behind() {
        let dir = TempDir::new().unwrap();
        let store = ConfigStore::new(dir.path().to_path_buf());
        let db_path = "C:/data/atomic.db";
        // Save twice to exercise both the create and the atomic-replace path.
        store.save_config(db_path, &FileConfig::default()).unwrap();
        store.save_config(db_path, &FileConfig::default()).unwrap();

        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one surviving config file, found: {entries:?}"
        );
        assert!(entries[0].ends_with(".json"));
    }
}
