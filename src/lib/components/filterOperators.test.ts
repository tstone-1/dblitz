import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  hasIncompleteOperator,
  hasIncompleteSegment,
  INCOMPLETE_OPS,
  OPERAND_REQUIRED_OPS,
  stripIncompleteSegments,
} from "./filterOperators";

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

describe("hasIncompleteSegment", () => {
  it("flags a bare operand-requiring operator in a semicolon list", () => {
    expect(hasIncompleteSegment("foo;<")).toBe(true);
  });

  it("does not flag a completed operator segment", () => {
    expect(hasIncompleteSegment("foo;<5")).toBe(false);
  });

  it("does not flag <> (a complete, operand-optional operator)", () => {
    expect(hasIncompleteSegment("<>")).toBe(false);
  });

  it("does not flag a plain contains value", () => {
    expect(hasIncompleteSegment("hello")).toBe(false);
  });
});

describe("hasIncompleteOperator (regex-aware)", () => {
  it("flags a bare operator in a non-regex filter", () => {
    expect(hasIncompleteOperator("foo;<", false)).toBe(true);
  });

  it("never flags a regex-mode filter, even one that is a bare operator", () => {
    // In regex mode `<` is a legal pattern, not a half-typed operator.
    expect(hasIncompleteOperator("<", true)).toBe(false);
    expect(hasIncompleteOperator("foo;<", true)).toBe(false);
  });

  it("treats an all-whitespace value as inert", () => {
    expect(hasIncompleteOperator("   ", false)).toBe(false);
  });
});

describe("stripIncompleteSegments", () => {
  it("drops a bare operator segment but keeps the complete ones", () => {
    expect(stripIncompleteSegments("foo;<")).toBe("foo");
  });

  it("reduces an entirely-incomplete value to empty (then dropped downstream)", () => {
    expect(stripIncompleteSegments("<")).toBe("");
  });

  it("leaves a fully-complete operator list untouched", () => {
    expect(stripIncompleteSegments(">5;<10")).toBe(">5;<10");
  });

  it("keeps <> and plain contains segments", () => {
    expect(stripIncompleteSegments("<>;hello")).toBe("<>;hello");
  });
});
