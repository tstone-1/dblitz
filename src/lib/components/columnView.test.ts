import { describe, expect, it } from "vitest";
import {
  applyColumnPreset,
  buildActiveFilters,
  colorPresetsForTheme,
  normalizePresetName,
  orderColumns,
  PRESET_NAME_MAX_LEN,
  removeColumnPreset,
  sameColumnList,
  stableColumnList,
  upsertColumnPreset,
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

describe("column presets", () => {
  const columns = ["id", "name", "price", "currency", "notes"];

  it("shows exactly the preset's columns in its order and hides the rest", () => {
    const applied = applyColumnPreset(columns, [], { name: "P", columns: ["price", "id"] });
    expect(applied).toEqual({
      hidden_columns: ["name", "currency", "notes"],
      column_order: ["price", "id", "name", "currency", "notes"],
      missing: [],
    });
    // The visible result is the preset, in its order.
    expect(visibleColumns(orderColumns(columns, applied!.column_order), applied!.hidden_columns))
      .toEqual(["price", "id"]);
  });

  it("keeps the hidden columns in their current order behind the shown ones", () => {
    const applied = applyColumnPreset(columns, ["notes", "currency", "name", "price", "id"], {
      name: "P",
      columns: ["id"],
    });
    expect(applied!.column_order).toEqual(["id", "notes", "currency", "name", "price"]);
  });

  it("hides a column the table gained after the preset was saved", () => {
    const applied = applyColumnPreset([...columns, "added_later"], [], {
      name: "P",
      columns: ["id", "name"],
    });
    expect(applied!.hidden_columns).toContain("added_later");
  });

  it("reports preset columns the table does not have and skips them", () => {
    const applied = applyColumnPreset(columns, [], { name: "P", columns: ["id", "gone", "also_gone"] });
    expect(applied!.missing).toEqual(["gone", "also_gone"]);
    expect(applied!.column_order[0]).toBe("id");
    expect(applied!.column_order).not.toContain("gone");
  });

  it("refuses a preset none of whose columns exist, instead of hiding everything", () => {
    expect(applyColumnPreset(columns, [], { name: "P", columns: ["gone"] })).toBeNull();
  });

  it("collapses a column listed twice in a preset", () => {
    const applied = applyColumnPreset(columns, [], { name: "P", columns: ["id", "id"] });
    expect(applied!.column_order.filter((c) => c === "id")).toHaveLength(1);
  });

  it("overwrites a preset of the same name in place and appends a new one", () => {
    const presets = [
      { name: "A", columns: ["id"] },
      { name: "B", columns: ["name"] },
    ];
    expect(upsertColumnPreset(presets, "A", ["price"])).toEqual([
      { name: "A", columns: ["price"] },
      { name: "B", columns: ["name"] },
    ]);
    expect(upsertColumnPreset(presets, "C", ["notes"]).map((p) => p.name)).toEqual(["A", "B", "C"]);
    // The input list is not mutated: the caller assigns the result.
    expect(presets[0].columns).toEqual(["id"]);
  });

  it("stores a copy of the column list, not the caller's array", () => {
    const visible = ["id"];
    const [saved] = upsertColumnPreset([], "A", visible);
    visible.push("name");
    expect(saved.columns).toEqual(["id"]);
  });

  it("removes a preset by name", () => {
    expect(removeColumnPreset([{ name: "A", columns: ["id"] }, { name: "B", columns: ["id"] }], "A"))
      .toEqual([{ name: "B", columns: ["id"] }]);
  });

  it("trims and caps a preset name the way config.rs does", () => {
    expect(normalizePresetName("  Pricing  ")).toBe("Pricing");
    expect([...normalizePresetName("ä".repeat(PRESET_NAME_MAX_LEN + 5))]).toHaveLength(PRESET_NAME_MAX_LEN);
  });
});
