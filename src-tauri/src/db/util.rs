/// Escape a SQL identifier (table/column name) for safe use in double-quoted contexts.
pub(super) fn safe_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

/// Converts any error with Display into Result<T, String>.
pub(crate) trait StrErr<T> {
    fn str_err(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> StrErr<T> for Result<T, E> {
    fn str_err(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}

/// Convert an OS file path into a SQLite URI with `?immutable=1`. Percent-
/// encodes the few characters that have special meaning in URIs and
/// normalizes Windows backslashes to forward slashes.
pub(super) fn path_to_sqlite_uri(path: &str) -> String {
    // Percent-encode in this order: % first (so we don't double-encode our
    // own escapes), then the others.
    let encoded = path
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('\\', "/");
    // UNC path "//server/share/db.sqlite" -> "file:////server/share/db.sqlite?immutable=1".
    // The four-slash form keeps the URI authority empty; "file://server/..."
    // would be parsed as a non-local authority and rejected by SQLite.
    if encoded.starts_with("//") {
        format!("file://{}?immutable=1", encoded)
    // Unix path "/foo/bar" -> "file:/foo/bar?immutable=1"
    // Windows path "C:/foo/bar" -> "file:/C:/foo/bar?immutable=1"
    } else if encoded.starts_with('/') {
        format!("file:{}?immutable=1", encoded)
    } else {
        format!("file:/{}?immutable=1", encoded)
    }
}

pub(super) fn read_row(row: &rusqlite::Row, col_count: usize) -> Vec<Option<String>> {
    let mut values: Vec<Option<String>> = Vec::with_capacity(col_count);
    for i in 0..col_count {
        let val: Option<String> = row
            .get::<_, rusqlite::types::Value>(i)
            .ok()
            .map(|v| match v {
                rusqlite::types::Value::Null => None,
                rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                rusqlite::types::Value::Real(f) => Some(f.to_string()),
                rusqlite::types::Value::Text(s) => Some(s),
                rusqlite::types::Value::Blob(b) => Some(format!("[BLOB {} bytes]", b.len())),
            })
            .unwrap_or(None);
        values.push(val);
    }
    values
}

/// Execute a prepared statement and collect all rows into a Vec.
pub(super) fn collect_rows(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_ident_escapes_quotes() {
        assert_eq!(safe_ident("normal"), "normal");
        assert_eq!(safe_ident("has\"quote"), "has\"\"quote");
        assert_eq!(safe_ident("two\"\"quotes"), "two\"\"\"\"quotes");
    }

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
        assert_eq!(
            path_to_sqlite_uri(r"\\server\share\db.sqlite"),
            "file:////server/share/db.sqlite?immutable=1"
        );
    }
}
