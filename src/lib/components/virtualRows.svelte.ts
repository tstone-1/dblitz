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

  return {
    getVisibleRow,
    getVisibleRows,
    firstChunkRows,
    beginReload,
    applyFirstChunk,
    isCurrent,
  };
}
