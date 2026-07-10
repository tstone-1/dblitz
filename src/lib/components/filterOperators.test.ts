import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { INCOMPLETE_OPS, OPERAND_REQUIRED_OPS } from "./filterOperators";

describe("filter operator metadata", () => {
  it("matches the backend operand-requiring operator set", () => {
    const backend = readFileSync("src-tauri/src/db/filters.rs", "utf8");
    const stringOps = [...backend.matchAll(/strip_prefix\("([^"]+)"\)/g)].map((m) => m[1]);
    const charOps = [...backend.matchAll(/strip_prefix\('([^']+)'\)/g)].map((m) => m[1]);
    const backendOps = [...stringOps, ...charOps]
      .filter((op) => op !== "<>")
      .sort();

    expect([...OPERAND_REQUIRED_OPS].sort()).toEqual(backendOps);
  });

  it("treats bare operand-requiring operators as incomplete", () => {
    for (const op of OPERAND_REQUIRED_OPS) expect(INCOMPLETE_OPS.test(op)).toBe(true);
    expect(INCOMPLETE_OPS.test("<>")).toBe(false);
    expect(INCOMPLETE_OPS.test(">10")).toBe(false);
  });

  it("ties INCOMPLETE_OPS itself to exactly filters.rs's operand-requiring prefixes", () => {
    // Parses filters.rs directly (independent of the OPERAND_REQUIRED_OPS
    // constant above) so this fails if INCOMPLETE_OPS's own construction ever
    // drifts from the backend's operand-requiring operator list.
    const backend = readFileSync("src-tauri/src/db/filters.rs", "utf8");
    const stringOps = [...backend.matchAll(/strip_prefix\("([^"]+)"\)/g)].map((m) => m[1]);
    const charOps = [...backend.matchAll(/strip_prefix\('([^']+)'\)/g)].map((m) => m[1]);
    const backendOps = [...stringOps, ...charOps]
      .filter((op) => op !== "<>")
      .sort();

    const incompleteOpsAlternatives = INCOMPLETE_OPS.source
      .replace(/^\^\(/, "")
      .replace(/\)\$$/, "")
      .split("|")
      .sort();

    expect(incompleteOpsAlternatives).toEqual(backendOps);
  });
});
