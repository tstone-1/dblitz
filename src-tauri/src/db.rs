use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn, error};

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

/// Sparse rowid index: maps chunk_index → starting rowid for O(log n) seeks.
/// Built once per table on first query, invalidated on table switch.
/// External writers cannot stale this cache because we open with
/// `?immutable=1` — the connection sees a frozen snapshot for its lifetime.
struct RowidIndex {
    /// chunk_index → rowid of first row in that chunk
    boundaries: Vec<i64>,
    /// total row count at time of index build
    total_rows: i64,
}

pub struct DbState {
    pub conn: Mutex<Option<Connection>>,
    pub current_path: Mutex<Option<String>>,
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

pub fn close_database(state: &DbState) {
    info!("Closing database");
    *state.conn.lock() = None;
    *state.current_path.lock() = None;
    state.rowid_indexes.lock().clear();
}

/// Convert an OS file path into a SQLite URI with `?immutable=1`. Percent-
/// encodes the few characters that have special meaning in URIs and
/// normalizes Windows backslashes to forward slashes.
fn path_to_sqlite_uri(path: &str) -> String {
    // Percent-encode in this order: % first (so we don't double-encode our
    // own escapes), then the others.
    let encoded = path
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('\\', "/");
    // Unix path "/foo/bar" → "file:/foo/bar?immutable=1"
    // Windows path "C:/foo/bar" → "file:/C:/foo/bar?immutable=1"
    if encoded.starts_with('/') {
        format!("file:{}?immutable=1", encoded)
    } else {
        format!("file:/{}?immutable=1", encoded)
    }
}

pub fn open_database(state: &DbState, path: &str) -> Result<Vec<TableInfo>, String> {
    info!(path, "Opening database (read-only, immutable)");
    // dblitz is a viewer, not an editor. Two layers of read-only:
    //   1. SQLITE_OPEN_READ_ONLY at the connection layer.
    //   2. ?immutable=1 in the URI tells SQLite to treat the file as a
    //      frozen snapshot — no shared-memory coordination, no `-shm`
    //      or `-wal` companion files created by us. The trade-off is
    //      that we won't see live writes from other processes; reopening
    //      the file is required to pick up changes.
    // Both together: SQLite refuses writes AND avoids touching anything
    // beyond the main file, which matters when the database lives in a
    // OneDrive-synced folder where stray companion files become noise.
    let uri = path_to_sqlite_uri(path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(&uri, flags).map_err(|e| {
        error!(path, error = %e, "Failed to open database");
        e.to_string()
    })?;
    // cache_size is per-connection and safe on a read-only connection.
    conn.execute_batch("PRAGMA cache_size=-64000;").str_err()?;

    let tables = get_tables_inner(&conn)?;

    *state.conn.lock() = Some(conn);
    *state.current_path.lock() = Some(path.to_string());

    Ok(tables)
}

fn get_tables_inner(conn: &Connection) -> Result<Vec<TableInfo>, String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .str_err()?;

    let table_names: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .str_err()?
        .collect::<Result<Vec<_>, _>>()
        .str_err()?;

    let mut tables = Vec::new();
    for name in table_names {
        // -1 signals "count unknown" to the frontend, which renders it as `?`.
        // Swallowing to 0 used to hide legitimate access errors (corrupt page,
        // missing index) behind a cheerful empty table.
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM \"{}\"", safe_ident(&name)),
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|e| {
                warn!(table = %name, error = %e, "Failed to count rows");
                -1
            });
        tables.push(TableInfo { name, row_count: count });
    }
    Ok(tables)
}

pub fn get_tables(state: &DbState) -> Result<Vec<TableInfo>, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    get_tables_inner(conn)
}

pub fn get_columns(state: &DbState, table: &str) -> Result<Vec<ColumnInfo>, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;

    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", safe_ident(table)))
        .str_err()?;

    let columns: Vec<ColumnInfo> = stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                cid: row.get(0)?,
                name: row.get(1)?,
                col_type: row.get::<_, String>(2).unwrap_or_default(),
                notnull: row.get::<_, bool>(3).unwrap_or(false),
                default_value: row.get(4).ok(),
                pk: row.get::<_, bool>(5).unwrap_or(false),
            })
        })
        .str_err()?
        .collect::<Result<Vec<_>, _>>()
        .str_err()?;

    Ok(columns)
}

pub fn get_schema(state: &DbState) -> Result<Vec<SchemaEntry>, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;

    let mut stmt = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
        .str_err()?;

    let entries: Vec<SchemaEntry> = stmt
        .query_map([], |row| {
            Ok(SchemaEntry {
                obj_type: row.get(0)?,
                name: row.get(1)?,
                tbl_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })
        .str_err()?
        .collect::<Result<Vec<_>, _>>()
        .str_err()?;

    Ok(entries)
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

/// Build a sparse rowid index for a table: sample the rowid at every chunk_size boundary.
/// This turns OFFSET-based queries into O(log n) rowid seeks.
fn build_rowid_index(conn: &Connection, safe_table: &str, chunk_size: i64) -> Option<RowidIndex> {
    // Check if table has rowid
    if conn.prepare(&format!("SELECT rowid FROM \"{}\" LIMIT 0", safe_table)).is_err() {
        return None;
    }

    let total_rows: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", safe_table), [], |row| row.get(0))
        .ok()?;

    // Scan all rowids in order — this reads only the B-tree keys, not row data
    let mut stmt = conn
        .prepare(&format!("SELECT rowid FROM \"{}\" ORDER BY rowid ASC", safe_table))
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

    Some(RowidIndex { boundaries, total_rows })
}

fn get_column_names(conn: &Connection, safe_table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", safe_table))
        .str_err()?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .str_err()?
        .collect::<Result<Vec<_>, _>>()
        .str_err()?;
    Ok(cols)
}

#[derive(Debug)]
struct WhereResult {
    clause: String,
    params: Vec<String>,
    regex_filters: Vec<(usize, Regex)>,
}

fn build_where_clause(
    columns: &[String],
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<WhereResult, String> {
    let mut where_parts: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut regex_filters: Vec<(usize, Regex)> = Vec::new();

    if !global_filter.is_empty() {
        let or_conditions: Vec<String> = columns
            .iter()
            .map(|c| format!("\"{}\" LIKE ?", safe_ident(c)))
            .collect();
        where_parts.push(format!("({})", or_conditions.join(" OR ")));
        let pattern = format!("%{}%", global_filter);
        for _ in columns {
            params.push(pattern.clone());
        }
    }

    for f in filters {
        if f.value.is_empty() {
            continue;
        }
        if f.is_regex {
            if let Some(idx) = columns.iter().position(|c| c == &f.column) {
                match Regex::new(&f.value) {
                    Ok(re) => regex_filters.push((idx, re)),
                    Err(e) => return Err(format!("Invalid regex '{}': {}", f.column, e)),
                }
            }
        } else {
            let col_escaped = safe_ident(&f.column);

            // Split on semicolon for multi-criteria: exclusions=AND, inclusions=OR
            let criteria: Vec<&str> = f.value.split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            if criteria.is_empty() {
                continue;
            }

            // Separate SQL fragments AND params for each group to keep
            // placeholder order in sync with the assembled SQL.
            let mut and_parts: Vec<String> = Vec::new();
            let mut and_params: Vec<String> = Vec::new();
            let mut or_parts: Vec<String> = Vec::new();
            let mut or_params: Vec<String> = Vec::new();

            for val in &criteria {
                if let Some(rest) = val.strip_prefix("<>") {
                    if rest.is_empty() {
                        // Bare <> means "non-empty"
                        and_parts.push(format!("\"{}\" IS NOT NULL AND \"{}\" != ''", col_escaped, col_escaped));
                    } else {
                        // Exclude: NOT LIKE (inverse of default contains-match)
                        and_parts.push(format!("\"{}\" NOT LIKE ?", col_escaped));
                        and_params.push(format!("%{}%", rest));
                    }
                } else if let Some(rest) = val.strip_prefix(">=") {
                    and_parts.push(format!("\"{}\" >= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix("<=") {
                    and_parts.push(format!("\"{}\" <= ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('>') {
                    and_parts.push(format!("\"{}\" > ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('<') {
                    and_parts.push(format!("\"{}\" < ?", col_escaped));
                    and_params.push(rest.to_string());
                } else if let Some(rest) = val.strip_prefix('=') {
                    or_parts.push(format!("\"{}\" = ?", col_escaped));
                    or_params.push(rest.to_string());
                } else {
                    or_parts.push(format!("\"{}\" LIKE ?", col_escaped));
                    or_params.push(format!("%{}%", val));
                }
            }

            // Assemble: OR group first, then AND parts — params must follow same order
            let mut col_parts: Vec<String> = Vec::new();
            if or_parts.len() == 1 {
                col_parts.push(or_parts.remove(0));
            } else if or_parts.len() > 1 {
                col_parts.push(format!("({})", or_parts.join(" OR ")));
            }
            col_parts.extend(and_parts);
            params.extend(or_params);
            params.extend(and_params);

            if col_parts.len() == 1 {
                where_parts.push(col_parts.remove(0));
            } else if col_parts.len() > 1 {
                where_parts.push(format!("({})", col_parts.join(" AND ")));
            }
        }
    }

    let clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_parts.join(" AND "))
    };

    Ok(WhereResult { clause, params, regex_filters })
}

/// Execute a prepared statement and collect all rows into a Vec.
fn collect_rows(
    stmt: &mut rusqlite::Statement,
    params: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<Vec<Option<String>>>, String> {
    let col_count = stmt.column_count();
    let mut rows_iter = stmt.query(params).str_err()?;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    while let Some(row) = rows_iter.next().str_err()? {
        rows.push(read_row(row, col_count));
    }
    Ok(rows)
}

/// Full scan with post-query regex filtering in Rust.
#[allow(clippy::too_many_arguments)]
fn query_with_regex_filter(
    conn: &Connection,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
    regex_filters: &[(usize, Regex)],
    offset: i64,
    limit: i64,
    columns: Vec<String>,
) -> Result<QueryResult, String> {
    let sql = format!("SELECT * FROM \"{}\"{}{}",safe_table, where_clause, order_clause);
    let mut stmt = conn.prepare(&sql).str_err()?;
    let col_count = stmt.column_count();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p as &dyn rusqlite::types::ToSql).collect();

    let mut all_rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut rows_iter = stmt.query(param_refs.as_slice()).str_err()?;

    while let Some(row) = rows_iter.next().str_err()? {
        let values = read_row(row, col_count);
        let matches = regex_filters.iter().all(|(idx, re)| {
            values.get(*idx).and_then(|v| v.as_ref()).map(|s| re.is_match(s)).unwrap_or(false)
        });
        if matches {
            all_rows.push(values);
        }
    }

    let total_rows = all_rows.len() as i64;
    let rows: Vec<Vec<Option<String>>> = all_rows
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();

    Ok(QueryResult { columns, rows, total_rows, offset })
}

/// O(log n) seek using sparse rowid index — only when no filters/sort.
#[allow(clippy::too_many_arguments)]
fn query_with_rowid_index(
    conn: &Connection,
    state: &DbState,
    table: &str,
    safe_table: &str,
    offset: i64,
    limit: i64,
    columns: Vec<String>,
) -> Option<Result<QueryResult, String>> {
    let chunk_idx = offset / limit;

    // Build index lazily on first access. The connection is opened with
    // ?immutable=1, so the snapshot is frozen for the connection's lifetime
    // and the cached index can never go stale until close_database.
    let mut indexes = state.rowid_indexes.lock();
    if !indexes.contains_key(table) {
        if let Some(idx) = build_rowid_index(conn, safe_table, limit) {
            indexes.insert(table.to_string(), idx);
        }
    }

    let idx = indexes.get(table)?;
    let total_rows = idx.total_rows;
    let chunk = chunk_idx as usize;

    if chunk >= idx.boundaries.len() {
        return None;
    }

    let start_rid = idx.boundaries[chunk];
    let (sql, query_params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if chunk + 1 < idx.boundaries.len() {
            let end_rid = idx.boundaries[chunk + 1];
            (
                format!("SELECT * FROM \"{}\" WHERE rowid >= ? AND rowid < ? ORDER BY rowid ASC", safe_table),
                vec![Box::new(start_rid), Box::new(end_rid)],
            )
        } else {
            (
                format!("SELECT * FROM \"{}\" WHERE rowid >= ? ORDER BY rowid ASC LIMIT ?", safe_table),
                vec![Box::new(start_rid), Box::new(limit)],
            )
        };

    drop(indexes); // release lock before querying

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = query_params.iter().map(|p| p.as_ref()).collect();
    let result = conn.prepare(&sql).str_err().and_then(|mut stmt| {
        let rows = collect_rows(&mut stmt, &param_refs)?;
        Ok(QueryResult { columns, rows, total_rows, offset })
    });
    Some(result)
}

/// Standard LIMIT/OFFSET query, with optional WHERE and ORDER BY.
#[allow(clippy::too_many_arguments)]
fn query_with_offset(
    conn: &Connection,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
    offset: i64,
    limit: i64,
    total_rows: i64,
    columns: Vec<String>,
) -> Result<QueryResult, String> {
    let sql = format!(
        "SELECT * FROM \"{}\"{}{} LIMIT ? OFFSET ?",
        safe_table, where_clause, order_clause
    );

    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = params
        .iter()
        .map(|p| Box::new(p.clone()) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    all_params.push(Box::new(limit));
    all_params.push(Box::new(offset));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        all_params.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql).str_err()?;
    let rows = collect_rows(&mut stmt, &param_refs)?;

    Ok(QueryResult { columns, rows, total_rows, offset })
}

/// Query a table with pagination, filtering, sorting, and regex support.
/// Dispatches to the optimal strategy based on filters and sort state.
pub fn query_table(
    state: &DbState,
    req: &QueryRequest,
) -> Result<QueryResult, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;

    let table = &req.table;
    let offset = req.offset;
    let limit = req.limit;
    let safe_table = safe_ident(table);
    let columns = get_column_names(conn, &safe_table)?;

    let WhereResult { clause: where_clause, params, regex_filters } =
        build_where_clause(&columns, &req.filters, &req.global_filter)?;

    let order_clause = match &req.sort_column {
        Some(col) => format!(
            " ORDER BY \"{}\" {}",
            safe_ident(col),
            if req.sort_asc { "ASC" } else { "DESC" }
        ),
        None => String::new(),
    };

    // Path 1: Regex filters require full scan + in-memory filtering
    if !regex_filters.is_empty() {
        return query_with_regex_filter(
            conn, &safe_table, &where_clause, &order_clause,
            &params, &regex_filters, offset, limit, columns,
        );
    }

    // Path 2: No filters — try rowid index for O(log n) seeks
    if where_clause.is_empty() {
        if req.sort_column.is_none() {
            if let Some(result) = query_with_rowid_index(
                conn, state, table, &safe_table, offset, limit, columns.clone(),
            ) {
                return result;
            }
        }

        // Fallback: custom sort or no rowid — use LIMIT/OFFSET with known count
        let total_rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", safe_table), [], |row| row.get(0))
            .str_err()?;

        return query_with_offset(
            conn, &safe_table, &where_clause, &order_clause,
            &params, offset, limit, total_rows, columns,
        );
    }

    // Path 3: With filters — LIMIT/OFFSET, count fetched separately by frontend
    query_with_offset(
        conn, &safe_table, &where_clause, &order_clause,
        &params, offset, limit, -1, columns,
    )
}

/// Separate count query — called asynchronously after data is displayed
pub fn count_rows(
    state: &DbState,
    table: &str,
    filters: &[ColumnFilter],
    global_filter: &str,
) -> Result<i64, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    let safe_table = safe_ident(table);
    let columns = get_column_names(conn, &safe_table)?;

    // Reuse shared helper; regex filters are ignored for count (applied in-memory by query_table)
    let WhereResult { clause: where_clause, params, .. } =
        build_where_clause(&columns, filters, global_filter)?;

    let sql = format!("SELECT COUNT(*) FROM \"{}\"{}",safe_table, where_clause);
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p as &dyn rusqlite::types::ToSql).collect();
    conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())
}

/// Classify a SQLite declared column type as numeric-affinity or not, using
/// the SQLite type-affinity rules (https://www.sqlite.org/datatype3.html §3.1).
/// Anything with TEXT/BLOB affinity — VARCHAR, CHAR, CLOB, TEXT, BLOB — stays
/// as a string in the xlsx so values like `123123123123` in a VARCHAR column
/// don't get coerced to an f64 and rendered in scientific notation by Excel.
fn is_numeric_affinity(declared: &str) -> bool {
    let t = declared.to_ascii_uppercase();
    // SQLite rule 1: INT wins first (e.g. `INTEGER`, `BIGINT`).
    if t.contains("INT") {
        return true;
    }
    // Rule 2: CHAR/CLOB/TEXT → TEXT affinity.
    if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        return false;
    }
    // Rule 3: BLOB or empty → BLOB affinity. Keep as string.
    if t.contains("BLOB") || t.is_empty() {
        return false;
    }
    // Rules 4 & 5: REAL/FLOA/DOUB or NUMERIC → numeric.
    true
}

pub fn export_to_xlsx(
    headers: &[String],
    rows: &[Vec<String>],
    column_types: &[String],
) -> Result<String, String> {
    use rust_xlsxwriter::*;

    if headers.is_empty() {
        return Err("No data to export".to_string());
    }

    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();

    // Pre-compute per-column "should parse as number" to avoid re-uppercasing
    // the declared type for every cell. If column_types is missing or shorter
    // than headers (defensive), default to numeric-parse for unknown columns
    // so existing callers that haven't been updated still behave as before.
    let numeric: Vec<bool> = (0..headers.len())
        .map(|i| column_types.get(i).is_none_or(|t| is_numeric_affinity(t)))
        .collect();

    // Write data first (headers + rows) so the table range is populated
    for (ci, h) in headers.iter().enumerate() {
        ws.write_string(0, ci as u16, h).str_err()?;
    }
    // Excel stores all numbers as IEEE-754 f64, so integers outside ±2^53
    // lose precision if written as numbers. Keep those as strings so values
    // like bigint IDs survive the round-trip exactly.
    const F64_EXACT_INT: i64 = 1i64 << 53;
    for (ri, row) in rows.iter().enumerate() {
        for (ci, val) in row.iter().enumerate() {
            // TEXT-affinity columns always write as string; numeric-affinity
            // columns try i64 first (so large IDs don't downcast to f64),
            // fall back to f64 for decimals, then to string for NULLs or
            // malformed values.
            if numeric.get(ci).copied().unwrap_or(true) {
                if let Ok(n) = val.parse::<i64>() {
                    if n.abs() <= F64_EXACT_INT {
                        ws.write_number((ri + 1) as u32, ci as u16, n as f64).str_err()?;
                    } else {
                        ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
                    }
                } else if let Ok(n) = val.parse::<f64>() {
                    ws.write_number((ri + 1) as u32, ci as u16, n).str_err()?;
                } else {
                    ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
                }
            } else {
                ws.write_string((ri + 1) as u32, ci as u16, val).str_err()?;
            }
        }
    }

    // Add table with "Dark Teal, Table Style Medium 2" = TableStyleMedium2
    let last_row = rows.len() as u32; // 0-indexed: header is row 0, last data row
    let last_col = if headers.is_empty() { 0 } else { (headers.len() - 1) as u16 };
    let columns: Vec<TableColumn> = headers.iter()
        .map(|h| TableColumn::new().set_header(h))
        .collect();
    let table = Table::new()
        .set_style(TableStyle::Medium2)
        .set_columns(&columns);
    ws.add_table(0, 0, last_row, last_col, &table)
        .str_err()?;

    // Autofit columns
    ws.autofit();

    // Save to temp file with timestamp for uniqueness across exports
    let temp_dir = std::env::temp_dir();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = temp_dir.join(format!("dblitz_export_{}.xlsx", ts));
    let path_str = path.to_string_lossy().to_string();
    wb.save(&path).str_err()?;

    Ok(path_str)
}

#[cfg(debug_assertions)]
#[derive(Debug, Serialize, Clone)]
pub struct BenchmarkResult {
    pub label: String,
    pub offset: i64,
    pub ms: f64,
    pub row_count: usize,
}

#[cfg(debug_assertions)]
pub fn benchmark_query(
    state: &DbState,
    table: &str,
    chunk_size: i64,
) -> Result<Vec<BenchmarkResult>, String> {
    use std::time::Instant;
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    let safe_table = safe_ident(table);

    let total: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", safe_table), [], |row| row.get(0))
        .str_err()?;

    let limit = chunk_size;
    let offsets: Vec<i64> = vec![0, total / 4, total / 2, total * 3 / 4];

    let mut results = Vec::new();

    // Method 1: LIMIT/OFFSET baseline
    for &off in &offsets {
        let sql = format!("SELECT * FROM \"{}\" LIMIT ? OFFSET ?", safe_table);
        let t0 = Instant::now();
        let mut stmt = conn.prepare(&sql).str_err()?;
        let col_count = stmt.column_count();
        let mut rows_iter = stmt.query(rusqlite::params![limit, off]).str_err()?;
        let mut count = 0usize;
        while let Some(row) = rows_iter.next().str_err()? {
            read_row(row, col_count);
            count += 1;
        }
        drop(rows_iter);
        drop(stmt);
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        results.push(BenchmarkResult {
            label: "LIMIT/OFFSET".to_string(), offset: off, ms: elapsed, row_count: count,
        });
    }

    // Method 2: rowid index seek
    // Build index if not already cached
    {
        let mut indexes = state.rowid_indexes.lock();
        if !indexes.contains_key(table) {
            let t0 = Instant::now();
            if let Some(idx) = build_rowid_index(conn, &safe_table, limit) {
                let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                results.push(BenchmarkResult {
                    label: "index build".to_string(), offset: 0, ms: build_ms,
                    row_count: idx.boundaries.len(),
                });
                indexes.insert(table.to_string(), idx);
            }
        }
    }

    let indexes = state.rowid_indexes.lock();
    if let Some(idx) = indexes.get(table) {
        for &off in &offsets {
            let chunk = (off / limit) as usize;
            if chunk >= idx.boundaries.len() { continue; }
            let start_rid = idx.boundaries[chunk];

            let t0 = Instant::now();
            let (sql, p1, p2): (String, i64, i64) = if chunk + 1 < idx.boundaries.len() {
                let end_rid = idx.boundaries[chunk + 1];
                (format!("SELECT * FROM \"{}\" WHERE rowid >= ? AND rowid < ? ORDER BY rowid ASC", safe_table), start_rid, end_rid)
            } else {
                (format!("SELECT * FROM \"{}\" WHERE rowid >= ? ORDER BY rowid ASC LIMIT ?", safe_table), start_rid, limit)
            };

            let mut stmt = conn.prepare(&sql).str_err()?;
            let col_count = stmt.column_count();
            let mut rows_iter = stmt.query(rusqlite::params![p1, p2]).str_err()?;
            let mut count = 0usize;
            while let Some(row) = rows_iter.next().str_err()? {
                read_row(row, col_count);
                count += 1;
            }
            drop(rows_iter);
            drop(stmt);
            let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
            results.push(BenchmarkResult {
                label: "rowid index".to_string(), offset: off, ms: elapsed, row_count: count,
            });
        }
    }

    Ok(results)
}

const SQL_RESULT_LIMIT: usize = 10_000;

pub fn execute_sql(state: &DbState, sql: &str) -> SqlResult {
    let guard = state.conn.lock();
    let conn = match guard.as_ref() {
        Some(c) => c,
        None => {
            return SqlResult {
                columns: vec![], rows: vec![], rows_affected: 0,
                error: Some("No database open".to_string()),
            }
        }
    };

    let trimmed = sql.trim();

    // dblitz is read-only. The connection is opened with SQLITE_OPEN_READ_ONLY
    // so any mutation attempt would fail at the SQLite level anyway, but we
    // reject it explicitly here to give the user a clear, actionable message
    // instead of "attempt to write a readonly database".
    let mut stmt = match conn.prepare(trimmed) {
        Ok(s) => s,
        Err(e) => {
            return SqlResult {
                columns: vec![], rows: vec![], rows_affected: 0,
                error: Some(e.to_string()),
            };
        }
    };

    if !stmt.readonly() {
        return SqlResult {
            columns: vec![], rows: vec![], rows_affected: 0,
            error: Some(
                "dblitz is a read-only viewer — write statements (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER, etc.) are not supported.".to_string(),
            ),
        };
    }

    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let col_count = columns.len();
    return match stmt.query([]) {
        Ok(mut rows_iter) => {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut truncated = false;
            loop {
                if rows.len() >= SQL_RESULT_LIMIT {
                    truncated = true;
                    break;
                }
                match rows_iter.next() {
                    Ok(Some(row)) => rows.push(read_row(row, col_count)),
                    Ok(None) => break,
                    Err(e) => {
                        return SqlResult { columns, rows, rows_affected: 0, error: Some(e.to_string()) };
                    }
                }
            }
            let error = if truncated {
                Some(format!("Result truncated to {} rows", SQL_RESULT_LIMIT))
            } else {
                None
            };
            SqlResult { columns, rows, rows_affected: 0, error }
        }
        Err(e) => SqlResult { columns: vec![], rows: vec![], rows_affected: 0, error: Some(e.to_string()) },
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter { column: column.to_string(), value: value.to_string(), is_regex: false }
    }

    fn regex_filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter { column: column.to_string(), value: value.to_string(), is_regex: true }
    }

    #[test]
    fn basic_contains_filter() {
        let columns = cols(&["name", "age"]);
        let filters = vec![filter("name", "foo")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" LIKE ?");
        assert_eq!(r.params, vec!["%foo%"]);
        assert!(r.regex_filters.is_empty());
    }

    #[test]
    fn global_filter_or_across_columns() {
        let columns = cols(&["name", "age"]);
        let r = build_where_clause(&columns, &[], "test").unwrap();
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? OR \"age\" LIKE ?)");
        assert_eq!(r.params, vec!["%test%", "%test%"]);
    }

    #[test]
    fn semicolon_multi_criteria() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "foo;bar")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? OR \"name\" LIKE ?)");
        assert_eq!(r.params, vec!["%foo%", "%bar%"]);
    }

    #[test]
    fn exclusion_not_like() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "<>bad")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" NOT LIKE ?");
        assert_eq!(r.params, vec!["%bad%"]);
    }

    #[test]
    fn bare_exclusion_non_empty() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "<>")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"name\" IS NOT NULL AND \"name\" != ''");
        assert!(r.params.is_empty());
    }

    #[test]
    fn comparison_operators() {
        let columns = cols(&["price"]);

        let r = build_where_clause(&columns, &[filter("price", ">100")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" > ?");
        assert_eq!(r.params, vec!["100"]);

        let r = build_where_clause(&columns, &[filter("price", "<=50")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" <= ?");
        assert_eq!(r.params, vec!["50"]);

        let r = build_where_clause(&columns, &[filter("price", ">=10")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" >= ?");
        assert_eq!(r.params, vec!["10"]);

        let r = build_where_clause(&columns, &[filter("price", "<5")], "").unwrap();
        assert_eq!(r.clause, " WHERE \"price\" < ?");
        assert_eq!(r.params, vec!["5"]);
    }

    #[test]
    fn exact_match_operator() {
        let columns = cols(&["status"]);
        let filters = vec![filter("status", "=active")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"status\" = ?");
        assert_eq!(r.params, vec!["active"]);
    }

    #[test]
    fn invalid_regex_returns_error() {
        let columns = cols(&["name"]);
        let filters = vec![regex_filter("name", "[invalid")];
        let r = build_where_clause(&columns, &filters, "");
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Invalid regex"));
    }

    #[test]
    fn valid_regex_produces_regex_filter() {
        let columns = cols(&["name", "age"]);
        let filters = vec![regex_filter("name", "^foo.*bar$")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert!(r.clause.is_empty());
        assert!(r.params.is_empty());
        assert_eq!(r.regex_filters.len(), 1);
        assert_eq!(r.regex_filters[0].0, 0); // column index
    }

    #[test]
    fn column_name_with_quotes_is_escaped() {
        let columns = cols(&["col\"name"]);
        let filters = vec![filter("col\"name", "test")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE \"col\"\"name\" LIKE ?");
        assert_eq!(r.params, vec!["%test%"]);
    }

    #[test]
    fn empty_filter_value_is_skipped() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert!(r.clause.is_empty());
        assert!(r.params.is_empty());
    }

    #[test]
    fn mixed_include_and_exclude_with_semicolons() {
        let columns = cols(&["name"]);
        let filters = vec![filter("name", "good;<>bad")];
        let r = build_where_clause(&columns, &filters, "").unwrap();
        assert_eq!(r.clause, " WHERE (\"name\" LIKE ? AND \"name\" NOT LIKE ?)");
        assert_eq!(r.params, vec!["%good%", "%bad%"]);
    }

    #[test]
    fn safe_ident_escapes_quotes() {
        assert_eq!(safe_ident("normal"), "normal");
        assert_eq!(safe_ident("has\"quote"), "has\"\"quote");
        assert_eq!(safe_ident("two\"\"quotes"), "two\"\"\"\"quotes");
    }

    #[test]
    fn path_to_sqlite_uri_encodes_special_chars() {
        // Unix-style path
        assert_eq!(
            path_to_sqlite_uri("/home/user/db.sqlite"),
            "file:/home/user/db.sqlite?immutable=1"
        );
        // Windows path with backslashes — normalized to forward slashes
        assert_eq!(
            path_to_sqlite_uri(r"C:\Users\mail\db.sqlite"),
            "file:/C:/Users/mail/db.sqlite?immutable=1"
        );
        // Spaces percent-encoded
        assert_eq!(
            path_to_sqlite_uri(r"C:\foo bar\db.sqlite"),
            "file:/C:/foo%20bar/db.sqlite?immutable=1"
        );
        // # and ? are URI-special and must be encoded
        assert_eq!(
            path_to_sqlite_uri(r"C:\with#hash\db.sqlite"),
            "file:/C:/with%23hash/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\with?question\db.sqlite"),
            "file:/C:/with%3Fquestion/db.sqlite?immutable=1"
        );
        // % must be encoded first so we don't double-escape our own escapes
        assert_eq!(
            path_to_sqlite_uri(r"C:\with%percent\db.sqlite"),
            "file:/C:/with%25percent/db.sqlite?immutable=1"
        );
    }

    /// Set up a temp SQLite file with one table, then open it via dblitz's
    /// production code path. Returns (state, path) — caller must clean up
    /// the file after closing the state.
    fn setup_temp_db_with_table() -> (DbState, std::path::PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dblitz_test_{}.sqlite", nanos));

        // Create the table on a separate writable connection (NOT via dblitz code).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
                .unwrap();
            conn.execute("INSERT INTO users (name) VALUES ('alice')", [])
                .unwrap();
        }

        // Now open via the production read-only + immutable code path.
        let state = DbState::new();
        open_database(&state, path.to_str().unwrap()).unwrap();
        (state, path)
    }

    #[test]
    fn execute_sql_rejects_writes_with_friendly_message() {
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "DELETE FROM users");

        assert!(result.error.is_some(), "expected DELETE to be rejected");
        let err = result.error.unwrap();
        assert!(
            err.contains("read-only viewer"),
            "expected friendly read-only message, got: {err}"
        );
        assert!(result.rows.is_empty());
        assert_eq!(result.rows_affected, 0);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_allows_select_on_readonly_connection() {
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "SELECT name FROM users");

        assert!(result.error.is_none(), "SELECT should succeed, got: {:?}", result.error);
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0].as_deref(), Some("alice"));

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }
}
