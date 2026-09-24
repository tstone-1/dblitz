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
- **Windows stays unsigned** — no Authenticode cert. SmartScreen warns on first launch; a Developer ID does nothing for that.
- The tap cask's `postflight` quarantine strip (added 2026-07-07 as the free workaround for the "damaged" error) was **removed** from `Casks/dblitz.rb` on 2026-07-25, once 26.7.6 proved a notarized app passes Gatekeeper with the quarantine flag intact. Do not reintroduce it: stripping the flag discards provenance, and needing it again would mean notarization has silently broken — which is the thing to investigate, not paper over.
