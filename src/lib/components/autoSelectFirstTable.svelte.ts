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
  let autoSelectedDb: string | null = null;

  return function check() {
    const path = appState.dbPath;
    if (!path) {
      autoSelectedDb = null;
      onReset?.();
      return;
    }
    if (appState.tables.length === 1 && autoSelectedDb !== path) {
      autoSelectedDb = path;
      onSelect(appState.tables[0].name);
    }
  };
}

/**
 * Pure decision helper for the "did the open database actually change?"
 * question that gates a per-database state reset (BrowseData used to
 * keep `selectedTable`/`columns`/`totalRows`/the row cache alive across a
 * Toolbar-driven `openDatabase()` call, so switching files left the grid
 * showing the previous database's rows). Extracted as a standalone pure
 * function - rather than baked into an effect - so the "open A -> open B
 * fires a reset, reopening the same path does not" contract is unit
 * testable without a component harness.
 *
 * A caller tracks `prevPath` itself (a plain, non-reactive `let`, mirroring
 * `autoSelectedDb` above) and calls this on every reactive check; a `true`
 * result means "reset now, and remember `nextPath` as the new baseline".
 */
export function didDbPathChange(
  prevPath: string | null,
  nextPath: string | null,
): boolean {
  return prevPath !== nextPath;
}
