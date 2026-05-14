# dblitz

A fast, read-only SQLite browser for desktop. Inspired by [DB Browser for SQLite](https://sqlitebrowser.org/), rebuilt with a modern stack (Tauri 2 + Rust + SvelteKit) and a sharper focus on *viewing* — not editing — wide, complex SQLite snapshots.

## Why another SQLite browser?

DB Browser for SQLite is the standard, and the right tool when you need to edit schemas or rows. But for the much more common case of just looking at a SQLite file — a multi-hundred-column export, an analytics snapshot, a debug dump — its modal dialogs, write-mode default, and Qt-era UI add friction.

dblitz is the tool I wanted: opens fast, scrolls fast, makes wide tables navigable, and remembers per-file view state (column widths, filters, hidden columns, window tint) so a database always looks the same the next time you open it.

## Compared to DB Browser for SQLite

**Similarities**

- Browse Data / Database Structure / Execute SQL tabs
- Per-column filters, sorting, SQL editor with autocomplete
- File-association launching for `.db` / `.sqlite` / `.sqlite3` / `.db3`
- Cross-platform desktop app

**Differences**

| | dblitz | DB Browser for SQLite |
|---|---|---|
| Mode | **Read-only by design** (`SQLITE_OPEN_READ_ONLY` + `?immutable=1`) | Read-write |
| Stack | Tauri 2 + Rust + SvelteKit (~15 MB on Windows) | Qt + C++ |
| Wide tables | Ctrl+F column finder — type a fragment to locate any of hundreds of columns | Horizontal scroll only |
| Per-file view state | Column widths, colors, order, hidden flags, sort, pinned filters, window tint/label — all persisted alongside the file | Limited |
| Multi-window | Each file opens in its own window; opening the same file again focuses the existing window | Single window |
| Cell selection | Spreadsheet-style click+drag, Shift+click, Ctrl+A; selection stats bar with Sum / Avg / Min / Max for numeric selections | Limited |
| Excel export | xlsx aware of SQLite type affinities — TEXT stays text, large `i64` IDs preserved verbatim instead of being coerced to `f64` | CSV |
| Cloud-sync safety | `?immutable=1` avoids creating `-shm` / `-wal` companion files next to the file | Creates them |
| Large tables | Virtual-scroll grid; rowid-indexed pagination for O(log n) row seeks | Page-on-demand |

**When to use which**

- **dblitz**: read-only inspection, very wide tables, comparing multiple files side-by-side, files in cloud-sync folders (OneDrive / Dropbox / iCloud), workflows where filters and widths should persist per file.
- **DB Browser for SQLite**: editing data, designing schemas, importing CSV, broader format support.

## Features

- Virtual-scroll grid for million-row tables
- Per-column filters with contains / exact / comparison / regex modes; `;` for OR, `|` for regex alternation
- **Pinned filters** persist per table — distinguishes "I'm just trying this out" from "this is the default for this table"
- **Find Column** (Ctrl+F) palette for locating columns by name fragment; scrolls the grid to the match and unhides hidden columns
- Column reorder via drag, hide, color (12 presets), and per-column width — all per file
- Sort by clicking column headers
- Cell selection with copy-as-TSV (Ctrl+C); selection stats bar shows Sum / Avg / Min / Max for numeric selections
- xlsx export honoring SQLite column affinities
- SQL editor (CodeMirror 6) with autocomplete and history of the last 100 queries
- **Window markers** — per-file tint + label (e.g. `PROD` / `QA`) to visually distinguish multiple windows
- Recent files dropdown
- Windows: file associations + jump list integration

## Install

Pre-built binaries for Windows, macOS, and Linux are attached to each [release](https://github.com/tstone-1/dblitz/releases/latest).

### Windows

Download either `dblitz_<version>_x64-setup.exe` (installer, registers file associations) or `dblitz.exe` (portable). Run it.

### macOS

Download the `.dmg` matching your architecture: `aarch64` for Apple Silicon, `x64` for Intel. Drag the `.app` into `/Applications`.

> **First-launch quarantine workaround.** macOS Gatekeeper blocks unsigned downloaded apps with a misleading "damaged" message. Once, after copying to Applications, run:
>
> ```
> xattr -dr com.apple.quarantine /Applications/dblitz.app
> ```
>
> Then double-click normally. A Homebrew Cask is planned to make this automatic.

### Linux

Download the `.AppImage` (run directly after `chmod +x`), `.deb` (Debian / Ubuntu: `sudo dpkg -i dblitz_*.deb`), or `.rpm`.

## Build from source

See [BUILD.md](./BUILD.md).

## License

MIT — see [LICENSE](./LICENSE).
