import { beforeEach, describe, expect, it } from "vitest";
import { loadSqlHistory, loadTheme } from "./store.svelte";

function createMemoryStorage(): Storage {
  let values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => { values = new Map(); },
    getItem: (key) => values.get(key) ?? null,
    key: (index) => Array.from(values.keys())[index] ?? null,
    removeItem: (key) => { values.delete(key); },
    setItem: (key, value) => { values.set(key, String(value)); },
  };
}

describe("store localStorage loading", () => {
  beforeEach(() => {
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: createMemoryStorage(),
    });
    window.localStorage.clear();
  });

  it("falls back to empty SQL history when localStorage is corrupt", () => {
    window.localStorage.setItem("dblitz-sql-history", "{not json");

    expect(loadSqlHistory()).toEqual([]);
  });

  it("filters malformed SQL history entries", () => {
    window.localStorage.setItem(
      "dblitz-sql-history",
      JSON.stringify([
        { sql: "select 1", timestamp: 1, error: false },
        { sql: "missing timestamp", error: false },
        null,
        { sql: "select 2", timestamp: 2, error: true },
      ]),
    );

    expect(loadSqlHistory()).toEqual([
      { sql: "select 1", timestamp: 1, error: false },
      { sql: "select 2", timestamp: 2, error: true },
    ]);
  });

  it("falls back to light theme for unknown stored values", () => {
    window.localStorage.setItem("dblitz-theme", "sepia");

    expect(loadTheme()).toBe("light");
  });

  it("loads the persisted dark theme", () => {
    window.localStorage.setItem("dblitz-theme", "dark");

    expect(loadTheme()).toBe("dark");
  });
});
