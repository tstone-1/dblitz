import type { XlsxExport } from "#lib/ipc.ts";

/**
 * The notice shown after an Excel export. One function for both callers
 * (Browse and the SQL tab), so neither can report the path and forget the cut
 * cells: Excel holds at most 32,767 characters in a cell, the export cuts a
 * longer one instead of failing, and a workbook that silently differs from the
 * grid is the thing this app exists not to produce.
 */
export function exportNotice(result: XlsxExport, includedRows?: number): string {
  const messages = [`Excel export written to ${result.path}`];
  if (includedRows !== undefined) messages.push(`Only the first ${includedRows.toLocaleString()} selected rows were exported`);
  if (result.truncated_cells > 0) {
    const cells = result.truncated_cells === 1 ? "1 cell was" : `${result.truncated_cells} cells were`;
    messages.push(`${cells} cut to Excel's limit of 32,767 characters`);
  }
  return messages.length === 1 ? messages[0] : messages.join(". ") + ".";
}
