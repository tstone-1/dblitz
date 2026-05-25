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
