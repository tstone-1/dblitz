import type { QueryResult } from "$lib/store.svelte";

type Row = (string | null)[];

export interface VirtualRowsDeps {
  chunkSize: number;
  getSelectedTable: () => string | null;
  loadChunk: (offset: number, limit: number) => Promise<QueryResult>;
  cancelQueries: () => Promise<void>;
  getVisibleColumns: () => string[];
  getColumnIndex: (column: string) => number | undefined;
  hasColumns: () => boolean;
  setColumns: (columns: string[]) => void;
  setTotalRows: (totalRows: number) => void;
  setError: (message: string) => void;
}

export function createVirtualRows(deps: VirtualRowsDeps) {
  let rowCache = $state<Map<number, Row[]>>(new Map());
  let pendingChunks = new Map<number, Promise<void>>();
  let epoch = 0;

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
    const offset = chunkIdx * deps.chunkSize;
    let task: Promise<void>;
    task = (async () => {
      const result = await deps.loadChunk(offset, deps.chunkSize);
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
          const result = await deps.loadChunk(chunkIdx * deps.chunkSize, deps.chunkSize);
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

  async function beginReload(): Promise<number | null> {
    epoch++;
    const myEpoch = epoch;
    await deps.cancelQueries();
    if (!isCurrent(myEpoch)) return null;
    rowCache = new Map();
    pendingChunks.clear();
    chunkRecency.clear();
    return myEpoch;
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
