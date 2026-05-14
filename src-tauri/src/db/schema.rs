use super::{ColumnInfo, DbState, SchemaEntry, StrErr, TableInfo};
use crate::db::safe_ident;
use rusqlite::{Connection, OpenFlags};
use tracing::{error, info, warn};

pub fn close_database(state: &DbState) {
    info!("Closing database");
    *state.conn.lock() = None;
    *state.current_path.lock() = None;
    state.rowid_indexes.lock().clear();
}

/// Convert an OS file path into a SQLite URI with `?immutable=1`. Percent-
/// encodes the few characters that have special meaning in URIs and
/// normalizes Windows backslashes to forward slashes.
fn path_to_sqlite_uri(path: &str) -> String {
    let encoded = path
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('\\', "/");
    if encoded.starts_with('/') {
        format!("file:{}?immutable=1", encoded)
    } else {
        format!("file:/{}?immutable=1", encoded)
    }
}

pub fn open_database(state: &DbState, path: &str) -> Result<Vec<TableInfo>, String> {
    info!(path, "Opening database (read-only, immutable)");
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

#[cfg(test)]
mod tests {
    use super::path_to_sqlite_uri;

    #[test]
    fn path_to_sqlite_uri_encodes_special_chars() {
        assert_eq!(
            path_to_sqlite_uri("/home/user/db.sqlite"),
            "file:/home/user/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\Users\mail\db.sqlite"),
            "file:/C:/Users/mail/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\foo bar\db.sqlite"),
            "file:/C:/foo%20bar/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\with#hash\db.sqlite"),
            "file:/C:/with%23hash/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\with?question\db.sqlite"),
            "file:/C:/with%3Fquestion/db.sqlite?immutable=1"
        );
        assert_eq!(
            path_to_sqlite_uri(r"C:\with%percent\db.sqlite"),
            "file:/C:/with%25percent/db.sqlite?immutable=1"
        );
    }
}
