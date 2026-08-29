/**
 * Clipboard serialization for a grid selection.
 *
 * The regression this exists for: the copy path used to be
 * `rows.map(r => r.join("\t")).join("\n")`. SQLite text can contain tabs, LF or
 * CRLF, so any such cell silently became extra spreadsheet columns or extra
 * rows on paste, while `navigator.clipboard.writeText` reported success. A
 * read-only viewer must not hand out a wrong copy of the data it is showing.
 *
 * Two representations are produced and both are put on the clipboard:
 *
 * * `html` - an HTML table. Spreadsheets (Excel, LibreOffice Calc, Google
 *   Sheets) prefer `text/html` when it is present and take cell boundaries from
 *   the markup, so a delimiter inside a cell cannot split it. In-cell line
 *   breaks are emitted as `<br>`, because a literal newline in HTML is
 *   collapsed to a space.
 * * `text` - RFC 4180-style quoting with tab as the delimiter, for every target
 *   that takes plain text (editors, terminals, chat). A field containing a tab,
 *   CR, LF or a double quote is wrapped in double quotes with inner quotes
 *   doubled.
 *
 * Neither format is byte-exact everywhere, and that limit is worth stating
 * rather than hiding: a tab inside a cell survives exactly in the plain-text
 * form, while an HTML consumer may collapse it to a space (the `white-space`
 * style below asks it not to; Excel ignores CSS on paste). What both forms do
 * guarantee is the property the bug broke - the pasted table has exactly the
 * rows and columns that were selected.
 */

export interface ClipboardTable {
  text: string;
  html: string;
}

/** Whether a field needs quoting in the tab-delimited plain-text form. */
function needsQuoting(field: string): boolean {
  return (
    field.includes("\t") ||
    field.includes("\n") ||
    field.includes("\r") ||
    field.includes('"')
  );
}

function quoteField(field: string): string {
  return needsQuoting(field) ? `"${field.replace(/"/g, '""')}"` : field;
}

function escapeHtml(field: string): string {
  return field
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/** Escaped cell content with CRLF/CR/LF all rendered as a single `<br>`. */
function htmlCell(field: string): string {
  return escapeHtml(field).replace(/\r\n|\r|\n/g, "<br>");
}

/**
 * Serialize a selection. `headers` is null when the user copied without them.
 *
 * Rows are separated by CRLF in the plain-text form: that is what spreadsheets
 * emit themselves, and a lone LF is what a quoted multi-line field is most
 * likely to contain, so keeping the two distinct helps a strict parser.
 */
export function serializeClipboardTable(
  headers: string[] | null,
  rows: string[][],
): ClipboardTable {
  const textLines: string[] = [];
  const htmlRows: string[] = [];

  if (headers) {
    textLines.push(headers.map(quoteField).join("\t"));
    htmlRows.push(
      `<tr>${headers.map((h) => `<th>${htmlCell(h)}</th>`).join("")}</tr>`,
    );
  }
  for (const row of rows) {
    textLines.push(row.map(quoteField).join("\t"));
    htmlRows.push(
      `<tr>${row.map((c) => `<td style="white-space:pre-wrap">${htmlCell(c)}</td>`).join("")}</tr>`,
    );
  }

  return {
    text: textLines.join("\r\n"),
    html: `<table>${htmlRows.join("")}</table>`,
  };
}

/**
 * Minimal shape of the clipboard API this module uses, so a test can supply a
 * double without standing up a whole `navigator`.
 */
export interface ClipboardWriter {
  write?: (items: ClipboardItem[]) => Promise<void>;
  writeText: (text: string) => Promise<void>;
}

/**
 * Put both representations on the clipboard.
 *
 * `navigator.clipboard.write` with a multi-type `ClipboardItem` is what lets a
 * spreadsheet pick the HTML while a text editor picks the plain text. It is
 * available in WebView2 and WKWebView, but it is also the part most likely to
 * be missing or refused (an older runtime, a permissions failure), and losing
 * the copy entirely would be a worse outcome than losing the rich form - so a
 * failure falls back to `writeText` with the quoted plain text, which still
 * carries correct cell boundaries.
 */
export async function writeClipboardTable(
  table: ClipboardTable,
  clipboard: ClipboardWriter,
  ClipboardItemCtor: (typeof globalThis)["ClipboardItem"] | undefined = globalThis.ClipboardItem,
): Promise<void> {
  if (clipboard.write && ClipboardItemCtor && typeof Blob !== "undefined") {
    try {
      const item = new ClipboardItemCtor({
        "text/html": new Blob([table.html], { type: "text/html" }),
        "text/plain": new Blob([table.text], { type: "text/plain" }),
      });
      await clipboard.write([item]);
      return;
    } catch {
      // Fall through to the plain-text path below.
    }
  }
  await clipboard.writeText(table.text);
}
