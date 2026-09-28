// SQLite against Parquet on the same data, through the same requests.
//
// Run: cargo run --release --example format_comparison_benchmark -- <dir> [rows] [repeats]
//
// `<dir>` keeps the two generated files between runs (generating 50M rows
// takes minutes); delete it to regenerate. The table is generated once by
// DuckDB and written as Parquet, then copied row by row into SQLite. SQLite
// has no LIST, STRUCT or TIMESTAMP, so those columns are stored as the text
// DuckDB renders for them - which is exactly what the grid shows for both.
//
// Both backends are driven through the SHIPPED code with identical
// `QueryRequest`s: SQLite by `db::open_database` + `db::query_table` +
// `db::execute_sql`, Parquet by `ParquetSession` + its `query_table` and
// `execute_sql` from `db::bench_api`. One-time builds (the SQLite rowid index
// or ordered-rowid list, the Parquet match list or sort cache) are the first
// request for a view, measured once; pages after it are medians. Both files
// were just written, so both are in the OS page cache.
use std::env;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use dblitz_lib::db::bench_api::{parquet_query_table, ParquetSession};
use dblitz_lib::db::{self, ColumnFilter, DbState, QueryRequest};

const DEFAULT_ROWS: i64 = 10_000_000;
const DEFAULT_REPEATS: usize = 5;
const CHUNK: i64 = 500;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = env::args()
        .nth(1)
        .expect("usage: format_comparison_benchmark <dir> [rows] [repeats]");
    let rows: i64 = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ROWS);
    let repeats: usize = env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_REPEATS);
    std::fs::create_dir_all(&dir)?;
    let pq_path = Path::new(&dir).join(format!("data-{rows}.parquet"));
    let db_path = Path::new(&dir).join(format!("data-{rows}.sqlite"));
    if !pq_path.exists() || !db_path.exists() {
        generate(&pq_path, &db_path, rows)?;
    }
    let pq_str = pq_path.to_str().expect("UTF-8 path");
    let db_str = db_path.to_str().expect("UTF-8 path");
    println!(
        "{rows} rows x 14 columns: Parquet {:.2} GB, SQLite {:.2} GB",
        gb(&pq_path),
        gb(&db_path)
    );

    // --- open
    let state = DbState::new();
    let t = Instant::now();
    db::open_database(&state, db_str)?;
    let sq_open = ms(t);
    let t = Instant::now();
    let session = ParquetSession::open(pq_str)?;
    let pq_open = ms(t);
    let gen = AtomicU64::new(0);

    let sq = |r: &QueryRequest| -> Result<f64, String> {
        let t = Instant::now();
        let out = db::query_table(&state, r)?;
        nonempty(out.rows.len(), r)?;
        Ok(ms(t))
    };
    let pq = |r: &QueryRequest| -> Result<f64, String> {
        let t = Instant::now();
        let out = parquet_query_table(&session, r, &gen)?;
        nonempty(out.rows.len(), r)?;
        Ok(ms(t))
    };
    let median = |f: &dyn Fn(&QueryRequest) -> Result<f64, String>, r: &QueryRequest| {
        let mut v = (0..repeats).map(|_| f(r)).collect::<Result<Vec<_>, _>>()?;
        v.sort_by(|a, b| a.total_cmp(b));
        Ok::<f64, String>(v[v.len() / 2])
    };

    println!("\n| Measurement | SQLite | Parquet | Faster |");
    println!("|---|---|---|---|");
    row("Open the file", sq_open, pq_open);

    // --- views: one-time build, then the median page at the middle and the end
    let views: [(&str, QueryRequest, i64); 4] = [
        ("Unfiltered", request(vec![], None), rows),
        (
            "Filtered, `cat` = one of 5 values",
            request(vec![filter("cat", "=alpha", false)], None),
            rows / 5,
        ),
        (
            "Regex filter on a hex string",
            request(vec![filter("name", "^ab.*9$", true)], None),
            -1,
        ),
        (
            "Sorted by a DOUBLE column",
            request(vec![], Some("val")),
            rows,
        ),
    ];
    for (label, base, total) in views {
        let first_sq = sq(&base)?;
        let first_pq = pq(&base)?;
        row(
            &format!("{label}: first page (builds the view)"),
            first_sq,
            first_pq,
        );
        let total = if total >= 0 {
            total
        } else {
            parquet_query_table(&session, &base, &gen)?
                .total_rows
                .unwrap_or(0)
        };
        for (where_, offset) in [("middle", total / 2), ("last", (total - CHUNK).max(0))] {
            let r = QueryRequest {
                offset,
                ..base.clone()
            };
            row(
                &format!("{label}: page, {where_}"),
                median(&sq, &r)?,
                median(&pq, &r)?,
            );
        }
    }

    // --- the SQL tab: whole-table queries
    let queries = [
        (
            "SQL: GROUP BY one column, count + average",
            "SELECT cat, count(*), avg(val) FROM data GROUP BY cat",
        ),
        (
            "SQL: count with a range predicate",
            "SELECT count(*) FROM data WHERE val > 5000",
        ),
        (
            "SQL: distinct count",
            "SELECT count(DISTINCT qty) FROM data",
        ),
        (
            "SQL: top 10 by a column",
            "SELECT id, val FROM data ORDER BY val DESC LIMIT 10",
        ),
    ];
    for (label, sql) in queries {
        let run_sq = || -> Result<f64, String> {
            let t = Instant::now();
            let r = db::execute_sql(&state, sql);
            r.error.map_or(Ok(()), Err)?;
            Ok(ms(t))
        };
        let run_pq = || -> Result<f64, String> {
            let t = Instant::now();
            let r = session.execute_sql(sql, &gen);
            r.error.map_or(Ok(()), Err)?;
            Ok(ms(t))
        };
        let med = |f: &dyn Fn() -> Result<f64, String>| -> Result<f64, String> {
            let mut v = (0..3).map(|_| f()).collect::<Result<Vec<_>, _>>()?;
            v.sort_by(|a, b| a.total_cmp(b));
            Ok(v[1])
        };
        row(label, med(&run_sq)?, med(&run_pq)?);
    }
    Ok(())
}

fn generate(pq: &Path, db: &Path, rows: i64) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let duck = duckdb::Connection::open_in_memory()?;
    duck.execute_batch(&format!(
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
                 {{'a': i % 7, 'b': 'x'}} AS meta
               FROM range({rows}) t(i))
         TO '{}' (FORMAT parquet, COMPRESSION zstd)",
        pq.to_str().unwrap().replace('\'', "''")
    ))?;
    println!("Parquet written in {:.1} s", t.elapsed().as_secs_f64());

    let t = Instant::now();
    let _ = std::fs::remove_file(db);
    let mut lite = rusqlite::Connection::open(db)?;
    lite.execute_batch(
        "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;
         CREATE TABLE data (id INTEGER, ts TEXT, cat TEXT, name TEXT, val REAL,
                            qty INTEGER, flag INTEGER, amount REAL, r1 REAL, r2 REAL,
                            r3 REAL, r4 REAL, tags TEXT, meta TEXT);",
    )?;
    let tx = lite.transaction()?;
    {
        let mut ins = tx.prepare("INSERT INTO data VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")?;
        let mut read = duck.prepare(&format!(
            "SELECT id, CAST(ts AS VARCHAR), cat, name, val, qty, flag::INTEGER,
                    amount::DOUBLE, r1, r2, r3, r4, CAST(tags AS VARCHAR), CAST(meta AS VARCHAR)
             FROM read_parquet('{}')",
            pq.to_str().unwrap().replace('\'', "''")
        ))?;
        let mut it = read.query([])?;
        while let Some(r) = it.next()? {
            ins.execute(rusqlite::params![
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, i32>(5)?,
                r.get::<_, i32>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, Option<f64>>(8)?,
                r.get::<_, f64>(9)?,
                r.get::<_, f64>(10)?,
                r.get::<_, f64>(11)?,
                r.get::<_, String>(12)?,
                r.get::<_, String>(13)?,
            ])?;
        }
    }
    tx.commit()?;
    println!("SQLite written in {:.1} s", t.elapsed().as_secs_f64());
    Ok(())
}

fn request(filters: Vec<ColumnFilter>, sort: Option<&str>) -> QueryRequest {
    QueryRequest {
        table: "data".to_string(),
        offset: 0,
        limit: CHUNK,
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

fn nonempty(n: usize, r: &QueryRequest) -> Result<(), String> {
    if n == 0 {
        Err(format!("empty page at offset {}", r.offset))
    } else {
        Ok(())
    }
}

fn row(label: &str, sqlite_ms: f64, parquet_ms: f64) {
    let fmt = |v: f64| {
        if v >= 1000.0 {
            format!("{:.1} s", v / 1000.0)
        } else if v >= 10.0 {
            format!("{v:.0} ms")
        } else {
            format!("{v:.2} ms")
        }
    };
    let (fast, slow, name) = if sqlite_ms <= parquet_ms {
        (sqlite_ms, parquet_ms, "SQLite")
    } else {
        (parquet_ms, sqlite_ms, "Parquet")
    };
    let ratio = if slow / fast < 1.5 {
        "about equal".to_string()
    } else {
        format!("{name} {:.0}x", slow / fast)
    };
    println!(
        "| {label} | {} | {} | {ratio} |",
        fmt(sqlite_ms),
        fmt(parquet_ms)
    );
}

fn gb(p: &Path) -> f64 {
    std::fs::metadata(p)
        .map(|m| m.len() as f64 / 1e9)
        .unwrap_or(0.0)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
