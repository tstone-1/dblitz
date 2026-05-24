import { beforeEach, describe, expect, it } from "vitest";
import { loadSqlHistory, loadTheme } from "./store.svelte";

describe("store localStorage loaders", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    storage.clear();
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        localStorage: {
          clear: () => storage.clear(),
          getItem: (key: string) => storage.get(key) ?? null,
          removeItem: (key: string) => {
            storage.delete(key);
          },
          setItem: (key: string, value: string) => {
            storage.set(key, value);
          },
        },
      },
    });
  });

  it("recovers from corrupt SQL history without throwing", () => {
    window.localStorage.setItem("dblitz-sql-history", "{bad json");

    expect(loadSqlHistory()).toEqual([]);
    expect(window.localStorage.getItem("dblitz-sql-history")).toBeNull();
  });

  it("filters malformed SQL history entries", () => {
    window.localStorage.setItem(
      "dblitz-sql-history",
      JSON.stringify([
        { sql: "SELECT 1", timestamp: 1, error: false },
        { sql: "SELECT 2", timestamp: "bad", error: false },
        null,
      ]),
    );

    expect(loadSqlHistory()).toEqual([{ sql: "SELECT 1", timestamp: 1, error: false }]);
  });

  it("only accepts supported theme values", () => {
    window.localStorage.setItem("dblitz-theme", "solarized");
    expect(loadTheme()).toBe("light");

    window.localStorage.setItem("dblitz-theme", "dark");
    expect(loadTheme()).toBe("dark");
  });
});
