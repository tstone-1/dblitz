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
  }: Props = $props();

  let showFilters = $derived(columnFilters != null);
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
  const reorder = createDragReorder(() => columns, onReorderColumn);
</script>

<div class="grid-container" bind:this={gridContainer}>
  <div class="scroll-viewport" bind:this={scrollContainer} bind:clientHeight={viewportHeight} onscroll={handleScroll}>
    <!-- Sticky header -->
    <div class="grid-row header-row">
      <div class="grid-cell row-num-header">#</div>
      {#each columns as col}
        <div class="grid-cell col-header" class:sortable={onSort != null}
          class:drag-over-header={reorder.reorderOverCol === col && reorder.reorderCol !== col}
          class:dragging={reorder.reorderCol === col}
          data-colidx={columns.indexOf(col)}
          onclick={() => { if (reorder.consumeReorder()) return; onSort?.(col); }}
          oncontextmenu={(e) => handleHeaderContextMenu(e, col)}
          onmousedown={(e) => onReorderColumn ? reorder.onMouseDown(e, col) : undefined}
          style={getColor(col) ? `background: ${getColor(col)};` : ''}>
          {col}{#if sortColumn === col}<span class="sort-indicator">{sortAsc ? ' \u25B2' : ' \u25BC'}</span>{/if}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="resize-handle" onmousedown={(e) => onResizeStart(e, col)} onclick={(e) => e.stopPropagation()}></div>
        </div>
      {/each}
    </div>

    {#if showFilters}
      <div class="grid-row filter-row" style="top: {HEADER_HEIGHT}px;">
        <div class="grid-cell row-num-header"></div>
        {#each columns as col}
          {@const f = columnFilters?.[col]}
          <div class="grid-cell filter-cell">
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
          </div>
        {/each}
      </div>
    {/if}

    <!-- Data rows -->
    <div class="scroll-spacer" style="height: {rowCount * ROW_HEIGHT}px;">
      {#each visibleRowIndices() as rowIdx (rowIdx)}
        {@const row = getRowData(rowIdx)}
        <div class="grid-row data-row" style="position: absolute; top: {rowIdx * ROW_HEIGHT}px; height: {ROW_HEIGHT}px; width: 100%;"
          oncontextmenu={(e) => handleContextMenu(e, rowIdx)}
          onmousedown={(e) => selection.onCellMouseDown(e, rowIdx)}>
          <div class="grid-cell row-num">{rowIdx + 1}</div>
          {#each columns as col, vi}
            {@const inSel = sel != null && rowIdx >= sel.r0 && rowIdx <= sel.r1 && vi >= sel.c0 && vi <= sel.c1}
            <div class="grid-cell data-cell" data-col={vi}
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
  <div class="ctx-backdrop" onclick={closeContextMenu} oncontextmenu={(e) => { e.preventDefault(); closeContextMenu(); }}></div>
  <div class="ctx-menu" style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;">
    <button class="ctx-item" onclick={() => copySelection(false)}>Copy</button>
    <button class="ctx-item" onclick={() => copySelection(true)}>Copy with headers</button>
    <div class="ctx-sep"></div>
    <button class="ctx-item" onclick={exportToExcel}>Open in Excel</button>
  </div>
{/if}

{#if headerCtx}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
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

  .header-row {
    position: sticky;
    top: 0;
    z-index: 2;
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
    position: sticky;
    z-index: 2;
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
  }

  .data-cell.sel-top { border-top: 1px solid var(--accent); }
  .data-cell.sel-bottom { border-bottom: 1px solid var(--accent); }
  .data-cell.sel-left { border-left: 1px solid var(--accent); }
  .data-cell.sel-right { border-right: 1px solid var(--accent); }

  .ctx-backdrop {
    position: fixed; inset: 0; z-index: 99;
  }

  .ctx-menu {
    position: fixed;
    z-index: 100;
    background: var(--bg-secondary);
    border: 1px solid var(--border-color);
    border-radius: 6px;
    padding: 4px 0;
    min-width: 180px;
    box-shadow: 0 4px 12px rgba(0,0,0,0.15);
  }

  .ctx-item {
    display: block;
    width: 100%;
    padding: 5px 12px;
    border: none;
    border-radius: 0;
    background: transparent;
    text-align: left;
    font-size: 12px;
    cursor: pointer;
  }
  .ctx-item:hover { background: var(--bg-hover); }

  .ctx-sep {
    height: 1px;
    background: var(--border-color);
    margin: 4px 0;
  }

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
