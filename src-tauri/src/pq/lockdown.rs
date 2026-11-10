//! The SQL tab's DuckDB instance, locked so a query can read the opened file
//! and nothing else.
//!
//! Two layers, and each stops something the other lets through:
//!
//! 1. **Settings**, applied once at open and then frozen with
//!    `lock_configuration`: external access off except for the one file,
//!    extension autoload/autoinstall off. This stops every file and network
//!    access - `COPY ... TO`, `EXPORT DATABASE`, `ATTACH`, `INSTALL`,
//!    `read_csv` of another file, `glob(...)`, another Parquet file, an https URL.
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
//! a [`RawDb`] and its side connection is the classifier.

use duckdb::{ffi, Connection};
use std::ffi::{CStr, CString};
use std::sync::Arc;

use super::raw::RawDb;
use super::{file_arg, sql_literal};

pub(crate) struct LockedDb {
    raw: RawDb,
}

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
    /// Opens an in-memory database with a `data` view over `reader`, which
    /// reads `path`, and locks it. `temp_dir` is the one directory it may spill
    /// to.
    pub(crate) fn open(path: &str, reader: &str, temp_dir: &str) -> Result<Self, String> {
        Self::open_inner(path, reader, temp_dir, true)
    }

    /// The same instance without the settings lock - only for the tests that
    /// prove the lock is what stops a write, not DuckDB's defaults.
    #[cfg(test)]
    fn open_unlocked(path: &str, temp_dir: &str) -> Result<Self, String> {
        let reader = super::FileFormat::Parquet.reader(&file_arg(path));
        Self::open_inner(path, &reader, temp_dir, false)
    }

    fn open_inner(path: &str, reader: &str, temp_dir: &str, lock: bool) -> Result<Self, String> {
        let locked = LockedDb {
            raw: RawDb::open_in_memory()?,
        };
        // Both spellings of the file: DuckDB checks a reader's glob pattern
        // against `allowed_paths` before expanding it, and then the file it
        // expanded to (`OpenerFileSystem::GlobFilesExtended`, then `OpenFile`).
        // For a name with `[`, `*` or `?` the pattern is [`file_arg`]'s
        // escaped one, which matches only this file.
        let mut allowed = vec![sql_literal(path)];
        let pattern = file_arg(path);
        if pattern != allowed[0] {
            allowed.push(pattern);
        }
        let allowed = allowed.join(", ");
        let tmp = sql_literal(temp_dir);
        locked
            .conn()
            .execute_batch(&format!(
                "SET autoinstall_known_extensions = false;
                 SET autoload_known_extensions = false;
                 SET allow_community_extensions = false;
                 SET memory_limit = '{mem}';
                 SET temp_directory = {tmp};",
                mem = super::MEMORY_LIMIT,
            ))
            .map_err(|e| e.to_string())?;
        // The reader carries text the file decided (a pinned reader's column
        // names and types), so it runs as exactly one statement: the C API's
        // prepare refuses a second, where a batch would have run it unlocked.
        // Still before the lock, as the order above requires.
        locked
            .raw
            .execute_side(&format!("CREATE VIEW data AS SELECT * FROM {reader}"), &[])?;
        if lock {
            locked
                .conn()
                .execute_batch(&format!(
                    "SET allowed_paths = [{allowed}];
                     SET allowed_directories = [{tmp}];
                     SET enable_external_access = false;
                     SET lock_configuration = true;"
                ))
                .map_err(|e| e.to_string())?;
        }
        Ok(locked)
    }

    pub(crate) fn conn(&self) -> &Connection {
        self.raw.conn()
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
        let classifier = self.raw.side();
        // SAFETY: `classifier` is a live connection on a live database; every
        // extracted/prepared handle is destroyed before returning.
        unsafe {
            let mut extracted: ffi::duckdb_extracted_statements = std::ptr::null_mut();
            let n = ffi::duckdb_extract_statements(classifier, c.as_ptr(), &mut extracted);
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
                let ok =
                    ffi::duckdb_prepare_extracted_statement(classifier, extracted, 0, &mut stmt);
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
        let file = file.to_str().unwrap();
        let reader = crate::pq::FileFormat::Parquet.reader(&file_arg(file));
        let db = LockedDb::open(file, &reader, tmp.to_str().unwrap()).unwrap();
        (db, dir)
    }

    #[test]
    fn a_reader_carrying_a_second_statement_is_refused_before_the_lock() {
        // The reader is the one piece of the pre-lock SQL the file has a say
        // in. Run as a batch, the COPY below ran on the still unlocked
        // instance and wrote its file.
        let dir = scratch_dir("lockdown-one-statement");
        let file = fixture(&dir, 10);
        let file = file.to_str().unwrap();
        let written = dir.join("injected.csv");
        let reader = format!(
            "{}; COPY (SELECT 1) TO {}",
            crate::pq::FileFormat::Parquet.reader(&file_arg(file)),
            sql_literal(written.to_str().unwrap())
        );
        let opened = LockedDb::open(file, &reader, dir.join("spill").to_str().unwrap());
        assert!(opened.is_err(), "a two-statement reader must not open");
        assert!(!written.exists(), "the second statement ran: {written:?}");
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

    /// Every statement here reads a file the test created next to the opened
    /// one; `an_unlocked_instance_really_reads` proves each returns rows
    /// without the lock, so a refusal below is the lock's doing and not a
    /// missing file (the `/etc/hosts` these replaced does not exist on Windows).
    fn read_attacks(dir: &Path) -> Vec<String> {
        let other = fixture(&dir.join("other"), 10);
        let text = dir.join("secret.csv");
        std::fs::write(&text, "secret\n42\n").unwrap();
        let lit = |p: &Path| sql_literal(p.to_str().unwrap());
        vec![
            format!("SELECT * FROM read_csv({})", lit(&text)),
            format!("SELECT * FROM read_text({})", lit(&text)),
            format!("SELECT * FROM glob({})", lit(&dir.join("*"))),
            format!("SELECT count(*) FROM read_parquet({})", lit(&other)),
        ]
    }

    #[test]
    fn an_unlocked_instance_really_reads() {
        let dir = scratch_dir("lockdown-read-control");
        let file = fixture(&dir, 10);
        let db =
            LockedDb::open_unlocked(file.to_str().unwrap(), dir.join("spill").to_str().unwrap())
                .unwrap();
        for sql in read_attacks(&dir) {
            let n = run(&db, &sql);
            assert!(
                matches!(n, Ok(n) if n > 0),
                "control: {sql} should read rows, got {n:?}"
            );
        }
    }

    #[test]
    fn nothing_outside_the_file_can_be_read() {
        let (db, dir) = locked();
        for sql in read_attacks(&dir) {
            assert!(run(&db, &sql).is_err(), "{sql} must be refused");
        }
        // No control for this one: without httpfs, which autoload may not
        // fetch, an https read fails locked or not.
        let https = "SELECT * FROM read_parquet('https://example.com/x.parquet')";
        assert!(run(&db, https).is_err(), "{https} must be refused");
    }

    #[test]
    fn extensions_and_settings_cannot_be_changed() {
        // The refusals are the assertion. There was also a check that a
        // `<dir>/ext` directory stayed absent, but nothing points DuckDB's
        // extension directory there, so it could not fail.
        let (db, _dir) = locked();
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
