/**
 * Where to put a context menu opened at the pointer so it stays inside the
 * window. Like a native menu: it opens right of and below the pointer, flips
 * to the left of / above the pointer when that side has no room, and is pushed
 * back inside by `margin` when neither side fits (a menu taller than a small
 * window). Opening at the raw pointer position cut the header menu of the last
 * column off at the right window border.
 */
export function placeContextMenu(
  pointer: { x: number; y: number },
  menu: { width: number; height: number },
  viewport: { width: number; height: number },
  margin = 4,
): { x: number; y: number } {
  return {
    x: placeAxis(pointer.x, menu.width, viewport.width, margin),
    y: placeAxis(pointer.y, menu.height, viewport.height, margin),
  };
}

function placeAxis(at: number, size: number, limit: number, margin: number): number {
  if (at + size <= limit - margin) return at;
  if (at - size >= margin) return at - size;
  return Math.max(margin, limit - margin - size);
}
