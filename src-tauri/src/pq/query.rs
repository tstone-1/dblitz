//! Browse Data paging for a Parquet file. Three paths, each measured on a
//! 50M-row, 14-column, 3 GB file (DuckDB 1.5.5, M-series Mac):
//!
//! - **Plain view**: `WHERE file_row_number BETWEEN` lets DuckDB skip row
//!   groups by their row-number statistics - 13-22 ms per 500 rows at any
//!   offset, cold or warm. `LIMIT/OFFSET` grew to 82 ms at row 50M.
//! - **Filtered, unsorted**: the matching row numbers are collected once, in
//!   file order, and each chunk is fetched with `IN (...)`. DuckDB skips row
//!   groups using the list's min and max: 17-31 ms per chunk.
//! - **Sorted** (filtered or not): the view is materialized once, rendered and
//!   in its own column types, into a DuckDB file in the cache directory, and
//!   paged by `rowid`, rendering each page on the way out. All 14 columns took
//!   14.7 s, 3.18 GB on disk and 2.7 GB of RAM; a page then costs ~5 ms. A cached order list was rejected: each chunk
//!   touches nearly every row group, ~500 ms whatever the chunk size.
//!
//! Only the active view is kept, as on the SQLite path. Its key is the full
//! WHERE (clause and params) plus the sort, so any change rebuilds.

use duckdb::{params_from_iter, Connection};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::filters::{build_where, Where};
use super::raw::RawDb;
use super::{check_table, quote_ident, render_expr, sql_literal, ParquetSession};
use crate::db::{ColumnFilter, QueryRequest, QueryResult};

/// Rendered cells, row by row, as the grid receives them.
type Rows = Vec<Vec<Option<String>>>;
/// One page of rows and the view's total row count.
type Page = (Rows, i64);

const MAX_QUERY_LIMIT: i64 = 100_000;
/// How often a list build checks for cancellation.
const CANCEL_CHECK_EVERY: usize = 65_536;
const CANCELLED: &str = "Query cancelled by a newer request";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewKey {
    filter: Where,
    /// Column name and ascending. `None` for an unsorted view.
    sort: Option<(String, bool)>,
}

enum View {
    Filtered { key: ViewKey, rows: Vec<i64> },
    Sorted { key: ViewKey, count: i64 },
}

pub(crate) struct Views {
    active: Option<View>,
    sort_cache: PathBuf,
    attached: bool,
}

impl Views {
    pub(crate) fn new(sort_cache: PathBuf) -> Self {
        Views {
            active: None,
            sort_cache,
            attached: false,
        }
    }
}

/// Detaches and deletes the sort cache. Called when the session drops.
pub(crate) fn drop_sort_cache(conn: &Connection, views: &mut Views) {
    views.active = None;
    if views.attached {
        let _ = conn.execute_batch("DETACH sc;");
        views.attached = false;
    }
    remove_cache_file(&views.sort_cache);
}

fn remove_cache_file(path: &Path) {
    let _ = std::fs::remove_file(path);
    let mut wal = path.as_os_str().to_owned();
    wal.push(".wal");
    let _ = std::fs::remove_file(PathBuf::from(wal));
}

pub fn query_table(
    s: &ParquetSession,
    req: &QueryRequest,
    generation: &AtomicU64,
) -> Result<QueryResult, String> {
    if req.limit <= 0 {
        return Err("Query limit must be greater than zero".to_string());
    }
    if req.limit > MAX_QUERY_LIMIT {
        return Err(format!("Query limit must not exceed {MAX_QUERY_LIMIT}"));
    }
    if req.offset < 0 {
        return Err("Query offset must be zero or greater".to_string());
    }
    check_table(&req.table)?;
    let gen = generation.load(Ordering::Relaxed);
    let filter = build_where(&s.columns, &req.filters, &req.global_filter)?;
    // A sort column the table does not have is ignored, as on SQLite.
    let sort = req
        .sort_column
        .as_ref()
        .filter(|c| s.columns.iter().any(|col| &col.name == *c))
        .map(|c| (c.clone(), req.sort_asc));
    let columns: Vec<String> = s.columns.iter().map(|c| c.name.clone()).collect();
    let proj = s
        .columns
        .iter()
        .map(render_expr)
        .collect::<Vec<_>>()
        .join(", ");

    let browse = s.browse.lock();
    let conn = browse.conn();
    let (rows, total) = if !s.row_numbers {
        offset_page(conn, &proj, &filter, sort.as_ref(), req, gen, generation)?
    } else if sort.is_none() && filter.clause.is_empty() {
        let end = req.offset.saturating_add(req.limit);
        let sql = format!(
            "SELECT {proj} FROM data_rn WHERE file_row_number >= {} AND file_row_number < {end} ORDER BY file_row_number",
            req.offset
        );
        (fetch(conn, &sql, &[], gen, generation)?, s.total_rows)
    } else {
        let key = ViewKey { filter, sort };
        let mut views = s.views.lock();
        if key.sort.is_some() {
            sorted_page(s, &browse, &mut views, key, &proj, req, gen, generation)?
        } else {
            filtered_page(conn, &mut views, key, &proj, req, gen, generation)?
        }
    };
    if generation.load(Ordering::Relaxed) != gen {
        return Err(CANCELLED.to_string());
    }
    Ok(QueryResult {
        columns,
        rows,
        total_rows: Some(total),
        offset: req.offset,
    })
}

pub(crate) fn count_rows(
    s: &ParquetSession,
    table: &str,
    filters: &[ColumnFilter],
    global_filter: &str,
    generation: &AtomicU64,
) -> Result<i64, String> {
    check_table(table)?;
    let filter = build_where(&s.columns, filters, global_filter)?;
    if filter.clause.is_empty() {
        return Ok(s.total_rows);
    }
    let browse = s.browse.lock();
    let conn = browse.conn();
    let gen = generation.load(Ordering::Relaxed);
    if !s.row_numbers {
        return count_where(conn, &filter);
    }
    let key = ViewKey { filter, sort: None };
    let mut views = s.views.lock();
    ensure_filtered(conn, &mut views, key, gen, generation).map(|rows| rows.len() as i64)
}

fn count_where(conn: &Connection, filter: &Where) -> Result<i64, String> {
    conn.query_row(
        &format!("SELECT count(*) FROM data{}", filter.clause),
        params_from_iter(&filter.params),
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// `ORDER BY` for a sort, NULLs placed as SQLite places them (first when
/// ascending), with the file order as the tiebreak so pages are stable.
fn order_by(sort: &(String, bool), tiebreak: &str) -> String {
    let (col, asc) = sort;
    let dir = if *asc {
        "ASC NULLS FIRST"
    } else {
        "DESC NULLS LAST"
    };
    format!(" ORDER BY {} {dir}{tiebreak}", quote_ident(col))
}

/// Fallback for a file whose own column is named `file_row_number`.
#[allow(clippy::too_many_arguments)]
fn offset_page(
    conn: &Connection,
    proj: &str,
    filter: &Where,
    sort: Option<&(String, bool)>,
    req: &QueryRequest,
    gen: u64,
    generation: &AtomicU64,
) -> Result<Page, String> {
    let order = sort.map(|s| order_by(s, "")).unwrap_or_default();
    let sql = format!(
        "SELECT {proj} FROM data{}{order} LIMIT {} OFFSET {}",
        filter.clause, req.limit, req.offset
    );
    let rows = fetch(conn, &sql, &filter.params, gen, generation)?;
    Ok((rows, count_where(conn, filter)?))
}

fn ensure_filtered<'v>(
    conn: &Connection,
    views: &'v mut Views,
    key: ViewKey,
    gen: u64,
    generation: &AtomicU64,
) -> Result<&'v [i64], String> {
    let fresh = matches!(&views.active, Some(View::Filtered { key: k, .. }) if *k == key);
    if !fresh {
        views.active = None;
        let sql = format!(
            "SELECT file_row_number FROM data_rn{} ORDER BY file_row_number",
            key.filter.clause
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut it = stmt
            .query(params_from_iter(&key.filter.params))
            .map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        while let Some(r) = it.next().map_err(|e| e.to_string())? {
            rows.push(r.get::<_, i64>(0).map_err(|e| e.to_string())?);
            if rows.len() % CANCEL_CHECK_EVERY == 0 && generation.load(Ordering::Relaxed) != gen {
                return Err(CANCELLED.to_string());
            }
        }
        views.active = Some(View::Filtered { key, rows });
    }
    match &views.active {
        Some(View::Filtered { rows, .. }) => Ok(rows),
        _ => unreachable!("set above"),
    }
}

#[allow(clippy::too_many_arguments)]
fn filtered_page(
    conn: &Connection,
    views: &mut Views,
    key: ViewKey,
    proj: &str,
    req: &QueryRequest,
    gen: u64,
    generation: &AtomicU64,
) -> Result<Page, String> {
    let all = ensure_filtered(conn, views, key, gen, generation)?;
    let total = all.len() as i64;
    let start = (req.offset as usize).min(all.len());
    let end = start.saturating_add(req.limit as usize).min(all.len());
    let chunk = &all[start..end];
    if chunk.is_empty() {
        return Ok((Vec::new(), total));
    }
    // Integer literals: the list is ours, and binding thousands of
    // parameters is slower than parsing them.
    let list = chunk
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {proj} FROM data_rn WHERE file_row_number IN ({list}) ORDER BY file_row_number"
    );
    Ok((fetch(conn, &sql, &[], gen, generation)?, total))
}

#[allow(clippy::too_many_arguments)]
fn sorted_page(
    s: &ParquetSession,
    browse: &RawDb,
    views: &mut Views,
    key: ViewKey,
    proj: &str,
    req: &QueryRequest,
    gen: u64,
    generation: &AtomicU64,
) -> Result<Page, String> {
    let conn = browse.conn();
    let fresh = matches!(&views.active, Some(View::Sorted { key: k, .. }) if *k == key);
    if !fresh {
        views.active = None;
        if !views.attached {
            if let Some(dir) = views.sort_cache.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            remove_cache_file(&views.sort_cache);
            conn.execute_batch(&format!(
                "ATTACH {} AS sc;",
                sql_literal(&views.sort_cache.to_string_lossy())
            ))
            .map_err(|e| format!("could not create the sort cache: {e}"))?;
            views.attached = true;
        }
        let sort = key.sort.as_ref().expect("sorted view");
        // The cache holds the columns in their own types and each page renders
        // its 500 rows on the way out. Caching the rendered text instead took
        // 34.5 s for a 50M-row, 14-column sort against 14.7 s this way.
        let names = s
            .columns
            .iter()
            .map(|c| quote_ident(&c.name))
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute_batch("DROP TABLE IF EXISTS sc.v;")
            .map_err(|e| e.to_string())?;
        let create = format!(
            "CREATE TABLE sc.v AS SELECT {names} FROM data_rn{}{}",
            key.filter.clause,
            order_by(sort, ", file_row_number")
        );
        // Built on the side connection, which is the one `view_progress` can
        // ask DuckDB about while this runs. The filter's params are values the
        // user typed, so they are bound, never spliced.
        s.building.store(true, Ordering::Release);
        let built = browse.execute_side(&create, &key.filter.params);
        s.building.store(false, Ordering::Release);
        if let Err(e) = built {
            let _ = conn.execute_batch("DROP TABLE IF EXISTS sc.v; CHECKPOINT sc;");
            return Err(if generation.load(Ordering::Relaxed) != gen {
                CANCELLED.to_string()
            } else {
                e
            });
        }
        let count: i64 = conn
            .query_row("SELECT count(*) FROM sc.v", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        views.active = Some(View::Sorted { key, count });
    }
    let Some(View::Sorted { count, .. }) = &views.active else {
        unreachable!("set above")
    };
    let end = req.offset.saturating_add(req.limit);
    let sql = format!(
        "SELECT {proj} FROM sc.v WHERE rowid >= {} AND rowid < {end} ORDER BY rowid",
        req.offset
    );
    Ok((fetch(conn, &sql, &[], gen, generation)?, *count))
}

/// Runs a query whose every column is already rendered to VARCHAR.
fn fetch(
    conn: &Connection,
    sql: &str,
    params: &[String],
    gen: u64,
    generation: &AtomicU64,
) -> Result<Rows, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let mut it = stmt
        .query(params_from_iter(params))
        .map_err(|e| e.to_string())?;
    let n = it.as_ref().map(|s| s.column_count()).unwrap_or(0);
    let mut out = Vec::new();
    while let Some(r) = it.next().map_err(|e| e.to_string())? {
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            row.push(r.get::<_, Option<String>>(i).map_err(|e| e.to_string())?);
        }
        out.push(row);
        if out.len() % 4096 == 0 && generation.load(Ordering::Relaxed) != gen {
            return Err(CANCELLED.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{fixture, scratch_dir};
    use super::*;

    fn req(offset: i64, limit: i64) -> QueryRequest {
        QueryRequest {
            table: "data".into(),
            offset,
            limit,
            filters: vec![],
            global_filter: String::new(),
            sort_column: None,
            sort_asc: true,
        }
    }

    fn filter(column: &str, value: &str, is_regex: bool) -> ColumnFilter {
        ColumnFilter {
            column: column.into(),
            value: value.into(),
            is_regex,
        }
    }

    fn session(rows: u64) -> ParquetSession {
        let dir = scratch_dir("query");
        ParquetSession::open(fixture(&dir, rows).to_str().unwrap()).unwrap()
    }

    fn ids(r: &QueryResult) -> Vec<i64> {
        r.rows
            .iter()
            .map(|row| row[0].as_ref().unwrap().parse().unwrap())
            .collect()
    }

    #[test]
    fn a_plain_page_at_any_offset_is_the_right_rows() {
        let s = session(2500);
        let g = AtomicU64::new(0);
        for offset in [0, 999, 1000, 2400] {
            let r = query_table(&s, &req(offset, 100), &g).unwrap();
            assert_eq!(
                ids(&r),
                (offset..(offset + 100).min(2500)).collect::<Vec<_>>()
            );
            assert_eq!(r.total_rows, Some(2500));
        }
        assert!(query_table(&s, &req(2500, 10), &g).unwrap().rows.is_empty());
    }

    #[test]
    fn cells_render_as_duckdb_casts_them() {
        let s = session(10);
        let r = query_table(&s, &req(0, 10), &AtomicU64::new(0)).unwrap();
        assert_eq!(r.columns, ["id", "name", "val", "cat", "b", "tags"]);
        // Row 0: val is NULL (0 % 7 == 0). Row 3: val 1.5, tags [0, 3].
        assert_eq!(r.rows[0][2], None);
        let row3: Vec<_> = r.rows[3]
            .iter()
            .map(|c| c.clone().unwrap_or_default())
            .collect();
        assert_eq!(row3, ["3", "n3", "1.5", "a", "[BLOB 3 bytes]", "[0, 3]"]);
    }

    #[test]
    fn a_filtered_view_pages_through_exactly_its_matches() {
        let s = session(2500);
        let g = AtomicU64::new(0);
        let mut q = req(0, 50);
        q.filters = vec![filter("cat", "=b", false)];
        let expected: Vec<i64> = (0..2500).filter(|i| i % 3 == 1).collect();
        let mut got = Vec::new();
        let mut offset = 0;
        loop {
            q.offset = offset;
            let r = query_table(&s, &q, &g).unwrap();
            assert_eq!(r.total_rows, Some(expected.len() as i64));
            if r.rows.is_empty() {
                break;
            }
            got.extend(ids(&r));
            offset += 50;
        }
        assert_eq!(got, expected);
        assert_eq!(
            count_rows(&s, "data", &q.filters, "", &g).unwrap(),
            expected.len() as i64
        );
    }

    #[test]
    fn a_regex_matches_rendered_text_with_null_as_empty() {
        let s = session(30);
        let g = AtomicU64::new(0);
        let mut q = req(0, 100);
        // `^$` matches exactly the NULL vals: rows 0, 7, 14, 21, 28.
        q.filters = vec![filter("val", "^$", true)];
        assert_eq!(ids(&query_table(&s, &q, &g).unwrap()), [0, 7, 14, 21, 28]);
        // `\.5$` matches the rendered text "0.5", "1.5", ... of odd ids.
        q.filters = vec![filter("val", r"\.5$", true)];
        let r = query_table(&s, &q, &g).unwrap();
        assert!(ids(&r).iter().all(|i| i % 2 == 1 && i % 7 != 0));
        // A BLOB never matches, whatever the pattern.
        q.filters = vec![filter("b", ".*", true)];
        assert!(query_table(&s, &q, &g).unwrap().rows.is_empty());
    }

    #[test]
    fn changing_the_filter_rebuilds_the_view() {
        let s = session(100);
        let g = AtomicU64::new(0);
        let mut q = req(0, 100);
        q.filters = vec![filter("cat", "=a", false)];
        let a = query_table(&s, &q, &g).unwrap().total_rows;
        q.filters = vec![filter("cat", "=b", false)];
        let b = query_table(&s, &q, &g).unwrap();
        assert_eq!((a, b.total_rows), (Some(34), Some(33)));
        assert!(ids(&b).iter().all(|i| i % 3 == 1));
    }

    #[test]
    fn a_sorted_view_pages_in_sort_order_with_nulls_first() {
        let s = session(2500);
        let g = AtomicU64::new(0);
        let mut q = req(0, 700);
        q.sort_column = Some("val".into());
        q.sort_asc = true;
        let mut got: Vec<Option<f64>> = Vec::new();
        for offset in (0..2500).step_by(700) {
            q.offset = offset;
            let r = query_table(&s, &q, &g).unwrap();
            assert_eq!(r.total_rows, Some(2500));
            got.extend(
                r.rows
                    .iter()
                    .map(|row| row[2].as_ref().map(|v| v.parse().unwrap())),
            );
        }
        assert_eq!(got.len(), 2500);
        let nulls = got.iter().take_while(|v| v.is_none()).count();
        assert_eq!(nulls, (0..2500).filter(|i| i % 7 == 0).count());
        let vals: Vec<f64> = got[nulls..].iter().map(|v| v.unwrap()).collect();
        assert!(vals.windows(2).all(|w| w[0] <= w[1]), "not sorted");
        // Descending puts the largest first and the NULLs last. The largest
        // is row 2498 (2499 = 7 * 357 is a NULL row).
        q.sort_asc = false;
        q.offset = 0;
        let r = query_table(&s, &q, &g).unwrap();
        assert_eq!(r.rows[0][2].as_deref(), Some("1249.0"));
        q.offset = 2499;
        let r = query_table(&s, &q, &g).unwrap();
        assert_eq!(r.rows[0][2], None);
    }

    #[test]
    fn a_sort_is_by_value_not_by_rendered_text() {
        // As text, "10" < "9". The cache must order by the column's type.
        let s = session(20);
        let g = AtomicU64::new(0);
        let mut q = req(0, 20);
        q.sort_column = Some("id".into());
        assert_eq!(
            ids(&query_table(&s, &q, &g).unwrap()),
            (0..20).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_sorted_and_filtered_view_holds_only_the_matches() {
        let s = session(300);
        let g = AtomicU64::new(0);
        let mut q = req(0, 300);
        q.filters = vec![filter("cat", "=c", false)];
        q.sort_column = Some("id".into());
        q.sort_asc = false;
        let r = query_table(&s, &q, &g).unwrap();
        let expected: Vec<i64> = (0..300).rev().filter(|i| i % 3 == 2).collect();
        assert_eq!(ids(&r), expected);
        assert_eq!(r.total_rows, Some(expected.len() as i64));
    }

    #[test]
    fn the_sort_cache_is_deleted_when_the_session_ends() {
        let s = session(100);
        let mut q = req(0, 10);
        q.sort_column = Some("val".into());
        query_table(&s, &q, &AtomicU64::new(0)).unwrap();
        let cache = s.views.lock().sort_cache.clone();
        assert!(cache.exists(), "precondition: sorting created {cache:?}");
        drop(s);
        assert!(!cache.exists(), "the sort cache outlived its session");
    }

    #[test]
    fn a_comparison_operand_that_does_not_fit_the_column_is_an_error() {
        let s = session(10);
        let mut q = req(0, 10);
        q.filters = vec![filter("id", ">abc", false)];
        assert!(query_table(&s, &q, &AtomicU64::new(0)).is_err());
    }

    /// A sort over enough rows that its build takes a visible while, run on
    /// another thread; returns the session and the join handle.
    fn start_big_sort(
        rows: u64,
    ) -> (
        std::sync::Arc<ParquetSession>,
        std::sync::Arc<AtomicU64>,
        std::thread::JoinHandle<Result<QueryResult, String>>,
    ) {
        let dir = scratch_dir("progress");
        let s = std::sync::Arc::new(
            ParquetSession::open(fixture(&dir, rows).to_str().unwrap()).unwrap(),
        );
        let g = std::sync::Arc::new(AtomicU64::new(0));
        let (s2, g2) = (s.clone(), g.clone());
        let handle = std::thread::spawn(move || {
            let mut q = req(0, 100);
            q.sort_column = Some("name".into());
            query_table(&s2, &q, &g2)
        });
        (s, g, handle)
    }

    /// Polls `view_progress` until it reports, or the build ends first.
    fn first_progress(
        s: &ParquetSession,
        handle: &std::thread::JoinHandle<Result<QueryResult, String>>,
    ) -> Option<f64> {
        while !handle.is_finished() {
            if let Some(p) = s.view_progress() {
                return Some(p);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn a_sort_build_reports_progress_and_nothing_else_does() {
        let idle = session(10);
        assert_eq!(
            idle.view_progress(),
            None,
            "an idle session reports nothing"
        );
        let (s, _g, handle) = start_big_sort(3_000_000);
        let seen = first_progress(&s, &handle);
        let r = handle.join().unwrap().unwrap();
        assert_eq!(r.total_rows, Some(3_000_000));
        let p = seen.expect("a 3M-row sort build must report progress while it runs");
        assert!((0.0..=100.0).contains(&p), "progress {p} out of range");
        assert_eq!(
            s.view_progress(),
            None,
            "no progress once the build is done"
        );
        // A page served from the finished cache reports nothing either.
        let mut q = req(1000, 100);
        q.sort_column = Some("name".into());
        query_table(&s, &q, &AtomicU64::new(0)).unwrap();
        assert_eq!(s.view_progress(), None);
    }

    #[test]
    fn interrupting_stops_a_sort_build() {
        let (s, _g, handle) = start_big_sort(3_000_000);
        assert!(
            first_progress(&s, &handle).is_some(),
            "precondition: the build was still running"
        );
        // No generation bump: with one, `query_table` reports "cancelled"
        // after a build that ran to completion, so only a build that really
        // stopped can fail here.
        s.interrupt();
        let err = handle.join().unwrap().unwrap_err();
        assert!(
            err.contains("INTERRUPT"),
            "the build was not stopped: {err}"
        );
        assert_eq!(s.view_progress(), None);
    }

    #[test]
    fn an_unknown_table_is_refused() {
        let s = session(10);
        let mut q = req(0, 10);
        q.table = "other".into();
        assert_eq!(
            query_table(&s, &q, &AtomicU64::new(0)).unwrap_err(),
            "no such table: other"
        );
    }
}
