// Packaged-app smoke test: the one check that crosses the webview <-> IPC seam.
//
// Every unit test in this repo (vitest and cargo alike) runs below that seam.
// This script launches the real built binary under tauri-driver (WebDriver),
// hands it a generated fixture database as its launch argument, and asserts
// that a row of that database actually renders in the grid. Passing proves the
// whole chain: bundled assets load, `get_initial_file` delivers the argv path,
// `open_database` and `query_table` cross IPC, and the grid renders the result.
//
// What it does NOT prove, established by experiment on 2026-08-05 rather than
// by reasoning: removing `connect-src ipc: http://ipc.localhost` from the CSP
// does not fail this test on Linux. The directive was deleted on a throwaway
// branch and every job stayed green, while a probe in the same run showed a
// cross-origin fetch still blocked -- so the CSP is live, but wry's Linux IPC
// does not travel over a channel `connect-src` governs. The trap that line
// exists to prevent is a WebView2/WKWebView one, and catching it in CI would
// take a Windows leg. Do not cite this script as cover for that directive.
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
// Hard deadline for the whole script. The CI job's `timeout-minutes` is the
// outer bound, but a job that hits it is killed with no diagnostic and its logs
// are the last thing that printed, which for a wedged session is "tauri-driver
// is up" and nothing else. Failing here instead prints why, and prints the
// driver's own stderr. Well inside the job's 25 minutes, and roughly 2.5x the
// slowest run observed to date (10 min 6 s, 2026-08-14, cold Rust cache -- and
// most of that was the build, which happens in an earlier step).
const OVERALL_TIMEOUT_MS = 10 * 60_000;
// No individual WebDriver call may hang forever. `fetch` has no default
// timeout, so without this a native driver that accepts the connection and
// never answers -- the classic wedged-session shape -- blocks the script
// outside any of the deadline loops above, which only advance between calls.
const REQUEST_TIMEOUT_MS = 120_000;

const log = (msg) => console.log(`[smoke] ${msg}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// tauri-driver's output is forwarded to this process's log AND kept, so a
// failure message can quote it. Inheriting the fd would put it in the CI log
// too, but only interleaved somewhere above the failure -- and the reason a
// session never comes up is almost always in these lines ("WebKitWebDriver not
// found", a port already bound, the app exiting at startup).
const DRIVER_LOG_LINES = 40;
/** @type {string[]} */
const driverLog = [];
// Module scope so the whole-script deadline below can reap the child. That path
// calls process.exit and therefore skips main()'s finally block, which is the
// only other place the driver is killed.
/** @type {import("node:child_process").ChildProcess | null} */
let driverProcess = null;
function recordDriverOutput(stream, chunk) {
  for (const line of String(chunk).split(/\r?\n/)) {
    if (line === "") continue;
    console.log(`[driver:${stream}] ${line}`);
    driverLog.push(`${stream}: ${line}`);
    if (driverLog.length > DRIVER_LOG_LINES) driverLog.shift();
  }
}
function driverDiagnostic() {
  if (driverLog.length === 0) return "tauri-driver printed nothing.";
  return `last ${driverLog.length} line(s) from tauri-driver:\n  ${driverLog.join("\n  ")}`;
}

const appPath =
  process.argv[2] ?? join(root, "src-tauri", "target", "debug", "dblitz");

async function webdriver(method, path, body) {
  let res;
  try {
    res = await fetch(`${BASE}${path}`, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch (e) {
    if (e?.name === "TimeoutError") {
      throw new Error(
        `${method} ${path} did not answer within ${REQUEST_TIMEOUT_MS / 1000}s; ` +
          driverDiagnostic(),
      );
    }
    throw e;
  }
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
  //
  // `shout` is a generated column, and it is here because the whole column
  // list depends on which PRAGMA the backend introspects with: `table_info`
  // omits generated columns while `SELECT *` returns them, so a regression
  // there renders a grid one column short of its own rows. Only a real
  // packaged run exercises that end to end.
  const fixtureDir = mkdtempSync(join(tmpdir(), "dblitz-smoke-"));
  const dbPath = join(fixtureDir, "smoke.sqlite");
  const db = new DatabaseSync(dbPath);
  db.exec(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, shout AS (upper(name)));" +
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
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  driverProcess = driver;
  driver.stdout.on("data", (c) => recordDriverOutput("out", c));
  driver.stderr.on("data", (c) => recordDriverOutput("err", c));

  let driverExited = false;
  /** @type {string} */
  let driverExitReason = "";
  driver.on("exit", (code, signal) => {
    driverExited = true;
    driverExitReason = signal ? `killed by ${signal}` : `exit code ${code}`;
  });
  // Without this, a missing `tauri-driver` binary raises an unhandled 'error'
  // event rather than a failed check, and the message ("spawn tauri-driver
  // ENOENT") never reaches the [FAIL] line the CI log is read for.
  driver.on("error", (e) => {
    driverExited = true;
    driverExitReason = `could not be spawned: ${e.message}`;
  });

  let sessionId = null;
  try {
    // Wait for the driver's HTTP endpoint to come up.
    const driverDeadline = Date.now() + DRIVER_STARTUP_TIMEOUT_MS;
    for (;;) {
      if (driverExited) {
        throw new Error(
          `tauri-driver ${driverExitReason} during startup; ${driverDiagnostic()}`,
        );
      }
      try {
        await webdriver("GET", "/status");
        break;
      } catch {
        if (Date.now() > driverDeadline) {
          throw new Error(
            `tauri-driver did not answer /status within ` +
              `${DRIVER_STARTUP_TIMEOUT_MS / 1000}s; ${driverDiagnostic()}`,
          );
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
      throw new Error(
        `no sessionId in response: ${JSON.stringify(session)}; ` +
          driverDiagnostic(),
      );
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
            JSON.stringify(state, null, 2) +
            `\n${driverDiagnostic()}`,
        );
      }
      await sleep(POLL_INTERVAL_MS);
    }

    console.log("[OK] grid renders a fixture row over production IPC");
    if (!state.cells.includes("ALICE")) {
      throw new Error(
        "grid rendered the ordinary columns but not the generated one; " +
          `cells: ${JSON.stringify(state.cells)}`,
      );
    }
    console.log("[OK] generated column renders alongside the ordinary ones");
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

// The whole-script deadline. `unref()` so the timer never keeps a successful
// run alive, and `process.exit` rather than `exitCode` because at this point
// something is stuck: a pending fetch or a live child would otherwise hold the
// event loop open until the job timeout kills it -- which is the outcome this
// exists to replace with a message.
const deadline = setTimeout(() => {
  console.error(
    `[FAIL] smoke test exceeded its ${OVERALL_TIMEOUT_MS / 60_000} minute ` +
      `deadline and made no verdict; ${driverDiagnostic()}`,
  );
  driverProcess?.kill("SIGKILL");
  process.exit(1);
}, OVERALL_TIMEOUT_MS);
deadline.unref();

main()
  .then(() => {
    clearTimeout(deadline);
  })
  .catch((e) => {
    clearTimeout(deadline);
    console.error(`[FAIL] ${e?.message ?? e}`);
    // Explicit, not `exitCode`: the finally block kills the driver, but a
    // socket left open by an aborted fetch can still keep the loop alive, and
    // an exit-code-only failure that never exits reads in CI as a hang.
    process.exit(1);
  });
