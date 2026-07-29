import { describe, expect, it } from "vitest";
import { createVirtualRows } from "./virtualRows.svelte";
import type { QueryResult } from "$lib/ipc";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("createVirtualRows", () => {
  it("dedupes in-flight chunk loads and projects visible columns", async () => {
    const loads: Array<{ offset: number; limit: number }> = [];
    let columns: string[] = ["id", "name", "status"];
    let totalRows = 0;
    const visibleColumns = ["name"];
    const firstLoad = deferred<QueryResult>();

    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: (offset, limit) => {
        loads.push({ offset, limit });
        return firstLoad.promise;
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => visibleColumns,
      getColumnIndex: (column) => columns.indexOf(column),
      hasColumns: () => columns.length > 0,
      setColumns: (nextColumns) => { columns = nextColumns; },
      setTotalRows: (nextTotalRows) => { totalRows = nextTotalRows; },
      setError: () => {},
    });

    expect(rows.getVisibleRow(1)).toBeNull();
    expect(rows.getVisibleRow(0)).toBeNull();
    expect(loads).toEqual([{ offset: 0, limit: 2 }]);

    firstLoad.resolve({
      columns,
      rows: [
        ["1", "alpha", "active"],
        ["2", "bravo", "archived"],
      ],
      total_rows: 2,
      offset: 0,
    });
    await loadsSettled();

    expect(totalRows).toBe(2);
    expect(rows.getVisibleRow(1)).toEqual(["bravo"]);
  });

  it("materializes visible row ranges across chunks", async () => {
    const columns = ["id", "name"];
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset, limit) => ({
        columns,
        rows: Array.from({ length: limit }, (_, idx) => {
          const id = offset + idx;
          return [String(id), `item-${id}`];
        }),
        total_rows: 10,
        offset,
      }),
      cancelQueries: async () => {},
      getVisibleColumns: () => ["name"],
      getColumnIndex: (column) => columns.indexOf(column),
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    await expect(rows.getVisibleRows(1, 3)).resolves.toEqual([
      ["item-1"],
      ["item-2"],
      ["item-3"],
    ]);
  });

  it("rejects stale materialization after a newer reload starts", async () => {
    const pending = deferred<QueryResult>();
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: () => pending.promise,
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    const materialized = rows.getVisibleRows(0, 0);
    await rows.beginReload();
    pending.resolve({
      columns: ["id"],
      rows: [["1"]],
      total_rows: 1,
      offset: 0,
    });

    await expect(materialized).rejects.toThrow("Selection changed");
  });

  it("reports background chunk errors without rethrowing unhandled rejections", async () => {
    const errors: string[] = [];
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async () => {
        throw new Error("load failed");
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: (message) => errors.push(message),
    });

    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(errors[0]).toContain("load failed");
  });

  it("does not surface a cancellation by a newer request as an error", async () => {
    // cancel_queries bumps a backend generation shared by the browse and SQL
    // tabs, so cancelling a SQL query rejects an in-flight browse chunk fetch
    // whose epoch is still current. That must not raise the error bar.
    const errors: string[] = [];
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async () => {
        // Shaped like the real IPC rejection: err_ctx prefixes the query
        // context onto the backend message.
        throw new Error('querying table "items": Query cancelled by a newer request');
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: (message) => errors.push(message),
    });

    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(errors).toEqual([]);
  });

  // chunkSize: 1 makes chunk index == row index, so the cap/eviction math
  // below is easy to reason about: chunk N is row N.
  function makeChunkCountingRows() {
    let loadCount = 0;
    const rows = createVirtualRows({
      chunkSize: 1,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        loadCount++;
        return { columns: ["id"], rows: [[String(offset)]], total_rows: null, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });
    return { rows, loadCountRef: () => loadCount };
  }

  const MAX_CACHED_CHUNKS = 200; // must match the constant in virtualRows.svelte.ts

  it("materializes ranges wider than the viewport cache", async () => {
    const { rows } = makeChunkCountingRows();

    const materialized = await rows.getVisibleRows(0, MAX_CACHED_CHUNKS);

    expect(materialized).toHaveLength(MAX_CACHED_CHUNKS + 1);
    expect(materialized[0]).toEqual(["0"]);
    expect(materialized.at(-1)).toEqual([String(MAX_CACHED_CHUNKS)]);
  });

  it("assembles a cache-sized range whose loads evict in-range chunks mid-flight", async () => {
    const { rows } = makeChunkCountingRows();

    // Chunks 0..99 are in-range AND the least-recently-used entries; chunks
    // 200..299 top the cache up to exactly the cap, so the first load below
    // pushes it over and eviction starts at chunk 0 and walks forward.
    for (let i = 0; i < 100; i++) await rows.getVisibleRows(i, i);
    for (let i = 200; i < 300; i++) await rows.getVisibleRows(i, i);

    // Exactly MAX_CACHED_CHUNKS wide, so this is NOT the wider-than-the-cache
    // case: chunks 100..199 have to load, and each load evicts one of the
    // already-cached 0..99 before assembly would have read it back. Reading
    // the shared cache back at assembly time therefore failed with
    // "Selection contains rows that could not be loaded." (W2).
    const materialized = await rows.getVisibleRows(0, MAX_CACHED_CHUNKS - 1);

    expect(materialized).toHaveLength(MAX_CACHED_CHUNKS);
    expect(materialized[0]).toEqual(["0"]);
    expect(materialized[99]).toEqual(["99"]);
    expect(materialized.at(-1)).toEqual([String(MAX_CACHED_CHUNKS - 1)]);
  });

  it("evicts the least-recently-used chunk once the cache exceeds its cap", async () => {
    const { rows } = makeChunkCountingRows();

    // Fill exactly up to the cap: chunks 0..199, no eviction yet. Chunk 0 is
    // loaded first and never touched again after that, so it's the
    // least-recently-used entry once something pushes the cache over cap.
    // (Deliberately not peeking at chunk 0 here -- reading it would itself
    // bump its recency and defeat the point of this test.)
    for (let i = 0; i < MAX_CACHED_CHUNKS; i++) {
      await rows.getVisibleRows(i, i);
    }

    // One more chunk pushes the cache over the cap.
    await rows.getVisibleRows(MAX_CACHED_CHUNKS, MAX_CACHED_CHUNKS);

    expect(rows.getVisibleRow(0)).toBeNull(); // evicted -- cache miss
    expect(rows.getVisibleRow(1)).toEqual(["1"]); // survives -- still cached
  });

  it("never evicts a chunk that keeps being accessed (simulating it staying on screen)", async () => {
    const { rows } = makeChunkCountingRows();

    for (let i = 0; i < MAX_CACHED_CHUNKS; i++) {
      await rows.getVisibleRows(i, i);
    }
    // Touch chunk 0 like the grid would on every render pass while it's
    // still part of the visible window.
    rows.getVisibleRow(0);

    for (let i = MAX_CACHED_CHUNKS; i < MAX_CACHED_CHUNKS + 10; i++) {
      await rows.getVisibleRows(i, i);
      rows.getVisibleRow(0); // "still visible" on every subsequent render
    }

    expect(rows.getVisibleRow(0)).toEqual(["0"]); // kept warm -- never evicted
    expect(rows.getVisibleRow(1)).toBeNull(); // long stale -- evicted instead
  });

  it("re-fetches an evicted chunk and serves it as a cache hit afterward", async () => {
    const { rows, loadCountRef } = makeChunkCountingRows();

    for (let i = 0; i <= MAX_CACHED_CHUNKS; i++) {
      await rows.getVisibleRows(i, i);
    }
    // Captured BEFORE the eviction check below: getVisibleRow() on a cache
    // miss kicks a background fetchChunk() as a side effect, so checking
    // loadCountRef() after that call would already include it.
    const loadsBeforeRefetch = loadCountRef();

    expect(rows.getVisibleRow(0)).toBeNull(); // evicted -- cache miss, kicks a background re-fetch
    await rows.getVisibleRows(0, 0); // dedupes with that in-flight fetch and awaits it
    expect(loadCountRef()).toBe(loadsBeforeRefetch + 1);
    expect(rows.getVisibleRow(0)).toEqual(["0"]); // cache hit now
  });

  it("fetches chunks with the snapshot captured at beginReload, not live state", async () => {
    // makeSnapshot reads a mutable `live` value; a background fetch kicked off
    // after `live` has moved on must still query with the value pinned when the
    // reload began (W2: a still-armed filter/sort debounce must not leak newer
    // state into the current epoch's cache).
    let live = 1;
    const seen: number[] = [];
    const rows = createVirtualRows<number>({
      chunkSize: 2,
      getSelectedTable: () => "items",
      makeSnapshot: () => live,
      loadChunk: async (offset, _limit, snapshot) => {
        seen.push(snapshot);
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 4, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    const reload = await rows.beginReload();
    expect(reload).not.toBeNull();
    expect(reload?.snapshot).toBe(1);

    live = 2; // component state moves on AFTER the reload began
    expect(rows.getVisibleRow(2)).toBeNull(); // chunk 1 miss -> background fetch
    await loadsSettled();

    expect(seen).toContain(1);
    expect(seen).not.toContain(2);
  });

  it("discards a chunk fetched under the old epoch once a newer reload begins", async () => {
    const pending = deferred<QueryResult>();
    let live = 1;
    const rows = createVirtualRows<number>({
      chunkSize: 2,
      getSelectedTable: () => "items",
      makeSnapshot: () => live,
      loadChunk: () => pending.promise,
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    await rows.beginReload(); // epoch 1, snapshot 1
    expect(rows.getVisibleRow(0)).toBeNull(); // kicks a fetch under epoch 1
    live = 2;
    await rows.beginReload(); // epoch 2 invalidates the epoch-1 fetch

    pending.resolve({ columns: ["id"], rows: [["1"], ["2"]], total_rows: 2, offset: 0 });
    await loadsSettled();

    expect(rows.getVisibleRow(0)).toBeNull(); // stale epoch-1 result was dropped
  });

  it("reset() clears the cache, drops pending loads, and invalidates in-flight chunks", async () => {
    // First loadChunk call resolves immediately (to populate a real cached
    // chunk); every later call returns the shared, not-yet-resolved
    // `pending` deferred (to simulate a fetch still in flight at reset time).
    const pending = deferred<QueryResult>();
    let calls = 0;
    let cancelCalls = 0;
    let totalRows = 0;
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        calls++;
        if (calls === 1) {
          return { columns: ["id"], rows: [["cached"]], total_rows: 1, offset };
        }
        return pending.promise;
      },
      cancelQueries: async () => { cancelCalls++; },
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: (n) => { totalRows = n; },
      setError: () => {},
    });

    // Populate chunk 0 (resolves immediately -- the first loadChunk call).
    await rows.getVisibleRows(0, 0);
    expect(rows.getVisibleRow(0)).toEqual(["cached"]);

    // Kick off a second, still-in-flight fetch for a different chunk.
    const staleLoad = rows.getVisibleRow(2);
    expect(staleLoad).toBeNull();

    rows.reset();
    expect(cancelCalls).toBe(1);

    // The in-flight load from before reset() must not repopulate the cache:
    // reset() bumped the epoch, so this resolution is stale by the time it
    // lands. `totalRows` must stay at the value the legitimate first load
    // set (1), NOT the stale second load's total_rows (99).
    pending.resolve({ columns: ["id"], rows: [["stale"]], total_rows: 99, offset: 2 });
    await loadsSettled();
    expect(totalRows).toBe(1);
    expect(rows.getVisibleRow(0)).toBeNull(); // cache was cleared by reset()
  });
});

async function loadsSettled() {
  await Promise.resolve();
  await Promise.resolve();
}
