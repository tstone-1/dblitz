// Before/after benchmark for the filtered+sorted scroll cache.
//
// Reproduces the two code paths that `query_table` chooses between when a view
// has an active (non-regex) filter and a sort:
//
//   BEFORE (un-cached, `query_with_offset`): every scroll chunk re-runs
//     SELECT * FROM t WHERE <filter> ORDER BY <col> LIMIT P OFFSET k
//   so each page re-scans, re-sorts, and skips `k` rows — cost grows with k.
//
//   AFTER (`build_ordered_rows` + `fetch_rows_by_rowids`): materialize the
//     matching rowids in view order ONCE, then serve each chunk as a
//     WHERE rowid IN (...) lookup.
//
// Run: cargo run --release --example filtered_scroll_benchmark [rows] [page] [repeats]
// (Release mode matters — the offset path's cost is in SQLite's C core.)
//
// Both paths are the SHIPPED functions, called through `dblitz_lib::db` and its
// `bench_api` re-exports, against a database opened by `open_database` — so
// these numbers describe the code that runs, not a copy of it that can drift
// away from it.
use std::env;
use std::time::{Duration, Instant};

use dblitz_lib::db::bench_api::{
    build_ordered_rows, fetch_rows_by_rowids, query_with_offset, quote_ident,
};
use dblitz_lib::db::{open_database, DbState};
use rusqlite::{params, Connection};

const DEFAULT_ROWS: i64 = 500_000;
const DEFAULT_PAGE: i64 = 200;
const DEFAULT_REPEATS: usize = 5;
const TABLE: &str = "items";
const WHERE_CLAUSE: &str = " WHERE \"name\" = ? AND \"tag\" = ?";
const ORDER_CLAUSE: &str = " ORDER BY \"n\" ASC";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = arg_i64(1).unwrap_or(DEFAULT_ROWS);
    let page = arg_i64(2).unwrap_or(DEFAULT_PAGE);
    let repeats = arg_usize(3).unwrap_or(DEFAULT_REPEATS);

    let temp = tempfile::NamedTempFile::new()?;
    let matched = {
        let mut conn = Connection::open(temp.path())?;
        seed_database(&mut conn, rows)?
    };

    let state = DbState::new();
    open_database(&state, temp.path().to_str().expect("temp path is UTF-8"))?;
    let guard = state.conn.lock();
    let conn = guard
        .as_ref()
        .expect("open_database published a connection");
    let quoted = quote_ident(TABLE);
    let params: Vec<String> = vec!["keep".to_string(), "SOT".to_string()];

    // Deep scroll targets into the *matched* set (page-aligned).
    let targets = [
        0,
        align(matched / 4, page),
        align(matched / 2, page),
        align(matched * 3 / 4, page),
        align((matched - page).max(0), page),
    ];

    // AFTER: one-time materialization of the ordered rowid list, exactly as
    // `query_with_ordered_rows` does it.
    let build_sql = format!("SELECT rowid FROM {quoted}{WHERE_CLAUSE}{ORDER_CLAUSE}");
    let generation = state.current_query_generation();
    let start = Instant::now();
    let order = build_ordered_rows(conn, &state, generation, &build_sql, &params, 0)?
        .expect("the build must not be cancelled here");
    let build_ms = duration_ms(start.elapsed());
    assert_eq!(order.len() as i64, matched);

    println!("Rows: {rows}  (matched by filter: {matched})  page: {page}  repeats: {repeats}");
    println!("Filtered-order build (one-time): {build_ms:.2} ms");
    println!();
    println!("| Scroll offset | BEFORE offset-scan ms | AFTER rowid-lookup ms | Speedup |");
    println!("|---------------|-----------------------|-----------------------|---------|");

    let mut before_total = 0.0;
    let mut after_total = build_ms;
    for &target in &targets {
        let expected = ((matched - target).min(page)).max(0) as usize;
        let before_ms = median_ms(repeats, expected, || {
            offset_page(conn, &quoted, &params, page, target)
        })?;
        let after_ms = median_ms(repeats, expected, || {
            rowid_page(conn, &quoted, &order, target, page)
        })?;
        before_total += before_ms;
        after_total += after_ms;
        println!(
            "| {} | {:.2} | {:.2} | {:.1}x |",
            target,
            before_ms,
            after_ms,
            before_ms / after_ms.max(0.001)
        );
    }

    println!();
    println!(
        "Total wall to visit all {} targets:  BEFORE {:.2} ms   AFTER {:.2} ms (incl. one-time build)   => {:.1}x",
        targets.len(),
        before_total,
        after_total,
        before_total / after_total.max(0.001)
    );
    Ok(())
}

fn arg_i64(index: usize) -> Option<i64> {
    env::args().nth(index)?.parse().ok()
}

fn arg_usize(index: usize) -> Option<usize> {
    env::args().nth(index)?.parse().ok()
}

fn align(row: i64, page: i64) -> i64 {
    row / page * page
}

/// Seed an on-disk table where rowid order differs from sort order and a column
/// filter matches half the rows. Returns the matched-row count.
fn seed_database(conn: &mut Connection, rows: i64) -> rusqlite::Result<i64> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = OFF;
        PRAGMA synchronous = OFF;
        CREATE TABLE items (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            tag TEXT NOT NULL,
            n INTEGER NOT NULL
        );
        ",
    )?;

    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare("INSERT INTO items (name, tag, n) VALUES (?1, 'SOT', ?2)")?;
        for id in 0..rows {
            // Even ids "keep" (matched), odd "drop". `n` runs opposite to rowid
            // so ORDER BY n forces a real (non-rowid) sort.
            let name = if id % 2 == 0 { "keep" } else { "drop" };
            stmt.execute(params![name, rows - id])?;
        }
    }
    tx.commit()?;
    Ok((rows + 1) / 2)
}

/// BEFORE path: the shipped `query_with_offset`, a fresh filtered+sorted scan
/// that skips `offset` rows per page.
fn offset_page(
    conn: &Connection,
    quoted_table: &str,
    params: &[String],
    limit: i64,
    offset: i64,
) -> Result<usize, String> {
    let result = query_with_offset(
        conn,
        quoted_table,
        WHERE_CLAUSE,
        ORDER_CLAUSE,
        params,
        offset,
        limit,
        None,
        Vec::new(),
    )?;
    Ok(result.rows.len())
}

/// AFTER path, per page: the shipped `fetch_rows_by_rowids` over one window of
/// the cached order (it batches to stay under SQLite's bound-parameter cap).
fn rowid_page(
    conn: &Connection,
    quoted_table: &str,
    order: &[i64],
    offset: i64,
    limit: i64,
) -> Result<usize, String> {
    let start = (offset as usize).min(order.len());
    let end = (offset as usize + limit as usize).min(order.len());
    let rows = fetch_rows_by_rowids(conn, quoted_table, "rowid", &order[start..end])?;
    Ok(rows.len())
}

fn median_ms<F>(
    repeats: usize,
    expected_rows: usize,
    mut f: F,
) -> Result<f64, Box<dyn std::error::Error>>
where
    F: FnMut() -> Result<usize, String>,
{
    let mut durations = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let start = Instant::now();
        let row_count = f()?;
        assert_eq!(row_count, expected_rows);
        durations.push(start.elapsed());
    }
    durations.sort();
    Ok(duration_ms(durations[durations.len() / 2]))
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
