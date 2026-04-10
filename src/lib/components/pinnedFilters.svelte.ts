/**
 * Pinned filter state machine — opt-in persistence for column and global filters.
 *
 * Mirrors the extraction pattern of cellSelection.svelte.ts and dragReorder.svelte.ts:
 * a small factory that owns its derived state and exposes a tight API. The host
 * component (BrowseData.svelte) supplies getters/setters for the ephemeral filter
 * state it owns; this helper drives the persistence layer (read/write to the
 * appState.fileConfig.tables[t].pinned_* fields via store.svelte.ts).
 */

import {
  appState,
  getTableConfig,
  ensureTableConfig,
  saveViewConfig,
} from "$lib/store.svelte";

type PinState = "none" | "pinned" | "modified";

interface ColumnFilter {
  value: string;
  is_regex: boolean;
}

export interface PinnedFiltersDeps {
  /** Currently selected table name (or null if none). */
  getSelectedTable: () => string | null;
  /** Live ephemeral column filter map. Mutated in-place by revert/clear. */
  getColumnFilters: () => Record<string, ColumnFilter>;
  /** Wholesale replace the column filter map (used by reset / clear-all). */
  setColumnFilters: (cf: Record<string, ColumnFilter>) => void;
  /** Live global filter value. */
  getGlobalFilter: () => string;
  /** Set the global filter value. */
  setGlobalFilter: (v: string) => void;
  /** Schedule a debounced data reload after a filter change. */
  triggerReload: () => void;
}

export function createPinnedFilters(deps: PinnedFiltersDeps) {
  // Internal state: position of the global-filter pin context menu (or null
  // if it isn't open). The column-pin context menu lives in DataGrid.svelte.
  let globalPinCtx = $state<{ x: number; y: number } | null>(null);

  function pinStateOf(col: string): PinState {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return "none";
    const pinned = getTableConfig(selectedTable).pinned_filters[col];
    if (!pinned) return "none";
    const live = deps.getColumnFilters()[col];
    const liveVal = live?.value ?? "";
    const liveRx = live?.is_regex ?? false;
    if (liveVal === pinned.value && liveRx === pinned.is_regex) return "pinned";
    return "modified";
  }

  // Map of column → pin state for columns that have a pinned filter.
  // Consumers default to "none" for missing keys via `pinStates?.[col] ?? "none"`.
  const pinStates = $derived.by<Record<string, PinState>>(() => {
    const out: Record<string, PinState> = {};
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return out;
    const pinned = getTableConfig(selectedTable).pinned_filters;
    for (const col of Object.keys(pinned)) out[col] = pinStateOf(col);
    return out;
  });

  const globalFilterPinState = $derived.by<PinState>(() => {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return "none";
    const pinned = getTableConfig(selectedTable).pinned_global_filter;
    if (pinned == null) return "none";
    return deps.getGlobalFilter() === pinned ? "pinned" : "modified";
  });

  // ---- Column-level pin actions ----------------------------------------

  function pinColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const f = deps.getColumnFilters()[col];
    if (!f || f.value.trim() === "") return; // no-op for empty input
    const cfg = ensureTableConfig(selectedTable);
    cfg.pinned_filters[col] = { value: f.value, is_regex: f.is_regex };
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function unpinColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    if (!(col in cfg.pinned_filters)) return;
    delete cfg.pinned_filters[col];
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function revertColumnFilter(col: string) {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const pinned = getTableConfig(selectedTable).pinned_filters[col];
    if (!pinned) return;
    const cf = deps.getColumnFilters();
    cf[col] = { value: pinned.value, is_regex: pinned.is_regex };
    deps.triggerReload();
  }

  function clearColumnFilter(col: string) {
    const cf = deps.getColumnFilters();
    if (cf[col]) {
      cf[col] = { value: "", is_regex: cf[col].is_regex };
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
    const cfg = ensureTableConfig(selectedTable);
    cfg.pinned_global_filter = gf;
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function unpinGlobalFilter() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    if (cfg.pinned_global_filter == null) return;
    cfg.pinned_global_filter = null;
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function revertGlobalFilter() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    const pinned = getTableConfig(selectedTable).pinned_global_filter;
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
    const cfg = getTableConfig(selectedTable);
    deps.setColumnFilters(
      Object.fromEntries(
        Object.entries(cfg.pinned_filters).map(([col, pf]) => [
          col,
          { value: pf.value, is_regex: pf.is_regex },
        ]),
      ),
    );
    deps.setGlobalFilter(cfg.pinned_global_filter ?? "");
    deps.triggerReload();
  }

  /** Wipe both ephemeral filters AND saved defaults. Destructive. */
  function clearAllFiltersIncludingPinned() {
    const selectedTable = deps.getSelectedTable();
    if (!selectedTable) return;
    deps.setColumnFilters({});
    deps.setGlobalFilter("");
    const cfg = ensureTableConfig(selectedTable);
    cfg.pinned_filters = {};
    cfg.pinned_global_filter = null;
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
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
