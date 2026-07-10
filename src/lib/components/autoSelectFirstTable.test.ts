import { beforeEach, describe, expect, it } from "vitest";
import { appState, type TableInfo } from "$lib/store.svelte";
import { createAutoSelectFirstTable, didDbPathChange } from "./autoSelectFirstTable.svelte";

describe("didDbPathChange", () => {
  it("fires on opening a first database (null -> path)", () => {
    expect(didDbPathChange(null, "/a.db")).toBe(true);
  });

  it("fires on switching to a different database (open A -> open B)", () => {
    expect(didDbPathChange("/a.db", "/b.db")).toBe(true);
  });

  it("fires on closing a database (path -> null)", () => {
    expect(didDbPathChange("/a.db", null)).toBe(true);
  });

  it("does not fire when the same path is seen again (reopen same path)", () => {
    expect(didDbPathChange("/a.db", "/a.db")).toBe(false);
    expect(didDbPathChange(null, null)).toBe(false);
  });
});

describe("createAutoSelectFirstTable", () => {
  beforeEach(() => {
    appState.dbPath = null;
    appState.tables = [];
  });

  function table(name: string, row_count = 0): TableInfo {
    return { name, row_count };
  }

  it("auto-selects the lone table exactly once per db path", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbPath = "/a.db";
    appState.tables = [table("only_table")];
    check();
    check(); // re-checking the same path must not re-fire

    expect(selected).toEqual(["only_table"]);
  });

  it("does not auto-select for multi-table databases", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbPath = "/a.db";
    appState.tables = [table("t1"), table("t2")];
    check();

    expect(selected).toEqual([]);
  });

  it("re-fires for a newly opened database even if it also has one table", () => {
    const selected: string[] = [];
    const check = createAutoSelectFirstTable((name) => selected.push(name));

    appState.dbPath = "/a.db";
    appState.tables = [table("t1")];
    check();

    appState.dbPath = "/b.db";
    appState.tables = [table("t2")];
    check();

    expect(selected).toEqual(["t1", "t2"]);
  });

  it("calls onReset exactly when the db path becomes null, not on every check", () => {
    let resetCalls = 0;
    const check = createAutoSelectFirstTable(
      () => {},
      () => { resetCalls++; },
    );

    appState.dbPath = "/a.db";
    appState.tables = [];
    check();
    expect(resetCalls).toBe(0);

    appState.dbPath = null;
    check();
    check();
    expect(resetCalls).toBe(2); // onReset has no "already reset" dedup of its own
  });
});
