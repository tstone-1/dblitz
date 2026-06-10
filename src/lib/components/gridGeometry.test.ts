import { describe, expect, it } from "vitest";
import {
  buildGridTemplate,
  rowIndexToVirtualTop,
  virtualScrollGeometry,
  virtualScrollTopToDataScroll,
  visibleRowIndices,
} from "./gridGeometry";

describe("grid geometry helpers", () => {
  it("returns no visible indices for an empty grid", () => {
    expect(
      visibleRowIndices({
        rowCount: 0,
        rowHeight: 26,
        scrollTop: 0,
        stickyHeight: 26,
        viewportHeight: 600,
        overscan: 20,
      }),
    ).toEqual([]);
  });

  it("accounts for sticky header height and overscan when culling rows", () => {
    const indices = visibleRowIndices({
      rowCount: 100,
      rowHeight: 10,
      scrollTop: 75,
      stickyHeight: 25,
      viewportHeight: 30,
      overscan: 2,
    });

    expect(indices[0]).toBe(3);
    expect(indices.at(-1)).toBe(10);
  });

  it("clamps overscanned rows at the start of the data range", () => {
    const indices = visibleRowIndices({
      rowCount: 100,
      rowHeight: 10,
      scrollTop: 10,
      stickyHeight: 0,
      viewportHeight: 30,
      overscan: 20,
    });

    expect(indices[0]).toBe(0);
    expect(indices.at(-1)).toBe(24);
  });

  it("clamps visible row indices to the available row range", () => {
    expect(
      visibleRowIndices({
        rowCount: 5,
        rowHeight: 10,
        scrollTop: 1_000,
        stickyHeight: 0,
        viewportHeight: 100,
        overscan: 20,
      }),
    ).toEqual([]);
  });

  it("builds grid template tracks from persisted widths and defaults", () => {
    expect(buildGridTemplate(["id", "name"], { id: 120 })).toBe(
      "60px 120px minmax(80px, 1fr)",
    );
  });

  it("compresses virtual spacer height while preserving row mapping", () => {
    const geometry = virtualScrollGeometry({
      rowCount: 5_000_000,
      rowHeight: 26,
      maxSpacerHeight: 20_000_000,
    });

    expect(geometry.spacerHeight).toBe(20_000_000);
    expect(geometry.scale).toBe(6.5);
    expect(virtualScrollTopToDataScroll(0, geometry, 600)).toBe(0);
    expect(
      virtualScrollTopToDataScroll(geometry.spacerHeight - 600, geometry, 600),
    ).toBe(geometry.naturalHeight - 600);
    expect(rowIndexToVirtualTop(0, 26, geometry, 600)).toBe(0);
    expect(rowIndexToVirtualTop(4_999_000, 26, geometry, 600)).toBeLessThan(
      geometry.spacerHeight,
    );
  });
});
