/**
 * The SQL tab's execute action, extracted from `ExecuteSQL.svelte` so its
 * publication rule is reachable by a test.
 *
 * The rule: `result = await executeSqlCmd(trimmed)` published unconditionally.
 * Start a slow query against database A, open database B while it runs, and the
 * backend's generation bump cancels A's statement -- which does not reject, it
 * returns the partial rows it had plus `error: "Query cancelled by a newer
 * request"`. Those rows then landed as B's result, complete with a history
 * entry for a query that never ran against B and column colours resolved
 * against B's tables.
 *
 * `createSessionOwnedRequest` is the same guard `DatabaseStructure.svelte` uses.
 * Two things are captured before the await -- a per-caller token and the
 * database-open generation -- and the result is published only if both still
 * hold. The executed SQL travels WITH the result rather than in a closure
 * variable, so the history entry can never be attributed to a different query.
 *
 * `running` is cleared unconditionally: it describes this component's button,
 * not the answer, and leaving it set would strand the Cancel button on screen.
 */

import type { SqlResult } from "$lib/ipc";
import type { SqlHistoryEntry } from "$lib/store.svelte";
import { createSessionOwnedRequest } from "./sessionOwnedRequest";

/** A completed run: the result plus the exact text that produced it. */
interface ExecutedQuery {
  sql: string;
  result: SqlResult;
}

export interface SqlExecutionDeps {
  /** Current database-open generation. Inject `() => appState.dbOpenGeneration`. */
  getGeneration: () => number;
  /** Raw editor text. */
  getSql: () => string;
  isRunning: () => boolean;
  setRunning: (running: boolean) => void;
  setResult: (result: SqlResult | null) => void;
  /** The query `result` came from -- colour resolution keys off this, not the
   *  editor text, which the user may have changed since. */
  setExecutedSql: (sql: string) => void;
  /** The IPC call. */
  executeSql: (sql: string) => Promise<SqlResult>;
  /** Append one entry to the persisted query history. */
  addHistoryEntry: (entry: SqlHistoryEntry) => void;
  /** Injectable clock so a test can pin the timestamp. */
  now?: () => number;
}

export function createSqlExecution(deps: SqlExecutionDeps): () => Promise<void> {
  const now = deps.now ?? (() => Date.now());

  const request = createSessionOwnedRequest<ExecutedQuery>({
    getGeneration: deps.getGeneration,
    publish: ({ sql, result }) => {
      deps.setResult(result);
      deps.setExecutedSql(sql);
      deps.addHistoryEntry({ sql, timestamp: now(), error: !!result.error });
    },
    // A thrown IPC failure is rendered as an error-shaped result so the SQL tab
    // shows it where every other error appears. Gated on ownership too: a
    // superseded request's failure is not the user's current problem.
    onError: (message) => {
      deps.setResult({
        columns: [],
        rows: [],
        column_types: [],
        error: message,
        truncated: false,
      });
    },
  });

  return async function executeSql(): Promise<void> {
    // Re-entrancy guard: the CodeMirror Ctrl+Enter keymap calls onexecute
    // unconditionally, so without this a second Ctrl+Enter (or Enter while the
    // button is disabled) would fire a concurrent invoke and a duplicate
    // history entry. Ctrl+Enter while running is simply ignored (not a cancel).
    if (deps.isRunning()) return;
    const trimmed = deps.getSql().trim();
    if (!trimmed) return;

    deps.setRunning(true);
    deps.setResult(null);
    try {
      await request(async () => ({
        sql: trimmed,
        result: await deps.executeSql(trimmed),
      }));
    } finally {
      deps.setRunning(false);
    }
  };
}
