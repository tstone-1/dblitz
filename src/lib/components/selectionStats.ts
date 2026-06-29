import type { SelectionBounds } from "./cellSelection.svelte";

export interface SelectionStats {
  rows: number;
  cols: number;
  sum: number | null;
  avg: number | null;
  min: number | null;
  max: number | null;
  numericPending: boolean;
}

interface BuildSelectionStatsOptions {
  selection: SelectionBounds | null;
  getRow: (index: number) => (string | null)[] | null;
  maxRows?: number;
  /** Membership test for a disjoint (Ctrl+Click) selection. When provided,
   *  `selection` is the union bounding box and only cells inside the union are
   *  counted; row/column totals report the distinct selected rows/columns. */
  isSelected?: (row: number, col: number) => boolean;
}

const DEFAULT_MAX_STATS_ROWS = 100_000;

export function buildSelectionStats({
  selection,
  getRow,
  maxRows = DEFAULT_MAX_STATS_ROWS,
  isSelected,
}: BuildSelectionStatsOptions): SelectionStats | null {
  if (!selection) return null;

  // Distinct selected rows/columns for a disjoint selection; bounding-box
  // dimensions for a plain rectangle.
  const selectedRows = new Set<number>();
  const selectedCols = new Set<number>();
  let selectedCells = 0;

  const capRow = Math.min(selection.r1, selection.r0 + maxRows - 1);
  let allNumeric = true;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let count = 0;
  let numericPending = false;
  // Stop accumulating numeric aggregates once a non-numeric or unloaded cell is
  // hit, but keep scanning so the selected row/column geometry stays complete
  // (membership is pure geometry and needs no row data).
  let numericStopped = false;

  for (let r = selection.r0; r <= capRow; r++) {
    let rowHasCell = false;
    for (let c = selection.c0; c <= selection.c1; c++) {
      if (isSelected && !isSelected(r, c)) continue;
      rowHasCell = true;
      if (isSelected) {
        selectedCols.add(c);
        selectedCells++;
      }
    }
    if (!rowHasCell) continue;
    if (isSelected) selectedRows.add(r);
    // Geometry is fully counted above; once numeric scanning has stopped, keep
    // looping (for a disjoint selection's row/col totals) but skip the cell math.
    if (numericStopped) {
      if (!isSelected) break;
      continue;
    }

    const row = getRow(r);
    if (!row) {
      allNumeric = false;
      numericPending = true;
      numericStopped = true;
      if (!isSelected) break;
      continue;
    }

    for (let c = selection.c0; c <= selection.c1; c++) {
      if (isSelected && !isSelected(r, c)) continue;
      const value = row[c];
      if (value === null || value === "") continue;

      const numberValue = Number(value);
      if (Number.isNaN(numberValue)) {
        allNumeric = false;
        numericStopped = true;
        break;
      }

      sum += numberValue;
      if (numberValue < min) min = numberValue;
      if (numberValue > max) max = numberValue;
      count++;
    }

    if (numericStopped && !isSelected) break;
  }

  const nRows = isSelected ? selectedRows.size : selection.r1 - selection.r0 + 1;
  const nCols = isSelected ? selectedCols.size : selection.c1 - selection.c0 + 1;
  const cellTotal = isSelected ? selectedCells : nRows * nCols;
  if (cellTotal <= 1) return null;

  return {
    rows: nRows,
    cols: nCols,
    sum: allNumeric && count > 0 ? sum : null,
    avg: allNumeric && count > 0 ? sum / count : null,
    min: allNumeric && count > 0 ? min : null,
    max: allNumeric && count > 0 ? max : null,
    numericPending,
  };
}
