// Parquet paging benchmark: the three Browse Data paths for a Parquet file.
//
// Run: cargo run --release --example parquet_benchmark [rows] [chunk] [repeats] [extra]
// (Release mode matters - DuckDB's own code is optimized either way, but the
// Rust side that reads every cell is not in a debug build.)
//
// The file is generated here, 14 columns of the kinds the renderer treats
// differently (integers, text, doubles, decimals, timestamps, booleans, a
// LIST and a STRUCT), written by DuckDB with ZSTD and its default row groups.
// `extra` appends that many columns of short text in long runs, for a wide
// file: `700000 500 5 86` is 100 columns in a few MB. The cost of a sorted view
// depends on the column count, which the 14 columns alone do not show.
// Every measured call goes through the SHIPPED code via `db::bench_api`: the
// session is opened by `ParquetSession::open` (both DuckDB instances, the
// app's memory limit and spill directory) and every page by `query_table`.
//
// One-time costs (open, building a filter's match list, building a sort
// cache) are single measurements, as the SQLite example's index build is: a
// second request for the same view is served from the cache, which is what
// the per-page medians then measure. The file has just been written, so it is
// in the OS page cache; on an SSD a cold read measured the same within a few
// milliseconds per page.
use std::env;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use dblitz_lib::db::bench_api::{parquet_query_table, ParquetSession};
use dblitz_lib::db::{ColumnFilter, QueryRequest};

const DEFAULT_ROWS: i64 = 10_000_000;
const DEFAULT_CHUNK_SIZE: i64 = 500;
const DEFAULT_REPEATS: usize = 5;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = arg(1).unwrap_or(DEFAULT_ROWS);
    let chunk = arg(2).unwrap_or(DEFAULT_CHUNK_SIZE);
    let repeats = arg(3).map(|r| r as usize).unwrap_or(DEFAULT_REPEATS);
    let extra = arg(4).unwrap_or(0);

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bench.parquet");
    let path_str = path.to_str().expect("temp path is UTF-8");
    let t = Instant::now();
    write_file(path_str, rows, extra)?;
    let size = std::fs::metadata(&path)?.len();
    println!(
        "Generated {rows} rows x {} columns, {:.1} MB, in {:.1} s",
        14 + extra,
        size as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let session = ParquetSession::open(path_str)?;
    println!("Open (both instances, schema, row count): {:.1} ms", ms(t));
    let generation = AtomicU64::new(0);
    let page = |req: &QueryRequest| -> Result<f64, String> {
        let t = Instant::now();
        let r = parquet_query_table(&session, req, &generation)?;
        if r.rows.is_empty() {
            return Err(format!("empty page at offset {}", req.offset));
        }
        Ok(ms(t))
    };
    let median = |req: &QueryRequest| -> Result<f64, String> {
        let mut v = (0..repeats)
            .map(|_| page(req))
            .collect::<Result<Vec<_>, _>>()?;
        v.sort_by(|a, b| a.total_cmp(b));
        Ok(v[v.len() / 2])
    };
    let depths = |total: i64| {
        let last = (total - chunk).max(0);
        [0, total / 4, total / 2, total * 3 / 4, last]
    };

    // Pages at 0%, 25%, 50%, 75% and the last page of a view of `total` rows.
    let sweep = |base: &QueryRequest, total: i64| -> Result<Vec<f64>, String> {
        depths(total)
            .iter()
            .map(|&offset| {
                median(&QueryRequest {
                    offset,
                    ..base.clone()
                })
            })
            .collect()
    };

    println!("\nMedian of {repeats} reads, {chunk}-row pages.\n");
    println!("| View | One-time build | Page at 0 | 25% | 50% | 75% | Last page |");
    println!("|------|----------------|-----------|-----|-----|-----|-----------|");

    let plain = request(chunk, vec![], None);
    let row = sweep(&plain, rows)?;
    print_row("Unfiltered", None, &row);

    // 1 row in 5 matches, spread evenly through the file.
    let filtered = request(chunk, vec![filter("cat", "=alpha", false)], None);
    let build = page(&filtered)?;
    let row = sweep(&filtered, rows / 5)?;
    print_row("Filtered, `cat` = one of 5 values", Some(build), &row);

    // A sparse regex over 32-character hex strings: ~1 row in 4,000 matches.
    let regex = request(chunk, vec![filter("name", "^ab.*9$", true)], None);
    let build = page(&regex)?;
    let matches = parquet_query_table(&session, &regex, &generation)?
        .total_rows
        .unwrap_or(0);
    let row = sweep(&regex, matches)?;
    print_row(
        &format!("Regex filter, {matches} matches"),
        Some(build),
        &row,
    );

    let sorted = request(chunk, vec![], Some("val"));
    let build = page(&sorted)?;
    let row = sweep(&sorted, rows)?;
    print_row("Sorted by a DOUBLE column", Some(build), &row);

    Ok(())
}

fn write_file(path: &str, rows: i64, extra: i64) -> Result<(), duckdb::Error> {
    let conn = duckdb::Connection::open_in_memory()?;
    // Run length and distinct count differ per column, so the columns do not
    // compress as copies of one another.
    let wide: String = (1..=extra)
        .map(|j| {
            let run = 500 + (j * 7919) % 20_000;
            let distinct = 3 + (j * 31) % 200;
            format!(
                ",
                 'value-' || lpad(((i // {run}) % {distinct})::VARCHAR, 5, '0') AS w{j}"
            )
        })
        .collect();
    conn.execute_batch(&format!(
        "COPY (SELECT i AS id,
                 TIMESTAMP '2020-01-01' + to_seconds(i) AS ts,
                 ['alpha','beta','gamma','delta','epsilon'][(i % 5) + 1] AS cat,
                 md5(i::VARCHAR) AS name,
                 (hash(i) % 1000000) / 100.0 AS val,
                 (i % 1000)::INTEGER AS qty,
                 (i % 3 = 0) AS flag,
                 ((hash(i * 7) % 100000000)::DECIMAL(18,4) / 100) AS amount,
                 CASE WHEN i % 10 = 0 THEN NULL ELSE random() END AS r1,
                 random() AS r2, random() AS r3, random() AS r4,
                 [i % 3, i % 5] AS tags,
                 {{'a': i % 7, 'b': 'x'}} AS meta{wide}
               FROM range({rows}) t(i))
         TO '{}' (FORMAT parquet, COMPRESSION zstd)",
        path.replace('\'', "''")
    ))
}

fn request(chunk: i64, filters: Vec<ColumnFilter>, sort: Option<&str>) -> QueryRequest {
    QueryRequest {
        table: "data".to_string(),
        offset: 0,
        limit: chunk,
        filters,
        global_filter: String::new(),
        sort_column: sort.map(str::to_string),
        sort_asc: true,
    }
}

fn filter(column: &str, value: &str, is_regex: bool) -> ColumnFilter {
    ColumnFilter {
        column: column.to_string(),
        value: value.to_string(),
        is_regex,
    }
}

fn print_row(label: &str, build_ms: Option<f64>, pages: &[f64]) {
    let build = match build_ms {
        Some(b) if b >= 1000.0 => format!("{:.1} s", b / 1000.0),
        Some(b) => format!("{b:.0} ms"),
        None => "-".to_string(),
    };
    let cells: Vec<String> = pages.iter().map(|p| format!("{p:.1} ms")).collect();
    println!("| {label} | {build} | {} |", cells.join(" | "));
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn arg(index: usize) -> Option<i64> {
    env::args().nth(index)?.parse().ok()
}
