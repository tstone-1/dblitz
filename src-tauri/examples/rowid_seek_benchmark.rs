// Deep-paging benchmark: dblitz's rowid seek against LIMIT/OFFSET and against
// DB Browser for SQLite's prefetch-window equivalent.
//
// Run: cargo run --release --example rowid_seek_benchmark [rows] [chunk] [repeats] [prefetch]
// (Release mode matters — the offset path's cost is in SQLite's C core.)
//
// Every measured path calls the SHIPPED code, through `dblitz_lib::db` and its
// `bench_api` re-exports: the database is opened by `open_database` (so the
// connection carries the same flags and PRAGMAs the app uses), the index is
// built by `build_rowid_index`, the seek runs the SQL `rowid_page_sql` writes,
// the baseline runs `query_with_offset`, and every row is materialized by
// `collect_rows`. This file used to re-implement all five, which meant a change
// to the real query path silently stopped being the thing these numbers
// describe.
use std::env;
use std::time::{Duration, Instant};

use dblitz_lib::db::bench_api::{
    build_rowid_index, collect_rows, query_with_offset, quote_ident, rowid_alias, rowid_page_sql,
};
use dblitz_lib::db::{open_database, DbState};
use rusqlite::{params, Connection};

const DEFAULT_ROWS: i64 = 1_000_000;
const DEFAULT_CHUNK_SIZE: i64 = 500;
const DEFAULT_REPEATS: usize = 5;
const DB_BROWSER_PREFETCH_SIZE: i64 = 50_000;
const DB_BROWSER_SOURCE_COMMIT: &str = "6cba47ef";
const TABLE: &str = "records";

#[derive(Debug)]
struct Measurement {
    target_row: i64,
    baseline_offset_ms: f64,
    db_browser_offset: i64,
    db_browser_rows: i64,
    db_browser_ms: f64,
    rowid_ms: f64,
    rowid_speedup_vs_baseline: f64,
    rowid_speedup_vs_db_browser: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = arg_i64(1).unwrap_or(DEFAULT_ROWS);
    let chunk_size = arg_i64(2).unwrap_or(DEFAULT_CHUNK_SIZE);
    let repeats = arg_usize(3).unwrap_or(DEFAULT_REPEATS);
    let db_browser_prefetch = arg_i64(4).unwrap_or(DB_BROWSER_PREFETCH_SIZE);

    let temp = tempfile::NamedTempFile::new()?;
    {
        let mut conn = Connection::open(temp.path())?;
        seed_database(&mut conn, rows)?;
    }

    // The real open path: read-only, immutable, with the app's PRAGMAs.
    let state = DbState::new();
    open_database(&state, temp.path().to_str().expect("temp path is UTF-8"))?;
    let guard = state.conn.lock();
    let conn = guard
        .as_ref()
        .expect("open_database published a connection");
    let quoted = quote_ident(TABLE);
    let alias = rowid_alias(conn, &quoted).expect("a plain rowid table has an addressable rowid");

    let target_rows = page_aligned_targets(rows, chunk_size);

    let generation = state.current_query_generation();
    let start = Instant::now();
    let index = build_rowid_index(conn, &state, generation, TABLE, &quoted, alias, chunk_size)
        .expect("index build must not be cancelled here");
    let index_build_ms = duration_ms(start.elapsed());

    let mut measurements = Vec::new();
    for target_row in target_rows {
        let (db_browser_offset, db_browser_rows) =
            db_browser_window(rows, db_browser_prefetch, target_row);
        let baseline_offset_ms = median_ms(repeats, chunk_size as usize, || {
            offset_page(conn, &quoted, chunk_size, target_row)
        })?;
        let db_browser_ms = median_ms(repeats, db_browser_rows as usize, || {
            offset_page(conn, &quoted, db_browser_rows, db_browser_offset)
        })?;
        let rowid_ms = median_ms(repeats, chunk_size as usize, || {
            rowid_page(
                conn,
                &quoted,
                alias,
                &index.boundaries,
                chunk_size,
                target_row,
            )
        })?;
        measurements.push(Measurement {
            target_row,
            baseline_offset_ms,
            db_browser_offset,
            db_browser_rows,
            db_browser_ms,
            rowid_ms,
            rowid_speedup_vs_baseline: baseline_offset_ms / rowid_ms.max(0.001),
            rowid_speedup_vs_db_browser: db_browser_ms / rowid_ms.max(0.001),
        });
    }

    println!("Rows: {rows}");
    println!("dblitz chunk size: {chunk_size}");
    println!("DB Browser-equivalent prefetch size: {db_browser_prefetch}");
    println!("DB Browser source commit analyzed: {DB_BROWSER_SOURCE_COMMIT}");
    println!("Repeats: {repeats}");
    println!("Rowid index build: {:.2} ms", index_build_ms);
    println!();
    println!(
        "| Target row | LIMIT/OFFSET 500 ms | DB Browser-equivalent rows | DB Browser-equivalent ms | dblitz rowid seek ms | Speedup vs 500 | Speedup vs DB4S |"
    );
    println!(
        "|------------|---------------------|----------------------------|--------------------------|----------------------|----------------|-----------------|"
    );
    for m in measurements {
        println!(
            "| {} | {:.2} | {} @ {} | {:.2} | {:.2} | {:.1}x | {:.1}x |",
            m.target_row,
            m.baseline_offset_ms,
            m.db_browser_rows,
            m.db_browser_offset,
            m.db_browser_ms,
            m.rowid_ms,
            m.rowid_speedup_vs_baseline,
            m.rowid_speedup_vs_db_browser
        );
    }

    Ok(())
}

fn arg_i64(index: usize) -> Option<i64> {
    env::args().nth(index)?.parse().ok()
}

fn arg_usize(index: usize) -> Option<usize> {
    env::args().nth(index)?.parse().ok()
}

fn db_browser_window(total_rows: i64, prefetch_size: i64, target_row: i64) -> (i64, i64) {
    let half_chunk = prefetch_size / 2;
    let row_begin = (target_row - half_chunk).max(0);
    let row_end = (target_row + half_chunk).min(total_rows);
    (row_begin, row_end - row_begin)
}

fn page_aligned_targets(rows: i64, chunk_size: i64) -> [i64; 5] {
    [
        0,
        align_to_page(rows / 4, chunk_size),
        align_to_page(rows / 2, chunk_size),
        align_to_page(rows * 3 / 4, chunk_size),
        align_to_page((rows - chunk_size).max(0), chunk_size),
    ]
}

fn align_to_page(row: i64, chunk_size: i64) -> i64 {
    row / chunk_size * chunk_size
}

fn seed_database(conn: &mut Connection, rows: i64) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = OFF;
        PRAGMA synchronous = OFF;
        CREATE TABLE records (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT NOT NULL,
            amount INTEGER NOT NULL,
            note TEXT NOT NULL
        );
        ",
    )?;

    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "
            INSERT INTO records (name, category, amount, note)
            VALUES (?1, ?2, ?3, ?4)
            ",
        )?;
        for id in 0..rows {
            stmt.execute(params![
                format!("record-{id}"),
                format!("category-{}", id % 20),
                id * 17,
                format!("Synthetic row {id} for dblitz paging benchmark")
            ])?;
        }
    }
    tx.commit()
}

/// The shipped OFFSET path, which is what dblitz falls back to for a table with
/// no addressable rowid - and what every viewer that has no index does for
/// every page.
fn offset_page(
    conn: &Connection,
    quoted_table: &str,
    limit: i64,
    offset: i64,
) -> Result<usize, String> {
    let result = query_with_offset(
        conn,
        quoted_table,
        "",
        "",
        &[],
        offset,
        limit,
        None,
        Vec::new(),
    )?;
    Ok(result.rows.len())
}

/// The shipped rowid seek: the SQL `rowid_page_sql` generates, run against the
/// boundaries `build_rowid_index` sampled, read by `collect_rows`.
fn rowid_page(
    conn: &Connection,
    quoted_table: &str,
    alias: &str,
    boundaries: &[i64],
    limit: i64,
    offset: i64,
) -> Result<usize, String> {
    debug_assert_eq!(
        offset % limit,
        0,
        "rowid benchmark offsets must be page-aligned"
    );
    let chunk = (offset / limit) as usize;
    let Some(start_rowid) = boundaries.get(chunk) else {
        return Ok(0);
    };
    let has_next = boundaries.get(chunk + 1).is_some();
    let sql = rowid_page_sql(quoted_table, alias, has_next);
    let second: i64 = boundaries.get(chunk + 1).copied().unwrap_or(limit);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = collect_rows(&mut stmt, &[start_rowid, &second])?;
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
