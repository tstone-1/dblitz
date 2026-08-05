# Security Policy

## Supported versions

Only the **latest release** is supported. dblitz uses CalVer (`YY.M.MICRO`) with
no maintenance branches, so fixes ship in the next release rather than as
backports. If you are running an older build, update before reporting — the
in-app updater (Settings → Check for updates) or `brew upgrade --cask dblitz`
will get you current.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/tstone-1/dblitz/security/advisories/new).
It is private to the maintainer until an advisory is published, and it keeps the
report attached to the repository.

Useful things to include, roughly in order of value: what an attacker gains, the
steps to reproduce, the dblitz version and OS, and a sample database file if the
issue depends on one (a minimal, non-confidential one — see below).

This is a personal project maintained in spare time. Expect an acknowledgement
within about a week. There is no bug bounty. If you would like credit in the
advisory and the changelog, say so and you will get it.

## What is in scope

dblitz is a **read-only** SQLite viewer that opens files you point it at. The
things most worth reporting follow from that:

- **Anything that lets dblitz write.** The read-only guarantee is enforced in
  layers: connections open `SQLITE_OPEN_READ_ONLY` with `?immutable=1`,
  non-readonly prepared statements are rejected, and a SQLite authorizer denies
  `ATTACH`/`DETACH`, transactions, and any PRAGMA outside a read-only
  introspection allowlist. A way through **any** of those layers is a real
  finding, including one that only mutates session state or reaches a file other
  than the one opened.
- **Malicious database files.** A `.sqlite` file is untrusted input. Memory
  corruption, path traversal, command execution, or reading a file outside the
  one opened — triggered by opening a crafted database — is in scope.
- **The updater.** Anything that would cause dblitz to install a payload not
  signed by the project's key, or to accept a manifest it should reject. Update
  payloads are verified with minisign against the public key committed in
  `tauri.conf.json`.
- **Escaping the webview sandbox.** The app runs under a strict CSP with a
  narrow set of Tauri capabilities. Injection that executes script in the
  webview, or that reaches a command outside the declared capability set,
  is in scope — including by way of a crafted database or a hand-edited config
  file whose values reach the UI.

## What is out of scope

- **Windows builds are unsigned.** SmartScreen warns on first launch. This is a
  known, documented state, not a vulnerability report.
- **Vulnerabilities in SQLite, Tauri, or other dependencies**, which belong
  upstream — unless dblitz's use of them makes the impact worse than upstream's
  own assessment, in which case please do report it.
- **Resource exhaustion from a deliberately enormous or corrupt database** where
  dblitz stays responsive enough to report an error or be cancelled.
- **Findings that require an already-compromised account**, such as an attacker
  who can already edit your config files or replace the application bundle.

## A note on sample files

If a report needs a database to reproduce, send the smallest one that shows the
problem, and make sure it holds nothing you would not publish. Advisories become
public once resolved, and attachments go with them.
