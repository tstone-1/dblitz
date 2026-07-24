// The Rust-mirroring DTO interfaces and the typed `invoke` wrappers live in
// ipc.ts (the single IPC boundary). Import the wrappers this module needs plus
// the config types it operates on. The public openDatabase/closeDatabase/
// saveViewConfig functions below add sequencing + error handling on top of the
// raw command wrappers, hence the aliased imports.
import {
  openDatabase as openDatabaseCmd,
  closeDatabase as closeDatabaseCmd,
  loadViewConfig,
  saveViewConfig as saveViewConfigCmd,
  getColumns,
} from "$lib/ipc";
import type { TableInfo, ColumnInfo, FileConfig, ViewConfig } from "$lib/ipc";

export interface SqlHistoryEntry {
  sql: string;
  timestamp: number;
  error: boolean;
}

export type Theme = "light" | "dark";

function browserStorage(): Storage | null {
  return typeof window === "undefined" ? null : window.localStorage;
}

export function loadSqlHistory(): SqlHistoryEntry[] {
  const storage = browserStorage();
  if (!storage) return [];
  const raw = storage.getItem("dblitz-sql-history");
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) throw new Error("SQL history is not an array");
    return parsed
      .filter((entry): entry is SqlHistoryEntry =>
        typeof entry === "object" &&
        entry !== null &&
        typeof (entry as SqlHistoryEntry).sql === "string" &&
        typeof (entry as SqlHistoryEntry).timestamp === "number" &&
        typeof (entry as SqlHistoryEntry).error === "boolean",
      )
      .slice(0, 100);
  } catch {
    storage.removeItem("dblitz-sql-history");
    return [];
  }
}

export function loadTheme(): Theme {
  const storage = browserStorage();
  if (!storage) return "light";
  const raw = storage.getItem("dblitz-theme");
  return raw === "dark" || raw === "light" ? raw : "light";
}

// Global reactive state
export const appState = $state({
  dbPath: null as string | null,
  // Bumped by openDatabase on every SUCCESSFUL open, including reopening the
  // already-open path. dbPath alone can't gate a frontend reset because a
  // Toolbar/recents reopen of the current file reopens the backend connection
  // (clearing its caches) without changing dbPath -- so the row cache, schema,
  // and SQL results would otherwise keep serving the previous connection's
  // stale data. Reset effects in BrowseData/DatabaseStructure/ExecuteSQL and
  // the single-table auto-select key on this instead of (or alongside) dbPath.
  dbOpenGeneration: 0,
  tables: [] as TableInfo[],
  activeTab: "structure" as "structure" | "browse" | "sql",
  loading: false,
  error: null as string | null,
  notice: null as string | null,
  tableColumns: {} as Record<string, string[]>, // table name -> column names for autocomplete
  tableColumnTypes: {} as Record<string, Record<string, string>>, // table -> col -> declared type (for xlsx export)
  fileConfig: { tables: {}, tint: null, label: null } as FileConfig,
  sqlHistory: loadSqlHistory(),
  theme: loadTheme(),
});

export function setTheme(theme: Theme) {
  appState.theme = theme;
  document.documentElement.setAttribute("data-theme", theme);
  browserStorage()?.setItem("dblitz-theme", theme);
}

export function initTheme() {
  document.documentElement.setAttribute("data-theme", appState.theme);
}

// Database opening spans several IPC calls (open, config load, then column
// introspection). Serialize those transactions so two user actions cannot race
// the singleton backend connection, and use a generation so an older request
// that was already running stops before publishing frontend state.
let databaseRequestGeneration = 0;
let databaseRequestQueue: Promise<void> = Promise.resolve();

async function runOpenDatabase(path: string, request: number) {
  if (request !== databaseRequestGeneration) return;
  try {
    const tables = await openDatabaseCmd(path);
    if (request !== databaseRequestGeneration) return;
    const config = await loadViewConfig();
    if (request !== databaseRequestGeneration) return;
    // Fetch column names for all tables (for SQL autocomplete + as a
    // schema source for filter validation before the first query result).
    const colMap: Record<string, string[]> = {};
    const typeMap: Record<string, Record<string, string>> = {};
    await Promise.all(tables.map(async (t) => {
      try {
        const cols = await getColumns(t.name);
        colMap[t.name] = cols.map((c) => c.name);
        const tmap: Record<string, string> = {};
        for (const c of cols) tmap[c.name] = c.col_type;
        typeMap[t.name] = tmap;
      } catch { /* best-effort: autocomplete works without columns */ }
    }));
    if (request !== databaseRequestGeneration) return;

    // Single synchronous publish - auto-select effect sees consistent state.
    // Order matters: `appState.tables = tables` MUST be last because it's
    // the trigger for the auto-select effect in BrowseData. By the time the
    // effect fires, dbPath/fileConfig/tableColumns must already be in place.
    appState.dbPath = path;
    appState.fileConfig = config;
    appState.tableColumns = colMap;
    appState.tableColumnTypes = typeMap;
    // Bump BEFORE `tables` so that by the time the reset+auto-select effect
    // fires (Svelte batches effects to after this synchronous publish), the
    // new generation is already visible alongside the fresh tables.
    appState.dbOpenGeneration++;
    appState.tables = tables;
    if (tables.length === 1) appState.activeTab = "browse";
  } catch (e) {
    if (request === databaseRequestGeneration) appState.error = String(e);
  } finally {
    if (request === databaseRequestGeneration) appState.loading = false;
  }
}

export function openDatabase(path: string): Promise<void> {
  const request = ++databaseRequestGeneration;
  appState.loading = true;
  appState.error = null;
  appState.notice = null;
  const task = databaseRequestQueue.then(() => runOpenDatabase(path, request));
  databaseRequestQueue = task.catch(() => {});
  return task;
}

export async function closeDatabase() {
  const request = ++databaseRequestGeneration;
  appState.loading = true;
  const task = databaseRequestQueue.then(async () => {
    try {
      await closeDatabaseCmd();
    } catch (e) {
      console.error("Failed to close database:", e);
    }
    if (request !== databaseRequestGeneration) return;
    appState.dbPath = null;
    appState.tables = [];
    appState.tableColumns = {};
    appState.tableColumnTypes = {};
    appState.fileConfig = { tables: {}, tint: null, label: null };
    appState.loading = false;
  });
  databaseRequestQueue = task.catch(() => {});
  await task;
}

export function persistSqlHistory() {
  browserStorage()?.setItem(
    "dblitz-sql-history",
    JSON.stringify(appState.sqlHistory),
  );
}

// View-config saves happen silently in the background (every filter
// pin, column resize, sort change, ... calls saveViewConfig() as a fire-and-
// forget `void` from updateTableConfig). A failure there used to only hit
// the console, so a broken/read-only config path left the user editing view
// settings that were never actually persisted, with zero on-screen signal.
// Surface the FIRST failure of the session via the notice bar so the user
// notices at all, but don't nag on every subsequent keystroke-driven
// autosave once they've seen it - that's what this module-level flag is for.
let saveFailureNotified = false;

export async function saveViewConfig() {
  try {
    await saveViewConfigCmd(appState.fileConfig);
  } catch (e) {
    console.error("Failed to save view config:", e);
    if (!saveFailureNotified) {
      saveFailureNotified = true;
      appState.notice = `View settings could not be saved: ${String(e)}`;
    }
  }
}

const defaultViewConfig: ViewConfig = Object.freeze({
  hidden_columns: [],
  column_colors: {},
  sort_column: null,
  sort_asc: true,
  column_order: [],
  pinned_filters: {},
  pinned_global_filter: null,
  column_widths: {},
});

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
function commitTableConfig(tableName: string, cfg: ViewConfig) {
  appState.fileConfig.tables[tableName] = { ...cfg };
}

/**
 * Mutate a table's config and persist it in one step. Every caller wants both
 * the in-memory publish (`commitTableConfig`) and the on-disk save
 * (`saveViewConfig`) — no caller has ever skipped the save — so this folds
 * them together instead of pairing two calls at every site.
 */
export function updateTableConfig(
  tableName: string,
  mutate: (cfg: ViewConfig) => void,
): ViewConfig {
  const cfg = ensureTableConfig(tableName);
  mutate(cfg);
  commitTableConfig(tableName, cfg);
  void saveViewConfig();
  return cfg;
}

/** Ensures a mutable config entry exists. Call from event handlers only. */
export function ensureTableConfig(tableName: string): ViewConfig {
  if (!appState.fileConfig.tables[tableName]) {
    appState.fileConfig.tables[tableName] = {
      hidden_columns: [],
      column_colors: {},
      sort_column: null,
      sort_asc: true,
      column_order: [],
      pinned_filters: {},
      pinned_global_filter: null,
      column_widths: {},
    };
  }
  return appState.fileConfig.tables[tableName];
}
