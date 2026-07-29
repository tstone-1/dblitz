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
  /** Exact distinct row/column counts from the selection rectangles. Supplying
   *  both also skips the per-cell geometry scan entirely — see `trackGeometry`
   *  below — so the loop only does the numeric aggregation, which stops at the
   *  first unloaded row. */
  selectedRowCount?: number;
  selectedColumnCount?: number;
  hasMultipleSelectedCells?: boolean;
}

export const DEFAULT_MAX_STATS_ROWS = 100_000;

export function buildSelectionStats({
  selection,
  getRow,
  maxRows = DEFAULT_MAX_STATS_ROWS,
  isSelected,
  selectedRowCount,
  selectedColumnCount,
  hasMultipleSelectedCells,
}: BuildSelectionStatsOptions): SelectionStats | null {
  if (!selection) return null;

  // Distinct selected rows/columns for a disjoint selection; bounding-box
  // dimensions for a plain rectangle.
  const selectedRows = new Set<number>();
  const selectedCols = new Set<number>();
  let selectedCells = 0;

  // Whether the per-cell geometry tally above has to be accumulated at all.
  // It is only ever read when the caller did NOT supply exact counts, and
  // DataGrid always does (cellSelection derives them from the selection
  // rectangles, which costs O(rectangles), not O(cells)). Accumulating it
  // there is pure redundancy, and expensive redundancy: after Ctrl+A on a wide
  // table it walks maxRows x columns cells synchronously inside a `$derived`,
  // on every render pass, long after the numeric scan has given up.
  // With exact counts the loop keeps exactly one job besides the aggregates:
  // "does this row contain a selected cell?", so an unloaded row holding no
  // selected cell can't fake a pending aggregate. That answer breaks at the
  // first hit, and the whole loop can stop once numeric scanning has stopped.
  // Exact row+column counts also settle the multiple-cells question on their
  // own -- a selection built from rectangles covers exactly one cell iff it
  // covers exactly one row and one column -- so `selectedCells` is redundant
  // too. See `cellTotal` below.
  const trackGeometry =
    isSelected != null
    && (selectedRowCount === undefined || selectedColumnCount === undefined);

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
      if (!trackGeometry) break; // membership is the only answer needed
      selectedCols.add(c);
      selectedCells++;
    }
    if (!rowHasCell) continue;
    if (trackGeometry) selectedRows.add(r);
    // Geometry is fully counted above; once numeric scanning has stopped, keep
    // looping only while the row/col totals still have to be tallied here.
    if (numericStopped) {
      if (!trackGeometry) break;
      continue;
    }

    const row = getRow(r);
    if (!row) {
      allNumeric = false;
      numericPending = true;
      numericStopped = true;
      if (!trackGeometry) break;
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

    if (numericStopped && !trackGeometry) break;
  }

  const boxCols = selection.c1 - selection.c0 + 1;
  const nRows = selectedRowCount
    ?? (isSelected ? selectedRows.size : selection.r1 - selection.r0 + 1);
  const nCols = selectedColumnCount ?? (isSelected ? selectedCols.size : boxCols);
  // Only the scanned tally can answer this for a disjoint selection whose
  // geometry was measured here; with exact counts `nRows * nCols > 1` is an
  // equivalent test (see `trackGeometry`), and for a plain rectangle it has
  // always been the definition.
  const cellTotal = trackGeometry ? selectedCells : nRows * nCols;
  if (!(hasMultipleSelectedCells ?? cellTotal > 1)) return null;

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
