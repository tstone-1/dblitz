// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import { EditorView } from "@codemirror/view";
import Page from "../routes/+page.svelte";
import { appState } from "./store.svelte";
import { beginClipboardCopy } from "./components/clipboardTable";
import { executeSql } from "./ipc";

vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("#lib/ipc.ts", async (original) => ({
  ...await original<typeof import("./ipc")>(),
  getInitialFile: async () => null,
  validateSql: async () => ({ error: null }),
  executeSql: vi.fn(async () => ({ columns: [], rows: [], column_types: [], error: null, truncated: false })),
}));
vi.mock("./updaterCommands", async original => ({
  ...await original<typeof import("./updaterCommands")>(),
  updateStatus: async () => ({ currentVersion: "1", updated: false, selfUpdateSupported: false }),
  getCheckForUpdatesOnStartup: async () => false,
}));

let target: HTMLElement;
let page: ReturnType<typeof mount>;

beforeEach(() => {
  window.localStorage.clear();
  target = document.body.appendChild(document.createElement("div"));
  vi.stubGlobal("ResizeObserver", class { observe() {} unobserve() {} disconnect() {} });
  page = mount(Page, { target });
  flushSync();
});

afterEach(async () => {
  await unmount(page);
  target.remove();
  appState.dbPath = null;
  appState.activeTab = "structure";
  appState.sqlHistory = [];
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

it("executes the clicked history entry even when its editor text has a selection", async () => {
  const statement = "SELECT (SELECT 7) + 1";
  appState.dbPath = "example.db";
  appState.activeTab = "sql";
  appState.sqlHistory = [{ sql: statement, timestamp: 0, error: false }];
  window.localStorage.setItem("dblitz-sql-history", JSON.stringify(appState.sqlHistory));
  flushSync();
  await vi.waitFor(() => expect(target.querySelector(".cm-content")).toBeTruthy());
  target.querySelector<HTMLButtonElement>(".history-btn")!.click();
  flushSync();
  const entry = target.querySelector<HTMLButtonElement>(".history-entry")!;
  entry.click();
  flushSync();
  const view = EditorView.findFromDOM(target.querySelector(".cm-content")!)!;
  expect(view.state.doc.toString()).toBe(statement);
  view.dispatch({ selection: { anchor: 8, head: 16 } });
  flushSync();
  entry.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
  await vi.waitFor(() => expect(appState.sqlHistory).toHaveLength(2));
  expect(executeSql).toHaveBeenLastCalledWith(statement);
  expect(appState.sqlHistory[0].sql).toBe(statement);
  target.querySelector<HTMLButtonElement>(".run-btn")!.click();
  await vi.waitFor(() => expect(executeSql).toHaveBeenLastCalledWith("SELECT 7"));
});

it.each(["input", "div"])("a native copy in a %s retires a pending grid write", async tag => {
  const publish = beginClipboardCopy();
  const field = target.appendChild(document.createElement(tag));
  if (tag === "div") field.contentEditable = "true";
  field.addEventListener("copy", event => event.stopPropagation());
  expect(field.dispatchEvent(new Event("copy", { bubbles: true, cancelable: true }))).toBe(true);
  const write = vi.fn(async () => {});
  expect(await publish(write)).toBe(false);
  expect(write).not.toHaveBeenCalled();
});
