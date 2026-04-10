<script lang="ts">
  interface Props {
    columns: string[];
    hiddenColumns: string[];
    columnOrder: string[];
    colorPresets: string[];
    getColumnColor: (col: string) => string;
    onToggleHidden: (col: string) => void;
    onSetColor: (col: string, color: string) => void;
    onReorder: (fromCol: string, toCol: string) => void;
    onResetOrder: () => void;
  }

  let {
    columns,
    hiddenColumns,
    columnOrder,
    colorPresets,
    getColumnColor,
    onToggleHidden,
    onSetColor,
    onReorder,
    onResetOrder,
  }: Props = $props();

  let dragCol = $state<string | null>(null);
  let dragOverCol = $state<string | null>(null);

  function orderedColumns(): string[] {
    if (columnOrder.length > 0) {
      const inOrder = new Set(columnOrder);
      const newCols = columns.filter((c) => !inOrder.has(c));
      return [...columnOrder.filter((c) => columns.includes(c)), ...newCols];
    }
    return columns;
  }

  function handleDragStart(col: string, e: DragEvent) {
    dragCol = col;
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function handleDragOver(col: string, e: DragEvent) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dragOverCol = col;
  }

  function handleDrop(targetCol: string) {
    if (!dragCol || dragCol === targetCol) { dragCol = null; dragOverCol = null; return; }
    onReorder(dragCol, targetCol);
    dragCol = null;
    dragOverCol = null;
  }

  function handleDragEnd() {
    dragCol = null;
    dragOverCol = null;
  }

  const hiddenSet = $derived(new Set(hiddenColumns));
</script>

<div class="column-settings">
  <div class="settings-header">
    <div class="settings-title">Column Visibility, Order & Colors</div>
    {#if columnOrder.length > 0}
      <button onclick={onResetOrder} class="reset-order-btn">Reset Order</button>
    {/if}
  </div>
  <div class="settings-grid" role="list">
    {#each orderedColumns() as col (col)}
      <div class="setting-row"
        role="listitem"
        class:drag-over={dragOverCol === col && dragCol !== col}
        draggable="true"
        ondragstart={(e) => handleDragStart(col, e)}
        ondragover={(e) => handleDragOver(col, e)}
        ondrop={() => handleDrop(col)}
        ondragend={handleDragEnd}>
        <span class="drag-handle" title="Drag to reorder">&#x2807;</span>
        <label title={col}>
          <input type="checkbox" checked={!hiddenSet.has(col)} onchange={() => onToggleHidden(col)} />
          <span>{col}</span>
        </label>
        <div class="color-swatches">
          {#each colorPresets as color}
            <button class="swatch" class:active={getColumnColor(col) === color}
              style="background: {color || 'transparent'}; {!color ? 'border: 1px dashed var(--text-muted);' : ''}"
              onclick={() => onSetColor(col, color)} title={color || "No color"}></button>
          {/each}
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .column-settings {
    padding: 8px; border-bottom: 1px solid var(--border-color);
    background: var(--bg-secondary); max-height: 200px; overflow-y: auto; flex-shrink: 0;
  }
  .settings-header { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
  .settings-title { font-size: 11px; font-weight: 600; color: var(--text-muted); text-transform: uppercase; }
  .reset-order-btn { font-size: 10px; padding: 1px 6px; color: var(--text-muted); border-color: var(--border-color); }
  .reset-order-btn:hover { color: var(--text-primary); }
  .settings-grid { display: flex; flex-wrap: wrap; gap: 4px 16px; }
  .setting-row { display: flex; align-items: center; gap: 6px; font-size: 12px; cursor: grab; width: 260px; }
  .setting-row.drag-over { outline: 2px solid var(--accent); outline-offset: -1px; border-radius: 3px; }
  .setting-row label { display: flex; align-items: center; gap: 4px; cursor: pointer; width: 120px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex-shrink: 0; }
  .setting-row label span { overflow: hidden; text-overflow: ellipsis; }
  .drag-handle { color: var(--text-muted); font-size: 14px; cursor: grab; user-select: none; line-height: 1; }
  .color-swatches { display: flex; gap: 2px; }
  .swatch { width: 16px; height: 16px; border-radius: 3px; border: 1px solid var(--border-color); padding: 0; cursor: pointer; }
  .swatch.active { outline: 2px solid var(--accent); outline-offset: 1px; }
</style>
