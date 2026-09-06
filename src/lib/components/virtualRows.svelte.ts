import type { QueryResult } from "$lib/ipc";

// =============================================================================
// Reload / generation protocol (the whole picture, front to back)
// -----------------------------------------------------------------------------
// A database switch, a filter/sort change, and a same-path reopen all have to
// invalidate different amounts of cached state without ever letting a
// late-arriving async result repopulate a cache it no longer belongs to.
// The layers, outermost to innermost, and what each protects against:
//
// * store.databaseRequestGeneration (+ databaseRequestQueue) -- serializes
//   overlapping open/close IPC transactions against the singleton backend
//   connection; an older open that's still running stops before it publishes
//   frontend state. store.dbOpenGeneration -- bumped on EVERY successful open
//   (same-path reopen included) and is the signal every frontend reset keys on,
//   because dbPath alone can't detect "reopened the file I already had open".
//
// * BrowseData's reset effect -- on a dbOpenGeneration change, drops
//   selectedTable/columns/filters/sort and calls virtualRows.reset(). Its
//   `lastFilterState` memo dedupes the ~500ms filter debounce so an unchanged
//   filter never re-queries.
//
// * virtualRows.epoch -- the row-cache generation. beginReload() bumps epoch,
//   AWAITS cancelQueries() (a reload is commencing that the caller will await),
//   captures a per-epoch snapshot of table/filters/sort, then clears the cache.
//   reset() bumps epoch fire-and-forget (no reload to await) for a DB switch.
//   Every getRow-triggered background fetch is tagged with the epoch and the
//   snapshot LIVE AT beginReload -- never current component state -- so a chunk
//   fetched during a still-armed filter/sort debounce can't land new-filter
//   rows into the old epoch's cache (isCurrent() discards it; the snapshot
//   keeps its query self-consistent even before that check).
//
// * backend query_generation + cancellation token -- open_database bumps the
//   Rust-side generation and cancel_queries flips a token so in-flight SQL on
//   the old connection stops and returns partial rows rather than fighting the
//   new one.
//
// -----------------------------------------------------------------------------
// Request discipline (what stops a miss from becoming a storm)
// -----------------------------------------------------------------------------
// `getRow` runs synchronously inside the grid's render pass, once per visible
// row, and a miss used to fire an IPC call there and then. Three separate
// runaways came out of that, and each has its own guard below:
//
// * A chunk whose fetch FAILED was retried by the very next render, forever.
//   A regex filter of "(" returns "Invalid regex" per chunk, and the grid then
//   re-issued it every frame with no way out but changing the filter.
//   -> `failedChunks`, latched per epoch and cleared on the next reload.
//
// * A chunk the user scrolled PAST was still fetched. The request is issued
//   one tick late instead (`deps.defer`), and dropped at that point if it is no
//   longer inside the window the grid last rendered (`setVisibleWindow`).
//
// * A wide `getVisibleRows` range (Ctrl+A copy) fired every missing chunk at
//   once with `Promise.all` and bypassed the in-flight map entirely -- 200
//   concurrent `query_table` calls, duplicating whatever the viewport already
//   had in flight. -> both paths now share `pendingChunks` through
//   `loadShared`, and the range path runs at most `MAX_RANGE_CONCURRENCY` of
//   them.
// =============================================================================

type Row = (string | null)[];

// Backend message for a query the generation/cancellation token stopped (see
// `db/query.rs` and `db/sql.rs`). It arrives wrapped in query context by
// `err_ctx` -- `querying table "x": Query cancelled by a newer request` -- so
// callers must substring-match it rather than compare.
const CANCELLED_QUERY_MESSAGE = "Query cancelled by a newer request";

// `S` is an opaque per-reload query snapshot (table + filters + sort) captured
// by the caller. virtualRows never inspects it; it only pins the value taken at
// beginReload/reset time and hands it back to loadChunk so background fetches
// query with the epoch's state, not whatever the component holds now.
export interface VirtualRowsDeps<S = void> {
  chunkSize: number;
  getSelectedTable: () => string | null;
  makeSnapshot?: () => S;
  loadChunk: (offset: number, limit: number, snapshot: S) => Promise<QueryResult>;
  cancelQueries: () => Promise<void>;
  getVisibleColumns: () => string[];
  getColumnIndex: (column: string) => number | undefined;
  hasColumns: () => boolean;
  setColumns: (columns: string[]) => void;
  setTotalRows: (totalRows: number) => void;
  setError: (message: string) => void;
  /**
   * Runs a render-pass-deferred chunk request. Injected so a test can flush it
   * synchronously; production uses a macrotask, which is what lets a request
   * queued by one render pass be dropped after a later pass has scrolled the
   * chunk out of view.
   */
  defer?: (run: () => void) => void;
}

export function createVirtualRows<S = void>(deps: VirtualRowsDeps<S>) {
  let rowCache = $state<Map<number, Row[]>>(new Map());
  // Bumped whenever a chunk lands. Consumers that want to recompute something
  // from CACHED rows only (DataGrid's selection statistics) depend on this
  // instead of calling getRow and triggering fetches from inside a `$derived`.
  let cacheVersion = $state(0);
  // One in-flight load per chunk, shared by the viewport fetcher and the range
  // materializer so neither can duplicate the other's request.
  let pendingChunks = new Map<number, Promise<QueryResult | null>>();
  // Chunks whose in-flight load must be written into rowCache when it lands.
  // A range wider than the cache cap deliberately does not publish (see
  // getVisibleRows), but a viewport fetch for the same chunk still does.
  let publishOnArrival = new Set<number>();
  // Chunks whose load failed in THIS epoch. Latched: the grid re-renders on
  // every scroll and every state change, so without this a chunk that cannot
  // load is re-requested forever.
  let failedChunks = new Set<number>();
  // Chunks a render pass asked for and whose request has not been issued yet.
  let scheduledChunks = new Set<number>();
  let flushScheduled = false;
  // Chunk range the grid last rendered; null until it reports one.
  let visibleChunks: { first: number; last: number } | null = null;
  let epoch = 0;

  const defer = deps.defer ?? ((run: () => void) => { setTimeout(run, 0); });

  // How many chunk loads a wide `getVisibleRows` range may have in flight.
  // Four is enough to keep the backend busy while a Ctrl+A copy over a
  // million-row table stays a queue rather than a stampede.
  const MAX_RANGE_CONCURRENCY = 4;

  function captureSnapshot(): S {
    return deps.makeSnapshot ? deps.makeSnapshot() : (undefined as S);
  }

  // The query state (table/filters/sort) this epoch's fetches must use. Pinned
  // at beginReload/reset so a getRow-triggered fetch can never read a newer
  // filter/sort that the component has since moved to but not yet reloaded.
  let currentSnapshot: S = captureSnapshot();

  // Bounds how many chunks stay resident in rowCache so scrolling through a
  // huge (multi-million-row) table doesn't grow memory without limit. 200
  // chunks x the app's default 500-row CHUNK_SIZE = 100k rows resident --
  // generous scrollback before a chunk has to be refetched.
  const MAX_CACHED_CHUNKS = 200;

  // Plain (non-reactive) LRU recency tracker -- deliberately NOT $state.
  // getRow() runs synchronously inside the grid's render pass (every visible
  // row calls it on every re-render), so reassigning a $state Map there just
  // to bump recency would write to reactive state mid-read, re-triggering
  // the very render that produced the write and risking a runaway reactive
  // loop. A plain Map does the bookkeeping instead: it relies on Map's
  // insertion-order guarantee -- delete+re-set moves a key to the end, so
  // the first key in iteration order is always the least-recently-used
  // chunk. Because currently-rendered chunks get touched on every render
  // pass, they're always at the recent end and are never the eviction
  // target as long as the cap comfortably exceeds the visible window.
  const chunkRecency = new Map<number, true>();

  function touchRecency(chunkIdx: number) {
    chunkRecency.delete(chunkIdx);
    chunkRecency.set(chunkIdx, true);
  }

  function evictIfOverCap(cache: Map<number, Row[]>) {
    if (cache.size <= MAX_CACHED_CHUNKS) return;
    for (const chunkIdx of chunkRecency.keys()) {
      if (cache.size <= MAX_CACHED_CHUNKS) break;
      if (cache.delete(chunkIdx)) chunkRecency.delete(chunkIdx);
    }
  }

  function isCurrent(myEpoch: number): boolean {
    return myEpoch === epoch;
  }

  /**
   * A projector for the current visible-column set.
   *
   * Built ONCE per projection pass. `projectVisible` used to call
   * `deps.getVisibleColumns()` (in BrowseData: rebuild the ordered list, then
   * the filtered visible list) and `deps.getColumnIndex` per row, so a Ctrl+A
   * copy over 100k rows rebuilt two arrays 100k times.
   */
  function visibleProjector(): (fullRow: Row) => Row {
    const indices = deps
      .getVisibleColumns()
      .map((col) => deps.getColumnIndex(col) ?? 0);
    return (fullRow) => indices.map((i) => fullRow[i] ?? null);
  }

  function projectVisible(fullRow: Row): Row {
    return visibleProjector()(fullRow);
  }

  function applyResult(
    chunkIdx: number,
    result: QueryResult,
    columnMode: "always" | "if-empty",
  ) {
    if (result.total_rows !== null) deps.setTotalRows(result.total_rows);
    if (columnMode === "always" ? result.columns.length > 0 : !deps.hasColumns()) {
      deps.setColumns(result.columns);
    }

    const newCache = new Map(rowCache);
    newCache.set(chunkIdx, result.rows);
    touchRecency(chunkIdx);
    evictIfOverCap(newCache);
    rowCache = newCache;
    cacheVersion++;
  }

  /**
   * The single in-flight load for a chunk. Both the viewport fetcher and the
   * range materializer go through here, so a chunk is never requested twice
   * concurrently regardless of which of them asked first.
   *
   * Resolves to `null` when the epoch moved on while the load was in flight --
   * that answer belongs to a query nobody is showing any more.
   */
  function loadShared(chunkIdx: number, publish: boolean): Promise<QueryResult | null> {
    if (publish) publishOnArrival.add(chunkIdx);
    const pending = pendingChunks.get(chunkIdx);
    if (pending) return pending;

    const myEpoch = epoch;
    const snapshot = currentSnapshot;
    const offset = chunkIdx * deps.chunkSize;
    let tracked: Promise<QueryResult | null>;
    const task = deps.loadChunk(offset, deps.chunkSize, snapshot).then((result) => {
      if (!isCurrent(myEpoch)) return null;
      if (publishOnArrival.has(chunkIdx)) applyResult(chunkIdx, result, "if-empty");
      return result;
    });
    tracked = task.finally(() => {
      if (pendingChunks.get(chunkIdx) === tracked) pendingChunks.delete(chunkIdx);
      publishOnArrival.delete(chunkIdx);
    });
    pendingChunks.set(chunkIdx, tracked);
    return tracked;
  }

  function fetchChunk(chunkIdx: number): Promise<void> {
    const myEpoch = epoch;
    return loadShared(chunkIdx, true).then(
      () => {},
      (e: unknown) => {
        const message = String(e);
        // `cancel_queries` bumps a backend generation shared by BOTH tabs, so
        // cancelling a SQL-tab query rejects any browse chunk fetch that happens
        // to be in flight -- with this epoch still current, so the isCurrent
        // guard alone does not filter it. That is a successful user action, not
        // a failure: the chunk simply isn't cached, and the next render calls
        // getRow() again and refetches it. Reporting it would put a red error
        // bar up for pressing Cancel -- and latching it would make a cancel
        // permanently blank the rows it interrupted.
        if (!isCurrent(myEpoch) || message.includes(CANCELLED_QUERY_MESSAGE)) return;
        failedChunks.add(chunkIdx);
        deps.setError(message);
      },
    );
  }

  /**
   * Note that the grid wants a chunk, and issue the request one tick later.
   *
   * The deferral is what makes the visible-window check meaningful: by the time
   * the flush runs, the grid may have rendered again at a different scroll
   * position, and a chunk that is no longer under the viewport is dropped
   * rather than fetched for nobody. `pendingChunks` still dedupes anything that
   * does get issued.
   */
  function scheduleFetch(chunkIdx: number) {
    if (scheduledChunks.has(chunkIdx) || pendingChunks.has(chunkIdx)) return;
    scheduledChunks.add(chunkIdx);
    const myEpoch = epoch;
    if (flushScheduled) return;
    flushScheduled = true;
    defer(() => {
      flushScheduled = false;
      const requested = scheduledChunks;
      scheduledChunks = new Set();
      if (!isCurrent(myEpoch)) return;
      for (const idx of requested) {
        if (failedChunks.has(idx)) continue;
        if (visibleChunks && (idx < visibleChunks.first || idx > visibleChunks.last)) continue;
        void fetchChunk(idx);
      }
    });
  }

  /**
   * The row range the grid last rendered. Called from the render pass, and it
   * writes only plain (non-reactive) state -- the same discipline
   * `touchRecency` follows and for the same reason.
   */
  function setVisibleWindow(firstRow: number, lastRow: number) {
    visibleChunks = {
      first: Math.floor(firstRow / deps.chunkSize),
      last: Math.floor(lastRow / deps.chunkSize),
    };
  }

  function getRow(index: number): Row | null {
    const chunkIdx = Math.floor(index / deps.chunkSize);
    const chunk = rowCache.get(chunkIdx);
    if (!chunk) {
      if (!failedChunks.has(chunkIdx)) scheduleFetch(chunkIdx);
      return null;
    }
    touchRecency(chunkIdx);
    return chunk[index - chunkIdx * deps.chunkSize] ?? null;
  }

  /** Cached-only read: never schedules a fetch. */
  function peekRow(index: number): Row | null {
    const chunkIdx = Math.floor(index / deps.chunkSize);
    const chunk = rowCache.get(chunkIdx);
    if (!chunk) return null;
    return chunk[index - chunkIdx * deps.chunkSize] ?? null;
  }

  function getVisibleRow(index: number): Row | null {
    const fullRow = getRow(index);
    return fullRow ? projectVisible(fullRow) : null;
  }

  /**
   * `getVisibleRow` without the side effect.
   *
   * DataGrid's selection statistics are computed from whatever is cached; they
   * must never start a fetch, because they run for every cell of the selection
   * and used to walk a Ctrl+A selection chunk by chunk, pulling the entire table
   * through IPC to produce a Sum/Avg nobody asked to wait for.
   */
  function peekVisibleRow(index: number): Row | null {
    const fullRow = peekRow(index);
    return fullRow ? projectVisible(fullRow) : null;
  }

  async function getVisibleRows(start: number, end: number): Promise<Row[]> {
    if (!deps.getSelectedTable()) return [];

    const myEpoch = epoch;
    const firstChunk = Math.floor(start / deps.chunkSize);
    const lastChunk = Math.floor(end / deps.chunkSize);
    const chunkCount = lastChunk - firstChunk + 1;

    // Every multi-chunk range is assembled out of `chunks` -- a map local to
    // this call -- and never by reading `rowCache` back once the loads have
    // finished. Reading it back is what W2 broke on: each load runs
    // applyResult -> evictIfOverCap, and an in-range chunk that was ALREADY
    // cached is only recency-touched at assembly time, i.e. after the
    // evictions have had their chance to drop it. A Ctrl+A copy over a full
    // cache then failed with "Selection contains rows that could not be
    // loaded." for chunks that had been fetched perfectly well. A local map is
    // immune to that regardless of how wide the range is.
    const chunks = new Map<number, Row[]>();

    if (chunkCount === 1) {
      // Single chunk: nothing can evict it between the load and the read (the
      // load puts it at the most-recent end of the LRU order), so take the
      // cheap path -- it dedupes against an in-flight viewport fetch through
      // `pendingChunks` and leaves the chunk cached for the grid to reuse.
      if (!rowCache.has(firstChunk)) await fetchChunk(firstChunk);
      const cached = rowCache.get(firstChunk);
      if (cached) {
        touchRecency(firstChunk);
        chunks.set(firstChunk, cached);
      }
    } else {
      // Ranges that still fit in the cache publish what they fetch, so a copy
      // also warms the viewport for subsequent scrolling (and keeps
      // applyResult's total-rows/columns side effects). Ranges wider than the
      // cap deliberately do not: they cannot all stay resident anyway, so
      // publishing them would only evict the user's actual viewport chunks.
      const publishFetched = chunkCount <= MAX_CACHED_CHUNKS;
      const missing: number[] = [];
      for (let index = 0; index < chunkCount; index++) {
        const chunkIdx = firstChunk + index;
        // Read the cache before the first await, so every task in this batch
        // sees the same pre-load contents and a later eviction cannot unsee
        // a hit that was there when the range started.
        const cached = rowCache.get(chunkIdx);
        if (cached) {
          touchRecency(chunkIdx);
          chunks.set(chunkIdx, cached);
        } else {
          missing.push(chunkIdx);
        }
      }

      // A bounded worker pool rather than `Promise.all` over every chunk: a
      // Ctrl+A copy of a million rows is 2,000 chunks, and firing them at once
      // both stampedes the single backend connection and duplicates whatever
      // the viewport already had in flight.
      let next = 0;
      const worker = async () => {
        while (next < missing.length) {
          const chunkIdx = missing[next++];
          const result = await loadShared(chunkIdx, publishFetched);
          if (result) chunks.set(chunkIdx, result.rows);
        }
      };
      await Promise.all(
        Array.from(
          { length: Math.min(MAX_RANGE_CONCURRENCY, missing.length) },
          worker,
        ),
      );
    }

    if (!isCurrent(myEpoch)) {
      throw new Error("Selection changed while rows were loading. Try again.");
    }

    // One projector for the whole range, not one per row.
    const project = visibleProjector();
    const out: Row[] = [];
    for (let idx = start; idx <= end; idx++) {
      const chunkIdx = Math.floor(idx / deps.chunkSize);
      const fullRow = chunks.get(chunkIdx)?.[idx - chunkIdx * deps.chunkSize];
      if (!fullRow) throw new Error("Selection contains rows that could not be loaded.");
      out.push(project(fullRow));
    }
    return out;
  }

  /** Everything that is scoped to one epoch's cache contents. */
  function clearEpochState() {
    rowCache = new Map();
    pendingChunks.clear();
    publishOnArrival.clear();
    failedChunks.clear();
    scheduledChunks.clear();
    chunkRecency.clear();
    cacheVersion++;
  }

  async function beginReload(): Promise<{ epoch: number; snapshot: S } | null> {
    epoch++;
    const myEpoch = epoch;
    // Pin the query snapshot for this epoch BEFORE awaiting -- fetches kicked
    // off between now and the next reload must all query the same state.
    currentSnapshot = captureSnapshot();
    await deps.cancelQueries();
    if (!isCurrent(myEpoch)) return null;
    clearEpochState();
    return { epoch: myEpoch, snapshot: currentSnapshot };
  }

  function applyFirstChunk(myEpoch: number, result: QueryResult): boolean {
    if (!isCurrent(myEpoch)) return false;
    applyResult(0, result, "always");
    return true;
  }

  /**
   * The first chunk of this reload failed.
   *
   * Two things have to happen together, and doing only one of them is what made
   * a bad filter look like a corrupted grid. `beginReload` has already emptied
   * the cache, but the caller's `totalRows` still holds the PREVIOUS query's
   * count -- so the grid renders that many empty rows, each of which calls
   * getRow, misses, and re-fires the failing query. Zeroing the row count
   * leaves the error banner alone on screen, and latching chunk 0 stops the
   * retry loop for anything that does still get rendered.
   */
  function failFirstChunk(myEpoch: number): boolean {
    if (!isCurrent(myEpoch)) return false;
    failedChunks.add(0);
    deps.setTotalRows(0);
    return true;
  }

  function firstChunkRows(): Row[] {
    return rowCache.get(0) ?? [];
  }

  /**
   * Hard reset for a database switch (no table selected / a different
   * database was just opened). Unlike `beginReload()`, this does not await
   * `cancelQueries()` -- there's no reload commencing for the caller to wait
   * on, only stale state to drop -- and it doesn't hand back an epoch. Still
   * bumps `epoch` so any fetch still in flight from the old database is
   * ignored (via `isCurrent`) when it eventually resolves, instead of
   * repopulating the cache with the wrong database's rows.
   *
   * NOT safe to call unconditionally from a Svelte `$effect`: `captureSnapshot()`
   * reads the caller's table/filter/sort state, so an effect that also *writes*
   * that state (as a database-switch reset does) invalidates itself and loops.
   * Call it only on an actual transition — see `resetFired` in
   * `autoSelectFirstTable.svelte.ts`, where doing otherwise produced
   * `effect_update_depth_exceeded` on every launch.
   */
  function reset(): void {
    epoch++;
    currentSnapshot = captureSnapshot();
    clearEpochState();
    visibleChunks = null;
    void deps.cancelQueries();
  }

  return {
    getVisibleRow,
    peekVisibleRow,
    getVisibleRows,
    firstChunkRows,
    setVisibleWindow,
    beginReload,
    applyFirstChunk,
    failFirstChunk,
    isCurrent,
    reset,
    get cacheVersion() {
      return cacheVersion;
    },
  };
}
