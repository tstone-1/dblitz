/**
 * The SQL tab's live error check.
 *
 * While the user types, the text that Execute would run (the selection, the
 * statement at the cursor, or the whole editor; see `runnableRange`) goes to
 * the backend's `validate_sql`, which applies every gate Execute applies and
 * prepares the statement without running it.
 * A problem is underlined where the engine says it is, with the engine's own
 * message, and a misspelt table, column or function name gets "Replace with
 * ..." fixes.
 *
 * The verdict is also reported to the caller (`onCheck`), which disables
 * Execute while the CURRENT text has a known error. The report carries the
 * exact text that was checked, so a verdict about text the user has since
 * changed can never block a run.
 *
 * Everything that turns a message into a range and a suggestion is a pure
 * function, tested on messages captured from SQLite and DuckDB.
 */

import { linter, type Action, type Diagnostic } from "@codemirror/lint";
import { StateEffect, type EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { SqlCheck } from "#lib/ipc.ts";
import {
  functionsFor,
  quoteIdent,
  statementAtCursor,
  tablesInStatement,
  type SqlSchema,
} from "./sqlCompletion";

/** What Execute runs: the selection, the statement at the cursor, or all. */
export type RunnableKind = "selection" | "statement" | "all";

export interface RunnableRange {
  text: string;
  from: number;
  to: number;
  kind: RunnableKind;
}

/** The text Execute runs and where it is: the main selection when it is not
 *  empty, else the statement at the cursor when the document holds several
 *  (`statementAtCursor`), else the whole document. */
export function runnableRange(state: EditorState): RunnableRange {
  const sel = state.selection.main;
  if (!sel.empty) {
    return { text: state.sliceDoc(sel.from, sel.to), from: sel.from, to: sel.to, kind: "selection" };
  }
  const doc = state.doc.toString();
  const stmt = statementAtCursor(doc, sel.head);
  if (stmt) return { text: doc.slice(stmt.from, stmt.to), ...stmt, kind: "statement" };
  return { text: doc, from: 0, to: doc.length, kind: "all" };
}

/** The first paragraph of an engine message. DuckDB appends a `LINE n:`
 *  excerpt with a marker, which the underline replaces. */
export function summarizeMessage(message: string): string {
  const cut = message.search(/\n\s*\nLINE \d+:/);
  return (cut === -1 ? message : message.slice(0, cut)).trim();
}

export type MissingKind = "table" | "column" | "function";

/** The name an "unknown name" error is about, from SQLite's or DuckDB's
 *  wording; null for every other error. */
export function missingName(message: string): { kind: MissingKind; name: string } | null {
  const patterns: [RegExp, MissingKind][] = [
    [/no such column: (?:\S+\.)?(\S+)/, "column"],
    [/no such table: (?:main\.)?(\S+)/, "table"],
    [/no such function: (\S+)/, "function"],
    [/Referenced column "([^"]+)" not found/, "column"],
    [/does not have a column named "([^"]+)"/, "column"],
    [/Table with name (\S+) does not exist/, "table"],
    [/Function with name (\S+) does not exist/, "function"],
  ];
  for (const [re, kind] of patterns) {
    const m = re.exec(message);
    if (m) return { kind, name: m[1].replace(/^"|"$/g, "") };
  }
  return null;
}

/** Optimal string alignment distance (Levenshtein plus adjacent swaps), case-
 *  insensitive: SQL names are, and `custmoer` is one swap from `customer`. */
export function editDistance(a: string, b: string): number {
  a = a.toLowerCase();
  b = b.toLowerCase();
  const d: number[][] = Array.from({ length: a.length + 1 }, (_, i) =>
    Array.from({ length: b.length + 1 }, (_, j) => (i === 0 ? j : j === 0 ? i : 0)),
  );
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) {
        d[i][j] = Math.min(d[i][j], d[i - 2][j - 2] + 1);
      }
    }
  }
  return d[a.length][b.length];
}

/** Up to `max` candidates close enough to `name` to be what was meant,
 *  nearest first. "Close enough" grows with the length: one edit for short
 *  names, a third of the length for long ones. */
export function suggestNames(name: string, candidates: Iterable<string>, max = 3): string[] {
  const limit = Math.max(1, Math.floor(name.length / 3));
  const seen = new Set<string>();
  const scored: [number, string][] = [];
  for (const c of candidates) {
    if (seen.has(c)) continue;
    seen.add(c);
    const d = editDistance(name, c);
    if (d > 0 && d <= limit) scored.push([d, c]);
  }
  scored.sort((x, y) => x[0] - y[0] || x[1].localeCompare(y[1]));
  return scored.slice(0, max).map(([, c]) => c);
}

const TOKEN = /"(?:[^"]|"")*"?|\[[^\]]*\]?|`[^`]*`?|[\w$]+|\S/y;

/** The token starting at `at` in `text`: an identifier (quoted or bare), a
 *  word, or else one character. */
function tokenAt(text: string, at: number): { from: number; to: number } {
  TOKEN.lastIndex = at;
  const m = TOKEN.exec(text);
  return { from: at, to: at + (m ? m[0].length : 1) };
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Where `name` occurs as a whole word in `text`, preferring the first
 *  occurrence at or after `from`. */
function findWord(text: string, name: string, from: number): { from: number; to: number } | null {
  const re = new RegExp(`(?<![\\w$])"?${escapeRegExp(name)}"?(?![\\w$])`, "gi");
  let first: { from: number; to: number } | null = null;
  for (const m of text.matchAll(re)) {
    const hit = { from: m.index, to: m.index + m[0].length };
    if (m.index >= from) return hit;
    first ??= hit;
  }
  return first;
}

/**
 * The range to underline for a problem in `text`, relative to `text`.
 *
 * With an engine offset, the token there; when the message names an unknown
 * name, the occurrence of that name at or after the offset (SQLite points a
 * qualified `u.nmae` at the `u`). Without an offset, the named token if it can
 * be found, else the whole statement. An offset at the very end (an
 * incomplete statement) underlines the last token.
 */
export function problemRange(
  text: string,
  message: string,
  offset: number | null,
): { from: number; to: number } {
  const missing = missingName(message);
  if (missing) {
    const hit = findWord(text, missing.name, offset ?? 0);
    if (hit) return hit;
  }
  if (offset !== null && offset < text.trimEnd().length) {
    return tokenAt(text, offset);
  }
  const named = /near "([^"]+)"|at or near "([^"]+)"/.exec(message);
  const near = named?.[1] ?? named?.[2];
  if (near) {
    const hit = findWord(text, near, 0);
    if (hit) return hit;
  }
  const trimmedEnd = text.trimEnd().length;
  if (offset !== null && trimmedEnd > 0) {
    const lastToken = /(?:"[^"]*"|[\w$]+|\S)\s*$/.exec(text.slice(0, trimmedEnd));
    return { from: lastToken ? lastToken.index : trimmedEnd - 1, to: trimmedEnd };
  }
  const lead = text.length - text.trimStart().length;
  return { from: lead, to: Math.max(lead + 1, trimmedEnd) };
}

/** Candidate names for a missing one: for a column, the columns of the tables
 *  the statement names come first. */
export function candidatesFor(kind: MissingKind, schema: SqlSchema, statement: string): string[] {
  const tables = Object.keys(schema.columns);
  if (kind === "table") return tables;
  if (kind === "function") return functionsFor(schema.engine).map((f) => f.name);
  const inScope = tablesInStatement(statement, tables);
  return [...inScope, ...tables].flatMap((t) => schema.columns[t] ?? []);
}

/** Build the diagnostic for one failed check of `text` (which starts at
 *  document position `base`). */
export function diagnosticFor(
  text: string,
  base: number,
  check: SqlCheck & { error: string },
  schema: SqlSchema,
): Diagnostic {
  const range = problemRange(text, check.error, check.offset);
  const missing = missingName(check.error);
  const actions: Action[] = missing
    ? suggestNames(missing.name, candidatesFor(missing.kind, schema, text)).map((name) => ({
        name: `Replace with ${name}`,
        apply(view: EditorView, from: number, to: number) {
          view.dispatch({ changes: { from, to, insert: quoteIdent(name) } });
        },
      }))
    : [];
  return {
    from: base + range.from,
    to: base + range.to,
    severity: "error",
    message: summarizeMessage(check.error),
    actions,
  };
}

export interface CheckReport {
  /** The trimmed text the verdict is about. */
  sql: string;
  /** The database-open generation the verdict is about. */
  generation: number;
  /** Null when there is no verdict (nothing open, or the SQL tab was busy). */
  check: SqlCheck | null;
}

/** The Execute gate: the check's message when `report` is a failed verdict
 *  about exactly `runnable` (trimmed) in database `generation`, else null. */
export function blockReasonFor(
  report: CheckReport | null,
  runnable: string,
  generation: number,
): string | null {
  if (!report?.check?.error) return null;
  if (report.generation !== generation || report.sql !== runnable.trim()) return null;
  return summarizeMessage(report.check.error);
}

export interface SqlLintOptions {
  check: (sql: string) => Promise<SqlCheck | null>;
  getSchema: () => SqlSchema;
  /** The database-open generation: part of the cache key, because the same
   *  text can be valid in one file and wrong in the next. */
  getGeneration: () => number;
  onCheck: (report: CheckReport) => void;
  /** Debounce, ms. */
  delay?: number;
}

/** Dispatched by `requestRecheck`; the linter treats it like an edit. */
const recheck = StateEffect.define<null>();

/** Re-check the current text, e.g. after another database was opened.
 *  (`forceLinting` would not do: it only hurries a check already scheduled.) */
export function requestRecheck(view: EditorView): void {
  view.dispatch({ effects: recheck.of(null) });
}

/**
 * The lint extension. It re-checks on every edit (after `delay`) and when the
 * selection changes what Execute would run. The last answer is cached by text,
 * so moving the cursor around costs no IPC; a failed IPC call is reported as
 * "no verdict", never as an error that blocks Execute.
 */
export function sqlLinter(o: SqlLintOptions) {
  let last: { text: string; generation: number; check: SqlCheck | null } | null = null;
  return linter(
    async (view) => {
      const { text, from } = runnableRange(view.state);
      const generation = o.getGeneration();
      if (!text.trim()) {
        o.onCheck({ sql: "", generation, check: null });
        return [];
      }
      let check: SqlCheck | null;
      if (last && last.text === text && last.generation === generation && last.check) {
        check = last.check;
      } else {
        check = await o.check(text).catch(() => null);
        // A null verdict is reused by nobody (see the condition above): the
        // next pass may get a real one.
        last = { text, generation, check };
      }
      o.onCheck({ sql: text.trim(), generation, check });
      if (!check?.error) return [];
      return [diagnosticFor(text, from, { ...check, error: check.error }, o.getSchema())];
    },
    {
      delay: o.delay ?? 400,
      needsRefresh: (update) =>
        (update.selectionSet && !update.docChanged) ||
        update.transactions.some((tr) => tr.effects.some((e) => e.is(recheck))),
    },
  );
}
