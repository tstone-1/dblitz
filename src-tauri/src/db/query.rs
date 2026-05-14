use super::{
    build_rowid_index, read_row, safe_ident, ColumnFilter, DbState, QueryRequest, QueryResult,
    StrErr, MAX_QUERY_LIMIT,
};
use regex::Regex;
use rusqlite::Connection;

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
            if !columns.iter().any(|c| c == &f.column) {
                continue;
            }
            let col_escaped = safe_ident(&f.column);
            let criteria: Vec<&str> = f
                .value
                .split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            if criteria.is_empty() {
                continue;
            }

            let mut and_parts: Vec<String> = Vec::new();
            let mut and_params: Vec<String> = Vec::new();
            let mut or_parts: Vec<String> = Vec::new();
            let mut or_params: Vec<String> = Vec::new();

            for val in &criteria {
                if let Some(rest) = val.strip_prefix("<>") {
                    if rest.is_empty() {
                        and_parts.push(format!(
                            "\"{}\" IS NOT NULL AND \"{}\" != ''",
                            col_escaped, col_escaped
                        ));
                    } else {
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

    Ok(WhereResult {
        clause,
        params,
        regex_filters,
    })
}

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
    let mut matched_count = 0i64;
    let mut rows_iter = stmt.query(param_refs.as_slice()).str_err()?;

    while let Some(row) = rows_iter.next().str_err()? {
        let values = read_row(row, col_count);
        let matches = regex_filters.iter().all(|(idx, re)| {
            values
                .get(*idx)
                .and_then(|v| v.as_ref())
                .map(|s| re.is_match(s))
                .unwrap_or(false)
        });
        if matches {
            if matched_count >= offset && (rows.len() as i64) < limit {
                rows.push(values);
            }
            matched_count += 1;
        }
    }

    Ok(QueryResult {
        columns,
        rows,
        total_rows: matched_count,
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
    let index_key = format!("{table}\0{limit}");

    let mut indexes = state.rowid_indexes.lock();
    if let std::collections::hash_map::Entry::Vacant(entry) = indexes.entry(index_key.clone()) {
        if let Some(idx) = build_rowid_index(conn, safe_table, limit) {
            entry.insert(idx);
        }
    }

    let idx = indexes.get(&index_key)?;
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
            total_rows,
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

    Ok(QueryResult {
        columns,
        rows,
        total_rows,
        offset,
    })
}

pub fn query_table(state: &DbState, req: &QueryRequest) -> Result<QueryResult, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;

    let table = &req.table;
    let offset = req.offset;
    let limit = req.limit;
    if offset < 0 {
        return Err("Invalid offset: must be non-negative".to_string());
    }
    if !(1..=MAX_QUERY_LIMIT).contains(&limit) {
        return Err(format!(
            "Invalid limit: must be between 1 and {MAX_QUERY_LIMIT}"
        ));
    }
    let safe_table = safe_ident(table);
    let columns = get_column_names(conn, &safe_table)?;

    let WhereResult {
        clause: where_clause,
        params,
        regex_filters,
    } = build_where_clause(&columns, &req.filters, &req.global_filter)?;

    let order_clause = match &req.sort_column {
        Some(col) if columns.iter().any(|c| c == col) => format!(
            " ORDER BY \"{}\" {}",
            safe_ident(col),
            if req.sort_asc { "ASC" } else { "DESC" }
        ),
        _ => String::new(),
    };

    if !regex_filters.is_empty() {
        return query_with_regex_filter(
            conn,
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
        if req.sort_column.is_none() {
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
            total_rows,
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
        -1,
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
    use crate::db::schema::{close_database, open_database};
    use rusqlite::Connection;

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: false,
        }
    }

    fn regex_filter(column: &str, value: &str) -> ColumnFilter {
        ColumnFilter {
            column: column.to_string(),
            value: value.to_string(),
            is_regex: true,
        }
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
        assert_eq!(r.regex_filters[0].0, 0);
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

    fn create_temp_db_with_rows(name: &str, row_count: usize) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dblitz_test_{name}_{nanos}.sqlite"));

        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
            .unwrap();
        for idx in 0..row_count {
            conn.execute(
                "INSERT INTO users (name) VALUES (?)",
                [format!("user_{idx}")],
            )
            .unwrap();
        }

        path
    }

    #[test]
    fn open_database_clears_rowid_index_cache_between_files() {
        let state = DbState::new();
        let first_path = create_temp_db_with_rows("first", 3);
        let second_path = create_temp_db_with_rows("second", 7);

        open_database(&state, first_path.to_str().unwrap()).unwrap();
        let first = query_table(
            &state,
            &QueryRequest {
                table: "users".to_string(),
                offset: 0,
                limit: 2,
                filters: vec![],
                global_filter: String::new(),
                sort_column: None,
                sort_asc: true,
            },
        )
        .unwrap();
        assert_eq!(first.total_rows, 3);

        open_database(&state, second_path.to_str().unwrap()).unwrap();
        let second = query_table(
            &state,
            &QueryRequest {
                table: "users".to_string(),
                offset: 0,
                limit: 2,
                filters: vec![],
                global_filter: String::new(),
                sort_column: None,
                sort_asc: true,
            },
        )
        .unwrap();

        assert_eq!(second.total_rows, 7);
        assert_eq!(second.rows.len(), 2);

        close_database(&state);
        let _ = std::fs::remove_file(&first_path);
        let _ = std::fs::remove_file(&second_path);
    }
}
