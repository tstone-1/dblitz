export interface VisibleRowRangeOptions {
  rowCount: number;
  rowHeight: number;
  scrollTop: number;
  stickyHeight: number;
  viewportHeight: number;
  overscan: number;
}

export interface VirtualScrollGeometryOptions {
  rowCount: number;
  rowHeight: number;
  maxSpacerHeight: number;
}

export interface VirtualScrollGeometry {
  naturalHeight: number;
  spacerHeight: number;
  scale: number;
}

export function virtualScrollGeometry({
  rowCount,
  rowHeight,
  maxSpacerHeight,
}: VirtualScrollGeometryOptions): VirtualScrollGeometry {
  const naturalHeight = Math.max(0, rowCount * rowHeight);
  if (naturalHeight <= maxSpacerHeight) {
    return { naturalHeight, spacerHeight: naturalHeight, scale: 1 };
  }
  return { naturalHeight, spacerHeight: maxSpacerHeight, scale: naturalHeight / maxSpacerHeight };
}

function scrollRangeRatio(geometry: VirtualScrollGeometry, viewportHeight: number): number {
  const virtualRange = Math.max(0, geometry.spacerHeight - viewportHeight);
  const dataRange = Math.max(0, geometry.naturalHeight - viewportHeight);
  if (virtualRange === 0) return 1;
  return dataRange / virtualRange;
}

export function virtualScrollTopToDataScroll(
  scrollTop: number,
  geometry: VirtualScrollGeometry,
  viewportHeight: number,
): number {
  return Math.max(0, scrollTop * scrollRangeRatio(geometry, viewportHeight));
}

// Position a row within the (possibly compressed) scroll spacer.
//
// The spacer height is capped at MAX_SCROLL_SPACER_HEIGHT to stay under the
// browser's maximum element height, so for very large tables it is shorter
// than the natural `rowCount * rowHeight`. We must NOT scale the rendered
// rows' tops down by that ratio — doing so squishes the visible window to a
// fraction of ROW_HEIGHT (e.g. 26px -> ~11px for a 1.8M-row table). Instead we
// render the small visible window at its true row height, anchored so that the
// row aligned with the current data-scroll offset lands at `scrollTop`. When
// the spacer is uncompressed (scale 1) this reduces to `rowIndex * rowHeight`.
export function rowIndexToVirtualTop(
  rowIndex: number,
  rowHeight: number,
  geometry: VirtualScrollGeometry,
  viewportHeight: number,
  scrollTop: number,
): number {
  const dataScroll = virtualScrollTopToDataScroll(scrollTop, geometry, viewportHeight);
  return scrollTop + rowIndex * rowHeight - dataScroll;
}

export function visibleRowIndices({
  rowCount,
  rowHeight,
  scrollTop,
  stickyHeight,
  viewportHeight,
  overscan,
}: VisibleRowRangeOptions): number[] {
  if (rowCount === 0) return [];

  const dataScroll = Math.max(0, scrollTop - stickyHeight);
  const first = Math.max(0, Math.floor(dataScroll / rowHeight) - overscan);
  const last = Math.min(
    rowCount - 1,
    Math.ceil((dataScroll + viewportHeight) / rowHeight) + overscan,
  );

  const indices: number[] = [];
  for (let index = first; index <= last; index++) indices.push(index);
  return indices;
}

export function buildGridTemplate(columns: string[], columnWidths: Record<string, number>): string {
  const tracks = columns.map((column) => {
    const width = columnWidths[column];
    return width ? `${width}px` : "minmax(80px, 1fr)";
  });
  return `60px ${tracks.join(" ")}`;
}

export interface RevealRowOptions {
  row: number;
  rowHeight: number;
  geometry: VirtualScrollGeometry;
  viewportHeight: number;
  /** Height of the sticky header stack that covers the top of the viewport. */
  stickyHeight: number;
  scrollTop: number;
}

/**
 * The `scrollTop` that brings `row` fully into view, or null when it already
 * is. Row `r` sits `r * rowHeight - dataScroll` below the sticky header, where
 * `dataScroll` is what `virtualScrollTopToDataScroll` makes of `scrollTop`.
 *
 * An off-screen row is centred rather than placed at an edge. In a compressed
 * spacer (scale > 1) one pixel of `scrollTop` moves the data by several
 * pixels, and the browser rounds `scrollTop`, so an edge placement can land a
 * row just outside the viewport. Centring leaves half the viewport of slack.
 */
export function scrollTopToRevealRow({
  row,
  rowHeight,
  geometry,
  viewportHeight,
  stickyHeight,
  scrollTop,
}: RevealRowOptions): number | null {
  const visibleHeight = Math.max(rowHeight, viewportHeight - stickyHeight);
  const dataScroll = virtualScrollTopToDataScroll(scrollTop, geometry, viewportHeight);
  const top = row * rowHeight - dataScroll;
  if (top >= 0 && top + rowHeight <= visibleHeight) return null;

  const targetDataScroll = Math.max(0, row * rowHeight - (visibleHeight - rowHeight) / 2);
  const ratio = scrollRangeRatio(geometry, viewportHeight);
  const maxScrollTop = Math.max(0, geometry.spacerHeight + stickyHeight - viewportHeight);
  return Math.min(maxScrollTop, Math.round(targetDataScroll / ratio));
}
