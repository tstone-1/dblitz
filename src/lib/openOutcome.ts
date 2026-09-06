/**
 * Did an `openDatabase(path)` call actually open that database?
 *
 * `store.openDatabase` never throws -- it catches and reports through
 * `appState.error` -- so awaiting it tells the caller nothing. `+page.svelte`
 * switched to the Browse tab regardless, which on a failed open dropped the
 * user on an empty Browse panel reading "Open a SQLite database to browse
 * data." with the real error banner above it. That reads as the app having lost
 * the file it just refused to open.
 *
 * Both conditions are needed. `error === null` alone is satisfied by a
 * SUPERSEDED open (a second `openDatabase` started while the first was in
 * flight), which returns without publishing anything and without setting an
 * error. `dbPath === path` alone is satisfied by a failed reopen of the file
 * that was already open, where `dbPath` never changed.
 */
export function openSucceeded(args: {
  requestedPath: string;
  dbPath: string | null;
  error: string | null;
}): boolean {
  return args.error === null && args.dbPath === args.requestedPath;
}
