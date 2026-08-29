import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  cargoPackageVersion,
  checkReleaseContract,
  readReleaseFiles,
} from "../../scripts/release-preflight.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const readText = (file: string) => readFileSync(file, "utf8");

/** A tree that agrees with itself at `version`, as a baseline to perturb. */
function consistentFiles(version: string, changelogDate = "2026-08-23") {
  return {
    packageJson: JSON.stringify({ name: "dblitz", version }),
    packageLock: JSON.stringify({
      name: "dblitz",
      version,
      packages: { "": { name: "dblitz", version } },
    }),
    cargoToml: `[package]\nname = "dblitz"\nversion = "${version}"\n\n[dependencies]\nserde = "1.0"\n`,
    tauriConf: JSON.stringify({ productName: "dblitz", version }),
    changelog: `# Changelog\n\n## [${version}] - ${changelogDate}\n\n### Fixed\n- something\n`,
  };
}

describe("release preflight", () => {
  it("accepts a tag every version file agrees with", () => {
    // Positive control. Without it, a check that rejects everything passes
    // every rejection test below while blocking all releases.
    expect(checkReleaseContract({ tag: "v26.8.2", ...consistentFiles("26.8.2") })).toEqual([]);
  });

  it("rejects a tag one version ahead of every manifest", () => {
    // The gap this closes: nothing in the release pipeline compared the tag to
    // the tree, so this combination published 26.8.1 artifacts as 26.8.2.
    const failures = checkReleaseContract({ tag: "v26.8.2", ...consistentFiles("26.8.1") });

    expect(failures.length).toBeGreaterThan(0);
    expect(failures.join("\n")).toContain("package.json version is 26.8.1, expected 26.8.2");
  });

  for (const [label, mutate] of [
    ["package.json", (f: ReturnType<typeof consistentFiles>) => {
      f.packageJson = JSON.stringify({ name: "dblitz", version: "26.8.1" });
    }],
    ["package-lock.json top level", (f: ReturnType<typeof consistentFiles>) => {
      const lock = JSON.parse(f.packageLock);
      lock.version = "26.8.1";
      f.packageLock = JSON.stringify(lock);
    }],
    ['package-lock.json packages[""]', (f: ReturnType<typeof consistentFiles>) => {
      const lock = JSON.parse(f.packageLock);
      lock.packages[""].version = "26.8.1";
      f.packageLock = JSON.stringify(lock);
    }],
    ["Cargo.toml", (f: ReturnType<typeof consistentFiles>) => {
      f.cargoToml = f.cargoToml.replace('version = "26.8.2"', 'version = "26.8.1"');
    }],
    ["tauri.conf.json", (f: ReturnType<typeof consistentFiles>) => {
      f.tauriConf = JSON.stringify({ productName: "dblitz", version: "26.8.1" });
    }],
  ] as const) {
    it(`rejects a tag when only ${label} lags behind`, () => {
      // One file at a time: a check that only looked at package.json would pass
      // four of these five while the release still shipped mismatched artifacts.
      const files = consistentFiles("26.8.2");
      mutate(files);
      expect(checkReleaseContract({ tag: "v26.8.2", ...files })).not.toEqual([]);
    });
  }

  it("rejects a changelog heading still marked Unreleased", () => {
    // The published release notes link to CHANGELOG.md, so this would ship a
    // release documenting itself as unreleased.
    const files = consistentFiles("26.8.2", "Unreleased");
    const failures = checkReleaseContract({ tag: "v26.8.2", ...files });
    expect(failures.join("\n")).toContain("must be a YYYY-MM-DD release date");
  });

  it("rejects a version with no changelog entry at all", () => {
    const files = consistentFiles("26.8.2");
    files.changelog = "# Changelog\n\n## [26.8.1] - 2026-08-23\n";
    expect(checkReleaseContract({ tag: "v26.8.2", ...files }).join("\n")).toContain(
      "no \"## [26.8.2] - <date>\" heading",
    );
  });

  it("rejects a tag that is not CalVer YY.M.MICRO", () => {
    for (const tag of ["v1.2.3", "26.8.2", "vlatest", "v2026.8.2"]) {
      expect(checkReleaseContract({ tag, ...consistentFiles("26.8.2") })).not.toEqual([]);
    }
  });

  it("reads the [package] version, not a dependency's", () => {
    // A bare /^version = /m sweep would answer with the dependency below.
    const toml = '[dependencies]\nfoo = { version = "9.9.9" }\n\n[package]\nversion = "26.8.2"\n';
    expect(cargoPackageVersion(toml)).toBe("26.8.2");
    expect(cargoPackageVersion("[dependencies]\nversion = \"9.9.9\"\n")).toBe(null);
  });

  it("passes against this repository's own files at its declared version", () => {
    // Guards the check itself against drifting away from the real file layout:
    // a renamed key or moved file would make every synthetic test above pass
    // while the CI gate answered "missing" for everything.
    const files = readReleaseFiles(root, readText);
    const version = JSON.parse(files.packageJson).version as string;
    expect(checkReleaseContract({ tag: `v${version}`, ...files })).toEqual([]);
  });
});

describe("release workflow wires the preflight in", () => {
  const workflow = readText(join(root, ".github/workflows/release.yml"));

  it("runs the preflight script", () => {
    expect(workflow).toContain("scripts/release-preflight.mjs");
  });

  it("blocks draft creation on it", () => {
    // A gate that runs beside `create-release` instead of before it lets the
    // draft exist anyway, which is the state this is meant to prevent.
    expect(/create-release:\s*\n\s*needs: \[preflight, quality\]/.test(workflow)).toBe(true);
  });
});
