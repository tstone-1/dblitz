/// Escape a SQL identifier (table/column name) for safe use in double-quoted contexts.
/// Prefer [`quote_ident`] at call sites that interpolate the result directly into
/// SQL — it wraps the quotes for you, removing the "did I remember to quote
/// this" footgun. Kept `pub(super)` for `quote_ident` itself and for tests.
pub(super) fn safe_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

/// Fully quote a SQL identifier (table/column name) for direct interpolation
/// into a SQL string: escapes embedded double quotes and wraps the result in
/// double quotes. Unlike [`safe_ident`] alone, the caller can't forget the
/// wrapping quotes — that used to be a real footgun (every call site had to
/// remember `"\"{}\""` by hand).
pub(super) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", safe_ident(name))
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

/// Prefixes a human-readable operation description onto an already-stringified
/// error, so a command-boundary failure reaching `appState.error` in the UI
/// says what dblitz was doing (e.g. which table) instead of a bare
/// SQLite/IO message. `tracing` already carries this context structurally
/// via its fields; this carries the same context to the user-facing channel.
pub(crate) trait ErrCtx<T> {
    fn err_ctx(self, context: &str) -> Result<T, String>;
}

impl<T> ErrCtx<T> for Result<T, String> {
    fn err_ctx(self, context: &str) -> Result<T, String> {
        self.map_err(|e| format!("{context}: {e}"))
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
        let val: Option<String> = match row.get::<_, rusqlite::types::Value>(i) {
            Ok(rusqlite::types::Value::Null) => None,
            Ok(rusqlite::types::Value::Integer(n)) => Some(n.to_string()),
            Ok(rusqlite::types::Value::Real(f)) => Some(f.to_string()),
            Ok(rusqlite::types::Value::Text(s)) => Some(s),
            Ok(rusqlite::types::Value::Blob(b)) => Some(format!("[BLOB {} bytes]", b.len())),
            // `Row::get::<_, Value>` errors outright on a TEXT cell that isn't
            // valid UTF-8 (it can't materialize the cell as a Rust String) -
            // silently mapping that error to `.ok()` -> None turned a real
            // cell into a phantom NULL. Fall back to the raw storage and
            // lossily decode text instead of losing the cell; other per-cell
            // errors (there shouldn't be any at a valid index) still fall
            // through to None below.
            Err(_) => match row.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Text(bytes)) => {
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
                _ => None,
            },
        };
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

/// Build a collision-free path under the OS temp dir for a test fixture.
///
/// The obvious `SystemTime::now().as_nanos()` is *not* collision-free here, and
/// three fixtures used it. `cargo test` runs tests on parallel threads, and two
/// of them can read the same nanosecond — the clock is not re-read fast enough
/// to separate them — so both land on the same filename, and the second
/// `CREATE TABLE` fails with "table users already exists". That produced an
/// intermittent red `execute_sql_rejects_writes_with_friendly_message` that
/// passed on every re-run (seen 2026-07-25, during a release). A process id plus
/// a monotonic counter is unique by construction, across both concurrent threads
/// and concurrently running test binaries.
#[cfg(test)]
pub(super) fn unique_temp_path(prefix: &str, suffix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}_{}_{id}{suffix}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_temp_path_never_repeats_within_a_process() {
        // The regression this guards: the previous nanosecond-timestamp scheme
        // could return the same path twice in a tight loop.
        let paths: std::collections::HashSet<_> = (0..1000)
            .map(|_| unique_temp_path("t", ".sqlite"))
            .collect();
        assert_eq!(paths.len(), 1000, "unique_temp_path handed out a duplicate");
    }

    #[test]
    fn err_ctx_prefixes_context_onto_error() {
        let result: Result<(), String> = Err("no such table: ghost".to_string());
        let err = result.err_ctx("querying table \"ghost\"").unwrap_err();
        assert_eq!(err, "querying table \"ghost\": no such table: ghost");

        let ok: Result<i32, String> = Ok(42);
        assert_eq!(ok.err_ctx("irrelevant"), Ok(42));
    }

    #[test]
    fn safe_ident_escapes_quotes() {
        assert_eq!(safe_ident("normal"), "normal");
        assert_eq!(safe_ident("has\"quote"), "has\"\"quote");
        assert_eq!(safe_ident("two\"\"quotes"), "two\"\"\"\"quotes");
    }

    #[test]
    fn quote_ident_wraps_and_escapes() {
        assert_eq!(quote_ident("normal"), "\"normal\"");
        assert_eq!(quote_ident("has\"quote"), "\"has\"\"quote\"");
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

    #[test]
    fn read_row_recovers_invalid_utf8_text_as_lossy_string() {
        // CAST(x'FF' AS TEXT) stores a TEXT-storage-class cell holding a raw
        // byte that isn't valid UTF-8 on its own (a lone continuation byte) -
        // SQLite doesn't validate UTF-8 for TEXT storage, only rusqlite's
        // `Value` conversion does, and that's exactly the case this guards.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (col TEXT);
             INSERT INTO t (col) VALUES (CAST(x'FF' AS TEXT));",
        )
        .unwrap();

        let mut stmt = conn.prepare("SELECT col FROM t").unwrap();
        let col_count = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let values = read_row(row, col_count);

        assert_eq!(values.len(), 1);
        assert!(
            values[0].is_some(),
            "invalid-UTF-8 TEXT cell must not silently become NULL"
        );
        // U+FFFD is the lossy-decode replacement character for the invalid byte.
        assert_eq!(values[0].as_deref(), Some("\u{FFFD}"));
    }
}
