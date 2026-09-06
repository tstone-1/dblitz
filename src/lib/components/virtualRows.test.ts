import { describe, expect, it } from "vitest";
import { createVirtualRows } from "./virtualRows.svelte";
import type { QueryResult } from "$lib/ipc";

// Production defers a chunk request by one macrotask so a chunk the user has
// scrolled past can be dropped instead of fetched (see `scheduleFetch`). Every
// test below that is not specifically ABOUT the deferral injects this instead,
// so `getRow` still issues its request synchronously and the existing
// assertions keep measuring what they were written to measure.
const runNow = (run: () => void) => run();

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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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
      defer: runNow,
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

  // ---- Failure latch (a failing chunk is not retried every render) --------

  it("retries a failed chunk on every render without the latch, and never with it", async () => {
    // The reported defect: a regex filter of "(" makes every chunk fail with
    // "Invalid regex". The grid re-renders constantly, each render calls getRow
    // for every visible row, and each miss re-fired the same failing IPC call.
    const errors: string[] = [];
    let loadCount = 0;
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async () => {
        loadCount++;
        throw new Error("Invalid regex");
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
    expect(loadCount).toBe(1);
    expect(errors).toHaveLength(1);

    // Five more render passes over the same rows.
    for (let pass = 0; pass < 5; pass++) {
      expect(rows.getVisibleRow(0)).toBeNull();
      expect(rows.getVisibleRow(1)).toBeNull();
      await loadsSettled();
    }
    expect(loadCount).toBe(1);
    expect(errors).toHaveLength(1);
  });

  it("clears the failure latch on the next reload", async () => {
    // Positive control for the latch: it must not turn a transient failure into
    // a permanently blank table. Fixing the filter has to make the rows load.
    let fail = true;
    let loadCount = 0;
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        loadCount++;
        if (fail) throw new Error("Invalid regex");
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 2, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(loadCount).toBe(1);

    fail = false;
    await rows.beginReload();
    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(loadCount).toBe(2);
    expect(rows.getVisibleRow(0)).toEqual(["a"]);
  });

  it("does not latch a chunk cancelled by a newer request", async () => {
    // Pressing Cancel in the SQL tab rejects an in-flight browse chunk. That is
    // a successful user action; latching it would blank the rows it interrupted
    // until the next reload.
    let attempt = 0;
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        attempt++;
        if (attempt === 1) {
          throw new Error('querying table "items": Query cancelled by a newer request');
        }
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 2, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(rows.getVisibleRow(0)).toBeNull(); // next render retries
    await loadsSettled();
    expect(rows.getVisibleRow(0)).toEqual(["a"]);
    expect(attempt).toBe(2);
  });

  it("zeroes the row count and latches chunk 0 when the first chunk fails", async () => {
    // beginReload() has already emptied the cache, so leaving the previous
    // query's totalRows in place made the grid render that many blank rows --
    // each of which called getRow and re-fired the failing query.
    let totalRows = 99;
    let loadCount = 0;
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async () => {
        loadCount++;
        throw new Error("Invalid regex");
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: (next) => { totalRows = next; },
      setError: () => {},
    });

    const reload = await rows.beginReload();
    expect(reload).not.toBeNull();
    expect(rows.failFirstChunk(reload!.epoch)).toBe(true);
    expect(totalRows).toBe(0);

    // Anything still rendered must not re-fire the failing query.
    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(loadCount).toBe(0);
  });

  it("ignores failFirstChunk from a superseded reload", async () => {
    let totalRows = 42;
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => ({ columns: ["id"], rows: [], total_rows: 0, offset }),
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: (next) => { totalRows = next; },
      setError: () => {},
    });

    const stale = await rows.beginReload();
    await rows.beginReload(); // a newer reload takes over
    expect(rows.failFirstChunk(stale!.epoch)).toBe(false);
    expect(totalRows).toBe(42);
  });

  // ---- Deferral + visible window ------------------------------------------

  it("drops a deferred request for a chunk that scrolled out of view", async () => {
    const loaded: number[] = [];
    let flush: (() => void) | null = null;
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        loaded.push(offset);
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 1000, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
      defer: (run) => { flush = run; },
    });

    // Render pass 1: rows 0-1 (chunk 0) are on screen and miss.
    rows.setVisibleWindow(0, 1);
    expect(rows.getVisibleRow(0)).toBeNull();

    // The user scrolls before the deferred request is issued: pass 2 renders
    // rows 100-101 (chunk 50) and asks for those instead.
    rows.setVisibleWindow(100, 101);
    expect(rows.getVisibleRow(100)).toBeNull();

    expect(loaded).toEqual([]); // nothing issued yet -- that is the deferral
    flush!();
    await loadsSettled();

    // Chunk 50 is fetched; chunk 0 is dropped, because nothing is showing it.
    expect(loaded).toEqual([100]);
  });

  it("still fetches a chunk that is inside the reported window", async () => {
    // Emptiness control for the test above: a window check that rejected
    // everything would satisfy it just as well.
    const loaded: number[] = [];
    let flush: (() => void) | null = null;
    const rows = createVirtualRows({
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        loaded.push(offset);
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 1000, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
      defer: (run) => { flush = run; },
    });

    rows.setVisibleWindow(0, 5);
    expect(rows.getVisibleRow(0)).toBeNull();
    expect(rows.getVisibleRow(4)).toBeNull();
    expect(loaded).toEqual([]);
    flush!();
    await loadsSettled();
    expect(loaded.toSorted((a, b) => a - b)).toEqual([0, 4]);
  });

  // ---- Range materialisation concurrency ----------------------------------

  it("bounds concurrency and dedupes against an in-flight viewport fetch", async () => {
    // Ctrl+A over a wide selection used to fire every missing chunk at once
    // with Promise.all, bypassing the in-flight map entirely: 200 concurrent
    // query_table calls against one backend connection, duplicating whatever
    // the viewport already had going.
    const started: number[] = [];
    let inFlight = 0;
    let peakInFlight = 0;
    const gates: Array<() => void> = [];

    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: (offset) => {
        started.push(offset);
        inFlight++;
        peakInFlight = Math.max(peakInFlight, inFlight);
        return new Promise<QueryResult>((resolve) => {
          gates.push(() => {
            inFlight--;
            resolve({ columns: ["id"], rows: [["a"], ["b"]], total_rows: 40, offset });
          });
        });
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    // The viewport already has chunk 3 (rows 6-7) in flight.
    expect(rows.getVisibleRow(6)).toBeNull();
    expect(started).toEqual([6]);

    // Ctrl+A over rows 0..19 -- ten chunks, one of them already loading.
    const materialized = rows.getVisibleRows(0, 19);

    // Release the gates as they open, tracking the high-water mark.
    for (let guard = 0; guard < 100 && gates.length > 0; guard++) {
      gates.shift()!();
      await loadsSettled();
    }
    await materialized;

    expect(peakInFlight).toBeLessThanOrEqual(4);
    // Ten chunks, each requested exactly once: the viewport's chunk 3 was
    // joined, not re-requested.
    expect(started.toSorted((a, b) => a - b)).toEqual([0, 2, 4, 6, 8, 10, 12, 14, 16, 18]);
  });

  // ---- Cached-only reads ---------------------------------------------------

  it("peekVisibleRow never starts a fetch", async () => {
    // DataGrid's selection statistics run over every selected cell. Reading
    // them through getRow pulled the whole table across IPC, one chunk per
    // re-render, to compute a Sum nobody asked to wait for.
    const loaded: number[] = [];
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => {
        loaded.push(offset);
        return { columns: ["id"], rows: [["a"], ["b"]], total_rows: 100, offset };
      },
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    expect(rows.peekVisibleRow(0)).toBeNull();
    expect(rows.peekVisibleRow(50)).toBeNull();
    await loadsSettled();
    expect(loaded).toEqual([]);

    // Once a chunk is cached the peek reads it, so the guard is not "always
    // null".
    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(rows.peekVisibleRow(0)).toEqual(["a"]);
  });

  it("bumps cacheVersion when a chunk lands", async () => {
    const rows = createVirtualRows({
      defer: runNow,
      chunkSize: 2,
      getSelectedTable: () => "items",
      loadChunk: async (offset) => ({
        columns: ["id"], rows: [["a"], ["b"]], total_rows: 100, offset,
      }),
      cancelQueries: async () => {},
      getVisibleColumns: () => ["id"],
      getColumnIndex: () => 0,
      hasColumns: () => true,
      setColumns: () => {},
      setTotalRows: () => {},
      setError: () => {},
    });

    const before = rows.cacheVersion;
    expect(rows.getVisibleRow(0)).toBeNull();
    await loadsSettled();
    expect(rows.cacheVersion).toBeGreaterThan(before);
  });
});

/** Drain the microtask queue. A macrotask boundary flushes all of it, which a
 *  fixed number of `await Promise.resolve()` does not -- the load pipeline is
 *  several `.then`/`.finally` links deep. */
async function loadsSettled() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}
