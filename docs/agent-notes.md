# dblitz Agent Notes - detail

Sections moved verbatim out of `AGENTS.md`, which keeps a one-line pointer to each.
Read the section before working in its area.

## Development

### Node is pinned to 24, and `@types/node` with it

- Node is pinned to 24 in `.nvmrc`, and every CI `setup-node` step reads it via `node-version-file` so there is a single source of truth. `@types/node` is deliberately held on the matching major — `npm outdated` will keep offering a newer one; taking it means type-checking against an API surface the shipped runtime does not have. Bump `.nvmrc` and `@types/node` together or not at all.

### A green local gate can be measured against the wrong dependency

- **A green local gate can be measured against the wrong dependency.** `node_modules` drifts out of the declared range without anything failing: on 2026-08-05 it held `@types/node` **26.1.1** while `package.json` said `^24.13.3` and the lockfile said `24.13.3`, so `npm run check` type-checked against an API surface CI never uses — and reported 0 errors either way. `npm update` (or `npm ci`) reconciles it. Before trusting a type-check that a release depends on, confirm what is actually installed: `node -p "require('./node_modules/@types/node/package.json').version"`. The same drift follows a lockfile change pulled from another machine: on 2026-10-01 the lockfile pinned Tauri 2.12.0 while `node_modules` still held `@tauri-apps/api` 2.11.1, and `npm run tauri dev` stopped with "Found version mismatched Tauri packages". `npm ci` after pulling a lockfile change is the fix, not a version bump.

### Type-checking runs on TypeScript 7 through `svelte-check --tsgo`

- **Type-checking runs on TypeScript 7 through `svelte-check --tsgo`, and TypeScript 6 stays installed on purpose.** svelte-check still loads TypeScript's JavaScript compiler API, which TS 7 does not have, so `typescript` stays `^6` and TS 7 is the alias `@typescript/native`. Remove neither. `npm run check` goes through `scripts/svelte-check.mjs`, because svelte-check exits 0 when it crashes (a missing TS 7 included), and the wrapper fails any run without the `COMPLETED` summary line (`svelteCheck.test.ts`).

### Vitest's global environment is `node`

- **Vitest's global environment is `node`.** The two test files that need a DOM opt in per file with `// @vitest-environment jsdom` on line 1: `sqlEditorExtensions.test.ts` and `dragReorder.test.ts`. Keep it per file: a global jsdom would hide that most of this suite is pure logic. jsdom 30 has no `document.elementFromPoint`, which `dragReorder.test.ts` supplies and asserts is still absent.

### A green gate on one OS says nothing about a `cfg`-gated symbol on another

- **A green gate on one OS says nothing about a `cfg`-gated symbol on another, and `-D warnings` turns that into a hard build failure.** Hit cutting 26.8.1: `use updates::{InstallProvenance, UpdateStatus};` compiled clean on Windows and failed the Linux release leg with `error: unused import: InstallProvenance` — the only two places naming that type were `cfg(windows)` and `cfg(target_os = "macos")`, so on Linux the import was genuinely unused. Windows clippy structurally cannot see it. The tag had already been pushed; nothing published, because the `quality` job runs before `create-release`. Two habits: annotate the type on every arm of a `cfg` triple (`let provenance: InstallProvenance = ...`) so the name is used everywhere, and **check the other platform before pushing a tag** rather than after. `checks.yml`'s backend job covers all three OSes but only on push/PR, which a same-moment tag push races.

### A substring check over text holding a scratch path depends on the process id

- **A substring check over text holding a scratch path depends on the process id.** A `pq` test asserted `!view.contains("-1")` to prove the SQL tab's view does not rescan. The view definition holds the temp path, and on the 26.10.5 tag run that path held `-18004-49`, so the check failed in `quality` although the code was right. It had passed locally, because the local pid had no `-1` in that position. Nothing published, because `quality` runs before `create-release`. Assert the option by its name (`sample_size`), and read any negative `contains` over generated SQL as a check on the path as well.

### A Windows machine can run the Linux gate through WSL

- **A Windows machine can run the Linux gate for real, through WSL**, which is the cheapest way to close half of the gap above. Install the same apt deps `.github/tauri-linux-deps.txt` lists plus rustup stable, then run the gate against the checkout over `/mnt/...`, giving it its own target dir so it cannot thrash the Windows one:
  ```bash
  export CARGO_TARGET_DIR="$HOME/dblitz-linux-target"
  cd /mnt/<drive>/<path-to>/dblitz/src-tauri
  cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --check && cargo test
  ```
  Linux runs **10 fewer** Rust tests than Windows (measured 2026-10-29), and nothing runs on Linux that does not also run on Windows. The ten are the four tests in `platform/open_with.rs`, which write to a scratch key under HKCU, plus the tests in `platform/windows.rs` (both modules are `cfg(windows)`) — `uninstall_key_tracks_the_bundle_product_name`, `path_hash_ignores_case`, `path_hash_distinguishes_directories`, `path_hash_distinguishes_filenames`, `path_hash_is_nonzero_for_real_paths` — and `config::tests::normalize_for_dedup_is_case_insensitive_on_windows`. Diff the two `cargo test` name lists if that gap ever changes; do not compare totals, which move with every added test.

### There is no Windows cross-check from macOS

- **There is no Windows cross-check from macOS.** `cargo check --target x86_64-pc-windows-msvc` cannot run there: `ring` and `libsqlite3-sys` have C build scripts and the MSVC toolchain does not exist on macOS. The WSL recipe above covers Linux only. So `cfg(windows)` code changed on a Mac is verified by reading plus `checks.yml`'s Windows leg, and nothing else — push the branch and read that job before tagging.

### A Windows machine can run the macOS gate on a Mac over ssh

The reverse direction works without CI: a Mac with rustup reachable over ssh runs the
macOS Rust gate on an uncommitted Windows working tree. Nothing has to be pushed —
send the tree as a tar of tracked plus untracked files and the built `build/`
(`generate_context!` embeds it), into a scratch folder:

```bash
{ git ls-files -co --exclude-standard; find build -type f; } | tar -cf - -T - \
  | ssh <mac> 'rm -rf ~/dblitz-gate && mkdir -p ~/dblitz-gate && tar -xf - -C ~/dblitz-gate'
ssh <mac> 'export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin"; cd ~/dblitz-gate/src-tauri \
  && cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --check && cargo test'
```

A non-interactive ssh session has no `~/.cargo/bin` on `PATH`, so set it explicitly.
The macOS test list equals the Linux one (ten fewer than Windows; see the WSL entry).
The `lib.rs` split of 2026-10-29 moved code between `cfg` arms and was checked this way
on all three platforms before commit; a cold first run compiles DuckDB and takes
several minutes, so run it in the background.

### README screenshots are captured by `scripts/readme-screenshots.py`

The images in `docs/images/` come from the synthetic database
`scripts/make-demo-db.py` builds (fixed seed, byte-identical across runs, every row
invented), driven over CDP; the script's docstring has the launch steps. Five things
each cost a retry:

- **The toolbar shows the full database path**, and it lands in every image. Copy the
  database to a neutral path (`C:\demo\roastery.sqlite`) first; a scratch or home path
  publishes the user name.
- **A debug `dblitz.exe` loads its UI from Vite on port 1420.** Without `npm run dev`
  running the webview shows Edge's "localhost refused to connect" page, while the
  window title still names the file, because the Rust side opened it.
- **Playwright's own `page.screenshot` ignores a `deviceScaleFactor` override** and
  returns 1200 x 760 regardless; CDP's `Page.captureScreenshot` honours it, which is
  how the stills are 2x.
- **CodeMirror's view is not reachable from the DOM in this build**, and keystrokes
  are unsafe for setting text up: an open suggestion list takes Enter. The script
  sets text with a synthetic `paste` event (CodeMirror reads `clipboardData`) and
  types only what the GIF shows, then asserts the editor holds the intended query.
- **dblitz saves the sort per file**, across restarts, so clicking a header toggles
  from the previous run's state. The script reads `aria-sort` and clicks until it
  says `descending`.

### Inspecting a shipped build's webview (Windows)

A released `dblitz.exe` has no devtools: `toggle_devtools` is `cfg(debug_assertions)`
and the F12 handler is gated on SvelteKit's `dev`. WebView2 still honours its
environment variables though, so a **release** binary can be driven over CDP without
rebuilding anything:

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
$env:WEBVIEW2_USER_DATA_FOLDER = "$env:TEMP\dblitz-dbg-udf"
Start-Process dblitz.exe -ArgumentList '"<path to a database>"'
```

`http://127.0.0.1:9222/json` then lists the targets, and the `webSocketDebuggerUrl`
accepts `Runtime.enable` / `Runtime.evaluate` — enough to read
`document.body.innerText`, call `window.__TAURI_INTERNALS__.invoke(...)` against the
live backend, and catch `Runtime.exceptionThrown`.

**This is the only way to tell a backend failure from a webview failure in a shipped
build, and from outside they look identical.** The native window title is set by
`open_database` on the Rust side, so a title showing the filename proves the database
opened even while every panel renders its "Open a SQLite database…" placeholder. That
combination means the frontend died after a successful open — in the 26.7.5 case
(diagnosed 2026-08-14) an `effect_update_depth_exceeded` loop at mount, which left the
whole reactive graph wedged so the later publish of `dbPath` never reached the DOM.

Three traps, each of which reads as a different bug:

- **The separate user data folder is not optional while another instance is running.**
  Reusing one folder with different browser arguments fails webview creation with
  `0x8007139F` ("the group or resource is not in the correct state"), and the process
  then stays alive owning a window with no content at all.
- **A `tauri dev` build auto-opens devtools, and that is itself a CDP `page` target.**
  Filter it out (`!t.url.startsWith("devtools:")`) or you inspect the inspector.
- **Duplicate-instance detection will silently hijack the run.**
  `try_activate_existing` matches on the resolved absolute path, so launching a second copy on a
  file another instance already holds open just raises that window and exits 0.
  Close the other instance, or test against a different file.

## Architecture

### Parquet backend (`src-tauri/src/pq/`)

A Parquet, CSV or JSON file is served by an embedded DuckDB behind the **same 20 IPC
commands**, so the frontend has no second code path. The Rust names (`ParquetSession`,
`open_parquet`, `parquet_session()`) predate CSV and JSON and were kept; `pq::FileFormat`
is the only per-format switch. `commands.rs` dispatches on
`DbState::parquet_session()`; `db::open_parquet` and `db::close_database` keep the
failure-atomic open (both DuckDB instances are built before `DbState` changes, and a
SQLite open drops any Parquet session - `db::tests::opening_one_kind_of_file_drops_the_other`).
Parquet is detected by content (`PAR1` at both ends), so a renamed file still opens. CSV
and JSON have no magic bytes and go by extension (`.csv` `.tsv`, `.json` `.jsonl` `.ndjson`);
anything else reaches SQLite unchanged. The JSON reader is an extension, compiled in through
the `json` feature of the `duckdb` crate, because extension autoinstall is off.

- **CSV and JSON are loaded into the browse instance once, as the table `file_rows`**, and
  `data_rn` exposes its `rowid` as `file_row_number`. A table filled by
  `CREATE TABLE ... AS SELECT * FROM read_csv(...)` keeps file order
  (`preserve_insertion_order` is DuckDB's default), so `rowid` counts lines and the
  paging and sort-cache code in `pq/query.rs` serves all three formats unchanged. Reading
  the text file per page would re-parse all of it for every page and every sort. A file
  column named `rowid` hides the pseudo-column, so it sends that file to `LIMIT/OFFSET`
  like a `file_row_number` column does (`a_csv_with_its_own_rowid_column_pages_without_row_numbers`).
  The SQL tab's locked instance reads the file per query through the same reader
  function, with `allowed_paths` limited to it, as for Parquet. A load of a big CSV costs
  open time and memory that a Parquet open does not; the 2 GB `memory_limit` and the
  spill directory apply.
- **A CSV or JSON load tries DuckDB's 20,480-row sample first, and only a failure pays
  for a full scan (`load_text_file`).** A value below the sample that does not fit the
  sampled type (`-` in a numeric column) is a conversion error at load, and a JSON key
  first seen below it is an error too. `sample_size = -1` fixes both, but on a 233 MB CSV
  it costs 2.2 s against 0.16 s for the load, and it must not reach the SQL tab: a view
  binds again on every prepare, the live check prepares after every keystroke, and each
  bind would cost 2 s against 1 ms. So the full-scan path hands `LockedDb` a reader with
  the result written out: `auto_detect = false` plus dialect and columns for CSV, `columns`
  for JSON. The CSV reader is built from `sniff_csv`'s fields because its `Prompt` column
  does not escape (DuckDB 2.0.0): a path or column name with an apostrophe does not parse.
  `count(*)` does not reproduce the failure, because projection pushdown skips the
  conversion; use `SELECT *`. When the full scan fails too, the sampled error is reported,
  because it names the line where the sniffer's says only that detection failed. Ragged
  rows stay an error: `ignore_errors` would drop rows without saying so. Pinned by the
  `pq::tests` `a_csv_with_text_below_the_sample_...`,
  `the_pinned_csv_reader_survives_...`, `a_json_file_changing_below_the_sample_...` and
  `a_csv_the_full_scan_cannot_read_...`.
- **CSV and JSON get no installer file association; each platform offers a weaker one.**
  Tauri's NSIS association (`APP_ASSOCIATE`) writes `Software\Classes\.<ext>` itself, so
  dblitz would become the double-click handler for every CSV on a machine where the user
  never picked one; those files belong to spreadsheets and editors. Windows: a Settings
  checkbox (`platform/open_with.rs`) writes per-user `OpenWithProgids` entries plus a
  `dblitz.<ext>` ProgID, which only add dblitz to "Open with". The registry is the
  setting's only record, so the portable exe can set it too, and the checkbox reads it
  back on every open. The uninstaller removes the same keys through
  `windows/hooks.nsh`, guarded by `$UpdateMode <> 1` because every update runs the old
  uninstaller with `/UPDATE` first; a test checks its list against the Rust one. macOS:
  `tauri.macos.conf.json` declares them with `LSHandlerRank` `Alternate`. That file
  repeats the shared entries, because a platform config replaces the whole array, and a
  declaration with a content type gets one extension, because macOS ignores the
  extensions once `LSItemContentTypes` is set (`src/lib/fileAssociations.test.ts`).
  Linux gets nothing: a `.desktop` `MimeType` can make the app the default.

- **Two DuckDB instances, mirroring `conn`/`aux_conn`.** `browse` runs only SQL dblitz
  generates and stays unlocked, because it must `ATTACH` the sort cache and spill. `sql`
  (the SQL tab) is locked, see below. The file is exposed as the view `data`; `browse`
  also has `data_rn` with DuckDB's virtual `file_row_number`, unless the file has its own
  column of that name, in which case every view falls back to `LIMIT/OFFSET`.
- **The SQL tab lockdown is two layers, and the order is load-bearing** (`pq/lockdown.rs`).
  Settings (`allowed_paths` = the file, external access off, extension autoload/install off,
  then `lock_configuration`) stop every file and network access. A single-SELECT classifier
  stops what settings allow: `CREATE OR REPLACE VIEW data AS SELECT 42` went through with
  settings alone. The classifier must only ever see a **locked** instance, because
  preparing does I/O: preparing `EXPORT DATABASE '<dir>'` created the directory, and
  preparing a query over an https URL issued the GET. Pinned by
  `classifying_on_an_unlocked_instance_does_write` (the control) and
  `classifying_on_the_locked_instance_writes_nothing`. duckdb-rs keeps the statement type
  private, so the classifier uses the C API on a second raw connection;
  `Connection::open_from_raw` does not own the database, hence `LockedDb`'s `Drop`.
- **The SQL tab runs the user's SQL through `query('<literal>')`**, not a spliced
  subquery, so a trailing `;` or `-- comment` is harmless, and renders columns by
  position (`#1`) so duplicate names survive. DuckDB renames a repeated name (`a`, `a_1`).
- **Every cell is rendered by DuckDB with `CAST(col AS VARCHAR)`**, in the page query and
  in the filter SQL alike, so the grid text is exactly what a filter matches - the Parquet
  equivalent of `render_real`. BLOBs render as `[BLOB n bytes]` and never match.
- **A file controls parts of a type string** (ENUM values, STRUCT field names), and the
  `browse` instance is unlocked, so `pq/filters.rs::cast_target` interpolates a type only
  when it is letters, digits, spaces, `(),_`; anything else compares as text.
- **The filter grammar is parsed once**, by `db::filters::parse_criteria`, and both
  builders consume it. Text matching is `ILIKE` (DuckDB's `LIKE` is case-sensitive); a
  comparison operand is cast to the column type and one that does not fit is an error;
  a regex runs in DuckDB (RE2).
- **Paging** (measured 2026-09-28, 50M rows x 14 columns, 3 GB, M5, release):
  unfiltered pages use `file_row_number` ranges, 20-24 ms at any depth (`LIMIT/OFFSET`
  reached 82 ms at 50M). Filtered views cache the ascending matching row numbers and fetch
  chunks with `IN (...)`, ~22 ms. Sorted views are materialized **in their own column
  types** into `<cache dir>/dblitz/sort-cache/<pid>-<n>.duckdb` and paged by `rowid`,
  6-9 ms; each page is rendered on the way out. Caching rendered text instead took 34.5 s
  against 17.7 s. A cached order list was rejected in the spike: ~500 ms per chunk,
  because a sorted chunk touches nearly every row group.
- **A sorted view of a file up to `ORDER_LIST_MAX_CELLS` (200M cells, rows x columns, and
  at most 20M rows) is paged from an order list instead**: the row numbers in sort order,
  each chunk fetched with `IN (...)` and put back into list order in Rust, because the
  scan returns file order. It is the filtered path with a different `ORDER BY`
  (`View::Listed`). The reason is that the materialized build pays for copying every
  column, not for sorting: on 700,000 rows x 100 columns of short text the sort of key
  and row number took 20-40 ms, copying the columns into a memory table 1.6-2.0 s and
  into the cache file 4.5-7.3 s, with or without `ORDER BY`. No `force_compression`
  codec and no thread count brought the cache write under 3.4 s. Measured 2026-10-08,
  Windows, 32 threads, release, generated files (first sorted page, then a later page):

  | Rows x columns | Materialized | Order list |
  |---|---|---|
  | 425,000 x 16 | 0.28-0.30 s, 2 ms | 19-25 ms, 7-11 ms |
  | 700,000 x 100 | 5.4-7.3 s, 14-19 ms | 31-42 ms, 24-225 ms |
  | 1,900,000 x 41 | 4.4-4.6 s, 14-17 ms | 65-89 ms, 25-119 ms |
  | 2,000,000 x 100 | 16.6-17.9 s, 60-66 ms | 72-88 ms, 63-336 ms |
  | 10,000,000 x 14 | 3.3-4.3 s, 8-16 ms | 330-433 ms, 23-175 ms |
  | 5,000,000 x 100 | 44 s, or out of memory | 160-228 ms, 147-1,096 ms |

  Two real files confirm it, sorted by the first, middle and last column in both
  directions: 708,930 x 100 shows its first sorted page in 58-183 ms (5.3-5.9 s
  materialized) and later pages in 27-99 ms (20 ms); 1,891,987 x 41 in 182-222 ms
  (3.5-3.7 s) and 31-108 ms (21 ms).

  The low end of each page range is a sort column that follows file order, the high end
  one that scatters every page over the whole file. The last row is above the limit and
  still materializes: a page of more than a second was the reason to stop at 200M cells.
  The limit is a field on the session (`order_list_max_cells`) so that a test can force
  either path on a small fixture; `the_cell_count_decides_how_a_sorted_view_is_paged`
  pins the boundary and `a_page_of_an_order_list_keeps_the_sort_order_within_the_page`
  the reordering.
- **A sort cache that cannot be built falls back to the order list**, for that view and
  every later sort of the session (`sort_cache_failed`), whatever the file's size. The
  case it exists for: DuckDB sorts whole rows, and under the 2 GB limit a wide file
  fails with "Out of Memory Error: failed to pin block". The copy without `ORDER BY`
  completes, so it is the sort and not the write. Measured 2026-10-08, 32-thread
  Windows machine, sort by a scattered column:

  | Rows x columns | Threads: result |
  |---|---|
  | 5,000,000 x 50 | 32: ok, 15 s |
  | 5,000,000 x 100 | 32: fails after 19 s. 24, 16, 8, 4: ok, 45-50 s |
  | 10,000,000 x 100 | 32, 16: fail after 9-17 s. 4: ok, 88 s. 32 with a 4 GB limit: ok, 73 s |
  | 3,000,000 x 200 | 32, 16: fail after 11-14 s. 8: ok, 97 s |
  | 2,000,000 x 400 | 8: fails after 14 s. 4: fails after 80 s |

  No thread count is safe for every shape, which is why the fix is a fallback and not a
  cap; `preserve_insertion_order = false` changed nothing. The price is the wait for
  the failed build: the 5M x 100 file shows its first sorted page after 21 s and then
  pages in about 1 s. The fallback takes any build failure except a cancelled or
  interrupted one, so a cache file that cannot be written (a full disk) is covered too,
  and that is how `a_sort_the_cache_cannot_hold_is_paged_from_a_list` reaches it
  without needing an out-of-memory failure. Not built: predicting the failure, so the
  wait is spared.
- `parquet_benchmark` takes a fourth argument, the number of extra text columns:
  `700000 500 5 86` is the 100-column case.
- **`MEMORY_LIMIT` is 2 GB per instance**, and the process peaks above it (DuckDB limits
  its buffer pool only): 3 GB gave 13.0 s / 3.8 GB RSS on the sort above, 2 GB 17.7 s /
  2.65 GB. 1 GB starved the Parquet scan itself.
- **DuckDB creates its spill directory but not its parents**, so `ParquetSession::open`
  creates them; before that, every sort too big for memory failed on a fresh machine.
  The unit tests never spill, which is why only the 50M-row run found it.
- **Sort progress** (`view_progress`): duckdb-rs keeps the connection handle that
  `duckdb_query_progress` needs private, so both DuckDB instances are a `pq/raw.rs`
  `RawDb` - opened through the C API, with the wrapper `Connection` plus one raw "side"
  connection. The SQL tab's side connection is the classifier; the browse instance's
  builds sort caches, with `enable_progress_bar` and `progress_bar_time = 0` set on it,
  so another thread can read its progress and interrupt it. A `building` flag limits
  `view_progress` to the build itself. Measured on 50M rows the figure rose steadily
  from 0 to 100 (the scan is the first half). `interrupting_stops_a_sort_build`
  interrupts without bumping the generation, because with a bump `query_table`
  reports "cancelled" after a build that ran to completion - a test that could not
  tell whether the build stopped, and did not.
- **CI compiled DuckDB twice per job until `[profile.dev.build-override] debug = false`.**
  Without it, `cargo check`/`clippy` and `cargo build`/`test` compiled libduckdb-sys's
  build script in two profiles (its fingerprints differed only in `profile` and in the
  build script's own dependencies), so the script ran twice and each run compiled the
  C++. Locally it hid in the cache: both outputs existed side by side, so neither
  command rebuilt. `cargo check -v` and `cargo build -v` printing the same
  `libduckdb-sys-<hash>/out` is the check.
- The sort cache is detached and deleted when the session drops; stale sort caches and
  spill directories older than 24 h are swept at startup (`pq::sweep_stale_cache`).
- Timings go through the shipped code: `cargo run --release --example parquet_benchmark [rows]`
  generates its own file and calls `ParquetSession`/`query_table` via `db::bench_api`.
- Cost: `duckdb` with `bundled` + `parquet` (`bundled` alone has no Parquet reader), +35.7 MB
  stripped and a 147 s cold build on 10 cores; it pulls in the Arrow crates.

### The SQLite backend's modules

- SQLite backend code lives under `src-tauri/src/db/`, with `src-tauri/src/db.rs` as a thin facade re-exporting the submodules:
  - `schema.rs` — table/column introspection, the open batch, and row counts
  - `query.rs` — table paging, the rowid-index fast path, and regex filtering
  - `filters.rs` — the `WHERE` clause builder and column-filter operator parsing
  - `sql.rs` — arbitrary SQL execution plus the read-only / ATTACH-DETACH rejection gate
  - `export.rs` — XLSX export
  - `types.rs`, `util.rs` — shared DTOs and helpers (`safe_ident`, `read_row`, `render_real`, `StrErr`)
  - `bench_api` (in `db.rs`) — the one seam `src-tauri/examples/*.rs` may use. The two benchmarks call the shipped functions through a real `open_database` connection; they used to be re-implementations that could drift from what ships. Keep it that way when adding an example.

### All column discovery goes through `PRAGMA table_xinfo`

- **All column discovery goes through `PRAGMA table_xinfo`, never `table_info`, and `schema::table_columns` is the one place that reads it.** `table_info` omits generated columns; `SELECT *` returns them, and that mismatch corrupts paging silently. `table_columns` returns `visible` (`hidden` 0/2/3, exactly what `SELECT *` returns, in its order) and `declared` (every name, `hidden=1` included, used only for rowid-shadow detection, where erring wide costs the fast path and erring narrow corrupts data). Pinned by `query::tests::{generated_columns_are_reported_and_returned, regex_filter_index_survives_a_preceding_generated_column, generated_rowid_column_shadows_the_alias, generated_rowid_column_does_not_corrupt_paging}`, `schema::tests::get_columns_lists_generated_columns_with_contiguous_cids`, and the CI smoke fixture's `GENERATED ALWAYS` column.

### Every non-trivial view pages from one cached ordered-rowid list

- Every non-trivial view (sorted, filtered, regex-filtered) pages from **one** cached ordered-rowid list per table, built once and sliced per chunk. Its identity is `OrderKey`, and the `regex_signature` field in it is load-bearing rather than decorative: a regex is evaluated in Rust and never reaches the SQL, so two views with different patterns produce an **identical** `where_clause`, `params` and `order_clause`. Without that field one pattern's match set is served as another's, and clearing the regex keeps serving the narrowed set. Tables with no addressable rowid (WITHOUT ROWID, or all three aliases shadowed) keep the full-scan LIMIT/OFFSET fallback.

### A regex scan selects only the columns a pattern is applied to

- A regex scan selects only the columns a pattern is applied to (`project_regex_columns`), and the filter indices it hands on are indices into **that projection**, not into the table. Semantics, each with its own test in `query.rs`: NULL matches as the empty string, a BLOB never matches, a number matches the text the grid renders, and an unknown column name is an error rather than a silently dropped filter that shows every row (`regex_treats_null_as_the_empty_string`, `regex_never_matches_a_blob`, `regex_on_a_numeric_column_matches_the_text_the_grid_shows`, `regex_on_an_unknown_column_errors_instead_of_showing_every_row`, `regex_filter_matches_the_named_column_when_it_is_not_the_first`, `two_regex_filters_apply_to_their_own_columns`).

### Chunk 0 of an unsorted table is served with `LIMIT`

- Chunk 0 of an unsorted table is served with `LIMIT` and the rowid index is built on the first `offset > 0` (`first_chunk_is_served_without_building_the_rowid_index`). Unfiltered row counts are cached in `DbState::table_counts`, seeded at open and cleared in `clear_caches`; that is sound **only** because of `?immutable=1` (`open_seeds_the_row_count_cache_and_pages_read_it`, `a_view_pages_without_recounting_every_chunk`). Column introspection runs once per `query_table` (`one_column_pragma_per_query`). A crafted `COUNT(*)` cannot abort on allocation (`preallocated_bounds_a_crafted_row_count`).

### REAL cells render as SQLite renders them

- **REAL cells render as SQLite renders them** (`util::render_real`: `3.0`, `1.0e+20`, `-0.0` as `0.0`), so a filter typed against what the grid shows matches. `render_real_matches_sqlite_cast_as_text` checks it against SQLite itself in two classes — byte-identical up to 13 significant digits, same-double beyond, because SQLite's `%!.15g` last digits are approximate. On a column with **no** declared affinity `=<number>` also compares numerically (`filters::tests::exact_match_on_a_column_with_no_affinity_also_compares_numerically`); typed columns are untouched.

### `dblitz` is a strict read-only SQLite viewer

- `dblitz` is a **strict read-only SQLite viewer** by explicit decision (2026-04-10). Reject feature requests that imply mutation (inline cell edit, delete row, save-as, schema changes) and surface the decision before implementing. Enforcement is in **four** layers — preserve all four when changing query execution or database opening:
  1. Connections open with `SQLITE_OPEN_READ_ONLY`.
  2. Plus `?immutable=1` in the URI: the file is a frozen snapshot for the connection's lifetime, and no `-wal`/`-shm` companion files are ever created (source files may sit in cloud-synced folders other tools write to).
  3. `execute_sql` rejects non-readonly prepared statements (`stmt.readonly()`), and a SQLite authorizer denies ATTACH/DETACH and transactions at the engine level.
  4. `ALLOWED_INTROSPECTION_PRAGMAS` in `schema.rs`: the open batch and any PRAGMA path accept only read-only introspection pragmas. This is why `PRAGMA journal_mode=wal` is refused before the authorizer ever sees it.
  - `sql::tests::read_only_connection_refuses_a_write_the_allowlist_does_not_stop` pins "dblitz cannot write" but deliberately not *which* layer stops it: removing `READ_ONLY` alone, or `immutable` alone, leaves it green. Both must go for it to fail, so a layer deleted on its own is caught by review, not by that test.
  - Accepted trade-offs: dblitz does not see live writes from other processes during a session (reopen the file to pick up changes), and a file another process truncates mid-session surfaces as a read error ("database disk image is malformed") rather than a crash. Backend row/index caches need no invalidation because the snapshot is frozen.

### The SQL editor's live check goes through the same gates as Execute

- **The SQL editor's live check goes through the same gates as Execute, and must keep doing so.** `db::sql::prepare_checked` holds the ATTACH/DETACH gate, the prepare and the `stmt.readonly()` gate; `execute_sql` and `validate_sql` both call it, and `validate_sql` never steps. The Parquet side shares `refusal_message` and the classifier. The frontend disables Execute while the check reports an error for the exact text that would run (`blockReasonFor`), so a check that drifted from Execute would block valid queries. Pinned by `validate_sql_agrees_with_execute_sql_on_every_rejection` and `validate_matches_execute_and_never_runs_the_statement`. Four facts behind the design:
  - `validate_sql` takes the connection with `try_lock` and answers "no verdict" (`None`) when it is busy, so a check typed during a long query neither waits nor delays Execute (`validate_sql_gives_no_verdict_instead_of_waiting_for_a_busy_connection`).
  - SQLite's error offset comes from rusqlite's `Error::SqlInputError`, which exists only under `modern_sqlite` (implied by `bundled`). DuckDB's C API has no position; `pq::sql::duckdb_error_offset` parses the `LINE n:` excerpt and `^^^` marker at the end of its messages (character columns, long lines windowed with `...`) and drops the offset unless the excerpt matches the source line. Offsets cross IPC as UTF-16 indices (`SqlCheck::problem`).
  - The function lists the editor suggests live in `src/lib/components/sqlFunctions.json` and are checked against both engines by `every_function_the_editor_suggests_exists_in_the_bundled_*`. The bundled SQLite has no `SQLITE_ENABLE_MATH_FUNCTIONS`: `sqrt`, `ln`, `pow` do not exist. DuckDB's `coalesce` and `ifnull` are parser special forms missing from `duckdb_functions()`.
  - `@codemirror/lint`'s `forceLinting` does nothing unless a check is already scheduled; a database switch re-checks through `requestRecheck` (a state effect the linter's `needsRefresh` watches).

### `PRAGMA mmap_size` must not come back

- **`PRAGMA mmap_size` must not come back.** With `immutable=1` there is no locking, so a file shrinking under an open mapping killed the process with SIGBUS and printed nothing. Removing it turns that into the recoverable error above, and it was not even a win: mmap only helped repeated warm `COUNT(*)`, and cold every measured operation was slower with it. `schema::tests::open_database_does_not_memory_map_the_file` asserts `PRAGMA mmap_size` reads 0; the reasoning is in the comment at the open batch.

### There are two read-only connections to the same file

- **There are two read-only connections to the same file.** `conn` serves `query_table`/`count_rows` and owns the table-keyed caches; `aux_conn` serves `execute_sql`, `get_schema` and `get_columns`, so the SQL and Structure tabs do not queue behind a multi-second sort (0.02 ms against 734 ms during a 0.8 s sort of 5,000,000 rows). Two connections agree only because of `?immutable=1` — one frozen snapshot — so removing `immutable=1` means removing the second connection in the same change. Both are built by `schema::open_read_only_connection` (same flags, URI, open batch and authorizer, so they cannot drift), both are interrupted by `cancel_queries` and dropped by `close_database`, and `open_database` is the only site taking more than one lock: the order is `conn` -> `aux_conn` -> `table_counts`. Pinned by `sql::tests::execute_sql_does_not_wait_for_the_browse_connection`, `sql::tests::cancel_interrupts_the_secondary_connection` and `schema::tests::secondary_connection_enforces_every_read_only_layer`.

### Windows duplicate-instance detection is keyed by a hash of the path

- **Windows duplicate-instance detection is keyed by a 64-bit hash of the case-folded absolute path**, held in the `dblitz_db_path` window property (`SetPropW` stores one pointer-sized value, which is why it is a hash and not the string). It is not filename-only matching: same-named databases in different directories must open separately. Both the writer and the reader take their path from `launch_argument()`/`resolve_launch_argument()`, so no caller can hash a raw `argv[1]` against a marker holding the resolved path (`launch::open_request_tests::every_caller_of_the_launch_argument_gets_the_absolute_path`). The `FlashWindowEx` call that follows `SetForegroundWindow` in `try_activate_existing` is deliberately unconditional and must stay: activation is usually denied by the foreground lock, and without the flash the second launch exits silently and reads as "dblitz refuses to open this file". See the comment there for why the outcome cannot be tested reliably.

### A launch argument is the one path that arrives relative

- **A launch argument is the one path that arrives relative** — dialog, drag-and-drop, recents and macOS document-open events are all absolute. `absolutize_launch_path` resolves it in `launch.rs` rather than inside `path_to_sqlite_uri`, because the absolute form is also what lands in recents, is hashed into the config key, and is compared by the duplicate-instance check.

### A view-config save carries its own database path

- **A view-config save carries its own database path; the backend must never pick one from mutable state.** `save_view_config` is `#[tauri::command(async)]`, so it runs at a time the frontend does not control while `state.current_path` moves. `store.svelte.ts` serializes saves on their own chain (separate from `databaseRequestQueue`, so an autosave per drag frame cannot block an open) and captures the path *and* a detached JSON copy of the config at enqueue time — `$state.snapshot` returns the same object under this repo's vitest setup, so later edits still reached the queued save. The backend writes to the path it was handed: a save legitimately outlives its session, and the path is a lookup **key**, not a destination. Pinned by `store.svelte.test.ts` ("saves under the database that was open when the change was made", "serializes saves so an older snapshot cannot overwrite a newer one", "captures the config at enqueue time") and `config::tests::{save_config_keys_strictly_off_the_db_path_it_is_given, config_path_for_db_stays_inside_the_config_dir}`.

### The config key is a hash of the normalized path

- **The config key is `sha256(normalize_path_key(path))[..16]`** — case-folded and slash-normalized on Windows, byte-exact everywhere else, so one file cannot own two settings files. `ConfigStore` carries a `windows_path_semantics` flag so the Windows behaviour is tested on any OS. Reads fall back to the pre-normalization raw-path key and migrate on the next save, removing the old file (`view_config_key_folds_windows_path_spellings_together`, `unix_paths_stay_case_sensitive`, `load_config_falls_back_to_the_pre_normalization_key_then_migrates`).

### An `await` in a component publishes only if it still owns publication

- **An `await` in a component publishes only if it still owns publication.** Two facts, both captured BEFORE the await: a monotonic per-caller token and `appState.dbOpenGeneration`. The token alone lets a closed database's answer repopulate a reset panel; the generation alone lets two requests inside one session finish out of order and label table B with table A's columns. `createSessionOwnedRequest` in `sessionOwnedRequest.ts` is the seam — a publication guard, not a request bus: it cancels nothing, the in-flight call still completes and its answer is dropped, and its errors are gated too. Users: `DatabaseStructure.svelte` and `createSqlExecution` in `sqlExecution.ts` (a SQL result from a closed or replaced session is dropped, writes no history entry, and clears `running`). `store.svelte.ts` and `virtualRows.svelte.ts` enforce the same rule inline. Pinned by `sessionOwnedRequest.test.ts` and `sqlExecution.test.ts`.

### Find in the grid searches the rows the grid shows

- **Find in the grid (`gridFind.ts`, DataGrid's find bar) searches the rows the grid shows, through the grid's own row source.** It loads rows with `getVisibleRows`, so it uses the grid's chunk size: asking the backend for bigger pages to scan faster would rebuild the rowid index, which is keyed by chunk size (`rowid_index_rebuilds_when_chunk_size_changes`). It matches the cell strings the grid renders, NULL never matches, and a block that fails or comes back short stops the search with an error, never "No match" for rows it did not see. It stops when `viewToken` (the `virtualRows` epoch; the `rows` array identity in static mode) or the `columns` identity changes, so a filter edit mid-search cannot select a cell in the new view. Ctrl+F belongs to the grid and Ctrl+Shift+F to the column finder; the window-level key handler ignores events that are already `defaultPrevented` (CodeMirror's own search in the SQL editor) and grids that are not on screen (both tabs stay mounted). Vertical reveal goes through `scrollTopToRevealRow`, which centres the row because the compressed spacer's ratio exceeds one pixel per pixel; pinned for 1.8M and 100M rows in `gridGeometry.test.ts`.

### A column preset stores the visible columns, in order

- **A column preset stores the VISIBLE columns in order, not the hidden ones**, so a column the table gains later stays hidden when the preset is applied. Presets live per table in the file's `ViewConfig` (`column_presets`, `#[serde(default)]`); `sanitize_column_presets` drops blank names and empty presets, caps names at 64 chars and dedupes by name. Applying one, and Show all / Hide all, is one `updateTableConfig` call, so one save rather than one per column.

### A column's display name is not its identity

- **A column's display NAME is not its identity, and the clipboard is not a `join`.** `SelectionData` carries `columnIndices` and types are recovered positionally, because a result set may legitimately repeat a name (`SELECT a.value, b.value` typed `INTEGER, TEXT`). Serialization lives in `clipboardTable.ts` and puts both an HTML table and RFC 4180-quoted TSV on the clipboard, so a cell holding a tab or newline cannot become extra columns or rows. Neither form is byte-exact in every target — a tab survives exactly only in the plain-text form — but the pasted table always has exactly the selected rows and columns. Pinned by `selectionData.test.ts` ("records the source column index of every header", "resolves a later duplicate column name to its own declared type") and `clipboardTable.test.ts` ("round-trips cells containing tab, LF, CRLF and quotes").

### There are two column-reorder implementations

- **There are two column-reorder implementations and only one of them has ever been proven in WebView2.** `DataGrid`'s header reordering uses mouse events with a movement dead zone and document-level cleanup (`dragReorder.svelte.ts`, tests in `dragReorder.test.ts`); `ColumnSettings.svelte` uses HTML5 drag-and-drop with `dataTransfer`, driving the same `onReorder`. Do not replace the mouse-event path with HTML5 DnD without proving it in the Windows WebView2 runtime — and note the `ColumnSettings` path carries exactly that unproven risk today.

## CI

### The three workflows, and the tests that pin them

`.github/workflows/checks.yml` runs frontend checks, a Rust backend matrix
(ubuntu/windows/macos, clippy `-D warnings` on all three), and a `smoke` job.
`.github/workflows/release.yml` runs on a `v*` tag, and
`.github/workflows/sign-rehearsal.yml` is started by hand (see *Windows code
signing* below). All three are pinned and bounded;
`src/lib/releaseWorkflow.test.ts`, `checksWorkflow.test.ts` and
`windowsSigning.test.ts` fail on any drift.

### `DirectoryNotEmpty` from `libduckdb-sys` on Windows is the restored rust-cache

`Checks` failed on both Windows legs after the `Cargo.lock` update for 26.10.5:
`unpacking bundled DuckDB sources: Os { code: 145, kind: DirectoryNotEmpty }`. The
build script unpacks into a staging folder and renames it onto
`target/duckdb-sources/<hash>`, and that rename fails when the folder exists without
its `.complete` marker.

The marker goes missing when the cache is saved. `rust-cache`'s `cleanTargetDir`
treats a folder under `target` as a profile only if it has `build`, `.fingerprint`
or `deps`. Any other folder it descends into, deleting every file and keeping the
subfolders. So the cached `duckdb-sources/<hash>` is a tree of empty folders. The
cache key does contain the lockfile hash, but the restore key falls back to the
newest cache without it, so a `Cargo.lock` change restores that tree. On that run the
Windows legs ran the build script again, and macOS and Linux did not compile
`libduckdb-sys` at all, so they passed with the same folder. Why cargo reran it only
on Windows is not known.

`scripts/drop-incomplete-duckdb-sources.sh` now removes such a folder after the
restore, in both `checks.yml` jobs that restore a Rust cache, and prints what it
kept or removed. The release workflow does not run it: its legs log `No cache found`
on a new tag, because a tag cannot restore a cache saved on another tag. A tag moved
after a leg finished could hit the error. If the error comes back, delete the caches
and rerun the failed jobs; the cold build passed that way:

```bash
gh cache list --key v0-rust-backend-Windows_NT
gh cache delete <key>
gh run rerun <run id> --failed
```

### Every `uses:` is pinned to a full commit SHA

- **Every `uses:` in every workflow is pinned to a full 40-character commit SHA with a `# vX.Y.Z` comment.** The release build job holds the updater's minisign private key, the Apple credentials and a contents-write token at once (and, on the Windows leg, the login to the code signing certificate), and a mutable ref decided which code received them — note `dtolnay/rust-toolchain@stable` is a **branch**, which the shorthand disguises as a toolchain channel. Pinning does not make an action trustworthy; it makes the version reviewable and makes an upstream change arrive as a diff. `.github/dependabot.yml` advances the pins monthly in **two** groups: `tauri-apps/tauri-action` is excluded from the wildcard group and gets a `release-critical` group to itself, because it is the one action handed the signing material and the one whose breakage is silent (a bad `latest.json` publishes green and updates nobody). Dependabot cannot see a branch SHA at all, so check `gh api repos/<owner>/<repo>/tags` before pinning anything new.

### `dtolnay/rust-toolchain` is pinned to tag `v1`

- **`dtolnay/rust-toolchain` is pinned to tag `v1`, where `toolchain` is a required input**, so all three call sites pass `with: toolchain: stable`. Dropping that `with:` block fails the action, not the build — it is load-bearing, not decoration.

### Every job has a `timeout-minutes` with its observed maximum

- **Every job has a `timeout-minutes` with its observed maximum in a comment next to it**, so an infinite hang costs minutes rather than six hours. The release `build` legs are 90 minutes each and `max-parallel: 1`, so the job as a whole can legitimately take four times one leg.

### Every workflow defaults to `permissions: contents: read`

- **Every workflow defaults to `permissions: contents: read`**, with `contents: write` granted only to `create-release`, `build` and `publish`. `update-tap` goes the other way to `permissions: {}` because it authenticates as `TAP_GITHUB_TOKEN` throughout.

### `release.yml`'s concurrency group is the workflow, not the ref

- **`release.yml`'s concurrency group is `${{ github.workflow }}`, not the ref, with `cancel-in-progress: false`.** Each tag is its own ref, so a ref-scoped group would serialize nothing; two overlapping releases would then race the same `latest.json`.

### The Linux build prerequisites live in one file

- **The Linux build prerequisites live in one file, `.github/tauri-linux-deps.txt`**, read into `$deps` at all four apt sites with an emptiness check (a bare `$(grep ...)` substitution lets `apt-get` exit 0 on a missing file). Before it existed, `checks.yml` gated main against `libappindicator3-dev` while `release.yml` shipped installers built against `libayatana-appindicator3-dev`. The smoke job appends `webkit2gtk-driver xvfb` on its own install line. A composite action was rejected: `uses: ./...` cannot be SHA-pinned and would fail the pin test.

### The packaged-app smoke test, and exactly what it does not cover

The `smoke` job (Linux) builds the real binary, launches it under `tauri-driver`
against a generated fixture database (`scripts/smoke-test.mjs`), and asserts a row
of that database renders in the grid. It is the only check above the webview-IPC
seam — every vitest and cargo test runs below it — so it is what proves bundled
assets load, the launch argument reaches `get_initial_file`,
`open_database`/`query_table` cross IPC, and the grid paints. The fixture's
`shout` column is `GENERATED ALWAYS AS (upper(name))` on purpose, and the job
asserts `ALICE` renders: a regression in which PRAGMA the backend introspects with
renders a grid one column short of its own rows, visible only in a real packaged
run. The script has a 10-minute overall deadline, a 120-second per-request
timeout, forwards and quotes the driver's stderr, and handles `driver.on("error")`
so a missing `tauri-driver` is a message rather than an unhandled exception.

**It does NOT catch removal of `connect-src ipc: http://ipc.localhost` from the
CSP, and that was measured, not assumed.** The directive was deleted on a
throwaway branch on 2026-08-05 and **all five jobs stayed green**, while a probe
in the same run showed a cross-origin fetch still blocked — so the CSP is
enforced and the `--debug` build is not the reason; wry's Linux IPC simply does
not travel over a channel `connect-src` governs. That trap is a
WebView2/WKWebView phenomenon, so closing the gap needs a **Windows leg**
(`tauri-driver` supports Windows via `msedgedriver`). Until one exists, treat
the `connect-src` line as guarded by review only, and do not cite this job as
cover for it.

Two things worth keeping from how that was found. **Falsifying a gate can
disprove its scope rather than its correctness** — the job worked exactly as
built; what was wrong was the comment claiming which defect class it stopped,
and a comment asserting protection that does not exist is worse than none,
because the next person reads it and stops checking. And **the first
falsification attempt is cheap**: one throwaway branch, with the branch added to
`on.push.branches` so no PR ref is created (PR refs are owner-undeletable — see
the `xlsxturbo` cleanup). Keeping the other jobs enabled during the experiment
is what turned "smoke went green" into "smoke went green *and* nothing else
caught it either", which is the more useful result.

## Security

### Security reporting, and what is in scope

- `SECURITY.md` is the public reporting policy. Reports go through **GitHub
  private vulnerability reporting**, which is **enabled** on the repo (it was off
  until 2026-08-05; a policy file pointing at a disabled channel sends reporters
  to a button that does not exist). Toggle with
  `gh api -X PUT|DELETE repos/tstone-1/dblitz/private-vulnerability-reporting`.

- No contact e-mail appears anywhere public, deliberately — the advisory flow is
  the channel, and it keeps the personal address off a public repo.

- In scope, and worth knowing when triaging: a bypass of **any** of the four
  read-only layers above, a crafted database file (the app's one genuinely
  untrusted input), updater signature verification, webview escape, and a file
  signed with the Windows certificate that the release workflow did not build.
  A SmartScreen warning on a signed build, and the unsigned Windows builds up
  to 26.10.0, are documented states, not findings.

## Release

### Five version files must agree

- Versions use CalVer `YY.M.MICRO`. **Five** files must agree: `package.json`, `package-lock.json` (top-level *and* `packages[""]`), `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (the crate's own entry) and `src-tauri/tauri.conf.json`. Nothing fails when any of them drifts, which is how 26.7.6 shipped on a stale lockfile and how `Cargo.lock` was still on 26.8.1 while 26.8.2 was staged.
- **`node scripts/bump-version.mjs <YY.M.MICRO|next> [--release [date]]` sets all six sites** (since 2026-10-29). It edits text, so formatting stays; every JSON edit is re-parsed and compared with everything but the version fields, and the `Cargo.lock` edit is confined to the crate's own `[[package]]` block, the one line `cargo check` would write. It then runs `release-preflight.mjs`'s checks on what it wrote: `checkVersionFiles` without `--release`, the full tag contract with it. CHANGELOG: a pending `## [x] - Unreleased` heading is renamed to the new version (more than one is refused); `--release` dates it and refuses when the version has no section. Its main-module guard is the preflight's exported `isMainModule`, for the reason under *A script's main-module guard is part of the gate*. `src/lib/bumpVersion.test.ts` runs it on the real files and from a path with a space.
- **A test that reads the repository's own files must pass at every point of the release cycle.** The first release with the bump script failed CI's `quality` job: three tests ran `--release` on the real CHANGELOG and assumed a pending `Unreleased` section, which the release commit itself dates away. The local gate ran before the bump and was green. They now add a pending section to their copy when there is none, and bump to `99.1.0`, a version the repository will not reach. Run the full gate once more *after* `bump-version.mjs --release`, on the tree the tag will point at.

### The `preflight` job proves the tag names the version

- **The `preflight` job proves the tag names the version being published, and `create-release` needs it.** Everything else in `release.yml` is thorough about what it builds and signs, and none of it looks at the tag — so a `v26.8.2` tag on 26.8.1 manifests would publish 26.8.1 installers and updater metadata as 26.8.2 with nothing red anywhere. `scripts/release-preflight.mjs` checks CalVer and all five version files plus a **dated** `CHANGELOG.md` heading, before the draft release exists. It is dependency-free on purpose so an install failure cannot take the gate out; run it locally with `node scripts/release-preflight.mjs vYY.M.MICRO`. The `Cargo.lock` entry is looked up by the name `Cargo.toml` declares, and the `name =` match is anchored to the start of a line inside each `[[package]]` block — a substring search finds the crate again inside another package's `dependencies = [...]` array and reports a phantom duplicate. `src/lib/releasePreflight.test.ts` pins each version file failing on its own, plus a positive control.

### A script's main-module guard is part of the gate

- **A script's main-module guard is part of the gate.** That guard used to be a string comparison of `import.meta.url` against a `file://` URL built from `process.argv[1]`, which is false from any path containing a space (percent-encoding) or reached through a symlink (realpath): the script then exited 0 having run no check at all, and the workflow read that as a pass. It is now `import.meta.main` with a realpath fallback, and the workflow additionally greps the output for `^[OK] Release preflight`, failing with an explicit error otherwise. Any new gate script gets both halves. Pinned by `releasePreflight.test.ts` ("the preflight CLI actually runs from an awkward path") and `releaseWorkflow.test.ts` ("blocks draft creation on it").

### The `Unreleased` heading form is safe here

- The `## [version] - Unreleased` heading form is safe **here** — checked 2026-08-23: `release.yml` writes fixed notes pointing at CHANGELOG.md and never slices a section out of it by version heading, so a forgotten rename cannot publish a release whose notes say "Unreleased". Re-check that if the release notes ever become generated. Since 26.8.2 a forgotten rename is also **caught**: preflight requires a `YYYY-MM-DD` date on the heading for the tagged version.

### A long release run is not necessarily stuck

- A full release run takes about two hours: 132 to 137 minutes for 26.10.0, 26.10.2 and 26.10.3 (measured 2026-10-29), because the build legs run one at a time (`max-parallel: 1`) and each compiles the bundled DuckDB. A cold Rust cache adds more. Confirm the active job step before treating a long run as stuck.

### The draft release needs the numeric `releaseId`

- The draft-first release workflow must pass the numeric `releaseId` from `create-release` to `tauri-action`; GitHub's tag lookup returns 404 for a draft release, so reverting build uploads to `tagName` breaks the matrix.

### Expected `cargo audit` noise

- Expected `cargo audit` noise is the established allowed Tauri/Linux WebView transitive set (legacy GTK/glib/unic advisories). Treat any new advisory or materially different output as actionable; the npm `cookie` override (`^0.7.2`) that answered an old finding on the unreachable SvelteKit SSR path was removed with SvelteKit 3, which needs `cookie` 2; re-evaluate a `cookie` finding if one reappears.
- **`npm audit` reports 4 low `deepmerge` findings, and they have no fix** (measured 2026-10-29). GHSA-gcqx-w48m-w2h4 covers every `deepmerge` from 4.2.0 up; `@sveltejs/vite-plugin-svelte` uses it to merge config at build time, and nothing of it ships in the app. The "fix" `npm audit fix --force` offers is a downgrade to `@sveltejs/kit` 0.0.30: never run it. Re-check when `vite-plugin-svelte` drops or patches the dependency.

### In-app updater

- **The updater's minisign private key is the most safety-critical secret in this repo.** It lives in KeePass and in the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets; the public key is committed in `tauri.conf.json`. It is a **dblitz-only** key — deliberately not shared with `screenpick`. Losing it permanently orphans every installed copy: a new key cannot sign for clients holding the old public key, so each user would have to reinstall by hand. It is unrelated to OS code signing and is not fixed by adding an Apple Developer ID. Never print, echo, or paste the private key into a tool call; custody and regeneration live in [BUILD.md](../BUILD.md#updater-signing-key).
- **A release with no `latest.json` updates nobody, and looks green.** When the signing secret is missing or empty, `tauri-action` logs "Signature not found for the updater JSON. Skipping upload..." and still succeeds. The release ships with installers and no manifest, and the failure only surfaces as users silently never updating. BUILD.md's post-release checks exist for this.
- **The release matrix must stay `max-parallel: 1`.** `tauri-action` builds `latest.json` by read-modify-write against the release asset, so parallel legs clobber each other's platform entries and produce a manifest that updates only some platforms. dblitz has five legs (two of them macOS, two Windows), so this bites harder here than in a two-leg repo. The `publish` job reads `latest.json` back on the draft and refuses to publish unless all five platforms are in it. Nothing fails; the manifest is just incomplete.
- **Not every install can self-update, and the UI must not pretend otherwise.** The updater hands its payload to an installer, and neither a package-managed Linux install nor a standalone Windows exe has one that owns this copy. `InstallProvenance` in `updates.rs` enumerates all five artifacts and `self_update_supported` matches exhaustively, so a new artifact cannot inherit a default answer. Do not "fix" the gate by always offering the install. Pinned by `updates::tests::{macos_can_always_self_update, installed_windows_build_can_self_update, portable_windows_build_does_not_offer_install, linux_appimage_can_self_update, linux_deb_or_rpm_cannot_self_update}`.
- **Windows provenance is read from the registry, not guessed from the path.** `windows_install_provenance` in `platform/windows.rs` compares the running executable's directory against the `InstallLocation` the NSIS installer recorded under `Software\Microsoft\Windows\CurrentVersion\Uninstall\dblitz` (HKCU per-user, HKLM per-machine; the value is stored **with** surrounding quotes, so it must be trimmed). Path identity is the test rather than the key merely existing: a user can have dblitz installed *and* be running a portable copy out of Downloads. Anything unreadable reports portable, because the two failure directions are not symmetric — wrongly claiming portable costs one manual download, while wrongly claiming installed runs the NSIS installer, which cannot find the standalone exe and instead creates a *second*, installed copy at the new version while the file the user launches stays old. The last segment of that key is `productName` and nothing at build time couples them (`uninstall_key_tracks_the_bundle_product_name`).
- **`updates.rs` is deliberately pure** — no `tauri::` imports, no env reads, no `cfg!`, no registry access. Everything host-dependent is resolved in `lib.rs` and arrives as one `InstallProvenance`, which is what lets the Linux *and* Windows gates be unit-tested from any machine. Keep it that way. The `allow(dead_code)` on the enum is there because every build constructs exactly one variant; the tests construct all five.
- **`ConfigStore::record_run_version` is destructive by design.** It returns the previous version and immediately overwrites it, so "did we just update?" is only answerable at the moment it is called. `lib.rs` calls it once in `setup` and caches the result as `UpdateStatus` app state; a command that re-derived it on demand would always answer "no" (`record_run_version_is_a_no_op_on_an_unchanged_relaunch`).
- **New `AppConfig` fields need `#[serde(default)]` and a matching manual `Default`.** `load_app_config` falls back to `AppConfig::default()` on any parse error, so a field without a serde default makes every older `app.json` unparseable and silently wipes the user's recent-files list. And because `check_for_updates_on_startup` defaults to `true`, `Default` is hand-written — a derived one would give `false` and diverge. Pinned by `config::tests::{app_config_default_matches_its_serde_default, pre_updater_app_config_still_parses_and_keeps_recents}`.
- **A local updater test build shares the real app's bundle identifier** (`com.tstone.dblitz`), so it writes into the real `app.json`. Clear `last_run_version` afterwards or the next real launch believes it just updated.

### Distribution and the Homebrew tap

- Pushing a `v*` tag triggers `.github/workflows/release.yml`: it runs preflight and the quality gate, creates the GitHub release, builds/uploads artifacts (macOS `.dmg` for `aarch64` + `x64`, Windows, Linux), then the `update-tap` job auto-bumps the Homebrew cask.
- The macOS app is distributed via the Homebrew cask `dblitz` in the tap repo **`tstone-1/homebrew-dblitz`** (`Casks/dblitz.rb`). The cask URL pattern is `dblitz_<version>_<arch>.dmg` with `arch arm: "aarch64", intel: "x64"`.
- `update-tap` downloads the two macOS DMGs from the release, computes their `sha256`, and `sed`-edits `version` + both `sha256` lines in the tap's `Casks/dblitz.rb`, then commits/pushes `Bump dblitz cask to v<version>` as `tstone-1`. It pushes to a *different* repo, so it uses the **`TAP_GITHUB_TOKEN`** secret (a fine-grained PAT with Contents:read/write on `tstone-1/homebrew-dblitz`) — the default `GITHUB_TOKEN` cannot. If that secret is missing/unauthorized the `update-tap` job fails (but the build/release still succeed); re-set it with `gh secret set TAP_GITHUB_TOKEN --repo tstone-1/dblitz < <file>` (there is no `--body-file` flag; feed the value on stdin. The interactive prompt does NOT work through a non-interactive shell — it silently stores an empty value).
- **The one-line description lives in five places, and nothing checks that they agree**: the GitHub repository's About (`gh repo edit --description`), the cask's `desc` (which `update-tap` leaves alone, so it is edited in the tap repo by hand, never while a release's `update-tap` runs), `package.json`, `src-tauri/Cargo.toml` and the README's first line; SECURITY.md's scope paragraph names the formats too. When CSV and JSON support shipped in 26.10.4, only the README was updated, and the cask still said "SQLite browser" from before Parquet.
- `Casks/dblitz.rb` carries **`auto_updates true`** (added with the in-app updater). Without it Homebrew and the updater fight: `brew upgrade` reinstalls over a self-updated app, and `brew` reports the cask as outdated forever. This must survive the `update-tap` job — that job only `sed`-edits the `version`/`arm:`/`intel:` lines, so it does.
- The tap is a personal/untrusted tap: first use on a machine needs `brew trust --cask tstone-1/dblitz/dblitz`. Install/upgrade the local app with `brew install --cask dblitz` / `brew upgrade --cask dblitz`. To overwrite a pre-existing non-brew install, use `brew install --cask --force dblitz` (`--adopt` only works when the on-disk version already matches).

### macOS signing and notarization

**Since 26.7.6** the macOS build is signed with a Developer ID identity and notarized by Apple. Full setup, credential storage, and the traps hit while getting there are in [BUILD.md](../BUILD.md#macos-code-signing-and-notarization). What matters when editing CI:

- The signing identity is **team-wide, not per-app**: `Developer ID Application: Timo Stein (NVX72G8SJ8)`, the same certificate and the same App Store Connect `.p8` notarization key that `screenpick` uses. A Developer ID cert certifies a team, never an app, and Apple caps the account at 5 of them — so a second app reuses the first one's material rather than minting its own. Consequence: **rotating that certificate is a two-repo event.** Both repos' `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` / `APPLE_SIGNING_IDENTITY` secrets have to be updated together, or the repo you forgot silently drops to unsigned-and-blocked on its next release.
- Six repo secrets, macOS legs only: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`.
- **The signing vars are exported via `$GITHUB_ENV`, never listed in the build step's `env:` block.** A fork has none of these secrets; `env:` would hand the Tauri CLI an *empty* `APPLE_SIGNING_IDENTITY`, which it reads as "sign with this identity" and fails on. The conditional export step leaves them genuinely unset, so the CLI falls back to the `"signingIdentity": "-"` in `tauri.conf.json` and a fork still builds.
- **A skipped notarization exits 0.** Missing or malformed credentials make the bundler log `skipping app notarization` and succeed, shipping a signed-but-unnotarized app that Gatekeeper rejects on any machine that has never seen it — looks green, is broken. `release.yml` therefore ends each macOS leg with a verification step gating on `Authority=Developer ID Application`, the `runtime` flag, `stapler validate`, and `source=Notarized Developer ID`. Do not weaken that gate.
- **The signing gates fail CLOSED in `tstone-1/dblitz`, and that is the whole point of the shape.** Until 26.8.1 the preparation step checked two of the six Apple values and warned-and-exited-0 otherwise, while the notarization and verification steps were conditioned on `env.APPLE_SIGNING_IDENTITY != ''` — so dropping any one secret skipped precisely the gates written to catch a dropped secret, and `publish` still ran. All six are now required when `GITHUB_REPOSITORY` is `tstone-1/dblitz`, the verification step's condition also fires on `github.repository == 'tstone-1/dblitz'` so it cannot skip in its own failure case, and forks keep the ad-hoc unsigned path. **A gate that cannot run in the case it exists for is not a gate.** Pinned by `releaseWorkflow.test.ts` ("requires every Apple secret, not just the certificate and the key", "refuses to continue in the canonical repo when one is missing", "still builds ad-hoc signed in a fork", "runs the artifact verification on every canonical macOS leg").
- **The bundler notarizes the `.app`, not the `.dmg`.** After a signed build the DMG carries a Developer ID signature but no ticket, and `spctl -a -t open` rejects it as `Unnotarized Developer ID`. Since the DMG is what users download, `release.yml` notarizes and staples it in a separate step and re-uploads it over the asset `tauri-action` published (`gh release upload --clobber`, which resolves the draft by tag). That step runs **per matrix leg**, so both the `aarch64` and `x64` DMGs get their own submission — a shared/universal path would silently staple only one arch.
- Ordering that must hold: staple happens in the `build` job, `update-tap` hashes the DMG after `publish`, so the cask's `sha256` is of the **stapled** bytes. Moving the staple later, or the hash earlier, produces a cask whose checksum never matches the published asset.
- Updates inherit the signature: the updater bundler tars the already-stapled `.app` without re-signing, and the staple ticket lives at `Contents/CodeResources` (an ordinary file, not an xattr), so it survives the tar.
- Windows is signed with a different certificate by a different mechanism: see *Windows code signing* below. A Developer ID does nothing for SmartScreen.
- The tap cask's `postflight` quarantine strip (added 2026-07-07 as the free workaround for the "damaged" error) was **removed** from `Casks/dblitz.rb` on 2026-07-25, once 26.7.6 proved a notarized app passes Gatekeeper with the quarantine flag intact. Do not reintroduce it: stripping the flag discards provenance, and needing it again would mean notarization has silently broken — which is the thing to investigate, not paper over.

### Windows code signing

**Wired up 2026-10-08; the release after 26.10.0 is the first signed one.** The certificate, the method, the setup commands and the fallback by hand are in [BUILD.md](../BUILD.md#windows-code-signing), and the measurements behind the method are in `tpdf`'s `BUILD.md`, where it was worked out. What matters when editing CI:

- **The certificate is the maintainer's, not the app's**: Certum Open Source Code Signing, subject `CN=Open Source Developer Timo Stein`, valid until 2027-10-07, shared with `tpdf` and `screenpick`. The key is in Certum's SimplySign service and cannot be exported; the workflow holds a login (`CERTUM_EMAIL`, `CERTUM_OTP_URI`), not a key. Never print either value, and never write the address or the `otpauth://` URL into a file here.
- **The login lives in the GitHub environment `signing`, not in the repository secrets**, and only the two Windows legs name that environment: `environment: ${{ matrix.signs-windows && 'signing' || '' }}`. The empty string is no environment, so the Linux and macOS legs cannot read the login at all. Do not simplify that to `environment: signing`; it would hand the login to all five legs.
- **`signs-windows` is a flag in the matrix, and every path is built in the step.** `github.workspace` is empty inside a matrix, so a path assembled there starts at the drive's root.
- **The sign command is an overlay (`src-tauri/tauri.signing.conf.json`), passed with `--config` on the Windows legs only.** In `tauri.conf.json` it would make every local Windows build ask for the login.
- **`scripts/sign-windows.cmd` finds its script through `DBLITZ_SIGN_SCRIPT`, never through `%~dp0`.** makensis starts it by quoted name through `PATH` from its own folder to sign the uninstaller; with `%~dp0` that call fails, makensis prints `UninstFinalize command returned 64` and goes on, and the release ships an unsigned uninstaller with everything else signed and the build green.
- **`scripts/sign-windows.ps1` signs into a separate folder and copies back with retries.** `ssign` replaces a file by rename, which fails with "Access is denied" while Tauri still has the executable open. It also keeps the log, because Tauri prints nothing of a sign command that failed.
- **One login at a time.** A one-time code is good for one login, and repeated failed logins can lock the account. Both Windows legs and `sign-rehearsal.yml` share the concurrency group `certum-signing` with `cancel-in-progress: false`; the other three legs get a group of their own so they are not held up. The ARM leg's signing client is compiled in every release: the rehearsal runs on x64 only, and the cache key carries `runner.arch`, so it never finds an ARM copy. The group ends at the repository: a `tpdf` or `screenpick` release signing at the same moment is invisible to it.
- **`ssign` is installed into its own folder under `RUNNER_TEMP`** (`cargo install --root`), not into `~/.cargo/bin`, which `Swatinem/rust-cache` saves and restores. The built client is kept in a cache of its own (`actions/cache`, key `ssign-<os>-<arch>-<SSIGN_REV>`, no `restore-keys`), so the commit decides which copy is used and the Rust cache, whose keys do not name it, never holds it. A tag can read only a cache saved on `main`, so the copy a release uses is the one `sign-rehearsal.yml` saved there; a release that finds none builds the client. `ssign --version` and the copy of `sign-windows.cmd` run on a hit too. `SSIGN_REV` is written once in each workflow, in the job's `env:` (a step's own `env:` is not visible to the cache step), and must be the same commit in both.
- **The session file is removed in an `if: always()` step**, and the signing log is printed in another. Both must stay `always()`: the failing run is the one that needs them.
- **The verification step is the gate, and it runs before `Upload portable exe`.** It reads the installer and `src-tauri/target/release/dblitz.exe` (the file that step uploads; Tauri restores the unsigned executable after packing, so the step `Sign the portable exe` signs it after the build and before the session is removed), then installs silently, reads the install folder from the registry key the installer writes, and checks every executable file there, found by their `MZ` bytes and not by a list of names. `uninstall.exe` exists only after an install, which is why the step installs.
- **No MSI.** `src-tauri/tauri.windows.conf.json` sets `bundle.targets` to `["nsis"]` for Windows; `tauri.conf.json` keeps `"all"` for the other platforms. `ssign` signs PE files only. `updaterJsonPreferNsis: true` stays as it is.
- **The build job's checkout has `persist-credentials: false`**, because that job now compiles a program from another repository. Every step that talks to GitHub gets its token through `env`.
- All of the above is held by `src/lib/windowsSigning.test.ts`, each assertion shown to fail on its own mutation when it was written. None of it can run on a development machine; the first real proof of the leg is a tag.

### Windows on ARM is built natively on `windows-11-arm`

**Windows on ARM is built natively on `windows-11-arm`, not cross-compiled.** Without `--target` the output stays in `target/release`, which every Windows step reads, and the verify step can run the installer it checks; it also asserts the PE machine field (`0xAA64`), because a runner of the wrong architecture would otherwise publish an x64 build under the arm64 name. tauri-action names the updater entry from the runner's architecture (`windows-aarch64`). The ARM portable exe is `dblitz-arm64.exe`; x64 keeps `dblitz.exe` because existing links point at it. `checks.yml`'s backend matrix has the same runner, so an ARM compile failure shows on push rather than on a tag. Read [BUILD.md](BUILD.md#windows-code-signing) before touching the leg, and [docs/agent-notes.md](docs/agent-notes.md) for what must not be simplified away.
