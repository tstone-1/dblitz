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
pub(crate) use util::StrErr;

pub fn cancel_queries(state: &DbState) {
    state
        .query_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub fn close_database(state: &DbState) {
    tracing::info!("Closing database");
    cancel_queries(state);
    *state.conn.lock() = None;
    *state.current_path.lock() = None;
    state.rowid_indexes.lock().clear();
}
