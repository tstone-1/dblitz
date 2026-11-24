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

function renderGrid(getRows: () => Promise<string[][]>, countState: "known" | "failed" = "known", viewToken = () => 1) {
  const onNotice = vi.fn();
  const onExport = vi.fn(async () => {});
  grid = mount(DataGrid, { target, props: {
    columns: ["name", "id"], onNotice, onExport,
    mode: {
      kind: "virtual", totalRows: 1, getRow: () => ["alpha", "1"], peekRow: () => ["alpha", "1"],
      getRows, setVisibleWindow: () => {}, rowsVersion: 0, viewToken, countState,
    },
  }});
  flushSync();
  const container = target.querySelector<HTMLElement>(".grid-container")!;
  // jsdom has no layout. The keyboard shortcut is scoped to a visible grid.
  Object.defineProperty(container, "offsetParent", { value: target });
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

it("keeps the last Ctrl+C on the clipboard", async () => {
  const first = Promise.withResolvers<string[][]>();
  const rows = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce([["alpha", "1"]]);
  renderGrid(rows);
  const writeText = vi.fn(async (_text: string) => {});
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  const cells = target.querySelectorAll(".data-cell");
  function copy(index: number) {
    cells[index].dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
    window.dispatchEvent(new MouseEvent("mouseup", { button: 0, bubbles: true }));
    flushSync();
    window.dispatchEvent(new KeyboardEvent("keydown", { ctrlKey: true, key: "c" }));
  }

  copy(0);
  copy(1);
  await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith("1"));
  first.resolve([["alpha", "1"]]);
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(writeText).toHaveBeenCalledTimes(1);
});
