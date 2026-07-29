# dblitz - Build Instructions

> **Distribution model:** **macOS release builds are signed with a Developer ID
> identity and notarized by Apple** (since 26.7.6) — users open them normally.
> **Windows builds are unsigned**; SmartScreen warns on first launch. Local and
> fork builds stay ad-hoc signed (`"signingIdentity": "-"` in `tauri.conf.json`)
> unless you export `APPLE_SIGNING_IDENTITY` yourself — see
> [macOS code signing and notarization](#macos-code-signing-and-notarization).

## Prerequisites

- **Node.js** 24 (for frontend tooling). Pinned in `.nvmrc`; every CI
  `setup-node` step reads that file via `node-version-file`, so there is one
  source of truth. `@types/node` is held on the matching major deliberately —
  type-checking against a newer API surface than the runtime CI ships on passes
  locally and fails at runtime. Bump `.nvmrc` and `@types/node` together.
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

## Updater

### Updater signing key

Every update payload is signed with a **minisign** keypair (Tauri's updater
format). Clients verify it against `plugins.updater.pubkey` in
`tauri.conf.json` before installing anything. This is independent of Apple/
Windows code signing and is *not* fixed by adding a Developer ID.

- **Private key + password: KeePass**, and mirrored into the repo secrets
  `TAURI_SIGNING_PRIVATE_KEY` (the key file's **contents**) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- **Public key: committed** in `tauri.conf.json`. Public by design.
- **This is a dblitz-only key.** Do not reuse screenpick's, or any other app's —
  one compromised or lost key would then orphan two installed bases.
- **Losing the private key permanently orphans the installed base.** A new key
  cannot sign for clients that already hold the old public key, so every existing
  user would have to find and reinstall dblitz by hand. There is no recovery
  path. Keep the KeePass database backed up.

Generating it (only ever for a *new* app, never to "fix" a lost key). Create the
KeePass entry and generate the password there **first**, then copy it to the
clipboard — the commands below read it from there so it never lands in shell
history, a transcript, or an agent's tool log:

```sh
npx tauri signer generate -w ~/.tauri/dblitz.key -p "$(pbpaste)" --ci
gh auth switch --user tstone-1
gh secret set TAURI_SIGNING_PRIVATE_KEY --repo tstone-1/dblitz < ~/.tauri/dblitz.key
printf '%s' "$(pbpaste)" | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo tstone-1/dblitz
```

> **The interactive form needs a real TTY.** Plain
> `npx tauri signer generate -w ~/.tauri/dblitz.key` prompts for the password,
> and in any non-interactive shell — an agent tool call, a CI step, a piped
> command — that prompt **panics** rather than falling back:
> `called Result::unwrap() on an Err value: PError { kind: Io, ... message: "Device not configured" }`.
> That is a missing terminal, not a broken install. Use the `-p "$(pbpaste)" --ci`
> form above, or run the interactive command in a real terminal.
>
> On Windows, `Get-Clipboard` replaces `pbpaste`; note PowerShell's `$(...)` has
> the same "expanded before exec" property, so the password still stays out of
> the command text you typed.

Then paste the contents of `~/.tauri/dblitz.key.pub` into `plugins.updater.pubkey`
in `tauri.conf.json`. `src/lib/updaterConfig.test.ts` fails until you do — a build
with the committed placeholder produces artifacts no client can verify.

> Use `printf '%s'`, not `echo` — a trailing newline becomes part of the secret
> and surfaces later as a bogus "wrong password" signing failure in CI. And note
> `gh secret set` has **no** `--body-file` flag (that's `gh release`); it takes
> `-b`, `-f`, or stdin.

**Gotchas that cost time once already:**

- **The bundler reads `TAURI_SIGNING_PRIVATE_KEY` (contents), not
  `TAURI_SIGNING_PRIVATE_KEY_PATH`.** The `_PATH` form works only for the
  `tauri signer sign` CLI. With just `_PATH` set, the build runs all the way to
  the end and *then* fails with "A public key has been found, but no private
  key".
- **A release with no `latest.json` updates nobody, and looks completely green.**
  When the signing secret is missing or empty, `tauri-action` logs "Signature not
  found for the updater JSON. Skipping upload..." and **succeeds**. The release
  ships with installers and no manifest, and the failure only surfaces as users
  silently never updating. Always `curl` the manifest after publishing and check
  that *every* platform key is present (see post-release verification below).
- **Updater endpoints must be `https`.** Tauri validates this while
  deserializing the config, so a plain-`http` endpoint makes the packaged app
  **panic on startup** rather than merely warn.
  `dangerousInsecureTransportProtocol: true` is the documented escape hatch, and
  is only ever acceptable in a throwaway local test build.
  `src/lib/updaterConfig.test.ts` asserts both the https endpoint and the absence
  of that flag.
- **The release matrix must stay `max-parallel: 1`.** `tauri-action` builds
  `latest.json` by read-modify-write against the release asset, so parallel legs
  clobber each other's platform entries. Nothing fails; the manifest is just
  incomplete. Also asserted by `updaterConfig.test.ts`.

### Verifying the updater locally

Do this after any change to the updater wiring, and before trusting a release to
reach real users. It exercises signature verification, download, in-place bundle
replacement and relaunch without publishing anything or burning a tag.

Use a **throwaway keypair** so the real private key never leaves KeePass/CI:

```sh
SCRATCH=$(mktemp -d)
npx tauri signer generate -w "$SCRATCH/test.key" -p "" --ci -f

# In tauri.conf.json, TEMPORARILY: swap in "$SCRATCH/test.key.pub"'s contents as
# `pubkey`, point `endpoints` at http://localhost:8787/latest.json, and add
# "dangerousInsecureTransportProtocol": true

# Export the key BEFORE the first build, not just the second: with
# `createUpdaterArtifacts: true`, every build tries to sign, so an unexported
# key makes even the throwaway "old" build fail with "A public key has been
# found, but no private key" and exit 1 — after it has already written the
# bundle, which makes it look like a real failure when it is only the signing
# step. Exporting up front avoids the confusion entirely.
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$SCRATCH/test.key")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""

# 1. Build the "old" app at the current version and keep it aside.
npx tauri build --bundles app
cp -R src-tauri/target/release/bundle/macos/dblitz.app "$SCRATCH/installed/"

# 2. Bump the version everywhere, rebuild -> this is the update payload.
#    Both builds need the same temporary config, or the updated app panics on
#    relaunch against a config it cannot deserialize.
npx tauri build --bundles app

# 3. Serve dblitz.app.tar.gz plus a hand-written latest.json whose `signature`
#    is the contents of dblitz.app.tar.gz.sig, with darwin-aarch64 and
#    darwin-x86_64 entries.
python3 -m http.server 8787

# 4. Launch "$SCRATCH/installed/dblitz.app"; the bar appears ~10 s later.
```

`--bundles app` is enough to produce updater artifacts on macOS (`.app.tar.gz` +
`.sig`); no DMG build required, which turns a re-test into a ~20 s incremental
build.

The server log alone proves most of the chain, and is more reliable than
eyeballing the window title:

| Signal | What it proves |
|---|---|
| `GET /latest.json` ~10 s after launch | the startup check fired |
| `GET /dblitz.app.tar.gz` after you click **Install and restart** | download ran, and the signature verified — a bad signature aborts *before* the download completes |
| a *second* `GET /latest.json` seconds later | the app relaunched and re-checked as the new version |
| `defaults read "$SCRATCH/installed/dblitz.app/Contents/Info.plist" CFBundleShortVersionString` now reads the NEW version | the in-place bundle swap succeeded |
| `last_run_version` in the real `app.json` is the new version | the post-update transition was recorded |

In the UI: the update bar offers the new version, the "dblitz was updated to vX"
bar appears once after the relaunch, and a follow-up manual check reports up to
date.

> **Clicking the button is manual.** The bar is inside the webview and resists
> scripting — synthetic clicks are blocked without Accessibility trust, and the
> button does not respond to `AXPress` despite appearing in the accessibility
> tree. Do not sink time into automating it.

> **Afterwards, revert `tauri.conf.json` and every version file**, re-run
> `cargo check` to refresh `Cargo.lock`, and clear `last_run_version` from
> `~/Library/Application Support/dblitz/app.json` — the test build shares the real
> app's bundle identifier (`com.tstone.dblitz`), so it writes its version into the
> *real* config file and would otherwise make the next real launch believe it had
> just been updated.

## macOS code signing and notarization

**Status: enabled 2026-07-25, ships from 26.7.6.** macOS release builds are
signed with a Developer ID identity and notarized by Apple, so the `.dmg` opens
and the app launches with no Gatekeeper bypass. This replaced the tap cask's
`postflight` quarantine strip (removed 2026-07-25), which only ever helped
Homebrew users and did nothing for a direct `.dmg` download.

This is **independent of the updater's minisign key** — different key, different
purpose, different failure mode. Apple's signature authenticates the bundle to
Gatekeeper; the minisign key authenticates an update payload to an already
installed dblitz. See [Updater signing key](#updater-signing-key).

### The identity is shared with screenpick, and that has a consequence

dblitz signs with the **same** Developer ID Application certificate and the same
App Store Connect notarization key as `screenpick`:

| Thing | Value |
|---|---|
| Signing identity | `Developer ID Application: Timo Stein (NVX72G8SJ8)` |
| Team ID | `NVX72G8SJ8`, G2 sub-CA, valid to 2031-07-26 |
| Notarization Key ID | `T87S5KZQ4J` |
| Certificate backup | `apple-developer-id-NVX72G8SJ8.p12` — team-scoped on purpose, since it signs both apps |

That is the normal model, not a shortcut: a Developer ID Application certificate
certifies a *team*, never an app — nothing in it names a bundle — and Apple caps
an account at five of them precisely because they are not meant to be minted
per-app. What is per-app is the bundle identifier (`com.tstone.dblitz`), not the
signing material.

The consequence to remember: **certificate rotation is a two-repo event.** On
expiry, revocation, or compromise, reissue once and then update
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` and `APPLE_SIGNING_IDENTITY`
in **both** `tstone-1/dblitz` and `tstone-1/screenpick`. A repo left behind does
not fail loudly at rotation time — it fails at its next release, and only if the
CI verification gate below is intact.

Already-published releases survive a rotation: notarization tickets stay valid
after the signing certificate expires.

Where the credentials live, how the certificate was issued, and the import traps
hit while setting it up (`errSecNoSuchKeychain` on a GUI `.cer` import, the
missing G2 intermediate, `errSecInternalComponent` from the private key's ACL)
are documented once, in
[`screenpick/BUILD.md` → macOS code signing and notarization](https://github.com/tstone-1/screenpick/blob/main/BUILD.md#macos-code-signing-and-notarization).
They are properties of the machine and the Apple account, not of either app, so
they are not duplicated here.

### Building signed locally

`APPLE_SIGNING_IDENTITY` (env) **overrides** `bundle.macOS.signingIdentity` in
`tauri.conf.json`, so the committed `"-"` can stay: plain dev builds remain
ad-hoc, and exporting the identity switches a build to signed. Hardened runtime
is on by default and is required for notarization — do not turn it off.

```sh
export APPLE_SIGNING_IDENTITY="Developer ID Application: Timo Stein (NVX72G8SJ8)"
export APPLE_API_KEY=T87S5KZQ4J
export APPLE_API_ISSUER=<ISSUER-UUID>
export APPLE_API_KEY_PATH="$HOME/.appstoreconnect/private_keys/AuthKey_T87S5KZQ4J.p8"
npx tauri build --target aarch64-apple-darwin   # and/or x86_64-apple-darwin
```

Note dblitz builds **two separate per-arch bundles**, not one universal binary
like screenpick — so a local signed build notarizes whichever target you named,
and CI submits `aarch64` and `x64` independently.

**Budget real time, and keep the machine awake.** Apple's notary service is not
fast or predictable — minutes on a good day, the better part of an hour on a bad
one. `notarytool --wait` holds an open poll for the whole duration, and idle
sleep drops it (on a laptop, default battery idle sleep can be 1 minute):

```sh
npx tauri build --target aarch64-apple-darwin &
caffeinate -dimsu -w $!     # releases itself when the build exits
```

Once the payload has uploaded, the submission survives on Apple's side
regardless — a lost poll costs a `stapler staple`, not a rebuild.

### Verify — the build does NOT fail if notarization is skipped

When the credentials are missing or malformed the bundler logs `skipping app
notarization` and **exits 0**. The result is a signed, un-notarized app that
Gatekeeper still rejects on any machine that has never seen it: the same
looks-green failure shape as a release with no `latest.json`. Always check the
artifact:

```sh
APP=src-tauri/target/aarch64-apple-darwin/release/bundle/macos/dblitz.app
codesign -dv --verbose=4 "$APP" 2>&1 | grep -E 'Authority|TeamIdentifier|flags'
xcrun stapler validate "$APP"   # "The validate action worked!"
spctl -a -vvv -t exec "$APP"    # "source=Notarized Developer ID"
```

Expect `Authority=Developer ID Application: …` and `flags=…(runtime)`.

> **The bundler notarizes the `.app`, not the `.dmg`.** After a signed build the
> DMG carries a Developer ID signature but no ticket, and
> `spctl -a -t open --context context:primary-signature <dmg>` rejects it as
> `Unnotarized Developer ID`. Since the DMG is what users download — directly
> and through the Homebrew cask — opening it would still raise "Apple cannot
> check it for malicious software": most of the benefit lost, on an artifact
> that verifies clean if you only ever check the `.app`. `release.yml`
> therefore notarizes and staples the DMG in a separate step, **per matrix
> leg**, and re-uploads it over the asset `tauri-action` published
> (`gh release upload --clobber`, which resolves the still-draft release by
> tag). Verify a release DMG with the `-t open` form above, not just `-t exec`
> on the app.

Ordering that has to hold: the staple happens in the `build` job, and
`update-tap` hashes the DMG only after `publish` — so the cask's `sha256` is of
the stapled bytes. Move the staple later, or the hash earlier, and the cask ends
up with a checksum that never matches the published asset.

**Updates inherit the signature.** The app bundler signs → notarizes → staples
the `.app`, and the updater bundler tars *that* already-stapled bundle without
re-signing. The staple ticket lives at `Contents/CodeResources`, an ordinary file
rather than an extended attribute, which is why it survives being tarred.

### CI

Six repo secrets, consumed by the two macOS legs only:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | base64 of the `.p12` (`openssl base64 -A -in …`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Timo Stein (NVX72G8SJ8)` |
| `APPLE_API_KEY` | the Key ID, `T87S5KZQ4J` |
| `APPLE_API_ISSUER` | the Issuer UUID |
| `APPLE_API_KEY_P8` | the `.p8` contents; the workflow writes it to `$RUNNER_TEMP` and points `APPLE_API_KEY_PATH` at it |

Set them with `printf '%s' … | gh secret set …` — `echo` appends a newline, and a
trailing newline in `APPLE_CERTIFICATE_PASSWORD` surfaces at the *end* of a
release build as a wrong-password error that reads like a corrupt certificate.

The Tauri CLI imports the certificate itself from `APPLE_CERTIFICATE` /
`APPLE_CERTIFICATE_PASSWORD` — no manual `security create-keychain` step, and it
sets the key partition list so `codesign` never blocks on a prompt.

> **The signing vars must be exported via `$GITHUB_ENV`, never listed in the
> build step's `env:` block.** A fork has none of these secrets; `env:` would
> then pass an **empty** `APPLE_SIGNING_IDENTITY`, which the CLI reads as "sign
> with this identity" and fails on. A conditional export step leaves them
> genuinely unset, so the CLI falls back to the ad-hoc `"-"` and the fork builds.

Each macOS leg ends with a verification step that greps for
`Authority=Developer ID Application`, the `runtime` flag, a successful
`stapler validate`, and `source=Notarized Developer ID` — because a skipped
notarization exits 0 (see above), and without that gate an unnotarized release
ships looking green.

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
- [ ] Full quality gate passes (frontend type-check, unit tests, build, Rust
  fmt check, tests, clippy — the same checks CI runs after tagging): `npm run
  quality`
- [ ] All changes tested and working: `npm run tauri dev`

**Version & documentation:**
- [ ] Update version in all four files:
  - `src-tauri/Cargo.toml` (line 3)
  - `src-tauri/tauri.conf.json` (line 4)
  - `package.json` (line 3)
  - `package-lock.json` — **both** the top-level `"version"` and the one under
    `packages[""]`. Use `npm version <v> --no-git-tag-version`, which edits
    `package.json` and both lockfile entries in one go; hand-editing
    `package.json` alone leaves the lockfile behind on the previous version
    (26.7.6 shipped that way).
- [ ] Verify all version files agree:
  ```bash
  rg -n '"version"|^version =' package.json package-lock.json src-tauri/Cargo.toml src-tauri/tauri.conf.json | head
  ```
- [ ] Update `CHANGELOG.md` with new version entry and date

### 2. Build Release

```bash
npx tauri build
```

> **This exits 1 even when it succeeds.** With `createUpdaterArtifacts` enabled
> the bundler ends by signing the updater artifacts, and locally
> `TAURI_SIGNING_PRIVATE_KEY` is deliberately unset (the key lives only in
> KeePass and the repo secrets) — so the build writes the exe and both
> installers, *then* fails with "A public key has been found, but no private
> key". Judge this step by the artifact check below, not the exit code; signed
> updater artifacts only ever come from CI.

**Verify build:**
```bash
ls -lh src-tauri/target/release/dblitz.exe
```

### 3. Git Commit and Tag

```bash
git add -A
git commit -m "Release vYY.M.MICRO: Brief description"
git tag vYY.M.MICRO
git push origin main
git push origin vYY.M.MICRO
```

> **Push the tag by name, never `--tags`.** `--tags` pushes *every* local tag,
> including any left over from an experiment or a throwaway updater test build.
> Each `v*` tag that reaches the remote starts its own release workflow, so one
> stray tag publishes a release for a version that was never meant to ship.
> Pushing the one tag by name cannot do that.

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
machine through the Homebrew cask. Run `brew update` first so brew's tap clone
picks up the `update-tap` commit CI just pushed:

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
- [ ] **The updater manifest exists and covers every platform.** A release with no
      `latest.json` looks entirely green (see the Updater section above), so check
      it explicitly — expect `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64`
      and `linux-x86_64` keys, and a non-empty `signature` on each:
      ```sh
      curl -sL https://github.com/tstone-1/dblitz/releases/latest/download/latest.json \
        | python3 -m json.tool
      ```
- [ ] An older install actually offers the update (the point of all of the above):
      launch a previous build and confirm the bar appears within ~10 s
- **Windows:**
  - [ ] Run exe from build output to verify it works
  - [ ] Open a .sqlite file via double-click (file association test)
  - [ ] Check that the jump list populates after opening files
- **macOS:**
  - [ ] Installed version matches: `defaults read /Applications/dblitz.app/Contents/Info.plist CFBundleShortVersionString`
  - [ ] **The published DMG is notarized, not just the app inside it** — CI gates
        this, but verify the artifact users actually download, for both arches:
        ```sh
        gh release download "v${VERSION}" --pattern "dblitz_${VERSION}_*.dmg" -D /tmp/dmgcheck
        for d in /tmp/dmgcheck/*.dmg; do
          spctl -a -vvv -t open --context context:primary-signature "$d"
        done   # expect "source=Notarized Developer ID" for each
        ```
  - [ ] App launches from a *quarantined* copy without the "damaged" error —
        the notarization payoff. Mount the downloaded DMG and open it, rather
        than testing the brew-installed copy: `open -a dblitz`
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
# Update version: npm version <v> --no-git-tag-version covers package.json +
# package-lock.json; edit Cargo.toml and tauri.conf.json by hand
# Verify all four version files match
rg -n '"version"|^version =' package.json package-lock.json src-tauri/Cargo.toml src-tauri/tauri.conf.json | head
# Update CHANGELOG.md
npx tauri build
cp src-tauri/target/release/dblitz.exe /path/to/shared/tools/dblitz.exe
git add -A && git commit -m "Release vYY.M.MICRO: Description"
git tag vYY.M.MICRO && git push origin main && git push origin vYY.M.MICRO
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

Version must be updated in four files:
- `src-tauri/Cargo.toml` - Rust package version
- `src-tauri/tauri.conf.json` - Tauri app version
- `package.json` - npm package version
- `package-lock.json` - npm lockfile, in **two** places (top-level `"version"`
  and `packages[""].version`). Nothing fails when this drifts, which is why it
  drifted: `npm install` silently rewrites it, so a stale lockfile version
  surfaces as an unrelated dirty file in some later session's diff.

Before publishing, the exact same `YY.M.MICRO` value must appear in all four
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
