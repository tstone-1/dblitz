import { describe, expect, it } from "vitest";
import {
  blockReasonFor,
  diagnosticFor,
  editDistance,
  missingName,
  problemRange,
  suggestNames,
  summarizeMessage,
} from "./sqlLint";
import type { SqlSchema } from "./sqlCompletion";

const schema: SqlSchema = {
  columns: { users: ["id", "name"], customer: ["id", "email"] },
  types: {},
  engine: "sqlite",
};

// Messages captured from the engines on 2026-10-29 (SQLite through
// rusqlite's SqlInputError `msg`, DuckDB through the C API).
const DUCK_COLUMN =
  'Binder Error: Referenced column "nosuch" not found in FROM clause!\nCandidate bindings: "name"\n\nLINE 1: SELECT nosuch FROM data\n               ^^^^^^';
const DUCK_TABLE =
  'Catalog Error: Table with name dta does not exist!\nDid you mean "data"?\n\nLINE 1: SELECT * FROM dta\n                      ^^^';
const DUCK_FUNCTION =
  'Catalog Error: Scalar Function with name lenght does not exist!\nDid you mean "length"?\n\nLINE 1: SELECT lenght(id) FROM data\n               ^^^^^^^^^^';
const DUCK_QUALIFIED =
  'Binder Error: Values list "d" does not have a column named "nosuch"\n\nLINE 1: SELECT d.nosuch FROM data d\n               ^^^^^^^^';
const DUCK_SYNTAX = 'Parser Error: syntax error at or near "SELEC"\n\nLINE 1: SELEC 1\n        ^^^^^';

describe("summarizeMessage", () => {
  it("drops DuckDB's LINE excerpt and keeps the rest", () => {
    expect(summarizeMessage(DUCK_COLUMN)).toBe(
      'Binder Error: Referenced column "nosuch" not found in FROM clause!\nCandidate bindings: "name"',
    );
    expect(summarizeMessage("no such column: x")).toBe("no such column: x");
  });
});

describe("missingName", () => {
  it.each([
    ["no such column: nmae", "column", "nmae"],
    ["no such column: u.nmae", "column", "nmae"],
    ["no such table: custmer", "table", "custmer"],
    ["no such table: main.custmer", "table", "custmer"],
    ["no such function: lenght", "function", "lenght"],
    [DUCK_COLUMN, "column", "nosuch"],
    [DUCK_QUALIFIED, "column", "nosuch"],
    [DUCK_TABLE, "table", "dta"],
    [DUCK_FUNCTION, "function", "lenght"],
  ])("%j", (message, kind, name) => {
    expect(missingName(message)).toEqual({ kind, name });
  });

  it("is null for an error that names nothing missing", () => {
    expect(missingName('near "SELEC": syntax error')).toBeNull();
    expect(missingName(DUCK_SYNTAX)).toBeNull();
  });
});

describe("editDistance and suggestNames", () => {
  it("counts an adjacent swap as one edit, ignoring case", () => {
    expect(editDistance("custmoer", "customer")).toBe(1);
    expect(editDistance("NAME", "name")).toBe(0);
    expect(editDistance("", "abc")).toBe(3);
  });

  it("suggests close names nearest first and leaves out far ones", () => {
    // "custom" is three edits from "custmer", over the limit of two.
    expect(suggestNames("custmer", ["customer", "users", "custom"])).toEqual(["customer"]);
    expect(suggestNames("usres", ["users", "uses", "orders"])).toEqual(["users", "uses"]);
    expect(suggestNames("nmae", ["name", "id", "email"])).toEqual(["name"]);
    expect(suggestNames("xyz", ["name", "id"])).toEqual([]);
  });

  it("never suggests the name itself", () => {
    expect(suggestNames("name", ["name", "Name"])).toEqual([]);
  });
});

describe("problemRange", () => {
  const slice = (text: string, r: { from: number; to: number }) => text.slice(r.from, r.to);

  it("underlines the token at the engine's offset", () => {
    const sql = "  SELEC name FROM users";
    expect(slice(sql, problemRange(sql, 'near "SELEC": syntax error', 2))).toBe("SELEC");
  });

  it("underlines the missing name, not the qualifier the offset points at", () => {
    const sql = "SELECT u.nmae FROM users u";
    expect(slice(sql, problemRange(sql, "no such column: u.nmae", 7))).toBe("nmae");
  });

  it("finds the named token when the engine gave no offset", () => {
    const sql = "SELECT * FROM dta";
    expect(slice(sql, problemRange(sql, DUCK_TABLE, null))).toBe("dta");
    expect(slice("SELEC 1", problemRange("SELEC 1", DUCK_SYNTAX, null))).toBe("SELEC");
  });

  it("underlines the last token of an incomplete statement", () => {
    const sql = "SELECT * FROM  ";
    expect(slice(sql, problemRange(sql, "incomplete input", sql.length))).toBe("FROM");
  });

  it("falls back to the whole statement", () => {
    const sql = "  DROP TABLE users ";
    expect(slice(sql, problemRange(sql, "dblitz is a read-only viewer - write statements ...", null))).toBe(
      "DROP TABLE users",
    );
  });
});

describe("diagnosticFor", () => {
  it("places the diagnostic at the document position and offers fixes", () => {
    const text = "SELECT * FROM custmer";
    const d = diagnosticFor(text, 100, { error: "no such table: custmer", offset: 14 }, schema);
    expect([d.from, d.to]).toEqual([114, 121]);
    expect(d.severity).toBe("error");
    expect(d.actions!.map((a) => a.name)).toEqual(["Replace with customer"]);
  });

  it("prefers the columns of the tables the statement names", () => {
    const d = diagnosticFor("SELECT emial FROM customer", 0, { error: "no such column: emial", offset: 7 }, schema);
    expect(d.actions!.map((a) => a.name)).toEqual(["Replace with email"]);
  });
});

describe("blockReasonFor", () => {
  const failed = { sql: "SELECT nmae FROM users", generation: 3, check: { error: "no such column: nmae", offset: 7 } };

  it("blocks only the exact text it checked, in the same database", () => {
    expect(blockReasonFor(failed, "  SELECT nmae FROM users\n", 3)).toBe("no such column: nmae");
    expect(blockReasonFor(failed, "SELECT name FROM users", 3)).toBeNull();
    expect(blockReasonFor(failed, "SELECT nmae FROM users", 4)).toBeNull();
  });

  it("never blocks without a failed verdict", () => {
    expect(blockReasonFor(null, "x", 3)).toBeNull();
    expect(blockReasonFor({ ...failed, check: null }, failed.sql, 3)).toBeNull();
    expect(blockReasonFor({ ...failed, check: { error: null, offset: null } }, failed.sql, 3)).toBeNull();
  });
});
