import { describe, expect, it } from "vitest";
import { buildSelectionData, selectionColumnTypes } from "./selectionData";

describe("buildSelectionData", () => {
  it("loads unloaded virtual rows before serializing a selection", async () => {
    const data = await buildSelectionData({
      selection: { r0: 1, r1: 3, c0: 0, c1: 1 },
      columns: ["id", "name", "ignored"],
      getRow: () => null,
      getRows: async (start, end) => {
        expect([start, end]).toEqual([1, 3]);
        return [
          ["1", "alpha", "x"],
          ["2", "bravo", "y"],
          ["3", "charlie", "z"],
        ];
      },
    });

    expect(data).toEqual({
      headers: ["id", "name"],
      columnIndices: [0, 1],
      rows: [
        ["1", "alpha"],
        ["2", "bravo"],
        ["3", "charlie"],
      ],
      truncated: false,
    });
  });

  it("reports when a selection is truncated by the row cap", async () => {
    const data = await buildSelectionData({
      selection: { r0: 0, r1: 2, c0: 0, c1: 0 },
      columns: ["id"],
      getRow: (index) => [String(index)],
      maxRows: 2,
    });

    expect(data).toEqual({
      headers: ["id"],
      columnIndices: [0],
      rows: [["0"], ["1"]],
      truncated: true,
    });
  });

  it("blanks unselected cells and drops empty rows for a disjoint selection", async () => {
    // Union bounding box r0..r1=0..2, c0..c1=0..2. Selected cells: (0,0), (0,2),
    // (2,1). Row 1 has no selected cell and is dropped; unselected cells blank.
    const grid = [
      ["a0", "b0", "c0"],
      ["a1", "b1", "c1"],
      ["a2", "b2", "c2"],
    ];
    const selected = new Set(["0,0", "0,2", "2,1"]);
    const data = await buildSelectionData({
      selection: { r0: 0, r1: 2, c0: 0, c1: 2 },
      columns: ["A", "B", "C"],
      getRow: (index) => grid[index] ?? null,
      isSelected: (r, c) => selected.has(`${r},${c}`),
    });

    expect(data).toEqual({
      headers: ["A", "B", "C"],
      columnIndices: [0, 1, 2],
      rows: [
        ["a0", "", "c0"],
        ["", "b2", ""],
      ],
      truncated: false,
    });
  });

  it("fails instead of synthesizing blanks when no row materializer is provided", async () => {
    await expect(
      buildSelectionData({
        selection: { r0: 0, r1: 0, c0: 0, c1: 0 },
        columns: ["id"],
        getRow: () => null,
      }),
    ).rejects.toThrow("no row materializer");
  });

  it("reports materialized rows that could not be loaded", async () => {
    await expect(
      buildSelectionData({
        selection: { r0: 0, r1: 0, c0: 0, c1: 0 },
        columns: ["id"],
        getRow: () => null,
        getRows: async () => [],
      }),
    ).rejects.toThrow("could not be loaded");
  });

  it("records the source column index of every header, not just its name", async () => {
    // The regression this pins: a selection of columns 2..3 used to carry only
    // the sliced NAMES, so a consumer recovering per-column metadata searched
    // from index 0 and resolved the wrong occurrence of a repeated name.
    const data = await buildSelectionData({
      selection: { r0: 0, r1: 0, c0: 2, c1: 3 },
      columns: ["value", "other", "value", "value"],
      getRow: () => ["a", "b", "c", "d"],
    });

    expect(data?.headers).toEqual(["value", "value"]);
    expect(data?.columnIndices).toEqual([2, 3]);
  });
});

describe("selectionColumnTypes", () => {
  it("resolves a later duplicate column name to its own declared type", async () => {
    // `SELECT a.value, b.value` with types INTEGER, TEXT. Selecting ONLY the
    // second column previously resolved the first `value` (a forward search
    // that started at 0), so the export marked text numeric and the workbook
    // turned "00123" into 123.
    const columns = ["value", "value"];
    const columnTypes = ["INTEGER", "TEXT"];
    const data = await buildSelectionData({
      selection: { r0: 0, r1: 0, c0: 1, c1: 1 },
      columns,
      getRow: () => ["1", "00123"],
    });

    expect(data?.rows).toEqual([["00123"]]);
    expect(selectionColumnTypes(data!.columnIndices, columnTypes)).toEqual(["TEXT"]);
  });

  it("resolves an earlier duplicate to its own type as well", async () => {
    // Control for the case above: if the mapping ignored position entirely and
    // always answered "TEXT", the assertion above would pass for the wrong
    // reason. Selecting column 0 of the same result must give INTEGER.
    const data = await buildSelectionData({
      selection: { r0: 0, r1: 0, c0: 0, c1: 0 },
      columns: ["value", "value"],
      getRow: () => ["1", "00123"],
    });

    expect(selectionColumnTypes(data!.columnIndices, ["INTEGER", "TEXT"])).toEqual([
      "INTEGER",
    ]);
  });

  it("reports an unknown type as empty rather than guessing a neighbour's", () => {
    expect(selectionColumnTypes([0, 5], ["TEXT"])).toEqual(["TEXT", ""]);
  });
});
