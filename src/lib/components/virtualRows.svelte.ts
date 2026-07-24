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
// =============================================================================

type Row = (string | null)[];

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
}

export function createVirtualRows<S = void>(deps: VirtualRowsDeps<S>) {
  let rowCache = $state<Map<number, Row[]>>(new Map());
  let pendingChunks = new Map<number, Promise<void>>();
  let epoch = 0;

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

  function projectVisible(fullRow: Row): Row {
    return deps
      .getVisibleColumns()
      .map((col) => fullRow[deps.getColumnIndex(col) ?? 0] ?? null);
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
  }

  function fetchChunk(chunkIdx: number): Promise<void> {
    const pending = pendingChunks.get(chunkIdx);
    if (pending) return pending;

    const myEpoch = epoch;
    const snapshot = currentSnapshot;
    const offset = chunkIdx * deps.chunkSize;
    let task: Promise<void>;
    task = (async () => {
      const result = await deps.loadChunk(offset, deps.chunkSize, snapshot);
      if (!isCurrent(myEpoch)) return;
      applyResult(chunkIdx, result, "if-empty");
    })().catch((e) => {
      if (isCurrent(myEpoch)) deps.setError(String(e));
    }).finally(() => {
      if (pendingChunks.get(chunkIdx) === task) pendingChunks.delete(chunkIdx);
    });

    pendingChunks.set(chunkIdx, task);
    return task;
  }

  function getRow(index: number): Row | null {
    const chunkIdx = Math.floor(index / deps.chunkSize);
    const chunk = rowCache.get(chunkIdx);
    if (!chunk) {
      void fetchChunk(chunkIdx);
      return null;
    }
    touchRecency(chunkIdx);
    return chunk[index - chunkIdx * deps.chunkSize] ?? null;
  }

  function getVisibleRow(index: number): Row | null {
    const fullRow = getRow(index);
    return fullRow ? projectVisible(fullRow) : null;
  }

  async function getVisibleRows(start: number, end: number): Promise<Row[]> {
    if (!deps.getSelectedTable()) return [];

    const myEpoch = epoch;
    const snapshot = currentSnapshot;
    const firstChunk = Math.floor(start / deps.chunkSize);
    const lastChunk = Math.floor(end / deps.chunkSize);

    // Bulk operations can span more chunks than the viewport cache is allowed
    // to retain. Materialize those ranges independently so LRU eviction cannot
    // remove an early chunk before the result is assembled.
    if (lastChunk - firstChunk + 1 > MAX_CACHED_CHUNKS) {
      const chunks = new Map<number, Row[]>();
      await Promise.all(
        Array.from({ length: lastChunk - firstChunk + 1 }, async (_, index) => {
          const chunkIdx = firstChunk + index;
          const cached = rowCache.get(chunkIdx);
          if (cached) {
            touchRecency(chunkIdx);
            chunks.set(chunkIdx, cached);
            return;
          }
          const result = await deps.loadChunk(chunkIdx * deps.chunkSize, deps.chunkSize, snapshot);
          if (isCurrent(myEpoch)) chunks.set(chunkIdx, result.rows);
        }),
      );
      if (!isCurrent(myEpoch)) {
        throw new Error("Selection changed while rows were loading. Try again.");
      }

      const out: Row[] = [];
      for (let idx = start; idx <= end; idx++) {
        const chunkIdx = Math.floor(idx / deps.chunkSize);
        const fullRow = chunks.get(chunkIdx)?.[idx - chunkIdx * deps.chunkSize];
        if (!fullRow) throw new Error("Selection contains rows that could not be loaded.");
        out.push(projectVisible(fullRow));
      }
      return out;
    }

    const loads: Promise<void>[] = [];
    for (let chunkIdx = firstChunk; chunkIdx <= lastChunk; chunkIdx++) {
      if (!rowCache.has(chunkIdx)) loads.push(fetchChunk(chunkIdx));
    }

    await Promise.all(loads);
    if (!isCurrent(myEpoch)) {
      throw new Error("Selection changed while rows were loading. Try again.");
    }

    const out: Row[] = [];
    for (let idx = start; idx <= end; idx++) {
      const chunkIdx = Math.floor(idx / deps.chunkSize);
      const chunk = rowCache.get(chunkIdx);
      if (chunk) touchRecency(chunkIdx);
      const fullRow = chunk?.[idx - chunkIdx * deps.chunkSize];
      if (!fullRow) {
        throw new Error("Selection contains rows that could not be loaded.");
      }
      out.push(projectVisible(fullRow));
    }
    return out;
  }

  async function beginReload(): Promise<{ epoch: number; snapshot: S } | null> {
    epoch++;
    const myEpoch = epoch;
    // Pin the query snapshot for this epoch BEFORE awaiting -- fetches kicked
    // off between now and the next reload must all query the same state.
    currentSnapshot = captureSnapshot();
    await deps.cancelQueries();
    if (!isCurrent(myEpoch)) return null;
    rowCache = new Map();
    pendingChunks.clear();
    chunkRecency.clear();
    return { epoch: myEpoch, snapshot: currentSnapshot };
  }

  function applyFirstChunk(myEpoch: number, result: QueryResult): boolean {
    if (!isCurrent(myEpoch)) return false;
    applyResult(0, result, "always");
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
   */
  function reset(): void {
    epoch++;
    currentSnapshot = captureSnapshot();
    rowCache = new Map();
    pendingChunks.clear();
    chunkRecency.clear();
    void deps.cancelQueries();
  }

  return {
    getVisibleRow,
    getVisibleRows,
    firstChunkRows,
    beginReload,
    applyFirstChunk,
    isCurrent,
    reset,
  };
}
