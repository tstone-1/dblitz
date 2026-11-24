/** Header drag-to-reorder state machine for DataGrid columns. */

export function createDragReorder(
  getColumns: () => string[],
  getOnReorder: () => ((fromCol: string, toCol: string) => void) | undefined,
) {
  let reorderCol = $state<string | null>(null);
  let reorderOverCol = $state<string | null>(null);
  let didReorder = false;
  let cleanupFn: (() => void) | null = null;

  function onMouseDown(e: MouseEvent, col: string) {
    didReorder = false;
    if (e.button !== 0 || !getOnReorder()) return;
    if ((e.target as HTMLElement).classList.contains('resize-handle')) return;
    const startX = e.clientX;
    const startY = e.clientY;
    let started = false;

    function columnAt(me: MouseEvent): string | null {
      const header = document.elementFromPoint(me.clientX, me.clientY)?.closest<HTMLElement>('.col-header');
      return header?.dataset.colidx ? getColumns()[Number(header.dataset.colidx)] ?? null : null;
    }

    function onMove(me: MouseEvent) {
      const dx = me.clientX - startX;
      const dy = me.clientY - startY;
      if (!started && Math.abs(dx) + Math.abs(dy) < 6) return;
      if (!started) {
        started = true;
        reorderCol = col;
      }
      reorderOverCol = columnAt(me);
    }

    function cleanup() {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      cleanupFn = null;
    }

    function onUp(event: MouseEvent) {
      cleanup();
      reorderOverCol = started ? columnAt(event) : null;
      if (started && reorderCol && reorderOverCol && reorderCol !== reorderOverCol) {
        getOnReorder()?.(reorderCol, reorderOverCol);
      }
      didReorder = started && reorderOverCol !== null;
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
