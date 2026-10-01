import { describe, expect, it } from "vitest";
import { findCell, makeCellMatcher, type FindCellOptions } from "./gridFind";

type Row = (string | null)[];

// 6 rows x 3 visible columns. "1200" sits at (1,2), (3,0) and (4,1).
const ROWS: Row[] = [
  ["a", "b", "c"],
  ["d", "e", "x1200"],
  ["f", null, "g"],
  ["1200.0", "h", "i"],
  ["j", "AB1200", "k"],
  ["l", "m", "n"],
];

function harness(overrides: Partial<FindCellOptions> = {}, rows: Row[] = ROWS) {
  const loads: Array<[number, number]> = [];
  const opts: FindCellOptions = {
    rowCount: rows.length,
    colCount: rows[0]?.length ?? 0,
    start: null,
    direction: 1,
    matches: makeCellMatcher("1200"),
    loadRows: async (first, last) => {
      loads.push([first, last]);
      return rows.slice(first, last + 1);
    },
    blockRows: 2,
    isCancelled: () => false,
    ...overrides,
  };
  return { opts, loads, run: () => findCell(opts) };
}

describe("makeCellMatcher", () => {
  it("matches a case-insensitive substring of the cell text", () => {
    const m = makeCellMatcher("ab");
    expect(m("xABy")).toBe(true);
    expect(m("a b")).toBe(false);
  });

  it("never matches NULL, even when the query is the word NULL", () => {
    expect(makeCellMatcher("null")(null)).toBe(false);
    expect(makeCellMatcher("NULL")("NULL")).toBe(true);
  });

  it("matches nothing for an empty or blank query", () => {
    expect(makeCellMatcher("")("anything")).toBe(false);
    expect(makeCellMatcher("   ")("a   b")).toBe(false);
  });
});

describe("findCell", () => {
  it("finds the first match in reading order when nothing is selected", async () => {
    expect(await harness().run()).toEqual({ kind: "found", row: 1, col: 2, wrapped: false });
  });

  it("starts after the current cell, not on it", async () => {
    const { run } = harness({ start: { row: 1, col: 2 } });
    expect(await run()).toEqual({ kind: "found", row: 3, col: 0, wrapped: false });
  });

  it("walks left to right within a row before moving down", async () => {
    const rows: Row[] = [["1200", "x", "1200"]];
    const { run } = harness({ start: { row: 0, col: 0 } }, rows);
    expect(await run()).toEqual({ kind: "found", row: 0, col: 2, wrapped: false });
  });

  it("wraps from the last match to the first and says so", async () => {
    const { run } = harness({ start: { row: 4, col: 1 } });
    expect(await run()).toEqual({ kind: "found", row: 1, col: 2, wrapped: true });
  });

  it("goes to the previous match with direction -1", async () => {
    const { run } = harness({ start: { row: 4, col: 1 }, direction: -1 });
    expect(await run()).toEqual({ kind: "found", row: 3, col: 0, wrapped: false });
  });

  it("wraps backwards from the first match to the last", async () => {
    const { run } = harness({ start: { row: 1, col: 2 }, direction: -1 });
    expect(await run()).toEqual({ kind: "found", row: 4, col: 1, wrapped: true });
  });

  it("starts at the last cell for direction -1 with nothing selected", async () => {
    const { run } = harness({ direction: -1 });
    expect(await run()).toEqual({ kind: "found", row: 4, col: 1, wrapped: false });
  });

  it("finds the only match again from the match itself, as a wrap", async () => {
    const rows: Row[] = [["a", "b"], ["c", "1200"], ["d", "e"]];
    const { run } = harness({ start: { row: 1, col: 1 } }, rows);
    expect(await run()).toEqual({ kind: "found", row: 1, col: 1, wrapped: true });
  });

  it("reports no match after trying every cell once", async () => {
    const { run, loads } = harness({ matches: makeCellMatcher("zzz") });
    expect(await run()).toEqual({ kind: "none" });
    const rowsLoaded = loads.reduce((n, [a, b]) => n + b - a + 1, 0);
    expect(rowsLoaded).toBe(ROWS.length);
  });

  it("reports no match for an empty grid without loading anything", async () => {
    const { run, loads } = harness({ rowCount: 0 });
    expect(await run()).toEqual({ kind: "none" });
    expect(loads).toEqual([]);
  });

  it("loads aligned blocks inside the row range", async () => {
    const { run, loads } = harness({ start: { row: 2, col: 0 }, matches: makeCellMatcher("zzz"), blockRows: 4 });
    await run();
    for (const [first, last] of loads) {
      expect(first % 4).toBe(0);
      expect(last).toBeLessThanOrEqual(ROWS.length - 1);
    }
    expect(loads[0]).toEqual([0, 3]);
    expect(loads[1]).toEqual([4, 5]);
  });

  it("stops as cancelled when the view changes during a load", async () => {
    let cancelled = false;
    const { run } = harness({
      matches: makeCellMatcher("zzz"),
      loadRows: async (first, last) => {
        cancelled = first >= 2;
        return ROWS.slice(first, last + 1);
      },
      isCancelled: () => cancelled,
    });
    expect(await run()).toEqual({ kind: "cancelled" });
  });

  it("does not report a match found after a cancel", async () => {
    const { run } = harness({ isCancelled: () => true });
    expect(await run()).toEqual({ kind: "cancelled" });
  });

  it("fails instead of reporting no match when rows cannot be loaded", async () => {
    const { run } = harness({
      matches: makeCellMatcher("zzz"),
      loadRows: async (first) => {
        if (first === 2) throw new Error("chunk failed");
        return ROWS.slice(first, first + 2);
      },
    });
    await expect(run()).rejects.toThrow("chunk failed");
  });

  it("fails when a block comes back short", async () => {
    const { run } = harness({
      matches: makeCellMatcher("zzz"),
      loadRows: async (first, last) => ROWS.slice(first, last),
    });
    await expect(run()).rejects.toThrow("could not be loaded");
  });

  it("reports progress up to the row count", async () => {
    const seen: number[] = [];
    const { run } = harness({ matches: makeCellMatcher("zzz"), onProgress: (n) => seen.push(n) });
    await run();
    expect(seen).toEqual([2, 4, 6]);
  });
});
