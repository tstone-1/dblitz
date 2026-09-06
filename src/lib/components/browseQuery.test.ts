import { describe, expect, it, vi } from "vitest";
import { createBrowseQuery, type BrowseQueryDeps } from "./browseQuery.svelte";
import type { ColumnFilterValue, QueryResult, ViewConfig } from "$lib/ipc";

/** Drain the microtask queue. `reloadData` is several awaits deep -- and each
 *  reload bumps the epoch at its FIRST await, so two started back to back means
 *  the older one returns before it ever issues a query. Flushing between them
 *  is what makes each test exercise the stage it names. */
async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function freshConfig(): ViewConfig {
  return {
    hidden_columns: [],
    column_colors: {},
    sort_column: null,
    sort_asc: true,
    column_order: [],
    pinned_filters: {},
    pinned_global_filter: null,
    column_widths: {},
  };
}

interface HarnessOptions {
  /** Answers `query_table`. Default: two rows with a known total. */
  queryTable?: BrowseQueryDeps["queryTable"];
  countRows?: BrowseQueryDeps["countRows"];
  configs?: Record<string, ViewConfig>;
  cachedColumns?: Record<string, string[]>;
}

function harness(options: HarnessOptions = {}) {
  const state = {
    selectedTable: null as string | null,
    columns: [] as string[],
    totalRows: 0,
    globalFilter: "",
    columnFilters: {} as Record<string, ColumnFilterValue>,
    sortColumn: null as string | null,
    sortAsc: true,
    loading: false,
    countPending: false,
  };
  const errors: string[] = [];
  const configs: Record<string, ViewConfig> = options.configs ?? {};
  const cachedColumns = options.cachedColumns ?? { items: ["id", "name"] };
  const queries: Array<{ table: string; offset: number; sortColumn: string | null; sortAsc: boolean; globalFilter: string }> = [];
  let autoFitCalls = 0;

  const query = createBrowseQuery({
    chunkSize: 2,
    filterDebounceMs: 500,
    getSelectedTable: () => state.selectedTable,
    setSelectedTable: (value) => { state.selectedTable = value; },
    getColumns: () => state.columns,
    setColumns: (value) => { state.columns = value; },
    setTotalRows: (value) => { state.totalRows = value; },
    getGlobalFilter: () => state.globalFilter,
    setGlobalFilter: (value) => { state.globalFilter = value; },
    getColumnFilters: () => state.columnFilters,
    setColumnFilters: (value) => { state.columnFilters = value; },
    getSortColumn: () => state.sortColumn,
    setSortColumn: (value) => { state.sortColumn = value; },
    getSortAsc: () => state.sortAsc,
    setSortAsc: (value) => { state.sortAsc = value; },
    setLoading: (value) => { state.loading = value; },
    setCountPending: (value) => { state.countPending = value; },
    setError: (message) => errors.push(message),
    getVisibleColumns: () => state.columns,
    getColumnIndex: (col) => state.columns.indexOf(col),
    getCachedTableColumns: (table) => cachedColumns[table],
    getTableConfig: (table) => configs[table] ?? freshConfig(),
    ensureTableConfig: (table) => (configs[table] ??= freshConfig()),
    updateTableConfig: (table, mutate) => {
      const cfg = (configs[table] ??= freshConfig());
      mutate(cfg);
      configs[table] = { ...cfg };
    },
    queryTable:
      options.queryTable ??
      ((args) => {
        queries.push({
          table: args.table,
          offset: args.offset,
          sortColumn: args.sortColumn,
          sortAsc: args.sortAsc,
          globalFilter: args.globalFilter,
        });
        return Promise.resolve<QueryResult>({
          columns: ["id", "name"],
          rows: [["1", "a"], ["2", "b"]],
          total_rows: 2,
          offset: args.offset,
        });
      }),
    countRows: options.countRows ?? (() => Promise.resolve(0)),
    cancelQueries: () => Promise.resolve(),
    tick: () => Promise.resolve(),
    applyAutoWidths: () => { autoFitCalls++; },
    defer: (run) => run(),
  });

  return {
    query,
    state,
    errors,
    configs,
    queries,
    autoFitCalls: () => autoFitCalls,
  };
}

describe("createBrowseQuery.selectTable", () => {
  it("hydrates columns, sort and pinned filters, then loads the first chunk", async () => {
    // Positive control for the whole machine: without it, every "did not
    // publish" assertion below is satisfied by a machine that does nothing.
    const configs = {
      items: {
        ...freshConfig(),
        sort_column: "name",
        sort_asc: false,
        pinned_filters: { name: { value: "a", is_regex: false } },
        pinned_global_filter: "x",
      },
    };
    const h = harness({ configs });
    await h.query.selectTable("items");

    expect(h.state.selectedTable).toBe("items");
    expect(h.state.columns).toEqual(["id", "name"]);
    expect(h.state.sortColumn).toBe("name");
    expect(h.state.sortAsc).toBe(false);
    expect(h.state.columnFilters).toEqual({ name: { value: "a", is_regex: false } });
    expect(h.state.globalFilter).toBe("x");
    expect(h.state.totalRows).toBe(2);
    expect(h.state.loading).toBe(false);
    expect(h.queries).toHaveLength(1);
    expect(h.queries[0]).toMatchObject({ table: "items", offset: 0, sortColumn: "name", sortAsc: false });
  });

  it("drops a saved sort on a column the table no longer has", async () => {
    const configs = { items: { ...freshConfig(), sort_column: "gone", sort_asc: false } };
    const h = harness({ configs });
    await h.query.selectTable("items");

    expect(h.state.sortColumn).toBeNull();
    expect(h.state.sortAsc).toBe(true);
    expect(h.configs.items.sort_column).toBeNull();
  });

  it("does not auto-fit widths for a table the user has already left", async () => {
    // selectTable's tail has no epoch, so it re-reads the selection: click
    // table A (slow first load), then B, and A's resumed tail used to measure
    // B's grid and persist those widths into B's config.
    let resolveFirst!: (value: QueryResult) => void;
    const first = new Promise<QueryResult>((resolve) => { resolveFirst = resolve; });
    let call = 0;
    const h = harness({
      cachedColumns: { a: ["id"], b: ["id"] },
      queryTable: (args) => {
        call++;
        if (call === 1) return first;
        return Promise.resolve<QueryResult>({
          columns: ["id"], rows: [["1"]], total_rows: 1, offset: args.offset,
        });
      },
    });

    const slow = h.query.selectTable("a");
    await flush(); // "a" is now awaiting its first chunk
    const second = h.query.selectTable("b");
    resolveFirst({ columns: ["id"], rows: [["1"]], total_rows: 1, offset: 0 });
    await Promise.all([slow, second]);

    // Exactly one auto-fit, and it belongs to the table that is still selected.
    expect(h.state.selectedTable).toBe("b");
    expect(h.autoFitCalls()).toBe(1);
  });
});

describe("createBrowseQuery.reloadData", () => {
  it("zeroes the row count and reports the error when the first chunk fails", async () => {
    // The reported defect: a regex filter of "(" returns "Invalid regex".
    // beginReload() has already emptied the row cache, but totalRows still held
    // the previous query's count -- so the grid rendered that many blank rows,
    // each of which called getRow, missed, and re-fired the failing query.
    let calls = 0;
    const h = harness({
      queryTable: () => {
        calls++;
        return Promise.reject(new Error("Invalid regex"));
      },
    });
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];
    h.state.totalRows = 5000;

    await h.query.reloadData();

    expect(h.state.totalRows).toBe(0);
    expect(h.state.loading).toBe(false);
    expect(h.state.countPending).toBe(false);
    expect(h.errors[0]).toContain("Invalid regex");

    // And nothing the grid still renders can restart it.
    expect(h.query.virtualRows.getVisibleRow(0)).toBeNull();
    await flush();
    expect(calls).toBe(1);
  });

  it("keeps the row count when the first chunk succeeds", async () => {
    // Emptiness control for the test above: zeroing unconditionally would
    // blank a perfectly good table.
    const h = harness();
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];
    h.state.totalRows = 5000;

    await h.query.reloadData();
    expect(h.state.totalRows).toBe(2);
  });

  it("publishes nothing from a reload a newer one superseded", async () => {
    // Every post-await consumer re-checks ownership: rows, the row count, the
    // error and `loading` all belong to the newest reload only.
    let resolveFirst!: (value: QueryResult) => void;
    const first = new Promise<QueryResult>((resolve) => { resolveFirst = resolve; });
    let call = 0;
    const h = harness({
      queryTable: (args) => {
        call++;
        if (call === 1) return first;
        return Promise.resolve<QueryResult>({
          columns: ["id", "name"], rows: [["9", "z"]], total_rows: 1, offset: args.offset,
        });
      },
    });
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];

    const stale = h.query.reloadData();
    await flush(); // the stale reload is now awaiting its first chunk
    const newer = h.query.reloadData(); // newer reload wins
    await newer;
    expect(h.state.totalRows).toBe(1);

    resolveFirst({ columns: ["id", "name"], rows: [["1", "a"], ["2", "b"]], total_rows: 999, offset: 0 });
    await stale;

    expect(h.state.totalRows).toBe(1);
    expect(h.errors).toEqual([]);
  });

  it("ignores a deferred row count from a superseded reload", async () => {
    // count_rows is fired without awaiting, so its answer can arrive after the
    // user has changed the filter. It must not overwrite the new count.
    let resolveCount!: (value: number) => void;
    const pendingCount = new Promise<number>((resolve) => { resolveCount = resolve; });
    let countCall = 0;
    const h = harness({
      queryTable: (args) =>
        Promise.resolve<QueryResult>({
          columns: ["id", "name"],
          rows: [["1", "a"], ["2", "b"]],
          total_rows: null, // deferred count
          offset: args.offset,
        }),
      countRows: () => (++countCall === 1 ? pendingCount : Promise.resolve(7)),
    });
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];

    await h.query.reloadData();
    expect(h.state.countPending).toBe(true);

    await h.query.reloadData(); // a newer reload takes over
    resolveCount(999999);
    await flush();

    expect(h.state.totalRows).toBe(7);
  });

  it("does nothing at all with no table selected", async () => {
    const h = harness();
    await h.query.reloadData();
    expect(h.queries).toEqual([]);
    expect(h.state.loading).toBe(false);
  });
});

describe("createBrowseQuery.debouncedReload", () => {
  it("coalesces keystrokes into one query and dedupes an unchanged filter", async () => {
    vi.useFakeTimers();
    try {
      const h = harness();
      h.state.selectedTable = "items";
      h.state.columns = ["id", "name"];

      h.state.globalFilter = "a";
      h.query.debouncedReload();
      h.state.globalFilter = "ab";
      h.query.debouncedReload();
      h.state.globalFilter = "abc";
      h.query.debouncedReload();
      expect(h.queries).toEqual([]);

      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toHaveLength(1);
      expect(h.queries[0].globalFilter).toBe("abc");

      // Same filter again: no second round trip.
      h.query.debouncedReload();
      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not query while a filter holds a bare operator", async () => {
    vi.useFakeTimers();
    try {
      const h = harness();
      h.state.selectedTable = "items";
      h.state.columns = ["id", "name"];
      h.state.columnFilters = { id: { value: ">", is_regex: false } };
      h.query.debouncedReload();
      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("cancelDebounce drops a pending reload", async () => {
    vi.useFakeTimers();
    try {
      const h = harness();
      h.state.selectedTable = "items";
      h.state.columns = ["id", "name"];
      h.state.globalFilter = "z";
      h.query.debouncedReload();
      h.query.cancelDebounce();
      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("createBrowseQuery.handleSort", () => {
  it("toggles direction on the same column, resets to ascending on a new one, and persists both", async () => {
    const h = harness();
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];

    h.query.handleSort("name");
    expect(h.state.sortColumn).toBe("name");
    expect(h.state.sortAsc).toBe(true);
    await flush();

    h.query.handleSort("name");
    expect(h.state.sortAsc).toBe(false);
    await flush();

    h.query.handleSort("id");
    expect(h.state.sortColumn).toBe("id");
    expect(h.state.sortAsc).toBe(true);
    await flush();
    expect(h.configs.items.sort_column).toBe("id");
    expect(h.configs.items.sort_asc).toBe(true);
    expect(h.queries).toHaveLength(3);
  });

  it("strips a half-typed operator out of the query a sort click issues", async () => {
    // A bare ">" in a filter cell must not leave the grid ordered one way while
    // the header shows the other, so the sort reloads with the valid segments.
    const h = harness();
    h.state.selectedTable = "items";
    h.state.columns = ["id", "name"];
    h.state.columnFilters = { id: { value: ">10;>", is_regex: false } };

    h.query.handleSort("name");
    await flush();

    expect(h.query.buildFilters()).toEqual([
      { column: "id", value: ">10", is_regex: false },
    ]);
  });
});

describe("createBrowseQuery.resetForNewDatabase", () => {
  it("clears the debounce memo so the same filter re-queries in the new database", async () => {
    vi.useFakeTimers();
    try {
      const h = harness();
      h.state.selectedTable = "items";
      h.state.columns = ["id", "name"];
      h.state.globalFilter = "q";
      h.query.debouncedReload();
      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toHaveLength(1);

      h.query.resetForNewDatabase();
      h.state.selectedTable = "items";
      h.query.debouncedReload();
      await vi.advanceTimersByTimeAsync(500);
      expect(h.queries).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
