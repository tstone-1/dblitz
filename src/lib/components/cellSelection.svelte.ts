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

export function createCellSelection() {
  // Committed rectangles making up the (possibly disjoint) selection.
  let rects = $state<Rect[]>([]);
  // Index of the rectangle a drag/shift currently extends; -1 when none.
  let activeIndex = -1;
  let selecting = false;

  const bounds = $derived(rects.map(boundsOf));

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
    isSelected,
    onCellMouseDown,
    onCellMouseEnter,
    handleContextMenu,
    setSelection,
    cleanup,
  };
}
