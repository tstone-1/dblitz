import { beforeEach, describe, expect, it } from "vitest";
import { appState } from "$lib/store.svelte";
import type { TableInfo } from "$lib/ipc";
import { createAutoSelectFirstTable } from "./autoSelectFirstTable.svelte";

describe("createAutoSelectFirstTable", () => {
  beforeEach(() => {
    appState.dbPath = null;
    appState.tables = [];
    appState.dbOpenGeneration = 0;
  });

  function table(name: string, row_count = 0): TableInfo {
    return { name, row_count };
  }

  it("auto-selects the lone table exactly once per open generation", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    appState.tables = [table("only_table")];
    check();
    check(); // re-checking the same generation must not re-fire

    expect(selected).toEqual(["only_table"]);
  });

  it("does not auto-select for multi-table databases", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    appState.tables = [table("t1"), table("t2")];
    check();

    expect(selected).toEqual([]);
  });

  it("re-fires for a newly opened database even if it also has one table", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    appState.tables = [table("t1")];
    check();

    appState.dbOpenGeneration = 2;
    appState.dbPath = "/b.db";
    appState.tables = [table("t2")];
    check();

    expect(selected).toEqual(["t1", "t2"]);
  });

  it("re-fires when the SAME single-table path is reopened (generation bumps, path does not)", () => {
    // Reopening the already-open file is the natural refresh gesture: the
    // reset that reopen triggers clears the selection, so the lone table must
    // be auto-selected again even though dbPath never changed.
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    appState.tables = [table("only_table")];
    check();

    appState.dbOpenGeneration = 2; // same path reopened
    check();

    expect(selected).toEqual(["only_table", "only_table"]);
  });

  it("calls onReset once per close, not on every check while closed", () => {
    // Repeated firing is not merely wasteful: both consumers call check() from a
    // $effect, and BrowseData's onReset writes state that its own reset path
    // reads back, so an effect that re-fired it every run invalidated itself
    // until Svelte aborted with `effect_update_depth_exceeded`.
    let resetCalls = 0;
    const check = createAutoSelectFirstTable(
      () => {},
      () => { resetCalls++; },
    );

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    appState.tables = [];
    check();
    expect(resetCalls).toBe(0);

    appState.dbPath = null;
    check();
    check();
    check();
    expect(resetCalls).toBe(1);
  });

  it("does not reset on mount, when no database has been open yet", () => {
    // A freshly mounted component is already in the closed state; there is
    // nothing to clear, and clearing anyway is what started the effect loop.
    let resetCalls = 0;
    const check = createAutoSelectFirstTable(
      () => {},
      () => { resetCalls++; },
    );

    check();
    check();

    expect(resetCalls).toBe(0);
  });

  it("resets again on a second close", () => {
    // The latch must unlatch while a database is open, or only the first close
    // would ever clear the view.
    let resetCalls = 0;
    const check = createAutoSelectFirstTable(
      () => {},
      () => { resetCalls++; },
    );

    appState.dbOpenGeneration = 1;
    appState.dbPath = "/a.db";
    check();
    appState.dbPath = null;
    check();
    expect(resetCalls).toBe(1);

    appState.dbOpenGeneration = 2;
    appState.dbPath = "/b.db";
    check();
    appState.dbPath = null;
    check();
    expect(resetCalls).toBe(2);
  });
});
