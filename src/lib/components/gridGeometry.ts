export interface VisibleRowRangeOptions {
  rowCount: number;
  rowHeight: number;
  scrollTop: number;
  stickyHeight: number;
  viewportHeight: number;
  overscan: number;
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
