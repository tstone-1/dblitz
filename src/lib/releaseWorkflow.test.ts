import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// Repo root, two levels up from src/lib.
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readText(file: string): string {
  return readFileSync(join(root, file), "utf8");
}

const WORKFLOWS = [".github/workflows/checks.yml", ".github/workflows/release.yml"];

/** Every `uses:` line, with leading whitespace and the `uses:` keyword stripped. */
function actionRefs(workflow: string): string[] {
  return workflow
    .split(/\r?\n/)
    .map((line) => /^\s*(?:-\s*)?uses:\s*(.+)$/.exec(line)?.[1]?.trim())
    .filter((ref): ref is string => ref !== undefined);
}

/**
 * The release build job holds three secrets at once: the updater's minisign
 * private key, the Apple Developer ID material, and a contents-write token. A
 * mutable tag or branch decides which code receives them, so whoever controls
 * that ref controls the key that authorizes updates on the unsigned Windows and
 * Linux builds — and losing that key orphans every installed copy, because a
 * new one cannot sign for clients holding the old pubkey.
 *
 * Pinning to a full commit SHA does not make an action trustworthy; it makes
 * the version reviewable, and makes an upstream change arrive as a diff rather
 * than as a silent swap under a tag that already passed review.
 */
describe("workflow actions are pinned to reviewed commits", () => {
  // `dtolnay/rust-toolchain@stable` was a BRANCH, not even a tag — worth
  // stating, because the shorthand reads like a toolchain channel and its
  // mutability is easy to miss. Pinning it freezes the action, not the Rust
  // channel: the pinned action still installs whatever stable is that day.
  const PIN = /^[\w.-]+\/[\w.-]+@[0-9a-f]{40} # \S/;

  for (const file of WORKFLOWS) {
    describe(file, () => {
      const refs = actionRefs(readText(file));

      it("has actions to check at all", () => {
        // Emptiness control. A regex sweep over zero lines passes every
        // assertion below while proving nothing, and a workflow rename or a
        // change to the `uses:` layout would produce exactly that.
        expect(refs.length).toBeGreaterThan(0);
      });

      it("pins every action to a full 40-character commit SHA with a version comment", () => {
        const unpinned = refs.filter((ref) => !PIN.test(ref));
        expect(unpinned).toEqual([]);
      });
    });
  }

  it("pins the actions in the secret-bearing release build job", () => {
    // Stated separately from the sweep above because this is the job the rule
    // exists for: a future workflow that legitimately uses a floating ref
    // somewhere else must not quietly relax it here.
    const release = readText(".github/workflows/release.yml");
    const build = release.slice(release.indexOf("\n  build:"), release.indexOf("\n  publish:"));
    const refs = actionRefs(build);

    expect(refs.length).toBeGreaterThan(0);
    expect(refs.some((ref) => ref.startsWith("tauri-apps/tauri-action@"))).toBe(true);
    for (const ref of refs) {
      expect(ref).toMatch(/@[0-9a-f]{40} #/);
    }
  });
});

/**
 * A skipped notarization exits 0: the bundler logs "skipping app notarization"
 * and succeeds, shipping a signed-but-unnotarized app that Gatekeeper rejects
 * on any machine that has never seen it. The two gates that catch that were
 * themselves conditioned on the signing identity being non-empty, so the exact
 * failure they exist to catch skipped both — and `publish` still ran.
 */
describe("macOS release signing fails closed in the canonical repo", () => {
  const release = readText(".github/workflows/release.yml");

  const APPLE_SECRETS = [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_P8",
  ];

  const prepare = release.slice(
    release.indexOf("- name: Prepare Apple signing and notarization"),
    release.indexOf("- name: Build and upload artifacts"),
  );

  it("reads the prepare step", () => {
    // Emptiness control for the slice: an empty string satisfies "does not
    // contain X" for every X, so a renamed step would silently disarm the
    // assertions below rather than failing.
    expect(prepare).toContain("Prepare Apple signing and notarization");
    expect(prepare.length).toBeGreaterThan(200);
  });

  it("requires every Apple secret, not just the certificate and the key", () => {
    // The old two-variable check let four of the six through, so a rotation
    // that landed five produced a green, unnotarized, published release.
    for (const secret of APPLE_SECRETS) {
      expect(prepare).toContain(secret);
    }
    expect(prepare).toMatch(/for v in(?: APPLE_\w+){6}; do/);
  });

  it("refuses to continue in the canonical repo when one is missing", () => {
    expect(prepare).toContain('if [ "$GITHUB_REPOSITORY" = "tstone-1/dblitz" ]; then');
    expect(prepare).toContain("exit 1");
  });

  it("still builds ad-hoc signed in a fork", () => {
    // Forks have none of these secrets and must keep building; only the
    // canonical repo promises notarized artifacts.
    expect(prepare).toContain("::warning::Apple signing secrets absent");
  });

  it("runs the artifact verification on every canonical macOS leg", () => {
    // Not merely when an identity happens to be set — that is the condition
    // that made the check skip precisely when it was needed.
    const verify = release.slice(
      release.indexOf("- name: Verify the macOS build is signed, notarized and stapled"),
      release.indexOf("- name: Upload portable exe"),
    );
    expect(verify).toContain("Verify the macOS build is signed");
    expect(verify).toContain("github.repository == 'tstone-1/dblitz'");
    expect(verify).toContain("source=Notarized Developer ID");
  });
});

/**
 * BUILD.md is the release recipe for a PUBLIC repository, and it used to say
 * `git add -A` in two places. Blanket staging sweeps every untracked file in
 * the worktree into the release commit - a scratch script, a test database, a
 * file written for another branch - and in a public repo that is a
 * confidentiality problem as much as a tidiness one. The instruction is only
 * worth anything if it stays changed, hence this gate.
 */
describe("release instructions stage by explicit pathspec", () => {
  const build = readText("BUILD.md").split(/\r?\n/);
  // Command lines only. The document deliberately NAMES `git add -A` in prose
  // and in `#` comments to say not to use it, and that must not trip the check.
  const gitAddCommands = build
    .map((line) => line.trim())
    .filter((line) => line.startsWith("git add "));

  it("has git add commands to check at all", () => {
    // Emptiness control: a rewrite that removed the staging step entirely, or a
    // change to how the recipe is formatted, would otherwise pass silently.
    expect(gitAddCommands.length).toBeGreaterThan(0);
  });

  it("uses no blanket staging command", () => {
    const blanket = gitAddCommands.filter((line) =>
      /^git add\s+(-A\b|--all\b|\.\s*$|\.\s)/.test(line),
    );
    expect(blanket).toEqual([]);
  });
});

describe("workflow jobs are bounded and least-privileged", () => {
  for (const file of WORKFLOWS) {
    it(`gives every job in ${file} a timeout`, () => {
      // Only the `jobs:` section: the `on:` block has two-space keys too
      // (`push:`, `pull_request:`), and they are not jobs.
      const text = readText(file).split(/^jobs:$/m)[1] ?? "";
      const jobs = [...text.matchAll(/^ {2}([a-z][\w-]*):$/gm)].map((m) => m[1]);
      expect(jobs.length).toBeGreaterThan(0);
      const jobBlocks = text.split(/^ {2}(?=[a-z][\w-]*:$)/m).slice(1);
      const untimed = jobBlocks.filter((b) => !/^\s{4}timeout-minutes: \d+$/m.test(b));
      expect(untimed.map((b) => b.split(":")[0])).toEqual([]);
    });

    it(`${file} defaults to contents: read`, () => {
      expect(readText(file)).toMatch(/^permissions:\n {2}contents: read$/m);
    });
  }

  it("serializes releases on the workflow, not on the ref", () => {
    const release = readText(".github/workflows/release.yml");
    expect(release).toContain("group: ${{ github.workflow }}");
    expect(release).toContain("cancel-in-progress: false");
  });

  it("installs Linux deps from the one shared list", () => {
    // Command lines only: the comment above each install quotes the bad form.
    const lines = WORKFLOWS.flatMap((f) => readText(f).split("\n")).filter(
      (l) => !l.trimStart().startsWith("#"),
    );
    const installs = lines.filter((l) => /apt-get install/.test(l));
    expect(installs.length).toBeGreaterThan(0);
    const inline = installs.filter((l) => !/apt-get install -y \$deps\b/.test(l));
    expect(inline).toEqual([]);
    expect(readText(".github/tauri-linux-deps.txt")).toContain("libayatana-appindicator3-dev");
  });
});
