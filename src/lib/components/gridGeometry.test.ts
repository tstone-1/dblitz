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
  });

  it("renders the visible window at true row height even when the spacer is compressed", () => {
    const geometry = virtualScrollGeometry({
      rowCount: 5_000_000,
      rowHeight: 26,
      maxSpacerHeight: 20_000_000,
    });
    expect(geometry.scale).toBeGreaterThan(1);

    // At the top, rows sit at their natural offsets.
    expect(rowIndexToVirtualTop(0, 26, geometry, 600, 0)).toBe(0);
    expect(rowIndexToVirtualTop(1, 26, geometry, 600, 0)).toBe(26);

    // Deep inside the compressed spacer the visible window is still spaced a
    // full rowHeight apart (the regression squished it to rowHeight / scale),
    // and the row aligned with the scroll offset lands within one row of it.
    const scrollTop = 10_000_000;
    const dataScroll = virtualScrollTopToDataScroll(scrollTop, geometry, 600);
    const anchorRow = Math.round(dataScroll / 26);
    const topAnchor = rowIndexToVirtualTop(anchorRow, 26, geometry, 600, scrollTop);
    const topNext = rowIndexToVirtualTop(anchorRow + 1, 26, geometry, 600, scrollTop);
    expect(topNext - topAnchor).toBe(26);
    expect(Math.abs(topAnchor - scrollTop)).toBeLessThan(26);

    // At the very bottom, the last row's bottom edge lands exactly on the
    // spacer height — no overflow past the scroll range.
    const maxScroll = geometry.spacerHeight - 600;
    const lastTop = rowIndexToVirtualTop(5_000_000 - 1, 26, geometry, 600, maxScroll);
    expect(lastTop + 26).toBeCloseTo(geometry.spacerHeight, 6);
  });

  it("is identical to natural positioning when the spacer is uncompressed", () => {
    const geometry = virtualScrollGeometry({
      rowCount: 1_000,
      rowHeight: 26,
      maxSpacerHeight: 20_000_000,
    });
    expect(geometry.scale).toBe(1);

    // With scale 1, dataScroll === scrollTop, so the formula reduces to
    // rowIndex * rowHeight regardless of the current scroll offset.
    for (const scrollTop of [0, 137, 9_999]) {
      expect(rowIndexToVirtualTop(0, 26, geometry, 600, scrollTop)).toBe(0);
      expect(rowIndexToVirtualTop(42, 26, geometry, 600, scrollTop)).toBe(42 * 26);
    }
  });
});
