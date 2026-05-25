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
}

const DEFAULT_MAX_STATS_ROWS = 100_000;

export function buildSelectionStats({
  selection,
  getRow,
  maxRows = DEFAULT_MAX_STATS_ROWS,
}: BuildSelectionStatsOptions): SelectionStats | null {
  if (!selection) return null;

  const nRows = selection.r1 - selection.r0 + 1;
  const nCols = selection.c1 - selection.c0 + 1;
  if (nRows === 1 && nCols === 1) return null;

  const capRow = Math.min(selection.r1, selection.r0 + maxRows - 1);
  let allNumeric = true;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let count = 0;
  let numericPending = false;

  for (let r = selection.r0; r <= capRow; r++) {
    const row = getRow(r);
    if (!row) {
      allNumeric = false;
      numericPending = true;
      break;
    }

    for (let c = selection.c0; c <= selection.c1; c++) {
      const value = row[c];
      if (value === null || value === "") continue;

      const numberValue = Number(value);
      if (Number.isNaN(numberValue)) {
        allNumeric = false;
        break;
      }

      sum += numberValue;
      if (numberValue < min) min = numberValue;
      if (numberValue > max) max = numberValue;
      count++;
    }

    if (!allNumeric) break;
  }

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
