use std::sync::atomic::Ordering;

use super::types::{DbState, SqlResult};
use super::util::read_row;

// Execute SQL materializes the whole result set in one IPC round-trip (Rust
// Vec -> JSON -> JS), unlike Browse Data which pages in chunks. Keep this cap
// below `query_table`'s 100k `MAX_QUERY_LIMIT` so an ad-hoc query can't become
// the single heaviest allocation+serialization path in the app. 50k still
// covers any realistic interactive result; beyond that, page with LIMIT/OFFSET.
const SQL_RESULT_LIMIT: usize = 50_000;

impl SqlResult {
    /// Empty-result error: no columns/rows, just the message. The shape every
    /// pre-row rejection returns (no DB open, the ATTACH/DETACH gate, the
    /// write gate, a prepare failure, a query-start failure).
    fn error(message: String) -> Self {
        SqlResult {
            columns: vec![],
            rows: vec![],
            column_types: vec![],
            error: Some(message),
            truncated: false,
        }
    }

    /// Error carrying the partial columns/rows already collected before a
    /// mid-iteration failure (a `next()` error, or cancellation by a newer
    /// request), so the frontend can still render what was fetched.
    fn partial_error(
        columns: Vec<String>,
        rows: Vec<Vec<Option<String>>>,
        column_types: Vec<String>,
        message: String,
    ) -> Self {
        SqlResult {
            columns,
            rows,
            column_types,
            error: Some(message),
            truncated: false,
        }
    }
}

/// Skip leading whitespace, SQL comments, and empty-statement `;` separators
/// so statement-kind guards see the first executable token, not a prefix
/// comment or a leading semicolon. SQLite's own parser silently skips a
/// leading `;` (treating it as an empty statement) rather than erroring, so
/// without this the guard could be dodged by `;ATTACH ...`.
fn strip_leading_ws_and_comments(mut sql: &str) -> &str {
    loop {
        let trimmed = sql.trim_start();
        if let Some(rest) = trimmed.strip_prefix("--") {
            sql = rest.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(rest) = trimmed.strip_prefix("/*") {
            sql = rest.split_once("*/").map_or("", |(_, tail)| tail);
        } else if let Some(rest) = trimmed.strip_prefix(';') {
            sql = rest;
        } else {
            return trimmed;
        }
    }
}

/// True if `sql` starts with the keyword ATTACH or DETACH, case-insensitive,
/// followed by a non-identifier character. Catches the common forms
/// `ATTACH '...' AS x`, `ATTACH DATABASE '...' AS x`, `DETACH x`,
/// `DETACH DATABASE x` regardless of casing, leading SQL comments, and a
/// leading `;`.
fn is_attach_or_detach(sql: &str) -> bool {
    let lower = strip_leading_ws_and_comments(sql).to_ascii_lowercase();
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
        None => return SqlResult::error("No database open".to_string()),
    };

    let trimmed = sql.trim();

    // ATTACH/DETACH don't modify the main database file, so sqlite3's
    // `stmt.readonly()` reports them as read-only. But they let users
    // bring other database files into the connection — which violates
    // dblitz's "this viewer cannot reach beyond the file you opened"
    // promise. Reject them at the input boundary before prepare.
    if is_attach_or_detach(trimmed) {
        return SqlResult::error(
            "dblitz is a read-only viewer - ATTACH and DETACH are not allowed.".to_string(),
        );
    }

    let mut stmt = match conn.prepare(trimmed) {
        Ok(s) => s,
        Err(e) => return SqlResult::error(e.to_string()),
    };

    if !stmt.readonly() {
        return SqlResult::error(
            "dblitz is a read-only viewer - write statements (INSERT, UPDATE, DELETE, DROP, CREATE, ALTER, etc.) are not supported.".to_string(),
        );
    }

    let col_count = stmt.column_count();
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    // Per-column declared type (e.g. "INTEGER", "TEXT"), used by the XLSX
    // export to decide numeric vs. text formatting. `None` for a column with
    // no declared type (a computed expression like `SELECT 1+1`) - the
    // export side already treats a missing/empty decltype as
    // numeric-affinity, so an empty string here is the right default.
    let column_types: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.decl_type().unwrap_or("").to_string())
        .collect();
    // Snapshot the generation before running so a mid-iteration cancellation
    // (a newer request bumping it) can be detected without waiting on the
    // interrupt handle alone - see the loop below.
    let generation = state.query_generation.load(Ordering::Relaxed);
    let query_result = stmt.query([]);
    match query_result {
        Ok(mut rows_iter) => {
            let mut rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut truncated = false;
            loop {
                match rows_iter.next() {
                    Ok(Some(row)) => {
                        // A newer request (or an explicit cancel) bumped the
                        // generation while we were mid-fetch. `cancel_queries`
                        // also calls `InterruptHandle::interrupt()`, which
                        // breaks a query stuck deep inside a single blocking
                        // `sqlite3_step` (e.g. a grinding recursive CTE) - this
                        // check catches the rest: a query that keeps
                        // completing rows quickly but should still stop.
                        if state.query_generation.load(Ordering::Relaxed) != generation {
                            return SqlResult::partial_error(
                                columns,
                                rows,
                                column_types,
                                "Query cancelled by a newer request".to_string(),
                            );
                        }
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
                        return SqlResult::partial_error(
                            columns,
                            rows,
                            column_types,
                            e.to_string(),
                        );
                    }
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
        Err(e) => SqlResult::error(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{close_database, schema::open_database};
    use rusqlite::Connection;

    fn setup_temp_db_with_table() -> (DbState, std::path::PathBuf) {
        let path = crate::db::util::unique_temp_path("dblitz_test", ".sqlite");

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

    #[test]
    fn execute_sql_captures_per_column_decltypes() {
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "SELECT id, name, 1 + 1 AS computed FROM users");

        assert!(result.error.is_none(), "got: {:?}", result.error);
        assert_eq!(result.columns, vec!["id", "name", "computed"]);
        assert_eq!(result.column_types.len(), 3);
        assert_eq!(result.column_types[0], "INTEGER");
        assert_eq!(result.column_types[1], "TEXT");
        // A computed expression has no declared type - empty string, not an
        // error, and the export side treats that as numeric-affinity.
        assert_eq!(result.column_types[2], "");

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
    fn execute_sql_rejects_attach_behind_leading_comment_without_creating_file() {
        let (state, path) = setup_temp_db_with_table();
        let attach_path = path.with_file_name("dblitz_attach_should_not_exist.sqlite");
        let _ = std::fs::remove_file(&attach_path);
        let sql = format!(
            "/* leading comment */ -- and a line comment\nATTACH '{}' AS other",
            attach_path.to_string_lossy().replace('\\', "\\\\")
        );

        let result = execute_sql(&state, &sql);

        assert!(result.error.is_some(), "comment-prefixed ATTACH must fail");
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("ATTACH and DETACH are not allowed")),
            "expected ATTACH/DETACH guard message, got: {:?}",
            result.error
        );
        assert!(
            !attach_path.exists(),
            "rejected ATTACH must not create {}",
            attach_path.display()
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn execute_sql_rejects_attach_behind_leading_semicolon() {
        // SQLite's own parser silently skips a leading `;` (empty statement)
        // rather than erroring, so `;ATTACH ...` used to slip past
        // `is_attach_or_detach` (which only checked for comments/whitespace)
        // and reach `stmt.readonly()`, which reports ATTACH as read-only.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, ";ATTACH ':memory:' AS s");
        assert_rejected(&state, "/* c */;ATTACH ':memory:' AS s");

        // Must not create the target file, or leave the schema attached.
        let attach_path = path.with_file_name("dblitz_semicolon_attach_should_not_exist.sqlite");
        let _ = std::fs::remove_file(&attach_path);
        let sql = format!(
            ";ATTACH '{}' AS other",
            attach_path.to_string_lossy().replace('\\', "\\\\")
        );
        let result = execute_sql(&state, &sql);
        assert!(
            result.error.is_some(),
            "leading-semicolon ATTACH must be rejected"
        );
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|e| e.contains("ATTACH and DETACH are not allowed")),
            "expected ATTACH/DETACH guard message, got: {:?}",
            result.error
        );
        assert!(
            !attach_path.exists(),
            "rejected ATTACH must not create {}",
            attach_path.display()
        );

        // And "other" must not be a reachable schema afterward.
        let read_result = execute_sql(&state, "SELECT * FROM other.nonexistent");
        assert!(
            read_result.error.is_some(),
            "schema 'other' must not exist after a rejected ATTACH"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn authorizer_denies_attach_at_the_engine_level_bypassing_the_string_gate() {
        // The string gate in `is_attach_or_detach` has already been bypassed
        // twice by prefix tricks (a leading comment, then a leading `;`) -
        // it operates on the raw text before `prepare`. The authorizer
        // registered in `open_database` runs inside SQLite on the *parsed*
        // statement, so it can't be dodged by any lexical prefix. Prove that
        // by going around `execute_sql`'s string gate entirely and calling
        // `Connection::prepare` directly on the opened (authorizer-bearing)
        // connection.
        let (state, path) = setup_temp_db_with_table();
        {
            let guard = state.conn.lock();
            let conn = guard.as_ref().unwrap();
            let err = conn
                .prepare("ATTACH ':memory:' AS s")
                .expect_err("authorizer must deny ATTACH even reached directly");
            let msg = err.to_string();
            assert!(
                msg.to_ascii_lowercase().contains("not authorized"),
                "expected an authorization error, got: {msg}"
            );

            let err = conn
                .prepare("DETACH s")
                .expect_err("authorizer must deny DETACH even reached directly");
            assert!(
                err.to_string()
                    .to_ascii_lowercase()
                    .contains("not authorized"),
                "expected an authorization error, got: {err}"
            );
        }
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
    fn execute_sql_rejects_plain_begin_and_savepoint_via_authorizer() {
        // Plain BEGIN/BEGIN DEFERRED and SAVEPOINT are reported read-only by
        // `stmt.readonly()` and used to pass `execute_sql` unchallenged,
        // leaving transaction state on the shared connection with no
        // COMMIT/ROLLBACK path. The authorizer registered in `open_database`
        // now denies `AuthAction::Transaction`/`Savepoint` outright, so these
        // are rejected before they can execute.
        let (state, path) = setup_temp_db_with_table();
        assert_rejected(&state, "BEGIN");
        assert_rejected(&state, "BEGIN DEFERRED");
        assert_rejected(&state, "SAVEPOINT sp1");
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
        let dir = crate::db::util::unique_temp_path("dblitz_sidecar_test", "");
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

    #[test]
    fn cancel_interrupts_long_running_sql() {
        // A grinding recursive CTE has no natural end and no per-row cheap
        // point to check the generation counter from outside — the only way
        // to stop it is `InterruptHandle::interrupt()`, which forces the next
        // (or current) `sqlite3_step` call to return SQLITE_INTERRUPT.
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let (state, path) = setup_temp_db_with_table();
        let state = Arc::new(state);
        let worker_state = Arc::clone(&state);

        let handle = std::thread::spawn(move || {
            execute_sql(
                &worker_state,
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c) \
                 SELECT count(*) FROM c",
            )
        });

        // Give the query a moment to actually start grinding before cancelling.
        std::thread::sleep(Duration::from_millis(200));
        let cancel_started = Instant::now();
        crate::db::cancel_queries(&state);

        let result = handle.join().expect("execute_sql thread should not panic");
        let elapsed = cancel_started.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "cancel should interrupt the query within a bounded time, took {:?}",
            elapsed
        );
        assert!(
            result.error.is_some(),
            "an interrupted query should return an error, got: {:?}",
            result
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn authorizer_denies_state_changing_pragma() {
        // `case_sensitive_like=1` reports `readonly = true` to `stmt.readonly()`,
        // so it slips past the write gate - but it silently makes every later
        // LIKE filter case-sensitive on this shared connection. The authorizer's
        // PRAGMA allowlist must reject it, and the rejection must land on
        // execute_sql's normal error path (message set, no rows).
        let (state, path) = setup_temp_db_with_table();

        let denied = execute_sql(&state, "PRAGMA case_sensitive_like = 1");
        assert!(
            denied.error.is_some(),
            "a state-changing PRAGMA must be rejected"
        );
        assert!(denied.rows.is_empty(), "a denied PRAGMA returns no rows");

        // Browse-visible proof: LIKE is still case-insensitive (SQLite's
        // default), so an upper-cased needle still matches the stored 'alice'.
        // If the PRAGMA had taken effect, this would return zero rows.
        let like = execute_sql(&state, "SELECT name FROM users WHERE name LIKE 'ALICE'");
        assert!(like.error.is_none(), "got: {:?}", like.error);
        assert_eq!(
            like.rows.len(),
            1,
            "LIKE must remain case-insensitive: the denied PRAGMA must not have changed semantics"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn authorizer_allows_introspection_pragma() {
        // The read-only introspection allowlist must still let schema PRAGMAs
        // through - dblitz's own column lookup and the schema browser depend on
        // PRAGMA table_info reaching the engine.
        let (state, path) = setup_temp_db_with_table();

        let result = execute_sql(&state, "PRAGMA table_info(users)");
        assert!(
            result.error.is_none(),
            "an allowlisted introspection PRAGMA must succeed, got: {:?}",
            result.error
        );
        // table_info returns one row per column - users has id + name.
        assert_eq!(result.rows.len(), 2);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_then_query_table_round_trip_survives_pragma_authorizer() {
        // The PRAGMA allowlist must not break the internal PRAGMA table_info
        // that query_table issues (via get_column_names) on the shared,
        // authorizer-bearing connection. A full open -> query_table round trip
        // proves the internal introspection PRAGMAs still pass the gate.
        use crate::db::{query_table, QueryRequest};

        let (state, path) = setup_temp_db_with_table();

        let result = query_table(
            &state,
            &QueryRequest {
                table: "users".to_string(),
                offset: 0,
                limit: 10,
                filters: vec![],
                global_filter: String::new(),
                sort_column: None,
                sort_asc: true,
            },
        )
        .unwrap();

        assert_eq!(result.columns, vec!["id", "name"]);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][1].as_deref(), Some("alice"));

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }
}
