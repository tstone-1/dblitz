import { describe, expect, it } from "vitest";
import { createSqlExecution } from "./sqlExecution";
import type { SqlResult } from "$lib/ipc";
import type { SqlHistoryEntry } from "$lib/store.svelte";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function result(overrides: Partial<SqlResult> = {}): SqlResult {
  return {
    columns: ["id"],
    rows: [["1"]],
    column_types: ["INTEGER"],
    error: null,
    truncated: false,
    ...overrides,
  };
}

function harness() {
  const state = {
    generation: 1,
    sql: "SELECT 1",
    running: false,
    result: null as SqlResult | null,
    executedSql: "",
  };
  const history: SqlHistoryEntry[] = [];
  const calls: string[] = [];
  let pending = deferred<SqlResult>();

  const execute = createSqlExecution({
    getGeneration: () => state.generation,
    getSql: () => state.sql,
    isRunning: () => state.running,
    setRunning: (running) => { state.running = running; },
    setResult: (r) => { state.result = r; },
    setExecutedSql: (s) => { state.executedSql = s; },
    executeSql: (sql) => {
      calls.push(sql);
      return pending.promise;
    },
    addHistoryEntry: (entry) => history.push(entry),
    now: () => 1000,
  });

  return {
    state,
    history,
    calls,
    execute,
    settle: (value: SqlResult) => pending.resolve(value),
    fail: (reason: unknown) => pending.reject(reason),
    arm: () => { pending = deferred<SqlResult>(); },
  };
}

describe("createSqlExecution", () => {
  it("publishes a result nothing supersedes", async () => {
    // Positive control: every assertion below is satisfied by a guard that
    // publishes nothing at all, so this has to exist first.
    const h = harness();
    const done = h.execute();
    h.settle(result());
    await done;

    expect(h.state.result?.columns).toEqual(["id"]);
    expect(h.state.executedSql).toBe("SELECT 1");
    expect(h.history).toEqual([{ sql: "SELECT 1", timestamp: 1000, error: false }]);
    expect(h.state.running).toBe(false);
  });

  it("drops a result whose database session ended, and writes no history", async () => {
    // The reported defect: a slow query on database A, database B opened while
    // it runs. The backend cancels A's statement and returns its partial rows
    // plus "Query cancelled by a newer request" -- which used to land as B's
    // result, with a history entry and colours resolved against B's tables.
    const h = harness();
    const done = h.execute();
    h.state.generation = 2; // another database was opened
    h.settle(result({ rows: [["stale"]], error: "Query cancelled by a newer request" }));
    await done;

    expect(h.state.result).toBeNull();
    expect(h.state.executedSql).toBe("");
    expect(h.history).toEqual([]);
  });

  it("clears `running` even when the answer is dropped", async () => {
    // Ownership decides publication, never the button state: leaving `running`
    // set would strand Cancel on screen with nothing to cancel.
    const h = harness();
    const done = h.execute();
    h.state.generation = 2;
    h.settle(result());
    await done;

    expect(h.state.running).toBe(false);
  });

  it("drops a superseded failure instead of replacing the live error", async () => {
    const h = harness();
    const done = h.execute();
    h.state.generation = 2;
    h.fail(new Error("boom"));
    await done;

    expect(h.state.result).toBeNull();
  });

  it("renders an owned failure as an error-shaped result", async () => {
    const h = harness();
    const done = h.execute();
    h.fail(new Error("no such table: nope"));
    await done;

    expect(h.state.result?.error).toContain("no such table: nope");
    expect(h.state.result?.rows).toEqual([]);
    expect(h.history).toEqual([]);
  });

  it("attributes the history entry to the query that ran, not the editor text", async () => {
    // The executed SQL travels with the result, so editing the editor while a
    // query is in flight cannot mislabel the history entry.
    const h = harness();
    const done = h.execute();
    h.state.sql = "DROP TABLE something";
    h.settle(result());
    await done;

    expect(h.history[0].sql).toBe("SELECT 1");
    expect(h.state.executedSql).toBe("SELECT 1");
  });

  it("ignores a re-entrant execute while one is running", async () => {
    const h = harness();
    const done = h.execute();
    await h.execute(); // Ctrl+Enter pressed again
    h.settle(result());
    await done;

    expect(h.calls).toEqual(["SELECT 1"]);
    expect(h.history).toHaveLength(1);
  });

  it("does nothing for blank editor text", async () => {
    const h = harness();
    h.state.sql = "   ";
    await h.execute();

    expect(h.calls).toEqual([]);
    expect(h.state.running).toBe(false);
  });
});
