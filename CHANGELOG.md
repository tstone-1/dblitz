# Changelog

All notable changes to dblitz will be documented in this file.

Versioning follows [CalVer](https://calver.org/) using `YY.M.MICRO` format
(e.g., `26.4.0` = first April 2026 release).

## [26.4.7] - 2026-04-16

### Added
- **Auto-fit column widths**: columns now auto-size to fit their content (header + first 100 data rows) when a table is opened for the first time (no saved widths). Uses `canvas.measureText()` for fast, reflow-free measurement. Widths are clamped to 60–400 px and persisted to the per-file config.
- **"Auto-fit column widths" in header context menu**: right-click any column header to recompute all column widths from current data, resetting any manual sizing.
- **Recent-files dropdown shows window markers**: each entry renders its file's tint as a colored left border and its label as an inline pill, so PROD/QA distinctions are visible before the file is opened.

### Changed
- **Filter tooltip documents OR syntax**: the `.*` regex-toggle button now explains both paths — `foo;bar` for OR in text mode, `foo|bar` for alternation in regex mode — so the multi-value search feature is discoverable without docs.
- **Tighter left padding in grid cells and headers**: left padding reduced from 8px to 4px (right padding unchanged) so cell content sits closer to the column border.

### Fixed
- **Cell selection fill renders solid across rows**: moved the row separator from the row element's `border-bottom` onto each cell as an `inset box-shadow`, so the blue selection fill is no longer cut by 1px grey stripes between rows and the dark outer selection border stays continuous down the left/right edges. Selected cells adjacent to non-selected cells now preserve a continuous fill-colored bottom line, eliminating the 1px notch at selection boundaries.

## [26.4.6] - 2026-04-14

### Added
- **Per-file window marker** (Settings → Window Marker): pick one of seven toolbar tint colors and enter a short label (e.g. `PROD`, `QA`) to visually distinguish windows when opening same-named files from different directories in parallel. Both are persisted per-file alongside the rest of the view config and restored automatically on reopen.
- **Ctrl+C copies the current cell selection** to the clipboard as tab-separated values. Gated on having an active grid selection; defers to native copy when typing in inputs/SQL editor or when the user has a text selection.
- **Per-file column widths**: resizing a column now persists the width in the file's view config (per table, alongside column colors/order/hidden flags) and is restored on the next open.

### Fixed
- **Large text-stored numbers no longer shown in scientific notation** when using "Open in Excel". The xlsx exporter now respects the declared SQLite column type — `VARCHAR`/`TEXT`/`CLOB` columns stay as strings, so values like `123123123123` keep their original digits instead of being coerced to `f64`. Numeric columns (`INTEGER`, `REAL`, `NUMERIC`, ...) still export as numbers.

## [26.4.5] - 2026-04-13

### Fixed
- **Ctrl+A in column filter inputs now selects text** instead of selecting all grid cells. The grid's Ctrl+A handler no longer intercepts the shortcut when focus is inside a filter input or textarea.
- **Filter input responsiveness improved**: increased debounce delay from 300ms to 500ms so rapid edits (e.g. holding Backspace) don't trigger expensive intermediate queries. The query now fires once after the user stops typing.

## [26.4.4] - 2026-04-13

### Added
- **Ctrl+A selects all cells** in the Browse Data grid. The browser's default select-all is intercepted so only the table body is selected, not the surrounding UI.
- **Shift+Click extends cell selection** from the current anchor to the clicked cell, matching standard spreadsheet behavior.
- **Selection statistics bar** appears at the bottom of the grid when multiple cells are selected, showing row/column count. When all non-empty values in the selection are numeric, Sum, Avg, Min, and Max are displayed. Capped at 100k rows for performance.
- **Multi-instance support**: each SQLite file opens in its own window. Duplicate detection uses a Win32 window property (path hash via `SetPropW`/`GetPropW`) so the same file is never opened twice, while different files with the same name in different directories open correctly in separate windows.

### Changed
- **Window title shows filename only** (e.g. `file.sqlite - dblitz v26.4.4`) for a cleaner taskbar and title bar. The full file path is now displayed in the toolbar instead, using a flex layout that expands to fill available space rather than truncating at a fixed 300px width.
- **Copy/export capped at 100k rows** to prevent runaway chunk fetches on large virtual-scroll tables when the selection exceeds loaded data.

## [26.4.3] - 2026-04-10

### Added
- **"Open DB" split button with recent-files dropdown**: a chevron next to the Open DB toolbar button drops down the most recently opened databases (capped at 10, deduped, most-recent first). Each entry shows the file name with the parent directory dimmed beneath it. Click any entry to reopen the database; a "Clear recent files" item at the bottom wipes the list. Recents are tracked at the backend in `app.json` next to the existing per-DB view configs and stay self-cleaning — entries pointing at files that no longer exist are filtered out at read time. The list is updated automatically on every successful `open_database` call (manual open, file association, jump-list, command-line argument), so the chevron is always in sync regardless of how the database was opened.

### Changed
- **New SQLite-themed app icon** replaces the default Tauri rocket. Source SVG (`src-tauri/icons/sqlite.svg`, MIT-licensed, by [vscode-icons](https://github.com/vscode-icons/vscode-icons)) is committed to the repo so the full icon set can be regenerated at any time with `npx tauri icon src-tauri/icons/sqlite.svg`. All platform variants (`.ico` multi-size for Windows, `.icns` for macOS, multiple `.png` sizes, plus iOS/Android/Microsoft Store assets) are produced by Tauri's built-in icon generator from that single source.
- **Window title now includes the open file name**: title is `<filename> - dblitz v<version>` while a database is open, falling back to plain `dblitz v<version>` when none is. Lets users distinguish multiple dblitz instances at a glance from the taskbar / Alt-Tab list when comparing several SQLite files side by side. The title is updated by the backend on every `open_database` / `close_database` call so file-association launches, jump-list opens, and the toolbar "Open DB" button all behave the same.

## [26.4.2] - 2026-04-10

### Added
- **Auto-select on Structure tab**: opening a single-table database now also auto-selects the lone table on the Structure tab (previously only Browse Data did this), so the user doesn't have to click the table to see its columns.
- **Read-only enforcement test coverage**: three new unit tests in `db.rs` covering the friendly write-rejection message, that SELECT still works on the read-only connection, and that `path_to_sqlite_uri` correctly percent-encodes spaces, `#`, `?`, and `%` while normalizing Windows backslashes.

### Changed
- **`pinnedFilters.svelte.ts` extracted from `BrowseData.svelte`**: the pinned-filter state machine (12 functions, two derived stores, the global pin context menu state) now lives in its own file mirroring the existing `cellSelection.svelte.ts` / `dragReorder.svelte.ts` extraction pattern. `BrowseData.svelte` shrinks by ~145 LOC.
- **`autoSelectFirstTable.svelte.ts` extracted as a shared helper**: the "track which db path was last auto-selected, fire `onSelect` when there's exactly one table and the path changed" bookkeeping is consolidated. Each consumer (BrowseData, DatabaseStructure) gets its own closure-scoped flag — no shared module state.
- **Verbose log field cleanup**: dropped the redundant `uri` field from the `open_database` error log (it's a deterministic function of `path` and the OneDrive paths can be sensitive).

### Fixed
- **Filter row jittered 1px while scrolling**: the per-column filter row used `position: sticky` with a hardcoded `top: 26px` matching the `HEADER_HEIGHT` constant, but the header's actual rendered height drifted by a pixel under Windows display scaling (125%/150%) due to subpixel rounding — so the filter row appeared to wiggle up/down by 1px on every scroll tick. Fixed by wrapping the header row and filter row in a single `.sticky-header` div pinned at `top: 0`, so they move as one unit and there's no cross-element offset to compute.

## [26.4.1] - 2026-04-10

### Added
- **Pinned (persistent) filters** — opt-in persistence for column and global filters. Click the pin button next to any filter input to save its current value as the per-table default; restored automatically on every reopen. Three visual states (none / pinned / modified) communicate at a glance whether the live filter matches the saved version. Right-click any pin for a context menu (Pin / Re-pin / Unpin / Revert / Clear).
- **Header pin glyph** — columns with a saved filter show a small inert pin icon next to the column name, so persistent filters are visible at a glance even without scrolling to the filter row.
- **Reset filters toolbar button** — discards ephemeral filter edits and restores pinned defaults. Shift+click also wipes the saved defaults (destructive).
- **Keyboard-accessible column sort** — column headers are now in the tab order; press Enter or Space to toggle sort. `aria-sort` announces direction to screen readers. ARIA grid roles added throughout the data grid.
- **SQL history rerun via double-click** — clicking a query in the SQL history loads it into the editor; double-clicking loads and executes it in one gesture.

### Changed
- **dblitz is now a strictly read-only viewer.** SQLite connections are opened with `SQLITE_OPEN_READ_ONLY` *plus* `?immutable=1` in the URI, so the engine treats the file as a frozen snapshot for the connection's lifetime. Write statements (INSERT/UPDATE/DELETE/DROP/CREATE/ALTER) are rejected at three layers: the SQL editor's placeholder advertises read-only, the Rust backend rejects non-readonly prepared statements with a friendly error, and SQLite refuses any write at the lowest layer. The `?immutable=1` flag also prevents SQLite from creating `-shm`/`-wal` companion files next to the database — important when the database lives in a OneDrive-synced folder where stray companion files become sync noise. Trade-off: dblitz won't see live writes from other processes; reopening the file is required to pick up changes. The `PRAGMA journal_mode=WAL` PRAGMA was dropped (writes the file header → fails on read-only); files already in WAL mode are still readable.
- **Single-table databases now jump straight to the Browse tab** when opened from the toolbar "Open DB" button (previously only the CLI/jump-list/file-association paths did this). Multi-table databases still leave the active tab alone so the user can inspect Structure first.
- **Auto-select effect now re-fires for each new database**, not just the first one. Closing a single-table DB and opening another single-table DB now correctly auto-selects the second one's table (previously a stale flag prevented re-selection).
- **SQL history panel no longer collapses on selection** — clicking a history entry loads the query but keeps the panel open so you can keep browsing or comparing.
- **Shared context menu primitives** (`.ctx-backdrop`, `.ctx-menu`, `.ctx-item`, `.ctx-sep`) promoted to global `app.css`. Removed three near-identical local copies.

### Fixed
- **Pinned filters race on first open**: opening a database with a saved column filter would leave the filter unapplied and the pin button stuck in "modified" state until the user manually re-pinned. Caused by `openDatabase` publishing `appState.tables` to the frontend before `appState.fileConfig` finished loading — the auto-select effect fired against an empty config, leaving `columnFilters` empty after hydration. Fixed by batching all `appState` assignments after the awaits, plus pre-populating `columns` from the autocomplete cache in `selectTable` so `buildFilters()` has schema information before the first query result returns.
- **Selected cell content shifted 1px right**: applying a left selection border to the selection rectangle's leftmost column ate into the content box (with `box-sizing: border-box`), nudging the cell text. Replaced the four `border-{top,right,bottom,left}` rules with a composed `box-shadow: inset` driven by per-edge CSS custom properties — inset shadows draw inside the padding without modifying the content box, so cell text stays put.
- **Schema-drift safety for filters**: filters that reference columns no longer present in the schema (e.g. a pinned filter on a column that was renamed externally) are silently dropped at query time instead of producing a `no such column` SQL error and an empty result set.
- **Back-compat migration for old config files**: configs written by 26.4.0 (without the `pinned_filters` / `pinned_global_filter` fields) now load cleanly. Migration runs once at `openDatabase` time instead of being scattered across `ensureTableConfig` calls.
- **Drag-reorder reactivity**: the `onReorderColumn` callback in `dragReorder.svelte.ts` now reads the live prop value via getter, so column reorders fire correctly even after the parent re-renders with a new callback reference.
- **12 svelte-check accessibility warnings** resolved (column header roles, ARIA grid structure, draggable list items in ColumnSettings, empty CSS ruleset in Toolbar, miscellaneous a11y annotations).

## [26.4.0] - 2026-04-09

### Added
- Blazingly fast SQLite browser with virtual-scroll data grid
- Database structure viewer (tables, columns, raw SQL schema)
- SQL editor with CodeMirror 6 syntax highlighting and autocomplete
- Per-column filters with multi-criteria syntax (contains, exact, comparison, regex)
- Global filter across all columns
- Column resize via drag on header borders
- Column visibility toggle and background color presets (per-database, persisted)
- Column reorder via drag-and-drop (settings panel and grid headers)
- Header right-click context menu for hide/color
- Sort by clicking column headers (persisted per-database)
- Cell selection with click-and-drag, copy to clipboard, export to Excel
- Rowid-indexed pagination for O(log n) seeks on large tables
- SQL query history (last 100 queries, persisted in localStorage)
- Light/dark theme toggle
- Windows file associations for .db, .sqlite, .sqlite3, .db3
- Windows jump list integration (recent/pinned files in taskbar)
- Single-instance support (second launch forwards file to running window)
- Per-database view config persistence (~/.config/dblitz/)
