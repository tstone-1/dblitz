import { describe, expect, it } from "vitest";
import { shouldAutoFitWidths } from "./autoFitWidths";

describe("shouldAutoFitWidths", () => {
  it("auto-fits a freshly opened table that has no saved widths", () => {
    expect(
      shouldAutoFitWidths({
        requestedTable: "customers",
        currentTable: "customers",
        savedWidths: {},
      }),
    ).toBe(true);
  });

  it("treats absent saved widths from an older config as no saved widths", () => {
    for (const savedWidths of [undefined, null]) {
      expect(
        shouldAutoFitWidths({
          requestedTable: "customers",
          currentTable: "customers",
          savedWidths,
        }),
      ).toBe(true);
    }
  });

  it("leaves hand-tuned widths alone", () => {
    expect(
      shouldAutoFitWidths({
        requestedTable: "customers",
        currentTable: "customers",
        savedWidths: { id: 80 },
      }),
    ).toBe(false);
  });

  it("does not auto-fit when the selection moved on mid-reload", () => {
    // The regression: table A has no saved widths, so its resumed tail said
    // "auto-fit" -- but by then table B was selected, and the auto-fit would
    // have been measured from B's grid and persisted into B's config.
    expect(
      shouldAutoFitWidths({
        requestedTable: "a_slow_table",
        currentTable: "b_hand_tuned",
        savedWidths: {},
      }),
    ).toBe(false);
  });

  it("does not auto-fit for a superseded table that has saved widths either", () => {
    expect(
      shouldAutoFitWidths({
        requestedTable: "a_slow_table",
        currentTable: "b_hand_tuned",
        savedWidths: { id: 80 },
      }),
    ).toBe(false);
  });

  it("does not auto-fit when the database closed while the reload was in flight", () => {
    expect(
      shouldAutoFitWidths({
        requestedTable: "a_slow_table",
        currentTable: null,
        savedWidths: {},
      }),
    ).toBe(false);
  });

  it("matches table names exactly rather than by prefix", () => {
    expect(
      shouldAutoFitWidths({
        requestedTable: "orders",
        currentTable: "orders_2024",
        savedWidths: {},
      }),
    ).toBe(false);
    expect(
      shouldAutoFitWidths({
        requestedTable: "orders_2024",
        currentTable: "orders",
        savedWidths: {},
      }),
    ).toBe(false);
  });
});
