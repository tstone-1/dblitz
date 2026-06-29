import type { SelectionBounds } from "./cellSelection.svelte";

export interface SelectionDataOptions {
  selection: SelectionBounds | null;
  columns: string[];
  getRow: (index: number) => (string | null)[] | null;
  getRows?: (start: number, end: number) => Promise<(string | null)[][]>;
  maxRows?: number;
  /** Membership test for a disjoint (Ctrl+Click) selection. When provided,
   *  `selection` is the union bounding box: cells outside the union serialize as
   *  empty strings and rows with no selected cell are dropped. Omit it for a
   *  plain contiguous rectangle (every cell in `selection` is included). */
  isSelected?: (row: number, col: number) => boolean;
}

export interface SelectionData {
  headers: string[];
  rows: string[][];
  truncated: boolean;
}

export async function buildSelectionData({
  selection,
  columns,
  getRow,
  getRows,
  maxRows = 100_000,
  isSelected,
}: SelectionDataOptions): Promise<SelectionData | null> {
  if (!selection) return null;

  const headers = columns.slice(selection.c0, selection.c1 + 1);
  const lastRow = Math.min(selection.r1, selection.r0 + maxRows - 1);
  const truncated = lastRow < selection.r1;
  // For a disjoint selection this fetches the whole union bounding-box span,
  // including gap rows that get skipped below. Bounded by `maxRows`, so a sparse
  // selection across a huge vertical gap fetches at most `maxRows` rows, not the
  // full distance between the topmost and bottommost selected cell.
  const materialized = getRows ? await getRows(selection.r0, lastRow) : null;
  const rows: string[][] = [];

  for (let r = selection.r0; r <= lastRow; r++) {
    // A disjoint selection skips rows that contain no selected cell so the copy
    // doesn't carry blank gap rows from the union bounding box.
    if (isSelected) {
      let rowHasCell = false;
      for (let c = selection.c0; c <= selection.c1; c++) {
        if (isSelected(r, c)) { rowHasCell = true; break; }
      }
      if (!rowHasCell) continue;
    }

    const row = materialized ? materialized[r - selection.r0] : getRow(r);
    if (!row) {
      if (!getRows) {
        throw new Error("Selection contains unavailable rows and no row materializer was provided.");
      }
      throw new Error("Selection contains rows that could not be loaded.");
    }

    const cells: string[] = [];
    for (let c = selection.c0; c <= selection.c1; c++) {
      cells.push(isSelected && !isSelected(r, c) ? "" : (row[c] ?? ""));
    }
    rows.push(cells);
  }

  return { headers, rows, truncated };
}
