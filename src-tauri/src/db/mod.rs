mod export;
mod query;
mod schema;
mod sql_exec;

#[cfg(debug_assertions)]
mod benchmark;

#[cfg(debug_assertions)]
pub use benchmark::{benchmark_query, BenchmarkResult};
pub use export::export_to_xlsx;
pub use query::{count_rows, query_table};
pub use schema::{close_database, get_columns, get_schema, get_tables, open_database};
pub use sql_exec::execute_sql;

use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Escape a SQL identifier (table/column name) for safe use in double-quoted contexts.
fn safe_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

/// Converts any error with Display into Result<T, String>.
pub(crate) trait StrErr<T> {
    fn str_err(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> StrErr<T> for Result<T, E> {
    fn str_err(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// Sparse rowid index: maps chunk_index to starting rowid for O(log n) seeks.
struct RowidIndex {
    /// chunk_index to rowid of first row in that chunk
    boundaries: Vec<i64>,
    /// total row count at time of index build
    total_rows: i64,
}

const MAX_QUERY_LIMIT: i64 = 10_000;

pub struct DbState {
    pub conn: Mutex<Option<Connection>>,
    pub current_path: Mutex<Option<String>>,
    /// Keyed as `"{table}\0{chunk_size}"` because rowid boundaries depend on
    /// the page size used when building the sparse index.
    rowid_indexes: Mutex<HashMap<String, RowidIndex>>,
}

impl DbState {
    pub fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            current_path: Mutex::new(None),
            rowid_indexes: Mutex::new(HashMap::new()),
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
    pub total_rows: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub rows_affected: usize,
    pub error: Option<String>,
}

fn read_row(row: &rusqlite::Row, col_count: usize) -> Vec<Option<String>> {
    let mut values: Vec<Option<String>> = Vec::with_capacity(col_count);
    for i in 0..col_count {
        let val: Option<String> = row
            .get::<_, rusqlite::types::Value>(i)
            .ok()
            .map(|v| match v {
                rusqlite::types::Value::Null => None,
                rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                rusqlite::types::Value::Real(f) => Some(f.to_string()),
                rusqlite::types::Value::Text(s) => Some(s),
                rusqlite::types::Value::Blob(b) => Some(format!("[BLOB {} bytes]", b.len())),
            })
            .unwrap_or(None);
        values.push(val);
    }
    values
}

fn build_rowid_index(conn: &Connection, safe_table: &str, chunk_size: i64) -> Option<RowidIndex> {
    if conn
        .prepare(&format!("SELECT rowid FROM \"{}\" LIMIT 0", safe_table))
        .is_err()
    {
        return None;
    }

    let total_rows: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", safe_table),
            [],
            |row| row.get(0),
        )
        .ok()?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT rowid FROM \"{}\" ORDER BY rowid ASC",
            safe_table
        ))
        .ok()?;
    let mut rows_iter = stmt.query([]).ok()?;

    let mut boundaries: Vec<i64> = Vec::with_capacity((total_rows / chunk_size + 1) as usize);
    let mut idx = 0i64;
    while let Ok(Some(row)) = rows_iter.next() {
        if idx % chunk_size == 0 {
            if let Ok(rid) = row.get::<_, i64>(0) {
                boundaries.push(rid);
            }
        }
        idx += 1;
    }

    Some(RowidIndex {
        boundaries,
        total_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::safe_ident;

    #[test]
    fn safe_ident_escapes_quotes() {
        assert_eq!(safe_ident("normal"), "normal");
        assert_eq!(safe_ident("has\"quote"), "has\"\"quote");
        assert_eq!(safe_ident("two\"\"quotes"), "two\"\"\"\"quotes");
    }
}
