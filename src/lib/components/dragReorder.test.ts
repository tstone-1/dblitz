// @vitest-environment jsdom
//
// `createDragReorder` is the header drag-to-reorder state machine DataGrid uses.
// It is deliberately built on raw mouse events with a movement dead zone rather
// than HTML5 drag-and-drop, because HTML5 DnD has never been proven in the
// Windows WebView2 runtime (see AGENTS.md) -- so the dead zone, the
// `elementFromPoint` hit test and the click-suppression handshake are hand-
// rolled logic with no framework behind them, and nothing tested them.
//
// jsdom supplies `document` and listener registration. It does NOT implement
// `document.elementFromPoint` AT ALL -- the property is absent, which is why
// the helper below defines it rather than spying on it, and why the first test
// asserts that absence: a `vi.spyOn` there fails with "The property
// elementFromPoint is not defined on the object", which reads as a defect in
// the code under test rather than a gap in the environment.

import { beforeEach, describe, expect, it } from "vitest";
import { createDragReorder } from "./dragReorder.svelte";

const COLUMNS = ["id", "name", "status"];

/** Puts a `.col-header` for `colIdx` under the cursor for the next hit test. */
function headerUnderCursor(colIdx: number | null) {
  Object.defineProperty(document, "elementFromPoint", {
    configurable: true,
    writable: true,
    value: () => {
      if (colIdx === null) return null;
      const header = document.createElement("div");
      header.className = "col-header";
      header.dataset.colidx = String(colIdx);
      return header;
    },
  });
}

function mouse(type: string, clientX: number, clientY: number, target?: EventTarget) {
  const event = new window.MouseEvent(type, {
    bubbles: true,
    button: 0,
    clientX,
    clientY,
  });
  if (target) Object.defineProperty(event, "target", { value: target });
  return event;
}

function plainTarget() {
  const el = document.createElement("div");
  el.className = "col-header";
  return el;
}

describe("jsdom environment", () => {
  it("does not implement elementFromPoint, which the tests below supply", () => {
    // Environment fact asserted before the application: the drag hit test is
    // built on `document.elementFromPoint`, and if jsdom ever grows a real one
    // the stub below is silently replacing a working implementation.
    expect(
      Object.getOwnPropertyDescriptor(Document.prototype, "elementFromPoint"),
    ).toBeUndefined();
  });
});

beforeEach(() => {
  headerUnderCursor(null);
});

describe("createDragReorder", () => {
  it("reorders when the drag crosses the dead zone and ends over another header", () => {
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(2);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    expect(reorder.reorderCol).toBe("id");
    expect(reorder.reorderOverCol).toBe("status");

    document.dispatchEvent(mouse("mouseup", 140, 10));
    expect(calls).toEqual([["id", "status"]]);
    expect(reorder.reorderCol).toBeNull();
    expect(reorder.reorderOverCol).toBeNull();
  });

  it("does not start a drag inside the movement dead zone", () => {
    // A header click is a SORT. Without the dead zone every sort click that
    // moved the mouse by a pixel would be read as a reorder attempt and then
    // swallow its own click.
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(2);
    document.dispatchEvent(mouse("mousemove", 103, 12)); // |dx|+|dy| = 5 < 6
    expect(reorder.reorderCol).toBeNull();

    document.dispatchEvent(mouse("mouseup", 103, 12));
    expect(calls).toEqual([]);
    expect(reorder.consumeReorder()).toBe(false); // the click must still sort
  });

  it("consumes exactly one click after a completed drag", () => {
    // The handshake DataGrid relies on: the mouseup that ends a drag is
    // followed by a click on the header, and that click must not also sort.
    // Exactly one -- a latch that never cleared would eat the next real sort.
    const reorder = createDragReorder(
      () => COLUMNS,
      () => () => {},
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(1);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    document.dispatchEvent(mouse("mouseup", 140, 10));

    expect(reorder.consumeReorder()).toBe(true);
    expect(reorder.consumeReorder()).toBe(false);
  });

  it("sets didReorder even when the drop lands back on the source column", () => {
    // No reorder is emitted, but the pointer still travelled -- treating that
    // as a click would sort a column the user was only dragging.
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(0);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    document.dispatchEvent(mouse("mouseup", 140, 10));

    expect(calls).toEqual([]);
    expect(reorder.consumeReorder()).toBe(true);
  });

  it("ignores a mousedown that starts on the resize handle", () => {
    // The resize handle sits inside the header, so without this check every
    // column resize would also be a reorder drag.
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    const handle = document.createElement("div");
    handle.className = "resize-handle";
    reorder.onMouseDown(mouse("mousedown", 100, 10, handle), "id");
    headerUnderCursor(1);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    document.dispatchEvent(mouse("mouseup", 140, 10));

    expect(reorder.reorderCol).toBeNull();
    expect(calls).toEqual([]);
    expect(reorder.consumeReorder()).toBe(false);
  });

  it("ignores a drag when the grid supplies no reorder callback", () => {
    // Static mode (the SQL result grid) wires no `onReorderColumn`.
    const reorder = createDragReorder(
      () => COLUMNS,
      () => undefined,
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(1);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    document.dispatchEvent(mouse("mouseup", 140, 10));

    expect(reorder.reorderCol).toBeNull();
    expect(reorder.consumeReorder()).toBe(false);
  });

  it("ignores a header index that is out of range for the current columns", () => {
    // `data-colidx` is read back out of the DOM, and the column list can shrink
    // (hide a column) between render and drop.
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    headerUnderCursor(9);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    expect(reorder.reorderOverCol).toBeNull();
    document.dispatchEvent(mouse("mouseup", 140, 10));
    expect(calls).toEqual([]);
  });

  it("detaches its document listeners when destroyed mid-drag", () => {
    // DataGrid calls destroy() on unmount. A drag left armed would keep a
    // mousemove handler on `document` for the life of the page.
    const calls: Array<[string, string]> = [];
    const reorder = createDragReorder(
      () => COLUMNS,
      () => (from, to) => calls.push([from, to]),
    );

    reorder.onMouseDown(mouse("mousedown", 100, 10, plainTarget()), "id");
    reorder.destroy();
    headerUnderCursor(1);
    document.dispatchEvent(mouse("mousemove", 140, 10));
    document.dispatchEvent(mouse("mouseup", 140, 10));

    expect(reorder.reorderCol).toBeNull();
    expect(calls).toEqual([]);
  });
});
