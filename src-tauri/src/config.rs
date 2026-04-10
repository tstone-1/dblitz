use crate::db::StrErr;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Maximum number of recently-opened databases tracked in app config.
const RECENT_FILES_MAX: usize = 10;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PinnedFilter {
    pub value: String,
    pub is_regex: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ViewConfig {
    pub hidden_columns: Vec<String>,
    pub column_colors: HashMap<String, String>,
    pub sort_column: Option<String>,
    pub sort_asc: bool,
    pub selected_table: Option<String>,
    #[serde(default)]
    pub column_order: Vec<String>,
    #[serde(default)]
    pub pinned_filters: HashMap<String, PinnedFilter>,
    #[serde(default)]
    pub pinned_global_filter: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct FileConfig {
    /// Per-table view configs, keyed by table name
    pub tables: HashMap<String, ViewConfig>,
}

fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("dblitz")
}

fn config_path_for_db(db_path: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(db_path.as_bytes());
    let hash = hex::encode(hasher.finalize());
    config_dir().join(format!("{}.json", &hash[..16]))
}

pub fn load_config(db_path: &str) -> FileConfig {
    let path = config_path_for_db(db_path);
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(config) => return config,
                Err(e) => warn!(path = %path.display(), error = %e, "Config file corrupted, using defaults"),
            },
            Err(e) => warn!(path = %path.display(), error = %e, "Failed to read config file"),
        }
    }
    FileConfig::default()
}

pub fn save_config(db_path: &str, config: &FileConfig) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).str_err()?;
    let path = config_path_for_db(db_path);
    let json = serde_json::to_string_pretty(config).str_err()?;
    fs::write(&path, json).str_err()?;
    info!(path = %path.display(), "Saved view config");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// App-level config (recent files, etc.) — separate from per-DB view config.
// Stored at <config_dir>/app.json. One file, shared across all databases.
//
// The functions below come in pairs: a public no-arg version that uses the
// real OS config dir, plus a private `_in(dir)` variant that takes the base
// directory explicitly. The `_in` variants exist purely so the unit tests can
// run against an isolated `tempfile::TempDir` without polluting the user's
// real `~/.config/dblitz/app.json`. Production code always calls the no-arg
// public versions.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AppConfig {
    /// Most-recently-opened databases, most recent first. Capped at RECENT_FILES_MAX.
    #[serde(default)]
    pub recent_files: Vec<String>,
}

fn app_config_path_in(dir: &Path) -> PathBuf {
    dir.join("app.json")
}

fn load_app_config_in(dir: &Path) -> AppConfig {
    let path = app_config_path_in(dir);
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<AppConfig>(&s) {
                Ok(c) => return c,
                Err(e) => warn!(path = %path.display(), error = %e, "App config corrupted, using defaults"),
            },
            Err(e) => warn!(path = %path.display(), error = %e, "Failed to read app config"),
        }
    }
    AppConfig::default()
}

fn save_app_config_in(dir: &Path, config: &AppConfig) -> Result<(), String> {
    fs::create_dir_all(dir).str_err()?;
    let path = app_config_path_in(dir);
    let json = serde_json::to_string_pretty(config).str_err()?;
    fs::write(&path, json).str_err()?;
    Ok(())
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

fn push_recent_file_in(dir: &Path, path: &str) {
    let mut config = load_app_config_in(dir);
    let normalized = normalize_for_dedup(path);
    config.recent_files.retain(|p| normalize_for_dedup(p) != normalized);
    config.recent_files.insert(0, path.to_string());
    config.recent_files.truncate(RECENT_FILES_MAX);
    if let Err(e) = save_app_config_in(dir, &config) {
        warn!(error = %e, "Failed to save recent files");
    }
}

fn get_recent_files_in(dir: &Path) -> Vec<String> {
    load_app_config_in(dir)
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

fn clear_recent_files_in(dir: &Path) -> Result<(), String> {
    let mut config = load_app_config_in(dir);
    config.recent_files.clear();
    save_app_config_in(dir, &config)
}

/// Push a path to the front of the recent-files list. Dedupes against any
/// existing entry (case-insensitive on Windows) and caps the list length.
/// Failures are logged but not propagated — recents are best-effort.
pub fn push_recent_file(path: &str) {
    push_recent_file_in(&config_dir(), path);
}

/// Returns the recent-files list, lazily filtering out paths that no longer
/// exist on disk. Stale entries stay in the saved config until the next push.
pub fn get_recent_files() -> Vec<String> {
    get_recent_files_in(&config_dir())
}

/// Wipe the recent-files list.
pub fn clear_recent_files() -> Result<(), String> {
    clear_recent_files_in(&config_dir())
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
        push_recent_file_in(dir.path(), "c:/foo/a.db");
        // Same path again with different casing.
        push_recent_file_in(dir.path(), "C:/Foo/A.db");
        let config = load_app_config_in(dir.path());
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
        for i in 0..15 {
            push_recent_file_in(dir.path(), &format!("/tmp/file{i}.db"));
        }
        let config = load_app_config_in(dir.path());
        assert_eq!(config.recent_files.len(), RECENT_FILES_MAX);
        // Most-recently-pushed entry sits at the front of the list.
        assert_eq!(config.recent_files[0], "/tmp/file14.db");
        // Pushes 0..=4 dropped off the tail, push 5 is now the oldest.
        assert_eq!(config.recent_files[RECENT_FILES_MAX - 1], "/tmp/file5.db");
    }

    #[test]
    fn get_recent_files_filters_missing_paths_lazily() {
        let dir = TempDir::new().unwrap();
        let file_a = dir.path().join("a.db");
        let file_b = dir.path().join("b.db");
        std::fs::write(&file_a, b"").unwrap();
        std::fs::write(&file_b, b"").unwrap();
        push_recent_file_in(dir.path(), file_a.to_str().unwrap());
        push_recent_file_in(dir.path(), file_b.to_str().unwrap());
        // Delete one file out from under the recents list.
        std::fs::remove_file(&file_a).unwrap();
        // Read path filters dead entries — should only see B.
        let visible = get_recent_files_in(dir.path());
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0], file_b.to_str().unwrap());
        // But the saved file still contains both — filtering is lazy and
        // non-destructive, so a temporarily-unmounted drive doesn't wipe
        // the user's history.
        let saved = load_app_config_in(dir.path());
        assert_eq!(saved.recent_files.len(), 2);
    }

    #[test]
    fn clear_recent_files_wipes_list() {
        let dir = TempDir::new().unwrap();
        push_recent_file_in(dir.path(), "/tmp/a.db");
        push_recent_file_in(dir.path(), "/tmp/b.db");
        clear_recent_files_in(dir.path()).unwrap();
        let config = load_app_config_in(dir.path());
        assert!(config.recent_files.is_empty());
    }

    #[test]
    fn get_recent_files_caps_at_max_even_if_storage_overflows() {
        let dir = TempDir::new().unwrap();
        // Hand-craft a config with more than RECENT_FILES_MAX entries to
        // simulate someone editing app.json directly. All paths point at
        // a real file so the existence filter doesn't drop them.
        let real = dir.path().join("real.db");
        std::fs::write(&real, b"").unwrap();
        let real_str = real.to_str().unwrap().to_string();
        let bloated = AppConfig {
            recent_files: (0..50).map(|_| real_str.clone()).collect(),
        };
        save_app_config_in(dir.path(), &bloated).unwrap();
        // Read path must enforce the cap even though storage overflowed.
        let visible = get_recent_files_in(dir.path());
        assert_eq!(visible.len(), RECENT_FILES_MAX);
    }
}
