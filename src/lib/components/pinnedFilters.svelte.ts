/**
 * Pinned filter state machine — opt-in persistence for column and global filters.
 *
 * Mirrors the extraction pattern of cellSelection.svelte.ts and dragReorder.svelte.ts:
 * a small factory that owns its derived state and exposes a tight API. Every
 * dependency is injected through PinnedFiltersDeps -- the host (BrowseData.svelte)
 * supplies getters/setters for the ephemeral filter state it owns AND the config
 * read/write pair (getConfig/updateConfig). This module imports nothing from the
 * store, so it is unit-testable with plain fakes like its fully-injected siblings.
 */

import type { ColumnFilterValue, ViewConfig } from "#lib/ipc.ts";
import type { PinState } from "./pinLabel";

export function livePinnedFilters(config: ViewConfig, columns: string[]): Record<string, ColumnFilterValue> {
  const live = new Set(columns);
  return Object.fromEntries(Object.entries(config.pinned_filters)
    .filter(([name]) => live.has(name))
    .map(([name, filter]) => [name, { ...filter }]));
}

export interface PinnedFiltersDeps {
  getColumns: () => string[];
  /** Currently selected table name (or null if none). */
  getSelectedTable: () => string | null;
  /** Current ephemeral column filters. */
  getColumnFilters: () => Record<string, ColumnFilterValue>;
  /** Replace the column filter map, including when a column name shadows a prototype property. */
  setColumnFilters: (cf: Record<string, ColumnFilterValue>) => void;
  /** Live global filter value. */
  getGlobalFilter: () => string;
  /** Set the global filter value. */
  setGlobalFilter: (v: string) => void;
  /** Schedule a debounced data reload after a filter change. */
  triggerReload: () => void;
  /** Read a table's persisted view config (read-only). Injected store.getTableConfig. */
  getConfig: (tableName: string) => ViewConfig;
  /** Mutate + persist a table's view config in one step. Injected store.updateTableConfig. */
  updateConfig: (tableName: string, mutate: (cfg: ViewConfig) => void) => ViewConfig;
}

export function createPinnedFilters(deps: PinnedFiltersDeps) {
  // Internal state: position of the global-filter pin context menu (or null
  // if it isn't open). The column-pin context menu lives in DataGrid.svelte.
  let globalPinCtx = $state<{ x: number; y: number } | null>(null);

  function pinStateOf(col: string): PinState {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return "none";
    const saved = deps.getConfig(selectedTable).pinned_filters;
    const pinned = Object.hasOwn(saved, col) ? saved[col] : undefined;
    if (!pinned) return "none";
    const filters = deps.getColumnFilters();
    const live = Object.hasOwn(filters, col) ? filters[col] : undefined;
    const liveVal = live?.value ?? "";
    const liveRx = live?.is_regex ?? false;
    if (liveVal === pinned.value && liveRx === pinned.is_regex) return "pinned";
    return "modified";
  }

  // Map of column → pin state for columns that have a pinned filter.
  // Consumers default to "none" for missing keys via `pinStates?.[col] ?? "none"`.
  const pinStates = $derived.by<Record<string, PinState>>(() => {
    const out: Record<string, PinState> = Object.create(null);
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return out;
    const pinned = deps.getConfig(selectedTable).pinned_filters;
    for (const col of Object.keys(pinned)) out[col] = pinStateOf(col);
    return out;
  });

  const globalFilterPinState = $derived.by<PinState>(() => {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return "none";
    const pinned = deps.getConfig(selectedTable).pinned_global_filter;
    if (pinned == null) return "none";
    return deps.getGlobalFilter() === pinned ? "pinned" : "modified";
  });

  // ---- Column-level pin actions ----------------------------------------

  function pinColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const filters = deps.getColumnFilters();
    const f = Object.hasOwn(filters, col) ? filters[col] : undefined;
    if (!f || f.value.trim() === "") return; // no-op for empty input
    deps.updateConfig(selectedTable, (cfg) => {
      cfg.pinned_filters = { ...cfg.pinned_filters, [col]: { value: f.value, is_regex: f.is_regex } };
    });
  }

  function unpinColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    if (!Object.hasOwn(deps.getConfig(selectedTable).pinned_filters, col)) return;
    deps.updateConfig(selectedTable, (cfg) => {
      delete cfg.pinned_filters[col];
    });
  }

  function revertColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const saved = deps.getConfig(selectedTable).pinned_filters;
    const pinned = Object.hasOwn(saved, col) ? saved[col] : undefined;
    if (!pinned) return;
    const cf = deps.getColumnFilters();
    deps.setColumnFilters({ ...cf, [col]: { value: pinned.value, is_regex: pinned.is_regex } });
    deps.triggerReload();
  }

  function clearColumnFilter(col: string) {
    const cf = deps.getColumnFilters();
    if (Object.hasOwn(cf, col)) {
      deps.setColumnFilters({ ...cf, [col]: { value: "", is_regex: cf[col].is_regex } });
      deps.triggerReload();
    }
  }

  /** Pin button click: pin / re-pin / unpin depending on current state. */
  function togglePinColumnFilter(col: string) {
    const state = pinStateOf(col);
    if (state === "pinned") unpinColumnFilter(col);
    else pinColumnFilter(col); // covers "none" and "modified" (re-pin)
  }

  // ---- Global-filter pin actions ---------------------------------------

  function pinGlobalFilter() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const gf = deps.getGlobalFilter();
    if (gf.trim() === "") return;
    deps.updateConfig(selectedTable, (cfg) => {
      cfg.pinned_global_filter = gf;
    });
  }

  function unpinGlobalFilter() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    if (deps.getConfig(selectedTable).pinned_global_filter == null) return;
    deps.updateConfig(selectedTable, (cfg) => {
      cfg.pinned_global_filter = null;
    });
  }

  function revertGlobalFilter() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const pinned = deps.getConfig(selectedTable).pinned_global_filter;
    if (pinned == null) return;
    deps.setGlobalFilter(pinned);
    deps.triggerReload();
  }

  function toggleGlobalFilterPin() {
    if (globalFilterPinState === "pinned") unpinGlobalFilter();
    else pinGlobalFilter();
  }

  function clearGlobalFilter() {
    deps.setGlobalFilter("");
    deps.triggerReload();
  }

  // ---- Bulk reset / clear ----------------------------------------------

  /** Discard ephemeral edits and re-apply pinned defaults. Non-destructive. */
  function resetFiltersToPinned() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const cfg = deps.getConfig(selectedTable);
    deps.setColumnFilters(livePinnedFilters(cfg, deps.getColumns()));
    deps.setGlobalFilter(cfg.pinned_global_filter ?? "");
    deps.triggerReload();
  }

  /** Wipe both ephemeral filters AND saved defaults. Destructive. */
  function clearAllFiltersIncludingPinned() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    deps.setColumnFilters({});
    deps.setGlobalFilter("");
    deps.updateConfig(selectedTable, (cfg) => {
      cfg.pinned_filters = {};
      cfg.pinned_global_filter = null;
    });
    deps.triggerReload();
  }

  /** Reset toolbar button click — Shift+click also wipes pinned defaults. */
  function handleResetClick(e: MouseEvent) {
    if (e.shiftKey) clearAllFiltersIncludingPinned();
    else resetFiltersToPinned();
  }

  // ---- Global pin context menu (right-click on the global pin button) ---

  function openGlobalPinCtx(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    globalPinCtx = { x: e.clientX, y: e.clientY };
  }
  function closeGlobalPinCtx() { globalPinCtx = null; }

  return {
    // Reactive state — getters so consumers re-read each access.
    get pinStates() { return pinStates; },
    get globalFilterPinState() { return globalFilterPinState; },
    get globalPinCtx() { return globalPinCtx; },

    // Column-level handlers (passed to DataGrid as event callbacks).
    togglePinColumnFilter,
    revertColumnFilter,
    clearColumnFilter,

    // Global filter handlers.
    toggleGlobalFilterPin,
    revertGlobalFilter,
    unpinGlobalFilter,
    clearGlobalFilter,

    // Bulk reset / clear.
    handleResetClick,

    // Global pin context menu.
    openGlobalPinCtx,
    closeGlobalPinCtx,
  };
}
