use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

/// Sparse rowid index: maps chunk_index -> starting rowid for O(log n) seeks.
/// Built once per table on first query, invalidated on table switch.
/// External writers cannot stale this cache because we open with
/// `?immutable=1` - the connection sees a frozen snapshot for its lifetime.
pub(super) struct RowidIndex {
    /// chunk_index -> rowid of first row in that chunk
    pub(super) boundaries: Vec<i64>,
    /// total row count at time of index build
    pub(super) total_rows: i64,
    /// row count interval used to sample boundaries
    pub(super) chunk_size: i64,
}

pub struct DbState {
    pub conn: Mutex<Option<Connection>>,
    pub current_path: Mutex<Option<String>>,
    pub(super) rowid_indexes: Mutex<HashMap<String, RowidIndex>>,
    pub(super) query_generation: AtomicU64,
}

impl DbState {
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            current_path: Mutex::new(None),
            rowid_indexes: Mutex::new(HashMap::new()),
            query_generation: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TableInfo {
    pub name: String,
    pub row_count: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColumnInfo {
    pub cid: i64,
    pub name: String,
    pub col_type: String,
    pub notnull: bool,
    pub default_value: Option<String>,
    pub pk: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SchemaEntry {
    pub obj_type: String,
    pub name: String,
    pub tbl_name: String,
    pub sql: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColumnFilter {
    pub column: String,
    pub value: String,
    pub is_regex: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QueryRequest {
    pub table: String,
    pub offset: i64,
    pub limit: i64,
    pub filters: Vec<ColumnFilter>,
    pub global_filter: String,
    pub sort_column: Option<String>,
    pub sort_asc: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: Option<i64>,
    pub offset: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: usize,
    pub error: Option<String>,
    /// True when the result set exceeded `SQL_RESULT_LIMIT` and only the
    /// first N rows are returned. This is a non-fatal warning that travels
    /// *alongside* the rows - it is NOT folded into `error`, so the frontend
    /// can render the rows and a banner together.
    pub truncated: bool,
}
