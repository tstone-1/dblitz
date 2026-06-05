use super::types::{DbState, SqlResult};
use super::util::read_row;

// Execute SQL materializes the whole result set in one IPC round-trip (Rust
// Vec -> JSON -> JS), unlike Browse Data which pages in chunks. Keep this cap
// below `query_table`'s 100k `MAX_QUERY_LIMIT` so an ad-hoc query can't become
// the single heaviest allocation+serialization path in the app. 50k still
// covers any realistic interactive result; beyond that, page with LIMIT/OFFSET.
const SQL_RESULT_LIMIT: usize = 50_000;

/// True if `sql` (already trimmed) starts with the keyword ATTACH or DETACH,
/// case-insensitive, followed by a non-identifier character. Catches the
/// common forms `ATTACH '…' AS x`, `ATTACH DATABASE '…' AS x`, `DETACH x`,
/// `DETACH DATABASE x` regardless of casing. Does not attempt to skip SQL
/// comments — anyone crafting `/* x */ ATTACH …` is working hard enough to
/// bypass that the immutable connection remains the backstop.
fn is_attach_or_detach(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    let after = |kw: &str| -> bool {
        lower
            .strip_prefix(kw)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| !c.is_ascii_alphanumeric() && c != '_')
    };
    after("attach") || after("detach")
}

pub fn execute_sql(state: &DbState, sql: &str) -> SqlResult {
    let guard = state.conn.lock();
    let conn = match guard.as_ref() {
        Some(c) => c,
        None => {
            return SqlResult {
                columns: vec![],
                rows: vec![],
                rows_affected: 0,
                error: Some("No database open".to_string()),
                truncated: false,
            }
        }
    };

    let trimmed = sql.trim();

    // ATTACH/DETACH don't modify the main database file, so sqlite3's
    // `stmt.readonly()` reports them as read-only. But they let users
    // bring other database files into the connection — which violates
    // dblitz's "this viewer cannot reach beyond the file you opened"
    // promise. Reject them at the input boundary before prepare.
    if is_attach_or_detach(trimmed) {
        return SqlResult {
            columns: vec![],
            rows: vec![],
            rows_affected: 0,
            error: Some(
                "dblitz is a read-only viewer - ATTACH and DETACH are not allowed.".to_string(),
            ),
            truncated: false,
        };
    }

    let mut stmt = match conn.prepare(trimmed) {
        Ok(s) => s,
        Err(e) => {
            return SqlResult {
                columns: vec![],
                rows: vec![],
                rows_affected: 0,
                error: Some(e.to_string()),
                truncated: false,
            };
        }
    };

    if !stmt.readonly() {
        return SqlResult {
            columns: vec![],
            rows: vec![],
            rows_affected: 0,
            error: Some(
                "dblitz is a read-only viewer - write statements (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER, etc.) are not supported.".to_string(),
            ),
            truncated: false,
        };
    }

    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let col_count = columns.len();
    let query_result = stmt.query([]);
    match query_result {
        Ok(mut rows_iter) => {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut truncated = false;
            loop {
                match rows_iter.next() {
                    Ok(Some(row)) => {
                        // Only flag truncation once we've collected the cap
                        // AND confirmed at least one more row exists. Checking
                        // before the fetch would falsely truncate a result
                        // that lands exactly on the cap.
                        if rows.len() >= SQL_RESULT_LIMIT {
                            truncated = true;
                            break;
                        }
                        rows.push(read_row(row, col_count));
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return SqlResult {
                            columns,
                            rows,
                            rows_affected: 0,
                            error: Some(e.to_string()),
                            truncated: false,
                        };
                    }
                }
            }
            SqlResult {
                columns,
                rows,
                rows_affected: 0,
                error: None,
                truncated,
            }
        }
        Err(e) => SqlResult {
            columns: vec![],
            rows: vec![],
            rows_affected: 0,
            error: Some(e.to_string()),
            truncated: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{close_database, schema::open_database};
    use rusqlite::Connection;

    fn setup_temp_db_with_table() -> (DbState, std::path::PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dblitz_test_{}.sqlite", nanos));

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
                .unwrap();
            conn.execute("INSERT INTO users (name) VALUES ('alice')", [])
                .unwrap();
        }

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

        assert!(
            result.error.is_none(),
            "SELECT should succeed, got: {:?}",
            result.error
        );
        assert_eq!(result.columns, vec!["name"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0].as_deref(), Some("alice"));

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    /// Asserts the statement is rejected with the read-only error message,
    /// either through dblitz's `stmt.readonly()` gate or through SQLite's
    /// own read-only-connection enforcement — both are acceptable defenses.
    fn assert_rejected(state: &DbState, sql: &str) {
        let result = execute_sql(state, sql);
        assert!(result.error.is_some(), "expected {sql:?} to be rejected");
        assert!(
            result.rows.is_empty(),
            "rejected statement must return no rows"
        );
        assert_eq!(result.rows_affected, 0);
    }

    #[test]
    fn execute_sql_rejects_attach_database() {
        // ATTACH lets users bring other database files into the connection.
        // SQLite's `stmt.readonly()` reports it as read-only because it
        // doesn't touch the *current* file — so dblitz rejects it at the
        // input boundary instead. Case + form variants all covered.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, "ATTACH DATABASE ':memory:' AS scratch");
        assert_rejected(&state, "attach ':memory:' as scratch");
        assert_rejected(&state, "  ATTACH ':memory:' AS scratch");
        assert_rejected(&state, "DETACH scratch");
        assert_rejected(&state, "detach database scratch");

        // Sanity: a word that merely starts with `attach`/`detach` is NOT
        // an ATTACH/DETACH statement and must still be parsed by SQLite
        // (which will then reject it as a syntax error, not as ATTACH).
        let result = execute_sql(&state, "SELECT attached FROM users");
        let err = result.error.unwrap_or_default();
        assert!(
            !err.contains("ATTACH and DETACH are not allowed"),
            "guard misfired on identifier 'attached': {err}"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_rejects_write_pragma() {
        // `journal_mode=wal` requires writing to the database header. The
        // connection is opened with SQLITE_OPEN_READ_ONLY + immutable=1, so
        // the write must be refused.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, "PRAGMA journal_mode = wal");
        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_rejects_create_temp_table() {
        // CREATE TEMP TABLE writes to the temp schema. On a strict
        // read-only viewer this should be rejected even though it doesn't
        // touch the main database file — we don't want users accidentally
        // mutating session state through SQL.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, "CREATE TEMP TABLE scratch (x INTEGER)");
        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_rejects_begin_immediate() {
        // BEGIN IMMEDIATE acquires a RESERVED lock for writing. Plain
        // BEGIN/BEGIN DEFERRED defers locking until first write so SQLite
        // may accept those — IMMEDIATE is the unambiguous write-intent
        // form. Cover it explicitly so any regression in `stmt.readonly()`
        // handling shows up.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, "BEGIN IMMEDIATE");
        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_rejects_multi_statement_payload() {
        // `rusqlite::Connection::prepare` refuses multi-statement input
        // outright with "Multiple statements provided". That is a stronger
        // guarantee than "only first runs" — there's no way to smuggle a
        // DROP behind a leading SELECT. Pin the behavior so a future
        // switch to `execute_batch` would trip the test.
        let (state, path) = setup_temp_db_with_table();
        let result = execute_sql(&state, "SELECT name FROM users; DROP TABLE users");
        assert!(
            result.error.is_some(),
            "multi-statement payload should be rejected"
        );
        let err = result.error.unwrap();
        assert!(
            err.contains("Multiple statements"),
            "expected multi-statement rejection, got: {err}"
        );

        // And users must still be there.
        let check = execute_sql(&state, "SELECT COUNT(*) FROM users");
        assert!(check.error.is_none(), "users table must still exist");
        assert_eq!(check.rows[0][0].as_deref(), Some("1"));

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_truncates_large_result_without_erroring() {
        // A result set larger than SQL_RESULT_LIMIT must return the first N
        // rows WITH `truncated = true` and NO `error`. The truncation notice
        // is a non-fatal warning that travels alongside the rows - folding it
        // into `error` made the frontend hide all 10k rows it had received.
        let (state, path) = setup_temp_db_with_table();

        // A recursive CTE generates SQL_RESULT_LIMIT + 1 rows on a read-only
        // connection without needing any table data.
        let sql = format!(
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < {}) SELECT x FROM c",
            SQL_RESULT_LIMIT + 1
        );
        let result = execute_sql(&state, &sql);

        assert!(
            result.error.is_none(),
            "truncation must not surface as an error, got: {:?}",
            result.error
        );
        assert!(result.truncated, "result should be flagged truncated");
        assert_eq!(
            result.rows.len(),
            SQL_RESULT_LIMIT,
            "exactly the row cap should be returned"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_does_not_flag_exact_limit_as_truncated() {
        // Exactly SQL_RESULT_LIMIT rows is a complete result - the loop hits
        // `Ok(None)` before tripping the cap, so `truncated` stays false.
        let (state, path) = setup_temp_db_with_table();

        let sql = format!(
            "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x < {}) SELECT x FROM c",
            SQL_RESULT_LIMIT
        );
        let result = execute_sql(&state, &sql);

        assert!(result.error.is_none());
        assert!(
            !result.truncated,
            "an exactly-at-cap result is complete, not truncated"
        );
        assert_eq!(result.rows.len(), SQL_RESULT_LIMIT);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_allows_read_only_pragma() {
        // PRAGMA table_info is read-only and must work — it's used by the
        // schema browser and dblitz's own column lookup.
        let (state, path) = setup_temp_db_with_table();
        let result = execute_sql(&state, "PRAGMA table_info(users)");
        assert!(
            result.error.is_none(),
            "read-only PRAGMA should succeed, got: {:?}",
            result.error
        );
        // table_info returns one row per column — users has id + name.
        assert_eq!(result.rows.len(), 2);
        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_database_creates_no_wal_or_shm_sidecars() {
        // The README and module docs promise that opening a database with
        // ?immutable=1 leaves no `-wal` / `-shm` files next to the file.
        // This is the load-bearing test for that promise.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dblitz_sidecar_test_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test.sqlite");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);")
                .unwrap();
            conn.execute("INSERT INTO users (name) VALUES ('alice')", [])
                .unwrap();
        }

        let state = DbState::new();
        open_database(&state, db_path.to_str().unwrap()).unwrap();

        // Do an actual query so SQLite has reason to touch the file.
        let result = execute_sql(&state, "SELECT * FROM users");
        assert!(result.error.is_none(), "SELECT must succeed");

        close_database(&state);

        let wal = dir.join("test.sqlite-wal");
        let shm = dir.join("test.sqlite-shm");
        let journal = dir.join("test.sqlite-journal");
        assert!(
            !wal.exists(),
            "no -wal sidecar should be created (found at {})",
            wal.display()
        );
        assert!(
            !shm.exists(),
            "no -shm sidecar should be created (found at {})",
            shm.display()
        );
        assert!(
            !journal.exists(),
            "no -journal sidecar should be created (found at {})",
            journal.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
