use serde::Serialize;
use std::sync::atomic::Ordering;

use super::query::{build_rowid_index, rowid_alias, rowid_page_sql};
use super::types::DbState;
use super::util::{quote_ident, read_row, StrErr};

#[cfg(debug_assertions)]
#[derive(Debug, Serialize, Clone)]
pub struct BenchmarkResult {
    pub label: String,
    pub offset: i64,
    pub ms: f64,
    pub row_count: usize,
}

#[cfg(debug_assertions)]
pub fn benchmark_query(
    state: &DbState,
    table: &str,
    chunk_size: i64,
) -> Result<Vec<BenchmarkResult>, String> {
    use std::time::Instant;
    let guard = state.conn.lock();
    let conn = guard.as_ref().ok_or("No database open")?;
    let quoted_table = quote_ident(table);

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {}", quoted_table),
            [],
            |row| row.get(0),
        )
        .str_err()?;

    let limit = chunk_size;
    let offsets: Vec<i64> = vec![0, total / 4, total / 2, total * 3 / 4];
    let mut results = Vec::new();

    for &off in &offsets {
        let sql = format!("SELECT * FROM {} LIMIT ? OFFSET ?", quoted_table);
        let t0 = Instant::now();
        let mut stmt = conn.prepare(&sql).str_err()?;
        let col_count = stmt.column_count();
        let mut rows_iter = stmt.query(rusqlite::params![limit, off]).str_err()?;
        let mut count = 0usize;
        while let Some(row) = rows_iter.next().str_err()? {
            read_row(row, col_count);
            count += 1;
        }
        drop(rows_iter);
        drop(stmt);
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        results.push(BenchmarkResult {
            label: "LIMIT/OFFSET".to_string(),
            offset: off,
            ms: elapsed,
            row_count: count,
        });
    }

    // Benchmarking the rowid-index fast path only makes sense when the table
    // has a usable (unshadowed) rowid alias; skip it otherwise, same as the
    // real query path falling back to LIMIT/OFFSET in that case.
    let alias = match rowid_alias(conn, &quoted_table) {
        Some(a) => a,
        None => return Ok(results),
    };

    {
        let mut indexes = state.rowid_indexes.lock();
        if !indexes.contains_key(table) {
            let t0 = Instant::now();
            let generation = state.query_generation.load(Ordering::Relaxed);
            if let Some(idx) =
                build_rowid_index(conn, state, generation, &quoted_table, alias, limit)
            {
                let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                results.push(BenchmarkResult {
                    label: "index build".to_string(),
                    offset: 0,
                    ms: build_ms,
                    row_count: idx.boundaries.len(),
                });
                indexes.insert(table.to_string(), idx);
            }
        }
    }

    let indexes = state.rowid_indexes.lock();
    if let Some(idx) = indexes.get(table) {
        for &off in &offsets {
            let chunk = (off / limit) as usize;
            if chunk >= idx.boundaries.len() {
                continue;
            }
            let start_rid = idx.boundaries[chunk];

            let t0 = Instant::now();
            let (sql, p1, p2): (String, i64, i64) = if chunk + 1 < idx.boundaries.len() {
                let end_rid = idx.boundaries[chunk + 1];
                (
                    rowid_page_sql(&quoted_table, alias, true),
                    start_rid,
                    end_rid,
                )
            } else {
                (
                    rowid_page_sql(&quoted_table, alias, false),
                    start_rid,
                    limit,
                )
            };

            let mut stmt = conn.prepare(&sql).str_err()?;
            let col_count = stmt.column_count();
            let mut rows_iter = stmt.query(rusqlite::params![p1, p2]).str_err()?;
            let mut count = 0usize;
            while let Some(row) = rows_iter.next().str_err()? {
                read_row(row, col_count);
                count += 1;
            }
            drop(rows_iter);
            drop(stmt);
            let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
            results.push(BenchmarkResult {
                label: "rowid index".to_string(),
                offset: off,
                ms: elapsed,
                row_count: count,
            });
        }
    }

    Ok(results)
}
