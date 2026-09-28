//! An in-memory DuckDB database opened through the C API, with the usual
//! duckdb-rs `Connection` plus one extra raw connection ("side") that dblitz
//! drives itself.
//!
//! duckdb-rs keeps both things this module exists for private: the statement
//! type of a prepared statement (the SQL tab's classifier, [`super::lockdown`])
//! and the connection handle `duckdb_query_progress` needs (the sort-cache
//! build, [`super::query`]). Both need a connection handle of our own on the
//! same database, which the wrapper cannot give out.
//!
//! `Connection::open_from_raw` does NOT take ownership of the database, so
//! `Drop` releases the three handles itself, the wrapper first.

use duckdb::{ffi, Connection};
use std::ffi::{CStr, CString};

pub(crate) struct RawDb {
    /// `Option` only so `Drop` can release it before the database it borrows.
    conn: Option<Connection>,
    side: ffi::duckdb_connection,
    db: ffi::duckdb_database,
}

// SAFETY: the raw handles are used through `&self` only while the owning
// `Mutex` in `ParquetSession` is held, except via `SideHandle`, whose two calls
// DuckDB documents as safe from another thread. Database and connection
// handles may move between threads.
unsafe impl Send for RawDb {}

impl RawDb {
    pub(crate) fn open_in_memory() -> Result<Self, String> {
        let mut db: ffi::duckdb_database = std::ptr::null_mut();
        let mut side: ffi::duckdb_connection = std::ptr::null_mut();
        let memory = CString::new(":memory:").expect("no NUL");
        // SAFETY: plain C API calls with out-pointers we own; each failure path
        // releases what was already created.
        unsafe {
            if ffi::duckdb_open(memory.as_ptr(), &mut db) != ffi::duckdb_state_DuckDBSuccess {
                return Err("could not start the Parquet query engine".to_string());
            }
            if ffi::duckdb_connect(db, &mut side) != ffi::duckdb_state_DuckDBSuccess {
                ffi::duckdb_close(&mut db);
                return Err("could not connect to the Parquet query engine".to_string());
            }
        }
        // SAFETY: `db` is a valid, open database. `open_from_raw` borrows it.
        match unsafe { Connection::open_from_raw(db) } {
            Ok(conn) => Ok(RawDb {
                conn: Some(conn),
                side,
                db,
            }),
            Err(e) => {
                unsafe {
                    ffi::duckdb_disconnect(&mut side);
                    ffi::duckdb_close(&mut db);
                }
                Err(e.to_string())
            }
        }
    }

    pub(crate) fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("connection lives until drop")
    }

    pub(crate) fn side(&self) -> ffi::duckdb_connection {
        self.side
    }

    /// A handle for the two calls another thread may make on the side
    /// connection while it runs a statement.
    pub(crate) fn side_handle(&self) -> SideHandle {
        SideHandle(self.side)
    }

    /// Runs one statement on the side connection, binding `params` as text.
    pub(crate) fn execute_side(&self, sql: &str, params: &[String]) -> Result<(), String> {
        let c = CString::new(sql).map_err(|e| e.to_string())?;
        let params = params
            .iter()
            .map(|p| CString::new(p.as_str()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        // SAFETY: `side` is live for `self`'s lifetime; the statement and the
        // result are destroyed on every path.
        unsafe {
            let mut stmt: ffi::duckdb_prepared_statement = std::ptr::null_mut();
            let prepared = ffi::duckdb_prepare(self.side, c.as_ptr(), &mut stmt);
            let result = if prepared != ffi::duckdb_state_DuckDBSuccess {
                Err(error_text(ffi::duckdb_prepare_error(stmt)))
            } else {
                let mut bind = Ok(());
                for (i, p) in params.iter().enumerate() {
                    if ffi::duckdb_bind_varchar(stmt, (i + 1) as u64, p.as_ptr())
                        != ffi::duckdb_state_DuckDBSuccess
                    {
                        bind = Err(format!("could not bind parameter {}", i + 1));
                        break;
                    }
                }
                bind.and_then(|()| {
                    let mut res: ffi::duckdb_result = std::mem::zeroed();
                    let ok = ffi::duckdb_execute_prepared(stmt, &mut res);
                    let r = if ok == ffi::duckdb_state_DuckDBSuccess {
                        Ok(())
                    } else {
                        Err(error_text(ffi::duckdb_result_error(&mut res)))
                    };
                    ffi::duckdb_destroy_result(&mut res);
                    r
                })
            };
            ffi::duckdb_destroy_prepare(&mut stmt);
            result
        }
    }
}

fn error_text(e: *const std::os::raw::c_char) -> String {
    if e.is_null() {
        "the query failed".to_string()
    } else {
        // SAFETY: DuckDB returns a NUL-terminated string owned by the object
        // it came from, which is still alive here.
        unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
    }
}

impl Drop for RawDb {
    fn drop(&mut self) {
        // The wrapper connection first: it borrows `db` and must not outlive it.
        self.conn.take();
        // SAFETY: both handles were created in `open_in_memory` and are
        // released exactly once.
        unsafe {
            ffi::duckdb_disconnect(&mut self.side);
            ffi::duckdb_close(&mut self.db);
        }
    }
}

/// The side connection, for `duckdb_query_progress` and `duckdb_interrupt`
/// from a thread other than the one running the statement - which is what
/// both calls are for.
#[derive(Clone, Copy)]
pub(crate) struct SideHandle(ffi::duckdb_connection);

// SAFETY: see the type's doc. Valid only while its `RawDb` lives, which the
// owning `ParquetSession` guarantees: the handle never leaves it.
unsafe impl Send for SideHandle {}
unsafe impl Sync for SideHandle {}

impl SideHandle {
    /// Percent done of what the side connection is running, when DuckDB has
    /// an estimate. It needs `enable_progress_bar` on that connection.
    pub(crate) fn progress(&self) -> Option<f64> {
        // SAFETY: see `SideHandle`.
        let p = unsafe { ffi::duckdb_query_progress(self.0) };
        (p.percentage >= 0.0).then_some(p.percentage.min(100.0))
    }

    pub(crate) fn interrupt(&self) {
        // SAFETY: see `SideHandle`.
        unsafe { ffi::duckdb_interrupt(self.0) }
    }
}
