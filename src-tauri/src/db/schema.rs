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

pub fn open_database(state: &DbState, path: &str) -> Result<Vec<TableInfo>, String> {
    info!(path, "Opening database (read-only, immutable)");
    state.query_generation.fetch_add(1, Ordering::Relaxed);
    // Interrupt any query still running against the previous connection so
    // this open doesn't block waiting for `state.conn`'s lock behind it, and
    // so a grinding query on the old file doesn't keep running pointlessly
    // once the user has moved on to a new one.
    if let Some(handle) = state.interrupt_handle.lock().as_ref() {
        handle.interrupt();
    }
    // dblitz is a viewer, not an editor. Two layers of read-only:
    //   1. SQLITE_OPEN_READ_ONLY at the connection layer.
    //   2. ?immutable=1 in the URI tells SQLite to treat the file as a
    //      frozen snapshot.
    let uri = path_to_sqlite_uri(path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(&uri, flags).map_err(|e| {
        error!(path, error = %e, "Failed to open database");
        e.to_string()
    })?;
    // Read-only/immutable tuning, all safe because the file is a frozen
    // snapshot (no writer, no WAL):
    //   - cache_size=-64000  : 64 MiB page cache (negative = KiB, not pages).
    //   - mmap_size          : map up to 1 GiB so page reads skip read()
    //                          syscalls and the pager's double-buffer copy.
    //                          SQLite silently caps this at the file size.
    //   - temp_store=MEMORY  : keep sorter/temp-b-tree scratch in RAM so a
    //                          non-indexed ORDER BY (e.g. sorting a filtered
    //                          view) never spills to a temp file on disk.
    // These run BEFORE the authorizer is installed, deliberately: they are
    // exactly the kind of session-configuration PRAGMA the authorizer denies
    // (see `ALLOWED_INTROSPECTION_PRAGMAS`). The app needs them once at open;
    // installing the authorizer afterwards keeps them out of the allowlist so
    // no ad-hoc query can replay them and reshape Browse Data semantics.
    conn.execute_batch(
        "PRAGMA cache_size=-64000;\
         PRAGMA mmap_size=1073741824;\
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

    let interrupt_handle = conn.get_interrupt_handle();
    let tables = get_tables_inner(&conn)?;

    // Publish the new connection and drop the previous file's table-keyed
    // caches as one critical section, both under the `conn` lock. Ordering
    // invariant: `query_table` holds `conn` for its whole duration and only
    // ever builds a rowid/ordered-rows cache while holding it, so doing the
    // swap-and-clear under the same lock guarantees no query can observe the
    // new connection alongside the old file's cached rowids - which would
    // serve one page of stale rowids against the new file. Clearing *after*
    // publishing the connection (as this once did) left exactly that window.
    {
        let mut conn_guard = state.conn.lock();
        clear_caches(state);
        *conn_guard = Some(conn);
    }
    *state.interrupt_handle.lock() = Some(interrupt_handle);
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

pub fn get_tables(state: &DbState) -> Result<Vec<TableInfo>, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    get_tables_inner(conn)
}

pub fn get_columns(state: &DbState, table: &str) -> Result<Vec<ColumnInfo>, String> {
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;

    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote_ident(table)))
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
}
