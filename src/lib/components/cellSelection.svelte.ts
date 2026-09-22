/** Cell selection state machine for DataGrid. */

export interface CellAddr {
  row: number;
  col: number;
}

export interface SelectionBounds {
  r0: number;
  r1: number;
  c0: number;
  c1: number;
}

/** A selection rectangle stored as its defining anchor + end so a drag can keep
 *  extending it; bounds (min/max) are derived. The selection is a *list* of
 *  these so Ctrl+Click can build a disjoint, non-contiguous selection. A list of
 *  rectangles (rather than a Set of cells) keeps Ctrl+A over a multi-million-row
 *  table cheap — one rectangle, not one entry per cell. */
interface Rect {
  anchor: CellAddr;
  end: CellAddr;
}

function boundsOf(r: Rect): SelectionBounds {
  return {
    r0: Math.min(r.anchor.row, r.end.row),
    r1: Math.max(r.anchor.row, r.end.row),
    c0: Math.min(r.anchor.col, r.end.col),
    c1: Math.max(r.anchor.col, r.end.col),
  };
}

function contains(b: SelectionBounds, row: number, col: number): boolean {
  return row >= b.r0 && row <= b.r1 && col >= b.c0 && col <= b.c1;
}

function isSingleCellAt(b: SelectionBounds, row: number, col: number): boolean {
  return b.r0 === row && b.r1 === row && b.c0 === col && b.c1 === col;
}

function coveredIndexCount(intervals: Array<[number, number]>): number {
  if (intervals.length === 0) return 0;
  const sorted = intervals.toSorted((a, b) => a[0] - b[0] || a[1] - b[1]);
  let total = 0;
  let [start, end] = sorted[0];
  for (const [nextStart, nextEnd] of sorted.slice(1)) {
    if (nextStart <= end + 1) {
      end = Math.max(end, nextEnd);
    } else {
      total += end - start + 1;
      [start, end] = [nextStart, nextEnd];
    }
  }
  return total + end - start + 1;
}

function coversMultipleCells(bounds: SelectionBounds[]): boolean {
  let firstCell: CellAddr | null = null;
  for (const b of bounds) {
    if (b.r0 !== b.r1 || b.c0 !== b.c1) return true;
    if (!firstCell) {
      firstCell = { row: b.r0, col: b.c0 };
    } else if (firstCell.row !== b.r0 || firstCell.col !== b.c0) {
      return true;
    }
  }
  return false;
}

/** Per-cell render answer: is it selected, and which of its borders are the
 *  selection's own edge. */
export interface CellFlags {
  selected: boolean;
  top: boolean;
  bottom: boolean;
  left: boolean;
  right: boolean;
}

/** Shared instance for the overwhelmingly common "not selected" answer, so a
 *  full render pass allocates nothing for the cells it does not highlight. */
const NOT_SELECTED: CellFlags = Object.freeze({
  selected: false,
  top: false,
  bottom: false,
  left: false,
  right: false,
});

export function createCellSelection() {
  // Committed rectangles making up the (possibly disjoint) selection.
  let rects = $state<Rect[]>([]);
  // Index of the rectangle a drag/shift currently extends; -1 when none.
  let activeIndex = -1;
  let selecting = false;

  const bounds = $derived(rects.map(boundsOf));
  const selectedRowCount = $derived(
    coveredIndexCount(bounds.map((b) => [b.r0, b.r1])),
  );
  const selectedColumnCount = $derived(
    coveredIndexCount(bounds.map((b) => [b.c0, b.c1])),
  );
  const hasMultipleSelectedCells = $derived(coversMultipleCells(bounds));

  // Union bounding box of every rectangle — used by the copy/export/stats
  // helpers and by "is there a selection at all?" checks. Null when empty.
  const sel = $derived.by<SelectionBounds | null>(() => {
    if (bounds.length === 0) return null;
    let r0 = Infinity, r1 = -Infinity, c0 = Infinity, c1 = -Infinity;
    for (const b of bounds) {
      if (b.r0 < r0) r0 = b.r0;
      if (b.r1 > r1) r1 = b.r1;
      if (b.c0 < c0) c0 = b.c0;
      if (b.c1 > c1) c1 = b.c1;
    }
    return { r0, r1, c0, c1 };
  });

  /** True when (row, col) falls inside any selection rectangle. Reactive: reads
   *  the derived bounds, so callers in a component template re-run on change. */
  function isSelected(row: number, col: number): boolean {
    for (const b of bounds) if (contains(b, row, col)) return true;
    return false;
  }

  /** True when any selection rectangle spans this row / this column. DataGrid
   *  uses them for the crosshair: the tinted row and column bands, header and
   *  row number that stay visible once the selected cell is scrolled away. */
  function rowSelected(row: number): boolean {
    for (const b of bounds) if (row >= b.r0 && row <= b.r1) return true;
    return false;
  }

  function colSelected(col: number): boolean {
    for (const b of bounds) if (col >= b.c0 && col <= b.c1) return true;
    return false;
  }

  /**
   * Membership AND the four selection-border edges for one cell, in one call.
   *
   * DataGrid used to ask `isSelected` five times per cell per render (the cell
   * itself plus its four neighbours) to decide which borders to draw. For the
   * ordinary single-rectangle selection -- a drag, a Shift+click, Ctrl+A -- the
   * edges are just the rectangle's own bounds, so one `contains` answers all
   * five. The multi-rectangle path keeps the neighbour probes, because a cell
   * on the edge of one rectangle that touches another must NOT draw a border
   * there.
   */
  function cellFlags(row: number, col: number): CellFlags {
    if (bounds.length === 1) {
      const b = bounds[0];
      if (!contains(b, row, col)) return NOT_SELECTED;
      return {
        selected: true,
        top: row === b.r0,
        bottom: row === b.r1,
        left: col === b.c0,
        right: col === b.c1,
      };
    }
    if (!isSelected(row, col)) return NOT_SELECTED;
    return {
      selected: true,
      top: !isSelected(row - 1, col),
      bottom: !isSelected(row + 1, col),
      left: !isSelected(row, col - 1),
      right: !isSelected(row, col + 1),
    };
  }

  function colIdxFromEvent(e: MouseEvent): number {
    const cell = (e.target as HTMLElement).closest('[data-col]') as HTMLElement | null;
    return cell ? Number(cell.dataset.col) : -1;
  }

  function onCellMouseDown(e: MouseEvent, rowIdx: number) {
    if (e.button !== 0) return;
    const colIdx = colIdxFromEvent(e);
    if (colIdx < 0) return;
    // Cells are `user-select: none`, so clicking one does NOT collapse a
    // pre-existing document text selection (e.g. a stray Ctrl+A on the toolbar
    // path span selects the tab bar). Left intact, that selection makes the
    // Ctrl+C gate defer to native copy and copy the toolbar text instead of the
    // cell. Clicking a cell is an explicit "I want the grid selection" signal,
    // so drop any leftover text selection now.
    if (typeof window !== "undefined") window.getSelection()?.removeAllRanges();

    const here: CellAddr = { row: rowIdx, col: colIdx };
    const additive = e.ctrlKey || e.metaKey;

    if (additive) {
      // Ctrl+Click on a cell that is already its own single-cell rectangle
      // toggles it back off; otherwise begin a new disjoint rectangle that a
      // drag can extend.
      // Limitation: this only deselects standalone 1x1 rectangles. A cell that
      // is selected because it falls inside a *larger* rectangle cannot be
      // peeled out — Ctrl+Click there just stacks a redundant 1x1 on top (the
      // cell stays selected). Deselecting from within a block would require
      // splitting the covering rectangle, which we intentionally don't do.
      const existing = rects.findIndex((r) => isSingleCellAt(boundsOf(r), rowIdx, colIdx));
      if (existing >= 0) {
        rects = rects.filter((_, i) => i !== existing);
        activeIndex = -1;
        return;
      }
      rects = [...rects, { anchor: here, end: here }];
      activeIndex = rects.length - 1;
    } else if (e.shiftKey && activeIndex >= 0 && rects[activeIndex]) {
      // Extend the active rectangle from its existing anchor.
      const next = rects.slice();
      next[activeIndex] = { anchor: next[activeIndex].anchor, end: here };
      rects = next;
    } else {
      rects = [{ anchor: here, end: here }];
      activeIndex = 0;
    }

    selecting = true;
    document.addEventListener('mouseup', onSelectionEnd);
  }

  function onCellMouseEnter(rowIdx: number, colIdx: number) {
    if (!selecting || activeIndex < 0 || !rects[activeIndex]) return;
    const next = rects.slice();
    next[activeIndex] = { anchor: next[activeIndex].anchor, end: { row: rowIdx, col: colIdx } };
    rects = next;
  }

  function onSelectionEnd() {
    selecting = false;
    document.removeEventListener('mouseup', onSelectionEnd);
  }

  function handleContextMenu(e: MouseEvent, rowIdx: number) {
    e.preventDefault();
    const colIdx = colIdxFromEvent(e);
    if (colIdx < 0) return;
    // Right-clicking outside the current selection collapses it to that cell so
    // the menu's copy/export acts on what was clicked.
    if (!isSelected(rowIdx, colIdx)) {
      rects = [{ anchor: { row: rowIdx, col: colIdx }, end: { row: rowIdx, col: colIdx } }];
      activeIndex = 0;
    }
    return { x: e.clientX, y: e.clientY };
  }

  function setSelection(anchor: CellAddr, end: CellAddr) {
    rects = [{ anchor, end }];
    activeIndex = 0;
  }

  function cleanup() {
    document.removeEventListener('mouseup', onSelectionEnd);
  }

  return {
    get sel() { return sel; },
    get selectedRowCount() { return selectedRowCount; },
    get selectedColumnCount() { return selectedColumnCount; },
    get hasMultipleSelectedCells() { return hasMultipleSelectedCells; },
    isSelected,
    rowSelected,
    colSelected,
    cellFlags,
    onCellMouseDown,
    onCellMouseEnter,
    handleContextMenu,
    setSelection,
    cleanup,
  };
}
