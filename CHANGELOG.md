# Changelog

All notable changes to dblitz will be documented in this file.

Versioning follows [CalVer](https://calver.org/) using `YY.M.MICRO` format
(e.g., `26.4.0` = first April 2026 release).

## [26.4.1] - 2026-04-10

### Added
- **Pinned (persistent) filters** — opt-in persistence for column and global filters. Click the pin button next to any filter input to save its current value as the per-table default; restored automatically on every reopen. Three visual states (none / pinned / modified) communicate at a glance whether the live filter matches the saved version. Right-click any pin for a context menu (Pin / Re-pin / Unpin / Revert / Clear).
- **Header pin glyph** — columns with a saved filter show a small inert pin icon next to the column name, so persistent filters are visible at a glance even without scrolling to the filter row.
- **Reset filters toolbar button** — discards ephemeral filter edits and restores pinned defaults. Shift+click also wipes the saved defaults (destructive).
- **Keyboard-accessible column sort** — column headers are now in the tab order; press Enter or Space to toggle sort. `aria-sort` announces direction to screen readers. ARIA grid roles added throughout the data grid.

### Changed
- **Single-table databases now jump straight to the Browse tab** when opened from the toolbar "Open DB" button (previously only the CLI/jump-list/file-association paths did this). Multi-table databases still leave the active tab alone so the user can inspect Structure first.
- **Auto-select effect now re-fires for each new database**, not just the first one. Closing a single-table DB and opening another single-table DB now correctly auto-selects the second one's table (previously a stale flag prevented re-selection).
- **Shared context menu primitives** (`.ctx-backdrop`, `.ctx-menu`, `.ctx-item`, `.ctx-sep`) promoted to global `app.css`. Removed three near-identical local copies.

### Fixed
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
