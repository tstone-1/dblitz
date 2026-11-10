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

  it("runs clippy with warnings denied, and in no other form", () => {
    // A plain `cargo clippy` exits 0 on warnings, so it passes here and the
    // release's `npm run quality`, which denies them, fails after the tag.
    const clippy = workflow
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("#") && line.includes("cargo clippy"))
      .map((line) => line.trim());
    expect(clippy).toEqual(["run: cargo clippy --all-targets --all-features -- -D warnings"]);
    expect(pkg.scripts.quality).toContain("cargo clippy --all-targets --all-features -- -D warnings");
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

/**
 * An advisory is published against a lockfile that has not changed, so only a
 * schedule finds it between releases: RUSTSEC-2026-0459 shipped in 26.10.5 the
 * day it was published and nothing reported it for eleven days. The allowed
 * list is the part that rots, so it is held equal to the notes that explain it.
 */
describe("the audit workflow runs cargo audit on a schedule", () => {
  const workflow = readText(".github/workflows/audit.yml");
  const notes = readText("docs/agent-notes.md").replace(/\r\n/g, "\n");

  /** The JSON array assigned to `allowed=` in the audit step. */
  function allowedInWorkflow(): string[] {
    const m = /^ {10}allowed='(\[[^']*\])'$/m.exec(workflow);
    return m ? (JSON.parse(m[1]) as string[]) : [];
  }

  /** The entries the notes list as allowed: the sub-bullets of the entry. */
  function allowedInNotes(): string[] {
    const start = notes.indexOf("### Expected `cargo audit` noise");
    const end = notes.indexOf("\n### ", start + 1);
    const section = start < 0 ? "" : notes.slice(start, end < 0 ? undefined : end);
    const out: string[] = [];
    for (const line of section.split("\n").filter((l) => l.startsWith("  - "))) {
      out.push(...(line.match(/RUSTSEC-\d{4}-\d{4}/g) ?? []));
      out.push(...[...line.matchAll(/yanked `([^`]+)`/g)].map((m) => m[1]));
    }
    return out;
  }

  it("runs weekly and on demand, and on nothing a contributor can trigger", () => {
    expect(workflow).toMatch(/^on:\n {2}schedule:\n(?: {4}#.*\n)* {4}- cron: '[^']+'\n {2}workflow_dispatch:$/m);
    expect(workflow).not.toContain("pull_request");
    expect(workflow).not.toMatch(/^ {2}push:/m);
  });

  it("holds no secret and writes nothing", () => {
    expect(workflow).not.toContain("secrets.");
    expect(workflow).not.toContain("contents: write");
    expect(workflow).toContain("persist-credentials: false");
  });

  it("audits the shipped lockfile at a fixed cargo-audit version", () => {
    expect(workflow).toContain("cargo install cargo-audit --version 0.22.2 --locked");
    expect(workflow).toContain('cargo audit --file src-tauri/Cargo.lock --json > "$report"');
    // A crash and a vulnerability share exit code 1, so the report is read.
    expect(workflow).toContain("::error::cargo audit wrote no usable report");
  });

  it("allows the same entries the notes explain, each with a reason", () => {
    const allowed = allowedInWorkflow();
    // Emptiness control: a parse that finds nothing agrees with anything.
    expect(allowed.length).toBeGreaterThan(0);
    expect([...allowedInNotes()].sort()).toEqual([...allowed].sort());
    for (const entry of allowed) {
      expect(workflow).toMatch(new RegExp(`^ {10}# ${entry.replace(/[.]/g, "\\.")} `, "m"));
    }
  });

  it("never allows the SQLite vulnerability: it is fixed, not listed", () => {
    expect(allowedInWorkflow()).not.toContain("RUSTSEC-2026-0459");
  });
});
