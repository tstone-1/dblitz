// @vitest-environment jsdom
import { flushSync, mount, tick, unmount } from "svelte";
import { expect, it, vi } from "vitest";
import { getColumns } from "#lib/ipc.ts";
import DatabaseStructure from "./DatabaseStructure.svelte";

vi.mock("#lib/store.svelte.ts", () => ({
  appState: {
    dbPath: "/example.sqlite",
    dbOpenGeneration: 1,
    tables: [{ name: "alpha", row_count: 1 }, { name: "beta", row_count: 1 }],
  },
}));
vi.mock("#lib/ipc.ts", () => ({ getSchema: async () => [], getColumns: vi.fn() }));

it("clears the rendered columns as soon as another table is selected, including on failure", async () => {
  const column = { cid: 0, col_type: "INTEGER", pk: true, notnull: true, default_value: null };
  const pending = Promise.withResolvers<Awaited<ReturnType<typeof getColumns>>>();
  vi.mocked(getColumns)
    .mockResolvedValueOnce([{ ...column, name: "alpha_id" }])
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValueOnce([{ ...column, name: "beta_id" }]);
  const target = document.createElement("div");
  const component = mount(DatabaseStructure, { target });
  try {
    flushSync();
    const [alpha, beta] = target.querySelectorAll<HTMLButtonElement>(".table-item");
    alpha.click();
    await vi.waitFor(() => expect(target.querySelector(".col-name")?.textContent).toBe("alpha_id"));
    beta.click();
    await tick();
    expect(target.querySelector(".column-list .section-title")?.textContent).toContain("beta");
    expect(target.querySelector(".col-name")).toBeNull();
    expect(target.querySelector("[role=status]")?.textContent).toBe("Loading columns...");
    pending.reject(new Error("synthetic read failure"));
    await vi.waitFor(() => expect(target.querySelector("[role=status]")?.textContent).toBe("Could not load columns."));
    expect(target.querySelector(".col-name")).toBeNull();
    beta.click();
    await vi.waitFor(() => expect(target.querySelector(".col-name")?.textContent).toBe("beta_id"));
  } finally {
    await unmount(component);
  }
});
