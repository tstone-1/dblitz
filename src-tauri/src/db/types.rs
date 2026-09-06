use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;

/// Sparse rowid index: maps chunk_index -> starting rowid for O(log n) seeks.
///
/// Built lazily, on the first page request that actually needs it - chunk 0 is
/// served with a plain `LIMIT` instead, so opening a table never pays for the
/// full rowid scan. Entries live until the database is closed or another one is
/// opened (`clear_caches`); they are NOT dropped when the user switches table,
/// so scrolling back to a table already visited in this session keeps its index.
/// Valid only under dblitz's open-time promise that the file is not modified
/// while this immutable connection is alive.
///
/// `pub` because the benchmark examples under `src-tauri/examples/` measure the
/// shipped index build through [`crate::db::bench_api`].
pub struct RowidIndex {
    /// chunk_index -> rowid of first row in that chunk
    pub boundaries: Vec<i64>,
    /// total row count at time of index build
    pub total_rows: i64,
    /// row count interval used to sample boundaries
    pub chunk_size: i64,
}

/// Complete identity of an ordered view. Keeping the SQL fragments and bound
/// values as separate fields avoids an opaque string signature and guarantees
/// that a filter, parameter, or sort change invalidates the cached rowids.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct OrderKey {
    pub(super) where_clause: String,
    pub(super) params: Vec<String>,
    pub(super) order_clause: String,
    /// Column index + pattern source for every regex filter applied in Rust,
    /// in the order they were evaluated. Regex filters never reach the SQL, so
    /// `where_clause` and `params` are identical for two views that differ
    /// only by their pattern - without this field the cached rowid list of one
    /// regex would silently be served as the match set of another, and
    /// clearing the regex entirely would keep serving the narrowed set.
    /// Empty for every non-regex view.
    pub(super) regex_signature: Vec<(usize, String)>,
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
    /// The Browse Data connection. `query_table` and `count_rows` hold this
    /// lock for their whole duration - a sort or a filter on a large table can
    /// grind for seconds - and they are the only readers that build or consult
    /// the table-keyed caches below, which is why the caches are keyed to this
    /// connection alone.
    pub conn: Mutex<Option<Connection>>,
    /// A SECOND read-only connection to the same file, so the SQL tab and the
    /// Structure tab do not queue behind a grinding Browse Data page.
    ///
    /// This is correct only because of `?immutable=1`. Both connections open
    /// the same URI with the same flags, the same open batch and the same
    /// authorizer (one function, [`crate::db::schema::open_read_only_connection`],
    /// builds both so they cannot drift), and the file is a frozen snapshot for
    /// their lifetime - so two connections cannot disagree about its contents
    /// and no transaction or snapshot coordination is needed between them. Drop
    /// `immutable=1` and that stops being true: the two connections would then
    /// be able to observe different states of a file another process is
    /// writing, and every "the count cannot go stale" argument in this module
    /// would need re-arguing.
    ///
    /// Measured on the 870 MB / 5M-row table (macOS, M5, release build, three
    /// rounds each): an `execute_sql("SELECT 1")` issued 50 ms into a
    /// sort-by-name page waited **736-748 ms** on the shared connection - the
    /// rest of the ~800 ms sort - and **19-39 us** on this one. The same
    /// applies to the Structure tab and to any other command that does not
    /// need the browse caches.
    pub aux_conn: Mutex<Option<Connection>>,
    pub current_path: Mutex<Option<String>>,
    pub(super) rowid_indexes: Mutex<HashMap<String, RowidIndex>>,
    pub(super) ordered_rows: Mutex<HashMap<String, OrderedRows>>,
    /// Unfiltered `COUNT(*)` per table, for the lifetime of one open file.
    ///
    /// The connection is `?immutable=1`, so the file is a frozen snapshot and a
    /// count taken at open cannot go stale - which is what makes caching it
    /// correct rather than merely convenient. Seeded at open from the counts
    /// `get_tables_inner` already took, and filled in on demand for anything it
    /// could not count (a view, or a table whose count errored). Without it,
    /// every page of a WITHOUT ROWID table or a view re-counted the whole
    /// object: 109 ms per scroll chunk on a warm 5M-row table here.
    /// Cleared alongside the other table-keyed caches on open and close.
    pub(super) table_counts: Mutex<HashMap<String, i64>>,
    pub(super) query_generation: AtomicU64,
    /// Handle to interrupt whatever statement is currently executing on
    /// `conn`. Independent of `conn`'s own mutex, so calling `.interrupt()`
    /// on it never has to wait for a long-running query to release the lock -
    /// that's the whole point (it's how a stuck query gets unstuck at all).
    /// `None` when no database is open; replaced on every `open_database`
    /// and cleared on `close_database`.
    pub(super) interrupt_handle: Mutex<Option<rusqlite::InterruptHandle>>,
    /// The same, for [`Self::aux_conn`]. `cancel_queries` must interrupt BOTH
    /// handles: a statement grinding on one connection is invisible to the
    /// other's handle, so cancelling only the browse connection would leave a
    /// runaway recursive CTE in the SQL tab running forever.
    pub(super) aux_interrupt_handle: Mutex<Option<rusqlite::InterruptHandle>>,
}

/// Delegates to [`DbState::new`]. Required now that `db` is a public module:
/// a public type with a no-argument `new` and no `Default` is a clippy error
/// (`new_without_default`), and the two must not drift apart.
impl Default for DbState {
    fn default() -> Self {
        Self::new()
    }
}

impl DbState {
    /// The generation a query must still be under for its result to be
    /// published; bumped by every open and every cancel. `pub` only so the
    /// benchmark examples can call the shipped build functions, which take it
    /// as a parameter - passing a stale value makes them report cancellation.
    pub fn current_query_generation(&self) -> u64 {
        self.query_generation
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            aux_conn: Mutex::new(None),
            current_path: Mutex::new(None),
            rowid_indexes: Mutex::new(HashMap::new()),
            ordered_rows: Mutex::new(HashMap::new()),
            table_counts: Mutex::new(HashMap::new()),
            query_generation: AtomicU64::new(0),
            interrupt_handle: Mutex::new(None),
            aux_interrupt_handle: Mutex::new(None),
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
