/**
 * Browse Data's query state machine.
 *
 * Extracted from `BrowseData.svelte` in the style of `pinnedFilters.svelte.ts`:
 * the component keeps its `$state` declarations and injects a getter/setter for
 * each, so this file owns the LOGIC and none of the reactive plumbing. That
 * boundary is what makes the two invariants below reachable by a test at all --
 * inside the component they were unreachable, and both had already been broken
 * once each.
 *
 * The two invariants, and why each is here rather than in the component:
 *
 *   * **Every post-await consumer re-checks ownership.** `reloadData` runs
 *     several awaits deep and must publish nothing -- not rows, not a row
 *     count, not an error, not `loading = false` -- once a newer reload has
 *     started. It also queries with the snapshot pinned at `beginReload`, never
 *     with whatever the component holds by then.
 *
 *   * **A failed first chunk zeroes the row count.** `beginReload` empties the
 *     cache but leaves the caller's `totalRows` on the previous query's value,
 *     so a failing filter (a regex of `(` returns "Invalid regex") left the
 *     grid rendering that many blank rows, each of which called getRow, missed,
 *     and re-fired the failing query -- indefinitely. `virtualRows`'
 *     `failFirstChunk` zeroes the count and latches chunk 0.
 */

import type {
  ColumnFilter,
  ColumnFilterValue,
  QueryResult,
  ViewConfig,
} from "#lib/ipc.ts";
import { buildActiveFilters } from "./columnView";
import { hasIncompleteOperator, stripIncompleteSegments } from "./filterOperators";
import { shouldAutoFitWidths } from "./autoFitWidths";
import { createVirtualRows, isCancelledQueryError } from "./virtualRows.svelte";

/**
 * An immutable snapshot of the query state (table + filters + sort) taken at
 * reload time. virtualRows pins it per-epoch and feeds it back to loadChunk so
 * a background chunk fetch queries the epoch's state, never whatever the
 * component holds by the time the fetch fires.
 */
export interface QuerySnapshot {
  table: string | null;
  filters: ColumnFilter[];
  globalFilter: string;
  sortColumn: string | null;
  sortAsc: boolean;
}

export interface BrowseQueryDeps {
  chunkSize: number;
  filterDebounceMs: number;

  // ---- the component's reactive state, injected ---------------------------
  getSelectedTable: () => string | null;
  setSelectedTable: (table: string | null) => void;
  getColumns: () => string[];
  setColumns: (columns: string[]) => void;
  setTotalRows: (total: number) => void;
  getGlobalFilter: () => string;
  setGlobalFilter: (value: string) => void;
  getColumnFilters: () => Record<string, ColumnFilterValue>;
  setColumnFilters: (filters: Record<string, ColumnFilterValue>) => void;
  getSortColumn: () => string | null;
  setSortColumn: (column: string | null) => void;
  getSortAsc: () => boolean;
  setSortAsc: (asc: boolean) => void;
  setLoading: (loading: boolean) => void;
  setCountPending: (pending: boolean) => void;
  setError: (message: string) => void;

  // ---- view plumbing -------------------------------------------------------
  /** Visible (ordered, unhidden) column names, for row projection. */
  getVisibleColumns: () => string[];
  /** Index of a column in the raw result row. */
  getColumnIndex: (column: string) => number | undefined;
  /** Column names cached at open time, so filters survive the first query. */
  getCachedTableColumns: (table: string) => string[] | undefined;

  // ---- persisted config ----------------------------------------------------
  getTableConfig: (table: string) => ViewConfig;
  ensureTableConfig: (table: string) => ViewConfig;
  updateTableConfig: (table: string, mutate: (cfg: ViewConfig) => void) => void;

  // ---- IPC -----------------------------------------------------------------
  queryTable: (args: {
    table: string;
    offset: number;
    limit: number;
    filters: ColumnFilter[];
    globalFilter: string;
    sortColumn: string | null;
    sortAsc: boolean;
  }) => Promise<QueryResult>;
  countRows: (args: {
    table: string;
    filters: ColumnFilter[];
    globalFilter: string;
  }) => Promise<number>;
  cancelQueries: () => Promise<void>;

  /** Lets the caller's DOM settle before widths are measured. */
  tick: () => Promise<void>;
  /** Auto-fit and persist widths for a table opened without saved ones. */
  applyAutoWidths: () => void;
  /** Injectable one-tick deferral for background chunk requests (tests). */
  defer?: (run: () => void) => void;
}

export function createBrowseQuery(deps: BrowseQueryDeps) {
  // Plain `let` on purpose — this is a deduplication memo for debouncedReload,
  // not reactive state. Tracking it via `$state` would defeat the dedup (every
  // read/write would trigger downstream effects).
  let lastFilterState = "";
  let filterDebounce: ReturnType<typeof setTimeout> | null = null;

  function buildFilters(): ColumnFilter[] {
    // Drop filters for columns that no longer exist in the schema
    // (e.g. a pinned filter on a column that was renamed externally), and
    // strip bare half-typed operator segments (">" with no operand) from
    // non-regex values so a reload triggered by a discrete action (a sort
    // click) queries with the still-valid segments instead of a broken filter.
    const cleaned: Record<string, ColumnFilterValue> = {};
    for (const [col, f] of Object.entries(deps.getColumnFilters())) {
      cleaned[col] = f.is_regex
        ? f
        : { ...f, value: stripIncompleteSegments(f.value) };
    }
    return buildActiveFilters(deps.getColumns(), cleaned);
  }

  function makeSnapshot(): QuerySnapshot {
    return {
      table: deps.getSelectedTable(),
      filters: buildFilters(),
      globalFilter: deps.getGlobalFilter().trim(),
      sortColumn: deps.getSortColumn(),
      sortAsc: deps.getSortAsc(),
    };
  }

  function loadChunk(
    offset: number,
    limit: number,
    snapshot: QuerySnapshot,
  ): Promise<QueryResult> {
    // virtualRows also captures a snapshot when it resets with no table
    // selected, and a render pass in flight at that moment can still ask for a
    // row. `query_table`'s Rust `table` is a plain `String`, so forwarding the
    // null would fail Tauri's argument deserialization and surface as an opaque
    // error toast; an empty page is the honest answer. `total_rows: null` so
    // the caller's row count is left alone rather than being zeroed by a chunk
    // fetch belonging to no table.
    const table = snapshot.table;
    if (table === null) {
      return Promise.resolve({ columns: [], rows: [], total_rows: null, offset });
    }
    return deps.queryTable({
      table,
      offset,
      limit,
      filters: snapshot.filters,
      globalFilter: snapshot.globalFilter,
      sortColumn: snapshot.sortColumn,
      sortAsc: snapshot.sortAsc,
    });
  }

  const virtualRows = createVirtualRows<QuerySnapshot>({
    chunkSize: deps.chunkSize,
    getSelectedTable: deps.getSelectedTable,
    makeSnapshot,
    loadChunk,
    cancelQueries: () => deps.cancelQueries(),
    getVisibleColumns: deps.getVisibleColumns,
    getColumnIndex: deps.getColumnIndex,
    hasColumns: () => deps.getColumns().length > 0,
    setColumns: deps.setColumns,
    setTotalRows: deps.setTotalRows,
    setError: deps.setError,
    defer: deps.defer,
  });

  /**
   * Runs `run`, and once more if it was stopped by `cancel_queries` while this
   * epoch is still current.
   *
   * `cancel_queries` is shared by Browse and the SQL tab, so pressing Cancel on
   * a SQL query also stops a Browse first chunk or row count that happens to
   * be in flight. With the epoch still current nobody asked to stop THIS
   * query, and failing it zeroed the view under a red error bar. A cancel that
   * Browse issued itself comes from a newer reload, which has already moved
   * the epoch, so it is not retried. One retry only: a second cancel is
   * reported as the error it then is, rather than looping.
   */
  async function retryAfterSharedCancel<T>(
    myEpoch: number,
    run: () => Promise<T>,
  ): Promise<T> {
    try {
      return await run();
    } catch (e) {
      if (!virtualRows.isCurrent(myEpoch) || !isCancelledQueryError(e)) throw e;
      return run();
    }
  }

  /**
   * Resolves true only when this reload published its first chunk and is
   * still the current one when it returns. selectTable's auto-fit tail keys on
   * that: a superseded reload has no rows of its own to measure.
   */
  async function reloadData(): Promise<boolean> {
    if (!deps.getSelectedTable()) return false;
    deps.setLoading(true);
    const reload = await virtualRows.beginReload();
    if (reload === null) return false;
    const { epoch: myEpoch, snapshot } = reload;
    // beginReload() captures the snapshot synchronously, before its first
    // await, so it pins the non-null selectedTable guarded above. This branch
    // is therefore unreachable; it exists to hand countRows the plain `string`
    // its Rust signature requires without an assertion (see ipc.ts).
    const table = snapshot.table;
    if (table === null) {
      deps.setLoading(false);
      return false;
    }
    let published = false;
    try {
      // Use the epoch's pinned snapshot for the first chunk AND the row count
      // so both agree with the background chunk fetches virtualRows will run.
      const result = await retryAfterSharedCancel(myEpoch, () =>
        loadChunk(0, deps.chunkSize, snapshot),
      );
      if (!virtualRows.applyFirstChunk(myEpoch, result)) return false;
      published = true;

      if (result.total_rows !== null) {
        deps.setTotalRows(result.total_rows);
        deps.setCountPending(false);
      } else {
        deps.setTotalRows(
          result.rows.length < deps.chunkSize ? result.rows.length : deps.chunkSize,
        );
        deps.setCountPending(true);
        retryAfterSharedCancel(myEpoch, () =>
          deps.countRows({
            table,
            filters: snapshot.filters,
            globalFilter: snapshot.globalFilter,
          }),
        )
          .then((count) => {
            if (virtualRows.isCurrent(myEpoch)) {
              deps.setTotalRows(count);
              deps.setCountPending(false);
            }
          })
          .catch((e: unknown) => {
            if (virtualRows.isCurrent(myEpoch)) {
              deps.setCountPending(false);
              deps.setError(String(e));
            }
          });
      }

      await deps.tick();
    } catch (e) {
      // See the file header: the previous query's row count would otherwise
      // keep the grid rendering blank rows that re-fire the failing query.
      if (virtualRows.failFirstChunk(myEpoch)) {
        deps.setCountPending(false);
        deps.setError(String(e));
      }
    } finally {
      if (virtualRows.isCurrent(myEpoch)) deps.setLoading(false);
    }
    return published && virtualRows.isCurrent(myEpoch);
  }

  function hasIncompleteFilter(): boolean {
    // Segment/regex logic lives in filterOperators.ts (pure + tested).
    return Object.values(deps.getColumnFilters()).some((f) =>
      hasIncompleteOperator(f.value, f.is_regex),
    );
  }

  function filterMemoKey(): string {
    return deps.getGlobalFilter().trim() + JSON.stringify(deps.getColumnFilters());
  }

  function debouncedReload(): void {
    if (hasIncompleteFilter()) return;
    if (filterMemoKey() === lastFilterState) {
      // The edit was undone inside the debounce window (`10` -> `100` -> `10`):
      // the loaded view already matches, so the armed timer must go too. Left
      // armed, it used to record the `100` it was armed with as loaded while
      // reloadData() queried the live `10`, and retyping `100` then matched the
      // memo and never queried: the grid showed `10`'s rows under `100`.
      cancelDebounce();
      return;
    }
    cancelDebounce();
    filterDebounce = setTimeout(() => {
      filterDebounce = null;
      // The memo records what reloadData() is about to query, which is the
      // live state at fire time, not the state the timer was armed with.
      lastFilterState = filterMemoKey();
      void reloadData();
    }, deps.filterDebounceMs);
  }

  function handleSort(col: string): void {
    if (deps.getSortColumn() === col) {
      deps.setSortAsc(!deps.getSortAsc());
    } else {
      deps.setSortColumn(col);
      deps.setSortAsc(true);
    }
    const table = deps.getSelectedTable();
    if (table) {
      deps.updateTableConfig(table, (cfg) => {
        cfg.sort_column = deps.getSortColumn();
        cfg.sort_asc = deps.getSortAsc();
      });
    }
    // Always reload after a sort click. buildFilters() strips any half-typed
    // operator segment, so a bare operator in a filter cell can't leave the
    // grid persistently ordered one way while the header shows the other.
    void reloadData();
  }

  async function selectTable(name: string): Promise<void> {
    // Cancel any pending debounced reload from the outgoing table so it can't
    // fire against the incoming one and waste a round-trip.
    cancelDebounce();
    deps.setSelectedTable(name);
    // Pre-populate columns from the openDatabase-time autocomplete cache
    // so buildFilters() (called by reloadData below) sees the schema BEFORE
    // the first query result arrives. Without this, filters are dropped on
    // the very first query after a table switch because `valid` is empty.
    deps.setColumns(deps.getCachedTableColumns(name) ?? []);
    const cfg = deps.ensureTableConfig(name);
    if (cfg.sort_column && !deps.getColumns().includes(cfg.sort_column)) {
      deps.updateTableConfig(name, (tableCfg) => {
        tableCfg.sort_column = null;
        tableCfg.sort_asc = true;
      });
    }
    deps.setSortColumn(deps.getTableConfig(name).sort_column);
    deps.setSortAsc(deps.getTableConfig(name).sort_asc);
    // Hydrate ephemeral filter state from pinned defaults.
    // Orphaned filters (pinned column no longer in schema) are silently
    // dropped at query time by buildFilters() against the live `columns`.
    deps.setColumnFilters(
      Object.fromEntries(
        Object.entries(cfg.pinned_filters).map(([col, pf]) => [
          col,
          { value: pf.value, is_regex: pf.is_regex },
        ]),
      ),
    );
    deps.setGlobalFilter(cfg.pinned_global_filter ?? "");
    lastFilterState = filterMemoKey();

    const published = await reloadData();

    // A superseded reload has no rows to measure. Comparing table names below
    // cannot see that when the same table is selected twice during its first
    // load (a double-click on a slow table): the first call's tail passed the
    // name check, found no saved widths yet and auto-fitted on an empty row
    // cache, saving header-only widths that the second call then respected.
    if (!published) return;

    // Auto-fit column widths on first open (no saved widths for this table),
    // but only while this call is still the current selection. selectTable()
    // can run again while the await above is in flight -- click table A (no
    // saved widths, slow first load), then table B before A's first chunk
    // lands -- and applyAutoWidths() measures and persists through the LIVE
    // selected table, not `name`. A's resumed tail therefore wrote auto-fit
    // widths taken from B's grid (or from B's bare headers, if B's own chunk
    // had not arrived either) into B's config, silently discarding the widths
    // the user had hand-tuned there. Every other post-await consumer in this
    // file is epoch-guarded via virtualRows.isCurrent(); this tail has no
    // epoch, so it re-reads the selection instead.
    if (
      !shouldAutoFitWidths({
        requestedTable: name,
        currentTable: deps.getSelectedTable(),
        savedWidths: deps.getTableConfig(name).column_widths,
      })
    ) {
      return;
    }
    deps.applyAutoWidths();
  }

  function cancelDebounce(): void {
    if (filterDebounce) {
      clearTimeout(filterDebounce);
      filterDebounce = null;
    }
  }

  /**
   * Drop everything this machine caches about the outgoing database. The
   * component still owns clearing its own reactive fields; this is the part
   * that lives here.
   */
  function resetForNewDatabase(): void {
    lastFilterState = "";
    cancelDebounce();
    virtualRows.reset();
  }

  return {
    virtualRows,
    buildFilters,
    makeSnapshot,
    reloadData,
    debouncedReload,
    hasIncompleteFilter,
    handleSort,
    selectTable,
    cancelDebounce,
    resetForNewDatabase,
    /** Test-only view of the debounce memo. */
    get lastFilterState() {
      return lastFilterState;
    },
  };
}
