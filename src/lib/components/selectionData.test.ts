import { describe, expect, it } from "vitest";
import { buildSelectionData } from "./selectionData";

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
      rows: [["0"], ["1"]],
      truncated: true,
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
});
