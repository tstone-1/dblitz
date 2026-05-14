import { describe, expect, it } from "vitest";
import { createCellSelection } from "./cellSelection.svelte";

function cellEvent(col: number, options: MouseEventInit = {}) {
  const cell = document.createElement("div");
  cell.dataset.col = String(col);
  return new MouseEvent("mousedown", {
    bubbles: true,
    button: 0,
    ...options,
  }) as MouseEvent & { target: EventTarget };
}

function withTarget(event: MouseEvent, target: HTMLElement): MouseEvent {
  Object.defineProperty(event, "target", { value: target });
  return event;
}

describe("cellSelection", () => {
  it("creates a normalized drag selection", () => {
    const selection = createCellSelection();
    const startCell = document.createElement("div");
    startCell.dataset.col = "3";

    selection.onCellMouseDown(withTarget(cellEvent(3), startCell), 10);
    selection.onCellMouseEnter(8, 1);

    expect(selection.sel).toEqual({ r0: 8, r1: 10, c0: 1, c1: 3 });
    selection.cleanup();
  });

  it("extends from the anchor on shift-click", () => {
    const selection = createCellSelection();
    const firstCell = document.createElement("div");
    firstCell.dataset.col = "1";
    const secondCell = document.createElement("div");
    secondCell.dataset.col = "4";

    selection.onCellMouseDown(withTarget(cellEvent(1), firstCell), 2);
    selection.onCellMouseDown(withTarget(cellEvent(4, { shiftKey: true }), secondCell), 6);

    expect(selection.sel).toEqual({ r0: 2, r1: 6, c0: 1, c1: 4 });
    selection.cleanup();
  });

  it("right-click outside the selection moves the context selection", () => {
    const selection = createCellSelection();
    selection.setSelection({ row: 1, col: 1 }, { row: 3, col: 3 });
    const target = document.createElement("div");
    target.dataset.col = "5";
    const event = withTarget(new MouseEvent("contextmenu", { clientX: 7, clientY: 9 }), target);

    const pos = selection.handleContextMenu(event, 10);

    expect(pos).toEqual({ x: 7, y: 9 });
    expect(selection.sel).toEqual({ r0: 10, r1: 10, c0: 5, c1: 5 });
    selection.cleanup();
  });
});
