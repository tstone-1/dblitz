# dblitz Agent Notes

## Project

- `dblitz` is a public `tstone-1` repository for a Tauri + SvelteKit + TypeScript desktop SQLite browser.
- Treat the repository as public: do not add internal company data, private paths, secrets, or proprietary examples.
- Use a public-safe Git identity for commits; do not commit with private or company email addresses.

## Development

- Install frontend dependencies with `npm install`.
- Run the desktop app in development with `npm run tauri dev`.
- Use these checks as appropriate:
  - `npm run check`
  - `npm run build`
  - `cd src-tauri && cargo fmt --check`
  - `cd src-tauri && cargo test`
  - `cd src-tauri && cargo clippy`
- Use `npx tauri build` for local release builds. macOS DMG packaging may need to run outside a sandbox because Tauri invokes system image mounting tools.

## Architecture

- Frontend code lives under `src/`.
- Tauri/Rust backend code lives under `src-tauri/`.
- SQLite backend code lives under `src-tauri/src/db/`, with `src-tauri/src/db.rs` as a thin facade that re-exports the submodules:
  - `schema.rs` — table/column introspection and row counts
  - `query.rs` — table paging, the rowid-index fast path, and regex filtering
  - `filters.rs` — the `WHERE` clause builder and column-filter operator parsing
  - `sql.rs` — arbitrary SQL execution plus the read-only / ATTACH-DETACH rejection gate
  - `export.rs` — XLSX export
  - `types.rs`, `util.rs` — shared DTOs and helpers (`safe_ident`, `read_row`, `StrErr`)
  - `benchmark.rs` — `cfg(debug_assertions)` paging benchmarks
- `dblitz` is intended to be a read-only SQLite viewer. Preserve the read-only behavior when changing query execution or database opening logic.

## Release

- Versions use CalVer `YY.M.MICRO`.
- Update all version files together:
  - `package.json`
  - `src-tauri/Cargo.toml`
  - `src-tauri/tauri.conf.json`
- Update `CHANGELOG.md` before release commits.
- See `BUILD.md` for the release checklist. Its shared-tools copy path is a placeholder unless the user provides a real deployment target.
