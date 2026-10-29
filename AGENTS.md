# dblitz Agent Notes

## Project

- `dblitz` is a public `tstone-1` repository for a Tauri + SvelteKit + TypeScript desktop SQLite browser.
- Treat the repository as public: do not add internal company data, private paths, secrets, or proprietary examples.
- Use a public-safe Git identity for commits; do not commit with private or company email addresses.

## Working here

- Install frontend dependencies with `npm install`; run the desktop app with `npm run tauri dev`.
- `npm run quality` is the whole gate and is what CI runs: `npm run check`, `npm test`, `npm run build`, then in `src-tauri` `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets --all-features -- -D warnings`. Use the bare clippy invocation with `-D warnings` — a plain `cargo clippy` can pass locally and still fail CI.
- Use `npx tauri build` for local release builds. macOS DMG packaging may need to run outside a sandbox because Tauri invokes system image mounting tools.
- Frontend code lives under `src/`; Tauri/Rust backend code under `src-tauri/`.
- **Stage release commits by explicit pathspec.** `src/lib/releaseWorkflow.test.ts` rejects blanket staging in the release instructions (command lines only — prose naming `git add -A` to warn against it is exempt).
- See `BUILD.md` for the release checklist. There is no shared-folder deployment: the portable exe used to be copied to a shared tools folder by hand, and that step was dropped on 2026-09-17. Installed copies update through the in-app updater.
- Rebuild-and-overwrite under an existing version label is permitted only while that version is unpublished and only after asking the user. Once its tag and GitHub release are published, cut the next version instead.
- Keep review-fix release batches scoped to the fixes. Do not fold strategic refactors (file splits, major extractions) into the same release even when they touch the same files — defer them to separate, single-purpose releases unless the refactor is a genuine prerequisite for a fix.

## Index

<!-- docs-index: docs/agent-notes.md -->
The full text of every entry below is in `docs/agent-notes.md`, under the same
title. Read the entry before working in its area: a title is a claim, and the
entry holds the reasoning, the incident and the tests that pin it.

### Development

- *Node is pinned to 24, and `@types/node` with it* — `.nvmrc` is the one source CI reads; bump `.nvmrc` and `@types/node` together or not at all.
- *A green local gate can be measured against the wrong dependency* — `node_modules` drifts from the lockfile and nothing fails; `npm ci` after pulling a lockfile change.
- *Type-checking runs on TypeScript 7 through `svelte-check --tsgo`* — TS 6 stays installed on purpose; the wrapper fails a svelte-check run that crashed with exit 0.
- *Vitest's global environment is `node`* — DOM tests opt in per file with a `@vitest-environment jsdom` pragma.
- *A green gate on one OS says nothing about a `cfg`-gated symbol on another* — 26.8.1 failed on Linux over an import only Windows/macOS used; annotate types on every `cfg` arm, check the other OS before tagging.
- *A Windows machine can run the Linux gate through WSL* — the recipe, with its own target dir; Linux runs ten fewer Rust tests, all Windows-only.
- *There is no Windows cross-check from macOS* — `ring` and `libsqlite3-sys` need MSVC; `cfg(windows)` code changed on a Mac is verified only by the Windows CI leg.
- *A Windows machine can run the macOS gate on a Mac over ssh* — tar the uncommitted tree plus `build/` to a scratch folder and run the gate there; set `PATH` to `~/.cargo/bin`, ssh has none.
- *README screenshots are captured by `scripts/readme-screenshots.py`* — synthetic DB at a neutral path, Vite running for a debug build, CDP capture for 2x, paste instead of typing setup text, sort persists per file.
- *Inspecting a shipped build's webview (Windows)* — drive a release build over CDP through WebView2's environment variables, to tell a backend failure from a webview one.

### Architecture

- Keep the window title filename-only; the toolbar owns display of the full database path.
- Treat DB Browser for SQLite as the primary UX comparison point when evaluating viewer behavior and parity gaps.
- In `DataGrid.svelte`, compose new per-column state indicators with inset box shadows rather than background tints so user-selected column colors remain visible.
- Files that use Svelte runes outside a component must use the `.svelte.ts` extension. Keep template and `$derived` reads side-effect-free — selection statistics are computed in a debounced `$effect` over already-cached rows for exactly this reason. Create missing state only from event handlers or other explicit mutation paths.
- Preserve `connect-src ipc: http://ipc.localhost` in the Tauri CSP. Removing it can leave production IPC broken while development still appears healthy.
- Tauri and the direct `windows` dependency can resolve to different `windows-rs` versions. `window.hwnd()` must be rewrapped as `HWND(hwnd.0)` at that boundary; other HWNDs sourced from `EnumWindows` do not need blanket conversion.

- *Parquet backend (`src-tauri/src/pq/`)* — DuckDB serves Parquet, CSV and JSON behind the same IPC commands; CSV/JSON load into a table paged by `rowid`; the SQL tab lockdown is two layers in a load-bearing order.
- *The SQLite backend's modules* — what each file under `src-tauri/src/db/` holds, and `bench_api`, the one seam the benchmarks may use.
- *All column discovery goes through `PRAGMA table_xinfo`* — `table_info` omits generated columns `SELECT *` returns, which corrupts paging; `schema::table_columns` is the one reader.
- *Every non-trivial view pages from one cached ordered-rowid list* — keyed by `OrderKey`, whose `regex_signature` is load-bearing: two regex views produce identical SQL.
- *A regex scan selects only the columns a pattern is applied to* — filter indices index the projection; NULL, BLOB, number and unknown-column semantics each have a test.
- *Chunk 0 of an unsorted table is served with `LIMIT`* — the rowid index waits for `offset > 0`; cached row counts are sound only because of `immutable=1`.
- *REAL cells render as SQLite renders them* — `util::render_real` matches `CAST AS TEXT`, so a filter typed against the grid matches.
- *`dblitz` is a strict read-only SQLite viewer* — reject features that imply mutation; four enforcement layers, preserve all four when changing query execution or opening.
- *The SQL editor's live check goes through the same gates as Execute* — `prepare_checked` is shared, and a check error disables Execute, so drift blocks valid queries; offsets, function lists, rechecks.
- *`PRAGMA mmap_size` must not come back* — with `immutable=1` a file shrinking under the mapping killed the process with SIGBUS, and it was slower cold.
- *There are two read-only connections to the same file* — `conn` browses, `aux_conn` serves SQL and Structure; they agree only through `immutable=1`; lock order is fixed.
- *Windows duplicate-instance detection is keyed by a hash of the path* — case-folded absolute path in a window property; the unconditional `FlashWindowEx` must stay.
- *A launch argument is the one path that arrives relative* — `absolutize_launch_path` resolves it before recents, the config key and the instance check see it.
- *A view-config save carries its own database path* — saves are serialized and capture path and config at enqueue time; the path is a lookup key.
- *The config key is a hash of the normalized path* — case-folded on Windows, byte-exact elsewhere; old raw-path keys are read and migrated.
- *An `await` in a component publishes only if it still owns publication* — capture a per-caller token and `dbOpenGeneration` before the await; `createSessionOwnedRequest` is the seam.
- *Find in the grid searches the rows the grid shows* — through the grid's row source and chunk size; a failed block is an error, never "No match".
- *A column preset stores the visible columns, in order* — so a column the table gains later stays hidden; applying one is one config save.
- *A column's display name is not its identity* — types are recovered by position; the clipboard carries an HTML table plus RFC 4180 TSV.
- *There are two column-reorder implementations* — only `DataGrid`'s mouse-event one is proven in WebView2; `ColumnSettings` uses unproven HTML5 drag-and-drop.

### CI

- *The three workflows, and the tests that pin them* — `checks.yml`, `release.yml`, `sign-rehearsal.yml`; three test files fail on any drift.
- *Every `uses:` is pinned to a full commit SHA* — the release job holds every signing secret; Dependabot advances pins monthly, `tauri-action` in a group of its own.
- *`dtolnay/rust-toolchain` is pinned to tag `v1`* — where `toolchain` is a required input, so the `with:` block is load-bearing.
- *Every job has a `timeout-minutes` with its observed maximum* — release legs are 90 minutes each and run one at a time.
- *Every workflow defaults to `permissions: contents: read`* — write only for `create-release`, `build` and `publish`; `update-tap` has `permissions: {}`.
- *`release.yml`'s concurrency group is the workflow, not the ref* — each tag is its own ref, so a ref group would let two releases race `latest.json`.
- *The Linux build prerequisites live in one file* — `.github/tauri-linux-deps.txt`, read with an emptiness check at every apt site.
- *The packaged-app smoke test, and exactly what it does not cover* — the Linux `smoke` job is the only check above the webview-IPC seam, and it does not catch removal of `connect-src ipc:`.

### Security

- *Security reporting, and what is in scope* — GitHub private vulnerability reporting, no public e-mail; in scope: read-only bypasses, crafted files, updater signatures, webview escape.

### Release

- *Five version files must agree* — `package.json`, `package-lock.json` (twice), `Cargo.toml`, `Cargo.lock`, `tauri.conf.json`; nothing fails when one drifts, so `scripts/bump-version.mjs` sets all six sites and checks them.
- *The `preflight` job proves the tag names the version* — `scripts/release-preflight.mjs` checks CalVer, the five files and a dated CHANGELOG heading before the draft exists.
- *A script's main-module guard is part of the gate* — a string-compared guard exited 0 having checked nothing; use `import.meta.main` and grep the OK line.
- *The `Unreleased` heading form is safe here* — release notes are fixed text, never sliced from CHANGELOG; preflight requires a date on the tagged version.
- *A long release run is not necessarily stuck* — a full run takes about two hours (legs run one at a time); confirm the active job step before treating it as hung.
- *The draft release needs the numeric `releaseId`* — GitHub's tag lookup returns 404 for a draft, so `tauri-action` takes the id from `create-release`.
- *Expected `cargo audit` noise* — the allowed Tauri/Linux GTK set; any new advisory is actionable. Also npm's unfixable build-time `deepmerge` finding: never `npm audit fix --force`.
- *In-app updater* — the minisign key is the most critical secret; a release without `latest.json` looks green; `max-parallel: 1`; new `AppConfig` fields need `#[serde(default)]`.
- *Distribution and the Homebrew tap* — a `v*` tag releases and `update-tap` bumps `tstone-1/homebrew-dblitz`; the cask keeps `auto_updates true`.
- *macOS signing and notarization* — Developer ID shared with `screenpick` (a two-repo rotation); signing gates fail closed; the DMG is notarized separately.
- *Windows code signing* — Certum certificate shared with `tpdf` and `screenpick`, applied by `ssign` from the `signing` environment; no MSI. Read BUILD.md first.
- *Windows on ARM is built natively on `windows-11-arm`* — not cross-compiled; the verify step asserts the PE machine field; the portable exe is `dblitz-arm64.exe`.
