// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import DataGrid from "./DataGrid.svelte";

let grid: ReturnType<typeof mount>;
let target: HTMLElement;

beforeEach(() => {
  target = document.createElement("div");
  document.body.append(target);
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
});

afterEach(async () => {
  if (grid) await unmount(grid);
  target.remove();
  vi.unstubAllGlobals();
});

function renderGrid(getRows: () => Promise<string[][]>, countState: "known" | "failed" = "known", viewToken = () => 1, values = ["alpha", "1"]) {
  const onNotice = vi.fn();
  const onExport = vi.fn(async () => {});
  grid = mount(DataGrid, { target, props: {
    columns: ["name", "id"], onNotice, onExport,
    mode: {
      kind: "virtual", totalRows: 1, getRow: () => values, peekRow: () => values,
      getRows, setVisibleWindow: () => {}, rowsVersion: 0, viewToken, countState,
    },
  }});
  flushSync();
  const container = target.querySelector<HTMLElement>(".grid-container")!;
  // jsdom has no layout. The keyboard shortcut is scoped to a visible grid.
  Object.defineProperty(container, "offsetParent", { configurable: true, value: target });
  return { onNotice, onExport };
}

function openFind() {
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", ctrlKey: true }));
  flushSync();
  return target.querySelector<HTMLInputElement>(".find-input")!;
}

function type(input: HTMLInputElement, text: string) {
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

it("discards a pending Find match when the user edits the search text", async () => {
  const pending = Promise.withResolvers<string[][]>();
  const getRows = vi.fn(() => pending.promise);
  renderGrid(getRows);
  const input = openFind();
  type(input, "alpha");
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  expect(getRows).toHaveBeenCalledOnce();
  type(input, "beta");
  pending.resolve([["alpha"]]);
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(target.querySelector(".selected")).toBeNull();
  expect(target.querySelector(".find-status")?.textContent).toBe("");
  type(input, "alpha");
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await vi.waitFor(() => expect(target.querySelector(".selected")?.textContent?.trim()).toBe("alpha"));
});

it("prevents Find and Select All from treating a failed count as complete", async () => {
  const getRows = vi.fn(async () => [["alpha"]]);
  const { onNotice } = renderGrid(getRows, "failed");
  const input = openFind();
  type(input, "absent");
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  await vi.waitFor(() => expect(target.querySelector(".find-status")?.textContent).toContain("Search incomplete"));
  expect(getRows).not.toHaveBeenCalled();
  target.querySelector('[role="grid"]')!.dispatchEvent(new KeyboardEvent("keydown", {
    key: "a", ctrlKey: true, bubbles: true,
  }));
  expect(onNotice).toHaveBeenCalledWith("Retry the row count to select all rows.");
  expect(target.querySelector(".selected")).toBeNull();
});

it.each(["Copy with headers", "Open in Excel"])("%s keeps the selection from the click", async action => {
  const pending = Promise.withResolvers<string[][]>();
  const getRows = vi.fn(() => pending.promise);
  const { onExport } = renderGrid(getRows);
  const writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  const cells = target.querySelectorAll<HTMLElement>(".data-cell");
  cells[0].dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  flushSync();
  const button = [...target.querySelectorAll<HTMLButtonElement>(".ctx-item")]
    .find(item => item.textContent === action)!;
  button.click();
  expect(getRows).toHaveBeenCalledOnce();
  cells[1].dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
  flushSync();
  expect(target.querySelector(".selected")?.textContent?.trim()).toBe("1");

  pending.resolve([["alpha", "1"]]);
  if (action === "Copy with headers") {
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("name\r\nalpha"));
  } else {
    await vi.waitFor(() => expect(onExport).toHaveBeenCalledWith({
      headers: ["name"], columnIndices: [0], rows: [["alpha"]], truncated: false,
    }));
  }
});

it("drops an export if the table changes while selected rows load", async () => {
  let epoch = 0;
  const pending = Promise.withResolvers<string[][]>();
  const getRows = vi.fn(() => pending.promise);
  const { onExport } = renderGrid(getRows, "known", () => epoch);
  target.querySelector(".data-cell")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  flushSync();
  const button = [...target.querySelectorAll<HTMLButtonElement>(".ctx-item")]
    .find(item => item.textContent === "Open in Excel")!;
  button.click();
  expect(getRows).toHaveBeenCalledOnce();
  flushSync();
  expect(button.disabled).toBe(true);
  epoch++;
  pending.resolve([["alpha", "1"]]);
  await vi.waitFor(() => expect(button.disabled).toBe(false));
  expect(onExport).not.toHaveBeenCalled();
});

it.each(["0.0000001", "0"])("renders all four statistics for %s", async value => {
  renderGrid(async () => [], "known", () => 1, [value, value]);
  target.querySelector('[role="grid"]')!.dispatchEvent(new KeyboardEvent("keydown", {
    key: "a", ctrlKey: true, bubbles: true,
  }));
  const scientific = new Intl.NumberFormat(undefined, { notation: "scientific" });
  const cell = Number(value) ? scientific.format(1e-7) : "0";
  const sum = Number(value) ? scientific.format(2e-7) : "0";
  await vi.waitFor(() => expect([...target.querySelectorAll(".sel-stat")].map(e => e.textContent))
    .toEqual([`Sum: ${sum}`, `Avg: ${cell}`, `Min: ${cell}`, `Max: ${cell}`]));
});

function copyCell(cell: Element) {
  cell.dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
  window.dispatchEvent(new MouseEvent("mouseup", { button: 0, bubbles: true }));
  flushSync();
  window.dispatchEvent(new KeyboardEvent("keydown", { ctrlKey: true, key: "c" }));
}

it("preserves a SQL result copied while Browse rows are loading", async () => {
  const first = Promise.withResolvers<string[][]>();
  const getRows = vi.fn(() => first.promise);
  renderGrid(getRows);
  const writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  copyCell(target.querySelector(".data-cell")!);
  expect(getRows).toHaveBeenCalledOnce();
  Object.defineProperty(target.querySelector(".grid-container"), "offsetParent", { value: null });
  const panel = document.body.appendChild(document.createElement("div"));
  const sql = mount(DataGrid, { target: panel, props: {
    columns: ["result"], mode: { kind: "static", rows: [["new"]] },
  } });
  try {
    flushSync();
    Object.defineProperty(panel.querySelector(".grid-container"), "offsetParent", { value: panel });
    copyCell(panel.querySelector(".data-cell")!);
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("new"));
    first.resolve([["older", "1"]]);
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(writeText).toHaveBeenCalledTimes(1);
  } finally {
    await unmount(sql);
    panel.remove();
  }
});

it("keeps the last Ctrl+C on the clipboard", async () => {
  const first = Promise.withResolvers<string[][]>();
  const rows = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce([["alpha", "1"]]);
  renderGrid(rows);
  const writeText = vi.fn(async (_text: string) => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  const cells = target.querySelectorAll(".data-cell");
  copyCell(cells[0]);
  copyCell(cells[1]);
  await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("1"));
  first.resolve([["alpha", "1"]]);
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(writeText).toHaveBeenCalledTimes(1);
});
