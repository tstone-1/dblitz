use crate::db::StrErr;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

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
