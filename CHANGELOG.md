# Changelog

All notable changes to dblitz are documented in this file.

Versioning follows [CalVer](https://calver.org/) using `YY.M.MICRO` format
(e.g., `26.5.0` = first May 2026 release).

## [26.5.2] - 2026-05-14

- Split the Rust database backend into focused schema, query, SQL execution, export, and benchmark modules.
- Added a full local quality gate and wired it into the release workflow before release creation.
- Added frontend unit tests for persisted state loading, cell selection, and column drag reorder behavior.
- Hardened rowid-index pagination for non-chunk-aligned offsets.
- Reduced release inspection surface by disabling global Tauri injection and gating devtools commands to debug builds.

## [26.5.1] - 2026-05-14

- Removed unused shell command capability and plugin wiring.
- Hardened database query inputs, filter validation, and regex paging.
- Fixed rowid index cache invalidation when switching database files.
- Fixed virtual selection copy/export and partial selection statistics for unloaded rows.
- Hardened persisted SQL history/theme loading and refreshed build documentation.

## [26.5.0] - 2026-05-14

Initial public release. See the [README](./README.md) for a feature overview and comparison with DB Browser for SQLite.
