import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  hasIncompleteOperator,
  hasIncompleteSegment,
  INCOMPLETE_OPS,
  OPERAND_REQUIRED_OPS,
  stripIncompleteSegments,
  filterAfterListPaste,
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

describe("filterAfterListPaste", () => {
  it("turns a pasted multi-line list into an alternation and switches to regex", () => {
    const pasted = "GAN111-650WSB\n  GAN041-650WSB\n  GAN039-650NTB\n  GAN039-650NBB";
    expect(filterAfterListPaste(undefined, 0, 0, pasted)).toEqual({
      value: "GAN111-650WSB|GAN041-650WSB|GAN039-650NTB|GAN039-650NBB",
      is_regex: true,
    });
  });

  it("handles CRLF, a trailing newline, blank lines and duplicates from a spreadsheet copy", () => {
    expect(filterAfterListPaste(undefined, 0, 0, "A\r\n\r\nB\r\nA\r\n")).toEqual({
      value: "A|B",
      is_regex: true,
    });
  });

  it("escapes regex metacharacters so each line matches literally", () => {
    const f = filterAfterListPaste(undefined, 0, 0, "a.b\n(c)|d\nx+y")!;
    expect(f.value).toBe("a\\.b|\\(c\\)\\|d|x\\+y");
    const re = new RegExp(f.value);
    expect(re.test("a.b")).toBe(true);
    expect(re.test("axb")).toBe(false);
    expect(re.test("(c)|d")).toBe(true);
    expect(re.test("d")).toBe(false);
    expect(filterAfterListPaste(undefined, 0, 0, "C:\\dir\n$5")!.value).toBe("C:\\\\dir|\\$5");
  });

  it("leaves a single line to the browser's own paste", () => {
    expect(filterAfterListPaste(undefined, 0, 0, "GAN111-650WSB")).toBeNull();
    expect(filterAfterListPaste(undefined, 0, 0, "GAN111-650WSB\r\n")).toBeNull();
    expect(filterAfterListPaste(undefined, 0, 0, "")).toBeNull();
  });

  it("replaces a text-mode filter, whose text would change meaning as a regex", () => {
    expect(filterAfterListPaste({ value: "foo;bar", is_regex: false }, 3, 3, "A\nB")).toEqual({
      value: "A|B",
      is_regex: true,
    });
  });

  it("replaces only the selection in a regex-mode filter", () => {
    expect(filterAfterListPaste({ value: "^(X)$", is_regex: true }, 2, 3, "A\nB")).toEqual({
      value: "^(A|B)$",
      is_regex: true,
    });
  });
});
