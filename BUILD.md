# dblitz - Build Instructions

> **Distribution model:** **macOS release builds are signed with a Developer ID
> identity and notarized by Apple** (since 26.7.6) — users open them normally.
> **Windows release builds are signed with a Certum Open Source Code Signing
> certificate** (releases after 26.10.0), by the Windows leg of `release.yml` —
> see [Windows code signing](#windows-code-signing). A local Windows build is
> unsigned. Local and fork macOS builds stay ad-hoc signed
> (`"signingIdentity": "-"` in `tauri.conf.json`) unless you export
> `APPLE_SIGNING_IDENTITY` yourself — see
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

`npm run quality` runs all of the below in the order CI does. Use it before a
release; use the individual commands while iterating.

```bash
# Everything CI runs, in one command
npm run quality

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

**There is no Windows cross-check from macOS.**
`cargo check --target x86_64-pc-windows-msvc` cannot run there: `ring` and
`libsqlite3-sys` have C build scripts and the MSVC toolchain does not exist on
macOS. `cfg(windows)` code edited on a Mac is verified by reading plus the
Windows leg of `.github/workflows/checks.yml`, and nothing else - push the
branch and read that job before tagging. On Windows, WSL covers the Linux leg
the same way (see AGENTS.md).

## Build Output

### Windows

**Portable executable** (recommended):
- `src-tauri/target/release/dblitz.exe`

**Installer** (in `src-tauri/target/release/bundle/`):
- `nsis/dblitz_x.y.z_x64-setup.exe` - NSIS installer (registers file associations)

No MSI is built: `src-tauri/tauri.windows.conf.json` limits the Windows bundle
to `nsis`, and [Windows code signing](#windows-code-signing) has the reason.
The other platforms still build everything (`"targets": "all"`).

### macOS

In `src-tauri/target/release/bundle/`:
- `macos/dblitz.app` - the application bundle (what gets signed, notarized and stapled)
- `dmg/dblitz_x.y.z_<arch>.dmg` - the disk image users download
- `macos/dblitz.app.tar.gz` (+ `.sig`) - the updater payload, from `createUpdaterArtifacts`

`<arch>` is `aarch64` or `x64`. CI builds each arch with an explicit
`--target`, which inserts the triple into the path:
`src-tauri/target/<triple>/release/bundle/...`. A local build with no `--target`
uses the plain `release/` path above.

### Linux

In `src-tauri/target/release/bundle/`, one x86_64 artifact per subdirectory:
- `deb/*.deb`
- `rpm/*.rpm`
- `appimage/*.AppImage`

Only the AppImage can self-update; `.deb` and `.rpm` are owned by the package
manager.

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

## Windows code signing

**Status: wired up 2026-10-08, first used by the release after 26.10.0.** The
installer, the uninstaller it writes, `dblitz.exe` inside it and the portable
`dblitz.exe` on the release page are signed by the Windows leg of
`release.yml`. 26.10.0 and everything before it is unsigned.

This is **independent of the updater's minisign key**, like the Apple
signature: Windows reads the code signature, an installed dblitz reads the
minisign signature and nothing else. A stolen signing login lets somebody sign
their own program under this name; it does not let them update anybody's
dblitz.

### The certificate is shared with tpdf and screenpick

| Thing | Value |
|---|---|
| Certificate | Certum *Open Source Code Signing in the Cloud* |
| Subject | `CN=Open Source Developer Timo Stein` |
| Issuer | *Certum Code Signing 2021 CA* |
| Valid until | 2027-10-07 |
| Key | In Certum's SimplySign service. It cannot be exported |

The certificate names a person, not an application, so the same one signs
`tpdf` and `screenpick`. The method below was worked out in `tpdf`, whose
[`BUILD.md` → Signing with the Certum certificate](https://github.com/tstone-1/tpdf/blob/main/BUILD.md#signing-with-the-certum-certificate)
records the measurements: the runs, the four rehearsal tags that each failed
one step later than the last, and the release that shipped an unsigned
uninstaller. They are not repeated here. What follows from sharing:

- **Renewal in October 2027 is a three-repository event**, like the Apple
  certificate. The login does not change with a renewal, but the subject may,
  and each repository's `Verify the Windows build is signed` step names it.
- **A concurrency group ends at the repository.** Every job in this repository
  that signs is in the group `certum-signing`, but a release of `tpdf` or
  `screenpick` is invisible to it. Do not push a tag or start a rehearsal here
  while one of theirs is signing: see the three rules below.

### How a release is signed

Certum supports one way to use the key: log in to its desktop program with the
account's e-mail address and a six-digit code, after which the certificate
appears in the user's certificate store. A hosted runner has no person and no
phone, so the login is done by [`ssign`](https://github.com/Le-Syl21/ssign)
(MIT), an **unofficial** client for the SimplySign service. The Windows leg:

1. Fails at once if either secret is empty.
2. Takes `ssign` v0.1.7, built from the pinned commit
   `5fd4daf22155b19b645aef3dd467f3c4c9e440b3`, out of a cache whose key is
   that commit, and builds it with `cargo install --locked` when the cache has
   none. It is built outside the checkout and into a folder of its own under
   `RUNNER_TEMP`, so the Rust cache of the build, whose keys do not name that
   commit, never holds the program that reads the login. See
   [The cache of the signing client](#the-cache-of-the-signing-client).
3. Gives Tauri `src-tauri/tauri.signing.conf.json`, an overlay that sets
   `bundle.windows.signCommand` to `sign-windows.cmd`. Tauri then calls it for
   `dblitz.exe`, for the NSIS plugin DLLs, for the uninstaller (from inside
   makensis) and for the installer. The first call logs in and the later ones
   reuse the session, so one build is one login.
4. Signs the portable `dblitz.exe` in a step of its own, with the same
   wrapper and inside the session the build opened. Tauri signs
   `target\release\dblitz.exe`, packs it into the installer, and then puts
   the unsigned file back, so the file left in that folder is never the
   signed one.
   Then removes the session `ssign` leaves behind and prints the signing log,
   both also when the build failed.
5. Reads the signatures back with `scripts/verify-signature.ps1` and fails
   unless each is valid, timestamped and by the signer: the installer and the
   portable `dblitz.exe`, then, after running the installer silently, every
   executable file in the folder it installed to, the uninstaller among them.
6. Uploads the portable `dblitz.exe`, which is the file step 5 read.

The login is two secrets of the GitHub environment `signing`, and of no other
place: `CERTUM_EMAIL`, and `CERTUM_OTP_URI`, the whole `otpauth://` address
Certum shows once at activation. Keep the whole address and not only its
`secret`: `ssign` reads `algorithm`, `digits` and `period` from it, and an
authenticator that assumes SHA-1 shows six digits that are refused. Only the
Windows leg names the environment, so the Linux and macOS legs cannot read the
secrets. The environment accepts the `main` branch and `v*` tags.

Setting it up, once (not yet done for this repository on 2026-10-08):

```sh
gh auth switch --user tstone-1
gh api -X PUT repos/tstone-1/dblitz/environments/signing --input - <<'JSON'
{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
JSON
gh api -X POST repos/tstone-1/dblitz/environments/signing/deployment-branch-policies -f name=main -f type=branch
gh api -X POST repos/tstone-1/dblitz/environments/signing/deployment-branch-policies -f name='v*' -f type=tag

# Each value from the clipboard, so it is never in the command text.
pbpaste | gh secret set CERTUM_EMAIL --env signing --repo tstone-1/dblitz
pbpaste | gh secret set CERTUM_OTP_URI --env signing --repo tstone-1/dblitz

# Read back: two policies, `main branch` and `v* tag`, and two secret names.
gh api repos/tstone-1/dblitz/environments/signing/deployment-branch-policies \
  --jq '.branch_policies[] | "\(.name) \(.type)"'
gh secret list --env signing --repo tstone-1/dblitz
```

A job that names an environment which does not exist creates it, empty and
open to every branch. The first step of the Windows leg then fails on the
missing secrets, so nothing is signed, but delete such an environment and
create it as above.

### Five things a build like this got wrong before

Four were paid for in `tpdf`, and the code here started from the fixed state.
The fifth was found here, by the first tag. `src/lib/windowsSigning.test.ts`
holds each in place.

| What went wrong | What prevents it here |
|---|---|
| The overlay's path was put together in the job matrix, where `github.workspace` is empty, so Tauri was given `/src-tauri/tauri.signing.conf.json` | The matrix carries the flag `signs-windows`; the path is built in the step |
| `failed to run ssign` and no reason: Tauri shows nothing of a sign command that failed | `scripts/sign-windows.ps1` writes everything `ssign` prints to a log, and the leg prints it |
| `atomically replacing ...: Access is denied. (os error 5)` on the application's executable, a fraction of a second after Tauri had written to it. `ssign` replaces a file by renaming a signed copy over it | `ssign` signs into a folder of its own and the script copies the bytes back, again for up to thirty seconds |
| An unsigned `uninstall.exe` in a release whose other files were signed. makensis starts the sign command by its quoted name through `PATH` from its own folder, the wrapper looked for its script beside itself (`%~dp0`), and makensis printed `UninstFinalize command returned 64` and went on | The wrapper takes the script's path from `DBLITZ_SIGN_SCRIPT`, and the leg installs what it built and reads `uninstall.exe` where it lands |
| The portable `dblitz.exe` read `NotSigned` although the signing log said it was written. The first tag `v26.10.1` (run 37758696265, 2026-10-08) stopped there and published nothing. Tauri's bundler copies the executable before it packs, signs the file in place, packs it, and restores the copy: "Restore unsigned and unpatched binary" | The step `Sign the portable exe` signs the restored file after the build, inside the build's session, and the verification reads it before it is uploaded |

### Three rules the login brings

One code is good for one login, so two jobs that log in within the same 30
seconds make the second fail, and repeated failed logins can lock the account:

- Everything here that signs is in the concurrency group `certum-signing`.
- Do not log in to the desktop program while a signing job runs.
- Do not let two of the three repositories sign at the same time.

`ssign` keeps its session for twenty minutes in a file, under `%TEMP%\ssign`
or `.cache\ssign` in the home folder; both workflows remove it.

### The rehearsal

`sign-rehearsal.yml` proves the build of `ssign` and the login without
building dblitz and without a tag: it signs a plain executable and the
installer of a published, unsigned release (26.10.0 by default), starts the
wrapper the way makensis does, holds one of the files open so the copy has to
wait, reads both signatures back and installs from the signed installer. It
uploads nothing.

```sh
gh workflow run sign-rehearsal.yml --ref main
gh run list --workflow sign-rehearsal.yml --limit 1
```

Run it on `main` after the environment is first set up, whenever `SSIGN_REV`
changes, and when a release leg fails in the build step with a login error. It
also saves the built client for the next release, as the next section says. It
does **not** cover the release leg itself: the conditional environment, the overlay
reaching Tauri, and a real signature on the uninstaller are first seen on a
tag.

### The cache of the signing client

Both workflows keep the built `ssign` in a cache of GitHub Actions, so that it
is compiled once for each commit of `ssign` and not in every run. The key is
`ssign-<os>-<arch>-<SSIGN_REV>`, there is no fallback key, and the cached
folder holds this one program. `SSIGN_REV` is written once in each workflow;
the cache key and the build both read it.

Which cache a release reads is decided by GitHub, not by the workflow: a run
on a tag can restore only caches saved on the default branch, and on its own
ref. So the cache that releases use is the one `sign-rehearsal.yml` saves when
it is run on `main`. A tag that finds none builds the client as before and is
just as green; the copy it then saves belongs to that tag, and no other tag
can read it.

- After changing `SSIGN_REV`, run the rehearsal on `main` once. The key names
  the commit, so the first run after the change builds the client and saves
  it.
- A cache that is not read for 7 days is removed. The next run then builds the
  client again, and a rehearsal on `main` saves it again.
- The cached file is the program that reads the Certum login. Only a workflow
  run on `main` can replace the copy a release reads: a pull request from a
  fork cannot write a cache that `main` or a tag reads. A saved cache is never
  overwritten, so replacing it means deleting it first
  (`gh cache delete <key>`).
- Every run starts the restored client (`ssign --version`) before anything is
  signed, and copies `scripts/sign-windows.cmd` from the checkout over the
  copy the cache restored.

### There is no `.msi` any more

`ssign` signs executables only and refuses an MSI (`not a PE (no MZ
signature)`), and a release with a signed installer beside an unsigned package
is worse than one without the package. Of the eight releases 26.9.0 to 26.10.0
the `.msi` was downloaded once in total (read from the release assets'
download counts on 2026-10-08).

What it costs somebody who did install from it: a copy installed from the
`.msi` is not offered an in-app update. `windows_install_provenance` in
`lib.rs` recognises an installed copy by the registry key the NSIS installer
writes, which an MSI install does not have, so such a copy counts as portable
and is told to download the new version. Running the `-setup.exe` over it
leaves two installed copies, the MSI's and the new one. The release notes say
to uninstall the `.msi` once. This is read from the code and was not tried on
a Windows computer.

### The fallback by hand

When the workflow cannot sign, a file can be signed on a Windows computer with
Certum's SimplySign Desktop program logged in:

```
signtool sign /sha1 <thumbprint> /fd sha256 /tr http://time.certum.pl /td sha256 <file>
scripts/verify-signature.ps1 -Signer 'Open Source Developer Timo Stein' -Path <file>
```

A signature changes the file's bytes, so an installer signed this way no
longer matches the updater `.sig` made for the unsigned bytes. Sign first, then
make the `.sig`.

### What the login is worth to somebody who steals it

The two secrets are enough to sign any file as *Open Source Developer Timo
Stein* until the certificate is revoked or expires. `release.yml` and
`sign-rehearsal.yml` are the two workflows that name the environment, and
neither runs for a pull request. `ssign` is built from one pinned commit; its
dependencies are pinned by its own lock file and nothing else. A release runs
the copy of it that a run on `main` built and saved
([The cache of the signing client](#the-cache-of-the-signing-client)), so
whoever can run a workflow on `main` decides which program reads the login,
as was already true of the workflow files themselves. Its protocol is
reverse-engineered, so Certum can end it without notice; the cost of that is a
release that fails to build, not one that ships unsigned, because the Windows
leg reads every signature back.

## Release Procedure

### 1. Pre-release Checklist

**Update toolchains and dependencies:**
- [ ] Update Rust toolchain: `rustup update stable`
- [ ] Update Rust dependencies: `cd src-tauri && cargo update`
  - Confirm the update landed in the file, not only in cargo's output: `git diff --stat -- src-tauri/Cargo.lock` must list the lockfile whenever cargo printed `Updating` lines. On 2026-09-30 `cargo update` printed 55 updates and exited 0 three times in a row without writing the lockfile.
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
- [ ] Update version in all five files:
  - `src-tauri/Cargo.toml` (line 3)
  - `src-tauri/tauri.conf.json` (line 4)
  - `package.json` (line 3)
  - `package-lock.json` — **both** the top-level `"version"` and the one under
    `packages[""]`. Use `npm version <v> --no-git-tag-version`, which edits
    `package.json` and both lockfile entries in one go; hand-editing
    `package.json` alone leaves the lockfile behind on the previous version
    (26.7.6 shipped that way).
  - `src-tauri/Cargo.lock` — do **not** hand-edit. Run `cd src-tauri && cargo check`
    after bumping `Cargo.toml`; it rewrites the lock's own `dblitz` entry. Skipping
    this leaves a stale version committed, and nothing fails, because the next
    build regenerates it.
- [ ] Verify all version files agree — this is exactly what CI's `preflight` job
  runs, so a green answer here means the tag will not be rejected:
  ```bash
  node scripts/release-preflight.mjs vYY.M.MICRO
  ```
- [ ] Update `CHANGELOG.md` with new version entry and date
- [ ] Read the release notes written out in `release.yml` (`create-release`,
  the `NOTES` block). They are a literal, so nothing fails when a sentence in
  them has become false.

**Windows signing** (see [Windows code signing](#windows-code-signing)):
- [ ] If `SSIGN_REV` changed since the last release, the rehearsal has passed
  on `main`, which also saves the built client for the release:
  `gh workflow run sign-rehearsal.yml --ref main`
- [ ] No release or rehearsal of `tpdf` or `screenpick` is running, and nobody
  is logged in to the SimplySign desktop program

### 2. Build Release

```bash
npx tauri build
```

> **This exits 1 even when it succeeds.** With `createUpdaterArtifacts` enabled
> the bundler ends by signing the updater artifacts, and locally
> `TAURI_SIGNING_PRIVATE_KEY` is deliberately unset (the key lives only in
> KeePass and the repo secrets) — so the build writes the exe and the
> installer, *then* fails with "A public key has been found, but no private
> key". Judge this step by the artifact check below, not the exit code; signed
> updater artifacts only ever come from CI.

**Verify build:**
```bash
ls -lh src-tauri/target/release/dblitz.exe
```

### 3. Git Commit and Tag

```bash
# Look at the WHOLE worktree first. A release commit in a public repo is a bad
# place to discover that something unrelated came along for the ride.
git status --porcelain
git diff --stat

# Stage by explicit pathspec, never `git add -A` / `git add .`. Blanket staging
# sweeps in every untracked file in the tree - scratch scripts, a database you
# were testing against, work written for another branch - and this repository is
# public. Add any file the release genuinely touched to the list below.
git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock \
        src-tauri/tauri.conf.json CHANGELOG.md

# Read what is actually staged before committing it.
git diff --cached --stat
git status --porcelain   # anything still listed is deliberately NOT in this commit

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

> **Gotcha — notarization returns HTTP 403 "A required agreement is missing
> or has expired".** Apple has published an updated Developer Program License
> Agreement, and notarization stops until the **Account Holder** accepts it on
> https://developer.apple.com/account (a banner at the top of the page). The
> macOS legs fail at the notarize step; Windows and Linux succeed, `publish`
> and `update-tap` are skipped, and the release stays a draft, so nothing
> reaches users. After accepting, wait for the run to finish and run
> `gh run rerun <run-id> --failed`: only the failed legs rebuild. A leg that
> reaches notarization after the agreement is accepted passes in the same run.
> The identity is shared with `screenpick`, which is blocked the same way.
> (Seen on `26.10.0`, 2026-10-01.)

> **Gotcha — local `npx tauri build` fails with `LNK1114: cannot overwrite the
> original file '...libduckdb.a'; error code 5`.** Another process (typically
> the virus scanner) had the freshly written DuckDB archive open while `lib.exe`
> appended to it. Rerun the build; nothing needs cleaning. (Seen once, 26.10.0,
> right after `cargo update` moved `libduckdb-sys`.)

> **Gotcha — transient `publish`-job cancellation.** The `publish` job (a
> one-line `gh release edit --draft=false`) is occasionally **cancelled** by a
> GitHub Actions infra flake even when all four build legs succeed; `update-tap`
> then shows `skipped` and the release is left as a draft with all assets
> present. This is not a code failure. Re-run just the tail jobs — the builds are
> not rebuilt: `gh run rerun <run-id> --failed`, then re-watch. (Seen on both
> `26.7.1` publish attempts, 2026-07-09.)

### 4. Deploy locally

**Windows** — nothing to copy. An installed copy picks the release up through
the in-app updater; a fresh machine installs from the release's
`dblitz_<version>_x64-setup.exe`.

**macOS** — deploy the just-released build to this machine through the
Homebrew cask. Run `brew update` first so brew's tap clone
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
  - [ ] The release has a `-setup.exe`, its `.sig` and `dblitz.exe`, and no
        `.msi`: `gh release view vYY.M.MICRO --json assets -q '.assets[].name'`
  - [ ] The published files are signed. CI gates this on the files it built;
        this reads the ones users download. On a Windows computer, after
        downloading both, expect `Valid` and the signer twice:
        ```powershell
        Get-AuthenticodeSignature .\dblitz.exe, .\dblitz_*_x64-setup.exe |
          Format-List Path, Status, @{n='Signer';e={$_.SignerCertificate.Subject}}
        ```
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
git diff --stat -- src-tauri/Cargo.lock   # must list the lockfile if cargo printed updates
npm update && npm outdated
npm audit
cd src-tauri && cargo audit && cd ..
npm run quality
# Update version: npm version <v> --no-git-tag-version covers package.json +
# package-lock.json; edit Cargo.toml and tauri.conf.json by hand, then
# `cd src-tauri && cargo check` to rewrite Cargo.lock's own dblitz entry
# Update CHANGELOG.md (dated heading -- preflight rejects "Unreleased")
# Verify all five version files and the changelog heading -- the same script
# CI's preflight job runs, so a green answer here means the tag is accepted
node scripts/release-preflight.mjs vYY.M.MICRO
npx tauri build
git status --porcelain && git diff --stat   # review the whole tree before staging
# Explicit pathspec, never `git add -A` -- this repo is public (see step 3)
git add package.json package-lock.json src-tauri/Cargo.toml src-tauri/Cargo.lock \
        src-tauri/tauri.conf.json CHANGELOG.md
git diff --cached --stat && git commit -m "Release vYY.M.MICRO: Description"
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

Version must be updated in five files:
- `src-tauri/Cargo.toml` - Rust package version
- `src-tauri/Cargo.lock` - the crate's own entry. Not hand-edited: `cargo check`
  after the `Cargo.toml` bump rewrites it. Nothing fails when it drifts, because
  the next build regenerates it - which is how it was still on 26.8.1 while
  26.8.2 was being staged.
- `src-tauri/tauri.conf.json` - Tauri app version
- `package.json` - npm package version
- `package-lock.json` - npm lockfile, in **two** places (top-level `"version"`
  and `packages[""].version`). Nothing fails when this drifts, which is why it
  drifted: `npm install` silently rewrites it, so a stale lockfile version
  surfaces as an unrelated dirty file in some later session's diff.

Before publishing, the exact same `YY.M.MICRO` value must appear in all five
files, the local tag must be `vYY.M.MICRO`, and the GitHub release must point to
that tag. Do not leave a tag, release, or version file behind on an older patch.
`node scripts/release-preflight.mjs vYY.M.MICRO` checks all five plus a dated
`CHANGELOG.md` heading, and CI runs the same script before it creates the draft
release.

## Icons

Application icons are in `src-tauri/icons/`. **The source is
`src-tauri/icons/sqlite.svg`**, not one of the generated PNGs - `.gitignore`
says so, and regenerating from a raster output would resample it:

```bash
npx tauri icon src-tauri/icons/sqlite.svg
```

That writes every size plus `icon.ico` and `icon.icns`. The Android and iOS
output and `64x64.png` are gitignored, since no build target uses them.

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
│   │   ├── ipc.ts                # Every Tauri command wrapper + its DTOs
│   │   ├── updateState.svelte.ts # Updater UI state
│   │   ├── updaterCommands.ts    # Updater plugin calls
│   │   ├── openOutcome.ts        # "Did this open succeed?" predicate
│   │   └── components/           # UI components plus tested feature helpers
│   │       ├── browseQuery.svelte.ts     # Browse tab query state machine
│   │       ├── virtualRows.svelte.ts     # Chunked virtual row source
│   │       ├── cellSelection.svelte.ts   # Grid selection rectangles
│   │       ├── sqlEditorExtensions.ts    # CodeMirror setup (keymaps, dialect)
│   │       ├── sqlExecution.ts           # Session-owned SQL execution
│   │       ├── clipboardTable.ts         # HTML + RFC 4180 clipboard payloads
│   │       └── platformKeys.ts           # Cmd vs Ctrl labels
│   ├── app.css                   # Global styles + theme vars
│   └── app.html                  # HTML template
├── src-tauri/                    # Rust backend
│   ├── src/
│   │   ├── main.rs               # Entry point
│   │   ├── lib.rs                # Tauri commands, setup, Windows/macOS glue
│   │   ├── db.rs                 # Database facade + db::bench_api
│   │   ├── db/                   # schema, query, filters, sql, export, types, util
│   │   ├── config.rs             # Per-DB and app config persistence
│   │   └── updates.rs            # Pure update/provenance logic
│   ├── examples/                 # rowid_seek + filtered_scroll benchmarks
│   ├── capabilities/             # Tauri capability declarations
│   ├── icons/                    # App icons (source: sqlite.svg)
│   ├── Cargo.toml                # Rust dependencies
│   ├── Cargo.lock                # Carries the crate's own version
│   ├── tauri.conf.json           # Tauri config
│   ├── tauri.windows.conf.json   # Windows only: bundle the NSIS installer, no MSI
│   └── tauri.signing.conf.json   # Release overlay: the Windows sign command
├── scripts/                      # release-preflight.mjs, smoke-test.mjs,
│                                 # sign-windows.{cmd,ps1}, verify-signature.ps1
├── .github/                      # Workflows, dependabot, tauri-linux-deps.txt
├── package.json                  # npm config
├── AGENTS.md                     # Architecture and project conventions
├── SECURITY.md                   # Reporting policy
├── CHANGELOG.md                  # Version history
└── BUILD.md                      # This file
```
