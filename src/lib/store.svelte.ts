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

export interface ViewConfig {
  hidden_columns: string[];
  column_colors: Record<string, string>;
  sort_column: string | null;
  sort_asc: boolean;
  selected_table: string | null;
  column_order: string[];
}

export interface FileConfig {
  tables: Record<string, ViewConfig>;
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
  fileConfig: { tables: {} } as FileConfig,
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
    const tables = await invoke<TableInfo[]>("open_database", { path });
    appState.dbPath = path;
    appState.tables = tables;
    // Load per-file config
    const config = await invoke<FileConfig>("load_view_config");
    appState.fileConfig = config;
    // Fetch column names for all tables (for SQL autocomplete)
    const colMap: Record<string, string[]> = {};
    await Promise.all(tables.map(async (t) => {
      try {
        const cols = await invoke<ColumnInfo[]>("get_columns", { table: t.name });
        colMap[t.name] = cols.map((c) => c.name);
      } catch { /* best-effort: autocomplete works without columns */ }
    }));
    appState.tableColumns = colMap;
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
  appState.fileConfig = { tables: {} };
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
};

/** Read-only access — safe to call from templates/derived. */
export function getTableConfig(tableName: string): ViewConfig {
  return appState.fileConfig.tables[tableName] ?? defaultViewConfig;
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
    };
  }
  return appState.fileConfig.tables[tableName];
}
