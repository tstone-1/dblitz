import { describe, expect, it } from "vitest";
import { computeAutoWidths } from "./columnWidths";

function measurer() {
  return {
    font: "",
    measureText: (text: string) => ({ width: text.length * 10 }),
  };
}

describe("computeAutoWidths", () => {
  it("uses header and sampled cell widths", () => {
    expect(
      computeAutoWidths({
        columns: ["id", "description"],
        rows: [["1", "wide value"]],
        getColumnIndex: (column) => (column === "id" ? 0 : 1),
        measurer: measurer(),
      }),
    ).toEqual({
      id: 68,
      description: 158,
    });
  });

  it("measures NULL display text and ignores empty strings", () => {
    expect(
      computeAutoWidths({
        columns: ["x"],
        rows: [[null], [""]],
        getColumnIndex: () => 0,
        measurer: measurer(),
      }),
    ).toEqual({ x: 64 });
  });

  it("clamps auto widths to minimum and maximum values", () => {
    expect(
      computeAutoWidths({
        columns: ["x", "huge"],
        rows: [["", "x".repeat(100)]],
        getColumnIndex: (column) => (column === "x" ? 0 : 1),
        measurer: measurer(),
      }),
    ).toEqual({
      x: 60,
      huge: 400,
    });
  });

  it("limits sampled rows", () => {
    expect(
      computeAutoWidths({
        columns: ["x"],
        rows: [["small"], ["x".repeat(100)]],
        getColumnIndex: () => 0,
        measurer: measurer(),
        maxSample: 1,
      }),
    ).toEqual({ x: 74 });
  });
});
