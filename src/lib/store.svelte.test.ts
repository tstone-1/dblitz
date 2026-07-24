import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { appState, loadSqlHistory, loadTheme, openDatabase, saveViewConfig } from "./store.svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

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

// saveViewConfig() used to swallow failures into console.error only,
// leaving a broken/read-only config path with zero on-screen signal.
describe("saveViewConfig failure notice", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    appState.notice = null;
  });

  it("surfaces the first failure via appState.notice but does not re-notify on a second failure", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("disk full"));
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    await saveViewConfig();
    expect(appState.notice).toBe("View settings could not be saved: Error: disk full");
    expect(errorSpy).toHaveBeenCalledTimes(1);

    // Simulate the user dismissing the notice, then a second save failure.
    appState.notice = null;
    await saveViewConfig();

    // console.error still fires every time (nothing is swallowed silently),
    // but the notice bar is a once-per-session nag, not a per-keystroke one.
    expect(errorSpy).toHaveBeenCalledTimes(2);
    expect(appState.notice).toBeNull();

    errorSpy.mockRestore();
  });
});

describe("database open sequencing", () => {
  it("lets only the latest overlapping open request publish state", async () => {
    let resolveFirst!: (tables: { name: string; row_count: number }[]) => void;
    const firstOpen = new Promise<{ name: string; row_count: number }[]>((resolve) => {
      resolveFirst = resolve;
    });
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === "open_database") {
        return (args as { path: string }).path === "A.db"
          ? firstOpen
          : Promise.resolve([{ name: "b_table", row_count: 1 }]);
      }
      if (command === "load_view_config") {
        return Promise.resolve({ tables: {}, tint: null, label: null });
      }
      if (command === "get_columns") {
        return Promise.resolve([{ cid: 0, name: "b_col", col_type: "TEXT", notnull: false, default_value: null, pk: false }]);
      }
      return Promise.resolve(undefined);
    });

    const openingA = openDatabase("A.db");
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("open_database", { path: "A.db" }));
    const openingB = openDatabase("B.db");
    resolveFirst([{ name: "a_table", row_count: 1 }]);
    await Promise.all([openingA, openingB]);

    expect(appState.dbPath).toBe("B.db");
    expect(appState.tables).toEqual([{ name: "b_table", row_count: 1 }]);
    expect(appState.tableColumns).toEqual({ b_table: ["b_col"] });
  });
});

// Reopening the already-open file reopens the backend connection (clearing its
// caches) but leaves dbPath unchanged, so dbPath alone can't tell the frontend
// to drop its stale caches. dbOpenGeneration is the signal that DOES bump on
// every successful open, same-path included.
describe("database open generation", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((command) => {
      if (command === "open_database") return Promise.resolve([{ name: "t", row_count: 1 }]);
      if (command === "load_view_config") return Promise.resolve({ tables: {}, tint: null, label: null });
      if (command === "get_columns") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });
  });

  it("bumps dbOpenGeneration on every successful open, reopening the same path included", async () => {
    const before = appState.dbOpenGeneration;

    await openDatabase("A.db");
    expect(appState.dbOpenGeneration).toBe(before + 1);
    expect(appState.dbPath).toBe("A.db");

    await openDatabase("A.db"); // same path -- still a fresh session
    expect(appState.dbOpenGeneration).toBe(before + 2);
    expect(appState.dbPath).toBe("A.db");
  });
});
