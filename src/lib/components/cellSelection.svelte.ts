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

export function createCellSelection() {
  let selAnchor = $state<CellAddr | null>(null);
  let selEnd = $state<CellAddr | null>(null);
  let selecting = false;

  const sel = $derived.by(() => {
    if (!selAnchor || !selEnd) return null;
    return {
      r0: Math.min(selAnchor.row, selEnd.row),
      r1: Math.max(selAnchor.row, selEnd.row),
      c0: Math.min(selAnchor.col, selEnd.col),
      c1: Math.max(selAnchor.col, selEnd.col),
    };
  });

  function colIdxFromEvent(e: MouseEvent): number {
    const cell = (e.target as HTMLElement).closest('[data-col]') as HTMLElement | null;
    return cell ? Number(cell.dataset.col) : -1;
  }

  function onCellMouseDown(e: MouseEvent, rowIdx: number) {
    if (e.button !== 0) return;
    const colIdx = colIdxFromEvent(e);
    if (colIdx < 0) return;
    selecting = true;
    if (e.shiftKey && selAnchor) {
      selEnd = { row: rowIdx, col: colIdx };
    } else {
      selAnchor = { row: rowIdx, col: colIdx };
      selEnd = { row: rowIdx, col: colIdx };
    }
    document.addEventListener('mouseup', onSelectionEnd);
  }

  function onCellMouseEnter(rowIdx: number, colIdx: number) {
    if (!selecting) return;
    selEnd = { row: rowIdx, col: colIdx };
  }

  function onSelectionEnd() {
    selecting = false;
    document.removeEventListener('mouseup', onSelectionEnd);
  }

  function handleContextMenu(e: MouseEvent, rowIdx: number) {
    e.preventDefault();
    const colIdx = colIdxFromEvent(e);
    if (colIdx < 0) return;
    if (!sel || rowIdx < sel.r0 || rowIdx > sel.r1 || colIdx < sel.c0 || colIdx > sel.c1) {
      selAnchor = { row: rowIdx, col: colIdx };
      selEnd = { row: rowIdx, col: colIdx };
    }
    return { x: e.clientX, y: e.clientY };
  }

  function setSelection(anchor: CellAddr, end: CellAddr) {
    selAnchor = anchor;
    selEnd = end;
  }

  function cleanup() {
    document.removeEventListener('mouseup', onSelectionEnd);
  }

  return {
    get sel() { return sel; },
    onCellMouseDown,
    onCellMouseEnter,
    handleContextMenu,
    setSelection,
    cleanup,
  };
}
