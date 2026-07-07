# dblitz Agent Notes

## Project

- `dblitz` is a public `tstone-1` repository for a Tauri + SvelteKit + TypeScript desktop SQLite browser.
- Treat the repository as public: do not add internal company data, private paths, secrets, or proprietary examples.
- Use a public-safe Git identity for commits; do not commit with private or company email addresses.

## Development

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
- `dblitz` is intended to be a read-only SQLite viewer. Preserve the read-only behavior when changing query execution or database opening logic.

## Release

- Versions use CalVer `YY.M.MICRO`.
- Update all version files together:
  - `package.json`
  - `src-tauri/Cargo.toml`
  - `src-tauri/tauri.conf.json`
- Update `CHANGELOG.md` before release commits.
- See `BUILD.md` for the release checklist. Its shared-tools copy path is a placeholder unless the user provides a real deployment target.

### Distribution / Homebrew tap

- Pushing a `v*` tag triggers `.github/workflows/release.yml`: it runs the quality gate, creates the GitHub release, builds/uploads artifacts (macOS `.dmg` for `aarch64` + `x64`, Windows, Linux), then the `update-tap` job auto-bumps the Homebrew cask.
- The macOS app is distributed via the Homebrew cask `dblitz` in the tap repo **`tstone-1/homebrew-dblitz`** (`Casks/dblitz.rb`). The cask URL pattern is `dblitz_<version>_<arch>.dmg` with `arch arm: "aarch64", intel: "x64"`.
- `update-tap` downloads the two macOS DMGs from the release, computes their `sha256`, and `sed`-edits `version` + both `sha256` lines in the tap's `Casks/dblitz.rb`, then commits/pushes `Bump dblitz cask to v<version>` as `tstone-1`. It pushes to a *different* repo, so it uses the **`TAP_GITHUB_TOKEN`** secret (a fine-grained PAT with Contents:read/write on `tstone-1/homebrew-dblitz`) — the default `GITHUB_TOKEN` cannot. If that secret is missing/unauthorized the `update-tap` job fails (but the build/release still succeed); re-set it with `gh secret set TAP_GITHUB_TOKEN --repo tstone-1/dblitz --body-file <file>` (interactive prompt does NOT work through a non-interactive shell — it silently stores an empty value).
- The tap is a personal/untrusted tap: first use on a machine needs `brew trust --cask tstone-1/dblitz/dblitz`. Install/upgrade the local app with `brew install --cask dblitz` / `brew upgrade --cask dblitz`. To overwrite a pre-existing non-brew install, use `brew install --cask --force dblitz` (`--adopt` only works when the on-disk version already matches).

### macOS signing / Gatekeeper "damaged" error

- The macOS build is **ad-hoc signed only, NOT Apple-notarized** (`codesign` shows `Signature=adhoc`, `flags=…(adhoc,linker-signed)`). `release.yml`'s `tauri-action` build legs pass no Apple signing env vars, so Tauri falls back to ad-hoc.
- Consequence: a **quarantined** copy of an un-notarized app is rejected by Gatekeeper as **"dblitz is damaged and can't be opened"** (misleading — the app is fine). This bites any user who downloads the `.dmg` directly from GitHub Releases.
- **Mitigation in place (2026-07-07):** the tap cask `Casks/dblitz.rb` has a `postflight` that runs `xattr -dr com.apple.quarantine "#{appdir}/dblitz.app"`, so every `brew install`/`upgrade` clears the flag automatically. This block must survive the `update-tap` job — that job only `sed`-edits the `version`/`arm:`/`intel:` lines, so it does. The manual escape hatch (for direct `.dmg` downloads, which the cask can't help) is `xattr -dr com.apple.quarantine /Applications/dblitz.app`.
- **Proper fix deferred (decision 2026-07-07):** real notarization needs a paid **Apple Developer Program** membership ($99/yr) + a Developer ID Application cert. When ready, it's a small CI change — set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` as repo secrets and pass them as `env` on the two `macos-latest` build legs; Tauri v2 then auto sign→notarize→staple. That is the only thing that also fixes direct `.dmg` downloads. Chose the free cask-side quarantine strip for now to avoid the recurring cost.
