# dblitz Agent Notes

## Project

- `dblitz` is a public `tstone-1` repository for a Tauri + SvelteKit + TypeScript desktop SQLite browser.
- Treat the repository as public: do not add internal company data, private paths, secrets, or proprietary examples.
- Use a public-safe Git identity for commits; do not commit with private or company email addresses.

## Development

- Node is pinned to 24 in `.nvmrc`, and every CI `setup-node` step reads it via `node-version-file` so there is a single source of truth. `@types/node` is deliberately held on the matching major — `npm outdated` will keep offering a newer one; taking it means type-checking against an API surface the shipped runtime does not have. Bump `.nvmrc` and `@types/node` together or not at all.
- Install frontend dependencies with `npm install`.
- Run the desktop app in development with `npm run tauri dev`.
- Use these checks as appropriate:
  - `npm run check`
  - `npm run build`
  - `cd src-tauri && cargo fmt --check`
  - `cd src-tauri && cargo test`
  - `cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings` (matches CI/`npm run quality` — a bare `cargo clippy` can pass locally and still fail CI)
- Use `npx tauri build` for local release builds. macOS DMG packaging may need to run outside a sandbox because Tauri invokes system image mounting tools.

## Architecture

- Frontend code lives under `src/`.
- Tauri/Rust backend code lives under `src-tauri/`.
- SQLite backend code lives under `src-tauri/src/db/`, with `src-tauri/src/db.rs` as a thin facade that re-exports the submodules:
  - `schema.rs` — table/column introspection and row counts
  - `query.rs` — table paging, the rowid-index fast path, and regex filtering
  - `filters.rs` — the `WHERE` clause builder and column-filter operator parsing
  - `sql.rs` — arbitrary SQL execution plus the read-only / ATTACH-DETACH rejection gate
  - `export.rs` — XLSX export
  - `types.rs`, `util.rs` — shared DTOs and helpers (`safe_ident`, `read_row`, `StrErr`)
  - `benchmark.rs` — `cfg(debug_assertions)` paging benchmarks
- `dblitz` is a **strict read-only SQLite viewer** by explicit decision (2026-04-10). Reject feature requests that imply mutation (inline cell edit, delete row, save-as, schema changes) and surface the decision before implementing. Enforcement is layered — preserve all layers when changing query execution or database opening logic:
  1. Connections open with `SQLITE_OPEN_READ_ONLY`.
  2. Plus `?immutable=1` in the URI: the file is treated as a frozen snapshot for the connection's lifetime, and no `-wal`/`-shm` companion files are ever created (important because source files may sit in cloud-synced folders that other tools write to).
  3. `execute_sql` rejects non-readonly prepared statements (`stmt.readonly()`) with a friendly message, and a SQLite authorizer denies ATTACH/DETACH/transactions at the engine level.
  - Accepted trade-off: dblitz does not see live writes from other processes during a session; reopening the file is required to pick up changes. The SQL editor should always advertise read-only in its placeholder/hint text. Backend row/index caches need no invalidation logic because the snapshot is frozen.
- Windows duplicate-instance detection is keyed by the full database path through the `dblitz_db_path` Win32 window property. Do not replace it with filename-only matching: same-named databases in different directories must open separately. The `FlashWindowEx` call that follows `SetForegroundWindow` in `try_activate_existing` is deliberately unconditional and must stay: activation is usually denied by the foreground lock, and without the flash the second launch exits silently and reads as "dblitz refuses to open this file". See the comment there for why the outcome cannot be tested reliably.
- Keep the window title filename-only; the toolbar owns display of the full database path.
- Treat DB Browser for SQLite as the primary UX comparison point when evaluating viewer behavior and parity gaps.
- In `DataGrid.svelte`, compose new per-column state indicators with inset box shadows rather than background tints so user-selected column colors remain visible.
- Files that use Svelte runes outside a component must use the `.svelte.ts`
  extension. Keep template and `$derived` reads side-effect-free; create missing
  state only from event handlers or other explicit mutation paths.
- Preserve `connect-src ipc: http://ipc.localhost` in the Tauri CSP. Removing it
  can leave production IPC broken while development still appears healthy.
- `DataGrid` header reordering deliberately uses mouse events with a movement
  dead zone and document-level cleanup. Do not replace it with HTML5
  drag-and-drop without proving the behavior in the Windows WebView2 runtime.
- Tauri and the direct `windows` dependency can resolve to different `windows-rs` versions. `window.hwnd()` must be rewrapped as `HWND(hwnd.0)` at that boundary; other HWNDs sourced from `EnumWindows` do not need blanket conversion.

## Release

- Versions use CalVer `YY.M.MICRO`.
- Update all version files together:
  - `package.json`
  - `package-lock.json` — in **two** places (top-level `"version"` and `packages[""].version`). `npm version <v> --no-git-tag-version` does `package.json` and both lockfile entries at once. Nothing fails when this one drifts, which is exactly how 26.7.6 shipped with the lockfile still on 26.7.5.
  - `src-tauri/Cargo.toml`
  - `src-tauri/tauri.conf.json`
- Update `CHANGELOG.md` before release commits.
- See `BUILD.md` for the release checklist. Its shared-tools copy path is a placeholder unless the user provides a real deployment target.
- A release workflow can take 20 minutes or more when GitHub Actions has a cold Rust cache, even without dependency changes. Confirm the active job step before treating a long run as stuck.
- The draft-first release workflow must pass the numeric `releaseId` from `create-release` to `tauri-action`; GitHub's tag lookup returns 404 for a draft release, so reverting build uploads to `tagName` breaks the matrix.
- Rebuild-and-overwrite under an existing version label is permitted only while that version is unpublished and only after asking the user. Once its tag and GitHub release are published, cut the next version instead.
- Keep review-fix release batches scoped to the fixes. Do not fold strategic refactors (file splits, major extractions) into the same release even when they touch the same files — defer them to separate, single-purpose releases unless the refactor is a genuine prerequisite for a fix.
- Expected `cargo audit` noise is the established allowed Tauri/Linux WebView transitive set (legacy GTK/glib/unic advisories). Treat any new advisory or materially different output as actionable; historical npm `cookie` findings applied only to the unreachable SvelteKit SSR path and must be re-evaluated if they reappear.

### In-app updater

- **The updater's minisign private key is the most safety-critical secret in this
  repo.** It lives in KeePass and in the `TAURI_SIGNING_PRIVATE_KEY` /
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repo secrets; the public key is committed in
  `tauri.conf.json`. It is a **dblitz-only** key — deliberately not shared with
  `screenpick`. Losing it permanently orphans every installed copy: a new key cannot
  sign for clients holding the old public key, so each user would have to reinstall
  by hand. It is unrelated to OS code signing and is not fixed by adding an Apple
  Developer ID. Never print, echo, or paste the private key into a tool call;
  custody and regeneration live in [BUILD.md](BUILD.md#updater-signing-key).
- **A release with no `latest.json` updates nobody, and looks green.** When the
  signing secret is missing or empty, `tauri-action` logs "Signature not found for
  the updater JSON. Skipping upload..." and still succeeds. The release ships with
  installers and no manifest, and the failure only surfaces as users silently never
  updating. BUILD.md's post-release checks exist for this.
- **The release matrix must stay `max-parallel: 1`.** `tauri-action` builds
  `latest.json` by read-modify-write against the release asset, so parallel legs
  clobber each other's platform entries and produce a manifest that updates only
  some platforms. dblitz has four legs (two of them macOS), so this bites harder
  here than in a two-leg repo. Nothing fails; the manifest is just incomplete.
- **Not every install can self-update, and the UI must not pretend otherwise.** The
  Tauri updater supports **AppImage only** on Linux — `.deb`/`.rpm` installs are
  package-manager-owned and cannot be replaced in place. `src-tauri/src/updates.rs`
  gates this on `$APPIMAGE` and the frontend hides the Install button accordingly.
  The portable `dblitz.exe` likewise has no installer to hand off to. Do not "fix"
  the gate by always offering the install.
- **`updates.rs` is deliberately pure** — no `tauri::` imports, no env reads, no
  `cfg!`. The OS and `$APPIMAGE` are passed in from `lib.rs`, which is what lets the
  Linux gate be unit-tested from macOS and Windows. Keep it that way.
- **`ConfigStore::record_run_version` is destructive by design.** It returns the
  previous version and immediately overwrites it, so "did we just update?" is only
  answerable at the moment it is called. `lib.rs` calls it once in `setup` and caches
  the result as `UpdateStatus` app state; a command that re-derived it on demand
  would always answer "no".
- **New `AppConfig` fields need `#[serde(default)]` and a matching manual `Default`.**
  `load_app_config` falls back to `AppConfig::default()` on any parse error, so a
  field without a serde default makes every older `app.json` unparseable and silently
  wipes the user's recent-files list. And because `check_for_updates_on_startup`
  defaults to `true`, `Default` is hand-written — a derived one would give `false` and
  diverge from the serde default, silently opting users out. A test pins the two
  together.
- **A local updater test build shares the real app's bundle identifier**
  (`com.tstone.dblitz`), so it writes into the real `app.json`. Clear
  `last_run_version` afterwards or the next real launch believes it just updated.

### Distribution / Homebrew tap

- Pushing a `v*` tag triggers `.github/workflows/release.yml`: it runs the quality gate, creates the GitHub release, builds/uploads artifacts (macOS `.dmg` for `aarch64` + `x64`, Windows, Linux), then the `update-tap` job auto-bumps the Homebrew cask.
- The macOS app is distributed via the Homebrew cask `dblitz` in the tap repo **`tstone-1/homebrew-dblitz`** (`Casks/dblitz.rb`). The cask URL pattern is `dblitz_<version>_<arch>.dmg` with `arch arm: "aarch64", intel: "x64"`.
- `update-tap` downloads the two macOS DMGs from the release, computes their `sha256`, and `sed`-edits `version` + both `sha256` lines in the tap's `Casks/dblitz.rb`, then commits/pushes `Bump dblitz cask to v<version>` as `tstone-1`. It pushes to a *different* repo, so it uses the **`TAP_GITHUB_TOKEN`** secret (a fine-grained PAT with Contents:read/write on `tstone-1/homebrew-dblitz`) — the default `GITHUB_TOKEN` cannot. If that secret is missing/unauthorized the `update-tap` job fails (but the build/release still succeed); re-set it with `gh secret set TAP_GITHUB_TOKEN --repo tstone-1/dblitz < <file>` (there is no `--body-file` flag; feed the value on stdin. The interactive prompt does NOT work through a non-interactive shell — it silently stores an empty value).
- `Casks/dblitz.rb` carries **`auto_updates true`** (added with the in-app updater). Without it Homebrew and the updater fight: `brew upgrade` reinstalls over a self-updated app, and `brew` reports the cask as outdated forever. This must survive the `update-tap` job — that job only `sed`-edits the `version`/`arm:`/`intel:` lines, so it does.
- The tap is a personal/untrusted tap: first use on a machine needs `brew trust --cask tstone-1/dblitz/dblitz`. Install/upgrade the local app with `brew install --cask dblitz` / `brew upgrade --cask dblitz`. To overwrite a pre-existing non-brew install, use `brew install --cask --force dblitz` (`--adopt` only works when the on-disk version already matches).

### macOS signing and notarization

**Since 26.7.6** the macOS build is signed with a Developer ID identity and notarized by Apple. Full setup, credential storage, and the traps hit while getting there are in [BUILD.md → macOS code signing and notarization](BUILD.md#macos-code-signing-and-notarization). What matters when editing CI:

- The signing identity is **team-wide, not per-app**: `Developer ID Application: Timo Stein (NVX72G8SJ8)`, the same certificate and the same App Store Connect `.p8` notarization key that `screenpick` uses. A Developer ID cert certifies a team, never an app, and Apple caps the account at 5 of them — so a second app reuses the first one's material rather than minting its own. Consequence: **rotating that certificate is a two-repo event.** Both repos' `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` / `APPLE_SIGNING_IDENTITY` secrets have to be updated together, or the repo you forgot silently drops to unsigned-and-blocked on its next release.
- Six repo secrets, macOS legs only: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8`.
- **The signing vars are exported via `$GITHUB_ENV`, never listed in the build step's `env:` block.** A fork has none of these secrets; `env:` would hand the Tauri CLI an *empty* `APPLE_SIGNING_IDENTITY`, which it reads as "sign with this identity" and fails on. The conditional export step leaves them genuinely unset, so the CLI falls back to the `"signingIdentity": "-"` in `tauri.conf.json` and a fork still builds.
- **A skipped notarization exits 0.** Missing or malformed credentials make the bundler log `skipping app notarization` and succeed, shipping a signed-but-unnotarized app that Gatekeeper rejects on any machine that has never seen it — looks green, is broken. `release.yml` therefore ends each macOS leg with a verification step gating on `Authority=Developer ID Application`, the `runtime` flag, `stapler validate`, and `source=Notarized Developer ID`. Do not weaken that gate.
- **The bundler notarizes the `.app`, not the `.dmg`.** After a signed build the DMG carries a Developer ID signature but no ticket, and `spctl -a -t open` rejects it as `Unnotarized Developer ID`. Since the DMG is what users download, `release.yml` notarizes and staples it in a separate step and re-uploads it over the asset `tauri-action` published (`gh release upload --clobber`, which resolves the draft by tag). That step runs **per matrix leg**, so both the `aarch64` and `x64` DMGs get their own submission — a shared/universal path would silently staple only one arch.
- Ordering that must hold: staple happens in the `build` job, `update-tap` hashes the DMG after `publish`, so the cask's `sha256` is of the **stapled** bytes. Moving the staple later, or the hash earlier, produces a cask whose checksum never matches the published asset.
- Updates inherit the signature: the updater bundler tars the already-stapled `.app` without re-signing, and the staple ticket lives at `Contents/CodeResources` (an ordinary file, not an xattr), so it survives the tar.
- **Windows stays unsigned** — no Authenticode cert. SmartScreen warns on first launch; a Developer ID does nothing for that.
- The tap cask's `postflight` quarantine strip (added 2026-07-07 as the free workaround for the "damaged" error) was **removed** from `Casks/dblitz.rb` on 2026-07-25, once 26.7.6 proved a notarized app passes Gatekeeper with the quarantine flag intact (`brew reinstall --cask dblitz` keeps the `com.apple.quarantine` attribute, `spctl -a -t exec` reports `source=Notarized Developer ID`, and the app launches). Do not reintroduce it: stripping the flag discards provenance, and needing it again would mean notarization has silently broken — which is the thing to investigate, not paper over.
