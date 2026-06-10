import { describe, expect, it } from "vitest";
import { INCOMPLETE_OPS, OPERAND_REQUIRED_OPS } from "./filterOperators";

describe("filter operator metadata", () => {
  it("matches the backend operand-requiring operator set", () => {
    expect(OPERAND_REQUIRED_OPS).toEqual(["<", ">", ">=", "<=", "="]);
  });

  it("treats bare operand-requiring operators as incomplete", () => {
    for (const op of OPERAND_REQUIRED_OPS) expect(INCOMPLETE_OPS.test(op)).toBe(true);
    expect(INCOMPLETE_OPS.test("<>")).toBe(false);
    expect(INCOMPLETE_OPS.test(">10")).toBe(false);
  });
});
