<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onDestroy, tick } from "svelte";
  import { appState } from "$lib/store.svelte";
  import { createCellSelection } from "./cellSelection.svelte";
  import { createDragReorder } from "./dragReorder.svelte";

  const ROW_HEIGHT = 26;
  const HEADER_HEIGHT = 26;
  const FILTER_ROW_HEIGHT = 28;
  const OVERSCAN = 20;

  // Props
  interface Props {
    columns: string[];
    // Static mode: all rows in memory
    rows?: (string | null)[][];
    // Virtual scroll mode: total count + getter
    totalRows?: number;
    getRow?: (index: number) => (string | null)[] | null;
    // Optional: called when user scrolls to trigger chunk loading
    onScroll?: (scrollTop: number, viewportHeight: number) => void;
    // Optional: sorting
    sortColumn?: string | null;
    sortAsc?: boolean;
    onSort?: (col: string) => void;
    // Optional: column colors
    columnColors?: Record<string, string>;
    // Optional: per-column filters
    columnFilters?: Record<string, { value: string; is_regex: boolean }>;
    onFilterInput?: (col: string, value: string) => void;
    onToggleRegex?: (col: string) => void;
    // Optional: column management
    onHideColumn?: (col: string) => void;
    onSetColumnColor?: (col: string, color: string) => void;
    onReorderColumn?: (fromCol: string, toCol: string) => void;
    colorPresets?: string[];
    // Optional: pinned (persistent) filters
    pinStates?: Record<string, "none" | "pinned" | "modified">;
    onTogglePinFilter?: (col: string) => void;
    onRevertFilter?: (col: string) => void;
    onClearFilter?: (col: string) => void;
  }

  let {
    columns,
    rows = undefined,
    totalRows: totalRowsProp = undefined,
    getRow: getRowProp = undefined,
    onScroll = undefined,
    sortColumn = null,
    sortAsc = true,
    onSort = undefined,
    columnColors = {},
    columnFilters = undefined,
    onFilterInput = undefined,
    onToggleRegex = undefined,
    onHideColumn = undefined,
    onSetColumnColor = undefined,
    onReorderColumn = undefined,
    colorPresets = undefined,
    pinStates = undefined,
    onTogglePinFilter = undefined,
    onRevertFilter = undefined,
    onClearFilter = undefined,
  }: Props = $props();

  function pinStateOf(col: string): "none" | "pinned" | "modified" {
    return pinStates?.[col] ?? "none";
  }

  let showFilters = $derived(columnFilters != null);
  // Approximate sticky-header height used only for virtual-scroll row culling
  // (firstVisible/lastVisible). Actual rendered height may drift by a px or two
  // under display scaling — OVERSCAN absorbs the slack. Do NOT reuse this for
  // CSS positioning; the sticky wrapper handles that structurally.
  let stickyHeight = $derived(HEADER_HEIGHT + (showFilters ? FILTER_ROW_HEIGHT : 0));

  // Determine mode
  let isVirtual = $derived(getRowProp != null);
  let rowCount = $derived(isVirtual ? (totalRowsProp ?? 0) : (rows?.length ?? 0));

  function getRowData(index: number): (string | null)[] | null {
    if (isVirtual) return getRowProp!(index);
    return rows?.[index] ?? null;
  }

  // Scroll state
  let scrollTop = $state(0);
  let viewportHeight = $state(600);
  let scrollContainer: HTMLDivElement | undefined = $state();

  function firstVisible(): number {
    const dataScroll = Math.max(0, scrollTop - stickyHeight);
    return Math.max(0, Math.floor(dataScroll / ROW_HEIGHT) - OVERSCAN);
  }

  function lastVisible(): number {
    const dataScroll = Math.max(0, scrollTop - stickyHeight);
    return Math.min(rowCount - 1, Math.ceil((dataScroll + viewportHeight) / ROW_HEIGHT) + OVERSCAN);
  }

  function handleScroll(e: Event) {
    const el = e.target as HTMLDivElement;
    scrollTop = el.scrollTop;
    onScroll?.(scrollTop, viewportHeight);
  }

  function visibleRowIndices(): number[] {
    if (rowCount === 0) return [];
    const first = firstVisible();
    const last = lastVisible();
    const indices: number[] = [];
    for (let i = first; i <= last; i++) indices.push(i);
    return indices;
  }

  // Column widths — plain object, not reactive
  let columnWidths: Record<string, number> = {};
  let gridContainer: HTMLDivElement | undefined = $state();

  function buildGridTpl(): string {
    const cols = columns.map((c) => {
      const w = columnWidths[c];
      return w ? `${w}px` : 'minmax(80px, 1fr)';
    });
    return `60px ${cols.join(' ')}`;
  }

  function syncGridTplToDOM() {
    if (gridContainer) {
      gridContainer.style.setProperty('--grid-tpl', buildGridTpl());
    }
  }

  $effect(() => {
    void columns;
    tick().then(syncGridTplToDOM);
  });

  // Column resize
  let resizeCol: string | null = null;
  let resizeStartX = 0;
  let resizeStartW = 0;

  function onResizeStart(e: MouseEvent, col: string) {
    e.preventDefault();
    e.stopPropagation();
    resizeCol = col;
    resizeStartX = e.clientX;
    const header = (e.target as HTMLElement).parentElement;
    resizeStartW = header ? header.getBoundingClientRect().width : 150;
    document.addEventListener('mousemove', onResizeMove);
    document.addEventListener('mouseup', onResizeEnd);
  }

  function onResizeMove(e: MouseEvent) {
    if (!resizeCol) return;
    columnWidths[resizeCol] = Math.max(50, resizeStartW + (e.clientX - resizeStartX));
    syncGridTplToDOM();
  }

  function onResizeEnd() {
    resizeCol = null;
    document.removeEventListener('mousemove', onResizeMove);
    document.removeEventListener('mouseup', onResizeEnd);
  }

  // Selection (extracted to cellSelection.ts)
  const selection = createCellSelection();
  const sel = $derived(selection.sel);

  // Context menu
  let ctxMenu = $state<{ x: number; y: number } | null>(null);

  function handleContextMenu(e: MouseEvent, rowIdx: number) {
    const pos = selection.handleContextMenu(e, rowIdx);
    if (pos) ctxMenu = pos;
  }

  function closeContextMenu() { ctxMenu = null; }

  function getSelectionData(): { headers: string[]; rows: string[][] } | null {
    const b = sel;
    if (!b) return null;
    const headers = columns.slice(b.c0, b.c1 + 1);
    const selRows: string[][] = [];
    for (let r = b.r0; r <= b.r1; r++) {
      const row = getRowData(r);
      const cells: string[] = [];
      for (let c = b.c0; c <= b.c1; c++) {
        cells.push(row ? (row[c] ?? '') : '');
      }
      selRows.push(cells);
    }
    return { headers, rows: selRows };
  }

  async function copySelection(withHeaders: boolean) {
    const data = getSelectionData();
    if (!data) return;
    const lines: string[] = [];
    if (withHeaders) lines.push(data.headers.join('\t'));
    for (const row of data.rows) lines.push(row.join('\t'));
    await navigator.clipboard.writeText(lines.join('\n'));
    ctxMenu = null;
  }

  async function exportToExcel() {
    const data = getSelectionData();
    if (!data) return;
    try {
      await invoke("export_to_xlsx", { headers: data.headers, rows: data.rows });
    } catch (e) {
      appState.error = String(e);
    }
    ctxMenu = null;
  }

  // Reset column widths when columns change
  $effect(() => {
    void columns;
    columnWidths = {};
  });

  onDestroy(() => {
    document.removeEventListener('mousemove', onResizeMove);
    document.removeEventListener('mouseup', onResizeEnd);
    selection.cleanup();
    reorder.destroy();
  });

  function getColor(col: string): string {
    return columnColors[col] || '';
  }

  // Header context menu (color/hide)
  let headerCtx = $state<{ x: number; y: number; col: string } | null>(null);

  function handleHeaderContextMenu(e: MouseEvent, col: string) {
    e.preventDefault();
    e.stopPropagation();
    headerCtx = { x: e.clientX, y: e.clientY, col };
  }

  function closeHeaderCtx() { headerCtx = null; }

  // Header mouse-based reorder (extracted to dragReorder.ts)
  const reorder = createDragReorder(() => columns, () => onReorderColumn);

  function handleHeaderKeydown(e: KeyboardEvent, col: string) {
    if (!onSort) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onSort(col);
    }
  }

  // Pin button context menu (right-click on a filter pin)
  let pinCtx = $state<{ x: number; y: number; col: string } | null>(null);

  function handlePinContextMenu(e: MouseEvent, col: string) {
    e.preventDefault();
    e.stopPropagation();
    pinCtx = { x: e.clientX, y: e.clientY, col };
  }
  function closePinCtx() { pinCtx = null; }
</script>

<div class="grid-container" bind:this={gridContainer}>
  <div class="scroll-viewport" role="grid" bind:this={scrollContainer} bind:clientHeight={viewportHeight} onscroll={handleScroll}>
    <!-- Sticky header stack: header row + (optional) filter row pinned together
         to avoid 1px subpixel drift between two independently-sticky elements -->
    <div class="sticky-header">
      <div class="grid-row header-row" role="row">
        <div class="grid-cell row-num-header" role="columnheader">#</div>
        {#each columns as col}
          <div class="grid-cell col-header"
            role="columnheader"
            tabindex={onSort ? 0 : -1}
            aria-sort={sortColumn === col ? (sortAsc ? 'ascending' : 'descending') : 'none'}
            class:sortable={onSort != null}
            class:drag-over-header={reorder.reorderOverCol === col && reorder.reorderCol !== col}
            class:dragging={reorder.reorderCol === col}
            data-colidx={columns.indexOf(col)}
            onclick={() => { if (reorder.consumeReorder()) return; onSort?.(col); }}
            onkeydown={(e) => handleHeaderKeydown(e, col)}
            oncontextmenu={(e) => handleHeaderContextMenu(e, col)}
            onmousedown={(e) => onReorderColumn ? reorder.onMouseDown(e, col) : undefined}
            style={getColor(col) ? `background: ${getColor(col)};` : ''}>
            {col}{#if pinStateOf(col) !== "none"}<span class="header-pin-glyph" class:modified={pinStateOf(col) === "modified"} title={pinStateOf(col) === "modified" ? "Pinned filter (modified)" : "Pinned filter"}>
              <svg viewBox="0 0 16 16" width="9" height="9" aria-hidden="true"><path d="M9.5 1.5 L14.5 6.5 L11.5 7.5 L10 12 L7 9 L3 13 L2 14 L3 10 L6 7 L3 4 L7.5 2.5 Z" fill="currentColor" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/></svg>
            </span>{/if}{#if sortColumn === col}<span class="sort-indicator">{sortAsc ? ' \u25B2' : ' \u25BC'}</span>{/if}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div class="resize-handle" onmousedown={(e) => onResizeStart(e, col)} onclick={(e) => e.stopPropagation()}></div>
          </div>
        {/each}
      </div>

      {#if showFilters}
        <div class="grid-row filter-row" role="row" tabindex="-1">
          <div class="grid-cell row-num-header" role="gridcell" tabindex="-1"></div>
          {#each columns as col}
            {@const f = columnFilters?.[col]}
            {@const ps = pinStateOf(col)}
            <div class="grid-cell filter-cell" role="gridcell" tabindex="-1" data-pin-state={ps}>
              <input
                type="text"
                class="col-filter-input"
                placeholder="Filter..."
                value={f?.value ?? ''}
                oninput={(e) => onFilterInput?.(col, (e.target as HTMLInputElement).value)}
              />
              <button
                class="regex-toggle"
                class:active={f?.is_regex ?? false}
                title={f?.is_regex ? 'Regex mode' : 'Text mode'}
                onclick={() => onToggleRegex?.(col)}
              >.*</button>
              {#if onTogglePinFilter}
                <button
                  class="pin-btn filter-pin-btn"
                  data-pin-state={ps}
                  title={
                    ps === "pinned"
                      ? "Filter is saved — click to unpin"
                      : ps === "modified"
                        ? "Saved filter exists — click to update, right-click to revert"
                        : "Save filter as default for this column"
                  }
                  onclick={() => onTogglePinFilter?.(col)}
                  oncontextmenu={(e) => handlePinContextMenu(e, col)}
                  aria-label="Pin column filter"
                >
                  <svg viewBox="0 0 16 16" width="11" height="11" aria-hidden="true">
                    <path d="M9.5 1.5 L14.5 6.5 L11.5 7.5 L10 12 L7 9 L3 13 L2 14 L3 10 L6 7 L3 4 L7.5 2.5 Z"
                      fill={ps === "none" ? "none" : "currentColor"}
                      stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
                  </svg>
                </button>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Data rows -->
    <div class="scroll-spacer" style="height: {rowCount * ROW_HEIGHT}px;">
      {#each visibleRowIndices() as rowIdx (rowIdx)}
        {@const row = getRowData(rowIdx)}
        <div class="grid-row data-row"
          role="row"
          tabindex="-1"
          style="position: absolute; top: {rowIdx * ROW_HEIGHT}px; height: {ROW_HEIGHT}px; width: 100%;"
          oncontextmenu={(e) => handleContextMenu(e, rowIdx)}
          onmousedown={(e) => selection.onCellMouseDown(e, rowIdx)}>
          <div class="grid-cell row-num" role="gridcell" tabindex="-1">{rowIdx + 1}</div>
          {#each columns as col, vi}
            {@const inSel = sel != null && rowIdx >= sel.r0 && rowIdx <= sel.r1 && vi >= sel.c0 && vi <= sel.c1}
            <div class="grid-cell data-cell"
              role="gridcell"
              tabindex="-1"
              data-col={vi}
              class:selected={inSel}
              class:sel-top={inSel && rowIdx === sel?.r0}
              class:sel-bottom={inSel && rowIdx === sel?.r1}
              class:sel-left={inSel && vi === sel?.c0}
              class:sel-right={inSel && vi === sel?.c1}
              style={getColor(col) ? `background: ${getColor(col)};` : ''}
              onmouseenter={() => selection.onCellMouseEnter(rowIdx, vi)}>
              {#if !row}{:else if row[vi] === null}<span class="null-value">NULL</span>{:else}{row[vi]}{/if}
            </div>
          {/each}
        </div>
      {/each}
    </div>
  </div>
</div>

{#if ctxMenu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="ctx-backdrop" onclick={closeContextMenu} oncontextmenu={(e) => { e.preventDefault(); closeContextMenu(); }}></div>
  <div class="ctx-menu" style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;">
    <button class="ctx-item" onclick={() => copySelection(false)}>Copy</button>
    <button class="ctx-item" onclick={() => copySelection(true)}>Copy with headers</button>
    <div class="ctx-sep"></div>
    <button class="ctx-item" onclick={exportToExcel}>Open in Excel</button>
  </div>
{/if}

{#if pinCtx}
  {@const ctxState = pinStateOf(pinCtx.col)}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="ctx-backdrop" onclick={closePinCtx} oncontextmenu={(e) => { e.preventDefault(); closePinCtx(); }}></div>
  <div class="ctx-menu" style="left: {pinCtx.x}px; top: {pinCtx.y}px;">
    <button class="ctx-item" onclick={() => { onTogglePinFilter?.(pinCtx!.col); closePinCtx(); }}>
      {ctxState === "pinned" ? "Unpin filter" : ctxState === "modified" ? "Re-pin filter (save current value)" : "Pin filter (save as default)"}
    </button>
    {#if ctxState === "modified" && onRevertFilter}
      <button class="ctx-item" onclick={() => { onRevertFilter!(pinCtx!.col); closePinCtx(); }}>Revert to pinned value</button>
    {/if}
    {#if onClearFilter}
      <div class="ctx-sep"></div>
      <button class="ctx-item" onclick={() => { onClearFilter!(pinCtx!.col); closePinCtx(); }}>Clear filter</button>
    {/if}
  </div>
{/if}

{#if headerCtx}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="ctx-backdrop" onclick={closeHeaderCtx} oncontextmenu={(e) => { e.preventDefault(); closeHeaderCtx(); }}></div>
  <div class="ctx-menu" style="left: {headerCtx.x}px; top: {headerCtx.y}px;">
    {#if onHideColumn}
      <button class="ctx-item" onclick={() => { onHideColumn!(headerCtx!.col); closeHeaderCtx(); }}>Hide column</button>
    {/if}
    {#if onSetColumnColor && colorPresets}
      <div class="ctx-sep"></div>
      <div class="ctx-color-label">Color</div>
      <div class="ctx-color-row">
        {#each colorPresets as color}
          <button class="ctx-swatch" class:active={getColor(headerCtx.col) === color}
            style="background: {color || 'transparent'}; {!color ? 'border: 1px dashed var(--text-muted);' : ''}"
            onclick={() => { onSetColumnColor!(headerCtx!.col, color); closeHeaderCtx(); }}
            title={color || "No color"}></button>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .grid-container {
    flex: 1; display: flex; flex-direction: column; overflow: hidden;
  }

  .grid-row {
    display: grid;
    align-items: stretch;
    grid-template-columns: var(--grid-tpl);
  }

  .grid-cell {
    padding: 2px 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
    font-family: 'Cascadia Code', 'Cascadia Mono', 'Fira Code', 'Consolas', monospace;
    min-width: 0;
    border-right: 1px solid var(--border-color);
    display: flex;
    align-items: center;
  }

  .sticky-header {
    position: sticky;
    top: 0;
    z-index: 2;
    /* Must be at least as wide as its grid-row children so the sticky region
       spans the full horizontal scroll range. */
    min-width: fit-content;
  }

  .header-row {
    border-bottom: 1px solid var(--border-color);
    min-width: fit-content;
  }

  .col-header {
    background: var(--bg-tertiary);
    font-weight: 600;
    padding: 5px 8px;
    user-select: none;
    position: relative;
  }
  .col-header.sortable { cursor: pointer; }
  .col-header.sortable:hover { color: var(--accent); }

  .resize-handle {
    position: absolute;
    right: 0; top: 0; bottom: 0;
    width: 5px;
    cursor: col-resize;
    z-index: 1;
  }
  .resize-handle:hover {
    background: var(--accent);
    opacity: 0.4;
  }

  .row-num-header {
    background: var(--bg-tertiary);
    font-weight: 600;
    padding: 5px 8px;
    text-align: right;
    color: var(--text-muted);
  }

  .filter-row {
    border-bottom: 1px solid var(--border-color);
    min-width: fit-content;
  }

  .filter-cell {
    background: var(--bg-secondary);
    padding: 2px 4px;
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .col-filter-input {
    flex: 1;
    min-width: 0;
    padding: 1px 4px;
    font-size: 11px;
    border: 1px solid var(--border-color);
    border-radius: 3px;
    background: var(--bg-primary);
    color: var(--text-primary);
    height: 22px;
  }
  .col-filter-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .regex-toggle {
    padding: 1px 4px;
    font-size: 10px;
    font-family: monospace;
    border: 1px solid var(--border-color);
    border-radius: 3px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    height: 22px;
    flex-shrink: 0;
  }
  .regex-toggle.active {
    background: var(--accent);
    color: var(--bg-primary);
    border-color: var(--accent);
  }

  .filter-cell[data-pin-state="pinned"] {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .filter-cell[data-pin-state="modified"] {
    box-shadow: inset 2px 0 0 var(--warning);
  }

  .pin-btn {
    display: flex; align-items: center; justify-content: center;
    border: none; background: transparent;
    padding: 0 3px; cursor: pointer;
    color: var(--text-muted);
    height: 22px;
    flex-shrink: 0;
    transition: opacity 120ms, color 120ms;
  }
  .pin-btn[data-pin-state="none"] { opacity: 0.35; }
  .filter-cell:hover .pin-btn[data-pin-state="none"] { opacity: 0.7; }
  .pin-btn[data-pin-state="none"]:hover { opacity: 1; color: var(--text-primary); }
  .pin-btn[data-pin-state="pinned"] { color: var(--accent); opacity: 1; }
  .pin-btn[data-pin-state="modified"] { color: var(--warning); opacity: 1; }
  .pin-btn:hover { color: var(--accent); }

  .header-pin-glyph {
    display: inline-flex;
    align-items: center;
    margin-left: 4px;
    color: var(--accent);
    vertical-align: middle;
  }
  .header-pin-glyph.modified { color: var(--warning); }

  .sort-indicator { color: var(--accent); font-size: 11px; }

  .scroll-viewport {
    flex: 1; overflow: auto; position: relative;
  }

  .scroll-spacer {
    position: relative; width: 100%; min-width: fit-content;
  }

  .data-row {
    border-bottom: 1px solid color-mix(in srgb, var(--border-color) 40%, transparent);
  }

  .data-row:hover .grid-cell {
    background: var(--bg-hover);
  }

  .row-num {
    color: var(--text-muted); font-size: 11px; text-align: right; padding-right: 12px;
  }

  .data-cell {
    padding: 3px 8px;
    user-select: none;
  }

  .null-value {
    color: var(--text-muted);
    font-style: italic;
    opacity: 0.6;
  }

  .data-cell.selected {
    background: color-mix(in srgb, var(--accent) 20%, transparent) !important;
    border-color: transparent;
    /* Selection edges drawn as inset box-shadows so they don't shrink the
     * content box and shift the cell text by 1px. The four edges compose
     * via CSS custom properties below. */
    box-shadow:
      inset 0 var(--sel-t, 0px) 0 0 var(--accent),
      inset calc(-1 * var(--sel-r, 0px)) 0 0 0 var(--accent),
      inset 0 calc(-1 * var(--sel-b, 0px)) 0 0 var(--accent),
      inset var(--sel-l, 0px) 0 0 0 var(--accent);
  }

  .data-cell.sel-top { --sel-t: 1px; }
  .data-cell.sel-bottom { --sel-b: 1px; }
  .data-cell.sel-left { --sel-l: 1px; }
  .data-cell.sel-right { --sel-r: 1px; }

  /* .ctx-backdrop, .ctx-menu, .ctx-item, .ctx-sep promoted to app.css */

  .ctx-color-label {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    padding: 2px 12px;
  }

  .ctx-color-row {
    display: flex;
    gap: 3px;
    padding: 4px 12px 6px;
  }

  .ctx-swatch {
    width: 18px;
    height: 18px;
    border-radius: 3px;
    border: 1px solid var(--border-color);
    padding: 0;
    cursor: pointer;
  }
  .ctx-swatch.active {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .col-header.drag-over-header {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .col-header.dragging {
    opacity: 0.4;
  }
</style>
