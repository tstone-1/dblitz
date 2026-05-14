<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { EditorView, keymap, placeholder as phPlugin } from "@codemirror/view";
  import { EditorState, Compartment } from "@codemirror/state";
  import { sql, SQLDialect, type SQLConfig } from "@codemirror/lang-sql";
  import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
  import { searchKeymap } from "@codemirror/search";
  import { syntaxHighlighting, HighlightStyle } from "@codemirror/language";
  import { autocompletion, closeBracketsKeymap, type CompletionContext, type Completion } from "@codemirror/autocomplete";
  import { tags } from "@lezer/highlight";
  import { appState } from "$lib/store.svelte";

  interface Props {
    value: string;
    onchange?: (value: string) => void;
    onexecute?: () => void;
    placeholder?: string;
    schema?: Record<string, string[]>; // table -> column names
  }

  let {
    value = $bindable(""),
    onchange = undefined,
    onexecute = undefined,
    placeholder = "Enter SQL query...",
    schema = {},
  }: Props = $props();

  let container: HTMLDivElement;
  let view: EditorView | undefined;
  let themeCompartment = new Compartment();
  let sqlCompartment = new Compartment();
  let updating = false; // prevent feedback loop

  function buildTheme(dark: boolean) {
    return EditorView.theme({
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
      ".cm-activeLine": { backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 8 : 5}%, transparent)` },
      ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
        backgroundColor: `color-mix(in srgb, var(--accent) ${dark ? 25 : 20}%, transparent) !important`,
      },
      ".cm-scroller": { overflow: "auto" },
    }, { dark });
  }

  // Syntax highlighting colors using CSS variables
  const highlightStyle = HighlightStyle.define([
    { tag: tags.keyword, color: "var(--accent)" },
    { tag: tags.string, color: "var(--success)" },
    { tag: tags.number, color: "var(--warning)" },
    { tag: tags.comment, color: "var(--text-muted)", fontStyle: "italic" },
    { tag: tags.operator, color: "var(--text-secondary)" },
    { tag: tags.typeName, color: "var(--accent)" },
    { tag: tags.function(tags.variableName), color: "var(--accent)" },
    { tag: tags.propertyName, color: "var(--text-primary)" },
  ]);

  function getThemeExt() {
    return buildTheme(appState.theme === "dark");
  }

  function getSqlExt() {
    const config: SQLConfig = {
      dialect: SQLDialect.define({ keywords: [
        "select", "from", "where", "order", "by", "group", "having", "limit", "offset",
        "join", "left", "right", "inner", "outer", "cross", "on", "as", "and", "or", "not",
        "in", "is", "null", "like", "between", "exists", "case", "when", "then", "else", "end",
        "distinct", "union", "all", "intersect", "except",
        "count", "sum", "avg", "min", "max", "cast", "coalesce", "ifnull", "typeof",
        "insert", "into", "values", "update", "set", "delete",
        "create", "table", "drop", "alter", "index", "view", "trigger",
        "primary", "key", "foreign", "references", "unique", "check", "default",
        "begin", "commit", "rollback", "pragma", "explain", "analyze", "vacuum", "reindex",
        "asc", "desc", "with", "recursive", "glob", "regexp", "escape",
      ].join(" ") }),
      schema,
      upperCaseKeywords: true,
    };
    return sql(config);
  }

  // Custom completion: suggest all column names globally (not just after table.)
  let completionCompartment = new Compartment();
  let cachedCompletions: Completion[] = [];

  function rebuildColumnCompletions() {
    const seen = new Set<string>();
    cachedCompletions = [];
    for (const [table, cols] of Object.entries(schema)) {
      for (const col of cols) {
        if (!seen.has(col)) {
          seen.add(col);
          cachedCompletions.push({ label: col, type: "property", detail: table });
        }
      }
    }
  }

  function columnCompleter(ctx: CompletionContext) {
    const word = ctx.matchBefore(/\w+/);
    if (!word || word.from === word.to) return null;
    return {
      from: word.from,
      options: cachedCompletions,
      validFor: /^\w*$/,
    };
  }

  onMount(() => {
    rebuildColumnCompletions();
    const state = EditorState.create({
      doc: value,
      extensions: [
        keymap.of([
          ...defaultKeymap,
          ...historyKeymap,
          ...closeBracketsKeymap,
          ...searchKeymap,
          { key: "Ctrl-Enter", run: () => { onexecute?.(); return true; } },
          { key: "Meta-Enter", run: () => { onexecute?.(); return true; } },
        ]),
        history(),
        themeCompartment.of(getThemeExt()),
        sqlCompartment.of(getSqlExt()),
        syntaxHighlighting(highlightStyle),
        completionCompartment.of(autocompletion({ override: [columnCompleter] })),
        phPlugin(placeholder),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged && !updating) {
            const newVal = update.state.doc.toString();
            updating = true;
            value = newVal;
            onchange?.(newVal);
            updating = false;
          }
        }),
      ],
    });

    view = new EditorView({ state, parent: container });
  });

  // Sync external value changes into editor
  $effect(() => {
    if (view && !updating) {
      const current = view.state.doc.toString();
      if (value !== current) {
        updating = true;
        view.dispatch({
          changes: { from: 0, to: current.length, insert: value },
        });
        updating = false;
      }
    }
  });

  // Switch theme when app theme changes
  $effect(() => {
    void appState.theme;
    if (view) {
      view.dispatch({ effects: themeCompartment.reconfigure(getThemeExt()) });
    }
  });

  // Update SQL schema + column completions when schema changes
  $effect(() => {
    void schema;
    rebuildColumnCompletions();
    if (view) {
      view.dispatch({ effects: [
        sqlCompartment.reconfigure(getSqlExt()),
        completionCompartment.reconfigure(autocompletion({ override: [columnCompleter] })),
      ] });
    }
  });

  onDestroy(() => {
    view?.destroy();
  });
</script>

<div class="sql-editor" bind:this={container}></div>

<style>
  .sql-editor {
    border-bottom: 1px solid var(--border-color);
  }
  .sql-editor :global(.cm-editor) {
    outline: none;
  }
  .sql-editor :global(.cm-focused) {
    outline: none;
  }
</style>
