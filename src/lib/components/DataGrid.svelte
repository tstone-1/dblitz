<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { createCellSelection } from "./cellSelection.svelte";
  import { createDragReorder } from "./dragReorder.svelte";
  import { buildSelectionData, type SelectionData } from "./selectionData";
  import { buildSelectionStats } from "./selectionStats";
  import {
    buildGridTemplate,
    rowIndexToVirtualTop,
    virtualScrollGeometry,
    virtualScrollTopToDataScroll,
    visibleRowIndices as getVisibleRowIndices,
  } from "./gridGeometry";
  import { shouldHandleWindowCopy } from "./copyGate";

  const ROW_HEIGHT = 26;
  const HEADER_HEIGHT = 26;
  const FILTER_ROW_HEIGHT = 28;
  const OVERSCAN = 20;
  const MAX_SCROLL_SPACER_HEIGHT = 20_000_000;

  type GridMode =
    | { kind: "static"; rows: (string | null)[][] }
    | {
        kind: "virtual";
        totalRows: number;
        getRow: (index: number) => (string | null)[] | null;
        getRows: (start: number, end: number) => Promise<(string | null)[][]>;
      };

  // Props
  interface Props {
    columns: string[];
    mode: GridMode;
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
    // Optional: persisted column widths (px). Drives initial layout; emits
    // back via `onResizeColumn` once the user finishes a drag.
    initialColumnWidths?: Record<string, number>;
    onResizeColumn?: (col: string, width: number) => void;
    // Optional: reset all column widths to auto-fit
    onResetColumnWidths?: () => void;
    // Optional: export selected cells. Owners provide the app-level backend
    // call so this grid remains a presentational leaf.
    onExport?: (data: SelectionData) => Promise<void>;
    onNotice?: (message: string) => void;
    onError?: (message: string) => void;
    // Optional: locate-column signal. Bumping `n` re-triggers the effect for
    // the same column (e.g. user invokes Find on the same column twice).
    locateRequest?: { col: string; n: number } | null;
  }

  let {
    columns,
    mode,
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
    initialColumnWidths = undefined,
    onResizeColumn = undefined,
    onResetColumnWidths = undefined,
    onExport = undefined,
    onNotice = undefined,
    onError = undefined,
    locateRequest = null,
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

  let rowCount = $derived(mode.kind === "virtual" ? mode.totalRows : mode.rows.length);
  let scrollGeometry = $derived(virtualScrollGeometry({
    rowCount,
    rowHeight: ROW_HEIGHT,
    maxSpacerHeight: MAX_SCROLL_SPACER_HEIGHT,
  }));

  function getRowData(index: number): (string | null)[] | null {
    if (mode.kind === "virtual") return mode.getRow(index);
    return mode.rows[index] ?? null;
  }

  // Scroll state
  let scrollTop = $state(0);
  let viewportHeight = $state(600);
  let scrollContainer: HTMLDivElement | undefined = $state();

  function handleScroll(e: Event) {
    const el = e.target as HTMLDivElement;
    scrollTop = el.scrollTop;
  }

  function visibleRowIndices(): number[] {
    return getVisibleRowIndices({
      rowCount,
      rowHeight: ROW_HEIGHT,
      scrollTop: virtualScrollTopToDataScroll(scrollTop, scrollGeometry, viewportHeight),
      stickyHeight,
      viewportHeight,
      overscan: OVERSCAN,
    });
  }

  // Column widths — plain object, not reactive. The `$effect` below seeds
  // this from `initialColumnWidths` so a reopened table reuses prior sizing.
  let columnWidths: Record<string, number> = {};
  let gridContainer: HTMLDivElement | undefined = $state();

  function syncGridTplToDOM() {
    if (gridContainer) {
      gridContainer.style.setProperty('--grid-tpl', buildGridTemplate(columns, columnWidths));
    }
  }

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
    // Persist the final width once the drag ends (avoids thrashing
    // saveViewConfig on every mousemove).
    if (resizeCol && onResizeColumn) {
      const w = columnWidths[resizeCol];
      if (w) onResizeColumn(resizeCol, Math.round(w));
    }
    resizeCol = null;
    document.removeEventListener('mousemove', onResizeMove);
    document.removeEventListener('mouseup', onResizeEnd);
  }

  // Selection (extracted to cellSelection.ts)
  const selection = createCellSelection();
  const sel = $derived(selection.sel);

  const selStats = $derived(buildSelectionStats({ selection: sel, getRow: getRowData }));

  function fmtNum(n: number): string {
    return Number.isInteger(n) ? n.toLocaleString() : n.toLocaleString(undefined, { maximumFractionDigits: 6 });
  }

  // Context menu
  let ctxMenu = $state<{ x: number; y: number } | null>(null);
  let materializingSelection = $state(false);

  function handleContextMenu(e: MouseEvent, rowIdx: number) {
    const pos = selection.handleContextMenu(e, rowIdx);
    if (pos) ctxMenu = pos;
  }

  function closeContextMenu() { ctxMenu = null; }

  const MAX_COPY_ROWS = 100_000;

  async function copySelection(withHeaders: boolean) {
    materializingSelection = true;
    try {
      const data = await buildSelectionData({
        selection: sel,
        columns,
        getRow: getRowData,
        getRows: mode.kind === "virtual" ? mode.getRows : undefined,
        maxRows: MAX_COPY_ROWS,
      });
      if (!data) return;
      const lines: string[] = [];
      if (withHeaders) lines.push(data.headers.join('\t'));
      for (const row of data.rows) lines.push(row.join('\t'));
      await navigator.clipboard.writeText(lines.join('\n'));
      if (data.truncated) {
        onNotice?.(`Selection copied with the first ${data.rows.length.toLocaleString()} rows only.`);
      }
      ctxMenu = null;
    } catch (e) {
      onError?.(String(e));
    } finally {
      materializingSelection = false;
    }
  }

  async function exportSelection() {
    if (!onExport) return;
    materializingSelection = true;
    try {
      const data = await buildSelectionData({
        selection: sel,
        columns,
        getRow: getRowData,
        getRows: mode.kind === "virtual" ? mode.getRows : undefined,
        maxRows: MAX_COPY_ROWS,
      });
      if (!data) return;
      await onExport(data);
      if (data.truncated) {
        onNotice?.(`Excel export included the first ${data.rows.length.toLocaleString()} selected rows only.`);
      }
      ctxMenu = null;
    } catch (e) {
      onError?.(String(e));
    } finally {
      materializingSelection = false;
    }
  }

  // Re-seed column widths when the column set changes (table switch / new
  // file) or when initialColumnWidths is updated externally (e.g. auto-fit).
  // Drag-end saves cause a harmless resync (same values) — acceptable cost
  // for letting auto-size updates propagate without an extra signalling prop.
  $effect(() => {
    void columns;
    columnWidths = { ...(initialColumnWidths ?? {}) };
    tick().then(syncGridTplToDOM);
  });

  // Locate-column: scroll the requested header into horizontal view and
  // pulse a flash overlay on it. Triggered by parent bumping locateRequest.n;
  // tick() lets a just-unhidden column render before we query the DOM.
  // We track the previously-flashed header so a rapid second locate doesn't
  // leave the first header stuck with the class (the timer for it would have
  // been cleared by the new invocation).
  let flashTimer: ReturnType<typeof setTimeout> | null = null;
  let lastLocatedHeader: HTMLElement | null = null;
  $effect(() => {
    if (!locateRequest) return;
    const target = locateRequest.col;
    void locateRequest.n;
    tick().then(() => {
      if (!gridContainer) return;
      const idx = columns.indexOf(target);
      if (idx < 0) return;
      const header = gridContainer.querySelector<HTMLElement>(
        `.col-header[data-colidx="${idx}"]`,
      );
      if (!header) return;
      if (lastLocatedHeader && lastLocatedHeader !== header) {
        lastLocatedHeader.classList.remove("locate-flash");
      }
      header.scrollIntoView({ inline: "center", block: "nearest", behavior: "auto" });
      header.classList.remove("locate-flash");
      // Force reflow so the animation restarts even if class was just removed
      void header.offsetWidth;
      header.classList.add("locate-flash");
      lastLocatedHeader = header;
      if (flashTimer) clearTimeout(flashTimer);
      flashTimer = setTimeout(() => header.classList.remove("locate-flash"), 1100);
    });
  });

  onDestroy(() => {
    document.removeEventListener('mousemove', onResizeMove);
    document.removeEventListener('mouseup', onResizeEnd);
    selection.cleanup();
    reorder.destroy();
    if (flashTimer) clearTimeout(flashTimer);
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

  function handleGridKeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
      // Let inputs/textareas handle Ctrl+A natively (select all text)
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;
      e.preventDefault();
      if (rowCount > 0 && columns.length > 0) {
        selection.setSelection(
          { row: 0, col: 0 },
          { row: rowCount - 1, col: columns.length - 1 },
        );
      }
    }
  }

  // Ctrl+C copies the active selection. Attached at window level because the
  // viewport isn't focusable — clicking a cell doesn't move focus there, so a
  // listener on the viewport never sees the keystroke. We gate on "selection
  // exists" and "not typing in an input" to avoid clobbering native copy
  // behavior inside filter inputs, SQL editor, etc.
  function handleWindowKeydown(e: KeyboardEvent) {
    if (!(e.ctrlKey || e.metaKey) || e.key !== 'c') return;
    const active = document.activeElement as HTMLElement | null;
    const textSel = window.getSelection();
    if (!shouldHandleWindowCopy({
      hasSelection: selection.sel != null,
      targetTag: (e.target as HTMLElement)?.tagName,
      isContentEditable: active?.isContentEditable ?? false,
      hasTextSelection: textSel != null && textSel.toString().length > 0,
      gridVisible: gridContainer?.offsetParent != null,
    })) return;
    e.preventDefault();
    copySelection(false);
  }

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

<svelte:window onkeydown={handleWindowKeydown} />

<div class="grid-container" bind:this={gridContainer}>
  <div class="scroll-viewport" role="grid" tabindex="0" bind:this={scrollContainer} bind:clientHeight={viewportHeight} onscroll={handleScroll} onkeydown={handleGridKeydown}>
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
            class:has-active-filter={(columnFilters?.[col]?.value ?? '').trim() !== ''}
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
                title={f?.is_regex ? 'Regex mode (e.g. foo|bar matches either)' : 'Text mode — use ; for OR (foo;bar). Toggle for regex (foo|bar).'}
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
    <div class="scroll-spacer" style="height: {scrollGeometry.spacerHeight}px;">
      {#each visibleRowIndices() as rowIdx (rowIdx)}
        {@const row = getRowData(rowIdx)}
        <div class="grid-row data-row"
          role="row"
          tabindex="-1"
          style="position: absolute; top: {rowIndexToVirtualTop(rowIdx, ROW_HEIGHT, scrollGeometry, viewportHeight)}px; height: {ROW_HEIGHT}px; width: 100%;"
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

{#if selStats}
  <div class="sel-status-bar">
    <span>{selStats.rows} row(s), {selStats.cols} column(s)</span>
    {#if selStats.sum !== null}
      <span class="sel-stat">Sum: {fmtNum(selStats.sum)}</span>
      <span class="sel-stat">Avg: {fmtNum(selStats.avg!)}</span>
      <span class="sel-stat">Min: {fmtNum(selStats.min!)}</span>
      <span class="sel-stat">Max: {fmtNum(selStats.max!)}</span>
    {:else if selStats.numericPending}
      <span class="sel-stat">Numeric stats loading</span>
    {/if}
  </div>
{/if}

{#if ctxMenu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="ctx-backdrop" onclick={closeContextMenu} oncontextmenu={(e) => { e.preventDefault(); closeContextMenu(); }}></div>
  <div class="ctx-menu" style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;">
    <button class="ctx-item" disabled={materializingSelection} onclick={() => copySelection(false)}>Copy</button>
    <button class="ctx-item" disabled={materializingSelection} onclick={() => copySelection(true)}>Copy with headers</button>
    <div class="ctx-sep"></div>
    {#if onExport}
      <button class="ctx-item" disabled={materializingSelection} onclick={exportSelection}>Open in Excel</button>
    {/if}
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
    {#if onResetColumnWidths}
      <button class="ctx-item" onclick={() => { onResetColumnWidths!(); closeHeaderCtx(); }}>Auto-fit column widths</button>
    {/if}
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

  .sel-status-bar {
    display: flex;
    gap: 16px;
    padding: 3px 12px;
    font-size: 11px;
    font-family: 'Cascadia Code', 'Cascadia Mono', 'Fira Code', 'Consolas', monospace;
    color: var(--text-secondary);
    background: var(--bg-tertiary);
    border-top: 1px solid var(--border-color);
    flex-shrink: 0;
  }

  .sel-stat {
    color: var(--text-muted);
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
    padding: 5px 8px 5px 4px;
    user-select: none;
    position: relative;
  }
  .col-header.sortable { cursor: pointer; }
  .col-header.sortable:hover { color: var(--accent); }
  .col-header.has-active-filter {
    box-shadow: inset 0 -3px 0 var(--accent);
  }

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

  /* Row separator lives on each cell (not on .data-row) so that selected cells
   * can override it via the box-shadow stack below. Keep `.data-cell.selected`
   * declared AFTER this rule — same specificity (0,2,0), source order wins. */
  .data-row .grid-cell {
    box-shadow: inset 0 -1px 0 0 color-mix(in srgb, var(--border-color) 40%, transparent);
  }

  .data-row:hover .grid-cell {
    background: var(--bg-hover);
  }

  .row-num {
    color: var(--text-muted); font-size: 11px; text-align: right; padding-right: 12px;
  }

  .data-cell {
    padding: 3px 8px 3px 4px;
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
    /* Baseline bottom shadow in selection-fill color keeps the row separator
     * continuous across selected cells adjacent to non-selected cells (no
     * 1px notch at the selection boundary). sel-bottom overrides it below. */
    box-shadow:
      inset 0 -1px 0 0 color-mix(in srgb, var(--accent) 20%, transparent),
      inset 0 var(--sel-t, 0px) 0 0 var(--accent),
      inset calc(-1 * var(--sel-r, 0px)) 0 0 0 var(--accent),
      inset 0 calc(-1 * var(--sel-b, 0px)) 0 0 var(--accent),
      inset var(--sel-l, 0px) 0 0 0 var(--accent);
  }
  .data-cell.selected.sel-bottom {
    box-shadow:
      inset 0 var(--sel-t, 0px) 0 0 var(--accent),
      inset calc(-1 * var(--sel-r, 0px)) 0 0 0 var(--accent),
      inset 0 -1px 0 0 var(--accent),
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

  /* Locate-column flash: a 1s tinted overlay so the user can spot the
     scrolled-to column. The `locate-flash` class is added via JS in the
     locate effect, so it must be `:global` — otherwise Svelte's CSS scoping
     would rename it and the rule would silently fail to apply. */
  .col-header:global(.locate-flash)::after {
    content: '';
    position: absolute;
    inset: 0;
    pointer-events: none;
    background: color-mix(in srgb, var(--accent) 45%, transparent);
    animation: col-locate-flash-fade 1000ms ease-out forwards;
  }
  @keyframes col-locate-flash-fade {
    from { opacity: 1; }
    to { opacity: 0; }
  }
</style>
