//! The SQL tab for a Parquet file.
//!
//! The user's statement is classified first (see [`super::lockdown`]), then
//! run through DuckDB's `query()` table function with every column rendered to
//! text by position (`#1`, `#2`, ...). Passing the SQL as a string literal to
//! `query()` rather than splicing it into a subquery is what makes a trailing
//! `;` or `-- comment` harmless; positional references keep duplicate column
//! names (`SELECT a.x, b.x`) apart. `query()` itself also accepts only a
//! SELECT, a third check behind the classifier.
//!
//! One visible difference from the SQLite tab: DuckDB renames a repeated
//! column name in a subquery, so `SELECT 1 AS a, 2 AS a` shows `a` and `a_1`.

use std::sync::atomic::{AtomicU64, Ordering};

use super::lockdown::{LockedDb, Refusal};
use super::sql_literal;
use crate::db::SqlResult;

/// Same cap and truncation rule as the SQLite tab (`db::sql`).
const SQL_RESULT_LIMIT: usize = 50_000;

pub(crate) fn execute_sql(db: &LockedDb, sql: &str, generation: &AtomicU64) -> SqlResult {
    let sql = sql.trim();
    match db.classify(sql) {
        Ok(()) => {}
        Err(Refusal::Invalid(e)) => return SqlResult::error(e),
        Err(Refusal::NotOne(n)) => {
            return SqlResult::error(format!(
                "dblitz runs one statement at a time - this query has {n}."
            ))
        }
        Err(Refusal::NotSelect) => {
            return SqlResult::error(
                "dblitz is a read-only viewer - only SELECT queries (including FROM-first, DESCRIBE and SUMMARIZE) can run on a Parquet file.".to_string(),
            )
        }
    }
    let conn = db.conn();
    let source = format!("query({})", sql_literal(sql));

    let described: Result<Vec<(String, String)>, String> = (|| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT column_name, column_type FROM (DESCRIBE SELECT * FROM {source})"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.map(|r| r.map_err(|e| e.to_string())).collect()
    })();
    let (columns, column_types): (Vec<String>, Vec<String>) = match described {
        Ok(d) => d.into_iter().unzip(),
        Err(e) => return SqlResult::error(e),
    };
    if columns.is_empty() {
        return SqlResult {
            columns,
            rows: vec![],
            column_types,
            error: None,
            truncated: false,
        };
    }
    let proj = column_types
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let p = format!("#{}", i + 1);
            if super::is_blob(t) {
                format!("CASE WHEN {p} IS NULL THEN NULL ELSE '[BLOB ' || octet_length({p}) || ' bytes]' END")
            } else {
                format!("CAST({p} AS VARCHAR)")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let gen = generation.load(Ordering::Relaxed);
    let query = format!("SELECT {proj} FROM {source} LIMIT {}", SQL_RESULT_LIMIT + 1);
    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(e) => return SqlResult::error(e.to_string()),
    };
    let mut it = match stmt.query([]) {
        Ok(it) => it,
        Err(e) => return SqlResult::error(e.to_string()),
    };
    let n = columns.len();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    loop {
        match it.next() {
            Ok(Some(r)) => {
                if generation.load(Ordering::Relaxed) != gen {
                    return SqlResult::partial_error(
                        columns,
                        rows,
                        column_types,
                        "Query cancelled by a newer request".to_string(),
                    );
                }
                if rows.len() >= SQL_RESULT_LIMIT {
                    truncated = true;
                    break;
                }
                let row: Result<Vec<Option<String>>, _> =
                    (0..n).map(|i| r.get::<_, Option<String>>(i)).collect();
                match row {
                    Ok(row) => rows.push(row),
                    Err(e) => {
                        return SqlResult::partial_error(columns, rows, column_types, e.to_string())
                    }
                }
            }
            Ok(None) => break,
            Err(e) => return SqlResult::partial_error(columns, rows, column_types, e.to_string()),
        }
    }
    SqlResult {
        columns,
        rows,
        column_types,
        error: None,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, scratch_dir};
    use super::*;

    fn db(rows: u64) -> LockedDb {
        let dir = scratch_dir("sql");
        let file = fixture(&dir, rows);
        LockedDb::open(file.to_str().unwrap(), dir.join("spill").to_str().unwrap()).unwrap()
    }

    #[test]
    fn a_select_returns_rendered_cells_and_duckdb_types() {
        let r = execute_sql(
            &db(10),
            "SELECT id, val, b, tags FROM data WHERE id = 3; -- trailing comment",
            &AtomicU64::new(0),
        );
        assert_eq!(r.error, None);
        assert_eq!(r.columns, ["id", "val", "b", "tags"]);
        assert_eq!(r.column_types, ["BIGINT", "DOUBLE", "BLOB", "BIGINT[]"]);
        assert_eq!(
            r.rows,
            [[
                Some("3".into()),
                Some("1.5".into()),
                Some("[BLOB 3 bytes]".into()),
                Some("[0, 3]".into())
            ]]
        );
    }

    #[test]
    fn a_result_over_the_cap_is_truncated_not_failed() {
        let r = execute_sql(&db(10), "SELECT * FROM range(50001)", &AtomicU64::new(0));
        assert_eq!((r.error, r.rows.len(), r.truncated), (None, 50_000, true));
        let r = execute_sql(&db(10), "SELECT * FROM range(50000)", &AtomicU64::new(0));
        assert_eq!((r.rows.len(), r.truncated), (50_000, false));
    }

    #[test]
    fn a_write_is_refused_with_a_read_only_message() {
        let r = execute_sql(&db(10), "CREATE TABLE t AS SELECT 1", &AtomicU64::new(0));
        assert!(r.error.unwrap().contains("read-only"));
        let r = execute_sql(&db(10), "SELECT 1; SELECT 2", &AtomicU64::new(0));
        assert!(r.error.unwrap().contains("has 2"));
    }

    #[test]
    fn a_syntax_error_is_duckdbs_own_message() {
        let r = execute_sql(&db(10), "SELEC 1", &AtomicU64::new(0));
        assert!(r.error.unwrap().contains("syntax error"));
    }
}
