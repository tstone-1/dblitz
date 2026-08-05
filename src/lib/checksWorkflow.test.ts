import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// Repo root, two levels up from src/lib.
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readText(file: string): string {
  return readFileSync(join(root, file), "utf8");
}

/**
 * `npm run quality` is the release gate; checks.yml is what actually runs on
 * every push and PR. Whenever the two drift, the weaker one is the one guarding
 * main, and the gap only surfaces at release time on a commit that has already
 * landed. This pins the frontend half: every command the quality gate runs
 * before it hands over to cargo must also appear as a step in checks.yml.
 *
 * Deliberately derived from package.json rather than hardcoded, so a gate added
 * to `quality` later is a failing test here rather than a silent omission.
 */
describe("checks workflow covers the frontend quality gate", () => {
  const workflow = readText(".github/workflows/checks.yml");
  const pkg = JSON.parse(readText("package.json")) as {
    scripts: Record<string, string>;
  };

  // "npm run check && npm test && npm run build && cd src-tauri && cargo ..."
  // -- everything before the `cd` is the frontend leg; the cargo commands after
  // it belong to the backend matrix job and are checked by that job's steps.
  const frontendCommands = pkg.scripts.quality
    .split("cd src-tauri")[0]
    .split("&&")
    .map((command) => command.trim())
    .filter((command) => command.length > 0);

  it("parses the frontend commands out of the quality script", () => {
    // A parse that finds nothing would make every assertion below pass
    // vacuously, which reads exactly like full coverage.
    expect(frontendCommands.length).toBeGreaterThanOrEqual(3);
    expect(frontendCommands.every((command) => command.startsWith("npm"))).toBe(true);
  });

  it.each(frontendCommands)("runs %s on push and pull request", (command) => {
    expect(workflow).toContain(`run: ${command}`);
  });

  it("builds the frontend", () => {
    // Called out separately from the loop above because this is the one the
    // workflow was missing: `npm run check` type-checks but never runs Vite's
    // production build, so a failure exclusive to it reached main unchallenged.
    expect(workflow).toContain("run: npm run build");
  });
});

/**
 * The smoke job is the only check that crosses the webview <-> IPC seam --
 * every unit test (vitest and cargo alike) runs below it, and the defect class
 * it exists for (a CSP/capability regression that breaks production IPC while
 * dev stays healthy) is invisible to all of them. It cannot be part of
 * `npm run quality` because tauri-driver has no macOS backend, so this pin is
 * the only thing that notices the job being dropped or its steps drifting.
 */
describe("checks workflow runs the packaged-app smoke test", () => {
  const workflow = readText(".github/workflows/checks.yml");

  it("builds the packaged app rather than reusing the dev server", () => {
    // --no-bundle alone would be wrong (no frontend assets); a plain build
    // would be wrong too (bundling needs the updater signing secret). The
    // exact flag pair is the contract.
    expect(workflow).toContain("npx tauri build --debug --no-bundle");
  });

  it("drives the built binary under a virtual display", () => {
    expect(workflow).toContain(
      "xvfb-run --auto-servernum node scripts/smoke-test.mjs",
    );
  });

  it("ships the smoke script the job invokes", () => {
    // readText throws on a missing file, so a renamed/deleted script fails
    // here rather than for the first time inside the CI job.
    const script = readText("scripts/smoke-test.mjs");
    // The assertion the whole job exists for: the script must check that real
    // fixture data crossed IPC into the grid, not merely that a window opened.
    expect(script).toContain('.data-cell');
    expect(script).toContain("alice");
  });
});
