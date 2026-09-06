# dblitz

[![Checks][checks-badge]][checks-url]

A fast, read-only SQLite browser. Built with Tauri, Svelte, and Rust.

## Why dblitz?

If you've used **[DB Browser for SQLite](https://sqlitebrowser.org/)** to take
a quick look at a database file and wished for a tool that's strictly read-only,
persists every view setting per file, and is engineered for large tables —
that's dblitz.

It's a single-purpose viewer: **SQLite only, read-only only.** If you need to
edit data, DB Browser for SQLite is excellent. If you need to talk to many
different database engines, DBeaver covers you. dblitz is for inspecting SQLite
files quickly and safely.

### Read-only by design

Four layers enforce it, and each one would be enough on its own:

1. Connections open with `SQLITE_OPEN_READ_ONLY`.
2. The URI carries `?immutable=1`, so SQLite treats the file as a frozen
   snapshot — no journal, no `-wal`, no `-shm` files ever appear next to the
   database.
3. The SQL editor rejects any statement SQLite considers non-read-only
   (`INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE`, `ALTER`, …) with a clear
   message, and a SQLite authorizer denies `ATTACH`, `DETACH` and transactions
   at the engine level.
4. Only read-only introspection pragmas are accepted at all, so something like
   `PRAGMA journal_mode=wal` is refused before it reaches the engine.

`SECURITY.md` treats a way through **any** of the four as a real finding.

The `immutable` flag is a promise you make to SQLite, not a lock it enforces: it
tells SQLite the file won't change while open, so SQLite skips locking and never
checks for a `-wal` file. If another process writes to the database while
dblitz has it open, dblitz won't see committed-but-uncheckpointed WAL rows, may
show stale data, and can hit a read error mid-session. dblitz does not
memory-map the file, so a file that shrinks underneath it — a cloud sync, another
tool's `VACUUM` — surfaces as "database disk image is malformed" rather than
killing the process. Close the writing application before opening its database
in dblitz, or point dblitz at a copy.

### Built for large tables

The data grid is virtualized; rows are loaded in 500-row chunks. For the common
case — unfiltered, unsorted, page-aligned browsing — dblitz builds a sparse rowid
index on first access and pages with `WHERE rowid >= ? AND rowid < ?` instead of
`LIMIT … OFFSET …`, so jumping to a far page is a seek, not a scan. Filtered or
sorted views materialize the matching rowids once, in display order, and page by
rowid lookup; `WITHOUT ROWID` tables fall back to LIMIT/OFFSET. Switching tables
or changing filters cancels in-flight queries instead of queuing stale work.

### Benchmark snapshot

The rowid seek path is intended for deep paging in large, unfiltered tables.
This release-mode synthetic benchmark creates 1,000,000 rows and reads the row
values for each fetched page:

```bash
cd src-tauri
cargo run --release --example rowid_seek_benchmark -- 1000000 500 5
```

The example calls the same functions the app calls, over the same read-only
`?immutable=1` connection the app opens, rather than re-implementing the paging
it measures. Measured 2026-09-06 at `71f5d67` on macOS 26.6.2, Apple M5,
rustc 1.98.0, rusqlite 0.40.2 / libsqlite3-sys 0.38.2 (SQLite 3.53.2).

The DB Browser-equivalent column emulates the DB Browser for SQLite Browse Data
lazy fetch shape from upstream commit `6cba47ef`: `RowLoader::process()` appends
a `LIMIT/OFFSET` query, and the default prefetch window is 50,000 rows
([source][db4s-rowloader], [setting][db4s-prefetch]). This is a source-based
data-fetch comparison, not a GUI rendering benchmark.

Median of five reads per target row. The LIMIT/OFFSET and dblitz rowid columns
fetch 500 rows; the DB Browser-equivalent column fetches its 50,000-row prefetch
window, except at table edges.

| Target row | LIMIT/OFFSET 500 | DB Browser-equivalent 50k | dblitz rowid 500 |
|------------|------------------|---------------------------|------------------|
| 0 | 0.14 ms | 6.38 ms | 0.13 ms |
| 250,000 | 1.04 ms | 13.39 ms | 0.13 ms |
| 500,000 | 1.97 ms | 14.40 ms | 0.13 ms |
| 750,000 | 11.37 ms | 24.09 ms | 0.13 ms |
| 999,500 | 15.03 ms | 20.77 ms | 0.13 ms |

The one-time sparse rowid index build for that table measured 29.44 ms.

A second example measures scrolling a *filtered* view, where the matching rowids
are materialized once and every page is a lookup into that set:

```bash
cd src-tauri
cargo run --release --example filtered_scroll_benchmark
```

Over 500,000 rows with 250,000 matching and a 200-row page, building the ordered
set takes 37.30 ms and each page then costs 0.10-0.11 ms, against 40-69 ms per
page when every page re-ran the scan: 37.80 ms total against 297.30 ms.

### Remembers what you set

For every database you open, dblitz persists:

- sort column + direction
- column widths, order, hidden columns, per-column colors
- per-column pinned filters and the global filter

These live in one JSON file per database under the OS config directory. The
filename is a SHA-256 prefix of the database's absolute path, so the directory
listing never leaks file paths. Open the same database tomorrow and the view is
exactly where you left it.

### Multi-window workflows

When several databases are open at once, you can tint each window's toolbar one
of six preset colors and attach a short text label. Both render in the toolbar
and in the recent-files dropdown, so PROD and QA stay visually distinct.

## Features

**Browsing**
- Open `.db`, `.sqlite`, `.sqlite3`, `.db3` — plus an **All Files** filter in the
  Open dialog, because a SQLite database is identified by its header, not its
  name (`data.bin` and `cache.dat` open fine)
- Virtualized data grid; rowid-indexed seek paging for unfiltered scrolling in
  large tables
- Sort by any column
- Resize, reorder, hide, and color-tag columns
- REAL values are displayed exactly as SQLite's own text conversion prints them
  (`3.0`, not `3`), so a filter typed against what you see matches what is stored
- Find columns by name with **Ctrl+F** (Cmd+F on macOS) in the Browse Data
  view
- In-flight queries are cancelled when you switch tables or change filters

**Filtering**
- Per-column filters with comparison operators: `>`, `>=`, `<`, `<=`, `=`, and
  `<>` (NOT LIKE, or non-empty when used bare)
- Multiple criteria with `;` — e.g. `foo;bar` matches either, `foo;<>bar`
  matches `foo` but not `bar`
- Cross-column global filter (matches any column)
- Optional regex mode per column. A NULL cell matches as the empty string, so
  `^$` finds blanks; a BLOB never matches; a number matches the text the grid
  shows for it; and a pattern aimed at a column that does not exist is reported
  as an error rather than quietly matching every row
- Pin filters per table so they're restored when you reopen the database

**Schema and SQL**
- Browse tables, columns, primary keys, defaults, and raw `CREATE` SQL for every
  object in `sqlite_master` (tables, indexes, views, triggers)
- SELECT-only SQL editor with CodeMirror syntax highlighting and schema-aware
  autocomplete (table and column names)
- Results capped at 50,000 rows with a clear truncation message
- SQL history persisted in the WebView's local storage

**Export and copy**
- Select cells in the grid and copy. The clipboard carries both an HTML table
  and RFC 4180-quoted tab-separated text, so a cell holding a tab or a line break
  pastes as one cell instead of splitting into extra columns or rows
- Export the current selection to `.xlsx` using SQLite type-affinity rules;
  integers larger than 2⁵³ are written as strings to preserve precision
- "Open in Excel" writes the workbook to your OS temp folder by default; pick a
  different destination under **Settings → Excel Export Folder** (it falls back
  to temp if that folder later goes missing)

**Recent files**
- Up to 10 most-recently-opened databases, each enriched with its tint and label
- Dead paths are filtered out of the dropdown but kept in storage, so a
  temporarily-unmounted drive doesn't wipe your history

**Windows extras**
- Registers as a handler for `.db` / `.sqlite` / `.sqlite3` / `.db3`
- Adds opened files to the Windows recent-documents list (jump list)
- Double-clicking a file that's already open in another instance activates the
  existing window instead of launching a duplicate, whatever the path spelling

**macOS extras**
- Double-clicking an associated database file opens it, whether dblitz is
  already running or not (since 26.7.7)
- Opened files are added to the Dock icon's **Open Recent** menu

## Keyboard shortcuts

Cmd on macOS, Ctrl elsewhere; dblitz labels them for the platform you are on.

| Shortcut | What it does |
|----------|--------------|
| Ctrl+Enter | Run the statement in the SQL editor |
| Ctrl+F | Open the column finder (Browse Data) |
| Ctrl+A | Select every cell in the grid (not while typing in a field) |
| Ctrl+C | Copy the selection |
| Enter or Space | Sort by the focused column header |
| Ctrl+Click | Add another selection rectangle, or switch a single cell back off |
| Shift+Click | Extend the current selection rectangle |
| Shift+Click on **Reset** | Also clear the pinned filter defaults |

In the column finder: **Escape** closes it, **Up**/**Down** move through the
matches and wrap around, **Home**/**End** jump to the first and last, and
**Enter** scrolls the chosen column into view.

## Install

Download packaged builds from the GitHub releases page:

https://github.com/tstone-1/dblitz/releases

Each release ships:

- **Windows** — NSIS installer (`*-setup.exe`), MSI installer (`*.msi`), and a portable `dblitz.exe`
- **macOS** — `.dmg` and `.app.tar.gz` for both Intel (`x64`) and Apple Silicon
  (`aarch64`)
- **Linux** — `.deb`, `.rpm`, and `.AppImage` (all x86_64)

**macOS** builds are signed with a Developer ID identity and notarized by Apple
(since 26.7.6), so the `.dmg` opens and the app launches normally — no
Gatekeeper bypass, and no more "dblitz is damaged and can't be opened". You can
also install from the [Homebrew tap](https://github.com/tstone-1/homebrew-dblitz):

```sh
brew install --cask tstone-1/dblitz/dblitz
```

**Windows** builds are unsigned. SmartScreen warns on first launch — choose
*More info* → *Run anyway*.

## Updates

dblitz updates itself. About ten seconds after launch it checks the GitHub
releases page; when a newer version exists, a bar appears offering **Install and
restart**. The download is signed and its signature verified before anything is
installed. A check that fails — no network, GitHub unreachable — is silent by
design; only checks you asked for report back.

Under **Settings → Updates** you get the running version, a **Check for updates**
button, and a **Check at startup** switch to turn the automatic check off. The
manual button works regardless of that setting.

Two install methods can't self-update, and dblitz will say so instead of offering
a broken button:

- **The portable `dblitz.exe`** — there is no installer to hand off to. Download
  the new exe and replace it.
- **Linux `.deb` / `.rpm`** — those files are owned by your package manager. Only
  the `.AppImage` can replace itself. Use your package manager, or switch to the
  AppImage.

On macOS the Homebrew cask is marked `auto_updates true`, so `brew` knows dblitz
manages its own version and won't report the cask as permanently outdated after
an in-app update.

## Usage

1. Start `dblitz`.
2. Click **Open DB** and choose a SQLite database — or, on Windows,
   double-click any `.db` / `.sqlite` / `.sqlite3` / `.db3` file.
3. Use **Structure** to inspect schema, **Browse Data** to page through tables,
   or **Execute SQL** to run SELECT queries.

## User Data

dblitz stores its config under the OS-standard config directory in a `dblitz` folder:

| OS | Path |
|----|------|
| macOS | `~/Library/Application Support/dblitz/` |
| Linux | `~/.config/dblitz/` (or `$XDG_CONFIG_HOME/dblitz/`) |
| Windows | `%APPDATA%\dblitz\` |

Inside:

- `app.json` — the recent-files list (capped at 10), the Excel-export folder, the
  "check for updates at startup" preference, and the version that last ran (used
  to show the one-time "dblitz was updated" confirmation)
- `<sha256-prefix>.json` — one file per database, holding the per-database view
  config (sort, widths, order, hidden columns, colors, pinned filters, tint,
  label). The filename is a 16-character prefix of `SHA-256(absolute_path)`.

SQL query history is stored in the WebView's `localStorage` under the key
`dblitz-sql-history`.

## Development

See [BUILD.md](BUILD.md) for prerequisites, dev workflow, code-quality checks,
build commands, and the release procedure. The Architecture section of
[AGENTS.md](AGENTS.md) describes how the backend is laid out — the `db`
submodules, the rowid paging and caching rules, and the four read-only layers.

## Contributing

Issues and pull requests are welcome; this is a public repository under the MIT
licence. Before opening a PR, run `npm run quality` — it is the whole gate CI
runs (frontend type-check, unit tests, build, then `cargo fmt --check`,
`cargo test` and `cargo clippy --all-targets --all-features -- -D warnings`).
Security problems go through the private channel in [SECURITY.md](SECURITY.md),
not a public issue.

## License

MIT — see [LICENSE](LICENSE).

[checks-badge]: https://github.com/tstone-1/dblitz/actions/workflows/checks.yml/badge.svg
[checks-url]: https://github.com/tstone-1/dblitz/actions/workflows/checks.yml
[db4s-prefetch]: https://github.com/sqlitebrowser/sqlitebrowser/blob/6cba47ef/src/Settings.cpp#L150-L152
[db4s-rowloader]: https://github.com/sqlitebrowser/sqlitebrowser/blob/6cba47ef/src/RowLoader.cpp#L204-L229
