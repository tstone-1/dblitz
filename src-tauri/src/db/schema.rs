use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};
use std::sync::atomic::Ordering;
use tracing::{error, info, warn};

use super::clear_caches;
use super::types::{ColumnInfo, DbState, SchemaEntry, TableInfo};
use super::util::{path_to_sqlite_uri, quote_ident, StrErr};

/// Read-only introspection PRAGMAs the app and ad-hoc Execute-SQL queries may
/// run against the shared connection. The authorizer denies every PRAGMA not
/// on this list (see the `AuthAction::Pragma` arm below).
///
/// Why an allowlist rather than blocking a handful of known-bad names: several
/// *configuration* PRAGMAs report `readonly = true` to `stmt.readonly()` - so
/// they slip past dblitz's write gate - yet silently change query *semantics*
/// for every later Browse Data page on this shared connection.
/// `case_sensitive_like=1` turns LIKE filters case-sensitive;
/// `reverse_unordered_selects=1` can make the no-ORDER-BY offset path skip or
/// repeat rows across pages. The change persists until the file is reopened.
/// The set of harmless introspection PRAGMAs is small and stable; the set of
/// state-changing ones is open-ended, so we enumerate the safe ones and deny
/// the rest. Matched case-insensitively — SQLite PRAGMA names are.
const ALLOWED_INTROSPECTION_PRAGMAS: &[&str] = &[
    "table_info",
    "table_xinfo",
    "table_list",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
    "foreign_key_check",
    "database_list",
    "collation_list",
    "function_list",
    "module_list",
    "pragma_list",
    "compile_options",
    "integrity_check",
    "quick_check",
];

/// Open one read-only connection to `uri`, fully configured: open flags, the
/// tuning batch, and the authorizer.
///
/// Both of `DbState`'s connections are built here and nowhere else. They are
/// two handles on the same frozen snapshot and the whole safety argument for
/// the second one rests on them being identical - so a change to the flags, the
/// batch or the authorizer must be impossible to apply to one and forget on the
/// other. Splitting this back into two open sites is exactly that mistake.
///
/// dblitz is a viewer, not an editor. Two layers of read-only start here:
///   1. SQLITE_OPEN_READ_ONLY at the connection layer.
///   2. ?immutable=1 in the URI (built by the caller) tells SQLite to treat
///      the file as a frozen snapshot.
pub(super) fn open_read_only_connection(uri: &str) -> Result<Connection, String> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(uri, flags).map_err(|e| {
        error!(error = %e, "Failed to open database");
        e.to_string()
    })?;
    // Read-only tuning:
    //   - cache_size=-64000  : 64 MiB page cache (negative = KiB, not pages).
    //   - temp_store=MEMORY  : keep sorter/temp-b-tree scratch in RAM so a
    //                          non-indexed ORDER BY (e.g. sorting a filtered
    //                          view) never spills to a temp file on disk.
    //
    // There is deliberately NO `mmap_size` here, and it must not come back.
    // `?immutable=1` is a promise dblitz makes to SQLite, not one the operating
    // system enforces: the file can still be truncated or rewritten underneath
    // us by another process (a cloud-sync client, an ETL job, `sqlite3` in
    // another terminal). Without mmap that shows up as a recoverable error -
    // "database disk image is malformed" - which surfaces as a message in the
    // UI. With the file mapped, a read past the new end of file is a SIGBUS the
    // process cannot handle, so dblitz dies with no message and no chance to
    // save anything.
    //
    // Both outcomes were reproduced, as a control pair differing only in this
    // PRAGMA: open a 20 MB database `mode=ro&immutable=1`, read one row,
    // truncate the file from another handle, then `SELECT COUNT(*)`. With
    // `mmap_size=1073741824` the process died of signal 10 (SIGBUS) having
    // printed nothing; with `mmap_size=0` the same sequence returned
    // "database disk image is malformed" and exited 0.
    //
    // Measured cost of not mapping, on a 870 MB / 5M-row table (macOS, M5):
    // warm COUNT(*) 11 ms -> 109 ms, warm sorted-rowid build 660 ms -> 757 ms,
    // warm regex scan 3.2 s -> 3.3 s, warm page fetch unchanged at 0.3 ms; and
    // *cold* (page cache purged) every one of them is FASTER without mmap -
    // COUNT(*) 810 ms -> 370 ms, sort build 1.53 s -> 1.01 s, regex 4.18 s ->
    // 3.38 s. The one real regression, repeated warm COUNT(*), is answered by
    // the per-table count cache in `DbState::table_counts` instead.
    //
    // These run BEFORE the authorizer is installed, deliberately: they are
    // exactly the kind of session-configuration PRAGMA the authorizer denies
    // (see `ALLOWED_INTROSPECTION_PRAGMAS`). The app needs them once at open;
    // installing the authorizer afterwards keeps them out of the allowlist so
    // no ad-hoc query can replay them and reshape Browse Data semantics.
    conn.execute_batch(
        "PRAGMA cache_size=-64000;\
         PRAGMA temp_store=MEMORY;",
    )
    .str_err()?;
    // Engine-level backstop for the ATTACH/DETACH gate in sql.rs. That gate
    // is a lexical check on the input string (fast, gives a friendly error
    // message) and has already been bypassed twice by prefix tricks a
    // parser wouldn't fall for (a leading comment, then a leading `;`).
    // The authorizer runs on the *parsed* statement inside SQLite itself, so
    // no lexical prefix can dodge it — it is the durable fix, and the
    // string gate stays only for the friendlier UI error message.
    // Also deny Transaction/Savepoint: harmless on this READ_ONLY+immutable
    // connection (no locking, file never changes), but dblitz never needs an
    // explicit BEGIN/SAVEPOINT and leaving one open with no COMMIT/ROLLBACK
    // path is untidy state on a shared connection.
    // PRAGMAs are gated to a read-only introspection allowlist: a
    // configuration PRAGMA like `case_sensitive_like=1` reports read-only to
    // `stmt.readonly()` but silently changes query semantics for every later
    // Browse Data page on this shared connection.
    conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. } => Authorization::Deny,
        AuthAction::Pragma { pragma_name, .. } => {
            if ALLOWED_INTROSPECTION_PRAGMAS
                .iter()
                .any(|allowed| pragma_name.eq_ignore_ascii_case(allowed))
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        _ => Authorization::Allow,
    }))
    .str_err()?;
    Ok(conn)
}

pub fn open_database(state: &DbState, path: &str) -> Result<Vec<TableInfo>, String> {
    info!(path, "Opening database (read-only, immutable)");
    state.query_generation.fetch_add(1, Ordering::Relaxed);
    // Interrupt any query still running against either previous connection so
    // this open doesn't block waiting for a lock behind it, and so a grinding
    // query on the old file doesn't keep running pointlessly once the user has
    // moved on to a new one.
    if let Some(handle) = state.interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
    if let Some(handle) = state.aux_interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
    let uri = path_to_sqlite_uri(path);
    // Two connections on the same frozen snapshot: `conn` serves Browse Data
    // (and owns the table-keyed caches), `aux_conn` serves the SQL tab and the
    // Structure tab so those don't queue behind a multi-second sort.
    //
    // Both are opened BEFORE anything in `state` is touched, so a failure of
    // either one leaves the previously open database entirely intact - the same
    // guarantee the single-connection version gave, and the reason the second
    // open is not deferred to first use. A lazily opened second connection
    // would surface a bad-file error in the SQL tab minutes after the open the
    // user would blame it on.
    let conn = open_read_only_connection(&uri)?;
    let aux_conn = open_read_only_connection(&uri)?;

    // Publish the new interrupt handles BEFORE the open-time sweep below, not
    // after it. `get_tables_inner` runs one `COUNT(*)` per table on the new
    // connection, which on a large file is the longest-running statement of the
    // whole open - and while it ran, `state.interrupt_handle` still named the
    // *previous* connection. Cancel, and a second open arriving behind this one,
    // therefore interrupted a connection with nothing running on it (this
    // function already interrupted the old one, above) and left the sweep
    // grinding. The old handles are restored if the sweep fails, so a failed
    // open leaves the still-current connections cancellable. The aux handle is
    // published on the same schedule for the same reason - nothing runs on the
    // new aux connection during the sweep, but the handle must never name a
    // connection that is no longer the one a query could be running on.
    //
    // Safe to take alone: nothing acquires either handle lock while holding
    // `conn` or `aux_conn`, and neither new connection is reachable by any
    // query until the critical section below publishes it.
    let previous_handle = state
        .interrupt_handle
        .lock()
        .replace(conn.get_interrupt_handle());
    let previous_aux_handle = state
        .aux_interrupt_handle
        .lock()
        .replace(aux_conn.get_interrupt_handle());
    let tables = match get_tables_inner(&conn) {
        Ok(tables) => tables,
        Err(e) => {
            *state.interrupt_handle.lock() = previous_handle;
            *state.aux_interrupt_handle.lock() = previous_aux_handle;
            return Err(e);
        }
    };

    // Publish both new connections, the drop of the previous file's table-keyed
    // caches, and this file's row counts as one critical section, under the
    // `conn` lock. Ordering invariant: `query_table` holds `conn` for its whole
    // duration and only ever builds a rowid/ordered-rows cache while holding
    // it, so doing the swap-and-clear under the same lock guarantees no query
    // can observe the new connection alongside the old file's cached rowids -
    // which would serve one page of stale rowids against the new file. Clearing
    // *after* publishing the connection (as this once did) left exactly that
    // window.
    //
    // Lock ordering, and this is the only place that takes more than one of
    // them: `conn` -> `aux_conn` -> `table_counts`. Every reader takes exactly
    // one connection lock and never reaches for the other, so this is the only
    // site that could deadlock and it always acquires in that order.
    //
    // The counts seeded here are the ones `get_tables_inner` just measured. The
    // file is a frozen snapshot for this connection's lifetime, so they cannot
    // go stale, and seeding them means the first browse of a table does not
    // count it a second time. `-1` marks a table whose count failed; storing
    // that would hand a wrong total to the grid, so those are skipped and
    // counted on demand instead.
    {
        let mut conn_guard = state.conn.lock();
        let mut aux_guard = state.aux_conn.lock();
        clear_caches(state);
        let mut counts = state.table_counts.lock();
        for table in &tables {
            if table.row_count >= 0 {
                counts.insert(table.name.clone(), table.row_count);
            }
        }
        drop(counts);
        *conn_guard = Some(conn);
        *aux_guard = Some(aux_conn);
    }
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
        let count: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {}", quote_ident(&name)),
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|e| {
                warn!(table = %name, error = %e, "Failed to count rows");
                -1
            });
        tables.push(TableInfo {
            name,
            row_count: count,
        });
    }
    Ok(tables)
}

/// `PRAGMA table_xinfo`'s `hidden` code for a hidden virtual-table column
/// (fts5's `<name>` and `rank`): declared and queryable by name, but NOT
/// returned by `SELECT *`. The other codes - 0 ordinary, 2 generated virtual,
/// 3 generated stored - are all returned by `SELECT *` and so are all kept.
const HIDDEN_VIRTUAL_TABLE: i64 = 1;

/// A table's columns, split into the two lists dblitz needs.
///
/// Both come from one `PRAGMA table_xinfo` pass, and that is the whole point:
/// `PRAGMA table_info` **omits generated columns entirely**, which makes each
/// list below wrong in its own individually silent way.
pub(super) struct TableColumns {
    /// Exactly what `SELECT *` returns, in its order: ordinary columns plus
    /// virtual and stored generated ones, with hidden virtual-table columns
    /// dropped.
    ///
    /// Positional agreement with `SELECT *` is load-bearing, not incidental.
    /// `build_where_clause` turns a regex filter into a *column index* into
    /// this list (`filters.rs`), and the row scanner reads the value back out
    /// of the result row at that index. Built from `table_info`, a table like
    /// `CREATE TABLE t(a, b AS (a * 2), c)` yields `[a, c]`, so a regex filter
    /// on `c` resolves to index 1 and silently matches `b`'s values instead.
    pub visible: Vec<String>,
    /// Every declared name, hidden virtual-table columns included. Used only to
    /// decide whether a rowid alias is shadowed, where a missed shadow is
    /// unsafe and a spurious one merely costs the rowid fast path - so this
    /// list deliberately errs wide.
    pub declared: Vec<String>,
    /// Full `table_xinfo` detail for exactly the columns in [`Self::visible`],
    /// in the same order and 1:1 with it. This is what the Structure tab shows
    /// and what the global filter reads declared types from, so it comes out of
    /// the same PRAGMA pass rather than a second one that could disagree.
    pub info: Vec<ColumnInfo>,
}

/// Read a table's columns through `PRAGMA table_xinfo`.
///
/// Nothing in dblitz's own query building may use `table_info`, because its
/// omission of generated columns corrupts two separate things: the visible
/// column list stops matching `SELECT *` positionally (see
/// [`TableColumns::visible`]), and rowid-shadow detection stops seeing a
/// generated column that shadows `rowid`. The second is the dangerous one - a
/// `rowid` generated column is a legal, non-unique, non-monotonic expression,
/// and mistaking it for the real rowid feeds duplicate values into the
/// boundary cache every deep page seeks against.
///
/// `quoted_table` must already be quoted by the caller.
pub(super) fn table_columns(conn: &Connection, quoted_table: &str) -> Result<TableColumns, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_xinfo({})", quoted_table))
        .str_err()?;

    let rows: Vec<(ColumnInfo, i64)> = stmt
        .query_map([], |row| {
            Ok((
                ColumnInfo {
                    cid: row.get(0)?,
                    name: row.get(1)?,
                    col_type: row.get::<_, String>(2).unwrap_or_default(),
                    notnull: row.get::<_, bool>(3).unwrap_or(false),
                    default_value: row.get(4).ok(),
                    pk: row.get::<_, bool>(5).unwrap_or(false),
                },
                row.get::<_, i64>(6).unwrap_or(0),
            ))
        })
        .str_err()?
        .collect::<Result<Vec<_>, _>>()
        .str_err()?;

    let mut visible = Vec::with_capacity(rows.len());
    let mut declared = Vec::with_capacity(rows.len());
    let mut info = Vec::with_capacity(rows.len());
    for (column, hidden) in rows {
        declared.push(column.name.clone());
        // Drop only what `SELECT *` also withholds, so the Structure tab and
        // Browse Data agree on what the table has.
        if hidden != HIDDEN_VIRTUAL_TABLE {
            visible.push(column.name.clone());
            info.push(column);
        }
    }

    Ok(TableColumns {
        visible,
        declared,
        info,
    })
}

/// The columns the Structure tab lists: exactly what `SELECT *` returns, with
/// the full `table_xinfo` detail. Routed through [`table_columns`] rather than
/// reading the PRAGMA again here - the two lists have to agree about which
/// columns exist and in what order, and one reader is the only way to guarantee
/// that.
pub fn get_columns(state: &DbState, table: &str) -> Result<Vec<ColumnInfo>, String> {
    // The Structure tab reads the secondary connection: it must answer while
    // Browse Data is mid-sort, and it consults none of the table-keyed caches
    // the browse connection owns.
    let guard = state.aux_conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    Ok(table_columns(conn, &quote_ident(table))?.info)
}

pub fn get_schema(state: &DbState) -> Result<Vec<SchemaEntry>, String> {
    // Secondary connection, for the same reason as `get_columns`.
    let guard = state.aux_conn.lock();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::close_database;
    use rusqlite::Connection;

    /// Build a temp DB with the given schema/data, then reopen it through the
    /// real `open_database` path so tests exercise the same authorizer-bearing,
    /// read-only connection production uses.
    fn open_temp_db(setup_sql: &str) -> (DbState, std::path::PathBuf) {
        let path = crate::db::util::unique_temp_path("dblitz_schema_test", ".sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(setup_sql).unwrap();
        }
        let state = DbState::new();
        open_database(&state, path.to_str().unwrap()).unwrap();
        (state, path)
    }

    #[test]
    fn get_schema_returns_tables_views_indexes_triggers_with_sql() {
        // One of each object kind. An INTEGER PRIMARY KEY doesn't spawn an
        // autoindex (it's the rowid), so the only index is the explicit one -
        // keeping the expected set exactly four entries with non-null SQL.
        let (state, path) = open_temp_db(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             CREATE INDEX users_name_idx ON users(name);
             CREATE VIEW active_users AS SELECT id FROM users;
             CREATE TRIGGER users_guard AFTER INSERT ON users BEGIN SELECT 1; END;",
        );

        let entries = get_schema(&state).unwrap();

        // `ORDER BY type, name` => index, table, trigger, view.
        let types: Vec<&str> = entries.iter().map(|e| e.obj_type.as_str()).collect();
        assert_eq!(types, vec!["index", "table", "trigger", "view"]);
        assert!(
            entries.iter().all(|e| e.sql.is_some()),
            "every created object has a non-null CREATE statement, got: {entries:?}"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_database_does_not_memory_map_the_file() {
        // `?immutable=1` is a promise dblitz makes to SQLite, not one the OS
        // enforces: another process can still truncate the file. Unmapped, that
        // is a recoverable "database disk image is malformed"; mapped, it is a
        // SIGBUS that kills the process with no message. Both were reproduced.
        // So mmap must stay off, and this is the assertion that says so - the
        // PRAGMA it guards against reads as pure tuning at a glance.
        let (state, path) = open_temp_db("CREATE TABLE t (a INTEGER);");

        let guard = state.conn.lock();
        let conn = guard.as_ref().unwrap();
        // `mmap_size` is not on the introspection allowlist - deliberately, so
        // no ad-hoc query can turn mapping back on - so the authorizer has to
        // come off to ask the connection about itself. Dropping it changes what
        // statements are permitted, never what the connection is configured to
        // do, so the numbers below are the real ones.
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let mmap_size: i64 = conn
            .query_row("PRAGMA mmap_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mmap_size, 0, "the database file must not be memory-mapped");
        // The control: the other two open-time PRAGMAs ARE set, so this is a
        // test about mmap and not about a batch that silently stopped running.
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cache_size, -64_000);
        drop(guard);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_failed_open_leaves_the_previous_database_open_and_cancellable() {
        // What this pins is the user-visible property: opening a broken file
        // must not take away the database already open, and Cancel must still
        // stop a query on it.
        //
        // What it does NOT pin, stated plainly because the name suggests
        // otherwise: the `previous_handle` restore in `open_database`'s error
        // arm. That arm turns out to be unreachable through any file that can
        // be constructed here - measured, not assumed. `execute_batch` for the
        // open-time PRAGMAs prepares statements, which loads the schema, so a
        // file with a corrupt schema page fails THERE, before the handle is
        // replaced at all; a file of random bytes fails earlier still, at
        // connect. And `get_tables_inner`, the only step after the replacement,
        // swallows a per-table `COUNT(*)` failure as `-1` rather than
        // returning an error. Deleting the restore leaves this test green
        // (confirmed by running it that way), so the restore is two lines of
        // defence with no live caller, not a guard this test covers.
        use std::sync::Arc;
        use std::time::Duration;

        let (state, path) = open_temp_db("CREATE TABLE t (a INTEGER);");

        let corrupt = crate::db::util::unique_temp_path("dblitz_corrupt_schema", ".sqlite");
        {
            let conn = Connection::open(&corrupt).unwrap();
            conn.execute_batch("CREATE TABLE alpha (a); CREATE TABLE beta (b);")
                .unwrap();
        }
        let mut bytes = std::fs::read(&corrupt).unwrap();
        // Keep the 100-byte header valid and scribble over the schema page, so
        // the file connects and then fails on its first real read.
        for byte in bytes.iter_mut().take(1024).skip(100) {
            *byte = 0xAA;
        }
        std::fs::write(&corrupt, &bytes).unwrap();
        let error = open_database(&state, corrupt.to_str().unwrap())
            .expect_err("a database with a corrupt schema page must fail to open");
        assert!(
            error.to_ascii_lowercase().contains("malformed"),
            "expected a corruption error, got: {error}"
        );

        // The first database is still there.
        assert_eq!(
            get_columns(&state, "t").unwrap().len(),
            1,
            "a failed open must not disturb the database already open"
        );

        // ...and still cancellable. A grinding recursive CTE has no natural
        // end, so if the handle in `DbState` did not address this connection
        // the join below would never return and this test would hang rather
        // than fail - the same shape as `cancel_interrupts_long_running_sql`.
        let state = Arc::new(state);
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            crate::db::execute_sql(
                &worker_state,
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c) \
                 SELECT count(*) FROM c",
            )
        });
        std::thread::sleep(Duration::from_millis(200));
        crate::db::cancel_queries(&state);
        let result = worker.join().expect("the query thread should not panic");

        assert!(
            result.error.is_some(),
            "the still-open database must remain cancellable after a failed open"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&corrupt);
    }

    #[test]
    fn get_columns_reports_pk_notnull_default() {
        // A single column carrying all three flags at once: NOT NULL, a string
        // DEFAULT, and PRIMARY KEY. table_info reports the default as the raw
        // SQL text of the literal, i.e. including the quotes.
        let (state, path) =
            open_temp_db("CREATE TABLE t (a TEXT NOT NULL DEFAULT 'x' PRIMARY KEY, b INTEGER);");

        let columns = get_columns(&state, "t").unwrap();

        let a = columns.iter().find(|c| c.name == "a").expect("column a");
        assert!(a.notnull, "column a is declared NOT NULL");
        assert!(a.pk, "column a is the PRIMARY KEY");
        assert_eq!(a.default_value.as_deref(), Some("'x'"));

        let b = columns.iter().find(|c| c.name == "b").expect("column b");
        assert!(!b.notnull);
        assert!(!b.pk);
        assert_eq!(b.default_value, None);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn get_columns_lists_generated_columns_with_contiguous_cids() {
        // `table_info` omits generated columns and leaves gaps in the `cid`
        // values where it skipped them, so the Structure tab used to show
        // `a`(0) and `z`(3) for this table while Browse Data showed four
        // columns. `table_xinfo` reports all four, numbered 0..3.
        let (state, path) =
            open_temp_db("CREATE TABLE t (a INTEGER, v AS (a * 2), s AS (a * 3) STORED, z TEXT);");

        let columns = get_columns(&state, "t").unwrap();

        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "v", "s", "z"]);
        let cids: Vec<i64> = columns.iter().map(|c| c.cid).collect();
        assert_eq!(cids, vec![0, 1, 2, 3]);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn get_columns_hides_virtual_table_hidden_columns() {
        // The control for the filter: `table_xinfo` also reports columns
        // `SELECT *` does NOT return - fts5 declares a hidden column named
        // after the table plus `rank`. Listing those would make the Structure
        // tab claim columns Browse Data never shows, so `hidden = 1` is the one
        // kind that stays filtered out.
        let (state, path) = open_temp_db("CREATE VIRTUAL TABLE ft USING fts5(title, body);");

        let columns = get_columns(&state, "ft").unwrap();

        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["title", "body"]);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn secondary_connection_enforces_every_read_only_layer() {
        // The second connection is the one no `execute_sql` test reaches
        // through the gates in `sql.rs`, so it is exactly where a divergence
        // between the two opens would hide: it would still answer SELECTs
        // correctly and simply stop being read-only. Every layer named in
        // AGENTS.md is checked directly on it.
        //
        // The reason a single `open_read_only_connection` builds both is this
        // test's premise, not a tidiness preference - and the test is what
        // makes the premise falsifiable.
        let (state, path) = open_temp_db("CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);");

        let guard = state.aux_conn.lock();
        let conn = guard
            .as_ref()
            .expect("the secondary connection must be open");

        // 1. SQLITE_OPEN_READ_ONLY + ?immutable=1: a write is refused by the
        //    engine, with `execute_sql`'s `stmt.readonly()` gate bypassed
        //    entirely (an INSERT never reaches it here).
        let err = conn
            .execute("INSERT INTO t (a) VALUES (2)", [])
            .expect_err("the secondary connection must refuse an INSERT");
        assert!(
            err.to_string().to_ascii_lowercase().contains("readonly"),
            "expected a read-only refusal from SQLite, got: {err}"
        );

        // 2. The authorizer: ATTACH is denied on the parsed statement, so no
        //    lexical prefix trick and no missing string gate can reach it.
        let err = conn
            .prepare("ATTACH ':memory:' AS s")
            .expect_err("the authorizer must deny ATTACH on the secondary connection");
        assert!(
            err.to_string()
                .to_ascii_lowercase()
                .contains("not authorized"),
            "expected an authorization error, got: {err}"
        );

        // 3. The PRAGMA allowlist: a state-changing PRAGMA is refused.
        let err = conn
            .prepare("PRAGMA journal_mode = wal")
            .expect_err("a non-allowlisted PRAGMA must be denied on the secondary connection");
        assert!(
            err.to_string()
                .to_ascii_lowercase()
                .contains("not authorized"),
            "expected an authorization error, got: {err}"
        );

        // 4. The open batch really ran on this connection too: no memory
        //    mapping (see `open_database_does_not_memory_map_the_file` for why
        //    that matters), with `cache_size` as the control proving the batch
        //    is present rather than the whole thing being silently skipped.
        //    `mmap_size` is off the allowlist deliberately, so the authorizer
        //    has to come off to ask - which changes what statements are
        //    permitted, never how the connection is configured.
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        let mmap_size: i64 = conn
            .query_row("PRAGMA mmap_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            mmap_size, 0,
            "the secondary connection must not memory-map the file either"
        );
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cache_size, -64_000);
        drop(guard);

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_interrupts_the_browse_connection() {
        // The mirror of `sql::tests::cancel_interrupts_the_secondary_connection`,
        // and needed for the same reason: an interrupt handle can only stop a
        // statement on the connection it was taken from, so `cancel_queries`
        // has two arms and either one deleted alone leaves the other test
        // green. Nothing else covers the browse arm any more - every
        // `execute_sql`-based cancellation test now exercises the secondary
        // connection.
        //
        // The grinding CTE is run on `state.conn` directly rather than through
        // `query_table`, because no legal `QueryRequest` grinds forever; what
        // is being pinned is that `state.interrupt_handle` addresses THAT
        // connection.
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let (state, path) = open_temp_db("CREATE TABLE t (a INTEGER);");
        let state = Arc::new(state);
        let worker_state = Arc::clone(&state);

        let worker = std::thread::spawn(move || {
            let guard = worker_state.conn.lock();
            let conn = guard.as_ref().unwrap();
            conn.query_row(
                "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c) \
                 SELECT count(*) FROM c",
                [],
                |row| row.get::<_, i64>(0),
            )
        });

        std::thread::sleep(Duration::from_millis(200));
        let cancel_started = Instant::now();
        crate::db::cancel_queries(&state);
        // If the handle did not address the browse connection this join would
        // never return, so the failure mode is a hang rather than an assertion.
        let result = worker.join().expect("the query thread should not panic");
        let elapsed = cancel_started.elapsed();

        assert!(
            elapsed < Duration::from_secs(5),
            "cancel should interrupt the browse connection within a bounded time, took {elapsed:?}"
        );
        assert!(
            result.is_err(),
            "an interrupted query should return an error, got: {result:?}"
        );

        close_database(&state);
        let _ = std::fs::remove_file(&path);
    }
}
