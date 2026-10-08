# dblitz Agent Notes - detail

Sections moved verbatim out of `AGENTS.md`, which keeps a one-line pointer to each.
Read the section before working in its area.

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

### In-app updater

- **The updater's minisign private key is the most safety-critical secret in this repo.** It lives in KeePass and in the `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets; the public key is committed in `tauri.conf.json`. It is a **dblitz-only** key — deliberately not shared with `screenpick`. Losing it permanently orphans every installed copy: a new key cannot sign for clients holding the old public key, so each user would have to reinstall by hand. It is unrelated to OS code signing and is not fixed by adding an Apple Developer ID. Never print, echo, or paste the private key into a tool call; custody and regeneration live in [BUILD.md](../BUILD.md#updater-signing-key).
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
- **The login lives in the GitHub environment `signing`, not in the repository secrets**, and only the Windows leg names that environment: `environment: ${{ matrix.signs-windows && 'signing' || '' }}`. The empty string is no environment, so the Linux and macOS legs cannot read the login at all. Do not simplify that to `environment: signing`; it would hand the login to all four legs.
- **`signs-windows` is a flag in the matrix, and every path is built in the step.** `github.workspace` is empty inside a matrix, so a path assembled there starts at the drive's root.
- **The sign command is an overlay (`src-tauri/tauri.signing.conf.json`), passed with `--config` on the Windows leg only.** In `tauri.conf.json` it would make every local Windows build ask for the login.
- **`scripts/sign-windows.cmd` finds its script through `DBLITZ_SIGN_SCRIPT`, never through `%~dp0`.** makensis starts it by quoted name through `PATH` from its own folder to sign the uninstaller; with `%~dp0` that call fails, makensis prints `UninstFinalize command returned 64` and goes on, and the release ships an unsigned uninstaller with everything else signed and the build green.
- **`scripts/sign-windows.ps1` signs into a separate folder and copies back with retries.** `ssign` replaces a file by rename, which fails with "Access is denied" while Tauri still has the executable open. It also keeps the log, because Tauri prints nothing of a sign command that failed.
- **One login at a time.** A one-time code is good for one login, and repeated failed logins can lock the account. The Windows leg and `sign-rehearsal.yml` share the concurrency group `certum-signing` with `cancel-in-progress: false`; the other three legs get a group of their own so they are not held up. The group ends at the repository: a `tpdf` or `screenpick` release signing at the same moment is invisible to it.
- **`ssign` is installed into its own folder under `RUNNER_TEMP`** (`cargo install --root`), not into `~/.cargo/bin`, which `Swatinem/rust-cache` saves and restores. The program that holds the login is built from the pinned commit in every run. `SSIGN_REV` appears in both workflows and must be the same commit in both.
- **The session file is removed in an `if: always()` step**, and the signing log is printed in another. Both must stay `always()`: the failing run is the one that needs them.
- **The verification step is the gate, and it runs before `Upload portable exe`.** It reads the installer and `src-tauri/target/release/dblitz.exe` (the file that step uploads; Tauri signs it in place before packing it), then installs silently, reads the install folder from the registry key the installer writes, and checks every executable file there, found by their `MZ` bytes and not by a list of names. `uninstall.exe` exists only after an install, which is why the step installs.
- **No MSI.** `src-tauri/tauri.windows.conf.json` sets `bundle.targets` to `["nsis"]` for Windows; `tauri.conf.json` keeps `"all"` for the other platforms. `ssign` signs PE files only. `updaterJsonPreferNsis: true` stays as it is.
- **The build job's checkout has `persist-credentials: false`**, because that job now compiles a program from another repository. Every step that talks to GitHub gets its token through `env`.
- All of the above is held by `src/lib/windowsSigning.test.ts`, each assertion shown to fail on its own mutation when it was written. None of it can run on a development machine; the first real proof of the leg is a tag.

### Parquet backend (`src-tauri/src/pq/`)

A Parquet file is served by an embedded DuckDB behind the **same 20 IPC commands**, so
the frontend has no second code path. `lib.rs` dispatches on
`DbState::parquet_session()`; `db::open_parquet` and `db::close_database` keep the
failure-atomic open (both DuckDB instances are built before `DbState` changes, and a
SQLite open drops any Parquet session - `db::tests::opening_one_kind_of_file_drops_the_other`).
Detection is by content (`PAR1` at both ends), never by extension.

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
