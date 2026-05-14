import { afterEach, describe, expect, it, vi } from "vitest";
import { createDragReorder } from "./dragReorder.svelte";

function withTarget(event: MouseEvent, target: HTMLElement): MouseEvent {
  Object.defineProperty(event, "target", { value: target });
  return event;
}

describe("dragReorder", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reorders when the pointer is dragged over another header", () => {
    const onReorder = vi.fn();
    const columns = ["id", "name", "email"];
    const reorder = createDragReorder(() => columns, () => onReorder);
    const targetHeader = document.createElement("div");
    targetHeader.className = "col-header";
    targetHeader.dataset.colidx = "2";
    const startHeader = document.createElement("div");
    startHeader.className = "col-header";
    Object.defineProperty(document, "elementFromPoint", {
      configurable: true,
      value: vi.fn(() => targetHeader),
    });

    reorder.onMouseDown(
      withTarget(new MouseEvent("mousedown", { button: 0, clientX: 0, clientY: 0 }), startHeader),
      "name",
    );
    document.dispatchEvent(new MouseEvent("mousemove", { clientX: 10, clientY: 0 }));
    document.dispatchEvent(new MouseEvent("mouseup"));

    expect(onReorder).toHaveBeenCalledWith("name", "email");
    expect(reorder.consumeReorder()).toBe(true);
    expect(reorder.consumeReorder()).toBe(false);
    reorder.destroy();
  });

  it("ignores resize-handle drags", () => {
    const onReorder = vi.fn();
    const reorder = createDragReorder(() => ["id", "name"], () => onReorder);
    const resizeHandle = document.createElement("div");
    resizeHandle.className = "resize-handle";

    reorder.onMouseDown(
      withTarget(new MouseEvent("mousedown", { button: 0, clientX: 0, clientY: 0 }), resizeHandle),
      "id",
    );
    document.dispatchEvent(new MouseEvent("mousemove", { clientX: 10, clientY: 0 }));
    document.dispatchEvent(new MouseEvent("mouseup"));

    expect(onReorder).not.toHaveBeenCalled();
  });
});
