# dblitz

Blazingly fast, read-only SQLite browser built with Tauri, SvelteKit, TypeScript, and Rust.

`dblitz` is designed for quickly inspecting large SQLite databases without modifying them. It opens databases through SQLite's read-only and immutable connection modes, so browsing and SELECT queries do not create `-wal` or `-shm` sidecar files next to the database.

## Features

- Open `.db`, `.sqlite`, `.sqlite3`, and `.db3` files.
- Browse tables with a virtualized data grid for large result sets.
- Inspect tables, columns, primary keys, defaults, and raw schema SQL.
- Run read-only SQL queries with CodeMirror syntax highlighting and schema autocomplete.
- Filter per column or across all columns, including comparison operators, semicolon OR syntax, and optional regex mode.
- Sort, resize, reorder, hide, and color columns; settings are persisted per database.
- Find columns quickly with Ctrl+F in the Browse Data view.
- Pin table filters so they are restored when a database is reopened.
- Select cells, copy tab-separated values, and export selected data to `.xlsx`.
- Keep recent databases, SQL history, theme, and per-file window markers.
- On Windows, register SQLite file associations and integrate with recent documents / jump lists.

## Install

Download packaged builds from the GitHub releases page:

https://github.com/tstone-1/dblitz/releases

Available package formats depend on the release build, but Tauri can produce Windows installers, macOS app bundles / disk images, and Linux packages.

## Usage

1. Start `dblitz`.
2. Click **Open DB** and choose a SQLite database.
3. Use **Structure** to inspect schema, **Browse Data** to page through tables, or **Execute SQL** to run SELECT queries.

The SQL editor is intentionally read-only. Write statements such as `INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE`, and `ALTER` are rejected.

## Development

Prerequisites:

- Node.js 18+
- Rust stable via `rustup`
- Platform-specific Tauri prerequisites
- Windows builds require Visual Studio Build Tools with the Desktop development with C++ workload

Install dependencies:

```bash
npm install
```

Run the desktop app in development mode:

```bash
npm run tauri dev
```

Build frontend assets:

```bash
npm run build
```

Build the Tauri app:

```bash
npx tauri build
```

## Checks

```bash
npm run check
cd src-tauri && cargo fmt --check
cd src-tauri && cargo test
cd src-tauri && cargo clippy
```

For dependency and vulnerability checks:

```bash
npm audit
cd src-tauri && cargo audit
```

## Project Layout

```text
src/                  SvelteKit frontend
src/lib/components/   Toolbar, grid, schema, SQL editor, and table controls
src/lib/store.svelte.ts
                      Shared frontend state and Tauri command wrappers
src-tauri/            Tauri application and Rust backend
src-tauri/src/db.rs   SQLite access, filtering, querying, xlsx export
src-tauri/src/config.rs
                      Recent files and per-database view settings
BUILD.md             Release and build checklist
CHANGELOG.md         Version history
```

## User Data

`dblitz` stores view preferences under the OS config directory in a `dblitz` folder. This includes recent files, per-database column settings, pinned filters, theme, and optional window marker labels / colors.

SQL query history is stored in browser local storage inside the Tauri WebView.

## Versioning

Versions use CalVer in `YY.M.MICRO` format. For example, `26.4.9` is a 2026 April release.

When releasing, update the version in:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Also update `CHANGELOG.md`. See `BUILD.md` for the release checklist.

## License

`package.json` declares this project as MIT licensed.
