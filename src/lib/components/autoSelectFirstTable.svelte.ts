/**
 * Single-table auto-select helper.
 *
 * When the user opens a database with exactly one table, both BrowseData and
 * DatabaseStructure should auto-select that lone table — there's no choice
 * to make, so the user shouldn't have to click. This helper encapsulates the
 * "track which db path was last auto-selected, fire when conditions met"
 * bookkeeping so each component doesn't reinvent the same effect.
 *
 * Usage from a Svelte component:
 *
 *     const checkAutoSelect = createAutoSelectFirstTable(
 *       (tableName) => {
 *         sidebarCollapsed = true;
 *         selectTable(tableName);
 *       },
 *       () => {
 *         // optional: clear local view state when the database closes
 *         selectedTable = null;
 *         columns = [];
 *       },
 *     );
 *
 *     $effect(() => {
 *       checkAutoSelect();
 *     });
 *
 * The `$effect` lives in the calling component so its reactive reads of
 * `appState.dbPath` and `appState.tables` are tracked by the component's
 * own reactive scope. The factory only owns the per-instance `autoSelectedDb`
 * flag (via closure) so different consumers don't race against a shared one.
 */

import { appState } from "$lib/store.svelte";

export function createAutoSelectFirstTable(
  onSelect: (tableName: string) => void,
  onReset?: () => void,
): () => void {
  // Keyed on dbOpenGeneration, NOT dbPath: reopening the already-open file
  // (Toolbar/recents) must re-auto-select the lone table after the reset that
  // reopen triggers, and dbPath doesn't change in that case. 0 is the "no db
  // opened yet" sentinel matching appState.dbOpenGeneration's initial value.
  let autoSelectedGen = 0;

  return function check() {
    if (!appState.dbPath) {
      autoSelectedGen = 0;
      onReset?.();
      return;
    }
    const gen = appState.dbOpenGeneration;
    if (appState.tables.length === 1 && autoSelectedGen !== gen) {
      autoSelectedGen = gen;
      onSelect(appState.tables[0].name);
    }
  };
}
