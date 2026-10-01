/**
 * Find-in-grid: the cell order a search walks and the matcher it applies.
 *
 * The search runs over the rows exactly as the grid shows them -- filtered,
 * sorted, and already projected to the visible columns -- so hidden columns
 * are never searched and a hit is always a cell the user can see. Matching is
 * against the cell's string, which is the text the grid renders (the backend
 * already renders REAL and Parquet values to text), so "1200" finds "1200.0".
 */

type Row = (string | null)[];

export interface FindPosition {
  row: number;
  col: number;
}

/** 1 = next match, -1 = previous match. */
export type FindDirection = 1 | -1;

export type FindResult =
  | { kind: "found"; row: number; col: number; wrapped: boolean }
  | { kind: "none" }
  | { kind: "cancelled" };

/**
 * Case-insensitive substring match. NULL never matches: the "NULL" the grid
 * shows in such a cell is a label, not the cell's value. An empty or
 * whitespace-only query matches nothing rather than every cell.
 */
export function makeCellMatcher(query: string): (cell: string | null) => boolean {
  const needle = query.toLowerCase();
  if (needle.trim() === "") return () => false;
  return (cell) => cell !== null && cell.toLowerCase().includes(needle);
}

export interface FindCellOptions {
  rowCount: number;
  colCount: number;
  /** The cell the search moves away from; null starts at the first cell
   *  (forward) or the last cell (backward) and includes it. */
  start: FindPosition | null;
  direction: FindDirection;
  matches: (cell: string | null) => boolean;
  /** Rows `first..last` inclusive. Must reject rather than return a short
   *  array when rows cannot be loaded: a skipped block would be reported as
   *  "no match" for rows that were never searched. */
  loadRows: (first: number, last: number) => Promise<Row[]>;
  /** Rows per `loadRows` call. Blocks are aligned to multiples of this. */
  blockRows: number;
  /** Checked after every load; true stops the search with "cancelled". */
  isCancelled: () => boolean;
  /** Rows searched so far, called once per block. */
  onProgress?: (rowsSearched: number) => void;
}

/**
 * Walks every cell once in reading order (row by row, left to right; reversed
 * for direction -1), starting after `start` and wrapping at the end, so the
 * start cell itself is the last one tried. `wrapped` is true when the match
 * lies on the far side of the wrap.
 */
export async function findCell(opts: FindCellOptions): Promise<FindResult> {
  const { rowCount, colCount, direction, matches, blockRows } = opts;
  const total = rowCount * colCount;
  if (total === 0) return { kind: "none" };

  // Cells are numbered k = row * colCount + col. A null start sits just
  // outside the range, so step 1 lands on the first (or last) cell.
  const startK = opts.start
    ? opts.start.row * colCount + opts.start.col
    : direction === 1 ? -1 : total;

  let block: Row[] = [];
  let blockFirst = -1;
  let blockLast = -2;
  let rowsSearched = 0;

  for (let step = 1; step <= total; step++) {
    const k = (((startK + direction * step) % total) + total) % total;
    const row = Math.floor(k / colCount);
    if (row < blockFirst || row > blockLast) {
      blockFirst = Math.floor(row / blockRows) * blockRows;
      blockLast = Math.min(rowCount - 1, blockFirst + blockRows - 1);
      block = await opts.loadRows(blockFirst, blockLast);
      if (opts.isCancelled()) return { kind: "cancelled" };
      if (block.length !== blockLast - blockFirst + 1) {
        throw new Error("Search stopped: some rows could not be loaded.");
      }
      rowsSearched += blockLast - blockFirst + 1;
      opts.onProgress?.(Math.min(rowsSearched, rowCount));
    }
    const col = k - row * colCount;
    if (matches(block[row - blockFirst][col] ?? null)) {
      const wrapped = direction === 1 ? k <= startK : k >= startK;
      return { kind: "found", row, col, wrapped };
    }
  }
  return { kind: "none" };
}
