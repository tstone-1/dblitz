import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const hook = resolve(".githooks/pre-push");
const scratch = mkdtempSync(join(tmpdir(), "dblitz hook "));
const repo = join(scratch, "work");
const remote = join(scratch, "published.git");
const privateBin = join(scratch, "agent-memory", "bin");
const marker = "SYNTHETIC_PRIVATE_MARKER";
const env = {
  ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_"))),
  HOME: scratch,
  USERPROFILE: scratch,
  GIT_CONFIG_GLOBAL: join(scratch, "empty.gitconfig"),
  GIT_CONFIG_NOSYSTEM: "1",
  SKIP_AGENT_MEMORY_AUDIT: "",
  GIT_AUTHOR_NAME: "Fixture",
  GIT_COMMITTER_NAME: "Fixture",
  GIT_AUTHOR_EMAIL: "fixture@example.invalid",
  GIT_COMMITTER_EMAIL: "fixture@example.invalid",
};

function git(args: string[], input?: string) {
  const run = spawnSync("git", args, { cwd: repo, env, input, encoding: "utf8", timeout: 30000 });
  if (run.status !== 0) throw new Error(run.error?.message ?? run.stderr);
  return run.stdout.trim();
}

function commit(text: string) {
  writeFileSync(join(repo, "publish sample.mjs"), text);
  git(["add", "--", "publish sample.mjs"]);
  git(["commit", "-qm", "Synthetic fixture"]);
  return git(["rev-parse", "HEAD"]);
}

function update(ref: string, tip: string, old: string) {
  return `refs/heads/${ref} ${tip} refs/heads/${ref} ${old}\n`;
}

function audit(input: string, destination = remote) {
  const run = spawnSync("bash", [hook, "origin", destination], { cwd: repo, env, input, encoding: "utf8", timeout: 45000 });
  if (run.error) throw run.error;
  return { status: run.status, output: run.stdout + run.stderr };
}

describe("pre-push public-safe audit", { timeout: 60000 }, () => {
  let base: string, clean: string, privateReject: string, marked: string, fixed: string, zeros: string;

  beforeAll(() => {
    mkdirSync(repo);
    mkdirSync(privateBin, { recursive: true });
    writeFileSync(join(privateBin, "audit-agent-memory.ps1"), [
      "param([switch]$CheckPublicSafe, [string]$CheckRepoPath)",
      "$ForbiddenPatterns = @(",
      `    '${marker}'`,
      ")",
      "$ForbiddenCommitMessagePatterns = @(",
      "    '^[ \\t]*Synthetic message marker\\b'",
      ")",
      "$content = Get-Content -LiteralPath (Join-Path $CheckRepoPath 'publish sample.mjs') -Raw",
      "if ($content.Trim() -eq 'private checker case') { Write-Output 'private diagnostic sentinel'; exit 1 }",
      "if ($content -match $ForbiddenPatterns[0]) { Write-Output $ForbiddenPatterns[0]; exit 1 }",
      "Write-Output $ForbiddenPatterns[0]",
      "exit 0",
    ].join("\n"));
    git(["init", "-qb", "main"]);
    base = commit("published\n");
    git(["clone", "--quiet", "--bare", "--no-local", ".", remote]);
    clean = commit("ready\n");
    privateReject = commit("private checker case");
    marked = commit(marker.toLowerCase());
    fixed = commit("cleaned\n");
    zeros = "0".repeat(base.length);
  }, 60000);
  afterAll(() => rmSync(scratch, { recursive: true, force: true }));

  it.skipIf(spawnSync("pwsh", ["-NoProfile", "-Command", "exit 0"]).status !== 0)(
    "honors an independent private rejection and hides the diagnostic", () => {
      const run = audit(update("main", privateReject, clean));
      expect(run.status).toBe(1);
      expect(run.output).toContain("Private audit of outgoing revisions failed");
      expect(run.output).not.toContain("private diagnostic sentinel");
      expect(audit(update("main", clean, base)).status).toBe(0);
    },
  );

  it("checks outgoing messages independently of clean file contents", () => {
    const tree = git(["rev-parse", clean + "^{tree}"]);
    const tip = git(["commit-tree", tree, "-p", clean, "-m", "Ordinary subject", "-m", " Synthetic message marker"]);
    const run = audit(update("main", tip, clean));
    expect(run.status).toBe(1);
    expect(run.output).toContain("Outgoing commit message contains a forbidden pattern");
  });

  it("checks a committed mjs blob after it was cleaned in the working tree", () => {
    const run = audit(update("main", marked, base));
    expect(run.output).toMatch(/contains a forbidden public-repo pattern/);
    expect(run.output.toUpperCase()).not.toContain(marker);
    expect(run.status).toBe(1);
  });

  it("checks intermediate commits even when the new tip is clean", () => {
    const run = audit(update("main", fixed, base));
    expect(run.output).toMatch(/contains a forbidden public-repo pattern/);
    expect(run.status).toBe(1);
  });

  it("checks a second outgoing ref on the same push", () => {
    const run = audit(update("main", clean, base) + update("extra", fixed, zeros));
    expect(run.output).toMatch(/contains a forbidden public-repo pattern/);
    expect(run.status).toBe(1);
  });

  it("blocks a Git push and leaves the local bare destination at its old tip", () => {
    const run = spawnSync("git", ["-c", `core.hooksPath=${resolve(".githooks")}`, "push", remote, "main"], {
      cwd: repo, env, encoding: "utf8", timeout: 45000,
    });
    expect(run.stdout + run.stderr).toMatch(/contains a forbidden public-repo pattern/);
    expect(run.status).toBe(1);
    expect(git(["--git-dir", remote, "rev-parse", "refs/heads/main"])).toBe(base);
  });

  it("allows a clean new ref and ref deletion", () => {
    writeFileSync(join(repo, "publish sample.mjs"), marker);
    let run;
    try {
      run = audit(update("new", clean, zeros) + update("obsolete", zeros, base));
    } finally {
      git(["restore", "--worktree", "--", "publish sample.mjs"]);
    }
    expect(run.output).not.toContain(marker);
    expect(run.output).toMatch(/outgoing content passed/);
    expect(run.status).toBe(0);
  });

  it("uses the destination's published history as the boundary for a new ref", () => {
    const published = join(scratch, "already-published.git");
    git(["clone", "--quiet", "--bare", "--no-local", ".", published]);
    const run = audit(update("new", fixed, zeros), published);
    expect(run.output).toMatch(/outgoing content passed/);
    expect(run.status).toBe(0);
  });

  it("checks a tag that points directly at a blob", () => {
    const blob = git(["rev-parse", `${marked}:publish sample.mjs`]);
    const run = audit(`refs/tags/data ${blob} refs/tags/data ${zeros}\n`);
    expect(run.output).toMatch(/Blob .* contains a forbidden public-repo pattern/);
    expect(run.status).toBe(1);
  });

  it("stops when an outgoing tree points at an unreadable blob", () => {
    const tree = git(["mktree", "--missing"], `100644 blob ${"1".repeat(base.length)}\tmissing.txt\n`);
    const run = audit(update("tree", tree, base));
    expect(run.output).toMatch(/Cannot search tree/);
    expect(run.status).toBe(1);
  });
});
