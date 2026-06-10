import type { ColumnFilter, ColumnFilterValue, Theme } from "$lib/store.svelte";

export function orderColumns(columns: string[], configuredOrder: string[]): string[] {
  if (configuredOrder.length === 0) return columns;

  const inOrder = new Set(configuredOrder);
  const newColumns = columns.filter((column) => !inOrder.has(column));
  const existingOrderedColumns = configuredOrder.filter((column) => columns.includes(column));
  return [...existingOrderedColumns, ...newColumns];
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
