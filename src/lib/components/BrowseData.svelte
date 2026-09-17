<script lang="ts">
  import { tick, untrack } from "svelte";
  import {
    queryTable,
    cancelQueries,
    countRows,
    exportToXlsx,
    type ColumnFilterValue,
  } from "$lib/ipc";
  import {
    appState,
    getTableConfig,
    ensureTableConfig,
    updateTableConfig,
  } from "$lib/store.svelte";
  import DataGrid from "./DataGrid.svelte";
  import ColumnSettings from "./ColumnSettings.svelte";
  import ColumnFinder from "./ColumnFinder.svelte";
  import { createPinnedFilters } from "./pinnedFilters.svelte";
  import { createAutoSelectFirstTable } from "./autoSelectFirstTable.svelte";
  import { createDbGenerationReset } from "./dbGenerationReset.svelte";
  import { createBrowseQuery } from "./browseQuery.svelte";
  import {
    colorPresetsForTheme,
    orderColumns,
    stableColumnList,
    visibleColumns,
  } from "./columnView";
  import { computeAutoWidths } from "./columnWidths";
  import { currentModKeyLabel } from "./platformKeys";
  import type { SelectionData } from "./selectionData";
  import { pinGlyphPath } from "./pinGlyph";
  import ContextMenu from "./ContextMenu.svelte";
  import { pinToggleLabel } from "./pinLabel";

  const CHUNK_SIZE = 500;
  const FILTER_DEBOUNCE_MS = 500;

  // The Find-column shortcut accepts Ctrl+F and Cmd+F alike (see
  // onWindowKeydown); only the on-screen hint has to pick one, and naming Ctrl
  // on a Mac names the combination nobody presses there.
  const modKey = currentModKeyLabel();

  let selectedTable = $state<string | null>(null);
  let columns = $state<string[]>([]);
  let totalRows = $state(0);
  let globalFilter = $state("");
  let columnFilters = $state<Record<string, ColumnFilterValue>>({});
  let sortColumn = $state<string | null>(null);
  let sortAsc = $state(true);
  let loading = $state(false);
  let countPending = $state(false);
  let showColumnSettings = $state(false);
  let showFinder = $state(false);
  // Bumping `n` re-triggers the locate effect inside DataGrid even when the
  // user picks the same column twice in a row.
  let locateRequest = $state<{ col: string; n: number } | null>(null);
  let sidebarCollapsed = $state(false);

  // Auto-select the lone table when opening a single-table DB. The helper
  // owns the "did we already auto-select for this open?" bookkeeping.
  // `onReset` is not belt-and-braces: closing a database does not bump
  // dbOpenGeneration, so this is the ONLY thing that clears the view when
  // dbPath goes null.
  const checkAutoSelect = createAutoSelectFirstTable(
    (name) => {
      sidebarCollapsed = true;
      selectTable(name);
    },
    () => resetForNewDatabase(),
  );

  // BrowseData used to keep selectedTable/columns/totalRows/the row
  // cache alive across a Toolbar-driven openDatabase() call (Open DB /
  // recents), so switching to a different database left the grid showing
  // (and querying) the PREVIOUS database's table. Reset every per-database
  // local state whenever a new backend session is published. The close-to-null
  // case is covered separately: checkAutoSelect() fires onReset (->
  // resetForNewDatabase) whenever dbPath becomes null.
  const checkDbReset = createDbGenerationReset({
    getGeneration: () => appState.dbOpenGeneration,
    onReset: () => resetForNewDatabase(),
  });

  // The reset and the single-table auto-select deliberately live in ONE
  // effect (not two separate ones) so their ordering is guaranteed rather
  // than left to Svelte's effect-scheduling order: resetForNewDatabase()
  // always runs BEFORE checkAutoSelect() for the same open, so a single-table
  // DB's auto-selected table is never clobbered by the reset that opening it
  // triggered.
  // Both callbacks WRITE the state they also read back (selectTable() sets
  // selectedTable/columns/filters and then reloadData() reads all of it through
  // makeSnapshot). Left tracked, that is the shape that produced
  // `effect_update_depth_exceeded` in 26.7.5; it converged here only because
  // every one of those helpers carries its own "did I already fire?" latch,
  // which is a property of the helpers rather than of this effect. The two
  // signals this effect must actually re-run on are read explicitly, and the
  // callbacks run untracked, so nothing they touch can re-trigger it.
  $effect(() => {
    void appState.dbOpenGeneration;
    void appState.dbPath;
    void appState.tables;
    untrack(() => {
      checkDbReset();
      checkAutoSelect();
    });
  });

  // Both lists are `$derived` with a structural-equality memo, and both facts
  // are load-bearing.
  //
  // They used to be plain functions called from the template, so they rebuilt
  // on every reactive read -- and `virtualRows.projectVisible` called visCols()
  // once PER ROW, which meant a Ctrl+A copy over 100k rows rebuilt two arrays
  // 100k times.
  //
  // The memo is what keeps the identity stable. `commitTableConfig` replaces a
  // table's config entry wholesale, so a colour change, a filter pin or a width
  // save invalidates anything reading it -- and handing DataGrid a fresh array
  // with identical contents re-runs its width-reseed `$effect` (which resets
  // every column width from `initialColumnWidths`) and re-diffs every header.
  const stableAllCols = stableColumnList();
  const stableVisCols = stableColumnList();

  let allColsOrderedList = $derived.by(() => {
    if (!selectedTable) return stableAllCols(columns);
    return stableAllCols(orderColumns(columns, getTableConfig(selectedTable).column_order));
  });

  let visColsList = $derived.by(() => {
    if (!selectedTable) return stableVisCols(columns);
    return stableVisCols(
      visibleColumns(allColsOrderedList, getTableConfig(selectedTable).hidden_columns),
    );
  });

  function allColsOrdered(): string[] {
    return allColsOrderedList;
  }

  function visCols(): string[] {
    return visColsList;
  }

  // Precomputed column name -> index for O(1) lookups
  let colIndexMap = $derived(new Map(columns.map((c, i) => [c, i])));

  // The query state machine (snapshot/reload/debounce/sort/select plus the row
  // cache) lives in browseQuery.svelte.ts, in the same fully-injected style as
  // createPinnedFilters above: this component keeps every `$state` declaration
  // and hands over a getter/setter for each, so the machine's post-await
  // ownership rules are reachable by a test instead of being locked inside a
  // component nothing can mount.
  const query = createBrowseQuery({
    chunkSize: CHUNK_SIZE,
    filterDebounceMs: FILTER_DEBOUNCE_MS,
    getSelectedTable: () => selectedTable,
    setSelectedTable: (value) => { selectedTable = value; },
    getColumns: () => columns,
    setColumns: (value) => { columns = value; },
    setTotalRows: (value) => { totalRows = value; },
    getGlobalFilter: () => globalFilter,
    setGlobalFilter: (value) => { globalFilter = value; },
    getColumnFilters: () => columnFilters,
    setColumnFilters: (value) => { columnFilters = value; },
    getSortColumn: () => sortColumn,
    setSortColumn: (value) => { sortColumn = value; },
    getSortAsc: () => sortAsc,
    setSortAsc: (value) => { sortAsc = value; },
    setLoading: (value) => { loading = value; },
    setCountPending: (value) => { countPending = value; },
    setError: (message) => { appState.error = message; },
    getVisibleColumns: () => visColsList,
    getColumnIndex: (col) => colIndexMap.get(col),
    getCachedTableColumns: (table) => appState.tableColumns[table],
    getTableConfig,
    ensureTableConfig,
    updateTableConfig,
    queryTable,
    countRows,
    cancelQueries,
    tick,
    applyAutoWidths: () => applyAutoWidths(),
  });

  const virtualRows = query.virtualRows;
  const reloadData = query.reloadData;
  const debouncedReload = query.debouncedReload;
  const handleSort = query.handleSort;
  const selectTable = query.selectTable;

  /**
   * Clears every piece of per-database local state BrowseData caches about
   * whichever database was previously open: the selected table, its
   * columns/row-count, all filter/sort state, the in-flight loading flag, the
   * pending filter debounce, and the virtualRows row cache. Called whenever
   * appState.dbPath changes (see the merged reset+auto-select effect above) so
   * a database switch can never leave the grid showing -- or querying -- the
   * wrong database's table.
   */
  function resetForNewDatabase() {
    selectedTable = null;
    columns = [];
    totalRows = 0;
    // A reload superseded by this database switch returns early from
    // reloadData() -- either at `beginReload()` returning null or at the
    // isCurrent() check in its `finally` -- and never clears `loading`. The new
    // session then starts with the spinner stuck on until some later reload
    // happens to finish.
    loading = false;
    countPending = false;
    columnFilters = {};
    globalFilter = "";
    sortColumn = null;
    sortAsc = true;
    query.resetForNewDatabase();
  }

  function toggleColumnHidden(col: string) {
    if (!selectedTable) return;
    updateTableConfig(selectedTable, (cfg) => {
      const idx = cfg.hidden_columns.indexOf(col);
      if (idx >= 0) cfg.hidden_columns.splice(idx, 1);
      else cfg.hidden_columns.push(col);
    });
  }

  function setColumnColor(col: string, color: string) {
    if (!selectedTable) return;
    updateTableConfig(selectedTable, (cfg) => {
      if (color) cfg.column_colors[col] = color;
      else delete cfg.column_colors[col];
    });
  }

  function setColumnWidth(col: string, width: number) {
    if (!selectedTable) return;
    // Widths are a high-churn field compared to colors/hidden, but we only
    // write on drag-end (DataGrid emits once per resize), so the save cost
    // (folded into updateTableConfig) is bounded. No need to debounce further.
    updateTableConfig(selectedTable, (cfg) => {
      cfg.column_widths[col] = width;
    });
  }

  /** Compute reasonable column widths by measuring content with canvas. */
  function measureAutoWidths(): Record<string, number> {
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d')!;
    return computeAutoWidths({
      columns: visCols(),
      rows: virtualRows.firstChunkRows(),
      getColumnIndex: (col) => colIndexMap.get(col),
      measurer: ctx,
    });
  }

  /** Apply auto-fit widths and persist them. */
  function applyAutoWidths() {
    if (!selectedTable) return;
    const widths = measureAutoWidths();
    updateTableConfig(selectedTable, (cfg) => {
      cfg.column_widths = widths;
    });
  }

  /** Reset saved widths and recompute from content. */
  function resetColumnWidths() {
    applyAutoWidths();
  }

  async function exportSelection(data: SelectionData) {
    const types = data.headers.map((h) =>
      selectedTable ? (appState.tableColumnTypes[selectedTable]?.[h] ?? "") : "",
    );
    const path = await exportToXlsx({
      headers: data.headers,
      rows: data.rows,
      columnTypes: types,
    });
    appState.notice = `Excel export written to ${path}`;
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

  function setFilter(col: string, filter: { value: string; is_regex: boolean }) {
    columnFilters[col] = filter;
    debouncedReload();
  }

  function toggleRegex(col: string) {
    if (!columnFilters[col]) columnFilters[col] = { value: "", is_regex: true };
    else columnFilters[col] = { ...columnFilters[col], is_regex: !columnFilters[col].is_regex };
    if (columnFilters[col]?.value.trim()) debouncedReload();
  }

  // Pinned filter state machine — extracted helper. Fully injected: this
  // component supplies getters/setters for the ephemeral filter state it owns
  // AND the config read/write pair (getConfig/updateConfig), so the helper
  // imports nothing from the store itself. It owns the global-filter pin
  // context menu state.
  const pinned = createPinnedFilters({
    getSelectedTable: () => selectedTable,
    getColumnFilters: () => columnFilters,
    setColumnFilters: (cf) => { columnFilters = cf; },
    getGlobalFilter: () => globalFilter,
    setGlobalFilter: (v) => { globalFilter = v; },
    triggerReload: () => debouncedReload(),
    getConfig: getTableConfig,
    updateConfig: updateTableConfig,
  });

  let showFilterHelp = $state(false);

  function colorPresets(): string[] {
    return colorPresetsForTheme(appState.theme);
  }

  function reorderColumns(fromCol: string, toCol: string) {
    if (!selectedTable) return;
    const cfg = getTableConfig(selectedTable);
    const order = cfg.column_order.length > 0
      ? cfg.column_order.filter((c) => columns.includes(c))
      : [...columns];
    const fromIdx = order.indexOf(fromCol);
    const toIdx = order.indexOf(toCol);
    if (fromIdx < 0 || toIdx < 0) return;
    order.splice(fromIdx, 1);
    order.splice(toIdx, 0, fromCol);
    updateTableConfig(selectedTable, (next) => {
      next.column_order = order;
    });
  }

  function resetColumnOrder() {
    if (!selectedTable) return;
    updateTableConfig(selectedTable, (cfg) => {
      cfg.column_order = [];
    });
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

  // Locate a column in the grid: unhide it first if needed, then bump the
  // locate signal so DataGrid scrolls to and pulses the header.
  function locateColumn(col: string) {
    if (!selectedTable) return;
    if (getTableConfig(selectedTable).hidden_columns.includes(col)) {
      toggleColumnHidden(col);
    }
    locateRequest = { col, n: (locateRequest?.n ?? 0) + 1 };
  }

  // Ctrl+F opens the column finder. Gated to the browse tab so it doesn't
  // intercept in SQL editor / structure tabs. preventDefault stops the webview
  // from showing its own find UI.
  function onWindowKeydown(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== "f") return;
    if (appState.activeTab !== "browse") return;
    if (!selectedTable || columns.length === 0) return;
    e.preventDefault();
    showFinder = true;
  }
</script>

<svelte:window onkeydown={onWindowKeydown} />

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
                <path d={pinGlyphPath}
                  fill={pinned.globalFilterPinState === "none" ? "none" : "currentColor"}
                  stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
              </svg>
            </button>
          </div>
          <button
            class="reset-filters-btn"
            onclick={pinned.handleResetClick}
            title="Reset filters to their saved (pinned) defaults — Shift+click also clears the pinned defaults themselves"
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
                <div class="help-hint">Empty (NULL) cells are excluded by any active filter</div>
              </div>
            {/if}
          </div>
          <button onclick={() => (showColumnSettings = !showColumnSettings)} class="settings-btn">Columns</button>
          <button
            onclick={() => (showFinder = !showFinder)}
            class="settings-btn find-col-btn"
            title="Find column by name ({modKey}+F)"
            aria-label="Find column"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
              <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" stroke-width="1.5"/>
              <line x1="10.5" y1="10.5" x2="14" y2="14" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            <span>Find column</span>
            <kbd class="kbd-hint">{modKey}+F</kbd>
          </button>
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
          columns={visColsList}
          mode={{
            kind: "virtual",
            totalRows,
            getRow: virtualRows.getVisibleRow,
            peekRow: virtualRows.peekVisibleRow,
            getRows: virtualRows.getVisibleRows,
            setVisibleWindow: virtualRows.setVisibleWindow,
            rowsVersion: virtualRows.cacheVersion,
          }}
          columnColors={visColColors}
          sortColumn={sortColumn}
          sortAsc={sortAsc}
          onSort={handleSort}
          filtering={{
            columnFilters,
            onFilterInput: handleFilterInput,
            onToggleRegex: toggleRegex,
            onSetFilter: setFilter,
          }}
          columnOps={{
            onHideColumn: toggleColumnHidden,
            onSetColumnColor: setColumnColor,
            onReorderColumn: reorderColumns,
            colorPresets: colorPresets(),
            initialColumnWidths: selectedTable ? (getTableConfig(selectedTable).column_widths ?? {}) : {},
            onResizeColumn: setColumnWidth,
            onResetColumnWidths: resetColumnWidths,
          }}
          pinning={{
            pinStates: pinned.pinStates,
            onTogglePinFilter: pinned.togglePinColumnFilter,
            onRevertFilter: pinned.revertColumnFilter,
            onClearFilter: pinned.clearColumnFilter,
          }}
          onExport={exportSelection}
          onNotice={(message) => (appState.notice = message)}
          onError={(message) => (appState.error = message)}
          locateRequest={locateRequest}
        />

        <ColumnFinder
          columns={allColsOrderedList}
          hiddenColumns={selectedTable ? getTableConfig(selectedTable).hidden_columns : []}
          open={showFinder}
          onClose={() => (showFinder = false)}
          onLocate={locateColumn}
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
  <ContextMenu x={pinned.globalPinCtx.x} y={pinned.globalPinCtx.y} onClose={pinned.closeGlobalPinCtx}>
    <button class="ctx-item" onclick={() => { pinned.toggleGlobalFilterPin(); pinned.closeGlobalPinCtx(); }}>
      {pinToggleLabel(pinned.globalFilterPinState, "global filter")}
    </button>
    {#if pinned.globalFilterPinState === "modified"}
      <button class="ctx-item" onclick={() => { pinned.revertGlobalFilter(); pinned.closeGlobalPinCtx(); }}>Revert to pinned value</button>
    {/if}
    <div class="ctx-sep"></div>
    <button class="ctx-item" onclick={() => { pinned.clearGlobalFilter(); pinned.closeGlobalPinCtx(); }}>Clear global filter</button>
  </ContextMenu>
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
    position: relative; /* anchor for ColumnFinder popover */
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

  /* base .pin-btn (layout/color/hover-to-accent) promoted to app.css;
     padding/opacity here are this call site's local overrides. */
  .pin-btn {
    padding: 0 6px;
  }
  .pin-btn[data-pin-state="none"] { opacity: 0.45; }

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
  .find-col-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 6px 3px 8px;
  }
  .find-col-btn .kbd-hint {
    font-family: 'Cascadia Code', 'Cascadia Mono', 'Consolas', monospace;
    font-size: 10px;
    line-height: 1;
    padding: 2px 4px;
    border: 1px solid var(--border-color);
    border-radius: 3px;
    color: var(--text-muted);
    background: var(--bg-tertiary);
  }
  .row-info { margin-left: auto; color: var(--text-secondary); font-size: 12px; }
  .loading-indicator { color: var(--warning); font-size: 11px; animation: pulse 1s infinite; }
  @keyframes pulse { 50% { opacity: 0.5; } }

</style>
