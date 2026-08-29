/**
 * Fail-closed check that a release tag names the version the repository is
 * actually about to publish.
 *
 * Pushing any `v*` tag starts `.github/workflows/release.yml`. Everything after
 * that point is thorough - the quality gate, draft-first publication, signing,
 * notarization, updater-manifest assembly - and none of it looks at the tag. So
 * a `v26.8.2` tag on a tree whose manifests still say `26.8.1` produces a
 * 26.8.2 release containing 26.8.1 installers and 26.8.1 updater metadata, with
 * nothing red anywhere. BUILD.md asks a human to compare the versions by hand;
 * this is the same comparison, run by CI before the draft release exists.
 *
 * Kept dependency-free and pure (text in, failures out) so the same logic runs
 * in CI, from a shell before tagging, and under vitest against synthetic files.
 */

/** CalVer `YY.M.MICRO`, the scheme declared in BUILD.md and AGENTS.md. */
const CALVER = /^(\d{2})\.(\d{1,2})\.(\d+)$/;

/**
 * @typedef {object} ReleaseContractInput
 * @property {string} tag              The pushed tag, e.g. `v26.8.2`.
 * @property {string} packageJson      Raw `package.json`.
 * @property {string} packageLock      Raw `package-lock.json`.
 * @property {string} cargoToml        Raw `src-tauri/Cargo.toml`.
 * @property {string} cargoLock        Raw `src-tauri/Cargo.lock`.
 * @property {string} tauriConf        Raw `src-tauri/tauri.conf.json`.
 * @property {string} changelog        Raw `CHANGELOG.md`.
 */

/**
 * The `[package]` version from a Cargo manifest.
 *
 * Scoped to the `[package]` table on purpose: a bare `^version = ` sweep would
 * also match a `[dependencies]` entry written across lines and silently compare
 * the wrong number.
 *
 * @param {string} toml
 * @returns {string | null}
 */
export function cargoPackageVersion(toml) {
  const section = /^\[package\]$/m.exec(toml);
  if (!section) return null;
  const rest = toml.slice(section.index + section[0].length);
  const nextSection = /^\[/m.exec(rest);
  const body = nextSection ? rest.slice(0, nextSection.index) : rest;
  return /^version\s*=\s*"([^"]+)"/m.exec(body)?.[1] ?? null;
}

/**
 * The `[package]` name from a Cargo manifest.
 *
 * Read rather than hard-coded, so the lockfile lookup below stays coupled to
 * the crate this repository actually builds. A rename that touched only
 * `Cargo.toml` would otherwise leave the lock check silently searching for a
 * package that no longer exists.
 *
 * @param {string} toml
 * @returns {string | null}
 */
export function cargoPackageName(toml) {
  const section = /^\[package\]$/m.exec(toml);
  if (!section) return null;
  const rest = toml.slice(section.index + section[0].length);
  const nextSection = /^\[/m.exec(rest);
  const body = nextSection ? rest.slice(0, nextSection.index) : rest;
  return /^name\s*=\s*"([^"]+)"/m.exec(body)?.[1] ?? null;
}

/**
 * Every version `Cargo.lock` records for a package name.
 *
 * Returns an array because a lockfile may legitimately carry one name at
 * several versions (two majors of a shared dependency), and collapsing that to
 * a single answer would pick one arbitrarily. The caller requires exactly one
 * entry for the crate being released.
 *
 * Parsing is anchored inside each `[[package]]` block: `name = ` at the start
 * of a line. A dependency merely LISTED by that name sits inside a
 * `dependencies = [ ... ]` array whose entries are indented, so it cannot be
 * mistaken for a package declaration.
 *
 * @param {string} lock
 * @param {string} name
 * @returns {string[]}
 */
export function cargoLockVersions(lock, name) {
  /** @type {string[]} */
  const versions = [];
  for (const block of lock.split(/^\[\[package\]\]$/m).slice(1)) {
    const end = /^\[/m.exec(block);
    const body = end ? block.slice(0, end.index) : block;
    if (/^name\s*=\s*"([^"]+)"/m.exec(body)?.[1] !== name) continue;
    const version = /^version\s*=\s*"([^"]+)"/m.exec(body)?.[1];
    if (version !== undefined) versions.push(version);
  }
  return versions;
}

/**
 * Compare a release tag against every version file and the changelog.
 *
 * @param {ReleaseContractInput} input
 * @returns {string[]} One message per violation; empty means the tag is safe to release.
 */
export function checkReleaseContract(input) {
  /** @type {string[]} */
  const failures = [];

  if (!input.tag.startsWith("v")) {
    failures.push(`Tag "${input.tag}" does not start with "v".`);
    return failures;
  }
  const version = input.tag.slice(1);
  if (!CALVER.test(version)) {
    failures.push(`Tag "${input.tag}" is not CalVer YY.M.MICRO (e.g. v26.8.2).`);
    return failures;
  }

  /** @type {(label: string, actual: string | null | undefined) => void} */
  const expect = (label, actual) => {
    if (actual !== version) {
      failures.push(`${label} is ${actual ?? "missing"}, expected ${version} (from tag ${input.tag}).`);
    }
  };

  /** @type {(label: string, text: string) => any} */
  const parseJson = (label, text) => {
    try {
      return JSON.parse(text);
    } catch (e) {
      failures.push(`${label} is not valid JSON: ${String(e)}`);
      return null;
    }
  };

  const pkg = parseJson("package.json", input.packageJson);
  if (pkg) expect("package.json version", pkg.version);

  // Both lockfile sites, because they drift independently and nothing fails
  // when they do - that is exactly how 26.7.6 shipped with the lockfile still
  // on 26.7.5.
  const lock = parseJson("package-lock.json", input.packageLock);
  if (lock) {
    expect("package-lock.json version", lock.version);
    expect('package-lock.json packages[""].version', lock.packages?.[""]?.version);
  }

  expect("src-tauri/Cargo.toml version", cargoPackageVersion(input.cargoToml));

  // Cargo.lock carries the crate's own version too, and it is committed. Bumping
  // Cargo.toml alone leaves it behind: nothing fails, because the next build
  // simply regenerates it - which is the same "silent drift" shape that let
  // 26.7.6 ship on a stale package-lock.json. `cargo check` refreshes it.
  const crate = cargoPackageName(input.cargoToml);
  if (!crate) {
    failures.push("src-tauri/Cargo.toml has no [package] name, so Cargo.lock cannot be checked.");
  } else {
    const locked = cargoLockVersions(input.cargoLock, crate);
    if (locked.length === 0) {
      failures.push(`src-tauri/Cargo.lock has no [[package]] entry for "${crate}".`);
    } else if (locked.length > 1) {
      failures.push(
        `src-tauri/Cargo.lock has ${locked.length} [[package]] entries for "${crate}" (${locked.join(", ")}); expected exactly one.`,
      );
    } else {
      expect("src-tauri/Cargo.lock version", locked[0]);
    }
  }

  const tauri = parseJson("src-tauri/tauri.conf.json", input.tauriConf);
  if (tauri) expect("src-tauri/tauri.conf.json version", tauri.version);

  // The release notes point readers at CHANGELOG.md, so a heading still marked
  // Unreleased publishes a release that documents itself as unreleased.
  const heading = new RegExp(
    `^## \\[${version.replace(/\./g, "\\.")}\\] - (.+)$`,
    "m",
  ).exec(input.changelog);
  if (!heading) {
    failures.push(`CHANGELOG.md has no "## [${version}] - <date>" heading.`);
  } else if (!/^\d{4}-\d{2}-\d{2}$/.test(heading[1].trim())) {
    failures.push(
      `CHANGELOG.md heading for ${version} reads "${heading[1].trim()}"; it must be a YYYY-MM-DD release date.`,
    );
  }

  return failures;
}

/**
 * Read the repository's version files.
 *
 * @param {string} root
 * @param {(path: string) => string} readText
 * @returns {Omit<ReleaseContractInput, "tag">}
 */
export function readReleaseFiles(root, readText) {
  const at = (/** @type {string} */ p) => readText(`${root}/${p}`);
  return {
    packageJson: at("package.json"),
    packageLock: at("package-lock.json"),
    cargoToml: at("src-tauri/Cargo.toml"),
    cargoLock: at("src-tauri/Cargo.lock"),
    tauriConf: at("src-tauri/tauri.conf.json"),
    changelog: at("CHANGELOG.md"),
  };
}

// CLI: `node scripts/release-preflight.mjs v26.8.2`
if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  const { readFileSync } = await import("node:fs");
  const { dirname, join } = await import("node:path");
  const { fileURLToPath } = await import("node:url");

  const tag = process.argv[2];
  if (!tag) {
    console.error("usage: node scripts/release-preflight.mjs <tag>");
    process.exit(2);
  }
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const files = readReleaseFiles(root, (p) => readFileSync(p, "utf8"));
  const failures = checkReleaseContract({ tag, ...files });

  if (failures.length > 0) {
    console.error(`[FAIL] Release preflight rejected ${tag}:`);
    for (const failure of failures) console.error(`  - ${failure}`);
    console.error("\nNo release was created. Fix the version files (or the tag) and retag.");
    process.exit(1);
  }
  console.log(`[OK] Release preflight: ${tag} matches every version file and a dated changelog entry.`);
}
