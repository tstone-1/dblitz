import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { appState, loadSqlHistory, loadTheme, saveViewConfig } from "./store.svelte";

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

// W15: saveViewConfig() used to swallow failures into console.error only,
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
