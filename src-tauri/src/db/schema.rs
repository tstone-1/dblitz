use rusqlite::{Connection, OpenFlags};
use std::sync::atomic::Ordering;
use tracing::{error, info, warn};

use super::types::{ColumnInfo, DbState, SchemaEntry, TableInfo};
use super::util::{path_to_sqlite_uri, safe_ident, StrErr};

pub fn open_database(state: &DbState, path: &str) -> Result<Vec<TableInfo>, String> {
    info!(path, "Opening database (read-only, immutable)");
    state.query_generation.fetch_add(1, Ordering::Relaxed);
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
    conn.execute_batch("PRAGMA cache_size=-64000;").str_err()?;

    let tables = get_tables_inner(&conn)?;

    *state.conn.lock() = Some(conn);
    *state.current_path.lock() = Some(path.to_string());
    state.rowid_indexes.lock().clear();
    state.sorted_orders.lock().clear();

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
                &format!("SELECT COUNT(*) FROM \"{}\"", safe_ident(&name)),
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
        .prepare(&format!("PRAGMA table_info(\"{}\")", safe_ident(table)))
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
