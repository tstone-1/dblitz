//! The SQL tab's DuckDB instance, locked so a query can read the opened file
//! and nothing else.
//!
//! Two layers, and each stops something the other lets through:
//!
//! 1. **Settings**, applied once at open and then frozen with
//!    `lock_configuration`: external access off except for the one file,
//!    extension autoload/autoinstall off. This stops every file and network
//!    access - `COPY ... TO`, `EXPORT DATABASE`, `ATTACH`, `INSTALL`,
//!    `read_csv('/etc/hosts')`, `glob(...)`, another Parquet file, an https URL.
//! 2. **A statement classifier**: exactly one statement, and it must be a
//!    SELECT. Settings alone let `CREATE OR REPLACE VIEW data AS SELECT 42`
//!    through, which would silently replace the table the user is querying.
//!
//! **The order is load-bearing.** Classifying means preparing, and preparing
//! does I/O: measured on DuckDB 1.5.5, preparing `EXPORT DATABASE '<dir>'`
//! created the directory before the statement type was ever inspected, and
//! preparing a query over an https URL issued the GET. So the classifier only
//! ever runs against an instance whose settings are already locked -
//! `LockedDb::open` is the only constructor, and it locks before returning.
//! `classifying_on_the_locked_instance_writes_nothing` pins this.
//!
//! duckdb-rs keeps the prepared-statement type private, so the database is
//! opened through the C API: one raw connection classifies, and the same
//! database is handed to the wrapper with `Connection::open_from_raw`, which
//! does NOT take ownership of it - hence the hand-written `Drop`.

use duckdb::{ffi, Connection};
use std::ffi::{CStr, CString};
use std::sync::Arc;

use super::sql_literal;

pub(crate) struct LockedDb {
    /// `Option` only so `Drop` can release it before the database it borrows.
    conn: Option<Connection>,
    classifier: ffi::duckdb_connection,
    db: ffi::duckdb_database,
}

// SAFETY: the raw handles are used only through `&mut self` or while the
// owning `Mutex` in `ParquetSession` is held, never from two threads at once.
// DuckDB database and connection handles may move between threads.
unsafe impl Send for LockedDb {}

/// Why a statement was refused before it ran.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The statement did not parse or bind; DuckDB's own message.
    Invalid(String),
    /// More than one statement (or none).
    NotOne(u64),
    /// One statement, but not a SELECT.
    NotSelect,
}

impl LockedDb {
    /// Opens an in-memory database with a `data` view over `path` and locks
    /// it. `temp_dir` is the one directory it may spill to.
    pub(crate) fn open(path: &str, temp_dir: &str) -> Result<Self, String> {
        Self::open_inner(path, temp_dir, true)
    }

    /// The same instance without the settings lock - only for the tests that
    /// prove the lock is what stops a write, not DuckDB's defaults.
    #[cfg(test)]
    fn open_unlocked(path: &str, temp_dir: &str) -> Result<Self, String> {
        Self::open_inner(path, temp_dir, false)
    }

    fn open_inner(path: &str, temp_dir: &str, lock: bool) -> Result<Self, String> {
        let mut db: ffi::duckdb_database = std::ptr::null_mut();
        let mut classifier: ffi::duckdb_connection = std::ptr::null_mut();
        let memory = CString::new(":memory:").expect("no NUL");
        // SAFETY: plain C API calls with out-pointers we own; each failure path
        // releases what was already created.
        unsafe {
            if ffi::duckdb_open(memory.as_ptr(), &mut db) != ffi::duckdb_state_DuckDBSuccess {
                return Err("could not start the Parquet query engine".to_string());
            }
            if ffi::duckdb_connect(db, &mut classifier) != ffi::duckdb_state_DuckDBSuccess {
                ffi::duckdb_close(&mut db);
                return Err("could not connect to the Parquet query engine".to_string());
            }
        }
        // SAFETY: `db` is a valid, open database. `open_from_raw` borrows it
        // (not owned), so our `Drop` closes it after the wrapper is gone.
        let conn = match unsafe { Connection::open_from_raw(db) } {
            Ok(c) => c,
            Err(e) => {
                unsafe {
                    ffi::duckdb_disconnect(&mut classifier);
                    ffi::duckdb_close(&mut db);
                }
                return Err(e.to_string());
            }
        };
        let locked = LockedDb {
            conn: Some(conn),
            classifier,
            db,
        };
        let file = sql_literal(path);
        let tmp = sql_literal(temp_dir);
        locked
            .conn()
            .execute_batch(&format!(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET allow_community_extensions = false;
                 SET memory_limit = '{mem}';
                 SET temp_directory = {tmp};
                 CREATE VIEW data AS SELECT * FROM read_parquet({file});",
                mem = super::MEMORY_LIMIT,
            ))
            .map_err(|e| e.to_string())?;
        if lock {
            locked
                .conn()
                .execute_batch(&format!(
                    "SET allowed_paths = [{file}];
                     SET allowed_directories = [{tmp}];
                     SET enable_external_access = false;
                     SET lock_configuration = true;"
                ))
                .map_err(|e| e.to_string())?;
        }
        Ok(locked)
    }

    pub(crate) fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("connection lives until drop")
    }

    pub(crate) fn interrupt_handle(&self) -> Arc<duckdb::InterruptHandle> {
        self.conn().interrupt_handle()
    }

    /// Ok only for exactly one statement of type SELECT (which includes
    /// `FROM`-first queries, `DESCRIBE` and `SUMMARIZE`). Fails closed: a
    /// statement that does not prepare is refused, not run.
    pub(crate) fn classify(&self, sql: &str) -> Result<(), Refusal> {
        let c = CString::new(sql)
            .map_err(|_| Refusal::Invalid("the query contains a NUL byte".into()))?;
        // SAFETY: `classifier` is a live connection on a live database; every
        // extracted/prepared handle is destroyed before returning.
        unsafe {
            let mut extracted: ffi::duckdb_extracted_statements = std::ptr::null_mut();
            let n = ffi::duckdb_extract_statements(self.classifier, c.as_ptr(), &mut extracted);
            let verdict = if n == 0 {
                let e = ffi::duckdb_extract_statements_error(extracted);
                if e.is_null() {
                    Err(Refusal::NotOne(0))
                } else {
                    Err(Refusal::Invalid(
                        CStr::from_ptr(e).to_string_lossy().into_owned(),
                    ))
                }
            } else if n != 1 {
                Err(Refusal::NotOne(n))
            } else {
                let mut stmt: ffi::duckdb_prepared_statement = std::ptr::null_mut();
                let ok = ffi::duckdb_prepare_extracted_statement(
                    self.classifier,
                    extracted,
                    0,
                    &mut stmt,
                );
                let v = if ok != ffi::duckdb_state_DuckDBSuccess {
                    let e = ffi::duckdb_prepare_error(stmt);
                    Err(Refusal::Invalid(if e.is_null() {
                        "the query could not be prepared".to_string()
                    } else {
                        CStr::from_ptr(e).to_string_lossy().into_owned()
                    }))
                } else if ffi::duckdb_prepared_statement_type(stmt)
                    == ffi::duckdb_statement_type_DUCKDB_STATEMENT_TYPE_SELECT
                {
                    Ok(())
                } else {
                    Err(Refusal::NotSelect)
                };
                ffi::duckdb_destroy_prepare(&mut stmt);
                v
            };
            ffi::duckdb_destroy_extracted(&mut extracted);
            verdict
        }
    }
}

impl Drop for LockedDb {
    fn drop(&mut self) {
        // The wrapper connection first: it borrows `db` and must not outlive it.
        self.conn.take();
        // SAFETY: both handles were created in `open` and are released once.
        unsafe {
            ffi::duckdb_disconnect(&mut self.classifier);
            ffi::duckdb_close(&mut self.db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, scratch_dir};
    use super::*;
    use std::path::Path;

    /// Runs `sql` the way `execute_sql` does: classifier first, then execution.
    fn run(db: &LockedDb, sql: &str) -> Result<usize, String> {
        db.classify(sql).map_err(|r| format!("{r:?}"))?;
        let mut stmt = db.conn().prepare(sql).map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut n = 0;
        while rows.next().map_err(|e| e.to_string())?.is_some() {
            n += 1;
        }
        Ok(n)
    }

    fn locked() -> (LockedDb, std::path::PathBuf) {
        let dir = scratch_dir("lockdown");
        let file = fixture(&dir, 100);
        let tmp = dir.join("spill");
        let db = LockedDb::open(file.to_str().unwrap(), tmp.to_str().unwrap()).unwrap();
        (db, dir)
    }

    #[test]
    fn reading_the_opened_file_is_allowed() {
        let (db, _dir) = locked();
        assert_eq!(run(&db, "SELECT count(*) FROM data"), Ok(1));
        assert_eq!(run(&db, "FROM data LIMIT 3"), Ok(3));
        assert_eq!(run(&db, "SELECT * FROM data ORDER BY val LIMIT 3"), Ok(3));
        assert_eq!(
            run(
                &db,
                "SELECT count(*) FROM data WHERE regexp_matches(name, '1')"
            ),
            Ok(1)
        );
        assert!(
            run(&db, "DESCRIBE data").is_ok(),
            "DESCRIBE classifies as SELECT"
        );
        assert!(
            run(&db, "SUMMARIZE data").is_ok(),
            "SUMMARIZE classifies as SELECT"
        );
    }

    /// Every statement here writes a file or directory when run on an
    /// unlocked instance - `an_unlocked_instance_really_writes` proves the
    /// artifacts are reachable, so their absence below means something.
    fn write_attacks(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
        let p = |n: &str| dir.join(n);
        vec![
            (
                format!(
                    "COPY (SELECT 1) TO {}",
                    sql_literal(p("a.csv").to_str().unwrap())
                ),
                p("a.csv"),
            ),
            (
                format!(
                    "EXPORT DATABASE {}",
                    sql_literal(p("exp").to_str().unwrap())
                ),
                p("exp"),
            ),
            (
                format!(
                    "ATTACH {} AS x",
                    sql_literal(p("x.duckdb").to_str().unwrap())
                ),
                p("x.duckdb"),
            ),
            (
                format!(
                    "EXPLAIN ANALYZE COPY (SELECT 1) TO {}",
                    sql_literal(p("b.csv").to_str().unwrap())
                ),
                p("b.csv"),
            ),
            (
                format!(
                    "SELECT 1; COPY (SELECT 1) TO {}",
                    sql_literal(p("c.csv").to_str().unwrap())
                ),
                p("c.csv"),
            ),
        ]
    }

    #[test]
    fn an_unlocked_instance_really_writes() {
        // The control for `no_statement_can_write_a_file`: the same statements
        // on a plain in-memory DuckDB DO create their artifacts. Without it, a
        // typo'd path would make the locked test pass for the wrong reason.
        let dir = scratch_dir("lockdown-control");
        let conn = Connection::open_in_memory().unwrap();
        for (sql, artifact) in write_attacks(&dir) {
            conn.execute_batch(&sql).unwrap();
            assert!(
                artifact.exists(),
                "control: {sql} should write {artifact:?}"
            );
        }
    }

    #[test]
    fn no_statement_can_write_a_file() {
        let (db, dir) = locked();
        for (sql, artifact) in write_attacks(&dir) {
            assert!(run(&db, &sql).is_err(), "{sql} must be refused");
            assert!(!artifact.exists(), "{sql} wrote {artifact:?}");
        }
    }

    #[test]
    fn the_settings_layer_alone_stops_writes() {
        // Same attacks, bypassing the classifier: the settings must hold on
        // their own, so a classifier bug is not a write.
        let (db, dir) = locked();
        for (sql, artifact) in write_attacks(&dir) {
            let _ = db.conn().execute_batch(&sql);
            assert!(!artifact.exists(), "settings let {sql} write {artifact:?}");
        }
    }

    #[test]
    fn classifying_on_an_unlocked_instance_does_write() {
        // The control for the test below. If DuckDB ever stops doing I/O at
        // prepare time this goes red, and the ordering rule can be revisited.
        let dir = scratch_dir("lockdown-prepare-control");
        let file = fixture(&dir, 10);
        let db =
            LockedDb::open_unlocked(file.to_str().unwrap(), dir.join("spill").to_str().unwrap())
                .unwrap();
        let target = dir.join("prepared-export");
        let sql = format!("EXPORT DATABASE {}", sql_literal(target.to_str().unwrap()));
        assert_eq!(db.classify(&sql), Err(Refusal::NotSelect));
        assert!(
            target.exists(),
            "control: preparing EXPORT on an unlocked instance should create {target:?}"
        );
    }

    #[test]
    fn classifying_on_the_locked_instance_writes_nothing() {
        // Preparing EXPORT DATABASE creates its directory (measured on an
        // unlocked instance). This is the reason the classifier may only ever
        // see a locked instance.
        let (db, dir) = locked();
        let target = dir.join("prepared-export");
        let sql = format!("EXPORT DATABASE {}", sql_literal(target.to_str().unwrap()));
        // Either refusal is fine (the locked prepare may already fail with a
        // permission error); what matters is that nothing reached the disk.
        assert!(db.classify(&sql).is_err());
        assert!(!target.exists(), "classifying EXPORT created {target:?}");
    }

    #[test]
    fn nothing_outside_the_file_can_be_read() {
        let (db, dir) = locked();
        let other = fixture(&dir.join("other"), 10);
        for sql in [
            "SELECT * FROM read_csv('/etc/hosts')".to_string(),
            "SELECT * FROM read_text('/etc/hosts')".to_string(),
            "SELECT * FROM glob('/*')".to_string(),
            format!(
                "SELECT count(*) FROM read_parquet({})",
                sql_literal(other.to_str().unwrap())
            ),
            "SELECT * FROM read_parquet('https://example.com/x.parquet')".to_string(),
        ] {
            assert!(run(&db, &sql).is_err(), "{sql} must be refused");
        }
    }

    #[test]
    fn extensions_and_settings_cannot_be_changed() {
        let (db, dir) = locked();
        for sql in [
            "INSTALL httpfs",
            "LOAD httpfs",
            "SET enable_external_access = true",
            "RESET lock_configuration",
        ] {
            assert!(run(&db, sql).is_err(), "{sql} must be refused");
            // And not through the settings layer's back door either.
            assert!(
                db.conn().execute_batch(sql).is_err(),
                "settings let {sql} through"
            );
        }
        assert!(!dir.join("ext").exists());
    }

    #[test]
    fn the_catalog_cannot_be_changed() {
        let (db, _dir) = locked();
        for sql in [
            "CREATE OR REPLACE VIEW data AS SELECT 42 AS id",
            "CREATE TABLE t AS SELECT 1",
            "CREATE MACRO m(x) AS x",
            "CALL pragma_database_size()",
        ] {
            assert_eq!(
                db.classify(sql).map_err(|_| ()),
                Err(()),
                "{sql} must be refused"
            );
        }
        // The view the grid and the user query is still the file.
        assert_eq!(run(&db, "SELECT * FROM data"), Ok(100));
    }

    #[test]
    fn more_than_one_statement_is_refused() {
        let (db, _dir) = locked();
        assert_eq!(db.classify("SELECT 1; SELECT 2"), Err(Refusal::NotOne(2)));
    }
}
