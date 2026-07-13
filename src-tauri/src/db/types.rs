use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

/// Sparse rowid index: maps chunk_index -> starting rowid for O(log n) seeks.
/// Built once per table on first query, invalidated on table switch.
/// Valid only under dblitz's open-time promise that the file is not modified
/// while this immutable connection is alive.
pub(super) struct RowidIndex {
    /// chunk_index -> rowid of first row in that chunk
    pub(super) boundaries: Vec<i64>,
    /// total row count at time of index build
    pub(super) total_rows: i64,
    /// row count interval used to sample boundaries
    pub(super) chunk_size: i64,
}

/// Complete identity of an ordered view. Keeping the SQL fragments and bound
/// values as separate fields avoids an opaque string signature and guarantees
/// that a filter, parameter, or sort change invalidates the cached rowids.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OrderKey {
    pub(super) where_clause: String,
    pub(super) params: Vec<String>,
    pub(super) order_clause: String,
}

/// Full rowid order for one filtered and/or sorted view of a table. Sorted-only
/// and filtered views use the same representation because both ultimately page
/// through an ordered rowid list. Only one view per table is retained (the
/// active one): switching the filter or sort on the same table therefore
/// rebuilds rather than restoring a previously cached view. That single-entry
/// bound is deliberate — it roughly halves peak cache memory versus keeping a
/// separate sorted and filtered order resident, at the cost of one full
/// re-materialization when toggling between two views of the same table. Fast
/// scrolling *within* a view stays fully cached, which is the case that matters.
pub(super) struct OrderedRows {
    pub(super) key: OrderKey,
    pub(super) rowids: Vec<i64>,
}

pub struct DbState {
    pub conn: Mutex<Option<Connection>>,
    pub current_path: Mutex<Option<String>>,
    pub(super) rowid_indexes: Mutex<HashMap<String, RowidIndex>>,
    pub(super) ordered_rows: Mutex<HashMap<String, OrderedRows>>,
    pub(super) query_generation: AtomicU64,
    /// Handle to interrupt whatever statement is currently executing on
    /// `conn`. Independent of `conn`'s own mutex, so calling `.interrupt()`
    /// on it never has to wait for a long-running query to release the lock -
    /// that's the whole point (it's how a stuck query gets unstuck at all).
    /// `None` when no database is open; replaced on every `open_database`
    /// and cleared on `close_database`.
    pub(super) interrupt_handle: Mutex<Option<rusqlite::InterruptHandle>>,
}

impl DbState {
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            current_path: Mutex::new(None),
            rowid_indexes: Mutex::new(HashMap::new()),
            ordered_rows: Mutex::new(HashMap::new()),
            query_generation: AtomicU64::new(0),
            interrupt_handle: Mutex::new(None),
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
    /// Per-column declared type (e.g. `"INTEGER"`, `"TEXT"`), aligned 1:1
    /// with `columns`. Empty string for a column with no declared type (a
    /// computed expression like `SELECT 1+1`) - the XLSX export already
    /// treats a missing/empty decltype as numeric-affinity, so that's the
    /// correct "unknown, guess numeric" default. Used by the XLSX export so
    /// numbers don't get downgraded to text.
    pub column_types: Vec<String>,
    pub error: Option<String>,
    /// True when the result set exceeded `SQL_RESULT_LIMIT` and only the
    /// first N rows are returned. This is a non-fatal warning that travels
    /// *alongside* the rows - it is NOT folded into `error`, so the frontend
    /// can render the rows and a banner together.
    pub truncated: bool,
}
