import { describe, expect, it } from "vitest";
import type { ColumnFilterValue, ViewConfig } from "$lib/ipc";
import { createPinnedFilters } from "./pinnedFilters.svelte";

// A plain in-memory stand-in for the store's getTableConfig / updateTableConfig.
// pinnedFilters is fully injected, so its tests need no store and no mocked
// invoke -- just fakes, like its cellSelection / dragReorder siblings.
function makeConfigStore(seed: Record<string, ViewConfig> = {}) {
  const tables: Record<string, ViewConfig> = { ...seed };
  const blank = (): ViewConfig => ({
    hidden_columns: [],
    column_colors: {},
    sort_column: null,
    sort_asc: true,
    column_order: [],
    pinned_filters: {},
    pinned_global_filter: null,
    column_widths: {},
  });
  const getConfig = (t: string): ViewConfig => tables[t] ?? blank();
  const updateConfig = (
    t: string,
    mutate: (cfg: ViewConfig) => void,
  ): ViewConfig => {
    const cfg = tables[t] ?? blank();
    mutate(cfg);
    tables[t] = cfg;
    return cfg;
  };
  return { tables, getConfig, updateConfig };
}

describe("createPinnedFilters", () => {
  it("pins and unpins a column filter", () => {
    const store = makeConfigStore();
    let columnFilters: Record<string, ColumnFilterValue> = {
      name: { value: "alice", is_regex: false },
    };
    const pinned = createPinnedFilters({
      getSelectedTable: () => "users",
      getColumnFilters: () => columnFilters,
      setColumnFilters: (next) => { columnFilters = next; },
      getGlobalFilter: () => "",
      setGlobalFilter: () => {},
      triggerReload: () => {},
      getConfig: store.getConfig,
      updateConfig: store.updateConfig,
    });

    pinned.togglePinColumnFilter("name");

    expect(store.tables.users.pinned_filters.name).toEqual({
      value: "alice",
      is_regex: false,
    });
    expect(pinned.pinStates.name).toBe("pinned");

    pinned.togglePinColumnFilter("name");

    expect(store.tables.users.pinned_filters.name).toBeUndefined();
  });

  it("reverts a modified column filter and triggers reload", () => {
    const store = makeConfigStore({
      users: {
        hidden_columns: [],
        column_colors: {},
        sort_column: null,
        sort_asc: true,
        column_order: [],
        pinned_filters: { name: { value: "saved", is_regex: true } },
        pinned_global_filter: null,
        column_widths: {},
      },
    });
    let reloads = 0;
    const columnFilters: Record<string, ColumnFilterValue> = {
      name: { value: "edited", is_regex: false },
    };
    const pinned = createPinnedFilters({
      getSelectedTable: () => "users",
      getColumnFilters: () => columnFilters,
      setColumnFilters: () => {},
      getGlobalFilter: () => "",
      setGlobalFilter: () => {},
      triggerReload: () => { reloads += 1; },
      getConfig: store.getConfig,
      updateConfig: store.updateConfig,
    });

    pinned.revertColumnFilter("name");

    expect(columnFilters.name).toEqual({ value: "saved", is_regex: true });
    expect(reloads).toBe(1);
  });

  it("shift reset clears live filters and saved pinned defaults", () => {
    const store = makeConfigStore({
      users: {
        hidden_columns: [],
        column_colors: {},
        sort_column: null,
        sort_asc: true,
        column_order: [],
        pinned_filters: { name: { value: "saved", is_regex: false } },
        pinned_global_filter: "global",
        column_widths: {},
      },
    });
    let columnFilters: Record<string, ColumnFilterValue> = {
      name: { value: "edited", is_regex: false },
    };
    let globalFilter = "edited global";
    let reloads = 0;
    const pinned = createPinnedFilters({
      getSelectedTable: () => "users",
      getColumnFilters: () => columnFilters,
      setColumnFilters: (next) => { columnFilters = next; },
      getGlobalFilter: () => globalFilter,
      setGlobalFilter: (next) => { globalFilter = next; },
      triggerReload: () => { reloads += 1; },
      getConfig: store.getConfig,
      updateConfig: store.updateConfig,
    });

    pinned.handleResetClick({ shiftKey: true } as MouseEvent);

    expect(columnFilters).toEqual({});
    expect(globalFilter).toBe("");
    expect(store.tables.users.pinned_filters).toEqual({});
    expect(store.tables.users.pinned_global_filter).toBeNull();
    expect(reloads).toBe(1);
  });
});
