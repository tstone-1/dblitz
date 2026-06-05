import { describe, expect, it } from "vitest";
import { primaryTableFromSql, resolveResultColumnColors } from "./sqlTable";

describe("primaryTableFromSql", () => {
  it("reads a simple FROM table", () => {
    expect(primaryTableFromSql("SELECT * FROM db WHERE x = 1")).toBe("db");
  });

  it("is case-insensitive on the FROM keyword", () => {
    expect(primaryTableFromSql("select a, b from Orders")).toBe("Orders");
  });

  it("handles a trailing table with no WHERE", () => {
    expect(primaryTableFromSql("SELECT * FROM products")).toBe("products");
  });

  it("matches the real-world DISTINCT query", () => {
    const sql =
      "SELECT DISTINCT BusinessLine, PackageType FROM db " +
      "WHERE SalesItem_ProductState IN ('CQS', 'RFS') ORDER BY PackageType ASC LIMIT 20000";
    expect(primaryTableFromSql(sql)).toBe("db");
  });

  it("returns the table part of a schema-qualified name", () => {
    expect(primaryTableFromSql("SELECT * FROM main.records")).toBe("records");
  });

  it("unquotes a double-quoted identifier", () => {
    expect(primaryTableFromSql('SELECT * FROM "my table"')).toBe("my table");
  });

  it("unquotes a bracketed identifier", () => {
    expect(primaryTableFromSql("SELECT * FROM [weird name]")).toBe("weird name");
  });

  it("ignores the FROM inside a subquery and picks the outer table", () => {
    expect(
      primaryTableFromSql("SELECT * FROM outer WHERE id IN (SELECT id FROM inner)"),
    ).toBe("outer");
  });

  it("skips a CTE body FROM and returns the outer FROM", () => {
    const sql = "WITH c AS (SELECT * FROM cte_src) SELECT * FROM main_tbl";
    expect(primaryTableFromSql(sql)).toBe("main_tbl");
  });

  it("returns null when the FROM target is a subquery", () => {
    expect(primaryTableFromSql("SELECT * FROM (SELECT 1) AS t")).toBeNull();
  });

  it("returns null when there is no FROM", () => {
    expect(primaryTableFromSql("SELECT 1 + 1")).toBeNull();
  });

  it("does not treat a quoted column named from as the keyword", () => {
    expect(primaryTableFromSql('SELECT "from", x FROM ledger')).toBe("ledger");
  });

  it("does not treat FROM inside a string literal as the keyword", () => {
    expect(primaryTableFromSql("SELECT 'from here' AS note FROM journal")).toBe(
      "journal",
    );
  });

  it("ignores FROM inside comments", () => {
    expect(
      primaryTableFromSql("SELECT * /* from nope */ FROM real_tbl -- from also nope"),
    ).toBe("real_tbl");
  });

  it("does not treat a -- inside a string literal as a comment", () => {
    expect(primaryTableFromSql("SELECT '--' AS dashes FROM t")).toBe("t");
  });

  it("does not treat a /* */ inside a string literal as a comment", () => {
    expect(primaryTableFromSql("SELECT '/* not a comment */' AS x FROM ledger")).toBe(
      "ledger",
    );
  });

  it("skips a real comment sitting between FROM and the table name", () => {
    expect(primaryTableFromSql("SELECT * FROM /* the */ accounts")).toBe("accounts");
  });

  it("stops the identifier at an alias", () => {
    expect(primaryTableFromSql("SELECT * FROM sales s WHERE s.x > 0")).toBe("sales");
  });
});

describe("resolveResultColumnColors", () => {
  const colorsByTable: Record<string, Record<string, string>> = {
    db: { BusinessLine: "#f00", PackageType: "#0f0" },
  };
  const getColumnColors = (t: string) => colorsByTable[t] ?? {};

  it("colors only columns saved for the FROM table", () => {
    expect(
      resolveResultColumnColors({
        sql: "SELECT BusinessLine, MainArticleGroup FROM db",
        columns: ["BusinessLine", "MainArticleGroup"],
        tableNames: ["db"],
        getColumnColors,
      }),
    ).toEqual({ BusinessLine: "#f00" });
  });

  it("resolves the table name case-insensitively to its canonical config key", () => {
    expect(
      resolveResultColumnColors({
        sql: "SELECT PackageType FROM DB",
        columns: ["PackageType"],
        tableNames: ["db"],
        getColumnColors,
      }),
    ).toEqual({ PackageType: "#0f0" });
  });

  it("returns no colors when the query has no detectable FROM table", () => {
    expect(
      resolveResultColumnColors({
        sql: "SELECT 1 AS one",
        columns: ["one"],
        tableNames: ["db"],
        getColumnColors,
      }),
    ).toEqual({});
  });

  it("returns no colors when the FROM table has none saved", () => {
    expect(
      resolveResultColumnColors({
        sql: "SELECT a FROM other",
        columns: ["a"],
        tableNames: ["db", "other"],
        getColumnColors,
      }),
    ).toEqual({});
  });

  it("returns an empty map for an empty column list", () => {
    expect(
      resolveResultColumnColors({
        sql: "SELECT * FROM db",
        columns: [],
        tableNames: ["db"],
        getColumnColors,
      }),
    ).toEqual({});
  });
});
