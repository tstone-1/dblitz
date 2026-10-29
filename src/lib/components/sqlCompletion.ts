/**
 * Autocomplete for the SQL tab.
 *
 * Context-aware rather than one flat list: after FROM/JOIN it offers tables;
 * after `alias.` or `table.` it offers that table's columns; elsewhere it
 * offers the columns of the tables the statement already names first (with
 * their declared type), then functions with their signatures, keywords, and
 * the remaining columns. At the start of a statement it offers a few query
 * snippets.
 *
 * It suggests no write keywords (INSERT, UPDATE, DROP, ...): dblitz is a
 * read-only viewer and those statements are refused at Execute.
 *
 * Everything that decides WHAT to offer is a pure function over the statement
 * text, so it is tested without an editor; `sqlCompletionSource` only wires it
 * to CodeMirror. The scanning is deliberately lexical (regexes over the text
 * with string literals and comments masked out), not a SQL parser: it only has
 * to rank suggestions, and a wrong guess costs a less useful list, never a
 * wrong query.
 */

import {
  snippetCompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { SqlEngine } from "#lib/ipc.ts";
import functionLists from "./sqlFunctions.json";

export interface SqlSchema {
  /** table -> column names, in table order. */
  columns: Record<string, string[]>;
  /** table -> column -> declared type. */
  types: Record<string, Record<string, string>>;
  engine: SqlEngine;
}

export interface SqlFunctionDoc {
  name: string;
  args: string;
  doc: string;
}

/** The functions the editor suggests. Every name is checked against the real
 *  engine by `every_function_the_editor_suggests_exists_in_the_bundled_*`. */
export function functionsFor(engine: SqlEngine): SqlFunctionDoc[] {
  return functionLists[engine];
}

/** One identifier token: "double quoted", [bracketed], `backticked`, or bare. */
const IDENT = String.raw`(?:"(?:[^"]|"")+"|\[[^\]]+\]|` + "`[^`]+`" + String.raw`|[A-Za-z_][\w$]*)`;

/** Words that cannot be used as a bare identifier, so a name equal to one is
 *  inserted quoted. Not SQLite's full list: the ones a column is plausibly
 *  called (`order`, `group`, `key`, `values`, ...). */
const RESERVED = new Set(
  (
    "all and as asc between by case cast check collate create cross default delete desc " +
    "distinct drop else end except exists from full glob group having in index inner insert " +
    "intersect into is join key left like limit natural not null offset on or order outer " +
    "primary references right select set table then to union unique update using values when " +
    "where with"
  ).split(" "),
);

/** `name` as it must be written in SQL: bare when that is unambiguous,
 *  double-quoted (with `"` doubled) otherwise. */
export function quoteIdent(name: string): string {
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && !RESERVED.has(name.toLowerCase())) return name;
  return `"${name.replace(/"/g, '""')}"`;
}

/** The name a token refers to: quotes stripped, `""` undoubled. */
export function unquoteIdent(token: string): string {
  const first = token[0];
  if (first === '"') return token.slice(1, -1).replace(/""/g, '"');
  if (first === "[" || first === "`") return token.slice(1, -1);
  return token;
}

/**
 * `text` with string literals (quotes included) and comments replaced by
 * spaces, newlines kept, same length, so every offset still lines up.
 * Scanning the masked text keeps `'from users'` inside a literal from being
 * read as a FROM clause. Double quotes are identifiers in SQL and are left
 * alone.
 *
 * `literalFill` replaces literal characters instead of a space when a caller
 * must tell a literal from whitespace: trimming a statement with literals
 * masked as spaces cut `WHERE name = 'x'` down to `WHERE name =`.
 */
export function maskLiteralsAndComments(text: string, literalFill = " "): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === "'") {
      let j = i + 1;
      while (j < text.length) {
        if (text[j] === "'" && text[j + 1] === "'") j += 2;
        else if (text[j] === "'") break;
        else j++;
      }
      const end = Math.min(j + 1, text.length);
      out += text.slice(i, end).replace(/[^\n]/g, literalFill);
      i = end;
    } else if (c === "-" && text[i + 1] === "-") {
      const nl = text.indexOf("\n", i);
      const end = nl === -1 ? text.length : nl;
      out += " ".repeat(end - i);
      i = end;
    } else if (c === "/" && text[i + 1] === "*") {
      const close = text.indexOf("*/", i + 2);
      const end = close === -1 ? text.length : close + 2;
      out += text.slice(i, end).replace(/[^\n]/g, " ");
      i = end;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** The statement around `pos`: between the nearest `;` on either side that is
 *  not inside a literal or comment. */
export function statementBounds(text: string, pos: number): { from: number; to: number } {
  const masked = maskLiteralsAndComments(text);
  const before = masked.lastIndexOf(";", pos - 1);
  const after = masked.indexOf(";", pos);
  return { from: before + 1, to: after === -1 ? text.length : after };
}

/** Every statement in `text` that holds more than whitespace and comments, as
 *  ranges without the `;` and without surrounding whitespace or comments. */
export function statementRanges(text: string): { from: number; to: number }[] {
  const masked = maskLiteralsAndComments(text, "x");
  const out: { from: number; to: number }[] = [];
  let start = 0;
  for (let i = 0; i <= masked.length; i++) {
    if (i < masked.length && masked[i] !== ";") continue;
    const lead = masked.slice(start, i).search(/\S/);
    if (lead !== -1) {
      const body = masked.slice(start, i);
      out.push({ from: start + lead, to: start + body.trimEnd().length });
    }
    start = i + 1;
  }
  return out;
}

/**
 * The statement Execute runs when nothing is selected, or null when the text
 * holds at most one statement (then the whole text runs, exactly as before
 * this existed).
 *
 * The cursor inside a statement, or at its end, picks that statement. In the
 * gap between two, it picks the one that ends on the cursor's line (typing
 * `SELECT 1;` leaves the cursor after the `;`), otherwise the next one, and
 * after the last statement the last one.
 */
export function statementAtCursor(text: string, pos: number): { from: number; to: number } | null {
  const ranges = statementRanges(text);
  if (ranges.length <= 1) return null;
  const inside = ranges.find((r) => r.from <= pos && pos <= r.to);
  if (inside) return inside;
  const prevIndex = ranges.findLastIndex((r) => r.to < pos);
  const prev = ranges[prevIndex];
  const next = ranges[prevIndex + 1];
  if (prev && (!next || !text.slice(prev.to, pos).includes("\n"))) return prev;
  return next;
}

/** The known tables a statement names anywhere, in schema order. A lexical
 *  match: a column that shares a table's name also counts, which only boosts
 *  that table's columns. */
export function tablesInStatement(statement: string, tables: string[]): string[] {
  const masked = maskLiteralsAndComments(statement);
  const named = new Set<string>();
  for (const m of masked.matchAll(new RegExp(IDENT, "g"))) {
    named.add(unquoteIdent(m[0]).toLowerCase());
  }
  return tables.filter((t) => named.has(t.toLowerCase()));
}

const NOT_AN_ALIAS = new Set(
  (
    "where join left right inner outer full cross natural on using group order having limit " +
    "offset union except intersect window qualify as set"
  ).split(" "),
);

/**
 * Lower-cased qualifier -> table, for every table named after FROM, JOIN or a
 * comma in a FROM list, under its own name and under its alias
 * (`FROM users u`, `JOIN orders AS o`).
 */
export function qualifiersInStatement(statement: string, tables: string[]): Map<string, string> {
  const byLower = new Map(tables.map((t) => [t.toLowerCase(), t]));
  const out = new Map<string, string>();
  const masked = maskLiteralsAndComments(statement);
  const re = new RegExp(String.raw`(?:\bfrom\b|\bjoin\b|,)\s*(${IDENT})(?:\s+(?:as\s+)?(${IDENT}))?`, "gi");
  for (const m of masked.matchAll(re)) {
    const table = byLower.get(unquoteIdent(m[1]).toLowerCase());
    if (!table) continue;
    out.set(table.toLowerCase(), table);
    if (m[2]) {
      const alias = unquoteIdent(m[2]).toLowerCase();
      if (!NOT_AN_ALIAS.has(alias)) out.set(alias, table);
    }
  }
  // A table named anywhere still qualifies its own columns (`users.id` in a
  // subquery the regex above did not reach).
  for (const t of tablesInStatement(statement, tables)) out.set(t.toLowerCase(), t);
  return out;
}

export type CompletionKind = "start" | "table" | "column";

/** What the cursor is expecting, from the statement text before it (the word
 *  being typed excluded). */
export function completionKindAt(statementBefore: string): CompletionKind {
  const masked = maskLiteralsAndComments(statementBefore);
  if (!masked.trim()) return "start";
  const re =
    /\b(select|from|join|where|on|group|order|by|having|limit|offset|using|union|except|intersect|with|when|then|else|and|or|not|set|into|update|values|qualify|window|over|partition|describe|summarize)\b/gi;
  let last = "";
  for (const m of masked.matchAll(re)) last = m[1].toLowerCase();
  return last === "from" || last === "join" || last === "describe" || last === "summarize"
    ? "table"
    : "column";
}

const KEYWORDS_COMMON = [
  "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET",
  "JOIN", "LEFT JOIN", "INNER JOIN", "CROSS JOIN", "ON", "USING", "AS", "DISTINCT",
  "AND", "OR", "NOT", "IN", "IS NULL", "IS NOT NULL", "LIKE", "BETWEEN",
  "CASE", "WHEN", "THEN", "ELSE", "END", "ASC", "DESC", "NULLS FIRST", "NULLS LAST",
  "UNION", "UNION ALL", "EXCEPT", "INTERSECT", "WITH", "RECURSIVE", "EXISTS", "CAST",
  "OVER", "PARTITION BY",
];
const KEYWORDS_ENGINE: Record<SqlEngine, string[]> = {
  sqlite: ["GLOB", "COLLATE NOCASE", "EXPLAIN QUERY PLAN"],
  duckdb: ["ILIKE", "QUALIFY", "EXCLUDE", "GROUP BY ALL", "ORDER BY ALL", "TRY_CAST", "DESCRIBE", "SUMMARIZE"],
};
const START_KEYWORDS: Record<SqlEngine, string[]> = {
  sqlite: ["SELECT", "WITH", "EXPLAIN QUERY PLAN"],
  duckdb: ["SELECT", "WITH", "FROM", "DESCRIBE", "SUMMARIZE"],
};

function keywordOptions(words: string[], boost: number): Completion[] {
  return words.map((label) => ({ label, type: "keyword", boost }));
}

function snippetOptions(): Completion[] {
  return [
    snippetCompletion("SELECT * FROM ${table} LIMIT ${100}", {
      label: "SELECT * FROM … LIMIT 100",
      type: "text",
      detail: "snippet",
      boost: 6,
    }),
    snippetCompletion("SELECT COUNT(*) FROM ${table}", {
      label: "SELECT COUNT(*) FROM …",
      type: "text",
      detail: "snippet",
      boost: 5,
    }),
    snippetCompletion("SELECT ${column}, COUNT(*) AS n FROM ${table} GROUP BY 1 ORDER BY n DESC", {
      label: "SELECT …, COUNT(*) … GROUP BY",
      type: "text",
      detail: "value counts",
      boost: 4,
    }),
  ];
}

function tableOptions(schema: SqlSchema, boost: number): Completion[] {
  return Object.entries(schema.columns).map(([table, cols]) => ({
    label: table,
    type: "class",
    detail: `${cols.length} column${cols.length === 1 ? "" : "s"}`,
    apply: quoteIdent(table),
    boost,
  }));
}

/** One option per distinct column name across `tables`. `detail` carries the
 *  type when the name belongs to one table, the table names otherwise. */
function columnOptions(schema: SqlSchema, tables: string[], boost: number, skip?: Set<string>): Completion[] {
  const owners = new Map<string, string[]>();
  for (const t of tables) {
    for (const c of schema.columns[t] ?? []) {
      if (skip?.has(c)) continue;
      const list = owners.get(c);
      if (list) list.push(t);
      else owners.set(c, [t]);
    }
  }
  return [...owners].map(([col, ts]) => {
    const type = ts.length === 1 ? schema.types[ts[0]]?.[col] : undefined;
    return {
      label: col,
      type: "property",
      detail: type ? `${type} · ${ts[0]}` : ts.join(", "),
      apply: quoteIdent(col),
      boost,
    };
  });
}

function functionOptions(engine: SqlEngine, boost: number): Completion[] {
  return functionsFor(engine).map((f) =>
    snippetCompletion(f.args ? `${f.name}(\${})` : `${f.name}()`, {
      label: f.name,
      type: "function",
      detail: `(${f.args})`,
      info: f.doc,
      boost,
    }),
  );
}

/** The options for one completion request; the source below supplies the
 *  kind and the statement. */
export function buildOptions(kind: CompletionKind, schema: SqlSchema, statement: string): Completion[] {
  const engine = schema.engine;
  const tables = Object.keys(schema.columns);
  if (kind === "start") {
    return [...snippetOptions(), ...keywordOptions(START_KEYWORDS[engine], 2)];
  }
  const keywords = keywordOptions([...KEYWORDS_COMMON, ...KEYWORDS_ENGINE[engine]], -1);
  if (kind === "table") {
    return [...tableOptions(schema, 5), ...keywords];
  }
  const inScope = tablesInStatement(statement, tables);
  if (inScope.length === 0) {
    // Typing the SELECT list before the FROM clause: every column is a
    // candidate, and none is more likely than another.
    return [
      ...columnOptions(schema, tables, 1),
      ...functionOptions(engine, 0),
      ...tableOptions(schema, -1),
      ...keywords,
    ];
  }
  const scoped = columnOptions(schema, inScope, 3);
  const others = tables.filter((t) => !inScope.includes(t));
  return [
    ...scoped,
    ...functionOptions(engine, 0),
    ...keywords,
    ...tableOptions(schema, -2),
    ...columnOptions(schema, others, -3, new Set(scoped.map((o) => o.label))),
  ];
}

/** Columns of the table a qualifier (`users.` or the alias `u.`) refers to. */
export function qualifiedColumnOptions(schema: SqlSchema, statement: string, qualifier: string): Completion[] | null {
  const table = qualifiersInStatement(statement, Object.keys(schema.columns)).get(
    unquoteIdent(qualifier).toLowerCase(),
  );
  if (!table) return null;
  // Table order, not alphabetical: `boost` falls with the position.
  return (schema.columns[table] ?? []).map((col, i, all) => ({
    label: col,
    type: "property",
    detail: schema.types[table]?.[col] || undefined,
    apply: quoteIdent(col),
    boost: Math.max(-99, 99 - Math.round((i * 198) / Math.max(1, all.length))),
  }));
}

const QUALIFIED = new RegExp(String.raw`(${IDENT})\.([\w$]*)$`);

/** The CodeMirror completion source. `getSchema` is read on every request, so
 *  a schema change only has to replace what it returns. */
export function sqlCompletionSource(getSchema: () => SqlSchema) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const node = syntaxTree(ctx.state).resolveInner(ctx.pos, -1);
    if (/String|Comment|QuotedIdentifier/.test(node.name)) return null;

    const schema = getSchema();
    const doc = ctx.state.doc.toString();
    const bounds = statementBounds(doc, ctx.pos);
    const statement = doc.slice(bounds.from, bounds.to);
    const before = doc.slice(bounds.from, ctx.pos);

    const qualified = QUALIFIED.exec(before);
    if (qualified) {
      const options = qualifiedColumnOptions(schema, statement, qualified[1]);
      if (!options) return null;
      return { from: ctx.pos - qualified[2].length, options, validFor: /^[\w$]*$/ };
    }

    const word = ctx.matchBefore(/[\w$]+/);
    if (word && /^\d/.test(word.text)) return null; // typing a number
    if (!word && !ctx.explicit) return null;
    const from = word ? word.from : ctx.pos;
    const kind = completionKindAt(before.slice(0, before.length - (ctx.pos - from)));
    return { from, options: buildOptions(kind, schema, statement), validFor: /^[\w$]*$/ };
  };
}
