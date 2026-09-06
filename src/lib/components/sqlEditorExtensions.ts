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

import { EditorView, keymap, placeholder as phPlugin } from "@codemirror/view";
import { Prec, type Extension, type Compartment } from "@codemirror/state";
import { sql, SQLite } from "@codemirror/lang-sql";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { searchKeymap } from "@codemirror/search";
import { syntaxHighlighting, HighlightStyle } from "@codemirror/language";
import {
  autocompletion,
  closeBracketsKeymap,
  type CompletionContext,
  type Completion,
} from "@codemirror/autocomplete";
import { tags } from "@lezer/highlight";

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
 * Built ONCE, at module load, and never rebuilt: the editor overrides
 * completion with `columnCompleter` below, which disables lang-sql's own
 * schema-driven completion entirely, so the `schema` this used to be handed had
 * no effect on anything -- it was only ever reachable through the completion
 * source that is switched off. What lang-sql still contributes is highlighting,
 * and that depends on the dialect alone. dblitz ships SQLite, so the shipped
 * `SQLite` dialect is both more correct than the hand-rolled keyword list it
 * replaces (it knows SQLite's types, builtins and quoting rules) and free.
 */
export const sqlLanguage: Extension = sql({
  dialect: SQLite,
  upperCaseKeywords: true,
});

/** The execute bindings, at the highest precedence available. */
export function executeKeymap(onexecute: () => void): Extension {
  const run = () => {
    onexecute();
    return true;
  };
  return Prec.highest(
    keymap.of([
      { key: "Ctrl-Enter", run },
      { key: "Meta-Enter", run },
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

/**
 * Column-name completion.
 *
 * Deliberately a global source (not `table.` scoped): the point is to suggest
 * every column in the database from anywhere in the statement. It reads the
 * completion list through a getter so a schema change only has to rebuild the
 * array -- the extension itself never has to be replaced.
 */
export function columnCompletion(getCompletions: () => Completion[]): Extension {
  return autocompletion({
    override: [
      (ctx: CompletionContext) => {
        const word = ctx.matchBefore(/\w+/);
        if (!word || word.from === word.to) return null;
        return { from: word.from, options: getCompletions(), validFor: /^\w*$/ };
      },
    ],
  });
}

/** Build the completion list for a `{ table: [column, ...] }` schema map. */
export function buildColumnCompletions(
  schema: Record<string, string[]>,
): Completion[] {
  const seen = new Set<string>();
  const out: Completion[] = [];
  for (const [table, cols] of Object.entries(schema)) {
    for (const col of cols) {
      if (seen.has(col)) continue;
      seen.add(col);
      out.push({ label: col, type: "property", detail: table });
    }
  }
  return out;
}

export interface EditorExtensionOptions {
  dark: boolean;
  placeholder: string;
  /** Compartment the caller reconfigures when the app theme changes. */
  themeCompartment: Compartment;
  /** Live column completions; read on every completion request. */
  getCompletions: () => Completion[];
  onexecute: () => void;
  onDocChanged: (value: string) => void;
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
    columnCompletion(o.getCompletions),
    phPlugin(o.placeholder),
    EditorView.lineWrapping,
    EditorView.updateListener.of((update) => {
      if (update.docChanged) o.onDocChanged(update.state.doc.toString());
    }),
  ];
}
