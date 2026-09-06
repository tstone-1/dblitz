<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { EditorView } from "@codemirror/view";
  import { EditorState, Compartment } from "@codemirror/state";
  import type { Completion } from "@codemirror/autocomplete";
  import { appState } from "$lib/store.svelte";
  import {
    buildColumnCompletions,
    buildEditorExtensions,
    buildTheme,
  } from "./sqlEditorExtensions";

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
  let updating = false; // prevent feedback loop

  // Plain (non-reactive) cache read through a getter by the completion source,
  // so a schema change only rebuilds this array -- the extension itself never
  // has to be reconfigured, and the SQL language support is never rebuilt (it
  // depends on the dialect alone; see sqlEditorExtensions.ts).
  let cachedCompletions: Completion[] = [];

  onMount(() => {
    cachedCompletions = buildColumnCompletions(schema);
    const state = EditorState.create({
      doc: value,
      extensions: buildEditorExtensions({
        dark: appState.theme === "dark",
        placeholder,
        themeCompartment,
        getCompletions: () => cachedCompletions,
        onexecute: () => onexecute?.(),
        onDocChanged: (newVal) => {
          if (updating) return;
          updating = true;
          value = newVal;
          onchange?.(newVal);
          updating = false;
        },
      }),
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
    const dark = appState.theme === "dark";
    if (view) {
      view.dispatch({ effects: themeCompartment.reconfigure(buildTheme(dark)) });
    }
  });

  // Rebuild the column completion list when the schema changes. Nothing is
  // reconfigured: the completion source reads `cachedCompletions` through a
  // getter, so replacing the array is the whole update.
  $effect(() => {
    cachedCompletions = buildColumnCompletions(schema);
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
