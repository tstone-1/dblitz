<script lang="ts">
  import type { ColumnPreset } from "$lib/ipc";
  import {
    normalizePresetName,
    orderColumns,
    PRESET_NAME_MAX_LEN,
    sameColumnList,
  } from "./columnView";

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
    /** The columns the grid shows right now, in display order. */
    visibleColumns: string[];
    presets: ColumnPreset[];
    onSetAllHidden: (hidden: boolean) => void;
    onSavePreset: (name: string) => void;
    /** Returns the preset's columns this table lacks, or null when none of
     *  its columns exist here and nothing was applied. */
    onApplyPreset: (name: string) => string[] | null;
    onDeletePreset: (name: string) => void;
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
    visibleColumns,
    presets,
    onSetAllHidden,
    onSavePreset,
    onApplyPreset,
    onDeletePreset,
  }: Props = $props();

  let presetName = $state("");
  // Result of the last preset action, e.g. columns the preset named that this
  // table does not have. Cleared by the next action.
  let presetNotice = $state("");

  const normalizedName = $derived(normalizePresetName(presetName));
  const nameTaken = $derived(presets.some((p) => p.name === normalizedName));
  const canSave = $derived(normalizedName !== "" && visibleColumns.length > 0);

  function savePreset() {
    if (!canSave) return;
    onSavePreset(normalizedName);
    presetNotice = "";
    presetName = "";
  }

  function applyPreset(name: string) {
    const missing = onApplyPreset(name);
    if (missing === null) {
      presetNotice = `None of the columns in "${name}" exist in this table.`;
    } else if (missing.length > 0) {
      presetNotice = `${missing.length} column(s) from "${name}" are not in this table: ${missing.join(", ")}`;
    } else {
      presetNotice = "";
    }
  }

  function deletePreset(name: string) {
    onDeletePreset(name);
    presetNotice = "";
  }

  let dragCol = $state<string | null>(null);
  let dragOverCol = $state<string | null>(null);

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
    <button onclick={() => onSetAllHidden(false)} class="reset-order-btn" disabled={hiddenColumns.length === 0}>Show all</button>
    <button onclick={() => onSetAllHidden(true)} class="reset-order-btn" disabled={visibleColumns.length === 0}>Hide all</button>
    {#if columnOrder.length > 0}
      <button onclick={onResetOrder} class="reset-order-btn">Reset Order</button>
    {/if}
  </div>
  <div class="presets-row">
    <span class="presets-label">Presets</span>
    {#each presets as preset (preset.name)}
      <span class="preset-chip" class:active={sameColumnList(preset.columns, visibleColumns)}>
        <button class="preset-apply" onclick={() => applyPreset(preset.name)} title="Show only: {preset.columns.join(', ')}">{preset.name}</button>
        <button class="preset-delete" onclick={() => deletePreset(preset.name)} title="Delete preset" aria-label="Delete preset {preset.name}">&times;</button>
      </span>
    {/each}
    <input
      class="preset-name"
      type="text"
      placeholder="Preset name"
      maxlength={PRESET_NAME_MAX_LEN}
      bind:value={presetName}
      onkeydown={(e) => { if (e.key === "Enter") { e.preventDefault(); savePreset(); } }}
    />
    <button
      class="reset-order-btn"
      onclick={savePreset}
      disabled={!canSave}
      title={nameTaken ? `Replace "${normalizedName}" with the columns shown now` : "Save the columns shown now"}
    >{nameTaken ? "Overwrite" : "Save current"}</button>
    {#if presetNotice}<span class="preset-notice">{presetNotice}</span>{/if}
  </div>
  <div class="settings-grid" role="list">
    {#each orderColumns(columns, columnOrder) as col (col)}
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
  .reset-order-btn:hover:not(:disabled) { color: var(--text-primary); }
  .reset-order-btn:disabled { opacity: 0.5; cursor: default; }
  .presets-row { display: flex; align-items: center; flex-wrap: wrap; gap: 4px 6px; margin-bottom: 6px; font-size: 12px; }
  .presets-label { font-size: 11px; font-weight: 600; color: var(--text-muted); text-transform: uppercase; }
  .preset-chip { display: inline-flex; align-items: center; border: 1px solid var(--border-color); border-radius: 3px; }
  .preset-chip.active { border-color: var(--accent); }
  .preset-chip button { border: none; background: transparent; font-size: 11px; padding: 1px 6px; }
  .preset-delete { color: var(--text-muted); padding: 1px 4px !important; }
  .preset-delete:hover { color: var(--text-primary); }
  .preset-name { font-size: 11px; padding: 1px 4px; width: 120px; }
  .preset-notice { font-size: 11px; color: var(--text-muted); }
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
