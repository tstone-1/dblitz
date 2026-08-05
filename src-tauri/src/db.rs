#[cfg(debug_assertions)]
mod benchmark;
mod export;
mod filters;
mod query;
mod schema;
mod sql;
mod types;
mod util;

#[cfg(debug_assertions)]
pub use benchmark::{benchmark_query, BenchmarkResult};
pub use export::export_to_xlsx;
pub use query::{count_rows, query_table};
pub use schema::{get_columns, get_schema, get_tables, open_database};
pub use sql::execute_sql;
pub use types::{
    ColumnFilter, ColumnInfo, DbState, QueryRequest, QueryResult, SchemaEntry, SqlResult, TableInfo,
};
pub(crate) use util::{ErrCtx, StrErr};

pub fn cancel_queries(state: &DbState) {
    state
        .query_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Break a query out of a blocking sqlite3_step deep inside a single row
    // fetch (e.g. a grinding recursive CTE in execute_sql) rather than
    // relying solely on the generation bump above, which is only observed
    // between completed row fetches and can't interrupt one that never
    // finishes a row.
    if let Some(handle) = state.interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
}

/// Clears all per-table caches (rowid index and ordered row lists).
/// Must run on both open (a new file invalidates every table-keyed cache)
/// and close (release the materialized rowid vectors promptly rather than
/// leaving them resident until the next open).
pub(super) fn clear_caches(state: &DbState) {
    state.rowid_indexes.lock().clear();
    state.ordered_rows.lock().clear();
}

pub fn close_database(state: &DbState) {
    tracing::info!("Closing database");
    cancel_queries(state);
    *state.conn.lock() = None;
    *state.current_path.lock() = None;
    *state.interrupt_handle.lock() = None;
    clear_caches(state);
}

#[cfg(test)]
mod tests {
    use super::types::{OrderKey, OrderedRows, RowidIndex};
    use super::*;

    #[test]
    fn close_database_clears_all_caches() {
        let state = DbState::new();
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

        close_database(&state);

        assert!(
            state.rowid_indexes.lock().is_empty(),
            "rowid_indexes must be cleared on close"
        );
        assert!(
            state.ordered_rows.lock().is_empty(),
            "ordered_rows must be cleared on close"
        );
    }
}
