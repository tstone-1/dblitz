// @vitest-environment jsdom
import { flushSync, mount, unmount } from "svelte";
import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { appState } from "#lib/store.svelte.ts";
import BrowseData from "./BrowseData.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

it("opens a constructor table and filters its constructor column", async () => {
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    measureText: () => ({ width: 30 }),
  } as unknown as CanvasRenderingContext2D);
  vi.mocked(invoke).mockImplementation(async (command: string) => {
    if (command === "query_table") return { columns: ["constructor"], rows: [["alpha"]], total_rows: 1, offset: 0 };
    if (command === "count_rows") return 1;
  });
  appState.dbOpenGeneration++;
  appState.dbPath = "example.sqlite";
  appState.tables = [{ name: "constructor", row_count: 1 }];
  appState.tableColumns = { ["constructor"]: ["constructor"] };
  appState.fileConfig.tables = {};
  const target = document.createElement("div");
  const view = mount(BrowseData, { target });
  try {
    flushSync();
    await vi.waitFor(() => expect(target.querySelector(".data-cell")?.textContent?.trim()).toBe("alpha"));
    const input = target.querySelector<HTMLInputElement>(".col-filter-input")!;
    input.value = "alpha";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLButtonElement>(".regex-toggle")!.click();
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("query_table", expect.objectContaining({
      filters: [{ column: "constructor", value: "alpha", is_regex: true }],
    })), { timeout: 2500 });
  } finally {
    await unmount(view);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  }
});
