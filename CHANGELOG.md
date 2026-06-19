# Changelog

All notable changes to dblitz will be documented in this file.

Versioning follows [CalVer](https://calver.org/) using `YY.M.MICRO` format
(e.g., `26.4.0` = first April 2026 release).

## [26.6.8] - 2026-06-19

### Fixed
- **Ctrl+C copies the selected cell again after a stray text selection on the toolbar.** Clicking the file-path text top-left and pressing Ctrl+A triggers the webview's document-wide "select all", which text-selects the toolbar (the tab labels, Unload, Settings). Because grid cells are `user-select: none`, clicking a cell afterwards did not collapse that selection, so the Ctrl+C handler saw a live text selection and deferred to native copy — copying the toolbar text instead of the cell value. Mousing down on a cell now clears any leftover document text selection, so the grid copy wins. This is a second leg of the Ctrl+C copy issue first addressed in 26.6.7.

## [26.6.7] - 2026-06-19

### Fixed
- **Ctrl+C now copies the selected cell(s) regardless of Caps Lock or Shift.** The grid's copy shortcut compared the pressed key against a lowercase `'c'`, but with Caps Lock on (or Shift held) the webview reports the key as `'C'`, so the keystroke was rejected and nothing was copied — while right-click → Copy kept working because it bypasses the keyboard handler. The Ctrl+C, Ctrl+A (select all cells), and Ctrl+F (find column) shortcuts now match their keys case-insensitively.

### Dependencies
- Refreshed dependencies to latest compatible versions: `@sveltejs/kit 2.65.2 → 2.66.0`, `@types/node 25.9.3 → 25.9.4`, and the `camino 1.2.2 → 1.2.3` cargo lockfile patch. `npm audit` clean; `cargo audit` clean aside from the known allowed gtk-rs Linux transitive advisories. (`@types/node 26.0.0` is held back — it tracks the Node runtime, which is on v24 here.)

## [26.6.6] - 2026-06-18

### Added
- **Configurable "Open in Excel" export folder**. Settings now has an *Excel Export Folder* control where you can pick the directory exported `.xlsx` workbooks are written to. Previously every export landed in the OS temp folder (`%LocalAppData%\Temp` on Windows); that remains the default, and exports fall back to it automatically if the chosen folder is later deleted, renamed, or on an unmounted drive. The choice is stored app-wide in `app.json`.

### Performance
- **Filtered, sorted views no longer lag while scrolling.** With an active column/global filter plus a sort, every scroll chunk used to re-run the whole `WHERE` + `ORDER BY` + `LIMIT/OFFSET` query — and because `OFFSET` grows as you scroll, deep pages got progressively slower (visible jank when fling-scrolling). The matching rowids are now materialized once in display order and cached per (filter+sort) signature, so each chunk becomes a rowid lookup. In a 500k-row benchmark (filter matching 250k, sorted on an unindexed column), per-page fetch dropped from ~55–98 ms to ~0.1 ms (roughly 500–900× per page) after a one-time ~57 ms build. The exact match count now comes from that cached list, so the frontend also stops firing a separate `count_rows` scan for filtered views. (`WITHOUT ROWID` tables fall back to the previous offset path.)
- **Read-only connection tuning** for faster reads on large databases. Since the file is opened as a frozen immutable snapshot, the connection now enables memory-mapped I/O (`PRAGMA mmap_size`, up to 1 GiB, capped at file size) to skip `read()` syscalls and the pager's double-buffer copy, and keeps sort/temp-b-tree scratch in RAM (`PRAGMA temp_store=MEMORY`) so a non-indexed `ORDER BY` over a filtered view never spills to a temp file. No new sidecar files are created.

### Dependencies
- Refreshed dependencies to latest compatible versions: Tauri stack `2.11.2 → 2.11.3` (plus `tauri-build`/`-codegen`/`-macros`/`-runtime`/`-utils` `2.6.2 → 2.6.3`), `tray-icon 0.23.1 → 0.24.1`, `bytes`, `syn`, `muda`, `getrandom`, `web_atoms`, and npm patch bumps (`@sveltejs/kit 2.65.2`, `@tauri-apps/api 2.11.1`, `@codemirror/search 6.7.1`, `vitest 4.1.9`). `npm audit` clean; `cargo audit` clean aside from the known allowed gtk-rs Linux transitive advisories.

## [26.6.5] - 2026-06-15

### Fixed
- **Rows no longer render with squished height on very large tables**. Once a table is big enough to trigger virtual-spacer compression (about 770k+ rows), the grid scaled each rendered row's vertical position down by the compression ratio, collapsing rows to a fraction of their height (e.g. ~11px instead of 26px). Rows in the visible window are now positioned at their true height, anchored to the current scroll offset, with no change to uncompressed (smaller) tables.

### Internal
- Added regression tests covering true row-height spacing inside a compressed spacer, scale=1 backward compatibility under arbitrary scroll offsets, and the last row's bottom edge landing exactly on the spacer height at maximum scroll.

### Dependencies
- Refreshed dependencies to latest compatible versions: cargo lockfile patch/minor bumps (`wasm-bindgen 0.2.125`, `time 0.3.49`, `brotli 8.0.4`, `js-sys`/`web-sys 0.3.102`, and others) plus npm patch/minor updates. `npm audit` and `cargo audit` are clean (only the known allowed Tauri/Linux GTK transitive advisories).

## [26.6.4] - 2026-06-11

### Fixed
- **Execute SQL now rejects comment-prefixed ATTACH/DETACH statements**. The read-only SQL gate strips leading SQL comments before checking the first executable token, closing a bypass that could attach or create other SQLite files.
- **Column filters now treat `%`, `_`, and `\` as literal text** by escaping LIKE patterns with an explicit SQLite escape character.
- **Rowid-index construction now observes query cancellation** so a superseded first browse of a huge table can stop instead of holding the connection until the full scan finishes.
- **Selection truncation messages now render as notices, not errors**. Copy/export warnings use a separate notice channel instead of the red error bar.

### Changed
- **DataGrid now uses an explicit static/virtual mode contract** and delegates Excel export/error/notice handling to its owners, keeping app-layer IPC out of the grid component.
- **Table view-config updates now go through one helper** so sorting, widths, visibility, colors, ordering, and pinned filters all republish config consistently.
- **CI now compiles and tests the Windows backend code path** with a `windows-latest` backend job.

### Internal
- Added regression tests for comment-prefixed ATTACH rejection, literal LIKE escaping, filter parameter ordering, range filters, row counting with filters, rowid-index cancellation, pinned-filter reset behavior, and XLSX duplicate-header deduping.
- Strengthened Rust/TypeScript drift checks by parsing the opposite side's source for tint presets and filter operators.
- Removed duplicated column ordering logic, raw virtual-row cache exposure, unused virtual-row helper exports, and dead XLSX export branches.
- Updated BUILD.md release-check commands and component-layout notes.

### Dependencies
- Bumped npm lockfile entries to latest compatible patch/minor versions (`@sveltejs/kit 2.65.0`, `@types/node 25.9.3`) and pinned Rust manifest patch versions for `rusqlite 0.40.1` and `tauri-plugin-opener 2.5.4`. `npm audit` is clean; `cargo audit` reports only the known allowed Tauri/Linux GTK/unic transitive advisories.

## [26.6.3] - 2026-06-10

### Fixed
- **UNC network-share database paths now open correctly**. SQLite URI generation now preserves an empty URI authority for paths like `\\server\share\db.sqlite` instead of producing a non-local `file://server/...` authority.
- **Very large Browse Data tables remain scrollable past Chromium's layout-height cap**. The grid compresses the virtual spacer once it would exceed a safe pixel height and maps scrollbar position back to row indices by ratio, so multi-million-row tables can still reach the bottom.
- **Hidden grids no longer race on Ctrl+C**. Window-level copy handling now checks that the grid is visible, preventing inactive mounted tabs from writing stale selections to the clipboard.
- **Background chunk/count failures now settle cleanly**. Chunk load errors still surface in the error bar without producing unhandled promise rejections, and failed async row counts clear the `counting...` state.
- **Sorting no longer bypasses the incomplete-filter gate**. Header clicks now avoid firing a query while a half-typed operator such as `>` is still incomplete.
- **Excel export handles duplicate result column names** by deduping table captions before handing them to `rust_xlsxwriter`.
- **Copy and Excel export now report selection truncation** when a selection exceeds the 100k materialization cap.

### Changed
- README now documents the current 50,000-row Execute SQL cap.

### Internal
- Removed dead read-only `rows_affected` plumbing and the unused persisted `selected_table` field.
- Consolidated the frontend column-filter value and recent-file boundary types.
- Added regression tests for UNC URI conversion, regex count rejection, large virtual-scroll mapping, hidden-grid copy gating, operator/tint preset sync, duplicate-header XLSX export, selection truncation, and background chunk errors.
- Corrected the `immutable=1` cache comment to describe the file-stability promise instead of a frozen snapshot.

### Dependencies
- Bumped npm dependencies to the latest compatible patch/minor versions (`@codemirror/view 6.43.1`, `@sveltejs/kit 2.64.0`) and refreshed Rust transitive lockfile entries (`bitflags 2.13.0`, `http 1.4.2`, `regex 1.12.4`, `uuid 1.23.3`, `wasm-bindgen 0.2.123`, and related crates). `npm audit` is clean; `cargo audit` reports only the known allowed Tauri/GTK transitive advisories.

## [26.6.2] - 2026-06-08

### Fixed
- **Sorting a large table and scrolling to the bottom no longer freezes the app**. Every browse chunk on a sorted column re-ran a full-table `ORDER BY` (plus a per-chunk `COUNT(*)`) against a non-indexed column, and none of it was cancellable — so fling-scrolling a large sorted table to the bottom queued dozens of full sorts that serialized on the connection lock and hung the UI. A sorted, unfiltered table now materializes its rowid order **once** per sort key (cached, cancellable mid-build, cleared on db open/close), and each chunk becomes a rowid lookup instead of a fresh sort. `WITHOUT ROWID` tables fall back to the previous `ORDER BY` + `OFFSET` path; the rowid-`IN` page fetch is batched to stay under SQLite's bound-parameter limit.

### Dependencies
- Bumped Rust `rusqlite 0.40.0 → 0.40.1` (also `libsqlite3-sys 0.38.0 → 0.38.1`, `hashlink 0.11.0 → 0.12.0`) and npm `@sveltejs/kit 2.63.0 → 2.63.1`, `svelte 5.56.2 → 5.56.3`, `@types/node 25.9.1 → 25.9.2`. All minor/patch — no API churn. `npm audit` and `cargo audit` clean (only the known Linux-only gtk-rs unmaintained advisories remain).

## [26.6.1] - 2026-06-05

### Added
- **Column colors carry into Execute SQL results**: colors set on a table in Browse Data now apply to matching result columns when a query's primary `FROM` table is that table. Resolution is name-based and limited to the FROM table (the new `primaryTableFromSql` scanner skips subquery/CTE-body FROMs, quoted/bracketed identifiers, and comments/string literals); joins, aliases, and expressions stay uncolored.

### Changed
- **Execute SQL row cap raised 10,000 → 50,000**: a `LIMIT` larger than the old cap no longer silently stopped at 10k rows. The cap stays below `query_table`'s 100k ceiling because Execute SQL materializes the whole result in one IPC round-trip rather than paging; the truncation banner now suggests narrowing the query or paging with `OFFSET` instead of "add a LIMIT clause".

### Internal
- **`primaryTableFromSql` / `resolveResultColumnColors`** extracted to `src/lib/components/sqlTable.ts` as pure, dependency-free helpers with full Vitest coverage (FROM detection across quoting/comments/subqueries/CTEs, case-insensitive table resolution, name-based color mapping).

### Dependencies
- Bumped Rust `chrono 0.4.44 → 0.4.45` and `serde_with 3.20.0 → 3.21.0` (with `serde_with_macros`); bumped npm `svelte 5.56.1 → 5.56.2`. All minor/patch — no API churn. `npm audit` and `cargo audit` clean (only the known Linux-only gtk-rs unmaintained advisories remain).

## [26.6.0] - 2026-06-04

### Changed
- **Filter syntax help** now notes that empty (NULL) cells are excluded by any active column filter, matching the actual query behavior.

### Internal
- **Defensive upper bound on table paging**: `query_table` now rejects a `limit` above 100,000 rows. The UI only ever requests a fixed chunk size, so this is insurance against an out-of-range caller materializing an unreasonable result set in one go.
- **xlsx export cell-typing is now unit-tested**: the number-vs-text decision was extracted into a pure `classify_cell` helper, with tests covering numeric-affinity classification, the `i64`/`f64` paths, the ±2^53 precision boundary (large IDs emitted as strings), and the empty-data / row-wider-than-headers error paths. `export.rs` previously had no tests.
- **Column-filter operators consolidated**: the frontend derives its incomplete-operator check from a single `OPERAND_REQUIRED_OPS` constant, with cross-references between `BrowseData.svelte` and `db/filters.rs` so the operator set stays in sync across the IPC boundary.
- **`AGENTS.md` architecture section** rewritten to describe the `db/` module layout; it still described the pre-split `db.rs`.

### Dependencies
- Bumped `rusqlite 0.39 → 0.40` (also `libsqlite3-sys 0.37 → 0.38`) and `rust_xlsxwriter 0.94 → 0.95`. All 45 Rust unit tests pass; no API churn in the surface we use.
- Bumped npm deps to latest minor/patch: `svelte 5.56.1`, `@sveltejs/kit 2.63.0`, `vite 8.0.16`, `vitest 4.1.8`, `svelte-check 4.6.0`, `@codemirror/autocomplete 6.20.3`. `npm audit` and `cargo audit` both clean.

## [26.5.4] - 2026-05-25

### Fixed
- **Release workflow restores Intel macOS and Linux artifacts**. `v26.5.3` shipped only Windows + macOS arm64 because `main`'s `release.yml` matrix was missing `x86_64-apple-darwin` cross-compile and `ubuntu-latest`. Both are back, plus a `quality` gate that runs the full `npm run quality` before any builds start so a broken commit never produces partial release artifacts.

## [26.5.3] - 2026-05-25

> **Versioning note**: `v26.5.0`/`v26.5.1`/`v26.5.2` were cut on 2026-05-14 from a parallel implementation built on a separate machine; the tags exist on GitHub but the underlying commits were never reconciled with `main`'s history (`git merge-base main v26.5.2` found no common ancestor). `26.5.3` picks up where the visible release line left off and is built from `main`, which contains the long-running history through `26.4.9` plus subsequent refactor and hardening work. The Tauri bundle identifier (`com.tstone.dblitz`) is preserved from `v26.5.x` so existing macOS/Windows installs upgrade in place.

### Added
- **`LICENSE` file** committed to the repo (MIT). The README has claimed MIT licensing since the initial commit; the file itself was missing on `main`.
- **CI checks workflow** (`.github/workflows/checks.yml`) running frontend type-check + vitest and backend `cargo fmt --check` / `cargo clippy --all-targets --all-features -- -D warnings` / `cargo test` on every push and PR to `main`. Sister to the existing `release.yml` which only fires on tag push.
- **`quality` npm script** that runs the full local quality gate end-to-end: `npm run quality` covers frontend check + tests + build, then backend fmt + tests + clippy.

### Changed
- **`package.json` / `Cargo.toml` `description`** now reads "Read-only SQLite browser" instead of "Blazingly fast SQLite browser" — same honest framing the README rewrite settled on. Removes unsubstantiated marketing language from the npm/crate metadata.

### Security
- **ATTACH / DETACH explicitly rejected** in the SQL editor. SQLite's `stmt.readonly()` reports them as read-only because they don't touch the *current* database file — but they let a user reach a second database file through dblitz's read-only viewer surface. Both forms (`ATTACH …`, `ATTACH DATABASE …`, `DETACH …`, `DETACH DATABASE …`, case-insensitive) are blocked at the input boundary with a dedicated error message before `prepare()`.

### Internal
- Expanded `db/sql.rs` test module from 2 cases to 9. New coverage: ATTACH/DETACH rejection (across casing + form variants, plus an identifier-shadowing sanity check), write PRAGMA rejection (`journal_mode`), `CREATE TEMP TABLE` rejection, `BEGIN IMMEDIATE` rejection, multi-statement rejection (rusqlite's `prepare` refuses outright — stronger than "only first runs"), read-only PRAGMA acceptance (`table_info`), and a load-bearing assertion that opening + reading a database creates no `-wal` / `-shm` / `-journal` sidecars (backs the README's immutable promise with an actual test).
- README rewritten to lead with "Why dblitz?" positioning vs DB Browser for SQLite, design-rationale subsections (read-only by design, large tables, persistence, multi-window), feature regrouping by theme, and concrete OS-by-OS paths for the config directory. Stale Project Layout (predated the `db.rs` refactor) and Versioning sections moved to `BUILD.md`.
- `BUILD.md` Code Quality Commands block now includes `npm test`, `cargo test`, and `cargo fmt --check`.

## [26.4.9] - 2026-04-30

### Added
- **Find column (Ctrl+F)**: a search palette that locates any column by substring, intended for tables with hundreds of columns where horizontal scrolling alone is too slow. Type a fragment (e.g. `SLBU`) to filter the list, use Up/Down to navigate, Enter to scroll the grid to the matched header (centered) and pulse it briefly. Hidden columns appear in the results with a `hidden` badge and are unhidden automatically when located. Also reachable via the new **Find** button next to **Columns**.

### Fixed
- **Copy/export loads virtualized selection rows before serializing**: copying or opening a large selection in Excel now materializes unloaded chunks first instead of writing blank cells for rows that were not yet in the scroll cache.
- **Defensive persisted-state recovery**: corrupt SQL history in browser storage now falls back to an empty history instead of breaking startup, and unsupported theme values fall back to light mode.
- **Stale persisted sort columns are ignored and repaired** when a table schema changes, so a renamed/dropped column no longer breaks the first query after reopening a database.
- **Regex filters page through matches without retaining the full match set**, avoiding large intermediate allocations on broad regex searches.

### Internal
- Split the Rust database backend from a single `db.rs` into focused modules for schema, query, filters, SQL execution, export, benchmarking, shared types, and utilities.
- Removed unused Tauri shell plugin/global API surface; native access now stays on the module-based imports actually used by the app.
- Added targeted frontend unit tests for selection materialization and persisted localStorage recovery.
- Bumped npm devdeps to latest minor/patch: `svelte 5.55.5`, `@sveltejs/kit 2.58.0`, `vite 8.0.10`, `@codemirror/search 6.7.0`, `@codemirror/view 6.41.1`.
- Bumped `rusqlite 0.34 → 0.39` (also `libsqlite3-sys 0.32 → 0.37`). All 24 Rust unit tests pass; no API churn in the surface we use (Connection, Statement, Row, Value, params!).
- Bumped `sha2 0.10 → 0.11`. `Digest` trait + `Sha256::new()` unchanged at our call site (Win32 path-hash for duplicate-window detection).
- Bumped `windows 0.61 → 0.62`. Tauri still pins 0.61 transitively, so both versions live in the lockfile; the only place where they meet is `update_window_title`, which now rewraps `Tauri's HWND.0` into our 0.62 `HWND` (identical struct layout — `pub *mut c_void`). All other Win32 calls already use HWNDs sourced from EnumWindows so no further rewraps were needed.

## [26.4.8] - 2026-04-17

### Fixed
- **xlsx export preserves large integer IDs**: numeric-affinity columns now try `i64` parsing first and emit values beyond ±2^53 as strings instead of coercing to `f64`. Previously, a bigint-sized ID (e.g. snowflake IDs, large sequence keys) would silently lose precision when exported to Excel because xlsx stores all numbers as IEEE-754 doubles.
- **Filter debounce cancelled on table switch**: switching tables mid-debounce no longer fires a stale reload against the new table ~500ms later.
- **Row counts surface errors instead of hiding them**: if a `SELECT COUNT(*)` fails on a table (corrupt page, access issue), the sidebar now shows `?` and logs a warning, instead of silently reporting 0 rows.
- **Graceful fallback when window handle is unavailable** on the duplicate-window detection path, instead of panicking.

### Internal
- Extracted `commitTableConfig` helper in `store.svelte.ts` to consolidate the 10 call sites that re-published table configs via spread — one place to touch when reactivity semantics change.
- Added key expressions to the table-list `{#each}` loops so DOM reuse is correct when the table set changes.
- Added `tabindex="0"` to the data grid viewport for keyboard-focus accessibility.

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
