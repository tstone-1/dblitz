# dblitz Agent Notes

## Project

- `dblitz` is a public `tstone-1` repository for a Tauri + SvelteKit + TypeScript desktop SQLite browser.
- Treat the repository as public: do not add internal company data, private paths, secrets, or proprietary examples.
- Use a public-safe Git identity for commits; do not commit with private or company email addresses.

## Development

- Node is pinned to 24 in `.nvmrc`, and every CI `setup-node` step reads it via `node-version-file` so there is a single source of truth. `@types/node` is deliberately held on the matching major — `npm outdated` will keep offering a newer one; taking it means type-checking against an API surface the shipped runtime does not have. Bump `.nvmrc` and `@types/node` together or not at all.
- **A green local gate can be measured against the wrong dependency.** `node_modules` drifts out of the declared range without anything failing: on 2026-08-05 it held `@types/node` **26.1.1** while `package.json` said `^24.13.3` and the lockfile said `24.13.3`, so `npm run check` type-checked against an API surface CI never uses — and reported 0 errors either way. `npm update` (or `npm ci`) reconciles it. Before trusting a type-check that a release depends on, confirm what is actually installed: `node -p "require('./node_modules/@types/node/package.json').version"`.
- Install frontend dependencies with `npm install`; run the desktop app with `npm run tauri dev`.
- `npm run quality` is the whole gate and is what CI runs: `npm run check`, `npm test`, `npm run build`, then in `src-tauri` `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings`. Use the bare clippy invocation with `-D warnings` — a plain `cargo clippy` can pass locally and still fail CI.
- **Type-checking runs on TypeScript 7 through `svelte-check --tsgo`, and TypeScript 6 stays installed on purpose.** svelte-check still loads TypeScript's JavaScript compiler API, which TS 7 does not have, so `typescript` stays `^6` and TS 7 is the alias `@typescript/native`. Remove neither. `npm run check` goes through `scripts/svelte-check.mjs`, because svelte-check exits 0 when it crashes (a missing TS 7 included), and the wrapper fails any run without the `COMPLETED` summary line (`svelteCheck.test.ts`).
- **Vitest's global environment is `node`.** The two test files that need a DOM opt in per file with `// @vitest-environment jsdom` on line 1: `sqlEditorExtensions.test.ts` and `dragReorder.test.ts`. Keep it per file: a global jsdom would hide that most of this suite is pure logic. jsdom 30 has no `document.elementFromPoint`, which `dragReorder.test.ts` supplies and asserts is still absent.
- Use `npx tauri build` for local release builds. macOS DMG packaging may need to run outside a sandbox because Tauri invokes system image mounting tools.
- **A green gate on one OS says nothing about a `cfg`-gated symbol on another, and `-D warnings` turns that into a hard build failure.** Hit cutting 26.8.1: `use updates::{InstallProvenance, UpdateStatus};` compiled clean on Windows and failed the Linux release leg with `error: unused import: InstallProvenance` — the only two places naming that type were `cfg(windows)` and `cfg(target_os = "macos")`, so on Linux the import was genuinely unused. Windows clippy structurally cannot see it. The tag had already been pushed; nothing published, because the `quality` job runs before `create-release`. Two habits: annotate the type on every arm of a `cfg` triple (`let provenance: InstallProvenance = ...`) so the name is used everywhere, and **check the other platform before pushing a tag** rather than after. `checks.yml`'s backend job covers all three OSes but only on push/PR, which a same-moment tag push races.
- **A Windows machine can run the Linux gate for real, through WSL**, which is the cheapest way to close half of the gap above. Install the same apt deps `.github/tauri-linux-deps.txt` lists plus rustup stable, then run the gate against the checkout over `/mnt/...`, giving it its own target dir so it cannot thrash the Windows one:
  ```bash
  export CARGO_TARGET_DIR="$HOME/dblitz-linux-target"
  cd /mnt/<drive>/<path-to>/dblitz/src-tauri
  cargo clippy --all-targets --all-features -- -D warnings && cargo fmt --check && cargo test
  ```
  Linux runs **6 fewer** Rust tests than Windows, and nothing runs on Linux that does not also run on Windows. The six are the `cfg(all(test, windows))` block in `lib.rs` — `uninstall_key_tracks_the_bundle_product_name`, `path_hash_ignores_case`, `path_hash_distinguishes_directories`, `path_hash_distinguishes_filenames`, `path_hash_is_nonzero_for_real_paths` — and `config::tests::normalize_for_dedup_is_case_insensitive_on_windows`. Diff the two `cargo test` name lists if that gap ever changes; do not compare totals, which move with every added test.
- **There is no Windows cross-check from macOS.** `cargo check --target x86_64-pc-windows-msvc` cannot run there: `ring` and `libsqlite3-sys` have C build scripts and the MSVC toolchain does not exist on macOS. The WSL recipe above covers Linux only. So `cfg(windows)` code changed on a Mac is verified by reading plus `checks.yml`'s Windows leg, and nothing else — push the branch and read that job before tagging.

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

- Frontend code lives under `src/`; Tauri/Rust backend code under `src-tauri/`.
- SQLite backend code lives under `src-tauri/src/db/`, with `src-tauri/src/db.rs` as a thin facade re-exporting the submodules:
  - `schema.rs` — table/column introspection, the open batch, and row counts
  - `query.rs` — table paging, the rowid-index fast path, and regex filtering
  - `filters.rs` — the `WHERE` clause builder and column-filter operator parsing
  - `sql.rs` — arbitrary SQL execution plus the read-only / ATTACH-DETACH rejection gate
  - `export.rs` — XLSX export
  - `types.rs`, `util.rs` — shared DTOs and helpers (`safe_ident`, `read_row`, `render_real`, `StrErr`)
  - `bench_api` (in `db.rs`) — the one seam `src-tauri/examples/*.rs` may use. The two benchmarks call the shipped functions through a real `open_database` connection; they used to be re-implementations that could drift from what ships. Keep it that way when adding an example.
- **All column discovery goes through `PRAGMA table_xinfo`, never `table_info`, and `schema::table_columns` is the one place that reads it.** `table_info` omits generated columns; `SELECT *` returns them, and that mismatch corrupts paging silently. `table_columns` returns `visible` (`hidden` 0/2/3, exactly what `SELECT *` returns, in its order) and `declared` (every name, `hidden=1` included, used only for rowid-shadow detection, where erring wide costs the fast path and erring narrow corrupts data). Pinned by `query::tests::{generated_columns_are_reported_and_returned, regex_filter_index_survives_a_preceding_generated_column, generated_rowid_column_shadows_the_alias, generated_rowid_column_does_not_corrupt_paging}`, `schema::tests::get_columns_lists_generated_columns_with_contiguous_cids`, and the CI smoke fixture's `GENERATED ALWAYS` column.
- Every non-trivial view (sorted, filtered, regex-filtered) pages from **one** cached ordered-rowid list per table, built once and sliced per chunk. Its identity is `OrderKey`, and the `regex_signature` field in it is load-bearing rather than decorative: a regex is evaluated in Rust and never reaches the SQL, so two views with different patterns produce an **identical** `where_clause`, `params` and `order_clause`. Without that field one pattern's match set is served as another's, and clearing the regex keeps serving the narrowed set. Tables with no addressable rowid (WITHOUT ROWID, or all three aliases shadowed) keep the full-scan LIMIT/OFFSET fallback.
- A regex scan selects only the columns a pattern is applied to (`project_regex_columns`), and the filter indices it hands on are indices into **that projection**, not into the table. Semantics, each with its own test in `query.rs`: NULL matches as the empty string, a BLOB never matches, a number matches the text the grid renders, and an unknown column name is an error rather than a silently dropped filter that shows every row (`regex_treats_null_as_the_empty_string`, `regex_never_matches_a_blob`, `regex_on_a_numeric_column_matches_the_text_the_grid_shows`, `regex_on_an_unknown_column_errors_instead_of_showing_every_row`, `regex_filter_matches_the_named_column_when_it_is_not_the_first`, `two_regex_filters_apply_to_their_own_columns`).
- Chunk 0 of an unsorted table is served with `LIMIT` and the rowid index is built on the first `offset > 0` (`first_chunk_is_served_without_building_the_rowid_index`). Unfiltered row counts are cached in `DbState::table_counts`, seeded at open and cleared in `clear_caches`; that is sound **only** because of `?immutable=1` (`open_seeds_the_row_count_cache_and_pages_read_it`, `a_view_pages_without_recounting_every_chunk`). Column introspection runs once per `query_table` (`one_column_pragma_per_query`). A crafted `COUNT(*)` cannot abort on allocation (`preallocated_bounds_a_crafted_row_count`).
- **REAL cells render as SQLite renders them** (`util::render_real`: `3.0`, `1.0e+20`, `-0.0` as `0.0`), so a filter typed against what the grid shows matches. `render_real_matches_sqlite_cast_as_text` checks it against SQLite itself in two classes — byte-identical up to 13 significant digits, same-double beyond, because SQLite's `%!.15g` last digits are approximate. On a column with **no** declared affinity `=<number>` also compares numerically (`filters::tests::exact_match_on_a_column_with_no_affinity_also_compares_numerically`); typed columns are untouched.
- `dblitz` is a **strict read-only SQLite viewer** by explicit decision (2026-04-10). Reject feature requests that imply mutation (inline cell edit, delete row, save-as, schema changes) and surface the decision before implementing. Enforcement is in **four** layers — preserve all four when changing query execution or database opening:
  1. Connections open with `SQLITE_OPEN_READ_ONLY`.
  2. Plus `?immutable=1` in the URI: the file is a frozen snapshot for the connection's lifetime, and no `-wal`/`-shm` companion files are ever created (source files may sit in cloud-synced folders other tools write to).
  3. `execute_sql` rejects non-readonly prepared statements (`stmt.readonly()`), and a SQLite authorizer denies ATTACH/DETACH and transactions at the engine level.
  4. `ALLOWED_INTROSPECTION_PRAGMAS` in `schema.rs`: the open batch and any PRAGMA path accept only read-only introspection pragmas. This is why `PRAGMA journal_mode=wal` is refused before the authorizer ever sees it.
  - `sql::tests::read_only_connection_refuses_a_write_the_allowlist_does_not_stop` pins "dblitz cannot write" but deliberately not *which* layer stops it: removing `READ_ONLY` alone, or `immutable` alone, leaves it green. Both must go for it to fail, so a layer deleted on its own is caught by review, not by that test.
  - Accepted trade-offs: dblitz does not see live writes from other processes during a session (reopen the file to pick up changes), and a file another process truncates mid-session surfaces as a read error ("database disk image is malformed") rather than a crash. Backend row/index caches need no invalidation because the snapshot is frozen.
- **`PRAGMA mmap_size` must not come back.** With `immutable=1` there is no locking, so a file shrinking under an open mapping killed the process with SIGBUS and printed nothing. Removing it turns that into the recoverable error above, and it was not even a win: mmap only helped repeated warm `COUNT(*)`, and cold every measured operation was slower with it. `schema::tests::open_database_does_not_memory_map_the_file` asserts `PRAGMA mmap_size` reads 0; the reasoning is in the comment at the open batch.
- **There are two read-only connections to the same file.** `conn` serves `query_table`/`count_rows` and owns the table-keyed caches; `aux_conn` serves `execute_sql`, `get_schema` and `get_columns`, so the SQL and Structure tabs do not queue behind a multi-second sort (0.02 ms against 734 ms during a 0.8 s sort of 5,000,000 rows). Two connections agree only because of `?immutable=1` — one frozen snapshot — so removing `immutable=1` means removing the second connection in the same change. Both are built by `schema::open_read_only_connection` (same flags, URI, open batch and authorizer, so they cannot drift), both are interrupted by `cancel_queries` and dropped by `close_database`, and `open_database` is the only site taking more than one lock: the order is `conn` -> `aux_conn` -> `table_counts`. Pinned by `sql::tests::execute_sql_does_not_wait_for_the_browse_connection`, `sql::tests::cancel_interrupts_the_secondary_connection` and `schema::tests::secondary_connection_enforces_every_read_only_layer`.
- **Windows duplicate-instance detection is keyed by a 64-bit hash of the case-folded absolute path**, held in the `dblitz_db_path` window property (`SetPropW` stores one pointer-sized value, which is why it is a hash and not the string). It is not filename-only matching: same-named databases in different directories must open separately. Both the writer and the reader take their path from `launch_argument()`/`resolve_launch_argument()`, so no caller can hash a raw `argv[1]` against a marker holding the resolved path (`lib.rs::tests::every_caller_of_the_launch_argument_gets_the_absolute_path`). The `FlashWindowEx` call that follows `SetForegroundWindow` in `try_activate_existing` is deliberately unconditional and must stay: activation is usually denied by the foreground lock, and without the flash the second launch exits silently and reads as "dblitz refuses to open this file". See the comment there for why the outcome cannot be tested reliably.
- **A launch argument is the one path that arrives relative** — dialog, drag-and-drop, recents and macOS document-open events are all absolute. `absolutize_launch_path` resolves it in `lib.rs` rather than inside `path_to_sqlite_uri`, because the absolute form is also what lands in recents, is hashed into the config key, and is compared by the duplicate-instance check.
- **A view-config save carries its own database path; the backend must never pick one from mutable state.** `save_view_config` is `#[tauri::command(async)]`, so it runs at a time the frontend does not control while `state.current_path` moves. `store.svelte.ts` serializes saves on their own chain (separate from `databaseRequestQueue`, so an autosave per drag frame cannot block an open) and captures the path *and* a detached JSON copy of the config at enqueue time — `$state.snapshot` returns the same object under this repo's vitest setup, so later edits still reached the queued save. The backend writes to the path it was handed: a save legitimately outlives its session, and the path is a lookup **key**, not a destination. Pinned by `store.svelte.test.ts` ("saves under the database that was open when the change was made", "serializes saves so an older snapshot cannot overwrite a newer one", "captures the config at enqueue time") and `config::tests::{save_config_keys_strictly_off_the_db_path_it_is_given, config_path_for_db_stays_inside_the_config_dir}`.
- **The config key is `sha256(normalize_path_key(path))[..16]`** — case-folded and slash-normalized on Windows, byte-exact everywhere else, so one file cannot own two settings files. `ConfigStore` carries a `windows_path_semantics` flag so the Windows behaviour is tested on any OS. Reads fall back to the pre-normalization raw-path key and migrate on the next save, removing the old file (`view_config_key_folds_windows_path_spellings_together`, `unix_paths_stay_case_sensitive`, `load_config_falls_back_to_the_pre_normalization_key_then_migrates`).
- **An `await` in a component publishes only if it still owns publication.** Two facts, both captured BEFORE the await: a monotonic per-caller token and `appState.dbOpenGeneration`. The token alone lets a closed database's answer repopulate a reset panel; the generation alone lets two requests inside one session finish out of order and label table B with table A's columns. `createSessionOwnedRequest` in `sessionOwnedRequest.ts` is the seam — a publication guard, not a request bus: it cancels nothing, the in-flight call still completes and its answer is dropped, and its errors are gated too. Users: `DatabaseStructure.svelte` and `createSqlExecution` in `sqlExecution.ts` (a SQL result from a closed or replaced session is dropped, writes no history entry, and clears `running`). `store.svelte.ts` and `virtualRows.svelte.ts` enforce the same rule inline. Pinned by `sessionOwnedRequest.test.ts` and `sqlExecution.test.ts`.
- **A column's display NAME is not its identity, and the clipboard is not a `join`.** `SelectionData` carries `columnIndices` and types are recovered positionally, because a result set may legitimately repeat a name (`SELECT a.value, b.value` typed `INTEGER, TEXT`). Serialization lives in `clipboardTable.ts` and puts both an HTML table and RFC 4180-quoted TSV on the clipboard, so a cell holding a tab or newline cannot become extra columns or rows. Neither form is byte-exact in every target — a tab survives exactly only in the plain-text form — but the pasted table always has exactly the selected rows and columns. Pinned by `selectionData.test.ts` ("records the source column index of every header", "resolves a later duplicate column name to its own declared type") and `clipboardTable.test.ts` ("round-trips cells containing tab, LF, CRLF and quotes").
- Keep the window title filename-only; the toolbar owns display of the full database path.
- **There are two column-reorder implementations and only one of them has ever been proven in WebView2.** `DataGrid`'s header reordering uses mouse events with a movement dead zone and document-level cleanup (`dragReorder.svelte.ts`, tests in `dragReorder.test.ts`); `ColumnSettings.svelte` uses HTML5 drag-and-drop with `dataTransfer`, driving the same `onReorder`. Do not replace the mouse-event path with HTML5 DnD without proving it in the Windows WebView2 runtime — and note the `ColumnSettings` path carries exactly that unproven risk today.
- Treat DB Browser for SQLite as the primary UX comparison point when evaluating viewer behavior and parity gaps.
- In `DataGrid.svelte`, compose new per-column state indicators with inset box shadows rather than background tints so user-selected column colors remain visible.
- Files that use Svelte runes outside a component must use the `.svelte.ts` extension. Keep template and `$derived` reads side-effect-free — selection statistics are computed in a debounced `$effect` over already-cached rows for exactly this reason. Create missing state only from event handlers or other explicit mutation paths.
- Preserve `connect-src ipc: http://ipc.localhost` in the Tauri CSP. Removing it can leave production IPC broken while development still appears healthy.
- Tauri and the direct `windows` dependency can resolve to different `windows-rs` versions. `window.hwnd()` must be rewrapped as `HWND(hwnd.0)` at that boundary; other HWNDs sourced from `EnumWindows` do not need blanket conversion.

## CI

`.github/workflows/checks.yml` runs frontend checks, a Rust backend matrix
(ubuntu/windows/macos, clippy `-D warnings` on all three), and a `smoke` job.
`.github/workflows/release.yml` runs on a `v*` tag. Both are pinned and bounded;
`src/lib/releaseWorkflow.test.ts` and `checksWorkflow.test.ts` fail on any drift.

- **Every `uses:` in both workflows is pinned to a full 40-character commit SHA with a `# vX.Y.Z` comment.** The release build job holds the updater's minisign private key, the Apple credentials and a contents-write token at once, and a mutable ref decided which code received them — note `dtolnay/rust-toolchain@stable` is a **branch**, which the shorthand disguises as a toolchain channel. Pinning does not make an action trustworthy; it makes the version reviewable and makes an upstream change arrive as a diff. `.github/dependabot.yml` advances the pins monthly in **two** groups: `tauri-apps/tauri-action` is excluded from the wildcard group and gets a `release-critical` group to itself, because it is the one action handed the signing material and the one whose breakage is silent (a bad `latest.json` publishes green and updates nobody). Dependabot cannot see a branch SHA at all, so check `gh api repos/<owner>/<repo>/tags` before pinning anything new.
- **`dtolnay/rust-toolchain` is pinned to tag `v1`, where `toolchain` is a required input**, so all three call sites pass `with: toolchain: stable`. Dropping that `with:` block fails the action, not the build — it is load-bearing, not decoration.
- **Every job has a `timeout-minutes` with its observed maximum in a comment next to it**, so an infinite hang costs minutes rather than six hours. The release `build` legs are 90 minutes each and `max-parallel: 1`, so the job as a whole can legitimately take four times one leg.
- **Both workflows default to `permissions: contents: read`**, with `contents: write` granted only to `create-release`, `build` and `publish`. `update-tap` goes the other way to `permissions: {}` because it authenticates as `TAP_GITHUB_TOKEN` throughout.
- **`release.yml`'s concurrency group is `${{ github.workflow }}`, not the ref, with `cancel-in-progress: false`.** Each tag is its own ref, so a ref-scoped group would serialize nothing; two overlapping releases would then race the same `latest.json`.
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

- `SECURITY.md` is the public reporting policy. Reports go through **GitHub
  private vulnerability reporting**, which is **enabled** on the repo (it was off
  until 2026-08-05; a policy file pointing at a disabled channel sends reporters
  to a button that does not exist). Toggle with
  `gh api -X PUT|DELETE repos/tstone-1/dblitz/private-vulnerability-reporting`.
- No contact e-mail appears anywhere public, deliberately — the advisory flow is
  the channel, and it keeps the personal address off a public repo.
- In scope, and worth knowing when triaging: a bypass of **any** of the four
  read-only layers above, a crafted database file (the app's one genuinely
  untrusted input), updater signature verification, and webview escape. The
  unsigned Windows build is a documented state, not a finding.

## Release

- Versions use CalVer `YY.M.MICRO`. **Five** files must agree: `package.json`, `package-lock.json` (top-level *and* `packages[""]` — `npm version <v> --no-git-tag-version` does both), `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (never hand-edited; `cargo check` rewrites it after the `Cargo.toml` bump), and `src-tauri/tauri.conf.json`. Nothing fails when any of them drifts, which is how 26.7.6 shipped on a stale lockfile and how `Cargo.lock` was still on 26.8.1 while 26.8.2 was staged.
- **The `preflight` job proves the tag names the version being published, and `create-release` needs it.** Everything else in `release.yml` is thorough about what it builds and signs, and none of it looks at the tag — so a `v26.8.2` tag on 26.8.1 manifests would publish 26.8.1 installers and updater metadata as 26.8.2 with nothing red anywhere. `scripts/release-preflight.mjs` checks CalVer and all five version files plus a **dated** `CHANGELOG.md` heading, before the draft release exists. It is dependency-free on purpose so an install failure cannot take the gate out; run it locally with `node scripts/release-preflight.mjs vYY.M.MICRO`. The `Cargo.lock` entry is looked up by the name `Cargo.toml` declares, and the `name =` match is anchored to the start of a line inside each `[[package]]` block — a substring search finds the crate again inside another package's `dependencies = [...]` array and reports a phantom duplicate. `src/lib/releasePreflight.test.ts` pins each version file failing on its own, plus a positive control.
- **A script's main-module guard is part of the gate.** That guard used to be a string comparison of `import.meta.url` against a `file://` URL built from `process.argv[1]`, which is false from any path containing a space (percent-encoding) or reached through a symlink (realpath): the script then exited 0 having run no check at all, and the workflow read that as a pass. It is now `import.meta.main` with a realpath fallback, and the workflow additionally greps the output for `^[OK] Release preflight`, failing with an explicit error otherwise. Any new gate script gets both halves. Pinned by `releasePreflight.test.ts` ("the preflight CLI actually runs from an awkward path") and `releaseWorkflow.test.ts` ("blocks draft creation on it").
- The `## [version] - Unreleased` heading form is safe **here** — checked 2026-08-23: `release.yml` writes fixed notes pointing at CHANGELOG.md and never slices a section out of it by version heading, so a forgotten rename cannot publish a release whose notes say "Unreleased". Re-check that if the release notes ever become generated. Since 26.8.2 a forgotten rename is also **caught**: preflight requires a `YYYY-MM-DD` date on the heading for the tagged version.
- See `BUILD.md` for the release checklist. There is no shared-folder deployment: the portable exe used to be copied to a shared tools folder by hand, and that step was dropped on 2026-09-17. Installed copies update through the in-app updater.
- **Stage release commits by explicit pathspec.** `src/lib/releaseWorkflow.test.ts` rejects blanket staging in the release instructions (command lines only — prose naming `git add -A` to warn against it is exempt).
- A release workflow can take 20 minutes or more when GitHub Actions has a cold Rust cache, even without dependency changes. Confirm the active job step before treating a long run as stuck.
- The draft-first release workflow must pass the numeric `releaseId` from `create-release` to `tauri-action`; GitHub's tag lookup returns 404 for a draft release, so reverting build uploads to `tagName` breaks the matrix.
- Rebuild-and-overwrite under an existing version label is permitted only while that version is unpublished and only after asking the user. Once its tag and GitHub release are published, cut the next version instead.
- Keep review-fix release batches scoped to the fixes. Do not fold strategic refactors (file splits, major extractions) into the same release even when they touch the same files — defer them to separate, single-purpose releases unless the refactor is a genuine prerequisite for a fix.
- Expected `cargo audit` noise is the established allowed Tauri/Linux WebView transitive set (legacy GTK/glib/unic advisories). Treat any new advisory or materially different output as actionable; historical npm `cookie` findings applied only to the unreachable SvelteKit SSR path and must be re-evaluated if they reappear.

### In-app updater

- **The updater's minisign private key is the most safety-critical secret in this repo.** It lives in KeePass and in the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets; the public key is committed in `tauri.conf.json`. It is a **dblitz-only** key — deliberately not shared with `screenpick`. Losing it permanently orphans every installed copy: a new key cannot sign for clients holding the old public key, so each user would have to reinstall by hand. It is unrelated to OS code signing and is not fixed by adding an Apple Developer ID. Never print, echo, or paste the private key into a tool call; custody and regeneration live in [BUILD.md](BUILD.md#updater-signing-key).
- **A release with no `latest.json` updates nobody, and looks green.** When the signing secret is missing or empty, `tauri-action` logs "Signature not found for the updater JSON. Skipping upload..." and still succeeds. The release ships with installers and no manifest, and the failure only surfaces as users silently never updating. BUILD.md's post-release checks exist for this.
- **The release matrix must stay `max-parallel: 1`.** `tauri-action` builds `latest.json` by read-modify-write against the release asset, so parallel legs clobber each other's platform entries and produce a manifest that updates only some platforms. dblitz has four legs (two of them macOS), so this bites harder here than in a two-leg repo. Nothing fails; the manifest is just incomplete.
- **Not every install can self-update, and the UI must not pretend otherwise.** The updater hands its payload to an installer, and neither a package-managed Linux install nor a standalone Windows exe has one that owns this copy. `InstallProvenance` in `updates.rs` enumerates all five artifacts and `self_update_supported` matches exhaustively, so a new artifact cannot inherit a default answer. Do not "fix" the gate by always offering the install. Pinned by `updates::tests::{macos_can_always_self_update, installed_windows_build_can_self_update, portable_windows_build_does_not_offer_install, linux_appimage_can_self_update, linux_deb_or_rpm_cannot_self_update}`.
- **Windows provenance is read from the registry, not guessed from the path.** `windows_install_provenance` in `lib.rs` compares the running executable's directory against the `InstallLocation` the NSIS installer recorded under `Software\Microsoft\Windows\CurrentVersion\Uninstall\dblitz` (HKCU per-user, HKLM per-machine; the value is stored **with** surrounding quotes, so it must be trimmed). Path identity is the test rather than the key merely existing: a user can have dblitz installed *and* be running a portable copy out of Downloads. Anything unreadable reports portable, because the two failure directions are not symmetric — wrongly claiming portable costs one manual download, while wrongly claiming installed runs the NSIS installer, which cannot find the standalone exe and instead creates a *second*, installed copy at the new version while the file the user launches stays old. The last segment of that key is `productName` and nothing at build time couples them (`uninstall_key_tracks_the_bundle_product_name`).
- **`updates.rs` is deliberately pure** — no `tauri::` imports, no env reads, no `cfg!`, no registry access. Everything host-dependent is resolved in `lib.rs` and arrives as one `InstallProvenance`, which is what lets the Linux *and* Windows gates be unit-tested from any machine. Keep it that way. The `allow(dead_code)` on the enum is there because every build constructs exactly one variant; the tests construct all five.
- **`ConfigStore::record_run_version` is destructive by design.** It returns the previous version and immediately overwrites it, so "did we just update?" is only answerable at the moment it is called. `lib.rs` calls it once in `setup` and caches the result as `UpdateStatus` app state; a command that re-derived it on demand would always answer "no" (`record_run_version_is_a_no_op_on_an_unchanged_relaunch`).
- **New `AppConfig` fields need `#[serde(default)]` and a matching manual `Default`.** `load_app_config` falls back to `AppConfig::default()` on any parse error, so a field without a serde default makes every older `app.json` unparseable and silently wipes the user's recent-files list. And because `check_for_updates_on_startup` defaults to `true`, `Default` is hand-written — a derived one would give `false` and diverge. Pinned by `config::tests::{app_config_default_matches_its_serde_default, pre_updater_app_config_still_parses_and_keeps_recents}`.
- **A local updater test build shares the real app's bundle identifier** (`com.tstone.dblitz`), so it writes into the real `app.json`. Clear `last_run_version` afterwards or the next real launch believes it just updated.

### Distribution / Homebrew tap

- Pushing a `v*` tag triggers `.github/workflows/release.yml`: it runs preflight and the quality gate, creates the GitHub release, builds/uploads artifacts (macOS `.dmg` for `aarch64` + `x64`, Windows, Linux), then the `update-tap` job auto-bumps the Homebrew cask.
- The macOS app is distributed via the Homebrew cask `dblitz` in the tap repo **`tstone-1/homebrew-dblitz`** (`Casks/dblitz.rb`). The cask URL pattern is `dblitz_<version>_<arch>.dmg` with `arch arm: "aarch64", intel: "x64"`.
- `update-tap` downloads the two macOS DMGs from the release, computes their `sha256`, and `sed`-edits `version` + both `sha256` lines in the tap's `Casks/dblitz.rb`, then commits/pushes `Bump dblitz cask to v<version>` as `tstone-1`. It pushes to a *different* repo, so it uses the **`TAP_GITHUB_TOKEN`** secret (a fine-grained PAT with Contents:read/write on `tstone-1/homebrew-dblitz`) — the default `GITHUB_TOKEN` cannot. If that secret is missing/unauthorized the `update-tap` job fails (but the build/release still succeed); re-set it with `gh secret set TAP_GITHUB_TOKEN --repo tstone-1/dblitz < <file>` (there is no `--body-file` flag; feed the value on stdin. The interactive prompt does NOT work through a non-interactive shell — it silently stores an empty value).
- `Casks/dblitz.rb` carries **`auto_updates true`** (added with the in-app updater). Without it Homebrew and the updater fight: `brew upgrade` reinstalls over a self-updated app, and `brew` reports the cask as outdated forever. This must survive the `update-tap` job — that job only `sed`-edits the `version`/`arm:`/`intel:` lines, so it does.
- The tap is a personal/untrusted tap: first use on a machine needs `brew trust --cask tstone-1/dblitz/dblitz`. Install/upgrade the local app with `brew install --cask dblitz` / `brew upgrade --cask dblitz`. To overwrite a pre-existing non-brew install, use `brew install --cask --force dblitz` (`--adopt` only works when the on-disk version already matches).

### macOS signing and notarization

**Since 26.7.6** the macOS build is signed with a Developer ID identity and notarized by Apple. Full setup, credential storage, and the traps hit while getting there are in [BUILD.md](BUILD.md#macos-code-signing-and-notarization). What matters when editing CI:

- The signing identity is **team-wide, not per-app**: `Developer ID Application: Timo Stein (NVX72G8SJ8)`, the same certificate and the same App Store Connect `.p8` notarization key that `screenpick` uses. A Developer ID cert certifies a team, never an app, and Apple caps the account at 5 of them — so a second app reuses the first one's material rather than minting its own. Consequence: **rotating that certificate is a two-repo event.** Both repos' `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` / `APPLE_SIGNING_IDENTITY` secrets have to be updated together, or the repo you forgot silently drops to unsigned-and-blocked on its next release.
- Six repo secrets, macOS legs only: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`.
- **The signing vars are exported via `$GITHUB_ENV`, never listed in the build step's `env:` block.** A fork has none of these secrets; `env:` would hand the Tauri CLI an *empty* `APPLE_SIGNING_IDENTITY`, which it reads as "sign with this identity" and fails on. The conditional export step leaves them genuinely unset, so the CLI falls back to the `"signingIdentity": "-"` in `tauri.conf.json` and a fork still builds.
- **A skipped notarization exits 0.** Missing or malformed credentials make the bundler log `skipping app notarization` and succeed, shipping a signed-but-unnotarized app that Gatekeeper rejects on any machine that has never seen it — looks green, is broken. `release.yml` therefore ends each macOS leg with a verification step gating on `Authority=Developer ID Application`, the `runtime` flag, `stapler validate`, and `source=Notarized Developer ID`. Do not weaken that gate.
- **The signing gates fail CLOSED in `tstone-1/dblitz`, and that is the whole point of the shape.** Until 26.8.1 the preparation step checked two of the six Apple values and warned-and-exited-0 otherwise, while the notarization and verification steps were conditioned on `env.APPLE_SIGNING_IDENTITY != ''` — so dropping any one secret skipped precisely the gates written to catch a dropped secret, and `publish` still ran. All six are now required when `GITHUB_REPOSITORY` is `tstone-1/dblitz`, the verification step's condition also fires on `github.repository == 'tstone-1/dblitz'` so it cannot skip in its own failure case, and forks keep the ad-hoc unsigned path. **A gate that cannot run in the case it exists for is not a gate.** Pinned by `releaseWorkflow.test.ts` ("requires every Apple secret, not just the certificate and the key", "refuses to continue in the canonical repo when one is missing", "still builds ad-hoc signed in a fork", "runs the artifact verification on every canonical macOS leg").
- **The bundler notarizes the `.app`, not the `.dmg`.** After a signed build the DMG carries a Developer ID signature but no ticket, and `spctl -a -t open` rejects it as `Unnotarized Developer ID`. Since the DMG is what users download, `release.yml` notarizes and staples it in a separate step and re-uploads it over the asset `tauri-action` published (`gh release upload --clobber`, which resolves the draft by tag). That step runs **per matrix leg**, so both the `aarch64` and `x64` DMGs get their own submission — a shared/universal path would silently staple only one arch.
- Ordering that must hold: staple happens in the `build` job, `update-tap` hashes the DMG after `publish`, so the cask's `sha256` is of the **stapled** bytes. Moving the staple later, or the hash earlier, produces a cask whose checksum never matches the published asset.
- Updates inherit the signature: the updater bundler tars the already-stapled `.app` without re-signing, and the staple ticket lives at `Contents/CodeResources` (an ordinary file, not an xattr), so it survives the tar.
- **Windows stays unsigned** — no Authenticode cert. SmartScreen warns on first launch; a Developer ID does nothing for that.
- The tap cask's `postflight` quarantine strip (added 2026-07-07 as the free workaround for the "damaged" error) was **removed** from `Casks/dblitz.rb` on 2026-07-25, once 26.7.6 proved a notarized app passes Gatekeeper with the quarantine flag intact. Do not reintroduce it: stripping the flag discards provenance, and needing it again would mean notarization has silently broken — which is the thing to investigate, not paper over.
