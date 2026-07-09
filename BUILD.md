# dblitz - Build Instructions

## Prerequisites

- **Node.js** 18+ (for frontend tooling)
- **Rust** (latest stable via [rustup](https://rustup.rs/))
- **ripgrep** (`rg`) for release checklist verification commands
- **Windows**: Visual Studio Build Tools with "Desktop development with C++" workload

## Development

### Install Dependencies

```bash
npm install
```

### Run in Development Mode

```bash
npm run tauri dev
```

Starts the Tauri dev server with hot-reload for frontend changes. Rust backend changes trigger automatic recompilation.

### Code Quality Commands

```bash
# Frontend type-check
npm run check

# Frontend unit tests
npm test

# Rust check (no full build)
cd src-tauri && cargo check

# Rust linter
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings

# Rust unit tests
cd src-tauri && cargo test

# Rust formatter (apply)
cd src-tauri && cargo fmt

# Rust formatter (CI-style, fails on diff)
cd src-tauri && cargo fmt --check
```

## Build Output

### Windows

**Portable executable** (recommended):
- `src-tauri/target/release/dblitz.exe`

**Installers** (in `src-tauri/target/release/bundle/`):
- `nsis/dblitz_x.y.z_x64-setup.exe` - NSIS installer (registers file associations)
- `msi/dblitz_x.y.z_x64_en-US.msi` - MSI installer

## Release Procedure

### 1. Pre-release Checklist

**Update toolchains and dependencies:**
- [ ] Update Rust toolchain: `rustup update stable`
- [ ] Update Rust dependencies: `cd src-tauri && cargo update`
  - Review output for major version bumps — check changelogs before proceeding.
- [ ] Update npm dependencies: `npm update && npm outdated`
  - `npm outdated` shows remaining major-version updates. Review individually.
- [ ] No Rust vulnerabilities: `cd src-tauri && cargo audit` (install: `cargo install cargo-audit`)
- [ ] No npm vulnerabilities: `npm audit`

**Code quality:**
- [ ] Frontend type-check passes: `npm run check`
- [ ] Rust linter passes: `cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings`
- [ ] All changes tested and working: `npm run tauri dev`

**Version & documentation:**
- [ ] Update version in all three files:
  - `src-tauri/Cargo.toml` (line 3)
  - `src-tauri/tauri.conf.json` (line 4)
  - `package.json` (line 3)
- [ ] Verify all version files agree:
  ```bash
  rg -n '"version"|^version =' package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
  ```
- [ ] Update `CHANGELOG.md` with new version entry and date

### 2. Build Release

```bash
npx tauri build
```

**Verify build:**
```bash
ls -lh src-tauri/target/release/dblitz.exe
```

### 3. Git Commit and Tag

```bash
git add -A
git commit -m "Release vYY.M.MICRO: Brief description"
git tag vYY.M.MICRO
git push origin main --tags
```

Pushing the `vYY.M.MICRO` tag triggers `.github/workflows/release.yml`, which
runs the quality gate, creates a draft release, builds and uploads every
platform artifact (Windows, macOS `aarch64` + `x64` DMGs, Linux), flips the
release to published, then auto-bumps the Homebrew cask in
`tstone-1/homebrew-dblitz`. You do **not** build the cross-platform artifacts
locally — CI does. `npx tauri build` is only for a local desktop artifact.

**Release hygiene checks:**
- [ ] Local tag matches the version files exactly: `git describe --tags --exact-match`
- [ ] GitHub has the pushed tag: `git ls-remote --tags origin vYY.M.MICRO`
- [ ] Release workflow succeeded end-to-end: `gh run watch <run-id> --exit-status`
- [ ] Published (not draft) GitHub release exists for the tag: `gh release view vYY.M.MICRO --json isDraft`

> **Gotcha — transient `publish`-job cancellation.** The `publish` job (a
> one-line `gh release edit --draft=false`) is occasionally **cancelled** by a
> GitHub Actions infra flake even when all four build legs succeed; `update-tap`
> then shows `skipped` and the release is left as a draft with all assets
> present. This is not a code failure. Re-run just the tail jobs — the builds are
> not rebuilt: `gh run rerun <run-id> --failed`, then re-watch. (Seen on both
> `26.7.1` publish attempts, 2026-07-09.)

### 4. Deploy locally / to shared tools

**Windows** — copy the portable exe to a shared tools folder (stable filename, no version suffix):

```bash
cp src-tauri/target/release/dblitz.exe /path/to/shared/tools/dblitz.exe
```

**macOS** — there is no shared-exe step; deploy the just-released build to this
machine through the Homebrew cask (which also strips the Gatekeeper quarantine
via its `postflight`, avoiding the "damaged" error). Run `brew update` first so
brew's tap clone picks up the `update-tap` commit CI just pushed:

```bash
osascript -e 'quit app "dblitz"'      # if running, so the app bundle can be replaced
brew update                            # refresh the tap clone to the new cask version
brew upgrade --cask dblitz             # or: brew install --cask dblitz (first time)
```

First use of the tap on a machine needs `brew trust --cask tstone-1/dblitz/dblitz`.
To overwrite a pre-existing non-brew install, use `brew install --cask --force dblitz`.

### 5. Post-release Verification

- [ ] Confirm GitHub shows the new release as latest: `gh release list --limit 5`
- [ ] Confirm the tap cask bumped to the new version (both `sha256` lines updated)
- **Windows:**
  - [ ] Run exe from build output to verify it works
  - [ ] Open a .sqlite file via double-click (file association test)
  - [ ] Check that the jump list populates after opening files
- **macOS:**
  - [ ] Installed version matches: `defaults read /Applications/dblitz.app/Contents/Info.plist CFBundleShortVersionString`
  - [ ] No quarantine flag: `xattr -p com.apple.quarantine /Applications/dblitz.app` (expect "No such xattr")
  - [ ] App launches without the "damaged" error: `open -a dblitz`
  - [ ] After opening a database, it appears in the Dock icon's right-click **Recent** menu and **File → Open Recent** (`NSDocumentController`, added 26.7.1)

## Quick Reference

```bash
# Full release process (replace x.y.z with actual version)
rustup update stable
cd src-tauri && cargo update && cd ..
npm update && npm outdated
npm audit
cd src-tauri && cargo audit && cd ..
npm run check
cd src-tauri && cargo clippy --all-targets --all-features -- -D warnings && cd ..
# Update version in Cargo.toml, tauri.conf.json, package.json
# Verify all three version files match
rg -n '"version"|^version =' package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
# Update CHANGELOG.md
npx tauri build
cp src-tauri/target/release/dblitz.exe /path/to/shared/tools/dblitz.exe
git add -A && git commit -m "Release vYY.M.MICRO: Description"
git tag vYY.M.MICRO && git push origin main --tags
git describe --tags --exact-match
git ls-remote --tags origin vYY.M.MICRO
gh release view vYY.M.MICRO
gh release list --limit 5
```

## Version Management

Versions follow [CalVer](https://calver.org/) using the `YY.M.MICRO` format:

| Segment | Meaning | Example |
|---------|---------|---------|
| **YY** | Two-digit year | 26 = 2026 |
| **M** | Month (no zero-padding) | 4 = April |
| **MICRO** | Sequential release within that month, starting at 0 | 0, 1, 2... |

Examples: `26.4.0` (first April 2026 release), `26.4.1` (second), `26.5.0` (first May release).

Version must be updated in three files:
- `src-tauri/Cargo.toml` - Rust package version
- `src-tauri/tauri.conf.json` - Tauri app version
- `package.json` - npm package version

Before publishing, the exact same `YY.M.MICRO` value must appear in all three
files, the local tag must be `vYY.M.MICRO`, and the GitHub release must point to
that tag. Do not leave a tag, release, or version file behind on an older patch.

## Icons

Application icons are in `src-tauri/icons/`. To regenerate from a source PNG:

```bash
npm run tauri icon src-tauri/icons/icon.png
```

## Troubleshooting

### Rust Compilation Errors

```bash
rustup update
cd src-tauri && cargo clean
npx tauri build
```

### WebView2 Issues (Windows)

WebView2 runtime ships with Windows 11 and recent Windows 10 updates. For older systems, download from [Microsoft](https://developer.microsoft.com/en-us/microsoft-edge/webview2/).

### Port 1420 Already in Use

```bash
npx kill-port 1420
```

## File Structure

```
dblitz/
├── src/                          # Svelte frontend
│   ├── routes/
│   │   └── +page.svelte          # App shell
│   ├── lib/
│   │   ├── store.svelte.ts       # Global reactive state
│   │   └── components/          # UI components plus tested feature helpers
│   ├── app.css                   # Global styles + theme vars
│   └── app.html                  # HTML template
├── src-tauri/                    # Rust backend
│   ├── src/
│   │   ├── main.rs               # Entry point
│   │   ├── lib.rs                # Tauri commands & setup
│   │   ├── db.rs                 # Database facade
│   │   ├── db/                   # Schema/query/export/SQL modules
│   │   └── config.rs             # Per-DB config persistence
│   ├── icons/                    # App icons
│   ├── Cargo.toml                # Rust dependencies
│   └── tauri.conf.json           # Tauri config
├── package.json                  # npm config
├── CHANGELOG.md                  # Version history
└── BUILD.md                      # This file
```
