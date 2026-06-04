use regex::Regex;
use rusqlite::Connection;
use std::sync::atomic::Ordering;

use super::filters::{build_where_clause, WhereResult};
use super::types::{ColumnFilter, DbState, QueryRequest, QueryResult, RowidIndex};
use super::util::{collect_rows, read_row, safe_ident, StrErr};

/// Build a sparse rowid index for a table: sample the rowid at every chunk_size boundary.
/// This turns OFFSET-based queries into O(log n) rowid seeks.
pub(super) fn build_rowid_index(
    conn: &Connection,
    safe_table: &str,
    chunk_size: i64,
) -> Option<RowidIndex> {
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
        chunk_size,
    })
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

#[allow(clippy::too_many_arguments)]
fn query_with_regex_filter(
    conn: &Connection,
    state: &DbState,
    generation: u64,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
    regex_filters: &[(usize, Regex)],
    offset: i64,
    limit: i64,
    columns: Vec<String>,
) -> Result<QueryResult, String> {
    let sql = format!(
        "SELECT * FROM \"{}\"{}{}",
        safe_table, where_clause, order_clause
    );
    let mut stmt = conn.prepare(&sql).str_err()?;
    let col_count = stmt.column_count();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|p| p as &dyn rusqlite::types::ToSql)
        .collect();

    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut matched = 0i64;
    let mut rows_iter = stmt.query(param_refs.as_slice()).str_err()?;

    while let Some(row) = rows_iter.next().str_err()? {
        if state.query_generation.load(Ordering::Relaxed) != generation {
            return Err("Query cancelled by a newer request".to_string());
        }
        let values = read_row(row, col_count);
        let matches = regex_filters.iter().all(|(idx, re)| {
            values
                .get(*idx)
                .and_then(|v| v.as_ref())
                .map(|s| re.is_match(s))
                .unwrap_or(false)
        });
        if matches {
            if matched >= offset && rows.len() < limit as usize {
                rows.push(values);
            }
            matched += 1;
        }
    }

    Ok(QueryResult {
        columns,
        rows,
        total_rows: Some(matched),
        offset,
    })
}

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

    let mut indexes = state.rowid_indexes.lock();
    if indexes
        .get(table)
        .is_some_and(|idx| idx.chunk_size != limit)
    {
        indexes.remove(table);
    }
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
                format!(
                    "SELECT * FROM \"{}\" WHERE rowid >= ? AND rowid < ? ORDER BY rowid ASC",
                    safe_table
                ),
                vec![Box::new(start_rid), Box::new(end_rid)],
            )
        } else {
            (
                format!(
                    "SELECT * FROM \"{}\" WHERE rowid >= ? ORDER BY rowid ASC LIMIT ?",
                    safe_table
                ),
                vec![Box::new(start_rid), Box::new(limit)],
            )
        };

    drop(indexes);

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        query_params.iter().map(|p| p.as_ref()).collect();
    let result = conn.prepare(&sql).str_err().and_then(|mut stmt| {
        let rows = collect_rows(&mut stmt, &param_refs)?;
        Ok(QueryResult {
            columns,
            rows,
            total_rows: Some(total_rows),
            offset,
        })
    });
    Some(result)
}

#[allow(clippy::too_many_arguments)]
fn query_with_offset(
    conn: &Connection,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
    offset: i64,
    limit: i64,
    total_rows: Option<i64>,
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

    Ok(QueryResult {
        columns,
        rows,
        total_rows,
        offset,
    })
}

/// Defensive ceiling on a single page request. The UI only ever asks for a
/// fixed chunk size, so this is insurance against a caller passing a value that
/// would materialize an unreasonable number of rows into memory at once.
const MAX_QUERY_LIMIT: i64 = 100_000;

pub fn query_table(state: &DbState, req: &QueryRequest) -> Result<QueryResult, String> {
    if req.limit <= 0 {
        return Err("Query limit must be greater than zero".to_string());
    }
    if req.limit > MAX_QUERY_LIMIT {
        return Err(format!("Query limit must not exceed {MAX_QUERY_LIMIT}"));
    }
    if req.offset < 0 {
        return Err("Query offset must be zero or greater".to_string());
    }

    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    let generation = state.query_generation.load(Ordering::Relaxed);

    let table = &req.table;
    let offset = req.offset;
    let limit = req.limit;
    let safe_table = safe_ident(table);
    let columns = get_column_names(conn, &safe_table)?;

    let WhereResult {
        clause: where_clause,
        params,
        regex_filters,
    } = build_where_clause(&columns, &req.filters, &req.global_filter)?;

    let valid_sort_column = req
        .sort_column
        .as_ref()
        .filter(|col| columns.iter().any(|c| c == *col));

    let order_clause = match valid_sort_column {
        Some(col) => format!(
            " ORDER BY \"{}\" {}",
            safe_ident(col),
            if req.sort_asc { "ASC" } else { "DESC" }
        ),
        None => String::new(),
    };

    if !regex_filters.is_empty() {
        return query_with_regex_filter(
            conn,
            state,
            generation,
            &safe_table,
            &where_clause,
            &order_clause,
            &params,
            &regex_filters,
            offset,
            limit,
            columns,
        );
    }

    if where_clause.is_empty() {
        if req.sort_column.is_none() && offset % limit == 0 {
            if let Some(result) = query_with_rowid_index(
                conn,
                state,
                table,
                &safe_table,
                offset,
                limit,
                columns.clone(),
            ) {
                return result;
            }
        }

        let total_rows: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM \"{}\"", safe_table),
                [],
                |row| row.get(0),
            )
            .str_err()?;

        return query_with_offset(
            conn,
            &safe_table,
            &where_clause,
            &order_clause,
            &params,
            offset,
            limit,
            Some(total_rows),
            columns,
        );
    }

    query_with_offset(
        conn,
        &safe_table,
        &where_clause,
        &order_clause,
        &params,
        offset,
        limit,
        None,
        columns,
    )
}

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

    let WhereResult {
        clause: where_clause,
        params,
        ..
    } = build_where_clause(&columns, filters, global_filter)?;

    let sql = format!("SELECT COUNT(*) FROM \"{}\"{}", safe_table, where_clause);
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|p| p as &dyn rusqlite::types::ToSql)
        .collect();
    conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::open_database;
    use rusqlite::params;
    use tempfile::TempDir;

    fn regex_filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: true,
        }
    }

    fn state_with_memory_db(sql: &str) -> DbState {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(sql).unwrap();
        let state = DbState::new();
        *state.conn.lock() = Some(conn);
        state
    }

    fn temp_db_with_items(dir: &TempDir, name: &str, count: usize) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);")
            .unwrap();
        for idx in 0..count {
            conn.execute(
                "INSERT INTO items (name) VALUES (?)",
                params![format!("{name}-{idx}")],
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn query_table_ignores_stale_sort_column() {
        let state = state_with_memory_db(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users (name) VALUES ('alice'), ('bravo');",
        );
        let req = QueryRequest {
            table: "users".to_string(),
            offset: 0,
            limit: 10,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("missing_column".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.columns, vec!["id", "name"]);
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn regex_query_returns_only_requested_page_with_total_match_count() {
        let state = state_with_memory_db("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            for idx in 0..1_200 {
                tx.execute(
                    "INSERT INTO users (name) VALUES (?)",
                    [format!("match-{idx}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let req = QueryRequest {
            table: "users".to_string(),
            offset: 5,
            limit: 7,
            filters: vec![regex_filter("name", "^match-")],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.total_rows, Some(1_200));
        assert_eq!(result.rows.len(), 7);
        assert_eq!(result.rows[0][1].as_deref(), Some("match-5"));
        assert_eq!(result.rows[6][1].as_deref(), Some("match-11"));
    }

    #[test]
    fn rowid_index_rebuilds_when_chunk_size_changes() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            for idx in 0..1_500 {
                tx.execute(
                    "INSERT INTO items (name) VALUES (?)",
                    [format!("item-{idx}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }

        let first = QueryRequest {
            table: "items".to_string(),
            offset: 500,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };
        let second = QueryRequest {
            table: "items".to_string(),
            offset: 1_000,
            limit: 1_000,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        assert_eq!(
            query_table(&state, &first).unwrap().rows[0][1].as_deref(),
            Some("item-500")
        );
        assert_eq!(
            query_table(&state, &second).unwrap().rows[0][1].as_deref(),
            Some("item-1000")
        );
    }

    #[test]
    fn open_database_clears_rowid_indexes() {
        let dir = TempDir::new().unwrap();
        let first_path = temp_db_with_items(&dir, "first.sqlite", 1_500);
        let second_path = temp_db_with_items(&dir, "second.sqlite", 700);
        let state = DbState::new();

        open_database(&state, first_path.to_str().unwrap()).unwrap();
        let first = QueryRequest {
            table: "items".to_string(),
            offset: 1_000,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };
        assert_eq!(query_table(&state, &first).unwrap().total_rows, Some(1_500));

        open_database(&state, second_path.to_str().unwrap()).unwrap();
        let second = QueryRequest {
            table: "items".to_string(),
            offset: 0,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };
        let result = query_table(&state, &second).unwrap();

        assert_eq!(result.total_rows, Some(700));
        assert_eq!(result.rows[0][1].as_deref(), Some("second.sqlite-0"));
    }

    #[test]
    fn query_table_rejects_zero_limit() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 0,
            limit: 0,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        let err = query_table(&state, &req).unwrap_err();

        assert!(err.contains("limit"));
    }

    #[test]
    fn query_table_rejects_limit_above_ceiling() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 0,
            limit: MAX_QUERY_LIMIT + 1,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        let err = query_table(&state, &req).unwrap_err();

        assert!(err.contains("exceed"), "got: {err}");
    }

    #[test]
    fn query_table_non_boundary_offset_uses_requested_page() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            for idx in 0..1_000 {
                tx.execute(
                    "INSERT INTO items (name) VALUES (?)",
                    [format!("item-{idx}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 250,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.rows[0][1].as_deref(), Some("item-250"));
    }
}
