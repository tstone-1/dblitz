use std::env;
use std::hint::black_box;
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{params, Connection};

const DEFAULT_ROWS: i64 = 1_000_000;
const DEFAULT_CHUNK_SIZE: i64 = 500;
const DEFAULT_REPEATS: usize = 5;
const DB_BROWSER_PREFETCH_SIZE: i64 = 50_000;
const DB_BROWSER_SOURCE_COMMIT: &str = "6cba47ef";

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
    let mut conn = Connection::open(temp.path())?;
    seed_database(&mut conn, rows)?;

    let target_rows = page_aligned_targets(rows, chunk_size);

    let (boundaries, index_build_ms) = timed(|| build_rowid_boundaries(&conn, chunk_size))?;

    let mut measurements = Vec::new();
    for target_row in target_rows {
        let (db_browser_offset, db_browser_rows) =
            db_browser_window(rows, db_browser_prefetch, target_row);
        let baseline_offset_ms = median_ms(repeats, chunk_size as usize, || {
            query_offset(&conn, chunk_size, target_row)
        })?;
        let db_browser_ms = median_ms(repeats, db_browser_rows as usize, || {
            query_offset(&conn, db_browser_rows, db_browser_offset)
        })?;
        let rowid_ms = median_ms(repeats, chunk_size as usize, || {
            query_rowid_range(&conn, &boundaries, chunk_size, target_row)
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

fn build_rowid_boundaries(conn: &Connection, chunk_size: i64) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT rowid FROM records ORDER BY rowid ASC")?;
    let mut rows = stmt.query([])?;
    let mut boundaries = Vec::new();
    let mut idx = 0i64;

    while let Some(row) = rows.next()? {
        if idx % chunk_size == 0 {
            boundaries.push(row.get(0)?);
        }
        idx += 1;
    }

    Ok(boundaries)
}

fn query_offset(conn: &Connection, limit: i64, offset: i64) -> rusqlite::Result<usize> {
    let mut stmt = conn.prepare("SELECT * FROM records LIMIT ? OFFSET ?")?;
    let col_count = stmt.column_count();
    let mut rows = stmt.query(params![limit, offset])?;
    count_rows_and_read(&mut rows, col_count)
}

fn query_rowid_range(
    conn: &Connection,
    boundaries: &[i64],
    limit: i64,
    offset: i64,
) -> rusqlite::Result<usize> {
    debug_assert_eq!(
        offset % limit,
        0,
        "rowid benchmark offsets must be page-aligned"
    );
    let chunk = (offset / limit) as usize;
    let Some(start_rowid) = boundaries.get(chunk) else {
        return Ok(0);
    };

    if let Some(end_rowid) = boundaries.get(chunk + 1) {
        let mut stmt = conn
            .prepare("SELECT * FROM records WHERE rowid >= ? AND rowid < ? ORDER BY rowid ASC")?;
        let col_count = stmt.column_count();
        let mut rows = stmt.query(params![start_rowid, end_rowid])?;
        count_rows_and_read(&mut rows, col_count)
    } else {
        let mut stmt =
            conn.prepare("SELECT * FROM records WHERE rowid >= ? ORDER BY rowid ASC LIMIT ?")?;
        let col_count = stmt.column_count();
        let mut rows = stmt.query(params![start_rowid, limit])?;
        count_rows_and_read(&mut rows, col_count)
    }
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
