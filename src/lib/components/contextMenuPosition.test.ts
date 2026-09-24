import { describe, expect, it } from "vitest";
import { placeContextMenu } from "./contextMenuPosition";

const viewport = { width: 1200, height: 800 };
const menu = { width: 200, height: 300 };

describe("placeContextMenu", () => {
  it("opens at the pointer when the menu fits", () => {
    expect(placeContextMenu({ x: 100, y: 100 }, menu, viewport)).toEqual({ x: 100, y: 100 });
  });

  it("flips left of the pointer at the right window border", () => {
    expect(placeContextMenu({ x: 1150, y: 100 }, menu, viewport)).toEqual({ x: 950, y: 100 });
  });

  it("flips above the pointer at the bottom window border", () => {
    expect(placeContextMenu({ x: 100, y: 700 }, menu, viewport)).toEqual({ x: 100, y: 400 });
  });

  it("flips on both axes in the bottom-right corner", () => {
    expect(placeContextMenu({ x: 1190, y: 790 }, menu, viewport)).toEqual({ x: 990, y: 490 });
  });

  it("keeps a menu larger than the space on either side inside the window", () => {
    const tall = { width: 200, height: 780 };
    expect(placeContextMenu({ x: 100, y: 400 }, tall, viewport).y).toBe(16);
    const tiny = { width: 150, height: 100 };
    expect(placeContextMenu({ x: 100, y: 50 }, menu, tiny)).toEqual({ x: 4, y: 4 });
  });

  it("stays at the pointer when the menu exactly fits the margin", () => {
    expect(placeContextMenu({ x: 996, y: 496 }, menu, viewport)).toEqual({ x: 996, y: 496 });
    expect(placeContextMenu({ x: 997, y: 497 }, menu, viewport)).toEqual({ x: 797, y: 197 });
  });
});
