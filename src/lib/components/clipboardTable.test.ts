import { describe, expect, it, vi } from "vitest";
import {
  serializeClipboardTable,
  writeClipboardTable,
  type ClipboardTable,
} from "./clipboardTable";

/**
 * Parse the tab-delimited plain-text form back into a grid, RFC 4180 style.
 * A round trip is the assertion that matters here: counting quotes would let a
 * serializer that quotes the wrong thing still pass, while re-parsing proves
 * the pasted table has exactly the cells that were copied.
 */
function parseTsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let inQuotes = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') { field += '"'; i++; }
        else inQuotes = false;
      } else field += ch;
      continue;
    }
    if (ch === '"' && field === "") inQuotes = true;
    else if (ch === "\t") { row.push(field); field = ""; }
    else if (ch === "\r" && text[i + 1] === "\n") {
      row.push(field); field = ""; rows.push(row); row = []; i++;
    } else field += ch;
  }
  row.push(field);
  rows.push(row);
  return rows;
}

describe("serializeClipboardTable", () => {
  it("round-trips cells containing tab, LF, CRLF and quotes", () => {
    // The exact regression: a raw join('\t')/join('\n') turned each of these
    // into extra spreadsheet columns or extra rows, silently.
    const rows = [
      ["has\ttab", "plain"],
      ["has\nlf", "has\r\ncrlf"],
      ['has "quotes"', "has\rcr"],
    ];
    const { text } = serializeClipboardTable(["a", "b"], rows);

    expect(parseTsv(text)).toEqual([["a", "b"], ...rows]);
  });

  it("keeps the pasted shape at exactly the selected rows and columns", () => {
    // The property the bug broke, stated on its own: cell count per row and
    // row count must not depend on cell CONTENT.
    const rows = [
      ["a\tb\tc", "x"],
      ["d\ne", "y"],
    ];
    const parsed = parseTsv(serializeClipboardTable(null, rows).text);
    expect(parsed.length).toBe(2);
    expect(parsed.map((r) => r.length)).toEqual([2, 2]);
  });

  it("leaves ordinary cells unquoted", () => {
    // Control: if quoting were unconditional the round-trip tests above would
    // still pass, and every plain copy would grow noise.
    const { text } = serializeClipboardTable(["id", "name"], [["1", "alpha"]]);
    expect(text).toBe("id\tname\r\n1\talpha");
  });

  it("omits the header row when copying without headers", () => {
    const { text, html } = serializeClipboardTable(null, [["1"]]);
    expect(text).toBe("1");
    expect(html).not.toContain("<th>");
  });

  it("escapes HTML and emits in-cell line breaks as <br>", () => {
    // A literal newline inside a <td> is collapsed to a space by HTML parsers,
    // so it has to become a <br> or the copy loses the break.
    const { html } = serializeClipboardTable(["h<1>"], [["a&b", "line1\r\nline2"]]);
    expect(html).toContain("<th>h&lt;1&gt;</th>");
    expect(html).toContain("a&amp;b");
    expect(html).toContain("line1<br>line2");
    // One <br> for a CRLF, not two.
    expect(html.match(/<br>/g)).toHaveLength(1);
  });

  it("keeps a tab inside a cell out of the markup's cell boundaries", () => {
    const { html } = serializeClipboardTable(null, [["a\tb", "c"]]);
    expect(html.match(/<td/g)).toHaveLength(2);
  });
});

describe("writeClipboardTable", () => {
  const table: ClipboardTable = { text: "t", html: "<table></table>" };
  // Stand-in for the DOM constructor; the real one is absent under jsdom.
  const FakeItem = vi.fn(function (this: unknown, data: unknown) {
    (this as { data: unknown }).data = data;
  }) as unknown as typeof globalThis.ClipboardItem;

  it("writes both representations when the rich clipboard API is available", async () => {
    const write = vi.fn(async () => {});
    const writeText = vi.fn(async () => {});
    await writeClipboardTable(table, { write, writeText }, FakeItem);

    expect(write).toHaveBeenCalledOnce();
    expect(writeText).not.toHaveBeenCalled();
  });

  it("falls back to plain text when the rich write fails", async () => {
    // Losing the HTML flavour is acceptable; losing the copy is not.
    const write = vi.fn(async () => {
      throw new Error("NotAllowedError");
    });
    const writeText = vi.fn(async () => {});
    await writeClipboardTable(table, { write, writeText }, FakeItem);

    expect(writeText).toHaveBeenCalledWith("t");
  });

  it("falls back to plain text when ClipboardItem is unavailable", async () => {
    const write = vi.fn(async () => {});
    const writeText = vi.fn(async () => {});
    await writeClipboardTable(table, { write, writeText }, undefined);

    expect(write).not.toHaveBeenCalled();
    expect(writeText).toHaveBeenCalledWith("t");
  });
});
