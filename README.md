# dblitz

A fast, read-only SQLite browser. Built with Tauri, Svelte, and Rust.

## Why dblitz?

If you've used **[DB Browser for SQLite](https://sqlitebrowser.org/)** to take a quick look at a database file and wished for a tool that's strictly read-only, persists every view setting per file, and is engineered for large tables — that's dblitz.

It's a single-purpose viewer: **SQLite only, read-only only.** If you need to edit data, DB Browser for SQLite is excellent. If you need to talk to many different database engines, DBeaver covers you. dblitz is for inspecting SQLite files quickly and safely.

### Read-only by design

dblitz opens databases with `SQLITE_OPEN_READ_ONLY` *and* the `?immutable=1` URI flag. The connection cannot write, and the immutable flag tells SQLite the file is a frozen snapshot — so no journal, no `-wal`, no `-shm` files ever appear next to the database. The SQL editor additionally rejects any statement SQLite considers non-read-only (`INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE`, `ALTER`, …) with a clear message.

### Built for large tables

The data grid is virtualized; rows are loaded in 500-row chunks. For the common case — unfiltered, unsorted, page-aligned browsing — dblitz builds a sparse rowid index on first access and pages with `WHERE rowid >= ? AND rowid < ?` instead of `LIMIT … OFFSET …`, so jumping to a far page is a seek, not a scan. Filtered or sorted queries fall back to LIMIT/OFFSET. Switching tables or changing filters cancels in-flight queries instead of queuing stale work.

### Remembers what you set

For every database you open, dblitz persists:

- sort column + direction
- column widths, order, hidden columns, per-column colors
- per-column pinned filters and the global filter
- last selected table

These live in one JSON file per database under the OS config directory. The filename is a SHA-256 prefix of the database's absolute path, so the directory listing never leaks file paths. Open the same database tomorrow and the view is exactly where you left it.

### Multi-window workflows

When several databases are open at once, you can tint each window's toolbar one of six preset colors and attach a short text label. Both render in the toolbar and in the recent-files dropdown, so PROD and QA stay visually distinct.

## Features

**Browsing**
- Open `.db`, `.sqlite`, `.sqlite3`, `.db3`
- Virtualized data grid; rowid-indexed seek paging for unfiltered scrolling in large tables
- Sort by any column
- Resize, reorder, hide, and color-tag columns
- Find columns by name with **Ctrl+F** (Cmd+F on macOS) in the Browse Data view
- In-flight queries are cancelled when you switch tables or change filters

**Filtering**
- Per-column filters with comparison operators: `>`, `>=`, `<`, `<=`, `=`, and `<>` (NOT LIKE, or non-empty when used bare)
- Multiple criteria with `;` — e.g. `foo;bar` matches either, `foo;<>bar` matches `foo` but not `bar`
- Cross-column global filter (matches any column)
- Optional regex mode per column
- Pin filters per table so they're restored when you reopen the database

**Schema and SQL**
- Browse tables, columns, primary keys, defaults, and raw `CREATE` SQL for every object in `sqlite_master` (tables, indexes, views, triggers)
- SELECT-only SQL editor with CodeMirror syntax highlighting and schema-aware autocomplete (table and column names)
- Results capped at 10,000 rows with a clear truncation message
- SQL history persisted in the WebView's local storage

**Export and copy**
- Select cells in the grid and copy as tab-separated values (paste-ready into Excel or Google Sheets)
- Export the current selection to `.xlsx` using SQLite type-affinity rules; integers larger than 2⁵³ are written as strings to preserve precision

**Recent files**
- Up to 10 most-recently-opened databases, each enriched with its tint and label
- Dead paths are filtered out of the dropdown but kept in storage, so a temporarily-unmounted drive doesn't wipe your history

**Windows extras**
- Registers as a handler for `.db` / `.sqlite` / `.sqlite3` / `.db3`
- Adds opened files to the Windows recent-documents list (jump list)
- Double-clicking a file that's already open in another instance activates the existing window instead of launching a duplicate

## Install

Download packaged builds from the GitHub releases page:

https://github.com/tstone-1/dblitz/releases

Each release ships:

- **Windows** — NSIS installer (`*-setup.exe`), MSI installer (`*.msi`), and a portable `dblitz.exe`
- **macOS** — `.dmg` and `.app.tar.gz` for both Intel (`x64`) and Apple Silicon (`aarch64`)
- **Linux** — `.deb`, `.rpm`, and `.AppImage` (all x86_64)

dblitz is unsigned. macOS users may need to clear the quarantine attribute or right-click → Open the first time; the [Homebrew tap](https://github.com/tstone-1/homebrew-dblitz) handles this automatically.

## Usage

1. Start `dblitz`.
2. Click **Open DB** and choose a SQLite database — or, on Windows, double-click any `.db` / `.sqlite` / `.sqlite3` / `.db3` file.
3. Use **Structure** to inspect schema, **Browse Data** to page through tables, or **Execute SQL** to run SELECT queries.

## User Data

dblitz stores its config under the OS-standard config directory in a `dblitz` folder:

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/dblitz/` |
| Linux | `~/.config/dblitz/` (or `$XDG_CONFIG_HOME/dblitz/`) |
| Windows | `%APPDATA%\dblitz\` |

Inside:

- `app.json` — the recent-files list (capped at 10)
- `<sha256-prefix>.json` — one file per database, holding the per-database view config (sort, widths, order, hidden columns, colors, pinned filters, tint, label, last selected table). The filename is a 16-character prefix of `SHA-256(absolute_path)`.

SQL query history is stored in the WebView's `localStorage` under the key `dblitz-sql-history`.

## Development

See [BUILD.md](BUILD.md) for prerequisites, dev workflow, code-quality checks, build commands, and the release procedure.

## License

MIT — see `package.json`.
