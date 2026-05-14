# dblitz - Build Instructions

## Prerequisites

- **Node.js** 18+ (for frontend tooling)
- **Rust** (latest stable via [rustup](https://rustup.rs/))
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

# Rust check (no full build)
cd src-tauri && cargo check

# Rust linter
cd src-tauri && cargo clippy

# Rust formatter
cd src-tauri && cargo fmt
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
- [ ] Rust linter passes: `cd src-tauri && cargo clippy`
- [ ] All changes tested and working: `npm run tauri dev`

**Version & documentation:**
- [ ] Update version in all three files:
  - `src-tauri/Cargo.toml` (line 3)
  - `src-tauri/tauri.conf.json` (line 4)
  - `package.json` (line 3)
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

### 4. Deploy to Shared Tools

Copy the portable exe to a shared tools folder (stable filename, no version suffix):

```bash
cp src-tauri/target/release/dblitz.exe /path/to/shared/tools/dblitz.exe
```

### 5. Post-release Verification

- [ ] Run exe from build output to verify it works
- [ ] Open a .sqlite file via double-click (file association test)
- [ ] Check that jump list populates after opening files

## Quick Reference

```bash
# Full release process (replace x.y.z with actual version)
rustup update stable
cd src-tauri && cargo update && cd ..
npm update && npm outdated
npm audit
cd src-tauri && cargo audit && cd ..
npm run check
cd src-tauri && cargo clippy && cd ..
# Update version in Cargo.toml, tauri.conf.json, package.json
# Update CHANGELOG.md
npx tauri build
cp src-tauri/target/release/dblitz.exe /path/to/shared/tools/dblitz.exe
git add -A && git commit -m "Release vYY.M.MICRO: Description"
git tag vYY.M.MICRO && git push origin main --tags
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
ECOdb/
├── src/                          # Svelte frontend
│   ├── routes/
│   │   └── +page.svelte          # App shell
│   ├── lib/
│   │   ├── store.svelte.ts       # Global reactive state
│   │   └── components/
│   │       ├── Toolbar.svelte
│   │       ├── BrowseData.svelte
│   │       ├── DataGrid.svelte
│   │       ├── DatabaseStructure.svelte
│   │       ├── ExecuteSQL.svelte
│   │       └── SqlEditor.svelte
│   ├── app.css                   # Global styles + theme vars
│   └── app.html                  # HTML template
├── src-tauri/                    # Rust backend
│   ├── src/
│   │   ├── main.rs               # Entry point
│   │   ├── lib.rs                # Tauri commands & setup
│   │   ├── db.rs                 # SQLite operations
│   │   └── config.rs             # Per-DB config persistence
│   ├── icons/                    # App icons
│   ├── Cargo.toml                # Rust dependencies
│   └── tauri.conf.json           # Tauri config
├── package.json                  # npm config
├── CHANGELOG.md                  # Version history
└── BUILD.md                      # This file
```
