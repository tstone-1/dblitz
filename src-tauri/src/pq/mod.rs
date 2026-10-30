//! Parquet, CSV and JSON files, read through an embedded DuckDB. The names
//! (`ParquetSession`, `db::open_parquet`) predate CSV and JSON; one session
//! type serves all three, and [`FileFormat`] is the only difference.
//!
//! A file is presented as ONE table named `data`, both in the sidebar and to
//! the SQL tab, so every IPC command keeps the shape the SQLite path already
//! returns and the frontend needs no second code path.
//!
//! **CSV and JSON are loaded into the browse instance once, as a table.**
//! Paging and the sort cache need a stable row number (`file_row_number`),
//! which only `read_parquet` provides. A table's `rowid` is that number for a
//! table filled in file order, so `data_rn` exposes it under the same name and
//! [`query`] needs no format of its own. Reading the text file per page
//! instead would re-parse all of it for every page and every sort. The SQL
//! tab's instance reads the file per query, as it does for Parquet.
//!
//! Two DuckDB instances, mirroring the SQLite `conn`/`aux_conn` split:
//!
//! - `browse` serves Browse Data. It runs only SQL that dblitz generates (user
//!   values are bound or escaped as literals), so it stays unlocked: it has to
//!   attach the on-disk sort cache and be able to spill.
//! - `sql` serves the SQL tab and is locked down; see [`lockdown`].
//!
//! **Every cell is rendered by DuckDB, with `CAST(col AS VARCHAR)`**, in the
//! page query and in the filter SQL alike ([`render_expr`], [`text_expr`]). The
//! text the grid shows is therefore exactly the text a filter matches against -
//! the same guarantee `render_real` gives the SQLite path - and LIST, STRUCT,
//! DECIMAL and TIMESTAMP need no formatter of our own. A BLOB renders as
//! `[BLOB n bytes]`, like `util::render_cell`, and never matches a text filter.

mod filters;
mod lockdown;
mod query;
mod raw;
mod sql;

use duckdb::Connection;
use parking_lot::Mutex;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::db::{ColumnInfo, SchemaEntry, TableInfo};

pub use query::query_table;

/// The name the file is exposed under.
pub const TABLE: &str = "data";

/// Per DuckDB instance. DuckDB's default is 80% of RAM, which is not a
/// viewer's to take. The limit covers DuckDB's buffer pool only, so the process
/// peaks above it. Sorting a 50M-row, 14-column file: 3 GB cap, 13.0 s and
/// 3.8 GB of RSS; 2 GB cap, 17.7 s and 2.65 GB. Memory is the scarcer resource
/// on a desktop, and the sort runs once per view. A 1 GB cap starved the
/// Parquet scan itself on a 10-thread machine.
const MEMORY_LIMIT: &str = "2GB";

/// Sort-cache and spill files older than this are swept at startup. A crash
/// leaves them behind; a running session keeps writing or reading its own, and
/// on Windows an open file cannot be deleted anyway.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// The file formats DuckDB reads for dblitz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Parquet,
    Csv,
    Json,
}

impl FileFormat {
    /// Which DuckDB format `path` is, or `None` for the SQLite path.
    ///
    /// Parquet is detected by content, so a renamed Parquet file still opens.
    /// CSV and JSON have no magic bytes, so they go by extension: `.csv` and
    /// `.tsv` (DuckDB's sniffer finds the delimiter either way), `.json`,
    /// `.jsonl` and `.ndjson` (`read_json` tells an array from one object per
    /// line). Anything else goes to SQLite unchanged, so opening a file that is
    /// neither errors exactly as before.
    pub fn of(path: &str) -> Option<FileFormat> {
        if is_parquet(path) {
            return Some(FileFormat::Parquet);
        }
        let ext = Path::new(path)
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase();
        match ext.as_str() {
            "csv" | "tsv" => Some(FileFormat::Csv),
            "json" | "jsonl" | "ndjson" => Some(FileFormat::Json),
            _ => None,
        }
    }

    /// The table function that reads `file`, an already-quoted SQL literal.
    pub(crate) fn reader(self, file: &str) -> String {
        match self {
            FileFormat::Parquet => format!("read_parquet({file})"),
            FileFormat::Csv => format!("read_csv({file})"),
            FileFormat::Json => format!("read_json({file})"),
        }
    }

    /// The same reader, deciding from every row instead of the first 20,480.
    /// Not for the SQL tab's view: a view binds again on every prepare, and the
    /// live check prepares after every keystroke ([`FullScan`]).
    fn full_scan_reader(self, file: &str) -> String {
        match self {
            FileFormat::Parquet => self.reader(file),
            FileFormat::Csv => format!("read_csv({file}, sample_size = -1)"),
            FileFormat::Json => format!("read_json({file}, sample_size = -1)"),
        }
    }
}

/// What a CSV or JSON file needed when the first 20,480 rows did not describe
/// it, as when a column of numbers holds a `-` further down.
struct FullScan {
    /// A reader with the scan's result written out, so it detects nothing when
    /// it binds: 1 ms against 2 s on a 233 MB CSV.
    pinned_reader: String,
    /// Delimiter and header row, for CSV; the scan already found them.
    dialect: Option<(String, bool)>,
}

/// Loads a CSV or JSON file into `file_rows`. The sample decides first because
/// a full scan costs 13 times the load (2.2 s against 0.16 s on that file).
/// `None` means the sample was enough. When the full scan fails too, the file
/// is what is wrong, and the first error names the row.
fn load_text_file(
    conn: &Connection,
    format: FileFormat,
    file: &str,
) -> Result<Option<FullScan>, String> {
    match load_rows(conn, &format.reader(file)) {
        Ok(()) => Ok(None),
        Err(sampled) => load_full_scan(conn, format, file)
            .map(Some)
            .map_err(|_| sampled),
    }
}

fn load_rows(conn: &Connection, reader: &str) -> Result<(), String> {
    conn.execute_batch(&format!(
        "CREATE TABLE file_rows AS SELECT * FROM {reader};"
    ))
    .map_err(|e| e.to_string())
}

fn load_full_scan(conn: &Connection, format: FileFormat, file: &str) -> Result<FullScan, String> {
    if format == FileFormat::Csv {
        let (pinned_reader, dialect) = pinned_csv_reader(conn, file)?;
        load_rows(conn, &pinned_reader)?;
        return Ok(FullScan {
            pinned_reader,
            dialect: Some(dialect),
        });
    }
    load_rows(conn, &format.full_scan_reader(file))?;
    let columns: String = conn
        .query_row(
            "SELECT string_agg('''' || replace(name, '''', '''''') || ''': ''' || type || '''', ', ' ORDER BY cid)
             FROM pragma_table_info('file_rows')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(FullScan {
        pinned_reader: format!("read_json({file}, columns = {{{columns}}})"),
        dialect: None,
    })
}

/// Sniffs every row and writes the result out as a `read_csv` call. Built from
/// the fields because the sniffer's own `Prompt` column does not escape: a path
/// or a column name with an apostrophe in it does not parse.
fn pinned_csv_reader(conn: &Connection, file: &str) -> Result<(String, (String, bool)), String> {
    // The sniffer reports an unset option as this text, where `read_csv` wants ''.
    let option = |v: String| sql_literal(if v == "(empty)" { "" } else { &v });
    conn.query_row(
        &format!(
            "SELECT Delimiter, Quote, Escape, NewLineDelimiter, Comment, SkipRows, HasHeader,
                    array_to_string(list_transform(Columns, lambda c:
                        '''' || replace(c.name, '''', '''''') || ''': ''' || c.type || ''''), ', '),
                    DateFormat, TimestampFormat
             FROM sniff_csv({file}, sample_size = -1)"
        ),
        [],
        |r| {
            let delimiter: String = r.get(0)?;
            let header: bool = r.get(6)?;
            let mut reader = format!(
                "read_csv({file}, auto_detect = false, delim = {}, quote = {}, escape = {}, new_line = {}, comment = {}, skip = {}, header = {header}, columns = {{{}}}",
                option(delimiter.clone()),
                option(r.get(1)?),
                option(r.get(2)?),
                option(r.get(3)?),
                option(r.get(4)?),
                r.get::<_, i64>(5)?,
                r.get::<_, String>(7)?,
            );
            for (name, value) in [
                ("dateformat", r.get::<_, Option<String>>(8)?),
                ("timestampformat", r.get::<_, Option<String>>(9)?),
            ] {
                if let Some(v) = value.filter(|v| v != "(empty)") {
                    reader.push_str(&format!(", {name} = {}", sql_literal(&v)));
                }
            }
            reader.push(')');
            Ok((reader, (delimiter, header)))
        },
    )
    .map_err(|e| e.to_string())
}

/// `true` when the file starts AND ends with the Parquet magic `PAR1`.
fn is_parquet(path: &str) -> bool {
    fn check(path: &str) -> std::io::Result<bool> {
        let mut f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        if len < 12 {
            return Ok(false);
        }
        let mut head = [0u8; 4];
        let mut tail = [0u8; 4];
        f.read_exact(&mut head)?;
        f.seek(SeekFrom::End(-4))?;
        f.read_exact(&mut tail)?;
        Ok(&head == b"PAR1" && &tail == b"PAR1")
    }
    check(path).unwrap_or(false)
}

/// A single-quoted SQL string literal.
pub(crate) fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn is_blob(col_type: &str) -> bool {
    let t = col_type.trim().to_ascii_uppercase();
    t == "BLOB" || t == "BYTEA"
}

/// What the grid shows for a column.
pub(crate) fn render_expr(col: &ColumnInfo) -> String {
    let q = quote_ident(&col.name);
    if is_blob(&col.col_type) {
        format!(
            "CASE WHEN {q} IS NULL THEN NULL ELSE '[BLOB ' || octet_length({q}) || ' bytes]' END"
        )
    } else {
        format!("CAST({q} AS VARCHAR)")
    }
}

/// What a text filter matches against: the rendered text, except that a BLOB
/// is NULL - it never matches, as on the SQLite path.
pub(crate) fn text_expr(col: &ColumnInfo) -> String {
    if is_blob(&col.col_type) {
        "CAST(NULL AS VARCHAR)".to_string()
    } else {
        format!("CAST({} AS VARCHAR)", quote_ident(&col.name))
    }
}

fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("dblitz")
}

/// A fresh path for this process's next sort cache or spill directory.
fn session_path(kind: &str, suffix: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    cache_root()
        .join(kind)
        .join(format!("{}-{n}{suffix}", std::process::id()))
}

/// Removes sort-cache files and spill directories a crashed session left
/// behind. Best effort: anything in use or unreadable is skipped.
pub fn sweep_stale_cache() {
    for kind in ["sort-cache", "spill"] {
        let Ok(entries) = std::fs::read_dir(cache_root().join(kind)) else {
            continue;
        };
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_AFTER);
            if old {
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    }
}

pub struct ParquetSession {
    path: String,
    columns: Vec<ColumnInfo>,
    total_rows: i64,
    schema_sql: String,
    /// `false` when the file itself has a column named `file_row_number`,
    /// which would collide with DuckDB's virtual one; every view then falls
    /// back to `LIMIT/OFFSET` ([`query`]).
    row_numbers: bool,
    /// [`query::ORDER_LIST_MAX_CELLS`]; a field so that a test can force
    /// either sorted path on a small fixture.
    order_list_max_cells: i64,
    /// Set when a sort cache could not be built; sorted views are then paged
    /// from an order list for the rest of the session.
    sort_cache_failed: AtomicBool,
    browse: Mutex<raw::RawDb>,
    browse_interrupt: Arc<duckdb::InterruptHandle>,
    /// The browse instance's side connection, which builds sort caches; read
    /// for progress and interrupted from other threads while it runs.
    builder: raw::SideHandle,
    /// True while a sort cache is being built, so `view_progress` reports
    /// only that and never a stale figure from an earlier statement.
    building: AtomicBool,
    sql: Mutex<lockdown::LockedDb>,
    sql_interrupt: Arc<duckdb::InterruptHandle>,
    views: Mutex<query::Views>,
    spill_dir: PathBuf,
}

impl ParquetSession {
    /// Opens both instances and reads the file's shape. Nothing outside the
    /// returned session is touched, so a failure leaves whatever the caller
    /// had open intact.
    pub fn open(path: &str) -> Result<Self, String> {
        let format = FileFormat::of(path)
            .ok_or_else(|| format!("{path} is not a Parquet, CSV or JSON file"))?;
        let file = sql_literal(path);
        let reader = format.reader(&file);
        let spill_dir = session_path("spill", "");
        // DuckDB creates its temp directory on the first spill, but not that
        // directory's parents - on a machine that never ran dblitz, every
        // sort too big for memory failed with "Failed to create directory".
        if let Some(parent) = spill_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let spill = sql_literal(&spill_dir.to_string_lossy());

        let raw = raw::RawDb::open_in_memory()?;
        // The side connection builds sort caches. Progress is only tracked on
        // a connection with the progress bar enabled, and from the start of a
        // statement only with `progress_bar_time = 0`; printing stays off.
        for setting in [
            "SET enable_progress_bar = true",
            "SET enable_progress_bar_print = false",
            "SET progress_bar_time = 0",
        ] {
            raw.execute_side(setting, &[])?;
        }
        let browse = raw.conn();
        browse
            .execute_batch(&format!(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET memory_limit = '{MEMORY_LIMIT}';
                 SET temp_directory = {spill};"
            ))
            .map_err(|e| e.to_string())?;
        let mut full_scan = None;
        let source = match format {
            FileFormat::Parquet => reader.clone(),
            // `preserve_insertion_order` is DuckDB's default, so the table is
            // filled in file order and `rowid` counts lines (or array items).
            FileFormat::Csv | FileFormat::Json => {
                full_scan = load_text_file(browse, format, &file)?;
                "file_rows".to_string()
            }
        };
        browse
            .execute_batch(&format!("CREATE VIEW data AS SELECT * FROM {source};"))
            .map_err(|e| e.to_string())?;

        let columns = describe(browse)?;
        // A column of the file's own named `file_row_number` would collide
        // with the one `data_rn` adds, and one named `rowid` hides a table's
        // `rowid`; either sends every view to `LIMIT/OFFSET` ([`query`]).
        let row_numbers = !columns.iter().any(|c| {
            c.name.eq_ignore_ascii_case("file_row_number")
                || (format != FileFormat::Parquet && c.name.eq_ignore_ascii_case("rowid"))
        });
        if row_numbers {
            let rn = match format {
                FileFormat::Parquet => {
                    format!("SELECT * FROM read_parquet({file}, file_row_number = true)")
                }
                FileFormat::Csv | FileFormat::Json => {
                    "SELECT *, rowid AS file_row_number FROM file_rows".to_string()
                }
            };
            browse
                .execute_batch(&format!("CREATE VIEW data_rn AS {rn};"))
                .map_err(|e| e.to_string())?;
        }
        // Parquet answers from the footer, not by scanning: 18 ms cold on 50M
        // rows. CSV and JSON count the table loaded above.
        let total_rows: i64 = browse
            .query_row("SELECT count(*) FROM data", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let schema_sql =
            describe_file(browse, path, format, &file, total_rows, full_scan.as_ref())?;

        let sql_reader = full_scan.as_ref().map_or(&reader, |f| &f.pinned_reader);
        let sql = lockdown::LockedDb::open(path, sql_reader, &spill_dir.to_string_lossy())?;
        let browse_interrupt = browse.interrupt_handle();
        let builder = raw.side_handle();
        let sql_interrupt = sql.interrupt_handle();
        Ok(ParquetSession {
            path: path.to_string(),
            columns,
            total_rows,
            schema_sql,
            row_numbers,
            order_list_max_cells: query::ORDER_LIST_MAX_CELLS,
            sort_cache_failed: AtomicBool::new(false),
            browse: Mutex::new(raw),
            browse_interrupt,
            builder,
            building: AtomicBool::new(false),
            sql: Mutex::new(sql),
            sql_interrupt,
            views: Mutex::new(query::Views::new(session_path("sort-cache", ".duckdb"))),
            spill_dir,
        })
    }

    pub fn tables(&self) -> Vec<TableInfo> {
        vec![TableInfo {
            name: TABLE.to_string(),
            row_count: self.total_rows,
        }]
    }

    pub fn get_columns(&self, table: &str) -> Result<Vec<ColumnInfo>, String> {
        check_table(table)?;
        Ok(self.columns.clone())
    }

    pub fn get_schema(&self) -> Vec<SchemaEntry> {
        vec![SchemaEntry {
            obj_type: "view".to_string(),
            name: TABLE.to_string(),
            tbl_name: TABLE.to_string(),
            sql: Some(self.schema_sql.clone()),
        }]
    }

    pub fn execute_sql(&self, sql: &str, generation: &AtomicU64) -> crate::db::SqlResult {
        sql::execute_sql(&self.sql.lock(), sql, generation)
    }

    /// `None` while the SQL tab's instance is busy running a query; see
    /// `db::validate_sql` for why this never waits for the lock.
    pub fn validate_sql(&self, sql: &str) -> Option<crate::db::SqlCheck> {
        let db = self.sql.try_lock()?;
        Some(sql::validate_sql(&db, sql))
    }

    pub fn count_rows(
        &self,
        table: &str,
        filters: &[crate::db::ColumnFilter],
        global_filter: &str,
        generation: &AtomicU64,
    ) -> Result<i64, String> {
        query::count_rows(self, table, filters, global_filter, generation)
    }

    /// Breaks whatever either instance is running. Independent of the
    /// connection locks, so it never waits behind the query it is stopping.
    pub fn interrupt(&self) {
        self.browse_interrupt.interrupt();
        self.builder.interrupt();
        self.sql_interrupt.interrupt();
    }

    /// Percent done of the sort cache being built, if one is being built and
    /// DuckDB has an estimate. The scan of the file is the first half, the
    /// sort and the write the second; measured on 50M rows it rose steadily
    /// from 0 to 100 over the 9 s build.
    pub fn view_progress(&self) -> Option<f64> {
        if self.building.load(Ordering::Acquire) {
            self.builder.progress()
        } else {
            None
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for ParquetSession {
    fn drop(&mut self) {
        // Release the sort cache before deleting it: DETACH closes the file.
        query::drop_sort_cache(self.browse.get_mut().conn(), self.views.get_mut());
        let _ = std::fs::remove_dir_all(&self.spill_dir);
    }
}

fn check_table(table: &str) -> Result<(), String> {
    if table == TABLE {
        Ok(())
    } else {
        Err(format!("no such table: {table}"))
    }
}

fn describe(conn: &Connection) -> Result<Vec<ColumnInfo>, String> {
    let mut stmt = conn
        .prepare("SELECT column_name, column_type FROM (DESCRIBE data)")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (cid, row) in rows.enumerate() {
        let (name, col_type) = row.map_err(|e| e.to_string())?;
        out.push(ColumnInfo {
            cid: cid as i64,
            name,
            col_type,
            notnull: false,
            default_value: None,
            pk: false,
        });
    }
    Ok(out)
}

/// The Structure tab's text for the file: a comment header with what the
/// footer says, then the view the SQL tab queries. The frontend appends `;`.
fn describe_file(
    conn: &Connection,
    path: &str,
    format: FileFormat,
    file: &str,
    rows: i64,
    full_scan: Option<&FullScan>,
) -> Result<String, String> {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let shown_file = sql_literal(&name);
    let view = format!(
        "CREATE VIEW data AS SELECT * FROM {}",
        if full_scan.is_some() {
            format.full_scan_reader(&shown_file)
        } else {
            format.reader(&shown_file)
        }
    );
    match format {
        FileFormat::Parquet => {}
        FileFormat::Csv => {
            // What DuckDB's sniffer decided, since every column type follows from it.
            let (delimiter, header): (String, bool) =
                match full_scan.and_then(|f| f.dialect.clone()) {
                    Some(dialect) => dialect,
                    None => conn
                        .query_row(
                            &format!("SELECT Delimiter, HasHeader FROM sniff_csv({file})"),
                            [],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .map_err(|e| e.to_string())?,
                };
            let delimiter = match delimiter.as_str() {
                "\t" => "tab".to_string(),
                d => format!("'{d}'"),
            };
            let header = if header { "yes" } else { "no" };
            return Ok(format!(
                "-- CSV file: {name}\n-- Rows: {rows}\n-- Delimiter: {delimiter}\n-- Header row: {header}\n{view}"
            ));
        }
        FileFormat::Json => {
            return Ok(format!("-- JSON file: {name}\n-- Rows: {rows}\n{view}"));
        }
    }
    let (created_by, row_groups): (Option<String>, i64) = conn
        .query_row(
            &format!("SELECT created_by, num_row_groups FROM parquet_file_metadata({file})"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let compression: Option<String> = conn
        .query_row(
            &format!(
                "SELECT string_agg(DISTINCT compression, ', ' ORDER BY compression) FROM parquet_metadata({file})"
            ),
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut s = format!("-- Parquet file: {name}\n-- Rows: {rows}\n-- Row groups: {row_groups}\n");
    if let Some(c) = compression.filter(|c| !c.is_empty()) {
        s.push_str(&format!("-- Compression: {c}\n"));
    }
    if let Some(c) = created_by.filter(|c| !c.is_empty()) {
        s.push_str(&format!("-- Created by: {}\n", c.replace('\n', " ")));
    }
    s.push_str(&view);
    Ok(s)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh directory for one test. Every test gets its own, so parallel
    /// tests cannot see each other's artifacts.
    pub(crate) fn scratch_dir(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dblitz-pq-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `rows` rows with one column of each kind the renderer distinguishes.
    /// Row `i`: id = i, name = 'n<i>', val = i / 2.0 (NULL every 7th row),
    /// cat cycles a/b/c, b is a 3-byte BLOB, tags = [i % 3, i % 5].
    pub(crate) fn fixture(dir: &Path, rows: u64) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("fixture.parquet");
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&format!(
            "COPY (SELECT i AS id, 'n' || i AS name,
                          CASE WHEN i % 7 = 0 THEN NULL ELSE i / 2.0 END AS val,
                          ['a','b','c'][(i % 3) + 1] AS cat,
                          '\\x01\\x02\\x03'::BLOB AS b,
                          [i % 3, i % 5] AS tags
                   FROM range({rows}) t(i))
             TO {} (FORMAT parquet, ROW_GROUP_SIZE 1000)",
            sql_literal(path.to_str().unwrap())
        ))
        .unwrap();
        path
    }

    #[test]
    fn detects_parquet_by_content() {
        let dir = scratch_dir("detect");
        let pq = fixture(&dir, 5);
        assert!(is_parquet(pq.to_str().unwrap()));
        let renamed = dir.join("looks.sqlite");
        std::fs::copy(&pq, &renamed).unwrap();
        assert!(
            is_parquet(renamed.to_str().unwrap()),
            "the extension is irrelevant"
        );
        let other = dir.join("fake.parquet");
        std::fs::write(&other, b"PAR1 but not at the end").unwrap();
        assert!(!is_parquet(other.to_str().unwrap()));
        assert!(!is_parquet(dir.join("missing").to_str().unwrap()));
    }

    #[test]
    fn csv_and_json_are_detected_by_extension_and_parquet_by_content() {
        let dir = scratch_dir("detect-format");
        let of = |name: &str| {
            let p = dir.join(name);
            std::fs::write(&p, "a\n1\n").unwrap();
            FileFormat::of(p.to_str().unwrap())
        };
        assert_eq!(of("x.csv"), Some(FileFormat::Csv));
        assert_eq!(of("X.TSV"), Some(FileFormat::Csv));
        assert_eq!(of("x.json"), Some(FileFormat::Json));
        assert_eq!(of("x.jsonl"), Some(FileFormat::Json));
        assert_eq!(of("x.NDJSON"), Some(FileFormat::Json));
        assert_eq!(of("x.txt"), None);
        assert_eq!(of("x.sqlite"), None);
        assert_eq!(of("csv"), None, "a bare name is not an extension");
        // Content wins over the extension for Parquet.
        let renamed = dir.join("really.csv");
        std::fs::copy(fixture(&dir, 3), &renamed).unwrap();
        assert_eq!(
            FileFormat::of(renamed.to_str().unwrap()),
            Some(FileFormat::Parquet)
        );
    }

    #[test]
    fn a_tsv_reports_what_the_sniffer_decided() {
        let dir = scratch_dir("tsv");
        let path = dir.join("people.tsv");
        std::fs::write(&path, "name\tage\nAda\t36\nBen\t41\n").unwrap();
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        assert_eq!(s.tables()[0].row_count, 2);
        let cols = s.get_columns("data").unwrap();
        assert_eq!(cols[1].name, "age");
        assert_eq!(cols[1].col_type, "BIGINT");
        let schema = s.get_schema()[0].sql.clone().unwrap();
        assert_eq!(
            schema,
            "-- CSV file: people.tsv\n-- Rows: 2\n-- Delimiter: tab\n-- Header row: yes\n\
             CREATE VIEW data AS SELECT * FROM read_csv('people.tsv')"
        );
    }

    #[test]
    fn the_sql_tab_reads_the_opened_csv_and_no_other_file() {
        let dir = scratch_dir("csv-sql");
        let path = dir.join("rows.csv");
        let other = dir.join("other.csv");
        std::fs::write(&path, "id,price\n1,9.5\n2,12.0\n3,4.25\n").unwrap();
        std::fs::write(&other, "secret\n42\n").unwrap();
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        let g = AtomicU64::new(0);
        let r = s.execute_sql("SELECT sum(price) FROM data WHERE id > 1", &g);
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.rows[0][0].as_deref(), Some("16.25"));
        let escape = format!(
            "SELECT * FROM read_csv({})",
            sql_literal(other.to_str().unwrap())
        );
        let r = s.execute_sql(&escape, &g);
        assert!(r.rows.is_empty(), "read another file: {:?}", r.rows);
        assert!(r.error.is_some());
    }

    /// Good lines past DuckDB's 20,480-row sample.
    const SAMPLED: u64 = 25_000;

    /// `header`, then `SAMPLED` copies of `line`, then `late`.
    fn with_late_row(path: &Path, header: &str, line: &str, late: &str) {
        let mut text = format!("{header}\n");
        for _ in 0..SAMPLED {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(late);
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }

    /// The SQL the locked instance's `data` view runs on every bind.
    fn sql_view_definition(s: &ParquetSession) -> String {
        let r = s.execute_sql(
            "SELECT sql FROM duckdb_views() WHERE view_name = 'data'",
            &AtomicU64::new(0),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        r.rows[0][0].clone().unwrap()
    }

    #[test]
    fn a_csv_with_text_below_the_sample_opens_with_that_column_as_text() {
        let dir = scratch_dir("csv-late-type");
        let path = dir.join("late.csv");
        with_late_row(&path, "name,node", "a,123", "b,-");
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        assert_eq!(s.tables()[0].row_count, SAMPLED as i64 + 1);
        let cols = s.get_columns("data").unwrap();
        assert_eq!(cols[1].col_type, "VARCHAR");
        let r = s.execute_sql("SELECT node FROM data WHERE name = 'b'", &AtomicU64::new(0));
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.rows[0][0].as_deref(), Some("-"));
        let view = sql_view_definition(&s);
        assert!(
            view.contains("auto_detect") && !view.contains("-1"),
            "the view must not scan the file on every bind: {view}"
        );
        assert_eq!(
            s.get_schema()[0].sql.clone().unwrap(),
            format!(
                "-- CSV file: late.csv\n-- Rows: {}\n-- Delimiter: ','\n-- Header row: yes\n\
                 CREATE VIEW data AS SELECT * FROM read_csv('late.csv', sample_size = -1)",
                SAMPLED + 1
            )
        );
    }

    #[test]
    fn the_pinned_csv_reader_survives_what_the_sniffers_prompt_does_not() {
        let dir = scratch_dir("csv-pinned").join("o'brien dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("it's.csv");
        with_late_row(
            &path,
            "it's;\"a\"\"b\";day;seen",
            "1;x;31/12/2024;31/12/2024 08:30:00",
            // Text in the number column, and the first quote in the file.
            "-;\"x;y\";01/02/2025;01/02/2025 09:45:00",
        );
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        let cols = s.get_columns("data").unwrap();
        let shape: Vec<(&str, &str)> = cols
            .iter()
            .map(|c| (c.name.as_str(), c.col_type.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                ("it's", "VARCHAR"),
                ("a\"b", "VARCHAR"),
                ("day", "DATE"),
                ("seen", "TIMESTAMP")
            ]
        );
        let r = s.execute_sql(
            "SELECT \"a\"\"b\", day::VARCHAR, seen::VARCHAR FROM data WHERE \"it's\" = '-'",
            &AtomicU64::new(0),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.rows[0][0].as_deref(), Some("x;y"));
        assert_eq!(r.rows[0][1].as_deref(), Some("2025-02-01"));
        assert_eq!(r.rows[0][2].as_deref(), Some("2025-02-01 09:45:00"));
    }

    #[test]
    fn a_json_file_changing_below_the_sample_opens_with_every_key() {
        let dir = scratch_dir("json-late");
        let path = dir.join("late.jsonl");
        with_late_row(
            &path,
            "{\"id\":0}",
            "{\"id\":1}",
            "{\"id\":\"-\",\"it's\":2}",
        );
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        assert_eq!(s.tables()[0].row_count, SAMPLED as i64 + 2);
        let cols = s.get_columns("data").unwrap();
        assert_eq!(cols[1].name, "it's");
        let r = s.execute_sql(
            "SELECT \"it's\" FROM data WHERE \"it's\" IS NOT NULL",
            &AtomicU64::new(0),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.rows[0][0].as_deref(), Some("2"));
        let view = sql_view_definition(&s);
        assert!(view.contains("columns") && !view.contains("-1"), "{view}");
        assert!(s.get_schema()[0]
            .sql
            .as_deref()
            .unwrap()
            .ends_with("read_json('late.jsonl', sample_size = -1)"));
    }

    #[test]
    fn a_csv_the_full_scan_cannot_read_reports_the_first_error() {
        let dir = scratch_dir("csv-bad");
        let path = dir.join("bad.csv");
        with_late_row(&path, "a,b", "1,2", "1,2,3,4,5");
        let err = ParquetSession::open(path.to_str().unwrap())
            .err()
            .expect("a row with too many columns must not open");
        assert!(err.contains("Expected Number of Columns"), "{err}");
    }

    #[test]
    fn a_malformed_json_file_fails_to_open_with_duckdbs_message() {
        let dir = scratch_dir("bad-json");
        let path = dir.join("broken.json");
        std::fs::write(&path, "[{\"a\": 1}, {\"a\": ").unwrap();
        let err = ParquetSession::open(path.to_str().unwrap())
            .err()
            .expect("a truncated JSON file must not open");
        assert!(err.to_lowercase().contains("json"), "{err}");
    }

    #[test]
    fn open_reports_one_table_with_its_columns_and_count() {
        let dir = scratch_dir("open");
        let s = ParquetSession::open(fixture(&dir, 2500).to_str().unwrap()).unwrap();
        let t = s.tables();
        assert_eq!(
            (t.len(), t[0].name.as_str(), t[0].row_count),
            (1, "data", 2500)
        );
        let cols = s.get_columns("data").unwrap();
        let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "val", "cat", "b", "tags"]);
        assert_eq!(cols[0].col_type, "BIGINT");
        assert_eq!(cols[4].col_type, "BLOB");
        assert!(s.get_columns("other").is_err());
        let schema = s.get_schema()[0].sql.clone().unwrap();
        assert!(schema.contains("-- Rows: 2500"), "{schema}");
        // DuckDB treats ROW_GROUP_SIZE as approximate, so only the line's
        // presence is pinned, not the count.
        assert!(schema.contains("-- Row groups: "), "{schema}");
        assert!(schema.contains("-- Compression: SNAPPY"), "{schema}");
        assert!(
            schema.ends_with("read_parquet('fixture.parquet')"),
            "{schema}"
        );
    }

    #[test]
    fn open_creates_the_parent_of_the_spill_directory() {
        let dir = scratch_dir("spill-parent");
        let s = ParquetSession::open(fixture(&dir, 5).to_str().unwrap()).unwrap();
        assert!(
            s.spill_dir.parent().unwrap().is_dir(),
            "DuckDB only creates the last path component; a big sort fails without this"
        );
    }

    #[test]
    fn a_column_named_file_row_number_disables_the_fast_path_only() {
        let dir = scratch_dir("clash");
        let path = dir.join("clash.parquet");
        Connection::open_in_memory()
            .unwrap()
            .execute_batch(&format!(
                "COPY (SELECT i AS file_row_number, i * 10 AS x FROM range(20) t(i)) TO {} (FORMAT parquet)",
                sql_literal(path.to_str().unwrap())
            ))
            .unwrap();
        let s = ParquetSession::open(path.to_str().unwrap()).unwrap();
        assert!(!s.row_numbers);
        assert_eq!(s.tables()[0].row_count, 20);
    }
}
