#!/usr/bin/env node
// Runs svelte-check and fails unless it actually finished.
//
// svelte-check exits 0 when it crashes: its top-level catch logs the error and
// "svelte-check failed" and never sets an exit code. With --tsgo that includes
// the ordinary case of TypeScript 7 not being installed, so `npm run check`
// went green having checked nothing. This wrapper forces machine output and
// requires the COMPLETED summary line; anything else is a failure.
//
// Usage: node scripts/svelte-check.mjs [svelte-check args...]
import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

const SUMMARY = /^\d+ COMPLETED \d+ FILES (\d+) ERRORS (\d+) WARNINGS/m;

/**
 * Exit code for one svelte-check run. A zero exit without the summary line is
 * the crash case described above.
 * @param {number | null} status
 * @param {string} output
 * @returns {number}
 */
export function checkVerdict(status, output) {
  if (status !== 0) return status ?? 1;
  return SUMMARY.test(output) ? 0 : 1;
}

function isMainModule() {
  if (typeof import.meta.main === "boolean") return import.meta.main;
  if (!process.argv[1]) return false;
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isMainModule()) {
  const run = spawnSync("svelte-check", [...process.argv.slice(2), "--output", "machine"], {
    encoding: "utf8",
    shell: process.platform === "win32",
  });
  const output = `${run.stdout ?? ""}${run.stderr ?? ""}`;
  process.stdout.write(output);
  if (run.error) console.error(run.error);
  const code = run.error ? 1 : checkVerdict(run.status, output);
  if (code !== 0 && run.status === 0) {
    console.error("[FAIL] svelte-check exited 0 without a COMPLETED summary: it did not finish.");
  }
  process.exit(code);
}
