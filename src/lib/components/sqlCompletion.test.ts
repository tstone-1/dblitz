import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import {
  buildOptions,
  completionKindAt,
  functionsFor,
  maskLiteralsAndComments,
  qualifiedColumnOptions,
  qualifiersInStatement,
  quoteIdent,
  sqlCompletionSource,
  statementAtCursor,
  statementBounds,
  statementRanges,
  tablesInStatement,
  type SqlSchema,
} from "./sqlCompletion";
import { sqlLanguage } from "./sqlEditorExtensions";

const schema: SqlSchema = {
  columns: {
    users: ["id", "name", "created at"],
    orders: ["id", "user_id", "total"],
  },
  types: {
    users: { id: "INTEGER", name: "TEXT", "created at": "TEXT" },
    orders: { id: "INTEGER", user_id: "INTEGER", total: "REAL" },
  },
  engine: "sqlite",
};
const tables = Object.keys(schema.columns);

describe("maskLiteralsAndComments", () => {
  it("blanks literals and comments without moving any offset", () => {
    const sql = "SELECT 'from x' -- from y\nFROM /* from z */ users";
    const masked = maskLiteralsAndComments(sql);
    expect(masked.length).toBe(sql.length);
    expect(masked.match(/from/gi)).toEqual(["FROM"]);
    expect(masked.indexOf("users")).toBe(sql.indexOf("users"));
  });

  it("handles a doubled quote inside a literal and an unterminated literal", () => {
    expect(maskLiteralsAndComments("'it''s' x")).toBe("        x");
    expect(maskLiteralsAndComments("x 'open")).toBe("x      ");
  });
});

describe("statementBounds", () => {
  it("splits on semicolons outside literals", () => {
    const sql = "SELECT ';'; SELECT 2";
    const second = statementBounds(sql, sql.length);
    expect(sql.slice(second.from, second.to)).toBe(" SELECT 2");
    const first = statementBounds(sql, 3);
    expect(sql.slice(first.from, first.to)).toBe("SELECT ';'");
  });
});

describe("statementRanges", () => {
  it("skips blank and comment-only statements and trims each range", () => {
    const sql = "SELECT 1;  ; -- only a comment\n;\n  SELECT ';' ;";
    expect(statementRanges(sql).map((r) => sql.slice(r.from, r.to))).toEqual(["SELECT 1", "SELECT ';'"]);
  });
});

describe("statementAtCursor", () => {
  const pick = (docWithCursor: string) => {
    const pos = docWithCursor.indexOf("|");
    const doc = docWithCursor.replace("|", "");
    const r = statementAtCursor(doc, pos);
    return r && doc.slice(r.from, r.to);
  };

  it.each([
    ["SELECT 1; SELECT 2|", "SELECT 2"],
    ["SEL|ECT 1; SELECT 2", "SELECT 1"],
    ["|SELECT 1; SELECT 2", "SELECT 1"],
    ["SELECT 1;| SELECT 2", "SELECT 1"],
    ["SELECT 1;\n|\nSELECT 2", "SELECT 2"],
    ["SELECT 1;\nSELECT 2;\n|", "SELECT 2"],
    ["SELECT ';|'; SELECT 2", "SELECT ';'"],
    // A statement ending in a literal keeps it (it was once trimmed away).
    ["SELECT 1; SELECT * FROM t WHERE a = 'x'|", "SELECT * FROM t WHERE a = 'x'"],
  ])("%j -> %j", (doc, expected) => {
    expect(pick(doc)).toBe(expected);
  });

  it("is null for at most one statement, so the whole text runs", () => {
    expect(pick("SELECT 1;|")).toBeNull();
    expect(pick("SELECT 1; -- note|")).toBeNull();
    expect(pick("|")).toBeNull();
  });
});

describe("tablesInStatement", () => {
  it("finds tables case-insensitively and through quotes, not inside literals", () => {
    expect(tablesInStatement('SELECT * FROM "USERS"', tables)).toEqual(["users"]);
    expect(tablesInStatement("SELECT 'orders' FROM users", tables)).toEqual(["users"]);
    expect(tablesInStatement("SELECT 1", tables)).toEqual([]);
  });
});

describe("qualifiersInStatement", () => {
  it("maps table names and aliases, with and without AS", () => {
    const q = qualifiersInStatement("SELECT * FROM users u JOIN orders AS o ON o.user_id = u.id", tables);
    expect(q.get("u")).toBe("users");
    expect(q.get("o")).toBe("orders");
    expect(q.get("users")).toBe("users");
  });

  it("does not read a following keyword as an alias", () => {
    const q = qualifiersInStatement("SELECT * FROM users WHERE id = 1", tables);
    expect(q.has("where")).toBe(false);
    expect([...q.keys()]).toEqual(["users"]);
  });
});

describe("completionKindAt", () => {
  it.each([
    ["", "start"],
    ["  ", "start"],
    ["SELECT * FROM ", "table"],
    ["SELECT * FROM users JOIN ", "table"],
    ["SELECT * FROM users, ", "table"],
    ["SELECT * FROM users WHERE ", "column"],
    ["SELECT ", "column"],
    ["SELECT * FROM users WHERE name = 'from' AND ", "column"],
  ])("%j -> %s", (before, kind) => {
    expect(completionKindAt(before)).toBe(kind);
  });
});

describe("quoteIdent", () => {
  it("leaves plain names bare and quotes the rest", () => {
    expect(quoteIdent("user_id")).toBe("user_id");
    expect(quoteIdent("created at")).toBe('"created at"');
    expect(quoteIdent("order")).toBe('"order"');
    expect(quoteIdent('say "hi"')).toBe('"say ""hi"""');
    expect(quoteIdent("1st")).toBe('"1st"');
  });
});

describe("buildOptions", () => {
  const labels = (opts: { label: string }[]) => opts.map((o) => o.label);

  it("ranks the columns of the named tables above every other column", () => {
    const opts = buildOptions("column", schema, "SELECT  FROM users");
    const name = opts.find((o) => o.label === "name")!;
    expect(name.detail).toBe("TEXT · users");
    expect(name.boost).toBeGreaterThan(0);
    // `total` is also a SQLite function; this is the column.
    const total = opts.find((o) => o.label === "total" && o.type === "property")!;
    expect(total.detail).toBe("REAL · orders");
    expect(total.boost!).toBeLessThan(0);
    // `id` is in both tables but offered once, at the in-scope rank.
    expect(opts.filter((o) => o.label === "id")).toHaveLength(1);
    expect(opts.find((o) => o.label === "id")!.boost).toBe(name.boost);
  });

  it("inserts a column name that needs quoting quoted", () => {
    const opt = buildOptions("column", schema, "SELECT  FROM users").find((o) => o.label === "created at")!;
    expect(opt.apply).toBe('"created at"');
  });

  it("offers tables first after FROM", () => {
    const opts = buildOptions("table", schema, "SELECT * FROM ");
    expect(labels(opts).slice(0, 2)).toEqual(["users", "orders"]);
    expect(opts[0].detail).toBe("3 columns");
  });

  it("never suggests a write statement", () => {
    for (const kind of ["start", "table", "column"] as const) {
      for (const engine of ["sqlite", "duckdb"] as const) {
        const words = labels(buildOptions(kind, { ...schema, engine }, "SELECT  FROM users"));
        for (const w of ["INSERT", "UPDATE", "DELETE", "DROP", "CREATE", "ALTER", "ATTACH"]) {
          expect(words).not.toContain(w);
        }
      }
    }
  });

  it("offers each engine's own functions", () => {
    const sqlite = labels(buildOptions("column", schema, "SELECT "));
    const duckdb = labels(buildOptions("column", { ...schema, engine: "duckdb" }, "SELECT "));
    expect(sqlite).toContain("strftime");
    expect(duckdb).toContain("median");
    // Not in dblitz's SQLite build (no SQLITE_ENABLE_MATH_FUNCTIONS).
    expect(sqlite).not.toContain("sqrt");
    expect(functionsFor("sqlite").every((f) => f.doc.length > 0)).toBe(true);
  });

  it("starts a statement with snippets and starting keywords only", () => {
    const opts = labels(buildOptions("start", schema, ""));
    expect(opts[0]).toBe("SELECT * FROM … LIMIT 100");
    expect(opts).toContain("SELECT");
    expect(opts).not.toContain("users");
    expect(opts).not.toContain("WHERE");
  });
});

describe("qualifiedColumnOptions", () => {
  it("resolves an alias to its table's columns, in table order", () => {
    const opts = qualifiedColumnOptions(schema, "SELECT u. FROM users u", "u")!;
    expect(opts.map((o) => o.label)).toEqual(["id", "name", "created at"]);
    expect(opts[0].boost!).toBeGreaterThan(opts[2].boost!);
    expect(opts[1].detail).toBe("TEXT");
  });

  it("returns null for a qualifier that names no table", () => {
    expect(qualifiedColumnOptions(schema, "SELECT x. FROM users u", "x")).toBeNull();
  });
});

describe("sqlCompletionSource", () => {
  // The real source over a real EditorState with the shipped language, so the
  // syntax-tree checks (inside a string, a comment) run against lang-sql's
  // actual node names rather than an assumption about them.
  function complete(docWithCursor: string, explicit = false): CompletionResult | null {
    const pos = docWithCursor.indexOf("|");
    const doc = docWithCursor.replace("|", "");
    const state = EditorState.create({ doc, extensions: [sqlLanguage] });
    const source = sqlCompletionSource(() => schema);
    return source(new CompletionContext(state, pos, explicit)) as CompletionResult | null;
  }

  it("offers tables after FROM", () => {
    const r = complete("SELECT * FROM us|")!;
    expect(r.options[0].label).toBe("users");
    expect(r.from).toBe("SELECT * FROM ".length);
  });

  it("offers an alias's columns after the dot", () => {
    const r = complete("SELECT o.|  FROM orders o")!;
    expect(r.options.map((o) => o.label)).toEqual(["id", "user_id", "total"]);
    expect(r.from).toBe("SELECT o.".length);
  });

  it("stays quiet inside a string literal and a comment", () => {
    expect(complete("SELECT 'us|'")).toBeNull();
    expect(complete("SELECT 1 -- us|")).toBeNull();
  });

  it("stays quiet while a number is typed, and with no word unless asked", () => {
    expect(complete("SELECT 12|")).toBeNull();
    expect(complete("SELECT |")).toBeNull();
    expect(complete("SELECT |", true)!.options.length).toBeGreaterThan(0);
  });
});
