//! Writes the Parquet fixture `scripts/smoke-test.mjs` opens in the packaged
//! app. Node has no Parquet writer, so the smoke script runs this through
//! `cargo run --example make_parquet_fixture -- <out.parquet>`.
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
