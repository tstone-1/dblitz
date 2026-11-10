import type { ColumnFilter, ColumnFilterValue, ColumnPreset } from "#lib/ipc.ts";
import type { Theme } from "#lib/store.svelte.ts";

export function orderColumns(columns: string[], configuredOrder: string[]): string[] {
  if (configuredOrder.length === 0) return columns;

  const inOrder = new Set(configuredOrder);
  const newColumns = columns.filter((column) => !inOrder.has(column));
  const existingOrderedColumns = configuredOrder.filter((column) => columns.includes(column));
  return [...existingOrderedColumns, ...newColumns];
}

/**
 * The column order after dragging `fromCol` onto `toCol`, or null when either
 * is not a live column.
 *
 * It starts from the order the grid shows (`orderColumns`), not from the saved
 * `column_order` filtered to live columns: a column the table gained after the
 * order was saved is shown at the end but is absent from the saved list, and
 * its index of -1 made the drag silently do nothing, both from it and onto it.
 */
export function reorderColumn(
  columns: string[],
  configuredOrder: string[],
  fromCol: string,
  toCol: string,
): string[] | null {
  const order = [...orderColumns(columns, configuredOrder)];
  const fromIdx = order.indexOf(fromCol);
  const toIdx = order.indexOf(toCol);
  if (fromIdx < 0 || toIdx < 0) return null;
  order.splice(fromIdx, 1);
  order.splice(toIdx, 0, fromCol);
  return order;
}

export function visibleColumns(columns: string[], hiddenColumns: string[]): string[] {
  const hiddenSet = new Set(hiddenColumns);
  return columns.filter((column) => !hiddenSet.has(column));
}

export function buildActiveFilters(
  columns: string[],
  columnFilters: Record<string, ColumnFilterValue>,
): ColumnFilter[] {
  const validColumns = new Set(columns);
  return Object.entries(columnFilters)
    .filter(([column, filter]) => validColumns.has(column) && filter.value.trim() !== "")
    .map(([column, filter]) => ({
      column,
      value: filter.value,
      is_regex: filter.is_regex,
    }));
}

export function colorPresetsForTheme(theme: Theme): string[] {
  if (theme === "dark") {
    return [
      "",
      "#3b1c1c",
      "#1c3b1c",
      "#1c1c3b",
      "#3b3b1c",
      "#3b1c3b",
      "#1c3b3b",
      "#2d1f1f",
      "#1f2d1f",
    ];
  }

  return [
    "",
    "#fde8e8",
    "#e8fde8",
    "#e8e8fd",
    "#fdfde8",
    "#fde8fd",
    "#e8fdfd",
    "#f5eded",
    "#edf5ed",
  ];
}

/**
 * Element-wise equality for two column lists.
 *
 * Used to keep a recomputed list's IDENTITY stable when its contents did not
 * change. `commitTableConfig` replaces a table's config entry wholesale, so any
 * colour change, filter pin or width save invalidates every derived that reads
 * it -- and a fresh `visibleColumns(...)` array with the identical contents is
 * enough to re-run DataGrid's width-reseed effect and re-diff every header.
 */
export function sameColumnList(a: readonly string[], b: readonly string[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * Wraps a column-list computation so it returns the PREVIOUS array whenever the
 * newly computed one is element-wise equal. Call it from a `$derived.by`.
 */
export function stableColumnList(): (next: string[]) => string[] {
  let previous: string[] = [];
  return (next: string[]) => {
    if (sameColumnList(next, previous)) return previous;
    previous = next;
    return next;
  };
}

/** Longest preset name kept; config.rs truncates to the same `LABEL_MAX_LEN`. */
export const PRESET_NAME_MAX_LEN = 64;

/** What applying a preset writes into the table config, plus the preset's
 *  columns this table does not have. */
export interface PresetApplication {
  hidden_columns: string[];
  column_order: string[];
  missing: string[];
}

/**
 * Applying a preset shows exactly its columns, in its order, and hides every
 * other column. The hidden ones keep their current relative order behind the
 * shown ones, so "Show all" afterwards does not scramble them.
 *
 * Returns null when none of the preset's columns exist in this table: applying
 * it would leave a grid with no columns, which is never what a preset meant.
 */
export function applyColumnPreset(
  columns: string[],
  columnOrder: string[],
  preset: ColumnPreset,
): PresetApplication | null {
  const present = new Set(columns);
  const shown = [...new Set(preset.columns)].filter((column) => present.has(column));
  if (shown.length === 0) return null;
  const shownSet = new Set(shown);
  const rest = orderColumns(columns, columnOrder).filter((column) => !shownSet.has(column));
  return {
    hidden_columns: rest,
    column_order: [...shown, ...rest],
    missing: preset.columns.filter((column) => !present.has(column)),
  };
}

/** Saves under `name`, replacing a preset of the same name in place so its
 *  position in the list does not move. */
export function upsertColumnPreset(
  presets: ColumnPreset[],
  name: string,
  columns: string[],
): ColumnPreset[] {
  const next = { name, columns: [...columns] };
  const index = presets.findIndex((preset) => preset.name === name);
  if (index < 0) return [...presets, next];
  return presets.map((preset, i) => (i === index ? next : preset));
}

export function removeColumnPreset(presets: ColumnPreset[], name: string): ColumnPreset[] {
  return presets.filter((preset) => preset.name !== name);
}

/** Trimmed and capped the same way config.rs sanitizes it, so the name the
 *  UI shows is the name that survives a reload. */
export function normalizePresetName(name: string): string {
  return [...name.trim()].slice(0, PRESET_NAME_MAX_LEN).join("");
}
