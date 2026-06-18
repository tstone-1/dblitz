use regex::Regex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::filters::{build_where_clause, WhereResult};
use super::types::{
    ColumnFilter, DbState, FilteredOrder, QueryRequest, QueryResult, RowidIndex, SortedOrder,
};
use super::util::{collect_rows, read_row, safe_ident, StrErr};

/// Build a sparse rowid index for a table: sample the rowid at every chunk_size boundary.
/// This turns OFFSET-based queries into O(log n) rowid seeks.
pub(super) fn build_rowid_index(
    conn: &Connection,
    state: &DbState,
    generation: u64,
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
        if state.query_generation.load(Ordering::Relaxed) != generation {
            return None;
        }
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
    generation: u64,
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
        if let Some(idx) = build_rowid_index(conn, state, generation, safe_table, limit) {
            indexes.insert(table.to_string(), idx);
        } else if state.query_generation.load(Ordering::Relaxed) != generation {
            return Some(Err("Query cancelled by a newer request".to_string()));
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

/// Materialize the full rowid order for one sort key with a single
/// `ORDER BY`. Returns `Ok(None)` if a newer request bumps the generation
/// mid-build. Note: SQLite materializes a non-indexed sort on the first
/// `next()`, so this only bails *during row collection* once superseded — it
/// cannot interrupt the sort step itself.
fn build_sorted_order(
    conn: &Connection,
    state: &DbState,
    generation: u64,
    safe_table: &str,
    sort_column: &str,
    sort_asc: bool,
) -> Result<Option<Vec<i64>>, String> {
    let sql = format!(
        "SELECT rowid FROM \"{}\" ORDER BY \"{}\" {}",
        safe_table,
        safe_ident(sort_column),
        if sort_asc { "ASC" } else { "DESC" }
    );
    let mut stmt = conn.prepare(&sql).str_err()?;
    let mut rows_iter = stmt.query([]).str_err()?;
    let mut rowids: Vec<i64> = Vec::new();
    while let Some(row) = rows_iter.next().str_err()? {
        if state.query_generation.load(Ordering::Relaxed) != generation {
            return Ok(None);
        }
        rowids.push(row.get(0).str_err()?);
    }
    Ok(Some(rowids))
}

/// Fetch the given rowids and return their rows in the requested order.
/// `WHERE rowid IN (...)` doesn't preserve order, so we index by rowid and
/// re-emit by the caller's sequence.
fn fetch_rows_by_rowids(
    conn: &Connection,
    safe_table: &str,
    rowids: &[i64],
) -> Result<Vec<Vec<Option<String>>>, String> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }
    // Stay under the oldest `SQLITE_MAX_VARIABLE_NUMBER` (999) so an arbitrarily
    // large page (callers may request up to MAX_QUERY_LIMIT) can't blow the
    // bound-parameter limit. Order is reconstructed from `rowids` afterwards,
    // so batch boundaries don't matter.
    const BATCH: usize = 900;
    let mut by_rowid: HashMap<i64, Vec<Option<String>>> = HashMap::with_capacity(rowids.len());

    for batch in rowids.chunks(BATCH) {
        let mut placeholders = String::with_capacity(batch.len() * 2);
        for i in 0..batch.len() {
            if i > 0 {
                placeholders.push(',');
            }
            placeholders.push('?');
        }
        let sql = format!(
            "SELECT rowid, * FROM \"{}\" WHERE rowid IN ({})",
            safe_table, placeholders
        );
        let mut stmt = conn.prepare(&sql).str_err()?;
        let col_count = stmt.column_count();
        let params: Vec<&dyn rusqlite::types::ToSql> = batch
            .iter()
            .map(|r| r as &dyn rusqlite::types::ToSql)
            .collect();

        let mut rows_iter = stmt.query(params.as_slice()).str_err()?;
        while let Some(row) = rows_iter.next().str_err()? {
            let rid: i64 = row.get(0).str_err()?;
            // read_row includes the leading rowid at index 0; drop it so the
            // emitted row aligns with the table's declared columns.
            let full = read_row(row, col_count);
            by_rowid.insert(rid, full[1..].to_vec());
        }
    }

    let mut out = Vec::with_capacity(rowids.len());
    for rid in rowids {
        if let Some(r) = by_rowid.remove(rid) {
            out.push(r);
        }
    }
    Ok(out)
}

/// Serve a sorted, unfiltered page from the cached rowid order, building the
/// order on first use (or when the sort key changes). Turns each chunk into a
/// rowid lookup instead of a fresh full-table `ORDER BY`.
///
/// Returns `None` when the table has no usable rowid (`WITHOUT ROWID`),
/// signalling the caller to fall back to an `ORDER BY` + `OFFSET` query.
/// `Some(Err(..))` is a real failure or a cancellation.
#[allow(clippy::too_many_arguments)]
fn query_with_sorted_order(
    conn: &Connection,
    state: &DbState,
    generation: u64,
    table: &str,
    safe_table: &str,
    sort_column: &str,
    sort_asc: bool,
    offset: i64,
    limit: i64,
    columns: Vec<String>,
) -> Option<Result<QueryResult, String>> {
    // WITHOUT ROWID tables have no `rowid` column, so the cache can't address
    // their rows. Probe cheaply and bail to the offset path. Mirrors the guard
    // in build_rowid_index.
    if conn
        .prepare(&format!("SELECT rowid FROM \"{}\" LIMIT 0", safe_table))
        .is_err()
    {
        return None;
    }

    let mut orders = state.sorted_orders.lock();
    let fresh = orders
        .get(table)
        .is_some_and(|o| o.sort_column == sort_column && o.sort_asc == sort_asc);
    if !fresh {
        match build_sorted_order(conn, state, generation, safe_table, sort_column, sort_asc) {
            Ok(Some(rowids)) => {
                tracing::debug!(
                    table,
                    rows = rowids.len(),
                    sort_column,
                    sort_asc,
                    "built sorted order"
                );
                orders.insert(
                    table.to_string(),
                    SortedOrder {
                        sort_column: sort_column.to_string(),
                        sort_asc,
                        rowids,
                    },
                );
            }
            Ok(None) => return Some(Err("Query cancelled by a newer request".to_string())),
            Err(e) => return Some(Err(e)),
        }
    }

    let order = match orders.get(table) {
        Some(o) => o,
        None => return Some(Err("Sorted order missing after build".to_string())),
    };
    let total_rows = order.rowids.len() as i64;
    let start = offset.min(total_rows) as usize;
    let end = offset.saturating_add(limit).min(total_rows) as usize;
    let page: Vec<i64> = order.rowids[start..end].to_vec();
    drop(orders);

    Some(
        fetch_rows_by_rowids(conn, safe_table, &page).map(|rows| QueryResult {
            columns,
            rows,
            total_rows: Some(total_rows),
            offset,
        }),
    )
}

/// Materialize the ordered rowid list for a filtered (and optionally sorted)
/// view with a single scan. Returns `Ok(None)` if a newer request bumps the
/// generation mid-build. Like [`build_sorted_order`], SQLite materializes a
/// non-indexed `ORDER BY` on the first `next()`, so cancellation only takes
/// effect during row collection, not during the sort step itself.
fn build_filtered_order(
    conn: &Connection,
    state: &DbState,
    generation: u64,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
) -> Result<Option<Vec<i64>>, String> {
    // With no sort key, page in stable rowid order so cached chunks are
    // deterministic — this matches the natural scan order the un-cached OFFSET
    // path already produces for a rowid table.
    let effective_order = if order_clause.is_empty() {
        " ORDER BY rowid ASC"
    } else {
        order_clause
    };
    let sql = format!(
        "SELECT rowid FROM \"{}\"{}{}",
        safe_table, where_clause, effective_order
    );
    let mut stmt = conn.prepare(&sql).str_err()?;
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|p| p as &dyn rusqlite::types::ToSql)
        .collect();
    let mut rows_iter = stmt.query(param_refs.as_slice()).str_err()?;
    let mut rowids: Vec<i64> = Vec::new();
    while let Some(row) = rows_iter.next().str_err()? {
        if state.query_generation.load(Ordering::Relaxed) != generation {
            return Ok(None);
        }
        rowids.push(row.get(0).str_err()?);
    }
    Ok(Some(rowids))
}

/// Serve a filtered (and optionally sorted) page from a cached ordered-rowid
/// list, building it on first use (or when the filter/sort signature changes).
/// Turns each scroll chunk into a rowid lookup instead of a fresh
/// `WHERE` + `ORDER BY` + `OFFSET` scan whose cost grows with the offset.
///
/// The total match count falls out of the materialized list for free, so the
/// frontend no longer has to fire a separate `count_rows` scan for filtered
/// views. Returns `None` when the table has no usable rowid (`WITHOUT ROWID`),
/// signalling the caller to fall back to the `OFFSET` path. `Some(Err(..))` is
/// a real failure or a cancellation.
#[allow(clippy::too_many_arguments)]
fn query_with_filtered_order(
    conn: &Connection,
    state: &DbState,
    generation: u64,
    table: &str,
    safe_table: &str,
    where_clause: &str,
    order_clause: &str,
    params: &[String],
    offset: i64,
    limit: i64,
    columns: Vec<String>,
) -> Option<Result<QueryResult, String>> {
    // WITHOUT ROWID tables can't be addressed by rowid -> fall back to the
    // OFFSET path. Mirrors the guard in build_rowid_index / query_with_sorted_order.
    if conn
        .prepare(&format!("SELECT rowid FROM \"{}\" LIMIT 0", safe_table))
        .is_err()
    {
        return None;
    }

    // NUL separators keep the three components unambiguous regardless of their
    // contents (a clause/param can't contain a NUL byte).
    let signature = format!("{where_clause}\u{0}{params:?}\u{0}{order_clause}");

    let mut orders = state.filtered_orders.lock();
    let fresh = orders.get(table).is_some_and(|o| o.signature == signature);
    if !fresh {
        match build_filtered_order(
            conn,
            state,
            generation,
            safe_table,
            where_clause,
            order_clause,
            params,
        ) {
            Ok(Some(rowids)) => {
                tracing::debug!(table, rows = rowids.len(), "built filtered order");
                orders.insert(
                    table.to_string(),
                    FilteredOrder {
                        signature: signature.clone(),
                        rowids,
                    },
                );
            }
            Ok(None) => return Some(Err("Query cancelled by a newer request".to_string())),
            Err(e) => return Some(Err(e)),
        }
    }

    let order = match orders.get(table) {
        Some(o) => o,
        None => return Some(Err("Filtered order missing after build".to_string())),
    };
    let total_rows = order.rowids.len() as i64;
    let start = offset.min(total_rows) as usize;
    let end = offset.saturating_add(limit).min(total_rows) as usize;
    let page: Vec<i64> = order.rowids[start..end].to_vec();
    drop(orders);

    Some(
        fetch_rows_by_rowids(conn, safe_table, &page).map(|rows| QueryResult {
            columns,
            rows,
            total_rows: Some(total_rows),
            offset,
        }),
    )
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
        // Sorted, unfiltered: serve pages from a cached rowid order so each
        // chunk is a rowid lookup, not a fresh full-table ORDER BY. A stale
        // sort column (valid_sort_column == None) falls through to the
        // unsorted fast paths below. A WITHOUT ROWID table returns None and
        // falls through to the ORDER BY + OFFSET path (order_clause is set).
        if let Some(col) = valid_sort_column {
            if let Some(result) = query_with_sorted_order(
                conn,
                state,
                generation,
                table,
                &safe_table,
                col,
                req.sort_asc,
                offset,
                limit,
                columns.clone(),
            ) {
                return result;
            }
        } else if offset % limit == 0 {
            if let Some(result) = query_with_rowid_index(
                conn,
                state,
                generation,
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

    // Filtered (non-regex), sorted or not: serve pages from a cached ordered-
    // rowid list so each scroll chunk is a rowid lookup, not a fresh
    // WHERE + ORDER BY + OFFSET scan whose cost grows with the offset. The total
    // count comes from the materialized list, so this path also reports
    // `total_rows`, sparing the frontend a separate count_rows scan. A
    // WITHOUT ROWID table returns None and falls through to the OFFSET path.
    if let Some(result) = query_with_filtered_order(
        conn,
        state,
        generation,
        table,
        &safe_table,
        &where_clause,
        &order_clause,
        &params,
        offset,
        limit,
        columns.clone(),
    ) {
        return result;
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
        regex_filters,
    } = build_where_clause(&columns, filters, global_filter)?;
    if !regex_filters.is_empty() {
        return Err("count_rows does not support regex filters".to_string());
    }

    let sql = format!("SELECT COUNT(*) FROM \"{}\"{}", safe_table, where_clause);
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|p| p as &dyn rusqlite::types::ToSql)
        .collect();
    conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))
        .str_err()
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

    fn text_filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: false,
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
    fn sorted_query_pages_in_sort_order_via_cache() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, n INTEGER);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            // Insert in reverse so insertion (rowid) order differs from sort order.
            for n in (0..1_500).rev() {
                tx.execute("INSERT INTO items (n) VALUES (?)", [n]).unwrap();
            }
            tx.commit().unwrap();
        }

        let page = |offset: i64, asc: bool| QueryRequest {
            table: "items".to_string(),
            offset,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("n".to_string()),
            sort_asc: asc,
        };

        // Ascending: first page starts at 0, second page at 500.
        let first = query_table(&state, &page(0, true)).unwrap();
        assert_eq!(first.total_rows, Some(1_500));
        assert_eq!(first.rows.len(), 500);
        assert_eq!(first.rows[0][1].as_deref(), Some("0"));
        assert_eq!(first.rows[499][1].as_deref(), Some("499"));

        // Scroll to the bottom: last page is served from the same cached order.
        let last = query_table(&state, &page(1_000, true)).unwrap();
        assert_eq!(last.rows.len(), 500);
        assert_eq!(last.rows[0][1].as_deref(), Some("1000"));
        assert_eq!(last.rows[499][1].as_deref(), Some("1499"));

        // Flipping the direction rebuilds the cache and reverses the order.
        let desc = query_table(&state, &page(0, false)).unwrap();
        assert_eq!(desc.rows[0][1].as_deref(), Some("1499"));
        assert_eq!(desc.rows[499][1].as_deref(), Some("1000"));
    }

    #[test]
    fn sorted_query_clamps_offset_past_end() {
        let state = state_with_memory_db(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, n INTEGER);
             INSERT INTO items (n) VALUES (3), (1), (2);",
        );
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 500,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("n".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.total_rows, Some(3));
        assert!(result.rows.is_empty());
    }

    #[test]
    fn sorted_query_falls_back_on_without_rowid_table() {
        // WITHOUT ROWID tables have no rowid, so the sorted-order cache can't
        // address them. The query must fall back to ORDER BY + OFFSET and still
        // return correctly ordered rows (regression: this used to error out).
        let state = state_with_memory_db(
            "CREATE TABLE t (k TEXT PRIMARY KEY, v INTEGER) WITHOUT ROWID;
             INSERT INTO t (k, v) VALUES ('c', 3), ('a', 1), ('b', 2);",
        );
        let req = QueryRequest {
            table: "t".to_string(),
            offset: 0,
            limit: 10,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("v".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.total_rows, Some(3));
        let ks: Vec<_> = result.rows.iter().map(|r| r[0].as_deref()).collect();
        assert_eq!(ks, vec![Some("a"), Some("b"), Some("c")]);
    }

    #[test]
    fn filtered_sorted_query_pages_via_cache() {
        // Mirrors the reported slow scenario: a global filter AND a column
        // filter AND a sort on another column, then scrolling deep. Every chunk
        // must come from one materialized ordered-rowid list, not a re-run
        // WHERE + ORDER BY + OFFSET scan, and the total count must be exact so
        // the frontend skips its separate count_rows scan.
        let state = state_with_memory_db(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, tag TEXT, n INTEGER);",
        );
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            // Insert in reverse so rowid order differs from sort order. Even n ->
            // "keep", odd -> "drop"; every row is tagged "SOT" so the global
            // filter matches all rows and only the column filter narrows the set.
            for n in (0..1_500).rev() {
                let name = format!("{}-{}", if n % 2 == 0 { "keep" } else { "drop" }, n);
                tx.execute(
                    "INSERT INTO items (name, tag, n) VALUES (?, 'SOT', ?)",
                    params![name, n],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }

        // Column index: 0=id, 1=name, 2=tag, 3=n.
        let page = |offset: i64| QueryRequest {
            table: "items".to_string(),
            offset,
            limit: 500,
            filters: vec![text_filter("name", "keep")],
            global_filter: "SOT".to_string(),
            sort_column: Some("n".to_string()),
            sort_asc: true,
        };

        // 750 even values match, sorted ascending. Count is exact and present.
        let first = query_table(&state, &page(0)).unwrap();
        assert_eq!(first.total_rows, Some(750));
        assert_eq!(first.rows.len(), 500);
        assert_eq!(first.rows[0][3].as_deref(), Some("0"));
        assert_eq!(first.rows[499][3].as_deref(), Some("998"));

        // Deep page served from the same cached order — the part that used to lag.
        let deep = query_table(&state, &page(500)).unwrap();
        assert_eq!(deep.total_rows, Some(750));
        assert_eq!(deep.rows.len(), 250);
        assert_eq!(deep.rows[0][3].as_deref(), Some("1000"));
        assert_eq!(deep.rows[249][3].as_deref(), Some("1498"));
    }

    #[test]
    fn filtered_order_rebuilds_when_filter_changes() {
        // Changing the filter signature must rebuild the cache, not serve stale
        // rows from the previous filter's order.
        let state = state_with_memory_db(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO items (name) VALUES ('apple'), ('apricot'), ('banana'), ('cherry');",
        );
        let req = |needle: &str| QueryRequest {
            table: "items".to_string(),
            offset: 0,
            limit: 10,
            filters: vec![text_filter("name", needle)],
            global_filter: String::new(),
            sort_column: Some("name".to_string()),
            sort_asc: true,
        };

        let ap = query_table(&state, &req("ap")).unwrap();
        assert_eq!(ap.total_rows, Some(2));
        let names: Vec<_> = ap.rows.iter().map(|r| r[1].as_deref()).collect();
        assert_eq!(names, vec![Some("apple"), Some("apricot")]);

        let an = query_table(&state, &req("an")).unwrap();
        assert_eq!(an.total_rows, Some(1));
        assert_eq!(an.rows[0][1].as_deref(), Some("banana"));
    }

    #[test]
    fn filtered_unsorted_query_pages_in_rowid_order() {
        // A filter with no sort still goes through the cached path, paging in
        // stable rowid (insertion) order. Exercises build_filtered_order's
        // `ORDER BY rowid ASC` branch and confirms the exact total is reported
        // (so the frontend skips its separate count_rows scan).
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            // Even ids "keep", odd "drop". Insertion order == rowid order.
            for i in 0..1_500 {
                let name = format!("{}-{}", if i % 2 == 0 { "keep" } else { "drop" }, i);
                tx.execute("INSERT INTO items (name) VALUES (?)", params![name])
                    .unwrap();
            }
            tx.commit().unwrap();
        }

        let page = |offset: i64| QueryRequest {
            table: "items".to_string(),
            offset,
            limit: 500,
            filters: vec![text_filter("name", "keep")],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        };

        // 750 even ids match; first page is ids 0,2,..,998 in rowid order.
        let first = query_table(&state, &page(0)).unwrap();
        assert_eq!(first.total_rows, Some(750));
        assert_eq!(first.rows.len(), 500);
        assert_eq!(first.rows[0][0].as_deref(), Some("1"));
        assert_eq!(first.rows[499][0].as_deref(), Some("999"));

        // Deep page from the same cached order: ids 1000,1002,..,1498.
        let deep = query_table(&state, &page(500)).unwrap();
        assert_eq!(deep.rows.len(), 250);
        assert_eq!(deep.rows[0][0].as_deref(), Some("1001"));
        assert_eq!(deep.rows[249][0].as_deref(), Some("1499"));
    }

    #[test]
    fn filtered_query_clamps_offset_past_end() {
        // Offset past the matched-row count returns an empty page, not an error
        // or panic, while still reporting the true total. Mirrors
        // sorted_query_clamps_offset_past_end for the filtered path's own clamp.
        let state = state_with_memory_db(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO items (name) VALUES ('keep-a'), ('drop-b'), ('keep-c');",
        );
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 500,
            limit: 500,
            filters: vec![text_filter("name", "keep")],
            global_filter: String::new(),
            sort_column: Some("name".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.total_rows, Some(2));
        assert!(result.rows.is_empty());
    }

    #[test]
    fn filtered_query_falls_back_on_without_rowid_table() {
        // WITHOUT ROWID tables can't be addressed by the filtered-order cache,
        // so a filtered query must fall back to WHERE + OFFSET and still return
        // correctly filtered, sorted rows.
        let state = state_with_memory_db(
            "CREATE TABLE t (k TEXT PRIMARY KEY, v INTEGER) WITHOUT ROWID;
             INSERT INTO t (k, v) VALUES ('keep-c', 3), ('keep-a', 1), ('drop-b', 2);",
        );
        let req = QueryRequest {
            table: "t".to_string(),
            offset: 0,
            limit: 10,
            filters: vec![text_filter("k", "keep")],
            global_filter: String::new(),
            sort_column: Some("v".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        let ks: Vec<_> = result.rows.iter().map(|r| r[0].as_deref()).collect();
        assert_eq!(ks, vec![Some("keep-a"), Some("keep-c")]);
    }

    #[test]
    fn sorted_query_handles_page_larger_than_variable_cap() {
        // A page bigger than SQLITE_MAX_VARIABLE_NUMBER (oldest cap 999) must be
        // batched, not blow up the IN(...) bound-parameter limit.
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, n INTEGER);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            for n in (0..2_000).rev() {
                tx.execute("INSERT INTO items (n) VALUES (?)", [n]).unwrap();
            }
            tx.commit().unwrap();
        }
        let req = QueryRequest {
            table: "items".to_string(),
            offset: 0,
            limit: 1_500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("n".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.rows.len(), 1_500);
        assert_eq!(result.rows[0][1].as_deref(), Some("0"));
        // Crossing the 900-row batch boundary preserves order.
        assert_eq!(result.rows[899][1].as_deref(), Some("899"));
        assert_eq!(result.rows[900][1].as_deref(), Some("900"));
        assert_eq!(result.rows[1_499][1].as_deref(), Some("1499"));
    }

    #[test]
    fn sorted_query_handles_extreme_offset_without_panic() {
        let state = state_with_memory_db(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, n INTEGER);
             INSERT INTO items (n) VALUES (3), (1), (2);",
        );
        let req = QueryRequest {
            table: "items".to_string(),
            offset: i64::MAX,
            limit: 500,
            filters: vec![],
            global_filter: String::new(),
            sort_column: Some("n".to_string()),
            sort_asc: true,
        };

        let result = query_table(&state, &req).unwrap();

        assert_eq!(result.total_rows, Some(3));
        assert!(result.rows.is_empty());
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

    #[test]
    fn count_rows_rejects_regex_filters() {
        let state = state_with_memory_db(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users (name) VALUES ('alice'), ('bravo');",
        );

        let err = count_rows(&state, "users", &[regex_filter("name", "^a")], "").unwrap_err();

        assert!(err.contains("regex"), "got: {err}");
    }

    #[test]
    fn rowid_index_build_cancels_when_generation_changes() {
        let state = state_with_memory_db("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT);");
        {
            let mut guard = state.conn.lock();
            let conn = guard.as_mut().unwrap();
            let tx = conn.transaction().unwrap();
            for idx in 0..100 {
                tx.execute(
                    "INSERT INTO items (name) VALUES (?)",
                    [format!("item-{idx}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        state.query_generation.fetch_add(1, Ordering::Relaxed);
        let guard = state.conn.lock();
        let conn = guard.as_ref().unwrap();

        let result = build_rowid_index(conn, &state, 0, "items", 10);

        assert!(result.is_none());
    }

    #[test]
    fn count_rows_applies_filters() {
        let state = state_with_memory_db(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, city TEXT);
             INSERT INTO users (name, city) VALUES
               ('alice', 'Berlin'),
               ('bravo', 'Boston'),
               ('carol', 'Berlin');",
        );
        let filters = vec![ColumnFilter {
            column: "city".to_string(),
            value: "Berlin".to_string(),
            is_regex: false,
        }];

        let count = count_rows(&state, "users", &filters, "a").unwrap();

        assert_eq!(count, 2);
    }
}
