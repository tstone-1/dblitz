import { describe, expect, it } from "vitest";
import { buildSelectionStats, DEFAULT_MAX_STATS_ROWS } from "./selectionStats";

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
      capped: false,
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
      capped: false,
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
      capped: false,
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
      capped: false,
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
      capped: false,
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
      capped: false,
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

  it("caps scanned rows while reporting full selection dimensions and suppressing partial aggregates", () => {
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
      // Capped: only rows 0-1 were ever scanned, so the aggregates would be
      // partial. Row/col counts stay exact because this is a plain
      // rectangle (no `isSelected`), which is pure geometry regardless of
      // how much of it was scanned for data.
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: false,
      capped: true,
    });
  });

  it("reports the true row count for a capped, fully-dense (Ctrl+A-shaped) isSelected selection", () => {
    // Mirrors what Ctrl+A actually produces (see cellSelection.svelte.ts's
    // `setSelection`): a single filled rectangle exposed through
    // `isSelected`, spanning more rows than the aggregate-scan cap. Every
    // cell within the scanned window is selected, so the density check
    // proves the rectangle continues past the cap.
    const totalRows = 5;
    const maxRows = 3;
    const rows = Array.from({ length: totalRows }, (_, r) => [String(r), String(r * 10)]);

    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: totalRows - 1, c0: 0, c1: 1 },
        getRow: (index) => rows[index] ?? null,
        maxRows,
        isSelected: () => true,
      }),
    ).toEqual({
      rows: totalRows, // true count, not the scanned/capped 3
      cols: 2,
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: false,
      capped: true,
    });
  });

  it("falls back to the scanned-only count for a capped, genuinely sparse disjoint selection", () => {
    // A Ctrl+Click disjoint selection only ever selects a handful of cells,
    // so it shows gaps well inside the scanned window -- the density check
    // fails and we can only report what was actually proven selected.
    const selected = new Set(["0,0", "2,0"]); // rows 0 and 2 only, out of a 0..4 box
    const rows = [["1"], ["2"], ["3"], ["4"], ["5"]];

    expect(
      buildSelectionStats({
        selection: { r0: 0, r1: 4, c0: 0, c1: 0 },
        getRow: (index) => rows[index] ?? null,
        maxRows: 3, // caps the scan at row 2 (rows 3-4 never visited)
        isSelected: (r, c) => selected.has(`${r},${c}`),
      }),
    ).toEqual({
      rows: 2, // only what was actually scanned & proven (rows 0 and 2)
      cols: 1,
      sum: null,
      avg: null,
      min: null,
      max: null,
      numericPending: false,
      capped: true,
    });
  });

  it("exposes the default cap as DEFAULT_MAX_STATS_ROWS for callers to reference in UI copy", () => {
    expect(DEFAULT_MAX_STATS_ROWS).toBe(100_000);
  });
});
