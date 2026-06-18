// Before/after benchmark for the filtered+sorted scroll cache.
//
// Reproduces the two code paths that `query_table` chooses between when a view
// has an active (non-regex) filter and a sort:
//
//   BEFORE (un-cached, query_with_offset): every scroll chunk re-runs
//     SELECT * FROM t WHERE <filter> ORDER BY <col> LIMIT P OFFSET k
//   so each page re-scans, re-sorts, and skips `k` rows — cost grows with k.
//
//   AFTER (query_with_filtered_order): materialize the matching rowids in view
//     order ONCE, then serve each chunk as a WHERE rowid IN (...) lookup.
//
// Run: cargo run --release --example filtered_scroll_benchmark [rows] [page] [repeats]
// (Release mode matters — the offset path's cost is in SQLite's C core.)

use std::env;
use std::hint::black_box;
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection};

const DEFAULT_ROWS: i64 = 500_000;
const DEFAULT_PAGE: i64 = 200;
const DEFAULT_REPEATS: usize = 5;
const BATCH: usize = 900; // mirror fetch_rows_by_rowids' variable-cap batching

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = arg_i64(1).unwrap_or(DEFAULT_ROWS);
    let page = arg_i64(2).unwrap_or(DEFAULT_PAGE);
    let repeats = arg_usize(3).unwrap_or(DEFAULT_REPEATS);

    let temp = tempfile::NamedTempFile::new()?;
    let mut conn = Connection::open(temp.path())?;
    // Match the production read-only connection's sort tuning so the BEFORE
    // numbers aren't unfairly penalised by on-disk temp-file sort spills.
    conn.execute_batch("PRAGMA temp_store=MEMORY;")?;
    let matched = seed_database(&mut conn, rows)?;

    // Deep scroll targets into the *matched* set (page-aligned).
    let targets = [
        0,
        align(matched / 4, page),
        align(matched / 2, page),
        align(matched * 3 / 4, page),
        align((matched - page).max(0), page),
    ];

    // AFTER: one-time materialization of the ordered rowid list.
    let (order, build_ms) = timed(|| build_filtered_order(&conn))?;
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
        let before_ms = median_ms(repeats, expected, || query_offset(&conn, page, target))?;
        let after_ms = median_ms(repeats, expected, || {
            fetch_page_by_rowids(&conn, &order, target, page)
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

/// AFTER path, build step: WHERE <filter> ORDER BY <col> projected to rowids.
fn build_filtered_order(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    let mut stmt =
        conn.prepare("SELECT rowid FROM items WHERE name = 'keep' AND tag = 'SOT' ORDER BY n ASC")?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// BEFORE path: a fresh filtered+sorted scan that skips `offset` rows per page.
fn query_offset(conn: &Connection, limit: i64, offset: i64) -> rusqlite::Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT * FROM items WHERE name = 'keep' AND tag = 'SOT' ORDER BY n ASC LIMIT ? OFFSET ?",
    )?;
    let col_count = stmt.column_count();
    let mut rows = stmt.query(params![limit, offset])?;
    count_rows_and_read(&mut rows, col_count)
}

/// AFTER path, per-page: rowid lookup of one window of the cached order.
fn fetch_page_by_rowids(
    conn: &Connection,
    order: &[i64],
    offset: i64,
    limit: i64,
) -> rusqlite::Result<usize> {
    let start = (offset as usize).min(order.len());
    let end = (offset as usize + limit as usize).min(order.len());
    let page = &order[start..end];
    let mut total = 0usize;
    for batch in page.chunks(BATCH) {
        let placeholders = vec!["?"; batch.len()].join(",");
        let sql = format!("SELECT rowid, * FROM items WHERE rowid IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let col_count = stmt.column_count();
        let params: Vec<&dyn rusqlite::types::ToSql> = batch
            .iter()
            .map(|r| r as &dyn rusqlite::types::ToSql)
            .collect();
        let mut rows = stmt.query(params.as_slice())?;
        total += count_rows_and_read(&mut rows, col_count)?;
    }
    Ok(total)
}

fn count_rows_and_read(rows: &mut rusqlite::Rows<'_>, col_count: usize) -> rusqlite::Result<usize> {
    let mut count = 0usize;
    let mut payload_len = 0usize;
    while let Some(row) = rows.next()? {
        for col in 0..col_count {
            payload_len += match row.get_ref(col)? {
                ValueRef::Null => 0,
                ValueRef::Integer(_) => size_of::<i64>(),
                ValueRef::Real(_) => size_of::<f64>(),
                ValueRef::Text(value) | ValueRef::Blob(value) => value.len(),
            };
        }
        count += 1;
    }
    black_box(payload_len);
    Ok(count)
}

fn median_ms<F>(
    repeats: usize,
    expected_rows: usize,
    mut f: F,
) -> Result<f64, Box<dyn std::error::Error>>
where
    F: FnMut() -> rusqlite::Result<usize>,
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

fn timed<F, T>(f: F) -> Result<(T, f64), Box<dyn std::error::Error>>
where
    F: FnOnce() -> rusqlite::Result<T>,
{
    let start = Instant::now();
    let value = f()?;
    Ok((value, duration_ms(start.elapsed())))
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
