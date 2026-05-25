import type { SelectionBounds } from "./cellSelection.svelte";

export interface SelectionDataOptions {
  selection: SelectionBounds | null;
  columns: string[];
  getRow: (index: number) => (string | null)[] | null;
  getRows?: (start: number, end: number) => Promise<(string | null)[][]>;
  maxRows?: number;
}

export interface SelectionData {
  headers: string[];
  rows: string[][];
}

export async function buildSelectionData({
  selection,
  columns,
  getRow,
  getRows,
  maxRows = 100_000,
}: SelectionDataOptions): Promise<SelectionData | null> {
  if (!selection) return null;

  const headers = columns.slice(selection.c0, selection.c1 + 1);
  const lastRow = Math.min(selection.r1, selection.r0 + maxRows - 1);
  const materialized = getRows ? await getRows(selection.r0, lastRow) : null;
  const rows: string[][] = [];

  for (let r = selection.r0; r <= lastRow; r++) {
    const row = materialized ? materialized[r - selection.r0] : getRow(r);
    if (!row) {
      if (!getRows) {
        throw new Error("Selection contains unavailable rows and no row materializer was provided.");
      }
      throw new Error("Selection contains rows that could not be loaded.");
    }

    const cells: string[] = [];
    for (let c = selection.c0; c <= selection.c1; c++) {
      cells.push(row[c] ?? "");
    }
    rows.push(cells);
  }

  return { headers, rows };
}
