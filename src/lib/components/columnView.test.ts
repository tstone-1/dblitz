import { describe, expect, it } from "vitest";
import {
  buildActiveFilters,
  colorPresetsForTheme,
  orderColumns,
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
