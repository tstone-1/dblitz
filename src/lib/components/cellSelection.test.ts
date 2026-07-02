import { beforeEach, describe, expect, it } from "vitest";
import { createCellSelection } from "./cellSelection.svelte";

// Runs under the "node" test environment (no real DOM). `onCellMouseDown`
// unconditionally registers a `document` mouseup listener, so stub a minimal
// `document` before each test — mirrors the `window` stub pattern in
// store.svelte.test.ts.
beforeEach(() => {
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      addEventListener: () => {},
      removeEventListener: () => {},
    },
  });
});

/** Builds a DOM-lite MouseEvent stub good enough for cellSelection's own
 *  reads: `button`, `ctrlKey`/`metaKey`/`shiftKey`, `clientX`/`clientY`,
 *  `preventDefault`, and a `target.closest('[data-col]')` that resolves to
 *  the cell being interacted with. */
function makeCellEvent(
  colIdx: number,
  opts: Partial<{
    ctrlKey: boolean;
    metaKey: boolean;
    shiftKey: boolean;
    button: number;
    clientX: number;
    clientY: number;
  }> = {},
): MouseEvent {
  const target = { closest: () => ({ dataset: { col: String(colIdx) } }) };
  return {
    button: opts.button ?? 0,
    ctrlKey: opts.ctrlKey ?? false,
    metaKey: opts.metaKey ?? false,
    shiftKey: opts.shiftKey ?? false,
    clientX: opts.clientX ?? 0,
    clientY: opts.clientY ?? 0,
    target,
    preventDefault: () => {},
  } as unknown as MouseEvent;
}

describe("createCellSelection", () => {
  it("Ctrl+Click toggles a standalone single-cell rect off", () => {
    const selection = createCellSelection();

    selection.onCellMouseDown(makeCellEvent(1, { ctrlKey: true }), 2);
    expect(selection.isSelected(2, 1)).toBe(true);

    // Ctrl+Click the same standalone cell again toggles it back off.
    selection.onCellMouseDown(makeCellEvent(1, { ctrlKey: true }), 2);
    expect(selection.isSelected(2, 1)).toBe(false);
    expect(selection.sel).toBeNull();
  });

  it("Ctrl+Click a cell inside a larger rect does NOT deselect it (documented limitation)", () => {
    const selection = createCellSelection();

    // Drag out a 3x3 block from (0,0) to (2,2).
    selection.onCellMouseDown(makeCellEvent(0), 0);
    selection.onCellMouseEnter(2, 2);
    expect(selection.isSelected(1, 1)).toBe(true);

    // Ctrl+Click a cell inside that block: cannot be peeled out, so it stays
    // selected (a redundant 1x1 rect gets stacked on top instead).
    selection.onCellMouseDown(makeCellEvent(1, { ctrlKey: true }), 1);
    expect(selection.isSelected(1, 1)).toBe(true);
    // The rest of the original block is untouched.
    expect(selection.isSelected(0, 0)).toBe(true);
    expect(selection.isSelected(2, 2)).toBe(true);
  });

  it("Shift+Click extends the active rectangle from its anchor", () => {
    const selection = createCellSelection();

    selection.onCellMouseDown(makeCellEvent(0), 0);
    selection.onCellMouseDown(makeCellEvent(3, { shiftKey: true }), 3);

    expect(selection.sel).toEqual({ r0: 0, r1: 3, c0: 0, c1: 3 });
    expect(selection.isSelected(0, 0)).toBe(true);
    expect(selection.isSelected(3, 3)).toBe(true);
    expect(selection.isSelected(1, 2)).toBe(true);
  });

  it("right-click outside the selection collapses it; right-click inside preserves it", () => {
    const selection = createCellSelection();

    // Drag out a block from (0,0) to (1,1).
    selection.onCellMouseDown(makeCellEvent(0), 0);
    selection.onCellMouseEnter(1, 1);
    expect(selection.isSelected(0, 0)).toBe(true);

    // Right-click inside the block: selection is preserved.
    const insideResult = selection.handleContextMenu(makeCellEvent(1, { clientX: 10, clientY: 20 }), 1);
    expect(insideResult).toEqual({ x: 10, y: 20 });
    expect(selection.isSelected(0, 0)).toBe(true);
    expect(selection.isSelected(1, 1)).toBe(true);

    // Right-click outside the block: collapses the selection to the clicked cell.
    const outsideResult = selection.handleContextMenu(makeCellEvent(5, { clientX: 30, clientY: 40 }), 5);
    expect(outsideResult).toEqual({ x: 30, y: 40 });
    expect(selection.isSelected(0, 0)).toBe(false);
    expect(selection.isSelected(5, 5)).toBe(true);
  });

  it("union bounding box (sel) spans all disjoint rectangles", () => {
    const selection = createCellSelection();

    // Three disjoint standalone cells via Ctrl+Click.
    selection.onCellMouseDown(makeCellEvent(0, { ctrlKey: true }), 0);
    selection.onCellMouseDown(makeCellEvent(5, { ctrlKey: true }), 5);
    selection.onCellMouseDown(makeCellEvent(9, { ctrlKey: true }), 2);

    expect(selection.sel).toEqual({ r0: 0, r1: 5, c0: 0, c1: 9 });
    // The rect list — not just the bbox — decides membership: a cell inside
    // the union bbox but outside every rect is not selected.
    expect(selection.isSelected(2, 2)).toBe(false);
    expect(selection.isSelected(0, 0)).toBe(true);
    expect(selection.isSelected(5, 5)).toBe(true);
    expect(selection.isSelected(2, 9)).toBe(true);
  });
});
