//! Parquet files, read through an embedded DuckDB.
//!
//! A Parquet file is presented as ONE table named `data`, both in the sidebar
//! and to the SQL tab, so every IPC command keeps the shape the SQLite path
//! already returns and the frontend needs no second code path.
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
mod sql;

use duckdb::Connection;
use parking_lot::Mutex;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// `true` when the file starts AND ends with the Parquet magic `PAR1`.
/// Detection is by content, not extension: anything else goes to the SQLite
/// path unchanged, so opening a non-SQLite file errors exactly as before.
pub fn is_parquet(path: &str) -> bool {
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
    browse: Mutex<Connection>,
    browse_interrupt: Arc<duckdb::InterruptHandle>,
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
        let file = sql_literal(path);
        let spill_dir = session_path("spill", "");
        // DuckDB creates its temp directory on the first spill, but not that
        // directory's parents - on a machine that never ran dblitz, every
        // sort too big for memory failed with "Failed to create directory".
        if let Some(parent) = spill_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let spill = sql_literal(&spill_dir.to_string_lossy());

        let browse = Connection::open_in_memory().map_err(|e| e.to_string())?;
        browse
            .execute_batch(&format!(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET memory_limit = '{MEMORY_LIMIT}';
                 SET temp_directory = {spill};
                 CREATE VIEW data AS SELECT * FROM read_parquet({file});"
            ))
            .map_err(|e| e.to_string())?;

        let columns = describe(&browse)?;
        let row_numbers = !columns.iter().any(|c| c.name == "file_row_number");
        if row_numbers {
            browse
                .execute_batch(&format!(
                    "CREATE VIEW data_rn AS SELECT * FROM read_parquet({file}, file_row_number = true);"
                ))
                .map_err(|e| e.to_string())?;
        }
        // Answered from the footer, not by scanning: 18 ms cold on 50M rows.
        let total_rows: i64 = browse
            .query_row("SELECT count(*) FROM data", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let schema_sql = describe_file(&browse, path, &file, total_rows)?;

        let sql = lockdown::LockedDb::open(path, &spill_dir.to_string_lossy())?;
        let browse_interrupt = browse.interrupt_handle();
        let sql_interrupt = sql.interrupt_handle();
        Ok(ParquetSession {
            path: path.to_string(),
            columns,
            total_rows,
            schema_sql,
            row_numbers,
            browse: Mutex::new(browse),
            browse_interrupt,
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
        self.sql_interrupt.interrupt();
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for ParquetSession {
    fn drop(&mut self) {
        // Release the sort cache before deleting it: DETACH closes the file.
        query::drop_sort_cache(self.browse.get_mut(), self.views.get_mut());
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
fn describe_file(conn: &Connection, path: &str, file: &str, rows: i64) -> Result<String, String> {
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
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let mut s = format!("-- Parquet file: {name}\n-- Rows: {rows}\n-- Row groups: {row_groups}\n");
    if let Some(c) = compression.filter(|c| !c.is_empty()) {
        s.push_str(&format!("-- Compression: {c}\n"));
    }
    if let Some(c) = created_by.filter(|c| !c.is_empty()) {
        s.push_str(&format!("-- Created by: {}\n", c.replace('\n', " ")));
    }
    s.push_str(&format!(
        "CREATE VIEW data AS SELECT * FROM read_parquet({})",
        sql_literal(&name)
    ));
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
