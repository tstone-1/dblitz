use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags};
use std::sync::atomic::Ordering;
use tracing::{error, info, warn};

use super::clear_caches;
use super::types::{ColumnInfo, DbState, SchemaEntry, TableInfo};
use super::util::{path_to_sqlite_uri, quote_ident, StrErr};

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
    conn.authorizer(Some(|ctx: AuthContext<'_>| match ctx.action {
        AuthAction::Attach { .. }
        | AuthAction::Detach { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }))
    .str_err()?;
    // Read-only/immutable tuning, all safe because the file is a frozen
    // snapshot (no writer, no WAL):
    //   - cache_size=-64000  : 64 MiB page cache (negative = KiB, not pages).
    //   - mmap_size          : map up to 1 GiB so page reads skip read()
    //                          syscalls and the pager's double-buffer copy.
    //                          SQLite silently caps this at the file size.
    //   - temp_store=MEMORY  : keep sorter/temp-b-tree scratch in RAM so a
    //                          non-indexed ORDER BY (e.g. sorting a filtered
    //                          view) never spills to a temp file on disk.
    conn.execute_batch(
        "PRAGMA cache_size=-64000;\
         PRAGMA mmap_size=1073741824;\
         PRAGMA temp_store=MEMORY;",
    )
    .str_err()?;

    let interrupt_handle = conn.get_interrupt_handle();
    let tables = get_tables_inner(&conn)?;

    *state.conn.lock() = Some(conn);
    *state.interrupt_handle.lock() = Some(interrupt_handle);
    *state.current_path.lock() = Some(path.to_string());
    clear_caches(state);

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
