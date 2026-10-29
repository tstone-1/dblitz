/**
 * The CodeMirror extension list `SqlEditor.svelte` mounts.
 *
 * It lives outside the component so the one thing that is genuinely easy to get
 * wrong -- keymap PRECEDENCE -- is reachable by a test. Before this file
 * existed, the execute bindings were appended to a single `keymap.of([
 * ...defaultKeymap, ..., {key: "Ctrl-Enter"}, {key: "Meta-Enter"}])`.
 * `defaultKeymap` already binds `Mod-Enter` to `insertBlankLine`
 * (`@codemirror/commands`), `Mod` normalises to Ctrl on Windows/Linux and to
 * Meta on macOS, and CodeMirror runs bindings in registration order with the
 * first handler returning `true` winning. So on Windows/Linux Ctrl+Enter
 * inserted a blank line instead of executing, on macOS Cmd+Enter did, and only
 * Control+Enter on a Mac reached the execute binding at all.
 *
 * `Prec.highest` is the fix: it puts the execute bindings ahead of every other
 * keymap in the facet regardless of the order the extensions are listed in.
 */

import {
  Decoration,
  EditorView,
  ViewPlugin,
  keymap,
  placeholder as phPlugin,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { Prec, type Extension, type Compartment } from "@codemirror/state";
import { sql, SQLite } from "@codemirror/lang-sql";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { searchKeymap } from "@codemirror/search";
import { syntaxHighlighting, HighlightStyle, bracketMatching } from "@codemirror/language";
import {
  acceptCompletion,
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
} from "@codemirror/autocomplete";
import { tags } from "@lezer/highlight";
import type { SqlCheck } from "#lib/ipc.ts";
import { sqlCompletionSource, type SqlSchema } from "./sqlCompletion";
import { runnableRange, sqlLinter, type CheckReport, type RunnableRange } from "./sqlLint";

/** Editor theme. Colors come from the app's CSS variables, so the only thing
 *  this needs to know is whether CodeMirror should treat itself as dark. */
export function buildTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      "&": {
        backgroundColor: "var(--bg-input)",
        color: "var(--text-primary)",
        fontSize: "13px",
        minHeight: "120px",
        maxHeight: "300px",
      },
      ".cm-content": {
        fontFamily: "'Cascadia Code', 'Fira Code', monospace",
        caretColor: "var(--text-primary)",
        minHeight: "112px",
      },
      ".cm-cursor": { borderLeftColor: "var(--text-primary)" },
      ".cm-gutters": {
        backgroundColor: "var(--bg-secondary)",
        color: "var(--text-muted)",
        borderRight: "1px solid var(--border-color)",
      },
      ".cm-activeLine": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 8 : 5}%, transparent)`,
      },
      ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 25 : 20}%, transparent) !important`,
      },
      ".cm-scroller": { overflow: "auto" },
      ".cm-tooltip": {
        backgroundColor: "var(--bg-secondary)",
        color: "var(--text-primary)",
        border: "1px solid var(--border-color)",
      },
      ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 30 : 22}%, transparent)`,
        color: "var(--text-primary)",
      },
      ".cm-completionDetail": { color: "var(--text-muted)", fontStyle: "normal", marginLeft: "1em" },
      ".cm-completionMatchedText": { textDecoration: "none", color: "var(--accent)", fontWeight: "600" },
      ".cm-completionInfo": { maxWidth: "320px", whiteSpace: "normal" },
      ".cm-diagnostic-error": { borderLeftColor: "var(--error)" },
      ".cm-diagnosticAction": { backgroundColor: "var(--accent)", color: "var(--bg-primary)" },
      ".cm-activeStatement": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 7 : 5}%, transparent)`,
      },
      "&.cm-focused .cm-matchingBracket": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 30 : 22}%, transparent)`,
        outline: "none",
      },
    },
    { dark },
  );
}

/** Syntax highlighting colors, driven by the app's CSS variables. */
export const highlightStyle = HighlightStyle.define([
  { tag: tags.keyword, color: "var(--accent)" },
  { tag: tags.string, color: "var(--success)" },
  { tag: tags.number, color: "var(--warning)" },
  { tag: tags.comment, color: "var(--text-muted)", fontStyle: "italic" },
  { tag: tags.operator, color: "var(--text-secondary)" },
  { tag: tags.typeName, color: "var(--accent)" },
  { tag: tags.function(tags.variableName), color: "var(--accent)" },
  { tag: tags.propertyName, color: "var(--text-primary)" },
]);

/**
 * The SQL language support.
 *
 * Built ONCE, at module load, and never rebuilt: completion is overridden by
 * `sqlCompletionSource` (see `sqlCompletion.ts`), which reads the schema
 * through a getter, so lang-sql's own schema-driven completion is switched off
 * and its `schema` option would have no effect. What lang-sql contributes is
 * the parser and highlighting, and those depend on the dialect alone. The
 * `SQLite` dialect is used for Parquet files too: DuckDB quotes strings and
 * identifiers the same way, and the editor only highlights with it.
 */
export const sqlLanguage: Extension = sql({
  dialect: SQLite,
  upperCaseKeywords: true,
});

/** The execute bindings, at the highest precedence available. Tab accepts
 *  an open completion and otherwise falls through (`acceptCompletion` returns
 *  false), so Tab still moves focus out of the editor when nothing is open. */
export function executeKeymap(onexecute: () => void): Extension {
  const run = () => {
    onexecute();
    return true;
  };
  return Prec.highest(
    keymap.of([
      { key: "Ctrl-Enter", run },
      { key: "Meta-Enter", run },
      { key: "Tab", run: acceptCompletion },
    ]),
  );
}

/** Everything CodeMirror binds by default, at normal precedence. */
export function baseKeymap(): Extension {
  return keymap.of([
    ...defaultKeymap,
    ...historyKeymap,
    ...closeBracketsKeymap,
    ...searchKeymap,
  ]);
}

const activeStatementMark = Decoration.mark({ class: "cm-activeStatement" });

function activeStatementDecorations(view: EditorView): DecorationSet {
  const r = runnableRange(view.state);
  return r.kind === "statement" && r.to > r.from
    ? Decoration.set([activeStatementMark.range(r.from, r.to)])
    : Decoration.none;
}

/**
 * Tints the statement Ctrl/Cmd+Enter will run when the editor holds several
 * and nothing is selected. Without it, running one statement out of many is a
 * surprise; with a single statement nothing is tinted, because all of it runs.
 */
export const activeStatementHighlight = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = activeStatementDecorations(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet) {
        this.decorations = activeStatementDecorations(update.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

export interface EditorExtensionOptions {
  dark: boolean;
  placeholder: string;
  /** Compartment the caller reconfigures when the app theme changes. */
  themeCompartment: Compartment;
  /** Live schema for completion and quick fixes; read on every request. */
  getSchema: () => SqlSchema;
  /** The backend's prepare-only check (`validate_sql`). */
  check: (sql: string) => Promise<SqlCheck | null>;
  /** The database-open generation, so a cached verdict never outlives its file. */
  getGeneration: () => number;
  /** Every verdict of the live check, for the Execute gate. */
  onCheck: (report: CheckReport) => void;
  onexecute: () => void;
  onDocChanged: (value: string) => void;
  /** What Execute would run now; reported on every edit and cursor move. */
  onRunnableChanged: (runnable: RunnableRange) => void;
  /** Live-check debounce in ms; tests shorten it. */
  lintDelay?: number;
}

/**
 * The complete extension list, in the order the editor mounts it.
 *
 * `executeKeymap` is listed first AND wrapped in `Prec.highest`. Either alone
 * would work today; both together mean a later reordering of this array cannot
 * silently hand Ctrl/Cmd+Enter back to `insertBlankLine`.
 */
export function buildEditorExtensions(o: EditorExtensionOptions): Extension[] {
  return [
    executeKeymap(o.onexecute),
    baseKeymap(),
    history(),
    o.themeCompartment.of(buildTheme(o.dark)),
    sqlLanguage,
    syntaxHighlighting(highlightStyle),
    closeBrackets(),
    bracketMatching(),
    autocompletion({ override: [sqlCompletionSource(o.getSchema)] }),
    sqlLinter({
      check: o.check,
      getSchema: o.getSchema,
      getGeneration: o.getGeneration,
      onCheck: o.onCheck,
      delay: o.lintDelay,
    }),
    activeStatementHighlight,
    phPlugin(o.placeholder),
    EditorView.lineWrapping,
    EditorView.updateListener.of((update) => {
      if (update.docChanged) o.onDocChanged(update.state.doc.toString());
      if (update.docChanged || update.selectionSet) o.onRunnableChanged(runnableRange(update.state));
    }),
  ];
}
