use serde::Serialize;
use std::sync::atomic::Ordering;

use super::query::build_rowid_index;
use super::types::DbState;
use super::util::{read_row, safe_ident, StrErr};

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
    let safe_table = safe_ident(table);

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", safe_table),
            [],
            |row| row.get(0),
        )
        .str_err()?;

    let limit = chunk_size;
    let offsets: Vec<i64> = vec![0, total / 4, total / 2, total * 3 / 4];
    let mut results = Vec::new();

    for &off in &offsets {
        let sql = format!("SELECT * FROM \"{}\" LIMIT ? OFFSET ?", safe_table);
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

    {
        let mut indexes = state.rowid_indexes.lock();
        if !indexes.contains_key(table) {
            let t0 = Instant::now();
            let generation = state.query_generation.load(Ordering::Relaxed);
            if let Some(idx) = build_rowid_index(conn, state, generation, &safe_table, limit) {
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
                    format!(
                        "SELECT * FROM \"{}\" WHERE rowid >= ? AND rowid < ? ORDER BY rowid ASC",
                        safe_table
                    ),
                    start_rid,
                    end_rid,
                )
            } else {
                (
                    format!(
                        "SELECT * FROM \"{}\" WHERE rowid >= ? ORDER BY rowid ASC LIMIT ?",
                        safe_table
                    ),
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
