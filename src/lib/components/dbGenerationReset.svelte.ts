/**
 * Per-component "a new database session was published" latch.
 *
 * Three components (BrowseData, DatabaseStructure, ExecuteSQL) each cache local
 * state that belongs to one backend connection and must be dropped when another
 * one is opened. Each of them used to hand-roll the same six lines plus the same
 * three paragraphs of rationale; this factory owns the bookkeeping and the
 * rationale once, and every consumer keeps only what is genuinely site-specific
 * (which local state its own reset clears).
 *
 * Usage from a Svelte component:
 *
 *     const checkDbReset = createDbGenerationReset({
 *       getGeneration: () => appState.dbOpenGeneration,
 *       onReset: () => resetForNewDatabase(),
 *     });
 *
 *     $effect(() => {
 *       checkDbReset();
 *     });
 *
 * Why it is built the way it is:
 *
 * * Gated on `dbOpenGeneration`, NOT `dbPath`. Reopening the ALREADY-open file
 *   (Toolbar / recents) reopens the backend connection and clears its caches
 *   without changing `dbPath`, so a path-only gate would keep serving the
 *   previous connection's stale state. The generation bumps on every successful
 *   open, same-path included. The close-to-null case is NOT covered here --
 *   `createAutoSelectFirstTable`'s `onReset` owns that transition.
 *
 * * The `$effect` stays in the calling component, and this module owns no
 *   effect of its own. That keeps the reactive read of `appState` inside the
 *   component's own reactive scope, lets a caller order the check against other
 *   work in the same effect (BrowseData must reset BEFORE its auto-select runs,
 *   or the reset clobbers the table the auto-select just picked), and leaves the
 *   factory testable with plain function calls.
 *
 * * The last-seen generation is a plain `let` in the closure, never `$state`.
 *   It is only read and written by `check()`, so making it reactive would only
 *   risk an effect depending on its own write -- the class of bug that produced
 *   `effect_update_depth_exceeded` in `autoSelectFirstTable.svelte.ts` (see the
 *   `resetFired` comment there).
 *
 * * It is seeded with 0, matching `appState.dbOpenGeneration`'s initial value,
 *   so a component mounted before any database is open does not fire a reset on
 *   its first check. A component mounted AFTER a database is already open (a
 *   lazily-rendered tab) DOES reset on its first check, because it has no way to
 *   know it was never showing the previous session. That is harmless -- a fresh
 *   component's per-database state is empty -- and it is the pre-existing
 *   behaviour of all three hand-rolled copies this replaces.
 */

export interface DbGenerationResetDeps {
  /** Current database-open generation. Inject `() => appState.dbOpenGeneration`. */
  getGeneration: () => number;
  /** Drop whatever the caller caches about the previous database session. */
  onReset: () => void;
}

/**
 * Returns a `check()` to call from the consumer's `$effect`. It fires `onReset`
 * only when the generation actually changed since the previous check, and
 * reports whether it just did so -- letting a caller sequence follow-up work on
 * the transition without repeating the comparison.
 */
export function createDbGenerationReset(
  deps: DbGenerationResetDeps,
): () => boolean {
  let lastSeenGen = 0;

  return function check(): boolean {
    const gen = deps.getGeneration();
    if (gen === lastSeenGen) return false;
    lastSeenGen = gen;
    deps.onReset();
    return true;
  };
}
