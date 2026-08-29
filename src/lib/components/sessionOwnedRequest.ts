/**
 * "Is this answer still the one to publish?" for a component-local async read.
 *
 * Two races are lost by an `await` whose result is assigned unconditionally,
 * and both are reachable by clicking:
 *
 *   * **Superseded request.** Click table A, then table B while A's column
 *     request is still in flight. Whichever response arrives LAST wins, so a
 *     slow A landing after B leaves the panel showing B selected with A's
 *     columns - one table labelled with another table's schema.
 *   * **Superseded session.** Start a request against database A, then open
 *     database B. `createDbGenerationReset` clears the panel, and A's response
 *     then repopulates it from a connection that no longer exists.
 *
 * Two facts settle both, and both must be captured BEFORE the await: a
 * monotonic per-caller token (is this still the newest request of its kind?)
 * and the database-open generation (does this answer belong to the open
 * session?). Checking only the token republishes a dead session's data;
 * checking only the generation lets two requests inside one session finish out
 * of order.
 *
 * Scope, deliberately: this is a publication guard, not a request bus. It has
 * no registry, cancels nothing, and knows nothing about IPC - the in-flight
 * call still runs to completion, its answer is simply dropped. Each caller
 * creates its own, so two independent reads in one component (raw schema and
 * table columns) never supersede each other. `store.svelte.ts` and
 * `virtualRows.svelte.ts` already enforce the same rule inline; this exists
 * because the third and fourth copies would have been hand-rolled in a
 * component, where they are unreachable by a test.
 */

export interface SessionOwnedRequestDeps<T> {
  /** Current database-open generation. Inject `() => appState.dbOpenGeneration`. */
  getGeneration: () => number;
  /** Assign the result. Called only when this request still owns publication. */
  publish: (value: T) => void;
  /**
   * Report a failure. Also gated on ownership: a superseded request's error is
   * not the user's current problem, and surfacing it would replace a live error
   * message with an abandoned one.
   */
  onError: (message: string) => void;
}

/**
 * Returns `request(run)`: awaits `run()` and publishes its result only if
 * nothing has superseded it. Resolves either way, so a caller can `await` it
 * without having to know whether the answer was used.
 */
export function createSessionOwnedRequest<T>(
  deps: SessionOwnedRequestDeps<T>,
): (run: () => Promise<T>) => Promise<void> {
  // A plain closure variable, never `$state`: it is read and written only here,
  // so making it reactive would risk an effect depending on its own write - the
  // shape that produced `effect_update_depth_exceeded` elsewhere in this app.
  let latest = 0;

  return async function request(run: () => Promise<T>): Promise<void> {
    const token = ++latest;
    const generation = deps.getGeneration();
    const owns = () => token === latest && generation === deps.getGeneration();
    try {
      const value = await run();
      if (owns()) deps.publish(value);
    } catch (e) {
      if (owns()) deps.onError(String(e));
    }
  };
}
