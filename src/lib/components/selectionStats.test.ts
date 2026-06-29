import { describe, expect, it } from "vitest";
import { buildSelectionStats } from "./selectionStats";

describe("buildSelectionStats", () => {
  it("returns no stats for an empty or single-cell selection", () => {
    expect(buildSelectionStats({ selection: null, getRow: () => [] })).toBeNull();
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 0, c0: 0, c1: 0 },
        getRow: () => ["1"],
      }),
    ).toBeNull();
  });

  it("summarizes numeric selections while ignoring blanks and nulls", () => {
    const rows = [
      ["1", "2", ""],
      ["3", null, "4"],
    ];

    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 1, c0: 0, c1: 2 },
        getRow: (index) => rows[index] ?? null,
      }),
    ).toEqual({
      rows: 2,
      cols: 3,
      sum: 10,
      avg: 2.5,
      min: 1,
      max: 4,
      numericPending: false,
    });
  });

  it("suppresses numeric aggregates when any loaded value is non-numeric", () => {
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 0, c0: 0, c1: 1 },
        getRow: () => ["1", "not-a-number"],
      }),
    ).toEqual({
      rows: 1,
      cols: 2,
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: false,
    });
  });

  it("returns null numeric aggregates when the selection has no numeric cells", () => {
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 1, c0: 0, c1: 1 },
        getRow: (index) => (index === 0 ? ["", null] : [null, ""]),
      }),
    ).toEqual({
      rows: 2,
      cols: 2,
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: false,
    });
  });

  it("tracks negative and decimal values", () => {
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 1, c0: 0, c1: 1 },
        getRow: (index) => (index === 0 ? ["-1.5", "2.5"] : ["0", "-3"]),
      }),
    ).toEqual({
      rows: 2,
      cols: 2,
      sum: -2,
      avg: -0.5,
      min: -3,
      max: 2.5,
      numericPending: false,
    });
  });

  it("marks numeric stats as pending when a selected row is not loaded", () => {
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 1, c0: 0, c1: 0 },
        getRow: (index) => (index === 0 ? ["1"] : null),
      }),
    ).toEqual({
      rows: 2,
      cols: 1,
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: true,
    });
  });

  it("counts only selected cells and distinct rows/cols for a disjoint selection", () => {
    // Bounding box 0..2 x 0..2; selected cells (0,0)=1, (0,2)=10, (2,1)=4.
    const grid = [
      ["1", "99", "10"],
      ["99", "99", "99"],
      ["99", "4", "99"],
    ];
    const selected = new Set(["0,0", "0,2", "2,1"]);
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 2, c0: 0, c1: 2 },
        getRow: (index) => grid[index] ?? null,
        isSelected: (r, c) => selected.has(`${r},${c}`),
      }),
    ).toEqual({
      rows: 2, // distinct selected rows: 0 and 2
      cols: 3, // distinct selected cols: 0, 1, 2
      sum: 15,
      avg: 5,
      min: 1,
      max: 10,
      numericPending: false,
    });
  });

  it("returns null for a single-cell disjoint selection", () => {
    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 0, c0: 0, c1: 0 },
        getRow: () => ["1"],
        isSelected: (r, c) => r === 0 && c === 0,
      }),
    ).toBeNull();
  });

  it("caps scanned rows while reporting full selection dimensions", () => {
    const rows = [["1"], ["2"], ["not-scanned"]];

    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 2, c0: 0, c1: 0 },
        getRow: (index) => rows[index] ?? null,
        maxRows: 2,
      }),
    ).toEqual({
      rows: 3,
      cols: 1,
      sum: 3,
      avg: 1.5,
      min: 1,
      max: 2,
      numericPending: false,
    });
  });
});
