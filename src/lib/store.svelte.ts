import { invoke } from "@tauri-apps/api/core";

// Types matching Rust structs
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

export interface QueryResult {
  columns: string[];
  rows: (string | null)[][];
  total_rows: number;
  offset: number;
}

export interface SqlResult {
  columns: string[];
  rows: (string | null)[][];
  rows_affected: number;
  error: string | null;
}

export interface PinnedFilter {
  value: string;
  is_regex: boolean;
}

export interface ViewConfig {
  hidden_columns: string[];
  column_colors: Record<string, string>;
  sort_column: string | null;
  sort_asc: boolean;
  selected_table: string | null;
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

export interface SqlHistoryEntry {
  sql: string;
  timestamp: number;
  error: boolean;
}

export type Theme = "light" | "dark";

// Global reactive state
export const appState = $state({
  dbPath: null as string | null,
  tables: [] as TableInfo[],
  activeTab: "structure" as "structure" | "browse" | "sql",
  loading: false,
  error: null as string | null,
  tableColumns: {} as Record<string, string[]>, // table name -> column names for autocomplete
  tableColumnTypes: {} as Record<string, Record<string, string>>, // table -> col -> declared type (for xlsx export)
  fileConfig: { tables: {}, tint: null, label: null } as FileConfig,
  sqlHistory: (typeof localStorage !== "undefined"
    ? JSON.parse(localStorage.getItem("dblitz-sql-history") ?? "[]")
    : []) as SqlHistoryEntry[],
  theme: (typeof localStorage !== "undefined"
    ? (localStorage.getItem("dblitz-theme") as Theme) ?? "light"
    : "light") as Theme,
});

export function setTheme(theme: Theme) {
  appState.theme = theme;
  document.documentElement.setAttribute("data-theme", theme);
  localStorage.setItem("dblitz-theme", theme);
}

export function initTheme() {
  document.documentElement.setAttribute("data-theme", appState.theme);
}

export async function openDatabase(path: string) {
  appState.loading = true;
  appState.error = null;
  try {
    // Fetch everything BEFORE publishing to appState. Each appState
    // assignment is a reactive trigger; if we set `tables` first and then
    // await `load_view_config`, the auto-select effect in BrowseData fires
    // against an empty fileConfig and never re-runs once the real config
    // arrives. So load it all here, then publish in one synchronous batch.
    const tables = await invoke<TableInfo[]>("open_database", { path });
    const config = await invoke<FileConfig>("load_view_config");
    // Migrate any pre-pinned-filters entries from older config files.
    for (const t of Object.values(config.tables)) {
      if (!t.pinned_filters) t.pinned_filters = {};
      if (t.pinned_global_filter === undefined) t.pinned_global_filter = null;
      if (!t.column_widths) t.column_widths = {};
    }
    if (config.tint === undefined) config.tint = null;
    if (config.label === undefined) config.label = null;
    // Fetch column names for all tables (for SQL autocomplete + as a
    // schema source for filter validation before the first query result).
    const colMap: Record<string, string[]> = {};
    const typeMap: Record<string, Record<string, string>> = {};
    await Promise.all(tables.map(async (t) => {
      try {
        const cols = await invoke<ColumnInfo[]>("get_columns", { table: t.name });
        colMap[t.name] = cols.map((c) => c.name);
        const tmap: Record<string, string> = {};
        for (const c of cols) tmap[c.name] = c.col_type;
        typeMap[t.name] = tmap;
      } catch { /* best-effort: autocomplete works without columns */ }
    }));

    // Single synchronous publish — auto-select effect sees consistent state.
    // Order matters: `appState.tables = tables` MUST be last because it's
    // the trigger for the auto-select effect in BrowseData. By the time the
    // effect fires, dbPath/fileConfig/tableColumns must already be in place
    // so that selectTable can hydrate filters and pre-populate columns.
    appState.dbPath = path;
    appState.fileConfig = config;
    appState.tableColumns = colMap;
    appState.tableColumnTypes = typeMap;
    appState.tables = tables;
    // Single-table DBs: jump straight to Browse so the user sees data
    // immediately. Multi-table DBs leave the active tab alone — the user
    // may want to inspect Structure first to pick a table.
    if (tables.length === 1) {
      appState.activeTab = "browse";
    }
  } catch (e) {
    appState.error = String(e);
  } finally {
    appState.loading = false;
  }
}

export async function closeDatabase() {
  try {
    await invoke("close_database");
  } catch (e) {
    console.error("Failed to close database:", e);
  }
  appState.dbPath = null;
  appState.tables = [];
  appState.tableColumns = {};
  appState.tableColumnTypes = {};
  appState.fileConfig = { tables: {}, tint: null, label: null };
}

export function persistSqlHistory() {
  localStorage.setItem(
    "dblitz-sql-history",
    JSON.stringify(appState.sqlHistory),
  );
}

export async function refreshTables() {
  try {
    const tables = await invoke<TableInfo[]>("get_tables");
    appState.tables = tables;
  } catch (e) {
    appState.error = String(e);
  }
}

export async function saveViewConfig() {
  try {
    await invoke("save_view_config", { config: appState.fileConfig });
  } catch (e) {
    console.error("Failed to save view config:", e);
  }
}

const defaultViewConfig: ViewConfig = {
  hidden_columns: [],
  column_colors: {},
  sort_column: null,
  sort_asc: true,
  selected_table: null,
  column_order: [],
  pinned_filters: {},
  pinned_global_filter: null,
  column_widths: {},
};

/** Read-only access — safe to call from templates/derived. */
export function getTableConfig(tableName: string): ViewConfig {
  return appState.fileConfig.tables[tableName] ?? defaultViewConfig;
}

/**
 * Re-publish a table config into appState after mutating its fields. Svelte 5's
 * `$state` proxies are deep-reactive, but reassigning the entry with a fresh
 * object is the most robust way to make sure every consumer (derived state,
 * effects) sees the change — especially when a caller mutated multiple nested
 * fields before publishing.
 */
export function commitTableConfig(tableName: string, cfg: ViewConfig) {
  appState.fileConfig.tables[tableName] = { ...cfg };
}

/** Ensures a mutable config entry exists. Call from event handlers only. */
export function ensureTableConfig(tableName: string): ViewConfig {
  if (!appState.fileConfig.tables[tableName]) {
    appState.fileConfig.tables[tableName] = {
      hidden_columns: [],
      column_colors: {},
      sort_column: null,
      sort_asc: true,
      selected_table: null,
      column_order: [],
      pinned_filters: {},
      pinned_global_filter: null,
      column_widths: {},
    };
  }
  return appState.fileConfig.tables[tableName];
}
