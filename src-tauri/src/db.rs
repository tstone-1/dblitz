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
}

/// Clears all per-table caches (rowid index, sorted order, filtered order).
/// Must run on both open (a new file invalidates every table-keyed cache)
/// and close (release the materialized rowid vectors promptly rather than
/// leaving them resident until the next open). Single-sourced here after a
/// hand-synced pair of clear sites let `filtered_orders` (added later)
/// leak past `close_database` while `open_database` cleared it correctly.
pub(super) fn clear_caches(state: &DbState) {
    state.rowid_indexes.lock().clear();
    state.sorted_orders.lock().clear();
    state.filtered_orders.lock().clear();
}

pub fn close_database(state: &DbState) {
    tracing::info!("Closing database");
    cancel_queries(state);
    *state.conn.lock() = None;
    *state.current_path.lock() = None;
    clear_caches(state);
}

#[cfg(test)]
mod tests {
    use super::types::{FilteredOrder, RowidIndex, SortedOrder};
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
        state.sorted_orders.lock().insert(
            "t".to_string(),
            SortedOrder {
                sort_column: "id".to_string(),
                sort_asc: true,
                rowids: vec![1],
            },
        );
        state.filtered_orders.lock().insert(
            "t".to_string(),
            FilteredOrder {
                signature: "sig".to_string(),
                rowids: vec![1],
            },
        );

        close_database(&state);

        assert!(
            state.rowid_indexes.lock().is_empty(),
            "rowid_indexes must be cleared on close"
        );
        assert!(
            state.sorted_orders.lock().is_empty(),
            "sorted_orders must be cleared on close"
        );
        assert!(
            state.filtered_orders.lock().is_empty(),
            "filtered_orders must be cleared on close"
        );
    }
}
