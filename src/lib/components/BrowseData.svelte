<script lang="ts">
  import { tick } from "svelte";
  import {
    queryTable,
    cancelQueries,
    countRows,
    exportToXlsx,
    type ColumnFilter,
    type ColumnFilterValue,
    type QueryResult,
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
  import { createVirtualRows } from "./virtualRows.svelte";
  import {
    buildActiveFilters,
    colorPresetsForTheme,
    orderColumns,
    visibleColumns,
  } from "./columnView";
  import { computeAutoWidths } from "./columnWidths";
  import { hasIncompleteOperator, stripIncompleteSegments } from "./filterOperators";
  import type { SelectionData } from "./selectionData";
  import { pinGlyphPath } from "./pinGlyph";
  import ContextMenu from "./ContextMenu.svelte";
  import { pinToggleLabel } from "./pinLabel";

  const CHUNK_SIZE = 500;
  const FILTER_DEBOUNCE_MS = 500;

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
  let filterDebounce: ReturnType<typeof setTimeout> | null = null;
  let sidebarCollapsed = $state(false);

  // Auto-select the lone table when opening a single-table DB. The helper
  // owns the "did we already auto-select for this db path?" bookkeeping.
  // `onReset` is belt-and-braces for the close-to-null path: the merged
  // effect below already resets on every dbPath change (including to
  // null), but wiring this too means the reset also fires if this helper's
  // own internal bookkeeping (autoSelectedDb) is ever reused standalone.
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
  // local state whenever a new backend session is published.
  //
  // Gated on appState.dbOpenGeneration, not dbPath: reopening the ALREADY-open
  // file reopens the backend connection (clearing its caches) without changing
  // dbPath, so a dbPath-only gate would keep serving the previous connection's
  // stale rows. The generation bumps on every successful open, same-path
  // included; the close-to-null case is still covered because checkAutoSelect()
  // fires onReset (-> resetForNewDatabase) whenever dbPath becomes null.
  //
  // `prevDbGen` is a plain `let`, not `$state` -- it's only ever read and
  // written from inside this effect, so wrapping it reactively would just be
  // redundant bookkeeping (and risks the effect depending on its own write).
  //
  // The reset and the single-table auto-select deliberately live in ONE
  // effect (not two separate ones) so their ordering is guaranteed rather
  // than left to Svelte's effect-scheduling order: resetForNewDatabase()
  // always runs BEFORE checkAutoSelect() for the same open, so a single-table
  // DB's auto-selected table is never clobbered by the reset that opening it
  // triggered.
  let prevDbGen = 0;
  $effect(() => {
    const gen = appState.dbOpenGeneration;
    if (gen !== prevDbGen) {
      prevDbGen = gen;
      resetForNewDatabase();
    }
    checkAutoSelect();
  });

  function allColsOrdered(): string[] {
    if (!selectedTable) return columns;
    const cfg = getTableConfig(selectedTable);
    return orderColumns(columns, cfg.column_order);
  }

  function visCols(): string[] {
    if (!selectedTable) return columns;
    const cfg = getTableConfig(selectedTable);
    return visibleColumns(allColsOrdered(), cfg.hidden_columns);
  }

  function buildFilters(): ColumnFilter[] {
    // Drop filters for columns that no longer exist in the schema
    // (e.g. a pinned filter on a column that was renamed externally), and
    // strip bare half-typed operator segments (">" with no operand) from
    // non-regex values so a reload triggered by a discrete action (a sort
    // click) queries with the still-valid segments instead of a broken filter.
    const cleaned: Record<string, ColumnFilterValue> = {};
    for (const [col, f] of Object.entries(columnFilters)) {
      cleaned[col] = f.is_regex
        ? f
        : { ...f, value: stripIncompleteSegments(f.value) };
    }
    return buildActiveFilters(columns, cleaned);
  }

  // An immutable snapshot of the query state (table + filters + sort) taken at
  // reload time. virtualRows pins it per-epoch and feeds it back to loadChunk
  // so a background chunk fetch queries the epoch's state, never whatever the
  // component holds by the time the fetch fires (see W2 / the protocol comment
  // in virtualRows.svelte.ts).
  interface QuerySnapshot {
    table: string | null;
    filters: ColumnFilter[];
    globalFilter: string;
    sortColumn: string | null;
    sortAsc: boolean;
  }

  function makeSnapshot(): QuerySnapshot {
    return {
      table: selectedTable,
      filters: buildFilters(),
      globalFilter: globalFilter.trim(),
      sortColumn,
      sortAsc,
    };
  }

  // Precomputed column name -> index for O(1) lookups
  let colIndexMap = $derived(new Map(columns.map((c, i) => [c, i])));

  function loadChunk(
    offset: number,
    limit: number,
    snapshot: QuerySnapshot,
  ): Promise<QueryResult> {
    return queryTable({
      table: snapshot.table,
      offset,
      limit,
      filters: snapshot.filters,
      globalFilter: snapshot.globalFilter,
      sortColumn: snapshot.sortColumn,
      sortAsc: snapshot.sortAsc,
    });
  }

  const virtualRows = createVirtualRows<QuerySnapshot>({
    chunkSize: CHUNK_SIZE,
    getSelectedTable: () => selectedTable,
    makeSnapshot,
    loadChunk,
    cancelQueries: () => cancelQueries(),
    getVisibleColumns: () => visCols(),
    getColumnIndex: (col) => colIndexMap.get(col),
    hasColumns: () => columns.length > 0,
    setColumns: (nextColumns) => { columns = nextColumns; },
    setTotalRows: (nextTotalRows) => { totalRows = nextTotalRows; },
    setError: (message) => { appState.error = message; },
  });

  /**
   * Clears every piece of per-database local state BrowseData caches about
   * whichever database was previously open: the selected table, its
   * columns/row-count, all filter/sort state, the pending filter debounce,
   * and the virtualRows row cache. Called whenever appState.dbPath changes
   * (see the merged reset+auto-select effect above) so a database switch
   * can never leave the grid showing -- or querying -- the wrong database's
   * table.
   */
  function resetForNewDatabase() {
    selectedTable = null;
    columns = [];
    totalRows = 0;
    countPending = false;
    columnFilters = {};
    globalFilter = "";
    sortColumn = null;
    sortAsc = true;
    lastFilterState = "";
    if (filterDebounce) { clearTimeout(filterDebounce); filterDebounce = null; }
    virtualRows.reset();
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
    if (cfg.sort_column && !columns.includes(cfg.sort_column)) {
      updateTableConfig(name, (tableCfg) => {
        tableCfg.sort_column = null;
        tableCfg.sort_asc = true;
      });
    }
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
    const reload = await virtualRows.beginReload();
    if (reload === null) return;
    const { epoch: myEpoch, snapshot } = reload;
    try {
      // Use the epoch's pinned snapshot for the first chunk AND the row count
      // so both agree with the background chunk fetches virtualRows will run.
      const result = await loadChunk(0, CHUNK_SIZE, snapshot);
      if (!virtualRows.applyFirstChunk(myEpoch, result)) return;

      if (result.total_rows !== null) {
        totalRows = result.total_rows;
        countPending = false;
      } else {
        totalRows = result.rows.length < CHUNK_SIZE ? result.rows.length : CHUNK_SIZE;
        countPending = true;
        countRows({
          table: snapshot.table, filters: snapshot.filters, globalFilter: snapshot.globalFilter,
        }).then((count) => {
          if (virtualRows.isCurrent(myEpoch)) {
            totalRows = count;
            countPending = false;
          }
        }).catch((e) => {
          if (virtualRows.isCurrent(myEpoch)) {
            countPending = false;
            appState.error = String(e);
          }
        });
      }

      await tick();
    } catch (e) {
      if (virtualRows.isCurrent(myEpoch)) appState.error = String(e);
    } finally {
      if (virtualRows.isCurrent(myEpoch)) loading = false;
    }
  }

  // Plain `let` on purpose — this is a deduplication memo for debouncedReload,
  // not reactive state. Tracking it via `$state` would defeat the dedup (every
  // read/write would trigger downstream effects).
  let lastFilterState = "";

  function hasIncompleteFilter(): boolean {
    // Segment/regex logic lives in filterOperators.ts (pure + tested).
    return Object.values(columnFilters).some((f) =>
      hasIncompleteOperator(f.value, f.is_regex),
    );
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
      updateTableConfig(selectedTable, (cfg) => {
        cfg.sort_column = sortColumn;
        cfg.sort_asc = sortAsc;
      });
    }
    // Always reload after a sort click. buildFilters() strips any half-typed
    // operator segment, so a bare operator in a filter cell can't leave the
    // grid persistently ordered one way while the header shows the other.
    reloadData();
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
    await exportToXlsx({
      headers: data.headers,
      rows: data.rows,
      columnTypes: types,
    });
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
                <div class="help-hint">Empty (NULL) cells are excluded by any active filter</div>
              </div>
            {/if}
          </div>
          <button onclick={() => (showColumnSettings = !showColumnSettings)} class="settings-btn">Columns</button>
          <button
            onclick={() => (showFinder = !showFinder)}
            class="settings-btn find-col-btn"
            title="Find column by name"
            aria-label="Find column"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
              <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" stroke-width="1.5"/>
              <line x1="10.5" y1="10.5" x2="14" y2="14" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            <span>Find column</span>
            <kbd class="kbd-hint">Ctrl+F</kbd>
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
          columns={visCols()}
          mode={{
            kind: "virtual",
            totalRows,
            getRow: virtualRows.getVisibleRow,
            getRows: virtualRows.getVisibleRows,
          }}
          columnColors={visColColors}
          sortColumn={sortColumn}
          sortAsc={sortAsc}
          onSort={handleSort}
          filtering={{
            columnFilters,
            onFilterInput: handleFilterInput,
            onToggleRegex: toggleRegex,
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
          columns={allColsOrdered()}
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
