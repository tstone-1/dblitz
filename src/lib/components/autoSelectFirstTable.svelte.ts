import { appState } from "#lib/store.svelte.ts";

/** Select the sole table once per native database session, including same-path reopens. */
export function createAutoSelectFirstTable(onSelect: (name: string) => void): () => void {
  let selectedGeneration = 0;
  return () => {
    const generation = appState.dbOpenGeneration;
    if (!appState.dbPath || appState.tables.length !== 1 || generation === selectedGeneration) return;
    selectedGeneration = generation;
    onSelect(appState.tables[0].name);
  };
}
