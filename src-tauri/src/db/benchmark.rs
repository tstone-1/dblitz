use super::{build_rowid_index, read_row, safe_ident, DbState, StrErr};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Serialize, Clone)]
pub struct BenchmarkResult {
    pub label: String,
    pub offset: i64,
    pub ms: f64,
    pub row_count: usize,
}

pub fn benchmark_query(
    state: &DbState,
    table: &str,
    chunk_size: i64,
) -> Result<Vec<BenchmarkResult>, String> {
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
    if limit <= 0 {
        return Err("chunk_size must be positive".to_string());
    }
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
        let index_key = format!("{table}\0{limit}");
        let mut indexes = state.rowid_indexes.lock();
        if let std::collections::hash_map::Entry::Vacant(entry) = indexes.entry(index_key) {
            let t0 = Instant::now();
            if let Some(idx) = build_rowid_index(conn, &safe_table, limit) {
                let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                results.push(BenchmarkResult {
                    label: "index build".to_string(),
                    offset: 0,
                    ms: build_ms,
                    row_count: idx.boundaries.len(),
                });
                entry.insert(idx);
            }
        }
    }

    let index_key = format!("{table}\0{limit}");
    let indexes = state.rowid_indexes.lock();
    if let Some(idx) = indexes.get(&index_key) {
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
