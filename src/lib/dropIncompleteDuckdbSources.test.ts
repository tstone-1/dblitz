import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "drop-incomplete-duckdb-sources.sh");

const scratchDirs: string[] = [];

function scratchTarget(): string {
  const dir = mkdtempSync(join(tmpdir(), "dblitz-duckdb-sources-"));
  scratchDirs.push(dir);
  return dir;
}

/** A source folder as unpacked, or as rust-cache leaves it: subfolders, no files. */
function sourceFolder(path: string, complete: boolean): string {
  mkdirSync(join(path, "duckdb", "src"), { recursive: true });
  if (complete) {
    writeFileSync(join(path, ".complete"), "");
    writeFileSync(join(path, "duckdb", "manifest.json"), "{}");
  }
  return path;
}

function run(target: string) {
  return spawnSync("bash", [script, target], { encoding: "utf8" });
}

afterEach(() => {
  for (const dir of scratchDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

// Windows has no bash this suite can rely on: `bash` there can be the WSL stub.
// The Linux and macOS runs of the suite cover the script's logic.
describe.skipIf(process.platform === "win32")("drop-incomplete-duckdb-sources.sh", () => {
  it("removes a folder without .complete and keeps one with it", () => {
    const target = scratchTarget();
    const stripped = sourceFolder(join(target, "duckdb-sources", "aaaa"), false);
    const complete = sourceFolder(join(target, "duckdb-sources", "bbbb"), true);

    const result = run(target);

    expect(result.status, result.stderr).toBe(0);
    expect(existsSync(stripped)).toBe(false);
    expect(existsSync(join(complete, ".complete"))).toBe(true);
    expect(existsSync(join(complete, "duckdb", "manifest.json"))).toBe(true);
    expect(result.stdout).toContain("(no .complete)");
  });

  it("covers the folder of a build with an explicit target triple", () => {
    const target = scratchTarget();
    const stripped = sourceFolder(
      join(target, "aarch64-apple-darwin", "duckdb-sources", "aaaa"),
      false,
    );

    const result = run(target);

    expect(result.status, result.stderr).toBe(0);
    expect(existsSync(stripped)).toBe(false);
  });

  it("does nothing on a cold cache", () => {
    const target = scratchTarget();

    const result = run(target);

    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe("");
  });

  it("fails without a target folder rather than checking nothing", () => {
    const result = spawnSync("bash", [script], { encoding: "utf8" });

    expect(result.status).not.toBe(0);
  });
});

describe("checks workflow removes incomplete DuckDB sources", () => {
  const workflow = readFileSync(join(root, ".github/workflows/checks.yml"), "utf8");
  const jobs = workflow.split(/^ {2}(?=[a-z-]+:\n)/m);
  const cargoJobs = jobs.filter((job) => job.includes("Swatinem/rust-cache@"));

  it("finds the jobs that restore a Rust cache", () => {
    // The backend matrix and the smoke test. A split that finds none would make
    // the assertion below pass having checked nothing.
    expect(cargoJobs).toHaveLength(2);
  });

  it.each(cargoJobs.map((job) => [job.split(":")[0], job]))(
    "runs the script in %s between restoring the cache and the first cargo command",
    (_name, job) => {
      const restore = job.indexOf("Swatinem/rust-cache@");
      const clean = job.indexOf("bash scripts/drop-incomplete-duckdb-sources.sh src-tauri/target");
      const firstCargo = job.search(/^\s+run: (cargo|npx tauri|npm run tauri)|^\s+cargo /m);

      expect(clean).toBeGreaterThan(restore);
      expect(firstCargo).toBeGreaterThan(clean);
    },
  );
});
