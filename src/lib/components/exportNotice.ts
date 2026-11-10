import type { XlsxExport } from "#lib/ipc.ts";

/**
 * The notice shown after an Excel export. One function for both callers
 * (Browse and the SQL tab), so neither can report the path and forget the cut
 * cells: Excel holds at most 32,767 characters in a cell, the export cuts a
 * longer one instead of failing, and a workbook that silently differs from the
 * grid is the thing this app exists not to produce.
 */
export function exportNotice(result: XlsxExport): string {
  const written = `Excel export written to ${result.path}`;
  if (result.truncated_cells <= 0) return written;
  const cells = result.truncated_cells === 1 ? "1 cell was" : `${result.truncated_cells} cells were`;
  return `${written}. ${cells} cut to Excel's limit of 32,767 characters.`;
}
