<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { EditorView } from "@codemirror/view";
  import { EditorState, Compartment } from "@codemirror/state";
  import { appState } from "#lib/store.svelte.ts";
  import { validateSql } from "#lib/ipc.ts";
  import { buildEditorExtensions, buildTheme } from "./sqlEditorExtensions";
  import { runnableRange } from "./sqlLint";
  import type { SqlSchema } from "./sqlCompletion";
  import { requestRecheck, type CheckReport, type RunnableRange } from "./sqlLint";

  interface Props {
    value: string;
    /** What Execute would run: the selection, the statement at the cursor,
     *  or all of it (see runnableRange in sqlLint.ts). Called at mount and
     *  whenever it changes. */
    onrunnable?: (runnable: RunnableRange) => void;
    onchange?: (value: string) => void;
    onexecute?: () => void;
    /** Every verdict of the live check; see sqlLint.ts. */
    oncheck?: (report: CheckReport) => void;
    placeholder?: string;
  }

  let {
    value = $bindable(""),
    onrunnable = undefined,
    onchange = undefined,
    onexecute = undefined,
    oncheck = undefined,
    placeholder = "Enter SQL query...",
  }: Props = $props();

  let container: HTMLDivElement;
  let view: EditorView | undefined;
  let themeCompartment = new Compartment();
  let updating = false; // prevent feedback loop
  let last: RunnableRange | undefined; // last runnable reported

  // Plain (non-reactive) snapshot read through a getter by the completion
  // source and the linter, so a schema change only replaces this object -- no
  // extension is reconfigured, and the SQL language support is never rebuilt
  // (it depends on the dialect alone; see sqlEditorExtensions.ts).
  let schema: SqlSchema = { columns: {}, types: {}, engine: "sqlite" };

  onMount(() => {
    const state = EditorState.create({
      doc: value,
      extensions: buildEditorExtensions({
        dark: appState.theme === "dark",
        placeholder,
        themeCompartment,
        getSchema: () => schema,
        check: validateSql,
        getGeneration: () => appState.dbOpenGeneration,
        onCheck: (report) => oncheck?.(report),
        onexecute: () => onexecute?.(),
        onDocChanged: (newVal) => {
          if (updating) return;
          updating = true;
          value = newVal;
          onchange?.(newVal);
          updating = false;
        },
        onRunnableChanged: (next) => {
          if (next.text === last?.text && next.kind === last.kind) return;
          last = next;
          onrunnable?.(next);
        },
      }),
    });

    view = new EditorView({ state, parent: container });
    last = runnableRange(view.state);
    onrunnable?.(last);
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
    const dark = appState.theme === "dark";
    if (view) {
      view.dispatch({ effects: themeCompartment.reconfigure(buildTheme(dark)) });
    }
  });

  // A new database (or a reopen) changes what is valid: replace the schema
  // snapshot and re-check the current text against the new file at once,
  // rather than leaving the previous file's verdict on screen until the next
  // keystroke.
  $effect(() => {
    schema = {
      columns: appState.tableColumns,
      types: appState.tableColumnTypes,
      engine: appState.sqlEngine,
    };
    void appState.dbOpenGeneration;
    if (view) requestRecheck(view);
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
