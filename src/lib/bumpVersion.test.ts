import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { bumpVersionFiles, nextVersion } from "../../scripts/bump-version.mjs";
import { checkReleaseContract, readReleaseFiles } from "../../scripts/release-preflight.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
// A version the repository will not reach, so these tests never bump to the current one.
const TARGET = "99.1.0";

/**
 * The CHANGELOG with a pending section. A release commit dates the pending
 * heading, so on a release tree there is none and `--release` refuses, as it
 * should; the tests below must pass at every point of the cycle (they failed
 * the release gate of 26.10.4 by assuming a pending section).
 */
function withPendingSection(changelog: string): string {
  if (/^## \[[^\]]+\] - Unreleased$/m.test(changelog)) return changelog;
  const at = changelog.search(/^## \[/m);
  expect(at).toBeGreaterThan(0);
  const section = "## [99.0.0] - Unreleased\n\n### Added\n- A note.\n\n";
  return changelog.slice(0, at) + section + changelog.slice(at);
}

const repoFiles = () => {
  const files = readReleaseFiles(root, (p) => readFileSync(p, "utf8"));
  return { ...files, changelog: withPendingSection(files.changelog) };
};
const repoVersion = () => (JSON.parse(repoFiles().packageJson) as { version: string }).version;

/** Lines that differ between two texts of the same line count. */
function changedLines(before: string, after: string): string[] {
  const a = before.split("\n");
  const b = after.split("\n");
  expect(b.length).toBe(a.length);
  return b.filter((line, i) => line !== a[i]);
}

describe("nextVersion", () => {
  it("counts MICRO up within the month and starts a new month at .0", () => {
    expect(nextVersion("26.10.3", new Date(2026, 9, 29))).toBe("26.10.4");
    expect(nextVersion("26.10.3", new Date(2026, 10, 2))).toBe("26.11.0");
    expect(nextVersion("26.12.4", new Date(2027, 0, 5))).toBe("27.1.0");
  });

  it("refuses a current version from a later month than today", () => {
    expect(() => nextVersion("26.11.0", new Date(2026, 9, 29))).toThrow(/later than today/);
  });
});

describe("bumpVersionFiles on the repository's own files", () => {
  it("produces a tree the release preflight accepts", () => {
    const files = repoFiles();
    const bumped = bumpVersionFiles(files, TARGET, { releaseDate: "2026-11-03" });
    expect(checkReleaseContract({ tag: `v${TARGET}`, ...bumped })).toEqual([]);
    // Control: the unbumped tree fails the same check, so the pass above is the bump's doing.
    expect(checkReleaseContract({ tag: `v${TARGET}`, ...files })).not.toEqual([]);
  });

  it("changes the version lines and nothing else", () => {
    const files = repoFiles();
    const old = repoVersion();
    const bumped = bumpVersionFiles(files, TARGET, { releaseDate: "2026-11-03" });
    expect(changedLines(files.packageJson, bumped.packageJson)).toEqual([`  "version": "${TARGET}",`]);
    expect(changedLines(files.packageLock, bumped.packageLock)).toEqual([
      `  "version": "${TARGET}",`,
      `      "version": "${TARGET}",`,
    ]);
    expect(changedLines(files.cargoToml, bumped.cargoToml)).toEqual([`version = "${TARGET}"`]);
    expect(changedLines(files.cargoLock, bumped.cargoLock)).toEqual([`version = "${TARGET}"`]);
    expect(changedLines(files.tauriConf, bumped.tauriConf)).toHaveLength(1);
    expect(changedLines(files.changelog, bumped.changelog)).toEqual([`## [${TARGET}] - 2026-11-03`]);
    expect(old).not.toBe(TARGET);
  });
});

describe("bumpVersionFiles edge cases", () => {
  const base = (changelog: string) => ({
    packageJson: '{\n  "name": "dblitz",\n  "version": "1.2.3"\n}\n',
    // A dependency at the same version as the app, listed after the root entry.
    packageLock:
      '{\n  "name": "dblitz",\n  "version": "1.2.3",\n  "packages": {\n    "": {\n      "name": "dblitz",\n' +
      '      "version": "1.2.3"\n    },\n    "node_modules/x": {\n      "version": "1.2.3"\n    }\n  }\n}\n',
    cargoToml: '[package]\nname = "dblitz"\nversion = "1.2.3"\n\n[dependencies]\nx = { version = "1.2.3" }\n',
    cargoLock:
      '[[package]]\nname = "x"\nversion = "1.2.3"\n\n[[package]]\nname = "dblitz"\nversion = "1.2.3"\n' +
      'dependencies = [\n "x",\n]\n',
    tauriConf: '{\n  "productName": "dblitz",\n  "version": "1.2.3"\n}\n',
    changelog,
  });

  it("leaves a dependency at the same version alone", () => {
    const b = bumpVersionFiles(base("# Changelog\n\n## [1.2.3] - 2026-01-01\n"), "26.11.0");
    expect(b.packageLock).toContain('"node_modules/x": {\n      "version": "1.2.3"');
    expect(b.cargoLock).toContain('name = "x"\nversion = "1.2.3"');
    expect(b.cargoLock).toContain('name = "dblitz"\nversion = "26.11.0"');
    expect(b.cargoToml).toContain('x = { version = "1.2.3" }');
  });

  it("refuses an edit that would land on a dependency instead of the root entry", () => {
    const files = base("");
    // packages[""] without a version: the first "version" after it is x's.
    files.packageLock = files.packageLock.replace('"name": "dblitz",\n      "version": "1.2.3"', '"name": "dblitz"');
    expect(() => bumpVersionFiles(files, "26.11.0")).toThrow(/changed more than the version/);
  });

  it("inserts an Unreleased section when there is none, and refuses to date a missing one", () => {
    const log = "# Changelog\n\n## [1.2.3] - 2026-01-01\n- old\n";
    expect(bumpVersionFiles(base(log), "26.11.0").changelog).toBe(
      "# Changelog\n\n## [26.11.0] - Unreleased\n\n## [1.2.3] - 2026-01-01\n- old\n",
    );
    expect(() => bumpVersionFiles(base(log), "26.11.0", { releaseDate: "2026-11-03" })).toThrow(
      /no section for 26\.11\.0/,
    );
  });

  it("renames a pending Unreleased section to the new version", () => {
    const log = "# Changelog\n\n## [1.2.4] - Unreleased\n- new\n\n## [1.2.3] - 2026-01-01\n";
    expect(bumpVersionFiles(base(log), "26.11.0").changelog).toContain("## [26.11.0] - Unreleased\n- new");
  });

  it("refuses two Unreleased sections, a non-CalVer version and a malformed date", () => {
    const two = "## [1.2.4] - Unreleased\n\n## [1.2.5] - Unreleased\n";
    expect(() => bumpVersionFiles(base(two), "26.11.0")).toThrow(/2 "Unreleased" headings/);
    expect(() => bumpVersionFiles(base(""), "v26.11.0")).toThrow(/not CalVer/);
    expect(() => bumpVersionFiles(base(""), "26.11.0", { releaseDate: "3.11.2026" })).toThrow(/YYYY-MM-DD/);
  });
});

/**
 * End to end from a path with a space, where a wrong main-module guard exits 0
 * having done nothing (see release-preflight.mjs). The positive check is that
 * the files on disk changed and the preflight CLI then accepts the tag.
 */
describe("the bump CLI", () => {
  function copyTree() {
    const dir = mkdtempSync(join(tmpdir(), "bump version "));
    mkdirSync(join(dir, "scripts"));
    mkdirSync(join(dir, "src-tauri"));
    for (const f of [
      "package.json",
      "package-lock.json",
      "CHANGELOG.md",
      "src-tauri/Cargo.toml",
      "src-tauri/Cargo.lock",
      "src-tauri/tauri.conf.json",
      "scripts/release-preflight.mjs",
      "scripts/bump-version.mjs",
    ]) {
      copyFileSync(join(root, f), join(dir, f));
    }
    const changelog = join(dir, "CHANGELOG.md");
    writeFileSync(changelog, withPendingSection(readFileSync(changelog, "utf8")));
    return dir;
  }
  const node = (dir: string, ...args: string[]) =>
    spawnSync(process.execPath, args.map((a, i) => (i === 0 ? join(dir, a) : a)), { encoding: "utf8" });

  it("writes every file and leaves a tree the preflight CLI accepts", () => {
    const dir = copyTree();
    try {
      const run = node(dir, "scripts/bump-version.mjs", TARGET, "--release", "2026-11-03");
      expect(run.stderr).toBe("");
      expect(run.status).toBe(0);
      expect(run.stdout).toContain(`[OK] ${repoVersion()} -> ${TARGET}`);
      const written = readReleaseFiles(dir, (p) => readFileSync(p, "utf8"));
      expect((JSON.parse(written.tauriConf) as { version: string }).version).toBe(TARGET);
      const pre = node(dir, "scripts/release-preflight.mjs", `v${TARGET}`);
      expect(pre.status).toBe(0);
      expect(pre.stdout).toContain("[OK] Release preflight");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("fails without writing anything for a bad version", () => {
    const dir = copyTree();
    try {
      const before = readReleaseFiles(dir, (p) => readFileSync(p, "utf8"));
      const run = node(dir, "scripts/bump-version.mjs", "26.11");
      expect(run.status).toBe(1);
      expect(run.stderr).toContain("[FAIL]");
      expect(readReleaseFiles(dir, (p) => readFileSync(p, "utf8"))).toEqual(before);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
