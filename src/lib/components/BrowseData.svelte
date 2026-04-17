<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { tick } from "svelte";
  import {
    appState,
    getTableConfig,
    ensureTableConfig,
    commitTableConfig,
    saveViewConfig,
    type ColumnFilter,
    type QueryResult,
  } from "$lib/store.svelte";
  import DataGrid from "./DataGrid.svelte";
  import ColumnSettings from "./ColumnSettings.svelte";
  import { createPinnedFilters } from "./pinnedFilters.svelte";
  import { createAutoSelectFirstTable } from "./autoSelectFirstTable.svelte";

  const CHUNK_SIZE = 500;
  const FILTER_DEBOUNCE_MS = 500;

  let selectedTable = $state<string | null>(null);
  let columns = $state<string[]>([]);
  let totalRows = $state(0);
  let globalFilter = $state("");
  let columnFilters = $state<Record<string, { value: string; is_regex: boolean }>>({});
  let sortColumn = $state<string | null>(null);
  let sortAsc = $state(true);
  let loading = $state(false);
  let countPending = $state(false);
  let showColumnSettings = $state(false);
  let filterDebounce: ReturnType<typeof setTimeout> | null = null;
  let sidebarCollapsed = $state(false);

  // Row cache: chunks keyed by chunk index
  let rowCache = $state<Map<number, (string | null)[][]>>(new Map());
  let pendingChunks = new Set<number>();
  let epoch = 0;

  // Auto-select the lone table when opening a single-table DB. The helper
  // owns the "did we already auto-select for this db path?" bookkeeping.
  const checkAutoSelect = createAutoSelectFirstTable((name) => {
    sidebarCollapsed = true;
    selectTable(name);
  });
  $effect(() => { checkAutoSelect(); });

  function allColsOrdered(): string[] {
    if (!selectedTable) return columns;
    const cfg = getTableConfig(selectedTable);
    if (cfg.column_order.length > 0) {
      const inOrder = new Set(cfg.column_order);
      const newCols = columns.filter((c) => !inOrder.has(c));
      return [...cfg.column_order.filter((c) => columns.includes(c)), ...newCols];
    }
    return columns;
  }

  function visCols(): string[] {
    if (!selectedTable) return columns;
    const cfg = getTableConfig(selectedTable);
    const hiddenSet = new Set(cfg.hidden_columns);
    return allColsOrdered().filter((c) => !hiddenSet.has(c));
  }

  function buildFilters(): ColumnFilter[] {
    // Drop filters for columns that no longer exist in the schema
    // (e.g. a pinned filter on a column that was renamed externally).
    const valid = new Set(columns);
    return Object.entries(columnFilters)
      .filter(([col, f]) => valid.has(col) && f.value.trim() !== "")
      .map(([col, f]) => ({ column: col, value: f.value, is_regex: f.is_regex }));
  }

  function getRow(index: number): (string | null)[] | null {
    const chunkIdx = Math.floor(index / CHUNK_SIZE);
    const chunk = rowCache.get(chunkIdx);
    if (!chunk) {
      fetchChunk(chunkIdx);
      return null;
    }
    return chunk[index - chunkIdx * CHUNK_SIZE] ?? null;
  }

  // Precomputed column name -> index for O(1) lookups
  let colIndexMap = $derived(new Map(columns.map((c, i) => [c, i])));

  // Map visible column data back to full-row indices for DataGrid
  function getVisibleRow(index: number): (string | null)[] | null {
    const fullRow = getRow(index);
    if (!fullRow) return null;
    const vc = visCols();
    return vc.map((col) => fullRow[colIndexMap.get(col) ?? 0] ?? null);
  }

  async function fetchChunk(chunkIdx: number) {
    if (pendingChunks.has(chunkIdx)) return;
    pendingChunks.add(chunkIdx);
    const myEpoch = epoch;
    const offset = chunkIdx * CHUNK_SIZE;
    try {
      const result = await invoke<QueryResult>("query_table", {
        table: selectedTable,
        offset,
        limit: CHUNK_SIZE,
        filters: buildFilters(),
        globalFilter: globalFilter.trim(),
        sortColumn,
        sortAsc,
      });

      if (myEpoch !== epoch) return;

      if (result.total_rows >= 0) totalRows = result.total_rows;
      if (columns.length === 0) columns = result.columns;

      const newCache = new Map(rowCache);
      newCache.set(chunkIdx, result.rows);
      rowCache = newCache;
    } catch (e) {
      if (myEpoch === epoch) appState.error = String(e);
    } finally {
      pendingChunks.delete(chunkIdx);
    }
  }

  async function selectTable(name: string) {
    // Cancel any pending debounced reload from the outgoing table so it can't
    // fire against the incoming one and waste a round-trip.
    if (filterDebounce) { clearTimeout(filterDebounce); filterDebounce = null; }
    selectedTable = name;
    // Pre-populate columns from the openDatabase-time autocomplete cache
    // so buildFilters() (called by reloadData below) sees the schema BEFORE
    // the first query result arrives. Without this, filters are dropped on
    // the very first query after a table switch because `valid` is empty.
    columns = appState.tableColumns[name] ?? [];
    const cfg = ensureTableConfig(name);
    sortColumn = cfg.sort_column;
    sortAsc = cfg.sort_asc;
    // Hydrate ephemeral filter state from pinned defaults.
    // Orphaned filters (pinned column no longer in schema) are silently
    // dropped at query time by buildFilters() against the live `columns`.
    columnFilters = Object.fromEntries(
      Object.entries(cfg.pinned_filters).map(([col, pf]) => [
        col,
        { value: pf.value, is_regex: pf.is_regex },
      ]),
    );
    globalFilter = cfg.pinned_global_filter ?? "";
    lastFilterState = globalFilter.trim() + JSON.stringify(columnFilters);

    await reloadData();

    // Auto-fit column widths on first open (no saved widths for this table)
    const widthCfg = getTableConfig(name).column_widths;
    if (!widthCfg || Object.keys(widthCfg).length === 0) {
      applyAutoWidths();
    }
  }

  async function reloadData() {
    if (!selectedTable) return;
    loading = true;
    epoch++;
    rowCache = new Map();
    pendingChunks.clear();
    const myEpoch = epoch;
    try {
      const filters = buildFilters();

      const result = await invoke<QueryResult>("query_table", {
        table: selectedTable, offset: 0, limit: CHUNK_SIZE,
        filters, globalFilter: globalFilter.trim(), sortColumn, sortAsc,
      });

      if (myEpoch !== epoch) return;

      if (result.columns.length > 0) columns = result.columns;
      rowCache = new Map([[0, result.rows]]);

      if (result.total_rows >= 0) {
        totalRows = result.total_rows;
        countPending = false;
      } else {
        totalRows = result.rows.length < CHUNK_SIZE ? result.rows.length : CHUNK_SIZE;
        countPending = true;
        invoke<number>("count_rows", {
          table: selectedTable, filters, globalFilter: globalFilter.trim(),
        }).then((count) => {
          if (myEpoch === epoch) {
            totalRows = count;
            countPending = false;
          }
        });
      }

      await tick();
    } catch (e) {
      if (myEpoch === epoch) appState.error = String(e);
    } finally {
      if (myEpoch === epoch) loading = false;
    }
  }

  // Plain `let` on purpose — this is a deduplication memo for debouncedReload,
  // not reactive state. Tracking it via `$state` would defeat the dedup (every
  // read/write would trigger downstream effects).
  let lastFilterState = "";

  // Must match operator prefixes parsed in db.rs build_where_clause
  const INCOMPLETE_OPS = /^(<|>|>=|<=|=)$/;

  function hasIncompleteFilter(): boolean {
    return Object.values(columnFilters).some((f) => {
      if (f.is_regex || f.value.trim() === "") return false;
      // Check every semicolon-separated segment for incomplete operator
      return f.value.split(';').some((seg) => {
        const trimmed = seg.trim();
        return trimmed !== "" && INCOMPLETE_OPS.test(trimmed);
      });
    });
  }

  function debouncedReload() {
    if (hasIncompleteFilter()) return;
    const filterSnapshot = globalFilter.trim() + JSON.stringify(columnFilters);
    if (filterSnapshot === lastFilterState) return;
    if (filterDebounce) clearTimeout(filterDebounce);
    filterDebounce = setTimeout(() => {
      lastFilterState = filterSnapshot;
      reloadData();
    }, FILTER_DEBOUNCE_MS);
  }

  function handleSort(col: string) {
    if (sortColumn === col) { sortAsc = !sortAsc; }
    else { sortColumn = col; sortAsc = true; }
    if (selectedTable) {
      const cfg = ensureTableConfig(selectedTable);
      cfg.sort_column = sortColumn;
      cfg.sort_asc = sortAsc;
      saveViewConfig();
    }
    reloadData();
  }

  function toggleColumnHidden(col: string) {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    const idx = cfg.hidden_columns.indexOf(col);
    if (idx >= 0) cfg.hidden_columns.splice(idx, 1);
    else cfg.hidden_columns.push(col);
    commitTableConfig(selectedTable, cfg);
    saveViewConfig();
  }

  function setColumnColor(col: string, color: string) {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    if (color) cfg.column_colors[col] = color;
    else delete cfg.column_colors[col];
    commitTableConfig(selectedTable, cfg);
    saveViewConfig();
  }

  function setColumnWidth(col: string, width: number) {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    cfg.column_widths[col] = width;
    // Widths are a high-churn field compared to colors/hidden, but we only
    // write on drag-end (DataGrid emits once per resize), so the save cost
    // is bounded. No need to debounce further.
    saveViewConfig();
  }

  /** Compute reasonable column widths by measuring content with canvas. */
  function computeAutoWidths(): Record<string, number> {
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d')!;

    const CELL_PAD = 24;   // 4px left + 8px right + border + buffer
    const HDR_EXTRA = 24;  // sort arrow + pin glyph space
    const MIN_W = 60;
    const MAX_W = 400;
    const MAX_SAMPLE = 100;

    const vc = visCols();
    const widths: Record<string, number> = {};
    const chunk0 = rowCache.get(0) ?? [];
    const n = Math.min(chunk0.length, MAX_SAMPLE);

    for (const col of vc) {
      // Keep font strings in sync with DataGrid.svelte .grid-cell font
      // Header text (bold)
      ctx.font = '600 12px "Cascadia Code","Cascadia Mono","Fira Code","Consolas",monospace';
      let maxW = ctx.measureText(col).width + CELL_PAD + HDR_EXTRA;

      // Data cells (normal weight)
      ctx.font = '12px "Cascadia Code","Cascadia Mono","Fira Code","Consolas",monospace';
      const ci = colIndexMap.get(col);
      if (ci !== undefined) {
        for (let i = 0; i < n; i++) {
          const val = chunk0[i][ci];
          if (val === null) {
            const w = ctx.measureText('NULL').width + CELL_PAD;
            if (w > maxW) maxW = w;
          } else if (val) {
            const w = ctx.measureText(val).width + CELL_PAD;
            if (w > maxW) maxW = w;
          }
        }
      }
      widths[col] = Math.round(Math.min(MAX_W, Math.max(MIN_W, maxW)));
    }
    return widths;
  }

  /** Apply auto-fit widths and persist them. */
  function applyAutoWidths() {
    if (!selectedTable) return;
    const widths = computeAutoWidths();
    const cfg = ensureTableConfig(selectedTable);
    cfg.column_widths = widths;
    commitTableConfig(selectedTable, cfg);
    saveViewConfig();
  }

  /** Reset saved widths and recompute from content. */
  function resetColumnWidths() {
    applyAutoWidths();
  }

  function getColumnColor(col: string): string {
    if (!selectedTable) return "";
    return getTableConfig(selectedTable).column_colors[col] || "";
  }

  function handleFilterInput(col: string, value: string) {
    if (!columnFilters[col]) columnFilters[col] = { value, is_regex: false };
    else columnFilters[col] = { ...columnFilters[col], value };
    debouncedReload();
  }

  function toggleRegex(col: string) {
    if (!columnFilters[col]) columnFilters[col] = { value: "", is_regex: true };
    else columnFilters[col] = { ...columnFilters[col], is_regex: !columnFilters[col].is_regex };
    if (columnFilters[col]?.value.trim()) debouncedReload();
  }

  // Pinned filter state machine — extracted helper.
  // The helper owns the persistence layer (read/write to appState.fileConfig)
  // and the global-filter pin context menu state. It depends on getters/setters
  // for the ephemeral filter state owned by this component.
  const pinned = createPinnedFilters({
    getSelectedTable: () => selectedTable,
    getColumnFilters: () => columnFilters,
    setColumnFilters: (cf) => { columnFilters = cf; },
    getGlobalFilter: () => globalFilter,
    setGlobalFilter: (v) => { globalFilter = v; },
    triggerReload: () => debouncedReload(),
  });

  let showFilterHelp = $state(false);

  function colorPresets(): string[] {
    if (appState.theme === "dark") {
      return ["", "#3b1c1c", "#1c3b1c", "#1c1c3b", "#3b3b1c", "#3b1c3b", "#1c3b3b", "#2d1f1f", "#1f2d1f"];
    }
    return ["", "#fde8e8", "#e8fde8", "#e8e8fd", "#fdfde8", "#fde8fd", "#e8fdfd", "#f5eded", "#edf5ed"];
  }

  function reorderColumns(fromCol: string, toCol: string) {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    let order = cfg.column_order.length > 0
      ? cfg.column_order.filter((c) => columns.includes(c))
      : [...columns];
    const fromIdx = order.indexOf(fromCol);
    const toIdx = order.indexOf(toCol);
    if (fromIdx < 0 || toIdx < 0) return;
    order.splice(fromIdx, 1);
    order.splice(toIdx, 0, fromCol);
    cfg.column_order = order;
    commitTableConfig(selectedTable, cfg);
    saveViewConfig();
  }

  function resetColumnOrder() {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    cfg.column_order = [];
    commitTableConfig(selectedTable, cfg);
    saveViewConfig();
  }

  // Build column colors map for visible columns
  let visColColors = $derived.by(() => {
    const colors: Record<string, string> = {};
    for (const col of visCols()) {
      const c = getColumnColor(col);
      if (c) colors[col] = c;
    }
    return colors;
  });
</script>

{#if !appState.dbPath}
  <div class="empty">Open a SQLite database to browse data.</div>
{:else}
  <div class="browse-layout">
    <div class="sidebar" class:collapsed={sidebarCollapsed}>
      <button class="sidebar-toggle" onclick={() => (sidebarCollapsed = !sidebarCollapsed)} title={sidebarCollapsed ? 'Show tables' : 'Hide tables'}>
        {sidebarCollapsed ? '>' : '<'}
      </button>
      {#if !sidebarCollapsed}
        <div class="table-selector">
          {#each appState.tables as table (table.name)}
            <button
              class="table-btn"
              class:selected={selectedTable === table.name}
              onclick={() => selectTable(table.name)}
            >
              {table.name}
              <span class="cnt">{table.row_count < 0 ? '?' : table.row_count.toLocaleString()}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if selectedTable && columns.length > 0}
      <div class="data-area">
        <div class="filter-bar">
          <div class="global-filter-wrap" data-pin-state={pinned.globalFilterPinState}>
            <input
              type="text"
              placeholder="Global filter (all columns)..."
              bind:value={globalFilter}
              oninput={debouncedReload}
              class="global-filter"
            />
            <button
              class="pin-btn global-pin-btn"
              data-pin-state={pinned.globalFilterPinState}
              title={
                pinned.globalFilterPinState === "pinned"
                  ? "Global filter is saved — click to unpin"
                  : pinned.globalFilterPinState === "modified"
                    ? "Saved global filter exists — click to update, right-click to revert"
                    : "Save global filter as default for this table"
              }
              onclick={pinned.toggleGlobalFilterPin}
              oncontextmenu={pinned.openGlobalPinCtx}
              aria-label="Pin global filter"
            >
              <!-- pin glyph -->
              <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
                <path d="M9.5 1.5 L14.5 6.5 L11.5 7.5 L10 12 L7 9 L3 13 L2 14 L3 10 L6 7 L3 4 L7.5 2.5 Z"
                  fill={pinned.globalFilterPinState === "none" ? "none" : "currentColor"}
                  stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
              </svg>
            </button>
          </div>
          <button
            class="reset-filters-btn"
            onclick={pinned.handleResetClick}
            title="Reset filters to saved defaults (Shift+click: also clear pinned)"
            aria-label="Reset filters"
          >Reset</button>
          <div class="filter-help-wrap">
            <button class="filter-help-btn" class:active={showFilterHelp} onclick={() => (showFilterHelp = !showFilterHelp)} title="Filter syntax help">?</button>
            {#if showFilterHelp}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <div class="filter-help-backdrop" onclick={() => (showFilterHelp = false)}></div>
              <div class="filter-help-popover">
                <div class="help-title">Column filter syntax</div>
                <table class="help-table"><tbody>
                  <tr><td class="help-example">hello</td><td>contains "hello"</td></tr>
                  <tr><td class="help-example">=hello</td><td>exactly "hello"</td></tr>
                  <tr><td class="help-example">&lt;&gt;hello</td><td>not containing "hello"</td></tr>
                  <tr><td class="help-example">&lt;&gt;</td><td>non-empty values only</td></tr>
                  <tr><td class="help-example">&gt;100</td><td>greater than 100</td></tr>
                  <tr><td class="help-example">&lt;=50</td><td>at most 50</td></tr>
                </tbody></table>
                <div class="help-divider"></div>
                <div class="help-title">Combine with <code>;</code></div>
                <table class="help-table"><tbody>
                  <tr><td class="help-example">foo;bar</td><td>contains "foo" OR "bar"</td></tr>
                  <tr><td class="help-example">&lt;&gt;A;&lt;&gt;B</td><td>excludes "A" AND "B"</td></tr>
                  <tr><td class="help-example">&gt;10;&lt;100</td><td>between 10 and 100</td></tr>
                </tbody></table>
                <div class="help-divider"></div>
                <div class="help-hint">Toggle <code>.*</code> for regex mode</div>
              </div>
            {/if}
          </div>
          <button onclick={() => (showColumnSettings = !showColumnSettings)} class="settings-btn">Columns</button>
          <span class="row-info">{countPending ? 'counting...' : `${totalRows.toLocaleString()} rows`}</span>
          {#if loading}<span class="loading-indicator">Loading...</span>{/if}
        </div>

        {#if showColumnSettings && selectedTable}
          <ColumnSettings
            columns={columns}
            hiddenColumns={getTableConfig(selectedTable).hidden_columns}
            columnOrder={getTableConfig(selectedTable).column_order}
            colorPresets={colorPresets()}
            getColumnColor={getColumnColor}
            onToggleHidden={toggleColumnHidden}
            onSetColor={setColumnColor}
            onReorder={reorderColumns}
            onResetOrder={resetColumnOrder}
          />
        {/if}

        <DataGrid
          columns={visCols()}
          totalRows={totalRows}
          getRow={getVisibleRow}
          sortColumn={sortColumn}
          sortAsc={sortAsc}
          onSort={handleSort}
          columnColors={visColColors}
          columnFilters={columnFilters}
          onFilterInput={handleFilterInput}
          onToggleRegex={toggleRegex}
          onHideColumn={toggleColumnHidden}
          onSetColumnColor={setColumnColor}
          onReorderColumn={reorderColumns}
          colorPresets={colorPresets()}
          pinStates={pinned.pinStates}
          onTogglePinFilter={pinned.togglePinColumnFilter}
          onRevertFilter={pinned.revertColumnFilter}
          onClearFilter={pinned.clearColumnFilter}
          initialColumnWidths={selectedTable ? (getTableConfig(selectedTable).column_widths ?? {}) : {}}
          onResizeColumn={setColumnWidth}
          onResetColumnWidths={resetColumnWidths}
          columnTypes={selectedTable ? (appState.tableColumnTypes[selectedTable] ?? {}) : {}}
        />
      </div>
    {:else if selectedTable && loading}
      <div class="empty">Loading...</div>
    {:else if selectedTable}
      <div class="empty">No columns found. <button onclick={() => reloadData()}>Retry</button></div>
    {:else}
      <div class="empty">Select a table to browse.</div>
    {/if}
  </div>
{/if}

{#if pinned.globalPinCtx}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="ctx-backdrop" onclick={pinned.closeGlobalPinCtx} oncontextmenu={(e) => { e.preventDefault(); pinned.closeGlobalPinCtx(); }}></div>
  <div class="ctx-menu" style="left: {pinned.globalPinCtx.x}px; top: {pinned.globalPinCtx.y}px;">
    <button class="ctx-item" onclick={() => { pinned.toggleGlobalFilterPin(); pinned.closeGlobalPinCtx(); }}>
      {pinned.globalFilterPinState === "pinned" ? "Unpin global filter" : pinned.globalFilterPinState === "modified" ? "Re-pin global filter (save current value)" : "Pin global filter (save as default)"}
    </button>
    {#if pinned.globalFilterPinState === "modified"}
      <button class="ctx-item" onclick={() => { pinned.revertGlobalFilter(); pinned.closeGlobalPinCtx(); }}>Revert to pinned value</button>
    {/if}
    <div class="ctx-sep"></div>
    <button class="ctx-item" onclick={() => { pinned.clearGlobalFilter(); pinned.closeGlobalPinCtx(); }}>Clear global filter</button>
  </div>
{/if}

<style>
  .empty {
    display: flex; align-items: center; justify-content: center;
    height: 100%; color: var(--text-muted); font-size: 14px;
  }

  .browse-layout {
    display: flex; height: 100%; overflow: hidden;
  }

  .sidebar {
    display: flex;
    flex-shrink: 0;
    border-right: 1px solid var(--border-color);
  }

  .sidebar.collapsed {
    width: auto;
  }

  .sidebar-toggle {
    writing-mode: vertical-lr;
    width: 20px;
    padding: 8px 0;
    border: none;
    border-radius: 0;
    background: var(--bg-secondary);
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .sidebar-toggle:hover { background: var(--bg-hover); color: var(--text-primary); }

  .table-selector {
    width: 160px;
    overflow-y: auto;
    padding: 4px 0;
  }

  .table-btn {
    display: flex; justify-content: space-between; width: 100%;
    padding: 5px 10px; border: none; border-radius: 0;
    text-align: left; background: transparent; font-size: 12px;
  }
  .table-btn:hover { background: var(--bg-hover); }
  .table-btn.selected { background: var(--bg-tertiary); border-left: 3px solid var(--accent); }
  .cnt { color: var(--text-muted); font-size: 10px; }

  .data-area {
    flex: 1; display: flex; flex-direction: column; overflow: hidden;
  }

  .filter-bar {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 8px; border-bottom: 1px solid var(--border-color); flex-shrink: 0;
  }
  .global-filter-wrap {
    flex: 1; max-width: 300px;
    display: flex; align-items: stretch;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: var(--bg-primary);
    overflow: hidden;
    transition: border-color 120ms;
  }
  .global-filter-wrap[data-pin-state="pinned"] { border-color: var(--accent); }
  .global-filter-wrap[data-pin-state="modified"] { border-color: var(--warning); }
  .global-filter {
    flex: 1; min-width: 0;
    border: none; background: transparent;
    padding: 3px 6px;
    font-size: 12px; color: var(--text-primary);
  }
  .global-filter:focus { outline: none; }

  .pin-btn {
    display: flex; align-items: center; justify-content: center;
    border: none; background: transparent;
    padding: 0 6px; cursor: pointer;
    color: var(--text-muted);
    transition: color 120ms, opacity 120ms;
  }
  .pin-btn[data-pin-state="none"] { opacity: 0.45; }
  .pin-btn[data-pin-state="none"]:hover { opacity: 1; color: var(--text-primary); }
  .pin-btn[data-pin-state="pinned"] { color: var(--accent); opacity: 1; }
  .pin-btn[data-pin-state="modified"] { color: var(--warning); opacity: 1; }
  .pin-btn:hover { color: var(--accent); }

  .global-pin-btn {
    border-left: 1px solid var(--border-color);
    flex-shrink: 0;
  }

  .reset-filters-btn {
    font-size: 11px;
    padding: 3px 8px;
    border: 1px solid var(--border-color);
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }
  .reset-filters-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .filter-help-backdrop {
    position: fixed; inset: 0; z-index: 49;
  }
  .filter-help-wrap { position: relative; }
  .filter-help-btn {
    width: 22px; height: 22px; border-radius: 50%;
    border: 1px solid var(--border-color); background: transparent;
    color: var(--text-muted); font-size: 12px; font-weight: 600;
    cursor: pointer; padding: 0;
    display: flex; align-items: center; justify-content: center;
  }
  .filter-help-btn:hover, .filter-help-btn.active {
    background: var(--accent); color: var(--bg-primary); border-color: var(--accent);
  }

  .filter-help-popover {
    position: absolute; top: 28px; left: -60px; z-index: 50;
    background: var(--bg-secondary); border: 1px solid var(--border-color);
    border-radius: 8px; padding: 10px 14px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.18);
    width: 260px; font-size: 12px;
  }

  .help-title {
    font-size: 11px; font-weight: 600; color: var(--text-muted);
    text-transform: uppercase; margin-bottom: 4px;
  }
  .help-title code {
    text-transform: none; background: var(--bg-tertiary);
    padding: 1px 4px; border-radius: 3px; font-size: 12px;
  }

  .help-table {
    width: 100%; border-collapse: collapse;
  }
  .help-table td {
    padding: 2px 0; vertical-align: top;
  }
  .help-example {
    font-family: 'Cascadia Code', 'Consolas', monospace;
    color: var(--accent); white-space: nowrap; padding-right: 12px !important;
    width: 1%; /* shrink to content */
  }

  .help-divider {
    height: 1px; background: var(--border-color); margin: 6px 0;
  }

  .help-hint {
    color: var(--text-muted); font-size: 11px;
  }
  .help-hint code {
    background: var(--bg-tertiary); padding: 1px 4px; border-radius: 3px;
    font-family: monospace;
  }

  .settings-btn { font-size: 12px; padding: 3px 10px; }
  .row-info { margin-left: auto; color: var(--text-secondary); font-size: 12px; }
  .loading-indicator { color: var(--warning); font-size: 11px; animation: pulse 1s infinite; }
  @keyframes pulse { 50% { opacity: 0.5; } }

</style>
