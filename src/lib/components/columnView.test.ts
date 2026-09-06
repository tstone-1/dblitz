import { describe, expect, it } from "vitest";
import {
  buildActiveFilters,
  colorPresetsForTheme,
  orderColumns,
  sameColumnList,
  stableColumnList,
  visibleColumns,
} from "./columnView";

describe("column view helpers", () => {
  it("keeps configured column order and appends newly discovered columns", () => {
    expect(orderColumns(["id", "name", "status"], ["status", "missing", "id"])).toEqual([
      "status",
      "id",
      "name",
    ]);
  });

  it("returns schema order when there is no configured order", () => {
    expect(orderColumns(["id", "name"], [])).toEqual(["id", "name"]);
  });

  it("filters hidden columns from an ordered column list", () => {
    expect(visibleColumns(["status", "id", "name"], ["id"])).toEqual(["status", "name"]);
  });

  it("builds active filters only for existing columns with non-empty values", () => {
    expect(
      buildActiveFilters(["id", "name"], {
        id: { value: "  ", is_regex: false },
        name: { value: "foo", is_regex: true },
        stale: { value: "bar", is_regex: false },
      }),
    ).toEqual([{ column: "name", value: "foo", is_regex: true }]);
  });

  it("returns theme-specific column color presets", () => {
    expect(colorPresetsForTheme("dark")[1]).toBe("#3b1c1c");
    expect(colorPresetsForTheme("light")[1]).toBe("#fde8e8");
  });
});

describe("sameColumnList", () => {
  it("compares element-wise, not by identity", () => {
    expect(sameColumnList(["a", "b"], ["a", "b"])).toBe(true);
    expect(sameColumnList(["a", "b"], ["b", "a"])).toBe(false);
    expect(sameColumnList(["a"], ["a", "b"])).toBe(false);
    expect(sameColumnList([], [])).toBe(true);
  });
});

describe("stableColumnList", () => {
  it("keeps the same array instance while the contents are unchanged", () => {
    // The point of the memo: a config commit re-runs the derived, and a fresh
    // array with identical contents re-runs DataGrid's width-reseed effect and
    // its whole header diff for nothing.
    const stable = stableColumnList();
    const first = stable(["id", "name"]);
    const second = stable(["id", "name"]);
    expect(second).toBe(first);
  });

  it("publishes a new instance when the list actually changes", () => {
    // Emptiness control: a memo that always returned the first array would
    // satisfy the test above and freeze the grid on one table's columns.
    const stable = stableColumnList();
    const first = stable(["id", "name"]);
    const second = stable(["id", "name", "extra"]);
    expect(second).not.toBe(first);
    expect(second).toEqual(["id", "name", "extra"]);
    const third = stable(["id", "name", "extra"]);
    expect(third).toBe(second);
  });
});
