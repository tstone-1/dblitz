/** Header drag-to-reorder state machine for DataGrid columns. */

export function createDragReorder(
  getColumns: () => string[],
  onReorder: ((fromCol: string, toCol: string) => void) | undefined,
) {
  let reorderCol = $state<string | null>(null);
  let reorderOverCol = $state<string | null>(null);
  let didReorder = false;
  let cleanupFn: (() => void) | null = null;

  function onMouseDown(e: MouseEvent, col: string) {
    if (e.button !== 0 || !onReorder) return;
    if ((e.target as HTMLElement).classList.contains('resize-handle')) return;
    const startX = e.clientX;
    const startY = e.clientY;
    let started = false;

    function onMove(me: MouseEvent) {
      const dx = me.clientX - startX;
      const dy = me.clientY - startY;
      if (!started && Math.abs(dx) + Math.abs(dy) < 6) return;
      if (!started) {
        started = true;
        reorderCol = col;
      }
      const el = document.elementFromPoint(me.clientX, me.clientY);
      const headerEl = el?.closest('.col-header') as HTMLElement | null;
      if (headerEl) {
        const idx = Number(headerEl.dataset.colidx);
        const columns = getColumns();
        if (!isNaN(idx) && idx < columns.length) {
          reorderOverCol = columns[idx];
        }
      }
    }

    function cleanup() {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      cleanupFn = null;
    }

    function onUp() {
      cleanup();
      if (started && reorderCol && reorderOverCol && reorderCol !== reorderOverCol) {
        onReorder?.(reorderCol, reorderOverCol);
      }
      if (started) didReorder = true;
      reorderCol = null;
      reorderOverCol = null;
    }

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    cleanupFn = cleanup;
  }

  function consumeReorder(): boolean {
    if (didReorder) {
      didReorder = false;
      return true;
    }
    return false;
  }

  function destroy() {
    cleanupFn?.();
  }

  return {
    get reorderCol() { return reorderCol; },
    get reorderOverCol() { return reorderOverCol; },
    onMouseDown,
    consumeReorder,
    destroy,
  };
}
