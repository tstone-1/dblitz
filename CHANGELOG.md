# Changelog

All notable changes to dblitz are documented in this file.

Versioning follows [CalVer](https://calver.org/) using `YY.M.MICRO` format
(e.g., `26.5.0` = first May 2026 release).

## [26.5.1] - 2026-05-14

- Removed unused shell command capability and plugin wiring.
- Hardened database query inputs, filter validation, and regex paging.
- Fixed rowid index cache invalidation when switching database files.
- Fixed virtual selection copy/export and partial selection statistics for unloaded rows.
- Hardened persisted SQL history/theme loading and refreshed build documentation.

## [26.5.0] - 2026-05-14

Initial public release. See the [README](./README.md) for a feature overview and comparison with DB Browser for SQLite.
