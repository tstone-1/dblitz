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
use crate::db::{SqlCheck, SqlResult};

/// Same cap and truncation rule as the SQLite tab (`db::sql`).
const SQL_RESULT_LIMIT: usize = 50_000;

/// The message Execute shows for a classifier refusal. `validate_sql` shows
/// the same text, so the editor's live check reads exactly like the error
/// Execute would have produced.
fn refusal_message(refusal: Refusal) -> String {
    match refusal {
        Refusal::Invalid(e) => e,
        Refusal::NotOne(n) => {
            format!("dblitz runs one statement at a time - this query has {n}. Select the one to run.")
        }
        Refusal::NotSelect => "dblitz is a read-only viewer - only SELECT queries (including FROM-first, DESCRIBE and SUMMARIZE) can run on a Parquet file.".to_string(),
    }
}

/// The editor's live check: the classifier alone, which prepares (and so
/// parses and binds) the statement on the locked side connection without
/// running it.
pub(crate) fn validate_sql(db: &LockedDb, sql: &str) -> SqlCheck {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return SqlCheck::ok();
    }
    let lead = sql.len() - sql.trim_start().len();
    match db.classify(trimmed) {
        Ok(()) => SqlCheck::ok(),
        Err(r) => {
            let message = refusal_message(r);
            let offset = duckdb_error_offset(trimmed, &message).map(|o| lead + o);
            SqlCheck::problem(sql, message, offset)
        }
    }
}

/// Where a DuckDB error points, as a byte offset into `sql`.
///
/// The C API has no error position, but the message ends with an excerpt and
/// a marker under the offending token:
///
/// ```text
/// LINE 3:   nosuch
///           ^^^^^^
/// ```
///
/// The marker's column counts characters, and a long line is shown as a
/// window starting (and possibly ending) with `...`. The excerpt is matched
/// against the real source line before the column is trusted, so a message in
/// some other shape yields `None` rather than an underline in the wrong place.
pub(crate) fn duckdb_error_offset(sql: &str, message: &str) -> Option<usize> {
    let lines: Vec<&str> = message.lines().collect();
    let (i, rest) = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, l)| l.strip_prefix("LINE ").map(|r| (i, r)))?;
    let (num, excerpt) = rest.split_once(": ")?;
    let line_no: usize = num.parse().ok()?;
    let marker = lines.get(i + 1)?;
    let prefix_chars = "LINE ".len() + num.len() + ": ".len();
    let caret_col = marker
        .chars()
        .position(|c| c == '^')?
        .checked_sub(prefix_chars)?;

    let (excerpt, col_in_excerpt) = match excerpt.strip_prefix("...") {
        Some(e) => (e, caret_col.checked_sub(3)?),
        None => (excerpt, caret_col),
    };
    let excerpt = excerpt.strip_suffix("...").unwrap_or(excerpt);

    let source_line = sql.split('\n').nth(line_no.checked_sub(1)?)?;
    let line_start = source_line.as_ptr() as usize - sql.as_ptr() as usize;
    let at = source_line.find(excerpt)?;
    let token = source_line[at..].char_indices().nth(col_in_excerpt)?.0;
    Some(line_start + at + token)
}

pub(crate) fn execute_sql(db: &LockedDb, sql: &str, generation: &AtomicU64) -> SqlResult {
    let sql = sql.trim();
    if let Err(r) = db.classify(sql) {
        return SqlResult::error(refusal_message(r));
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
        let file = file.to_str().unwrap();
        let reader = crate::pq::FileFormat::Parquet.reader(&crate::pq::sql_literal(file));
        LockedDb::open(file, &reader, dir.join("spill").to_str().unwrap()).unwrap()
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

    #[test]
    fn validate_matches_execute_and_never_runs_the_statement() {
        let db = db(10);
        let gen = AtomicU64::new(0);
        for sql in [
            "SELECT nosuch FROM data",
            "SELEC 1",
            "CREATE TABLE t AS SELECT 1",
            "SELECT 1; SELECT 2",
        ] {
            let check = validate_sql(&db, sql);
            assert!(check.error.is_some(), "{sql:?} should not pass the check");
            assert_eq!(check.error, execute_sql(&db, sql, &gen).error, "{sql:?}");
        }
        assert_eq!(validate_sql(&db, "SELECT id FROM data"), SqlCheck::ok());
        // The offset indexes the text as sent, leading whitespace included.
        let check = validate_sql(
            &db,
            "
  SELECT nosuch FROM data",
        );
        assert_eq!(check.offset, Some(10));
        // Binding without running: an endless range validates at once.
        let started = std::time::Instant::now();
        assert_eq!(
            validate_sql(&db, "SELECT count(*) FROM range(9223372036854775807)"),
            SqlCheck::ok()
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn the_duckdb_error_marker_becomes_an_offset_on_the_right_line() {
        // Messages captured from DuckDB itself (2026-10-29).
        let m = "Binder Error: Referenced column \"nosuch\" not found in FROM clause!
Candidate bindings: \"name\"

LINE 3:   nosuch
          ^^^^^^";
        let sql = "SELECT id,
  -- note
  nosuch
FROM data";
        let at = duckdb_error_offset(sql, m).unwrap();
        assert_eq!(&sql[at..at + 6], "nosuch");

        let m = "Binder Error: x

LINE 1: SELECT 'äöü', nosuch FROM data
                      ^^^^^^";
        let sql = "SELECT 'äöü', nosuch FROM data";
        let at = duckdb_error_offset(sql, m).unwrap();
        assert_eq!(&sql[at..at + 6], "nosuch");
    }

    #[test]
    fn a_windowed_excerpt_is_found_in_the_long_line() {
        let sql = format!("SELECT {} nosuch FROM data", "id + ".repeat(60));
        let r = validate_sql(&db(10), &sql);
        assert!(r.error.as_deref().unwrap().contains("LINE 1: ..."), "{r:?}");
        let at = r.offset.unwrap();
        assert_eq!(&sql[at..at + 6], "nosuch");
    }

    #[test]
    fn a_marker_that_does_not_match_the_source_gives_no_offset() {
        // The excerpt is not in the statement: guessing would underline the
        // wrong text, so the answer is None.
        let m = "Parser Error: x

LINE 1: SELECT other
        ^^^^^";
        assert_eq!(duckdb_error_offset("SELEC 1", m), None);
        assert_eq!(
            duckdb_error_offset("SELEC 1", "Parser Error: no excerpt at all"),
            None
        );
        assert_eq!(
            duckdb_error_offset(
                "SELEC 1",
                "LINE 9: SELEC 1
        ^"
            ),
            None
        );
    }

    #[test]
    fn every_function_the_editor_suggests_exists_in_the_bundled_duckdb() {
        let list: serde_json::Value = serde_json::from_str(include_str!(
            "../../../src/lib/components/sqlFunctions.json"
        ))
        .unwrap();
        let names: Vec<&str> = list["duckdb"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert!(
            names.len() > 40,
            "the list should not be empty or truncated"
        );
        let conn = duckdb::Connection::open_in_memory().unwrap();
        // `coalesce` and `ifnull` are parser special forms, absent from the
        // catalog, so a name the catalog lacks is asked of the parser: an
        // arity complaint means it exists, "does not exist" means it does not.
        let exists = |name: &str| -> bool {
            let in_catalog = conn
                .query_row(
                    "SELECT count(*) FROM duckdb_functions() WHERE function_name = ?",
                    [name],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
                > 0;
            in_catalog
                || match conn.prepare(&format!("SELECT {name}(1, 2)")) {
                    Ok(_) => true,
                    Err(e) => !e.to_string().contains("does not exist"),
                }
        };
        let missing: Vec<&str> = names.iter().copied().filter(|n| !exists(n)).collect();
        assert!(missing.is_empty(), "not in this DuckDB: {missing:?}");
        assert!(
            !exists("no_such_function_xyz"),
            "control: the lookup can say no"
        );
    }
}
