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

export function rowIndexToVirtualTop(
  rowIndex: number,
  rowHeight: number,
  geometry: VirtualScrollGeometry,
  viewportHeight: number,
): number {
  return (rowIndex * rowHeight) / scrollRangeRatio(geometry, viewportHeight);
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
