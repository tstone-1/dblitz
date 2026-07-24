import { describe, expect, it } from "vitest";
import { pinToggleLabel } from "./pinLabel";

describe("pinToggleLabel", () => {
  it("labels the three states for a column filter", () => {
    expect(pinToggleLabel("pinned", "filter")).toBe("Unpin filter");
    expect(pinToggleLabel("modified", "filter")).toBe(
      "Re-pin filter (save current value)",
    );
    expect(pinToggleLabel("none", "filter")).toBe("Pin filter (save as default)");
  });

  it("labels the three states for the global filter", () => {
    expect(pinToggleLabel("pinned", "global filter")).toBe("Unpin global filter");
    expect(pinToggleLabel("modified", "global filter")).toBe(
      "Re-pin global filter (save current value)",
    );
    expect(pinToggleLabel("none", "global filter")).toBe(
      "Pin global filter (save as default)",
    );
  });
});
