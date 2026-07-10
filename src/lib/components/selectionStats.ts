import type { SelectionBounds } from "./cellSelection.svelte";

export interface SelectionStats {
  rows: number;
  cols: number;
  sum: number | null;
  avg: number | null;
  min: number | null;
  max: number | null;
  numericPending: boolean;
  /** True when the selection extends past `maxRows` and the aggregate scan
   *  (and, for a genuinely sparse/disjoint selection, the row/col count too)
   *  only covers the first `maxRows` rows. See the cap-handling comment in
   *  `buildSelectionStats` below. */
  capped: boolean;
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

export const DEFAULT_MAX_STATS_ROWS = 100_000;

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
  const capped = capRow < selection.r1;
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

  // DataGrid always passes `isSelected` (even for a plain click-drag
  // rectangle or Ctrl+A - see cellSelection.svelte.ts's `setSelection`,
  // which builds a single dense rect and exposes it through the same
  // `isSelected` membership function as a genuine Ctrl+Click disjoint
  // selection). When capped, we can't scan past `capRow` to know the true
  // membership of the tail rows - but if every cell scanned so far is
  // selected, the scanned window is a full sub-rectangle, which is exactly
  // what a plain rectangle / Ctrl+A produces (a real disjoint selection
  // built from Ctrl+Click only ever stacks a handful of cells and shows
  // gaps immediately, long before `maxRows` rows in). Treat a dense scanned
  // window as proof the same rectangle continues for the whole bounding box
  // and report the exact geometry; otherwise fall back to what was actually
  // proven selected within the scan.
  const scannedRowCount = capRow - selection.r0 + 1;
  const boxCols = selection.c1 - selection.c0 + 1;
  const scannedIsDense = !isSelected || selectedCells === scannedRowCount * boxCols;
  const reportExactGeometry = !isSelected || (capped && scannedIsDense);

  const nRows = reportExactGeometry ? selection.r1 - selection.r0 + 1 : selectedRows.size;
  const nCols = reportExactGeometry ? boxCols : selectedCols.size;
  const cellTotal = isSelected ? selectedCells : nRows * nCols;
  if (cellTotal <= 1) return null;

  // A capped scan only ever aggregates the first `maxRows` rows, so the
  // numeric aggregates are never trustworthy as a total once capped -
  // suppress them (null) rather than silently reporting a partial sum/avg/
  // min/max as if it covered the whole selection.
  const aggregatesAvailable = !capped && allNumeric && count > 0;

  return {
    rows: nRows,
    cols: nCols,
    sum: aggregatesAvailable ? sum : null,
    avg: aggregatesAvailable ? sum / count : null,
    min: aggregatesAvailable ? min : null,
    max: aggregatesAvailable ? max : null,
    numericPending,
    capped,
  };
}
