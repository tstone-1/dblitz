import { describe, expect, it } from "vitest";
import { createVirtualRows } from "./virtualRows.svelte";
import type { QueryResult } from "$lib/store.svelte";

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
});

async function loadsSettled() {
  await Promise.resolve();
  await Promise.resolve();
}
