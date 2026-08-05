// Packaged-app smoke test: the one check that crosses the webview <-> IPC seam.
//
// Every unit test in this repo (vitest and cargo alike) runs below that seam,
// so the defect class this exists for -- a CSP or capability regression that
// breaks production IPC while `tauri dev` stays perfectly healthy (see the
// `connect-src ipc:` note in AGENTS.md) -- is invisible to all of them. This
// script launches the real built binary under tauri-driver (WebDriver),
// hands it a generated fixture database as its launch argument, and asserts
// that a row of that database actually renders in the grid. Passing proves the
// whole chain: bundled assets load under the production CSP, `get_initial_file`
// delivers the argv path, `open_database` and `query_table` cross IPC, and the
// grid renders the result.
//
// Platform: Linux (webkit2gtk-driver) and Windows only -- tauri-driver has no
// macOS backend, which is why this runs as a Linux CI job and not in
// `npm run quality`. Requirements on Linux: the app built at
// src-tauri/target/debug/dblitz (`npx tauri build --debug --no-bundle`),
// `tauri-driver` and `WebKitWebDriver` on PATH (`cargo install tauri-driver`,
// `apt install webkit2gtk-driver`), and a display -- run under
// `xvfb-run --auto-servernum` on a headless machine.
//
// Usage: xvfb-run --auto-servernum node scripts/smoke-test.mjs [app-binary]
//
// Output sticks to ASCII ([OK]/[FAIL]) so it renders on any CI console.

import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
// node:sqlite is built into the Node this repo pins (.nvmrc = 24); using it
// keeps the fixture generation dependency-free. It may print an experimental
// warning on some point releases -- harmless.
import { DatabaseSync } from "node:sqlite";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DRIVER_PORT = 4444;
const NATIVE_PORT = 4445;
const BASE = `http://127.0.0.1:${DRIVER_PORT}`;
const DRIVER_STARTUP_TIMEOUT_MS = 30_000;
// The app has to cold-start, open the database, and run its first query before
// anything can render; a loaded CI runner can take a while to get there.
const RENDER_TIMEOUT_MS = 60_000;
const POLL_INTERVAL_MS = 500;

const log = (msg) => console.log(`[smoke] ${msg}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const appPath =
  process.argv[2] ?? join(root, "src-tauri", "target", "debug", "dblitz");

async function webdriver(method, path, body) {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(
      `${method} ${path} -> HTTP ${res.status}: ${JSON.stringify(json)}`,
    );
  }
  return json;
}

async function main() {
  if (!existsSync(appPath)) {
    throw new Error(
      `app binary not found at ${appPath} -- build it first with ` +
        "`npx tauri build --debug --no-bundle` (or pass the path as argv[1])",
    );
  }

  // Single-table fixture on purpose: BrowseData auto-selects the lone table,
  // so the grid renders without any UI interaction beyond the launch itself.
  const fixtureDir = mkdtempSync(join(tmpdir(), "dblitz-smoke-"));
  const dbPath = join(fixtureDir, "smoke.sqlite");
  const db = new DatabaseSync(dbPath);
  db.exec(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);" +
      "INSERT INTO users (name) VALUES ('alice'), ('bravo'), ('carol');",
  );
  db.close();
  log(`fixture created at ${dbPath}`);

  // tauri-driver proxies the W3C WebDriver protocol to the platform's native
  // driver (WebKitWebDriver on Linux), launching the app itself and exporting
  // TAURI_AUTOMATION so wry puts the webview into automation mode.
  const driver = spawn(
    "tauri-driver",
    ["--port", String(DRIVER_PORT), "--native-port", String(NATIVE_PORT)],
    { stdio: ["ignore", "inherit", "inherit"] },
  );
  let driverExited = false;
  driver.on("exit", () => {
    driverExited = true;
  });

  let sessionId = null;
  try {
    // Wait for the driver's HTTP endpoint to come up.
    const driverDeadline = Date.now() + DRIVER_STARTUP_TIMEOUT_MS;
    for (;;) {
      if (driverExited) throw new Error("tauri-driver exited during startup");
      try {
        await webdriver("GET", "/status");
        break;
      } catch {
        if (Date.now() > driverDeadline) {
          throw new Error("tauri-driver did not become ready in time");
        }
        await sleep(POLL_INTERVAL_MS);
      }
    }
    log("tauri-driver is up");

    // `args` delivers the fixture path as argv[1], the same route a CLI launch
    // or a Windows file association uses -- get_initial_file picks it up.
    const session = await webdriver("POST", "/session", {
      capabilities: {
        alwaysMatch: {
          browserName: "wry",
          "tauri:options": { application: appPath, args: [dbPath] },
        },
      },
    });
    sessionId = session.value?.sessionId;
    if (!sessionId) {
      throw new Error(`no sessionId in response: ${JSON.stringify(session)}`);
    }
    log(`session ${sessionId} started, app launched`);

    const execute = async (script) => {
      const res = await webdriver(
        "POST",
        `/session/${sessionId}/execute/sync`,
        { script, args: [] },
      );
      return res.value;
    };

    // Poll until the grid shows the fixture row (or time runs out). The state
    // snapshot doubles as the failure diagnostic -- on timeout it says how far
    // the app got (blank page? toolbar but no grid? grid but no rows?).
    const deadline = Date.now() + RENDER_TIMEOUT_MS;
    let state = null;
    for (;;) {
      state = await execute(`return {
        path: document.querySelector(".file-path")?.textContent?.trim() ?? "",
        cells: Array.from(document.querySelectorAll(".data-cell"))
          .slice(0, 12)
          .map((cell) => cell.textContent.trim()),
        body: document.body?.innerText?.slice(0, 400) ?? "",
      };`);
      if (state.cells.includes("alice")) break;
      if (Date.now() > deadline) {
        throw new Error(
          "grid never rendered the fixture row; last observed state: " +
            JSON.stringify(state, null, 2),
        );
      }
      await sleep(POLL_INTERVAL_MS);
    }

    console.log("[OK] grid renders a fixture row over production IPC");
    if (!state.path.includes("smoke.sqlite")) {
      throw new Error(
        `toolbar path does not show the opened database: "${state.path}"`,
      );
    }
    console.log("[OK] toolbar shows the opened database path");
    console.log("[PASS] packaged-app smoke test");
  } finally {
    if (sessionId) {
      // Best-effort: closing the session also closes the app.
      await webdriver("DELETE", `/session/${sessionId}`).catch(() => {});
    }
    driver.kill();
    rmSync(fixtureDir, { recursive: true, force: true });
  }
}

main().catch((e) => {
  console.error(`[FAIL] ${e?.message ?? e}`);
  process.exitCode = 1;
});
