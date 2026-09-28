//! Writes `scripts/fixtures/smoke.parquet`, the Parquet file
//! `scripts/smoke-test.mjs` opens in the packaged app. Node has no Parquet
//! writer, and running this from the smoke step recompiled DuckDB there, so the
//! output is committed. Regenerate it with
//! `cargo run --example make_parquet_fixture -- ../scripts/fixtures/smoke.parquet`.
//!
//! Not a benchmark and not part of the shipped app: it only writes a file.
//! `tags` is a LIST column on purpose - the grid shows it as DuckDB renders it
//! (`[1, 2]`), which only a real packaged run proves end to end.

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: make_parquet_fixture <out.parquet>");
    let conn = duckdb::Connection::open_in_memory().expect("in-memory DuckDB");
    conn.execute_batch(&format!(
        "COPY (SELECT * FROM (VALUES (1, 'alice', [1, 2]), (2, 'bravo', [3]), (3, 'carol', [])) t(id, name, tags))
         TO '{}' (FORMAT parquet)",
        out.replace('\'', "''")
    ))
    .expect("writing the fixture");
}
