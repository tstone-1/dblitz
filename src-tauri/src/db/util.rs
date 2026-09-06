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
pub fn quote_ident(name: &str) -> String {
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
pub(crate) fn path_to_sqlite_uri(path: &str) -> String {
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

/// Render one cell the way SQLite's own `CAST(x AS TEXT)` would.
///
/// Every cell dblitz shows and every value its filters compare against passes
/// through here, so this is where the grid and the WHERE clause agree - or, as
/// they did until this function existed, silently disagree.
///
/// Reads through `get_ref`, never `Row::get::<Value>`: `Value` copies a BLOB's
/// entire contents into a `Vec<u8>` only for the length to be printed, and it
/// errors outright on a TEXT cell that is not valid UTF-8 (SQLite does not
/// validate TEXT storage; rusqlite's `String` conversion does). `ValueRef`
/// borrows, so a 10 MB BLOB costs nothing and invalid UTF-8 decodes lossily
/// rather than turning a real cell into a phantom NULL.
pub(super) fn render_cell(value: rusqlite::types::ValueRef<'_>) -> Option<String> {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(n) => Some(n.to_string()),
        ValueRef::Real(f) => Some(render_real(f)),
        ValueRef::Text(bytes) => Some(match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(bytes).into_owned(),
        }),
        ValueRef::Blob(b) => Some(format!("[BLOB {} bytes]", b.len())),
    }
}

/// Render an `f64` the way SQLite renders a REAL as text, i.e. what
/// `CAST(col AS TEXT)`, `printf('%s', col)` and the `sqlite3` shell print.
///
/// Rust's `f64::Display` is NOT that, and the difference is not cosmetic: it
/// prints `3` where SQLite prints `3.0`, and `100000000000000000000` where
/// SQLite prints `1.0e+20`. dblitz's filters run inside SQLite, and `LIKE`
/// stringifies a REAL operand with SQLite's own conversion - so a user who
/// filtered on the number the grid had just shown them got nothing back. The
/// fix belongs here, in the rendering, because the grid is the side that was
/// lying about the value.
///
/// **Layout** is SQLite's, exactly: fixed notation while the decimal exponent
/// is in `-4..=16` and exponent notation outside it (`1.0e+17`, `1.0e-05`,
/// two-digit minimum exponent with an explicit sign), and a whole number always
/// keeps one decimal place - `3.0`, never `3` - which is what distinguishes a
/// REAL from an INTEGER on sight.
///
/// **Digits** are Rust's shortest round-tripping form, which is NOT always what
/// SQLite emits, and this is the one thing that could not be matched. SQLite
/// renders through `sqlite3FpDecode`, a fixed-point decoder whose last digit or
/// two are approximate, so it prints values no correctly-rounded formatter will
/// reproduce: `0.33333333333333332` for a double whose exact 17-digit rounding
/// ends `...331`, `0.019607843137254902` for one whose shortest form is the
/// 15-digit `0.0196078431372549`, `9.876543209999999e+18` for `9.87654321e18`.
/// Measured against SQLite 3.53.2 over 30,859 values, agreement is exact for
/// every normal value whose shortest form needs at most 13 significant digits
/// with a decimal exponent within +/-16 (14,227 of them, zero mismatches), and
/// for the rest the two texts always denote the *same double* (16,632 values,
/// zero disagreements) while sometimes differing in their last digits.
/// `render_real_matches_sqlite_cast_as_text` pins both halves of that.
///
/// What the residual divergence costs: dblitz's LIKE filters are `%contains%`
/// and SQLite's longer form almost always extends ours, so filtering on a
/// displayed value still matches; and `=` on a column with numeric affinity
/// converts the operand back to a double, where the round-trip guarantee makes
/// it exact. The gap that remains is `=` against a column with no affinity at
/// all, which `build_where_clause` closes separately (see its numeric-equality
/// arm).
pub(super) fn render_real(value: f64) -> String {
    // Neither is storable in a REAL cell (an INSERT of NaN stores NULL), so
    // these only arrive from a computed expression in the SQL editor. SQLite
    // prints infinities as `Inf`/`-Inf` and has no text form for NaN.
    if value.is_nan() {
        return "NULL".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Inf" } else { "-Inf" }.to_string();
    }
    // `-0.0` renders as `0.0`: SQLite's decoder drops the sign of zero, and
    // `{:e}` below would keep it.
    if value == 0.0 {
        return "0.0".to_string();
    }

    // `{:e}` is Rust's shortest round-tripping representation in scientific
    // form (`d.ddde<exp>`), which gives the significant digits and the decimal
    // exponent in one pass. Deriving the exponent from `log10` instead rounds
    // the wrong way exactly at the powers of ten, where the notation switches.
    let scientific = format!("{:e}", value);
    let (mantissa, exponent) = scientific
        .rsplit_once('e')
        .expect("Rust's LowerExp for f64 always emits an exponent");
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();

    if (-4..=16).contains(&exponent) {
        let mut out = String::with_capacity(digits.len() + 8);
        if exponent >= 0 {
            let integer_digits = exponent as usize + 1;
            if digits.len() <= integer_digits {
                out.push_str(&digits);
                for _ in digits.len()..integer_digits {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(&digits[..integer_digits]);
                out.push('.');
                out.push_str(&digits[integer_digits..]);
            }
        } else {
            out.push_str("0.");
            for _ in 0..(-exponent - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        }
        format!("{sign}{out}")
    } else {
        let mut out = String::with_capacity(digits.len() + 8);
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        format!(
            "{sign}{out}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    }
}

pub(super) fn read_row(row: &rusqlite::Row, col_count: usize) -> Vec<Option<String>> {
    read_row_from(row, 0, col_count)
}

/// Read columns `start..col_count` of `row`.
///
/// `start` exists for the scans that prepend a rowid to `SELECT *`: the rowid is
/// read separately as an `i64` and the row that reaches the grid must align with
/// the table's own columns. Skipping it here rather than building the full row
/// and dropping element 0 afterwards is what keeps a page fetch from cloning
/// every cell of every row it returns.
pub(super) fn read_row_from(
    row: &rusqlite::Row,
    start: usize,
    col_count: usize,
) -> Vec<Option<String>> {
    let mut values: Vec<Option<String>> = Vec::with_capacity(col_count.saturating_sub(start));
    for i in start..col_count {
        // A per-cell read error at a valid index shouldn't happen; report it as
        // an empty cell rather than failing the whole page.
        values.push(row.get_ref(i).ok().and_then(render_cell));
    }
    values
}

/// Execute a prepared statement and collect all rows into a Vec.
pub fn collect_rows(
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
            path_to_sqlite_uri(r"C:\Users\alice\db.sqlite"),
            "file:/C:/Users/alice/db.sqlite?immutable=1"
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

    /// Significant digits in a value's shortest round-tripping form.
    fn significant_digits(value: f64) -> usize {
        format!("{:e}", value)
            .rsplit_once('e')
            .map(|(mantissa, _)| mantissa.chars().filter(|c| c.is_ascii_digit()).count())
            .unwrap_or(0)
    }

    fn decimal_exponent(value: f64) -> i32 {
        if value == 0.0 {
            return 0;
        }
        format!("{:e}", value)
            .rsplit_once('e')
            .and_then(|(_, exp)| exp.parse().ok())
            .unwrap_or(0)
    }

    #[test]
    fn render_real_matches_sqlite_cast_as_text() {
        // The claim `render_real` makes is about SQLite's behaviour, so it is
        // checked against SQLite rather than against a table of expected
        // strings written from the same understanding that produced the code.
        //
        // Two classes, both asserted, because exact agreement is achievable for
        // one and provably not for the other (see `render_real`'s doc comment):
        //   - normal, <=13 significant digits, |exponent| <= 16: byte-identical.
        //   - everything else: the two texts must denote the SAME double.
        // Sizes of both classes are asserted too - a classification that
        // selected nothing would pass this test while proving nothing.
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let mut values: Vec<f64> = vec![
            0.0,
            -0.0,
            1.0,
            3.0,
            -3.0,
            0.1,
            2.5,
            1e20,
            -1e20,
            1e-5,
            1e-4,
            1e15,
            1e16,
            1e17,
            123456789012345678.0,
            1.0 / 3.0,
            std::f64::consts::PI,
            1234.5678,
            f64::MIN_POSITIVE,
            5e-324,
            f64::MAX,
            f64::MIN,
        ];
        for exponent in -308i32..=308 {
            for mantissa in ["1", "1.5", "9.87654321", "3.14159265358979", "7"] {
                values.push(format!("{mantissa}e{exponent}").parse().unwrap());
                values.push(format!("-{mantissa}e{exponent}").parse().unwrap());
            }
        }
        for i in 0..300 {
            values.push(i as f64 * 0.7);
            values.push(i as f64);
            values.push(-(i as f64) / 8.0);
        }
        for i in 1..300 {
            values.push(1.0 / i as f64);
            values.push(i as f64 / 7.0);
        }

        let mut exact = 0usize;
        let mut same_double = 0usize;
        for value in values {
            let expected: String = conn
                .query_row("SELECT CAST(? AS TEXT)", [value], |row| row.get(0))
                .unwrap();
            let rendered = render_real(value);

            let pinned = value == 0.0
                || (value.is_normal()
                    && significant_digits(value) <= 13
                    && decimal_exponent(value).abs() <= 16);
            if pinned {
                exact += 1;
                assert_eq!(
                    rendered, expected,
                    "render_real({value:e}) must equal SQLite's CAST(... AS TEXT)"
                );
            } else {
                same_double += 1;
                let ours: f64 = rendered.parse().unwrap_or(f64::NAN);
                let sqlite: f64 = expected.parse().unwrap_or(f64::NAN);
                assert_eq!(
                    ours, sqlite,
                    "render_real({value:e}) = {rendered} and SQLite's {expected} must \
                     denote the same double even where the digits differ"
                );
            }
        }

        assert!(
            exact > 1_000,
            "the exactly-matched class collapsed to {exact} values - a classification \
             that selects almost nothing would pass this test without proving anything"
        );
        assert!(
            same_double > 500,
            "the same-double class collapsed to {same_double} values"
        );
    }

    #[test]
    fn render_real_keeps_whole_numbers_distinguishable_from_integers() {
        // The four shapes the old `f64::Display` rendering got wrong, and the
        // reason the grid disagreed with its own filters.
        assert_eq!(render_real(3.0), "3.0");
        assert_eq!(render_real(1e20), "1.0e+20");
        assert_eq!(render_real(1e-5), "1.0e-05");
        assert_eq!(render_real(-0.0), "0.0");
    }

    #[test]
    fn read_row_renders_a_real_column_the_way_sqlite_does() {
        // End to end through the row reader the grid actually uses, not just
        // the formatter: a REAL cell must reach the frontend spelled the way a
        // filter running inside SQLite would spell it.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (r REAL);
             INSERT INTO t (r) VALUES (3.0), (1e20), (0.1);",
        )
        .unwrap();

        let mut stmt = conn.prepare("SELECT r, CAST(r AS TEXT) FROM t").unwrap();
        let col_count = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        let mut seen = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            let values = read_row(row, col_count);
            assert_eq!(
                values[0], values[1],
                "the grid's rendering of a REAL must equal SQLite's own text form"
            );
            seen.push(values[0].clone().unwrap());
        }
        assert_eq!(seen, vec!["3.0", "1.0e+20", "0.1"]);
    }

    #[test]
    fn read_row_from_skips_the_leading_columns() {
        // The rowid-prefixed scans read `SELECT rowid, *` and must hand the
        // grid the table's own columns, without copying every cell to drop the
        // first one.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (a TEXT, b TEXT);
             INSERT INTO t VALUES ('x', 'y');",
        )
        .unwrap();

        let mut stmt = conn.prepare("SELECT rowid, * FROM t").unwrap();
        let col_count = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();

        assert_eq!(
            read_row_from(row, 1, col_count),
            vec![Some("x".to_string()), Some("y".to_string())]
        );
        assert_eq!(
            read_row(row, col_count),
            vec![
                Some("1".to_string()),
                Some("x".to_string()),
                Some("y".to_string())
            ]
        );
    }

    #[test]
    fn read_row_reports_a_blob_by_length_without_materializing_it() {
        // `Row::get::<Value>` copied the whole BLOB out of SQLite just to print
        // its length. The observable half of that fix is the placeholder still
        // being right; the unobservable half is that a 4 MB cell now costs one
        // pointer read.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (b BLOB);
             INSERT INTO t (b) VALUES (zeroblob(4194304));",
        )
        .unwrap();

        let mut stmt = conn.prepare("SELECT b FROM t").unwrap();
        let col_count = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        let row = rows.next().unwrap().unwrap();

        assert_eq!(
            read_row(row, col_count),
            vec![Some("[BLOB 4194304 bytes]".to_string())]
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
