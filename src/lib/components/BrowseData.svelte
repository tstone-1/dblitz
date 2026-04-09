<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { tick } from "svelte";
  import {
    appState,
    getTableConfig,
    ensureTableConfig,
    saveViewConfig,
    type ColumnFilter,
    type QueryResult,
  } from "$lib/store.svelte";
  import DataGrid from "./DataGrid.svelte";
  import ColumnSettings from "./ColumnSettings.svelte";

  const CHUNK_SIZE = 500;

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
  let autoSelected = false;

  // Row cache: chunks keyed by chunk index
  let rowCache = $state<Map<number, (string | null)[][]>>(new Map());
  let pendingChunks = new Set<number>();
  let epoch = 0;

  // Auto-select single table & collapse sidebar
  $effect(() => {
    if (appState.tables.length === 1 && !autoSelected && appState.dbPath) {
      autoSelected = true;
      sidebarCollapsed = true;
      selectTable(appState.tables[0].name);
    } else if (appState.tables.length > 1) {
      autoSelected = false;
    }
  });

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
    return Object.entries(columnFilters)
      .filter(([_, f]) => f.value.trim() !== "")
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
    selectedTable = name;
    globalFilter = "";
    columnFilters = {};
    columns = [];
    const cfg = ensureTableConfig(name);
    sortColumn = cfg.sort_column;
    sortAsc = cfg.sort_asc;

    await reloadData();
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
    }, 300);
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
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function setColumnColor(col: string, color: string) {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    if (color) cfg.column_colors[col] = color;
    else delete cfg.column_colors[col];
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
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
    appState.fileConfig.tables[selectedTable] = { ...cfg };
    saveViewConfig();
  }

  function resetColumnOrder() {
    if (!selectedTable) return;
    const cfg = ensureTableConfig(selectedTable);
    cfg.column_order = [];
    appState.fileConfig.tables[selectedTable] = { ...cfg };
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
          {#each appState.tables as table}
            <button
              class="table-btn"
              class:selected={selectedTable === table.name}
              onclick={() => selectTable(table.name)}
            >
              {table.name}
              <span class="cnt">{table.row_count.toLocaleString()}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if selectedTable && columns.length > 0}
      <div class="data-area">
        <div class="filter-bar">
          <input type="text" placeholder="Global filter (all columns)..." bind:value={globalFilter} oninput={debouncedReload} class="global-filter" />
          <div class="filter-help-wrap">
            <button class="filter-help-btn" class:active={showFilterHelp} onclick={() => (showFilterHelp = !showFilterHelp)} title="Filter syntax help">?</button>
            {#if showFilterHelp}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
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
  .global-filter { flex: 1; max-width: 300px; }

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
