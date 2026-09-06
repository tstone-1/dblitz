mod export;
mod filters;
mod query;
mod schema;
mod sql;
mod types;
mod util;

/// The query internals the benchmark examples in `src-tauri/examples/` measure.
///
/// Not part of the app's IPC surface - no `#[tauri::command]` calls any of it -
/// and nothing outside `examples/` should. It exists so a benchmark measures the
/// code that ships instead of a copy of it: both examples used to re-implement
/// the rowid seek and the offset scan inline, which meant a change to the real
/// path silently stopped being the thing the README's numbers describe.
pub mod bench_api {
    pub use super::query::{
        build_ordered_rows, build_rowid_index, fetch_rows_by_rowids, query_with_offset,
        rowid_alias, rowid_page_sql,
    };
    pub use super::types::RowidIndex;
    pub use super::util::{collect_rows, quote_ident};
}

pub use export::export_to_xlsx;
pub use query::{count_rows, query_table};
pub use schema::{get_columns, get_schema, open_database};
pub use sql::execute_sql;
pub use types::{
    ColumnFilter, ColumnInfo, DbState, QueryRequest, QueryResult, SchemaEntry, SqlResult, TableInfo,
};
pub(crate) use util::{ErrCtx, StrErr};
// Re-exported for tests only: `lib.rs` pins the launch-path fix against the URI
// this actually produces, rather than against a restatement of the rule.
#[cfg(test)]
pub(crate) use util::path_to_sqlite_uri;

pub fn cancel_queries(state: &DbState) {
    state
        .query_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Break a query out of a blocking sqlite3_step deep inside a single row
    // fetch (e.g. a grinding recursive CTE in execute_sql) rather than
    // relying solely on the generation bump above, which is only observed
    // between completed row fetches and can't interrupt one that never
    // finishes a row.
    //
    // BOTH connections are interrupted. A statement grinding on one is
    // completely invisible to the other's handle, so interrupting only the
    // browse connection would leave a runaway recursive CTE in the SQL tab
    // running until the process exits - and Cancel would look like it had
    // worked, because the button is shared.
    if let Some(handle) = state.interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
    if let Some(handle) = state.aux_interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
}

/// Clears all per-table caches (rowid index, ordered row lists, row counts).
/// Must run on both open (a new file invalidates every table-keyed cache)
/// and close (release the materialized rowid vectors promptly rather than
/// leaving them resident until the next open).
///
/// Every table-keyed cache in `DbState` must be cleared here, and
/// `close_database_clears_all_caches` asserts that for each of them: a cache
/// that survives a file switch serves one file's answers about another's, and
/// `table_counts` is the one whose staleness is invisible - a wrong row count
/// renders as a scrollbar that is simply the wrong length.
pub(super) fn clear_caches(state: &DbState) {
    state.rowid_indexes.lock().clear();
    state.ordered_rows.lock().clear();
    state.table_counts.lock().clear();
}

pub fn close_database(state: &DbState) {
    tracing::info!("Closing database");
    cancel_queries(state);
    *state.conn.lock() = None;
    *state.aux_conn.lock() = None;
    *state.current_path.lock() = None;
    *state.interrupt_handle.lock() = None;
    *state.aux_interrupt_handle.lock() = None;
    clear_caches(state);
}

#[cfg(test)]
mod tests {
    use super::types::{OrderKey, OrderedRows, RowidIndex};
    use super::*;

    #[test]
    fn close_database_clears_all_caches() {
        // A REAL open, not a bare `DbState::new()`. The two connection
        // assertions below are only falsifiable against a state that actually
        // has connections: on a fresh `DbState` both are already `None`, so
        // deleting either `= None` line in `close_database` left this test
        // green - measured, which is how this open got added.
        let path = crate::db::util::unique_temp_path("dblitz_close_test", ".sqlite");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE t (a INTEGER);").unwrap();
        }
        let state = DbState::new();
        open_database(&state, path.to_str().unwrap()).unwrap();
        assert!(
            state.conn.lock().is_some() && state.aux_conn.lock().is_some(),
            "precondition: an open database holds both connections"
        );

        state.rowid_indexes.lock().insert(
            "t".to_string(),
            RowidIndex {
                boundaries: vec![0],
                total_rows: 1,
                chunk_size: 500,
            },
        );
        state.ordered_rows.lock().insert(
            "t".to_string(),
            OrderedRows {
                key: OrderKey {
                    where_clause: String::new(),
                    params: Vec::new(),
                    order_clause: " ORDER BY id ASC".to_string(),
                    regex_signature: Vec::new(),
                },
                rowids: vec![1],
            },
        );
        state.table_counts.lock().insert("t".to_string(), 1);

        close_database(&state);

        assert!(
            state.conn.lock().is_none(),
            "the browse connection must be dropped on close"
        );
        assert!(
            state.aux_conn.lock().is_none(),
            "the secondary (SQL/Structure) connection must be dropped on close \
             too - a connection left open holds the file and, on a network or \
             cloud-synced share, keeps a handle on a file the user believes \
             dblitz has let go of"
        );
        assert!(
            state.rowid_indexes.lock().is_empty(),
            "rowid_indexes must be cleared on close"
        );
        assert!(
            state.ordered_rows.lock().is_empty(),
            "ordered_rows must be cleared on close"
        );
        assert!(
            state.table_counts.lock().is_empty(),
            "table_counts must be cleared on close - a row count kept across a \
             file switch reports the previous file's size for a table of the \
             same name"
        );

        let _ = std::fs::remove_file(&path);
    }
}
