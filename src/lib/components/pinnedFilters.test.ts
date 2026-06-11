import { beforeEach, describe, expect, it, vi } from "vitest";
import { appState, type ColumnFilterValue } from "$lib/store.svelte";
import { createPinnedFilters } from "./pinnedFilters.svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

function resetConfig() {
  appState.fileConfig = { tables: {}, tint: null, label: null };
}

describe("createPinnedFilters", () => {
  beforeEach(() => {
    resetConfig();
  });

  it("pins and unpins a column filter", () => {
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
    });

    pinned.togglePinColumnFilter("name");

    expect(appState.fileConfig.tables.users.pinned_filters.name).toEqual({
      value: "alice",
      is_regex: false,
    });
    expect(pinned.pinStates.name).toBe("pinned");

    pinned.togglePinColumnFilter("name");

    expect(appState.fileConfig.tables.users.pinned_filters.name).toBeUndefined();
  });

  it("reverts a modified column filter and triggers reload", () => {
    appState.fileConfig.tables.users = {
      hidden_columns: [],
      column_colors: {},
      sort_column: null,
      sort_asc: true,
      column_order: [],
      pinned_filters: { name: { value: "saved", is_regex: true } },
      pinned_global_filter: null,
      column_widths: {},
    };
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
    });

    pinned.revertColumnFilter("name");

    expect(columnFilters.name).toEqual({ value: "saved", is_regex: true });
    expect(reloads).toBe(1);
  });

  it("shift reset clears live filters and saved pinned defaults", () => {
    appState.fileConfig.tables.users = {
      hidden_columns: [],
      column_colors: {},
      sort_column: null,
      sort_asc: true,
      column_order: [],
      pinned_filters: { name: { value: "saved", is_regex: false } },
      pinned_global_filter: "global",
      column_widths: {},
    };
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
    });

    pinned.handleResetClick({ shiftKey: true } as MouseEvent);

    expect(columnFilters).toEqual({});
    expect(globalFilter).toBe("");
    expect(appState.fileConfig.tables.users.pinned_filters).toEqual({});
    expect(appState.fileConfig.tables.users.pinned_global_filter).toBeNull();
    expect(reloads).toBe(1);
  });
});
