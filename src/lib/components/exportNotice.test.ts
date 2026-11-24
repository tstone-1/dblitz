import { describe, expect, it } from "vitest";
import { exportNotice } from "./exportNotice";

describe("exportNotice", () => {
  it("names the path alone when no cell was cut", () => {
    expect(exportNotice({ path: "/tmp/dblitz_export_1.xlsx", truncated_cells: 0 })).toBe(
      "Excel export written to /tmp/dblitz_export_1.xlsx",
    );
  });

  it("reports row and cell limits together with the destination", () => {
    for (const truncated_cells of [0, 1]) {
      const notice = exportNotice({ path: "result.xlsx", truncated_cells }, 100000);
      expect(notice).toContain("result.xlsx");
      expect(notice).toContain((100000).toLocaleString());
      expect(notice.includes("1 cell was cut")).toBe(truncated_cells === 1);
    }
  });

  it("says how many cells were cut, in the singular and the plural", () => {
    expect(exportNotice({ path: "/tmp/a.xlsx", truncated_cells: 1 })).toBe(
      "Excel export written to /tmp/a.xlsx. 1 cell was cut to Excel's limit of 32,767 characters.",
    );
    expect(exportNotice({ path: "/tmp/a.xlsx", truncated_cells: 12 })).toBe(
      "Excel export written to /tmp/a.xlsx. 12 cells were cut to Excel's limit of 32,767 characters.",
    );
  });
});
