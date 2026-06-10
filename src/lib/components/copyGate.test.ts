import { describe, expect, it } from "vitest";
import { shouldHandleWindowCopy } from "./copyGate";

const base = {
  hasSelection: true,
  targetTag: "DIV",
  isContentEditable: false,
  hasTextSelection: false,
  gridVisible: true,
};

describe("shouldHandleWindowCopy", () => {
  it("handles copy only for a visible grid with a cell selection", () => {
    expect(shouldHandleWindowCopy(base)).toBe(true);
    expect(shouldHandleWindowCopy({ ...base, gridVisible: false })).toBe(false);
    expect(shouldHandleWindowCopy({ ...base, hasSelection: false })).toBe(false);
  });

  it("does not override native text editing and selection copy", () => {
    expect(shouldHandleWindowCopy({ ...base, targetTag: "INPUT" })).toBe(false);
    expect(shouldHandleWindowCopy({ ...base, targetTag: "TEXTAREA" })).toBe(false);
    expect(shouldHandleWindowCopy({ ...base, isContentEditable: true })).toBe(false);
    expect(shouldHandleWindowCopy({ ...base, hasTextSelection: true })).toBe(false);
  });
});
