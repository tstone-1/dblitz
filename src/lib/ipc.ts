/**
 * Typed boundary for every Tauri command the frontend calls.
 *
 * This module is the ONLY place that hand-writes command-name strings and
 * argument shapes for `invoke`. Everything else imports a typed wrapper from
 * here, so a Rust command rename or signature change breaks in exactly one
 * file instead of silently drifting across seven call sites. It also owns the
 * interfaces that mirror the Rust structs returned/accepted by those commands.
 *
 * Invariant: `invoke(` appears in production code ONLY inside this file. Arg
 * keys and command names must match the `#[tauri::command]` signatures exactly
 * (Tauri camelCases Rust snake_case parameter names by default, so a Rust
 * `global_filter` arrives here as `globalFilter`).
 */

import { invoke } from "@tauri-apps/api/core";

// ---- Interfaces mirroring the Rust structs -------------------------------

export interface TableInfo {
  name: string;
  row_count: number;
}

export interface ColumnInfo {
  cid: number;
  name: string;
  col_type: string;
  notnull: boolean;
  default_value: string | null;
  pk: boolean;
}

export interface SchemaEntry {
  obj_type: string;
  name: string;
  tbl_name: string;
  sql: string | null;
}

export interface ColumnFilter {
  column: string;
  value: string;
  is_regex: boolean;
}

export interface ColumnFilterValue {
  value: string;
  is_regex: boolean;
}

export interface QueryResult {
  columns: string[];
  rows: (string | null)[][];
  total_rows: number | null;
  offset: number;
}

export interface SqlResult {
  columns: string[];
  rows: (string | null)[][];
  // Per-column declared type (e.g. "INTEGER", "TEXT"), aligned 1:1 with
  // `columns`. Empty string for a column with no declared type (a computed
  // expression). Used to pass the correct numeric/text formatting through to
  // an XLSX export of the SQL result.
  column_types: string[];
  error: string | null;
  // Non-fatal: set when the result exceeded the 50,000-row cap and only the
  // first N rows were returned. Travels alongside `rows`, not in `error`.
  truncated: boolean;
}

export interface PinnedFilter {
  value: string;
  is_regex: boolean;
}

export interface RecentFile {
  path: string;
  tint: string | null;
  label: string | null;
}

export interface ViewConfig {
  hidden_columns: string[];
  column_colors: Record<string, string>;
  sort_column: string | null;
  sort_asc: boolean;
  column_order: string[];
  pinned_filters: Record<string, PinnedFilter>;
  pinned_global_filter: string | null;
  column_widths: Record<string, number>;
}

export interface FileConfig {
  tables: Record<string, ViewConfig>;
  tint: string | null;
  label: string | null;
}

/**
 * Mirrors the Rust `UpdateStatus` (`src-tauri/src/updates.rs`), which is
 * `#[serde(rename_all = "camelCase")]` — unlike the DTOs above, whose fields
 * keep their Rust snake_case names.
 */
export interface UpdateStatus {
  /** Build that ran last, or null on a first run. */
  previousVersion: string | null;
  /** Build running now. */
  currentVersion: string;
  /** True only when a *different* version ran before this one. */
  updated: boolean;
  /**
   * False for a Linux `.deb`/`.rpm` install: the Tauri updater can only replace
   * an AppImage. The UI still reports an available version but must not offer
   * to install it.
   */
  selfUpdateSupported: boolean;
}

// ---- Command argument shapes ---------------------------------------------

export interface QueryTableArgs {
  // Non-nullable on purpose: the Rust `QueryRequest.table` is a plain `String`,
  // so a null never reaches the query at all -- it fails Tauri's argument
  // deserialization with an opaque error. Callers narrow before invoking.
  table: string;
  offset: number;
  limit: number;
  filters: ColumnFilter[];
  globalFilter: string;
  sortColumn: string | null;
  sortAsc: boolean;
}

export interface CountRowsArgs {
  /** Non-nullable for the same reason as `QueryTableArgs.table`. */
  table: string;
  filters: ColumnFilter[];
  globalFilter: string;
}

export interface ExportXlsxArgs {
  headers: string[];
  rows: string[][];
  columnTypes: string[];
}

// ---- Typed command wrappers ----------------------------------------------
// One per `#[tauri::command]`. Kept as thin passthroughs: the value here is the
// typed signature, not added logic.

/** Path of the file the app was launched with (CLI arg / file association). */
export function getInitialFile(): Promise<string | null> {
  return invoke<string | null>("get_initial_file");
}

/** Toggle the webview devtools (dev builds only). */
export function toggleDevtools(): Promise<void> {
  return invoke("toggle_devtools");
}

/** Open a database and return its tables. */
export function openDatabase(path: string): Promise<TableInfo[]> {
  return invoke<TableInfo[]>("open_database", { path });
}

/** Close the currently-open database. */
export function closeDatabase(): Promise<void> {
  return invoke("close_database");
}

/** Load the persisted view config for the open database. */
export function loadViewConfig(): Promise<FileConfig> {
  return invoke<FileConfig>("load_view_config");
}

/** Persist the view config for the open database. */
export function saveViewConfig(config: FileConfig): Promise<void> {
  return invoke("save_view_config", { config });
}

/** Column introspection for a table. */
export function getColumns(table: string): Promise<ColumnInfo[]> {
  return invoke<ColumnInfo[]>("get_columns", { table });
}

/** Full schema (CREATE statements) of the open database. */
export function getSchema(): Promise<SchemaEntry[]> {
  return invoke<SchemaEntry[]>("get_schema");
}

/** Fetch a filtered/sorted page of rows from a table. */
export function queryTable(args: QueryTableArgs): Promise<QueryResult> {
  return invoke<QueryResult>("query_table", { ...args });
}

/** Count total rows matching a filter (used when a page's count is deferred). */
export function countRows(args: CountRowsArgs): Promise<number> {
  return invoke<number>("count_rows", { ...args });
}

/** Cancel any in-flight query/count/SQL statement. */
export function cancelQueries(): Promise<void> {
  return invoke("cancel_queries");
}

/** Run an arbitrary (read-only) SQL statement. */
export function executeSql(sql: string): Promise<SqlResult> {
  return invoke<SqlResult>("execute_sql", { sql });
}

/** Export a selection to XLSX and open it. */
export function exportToXlsx(args: ExportXlsxArgs): Promise<void> {
  return invoke("export_to_xlsx", { ...args });
}

/** Current Excel export directory (null-coalesced to "" by callers). */
export function getExportDir(): Promise<string | null> {
  return invoke<string | null>("get_export_dir");
}

/** Set (or reset, with null) the Excel export directory. */
export function setExportDir(dir: string | null): Promise<void> {
  return invoke("set_export_dir", { dir });
}

/** Recently-opened databases (backend filters out files that no longer exist). */
export function getRecentFiles(): Promise<RecentFile[]> {
  return invoke<RecentFile[]>("get_recent_files");
}

/** Clear the recent-files list. */
export function clearRecentFiles(): Promise<void> {
  return invoke("clear_recent_files");
}

/**
 * This launch's version transition and whether this install can self-update.
 * Resolved once by Rust during setup — calling it repeatedly returns the same
 * answer, because reading it is destructive on the Rust side (see
 * `ConfigStore::record_run_version`).
 */
export function updateStatus(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("update_status");
}

/** Whether the automatic post-launch update check is enabled. */
export function getCheckForUpdatesOnStartup(): Promise<boolean> {
  return invoke<boolean>("get_check_for_updates_on_startup");
}

/** Persist the automatic-update-check opt-out. */
export function setCheckForUpdatesOnStartup(enabled: boolean): Promise<void> {
  return invoke("set_check_for_updates_on_startup", { enabled });
}
