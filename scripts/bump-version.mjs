/**
 * Set the release version in all five version files at once, and keep the
 * CHANGELOG heading in step.
 *
 *   node scripts/bump-version.mjs next                 # next CalVer from today's date
 *   node scripts/bump-version.mjs 26.11.0              # an explicit version
 *   node scripts/bump-version.mjs next --release       # ...and date the CHANGELOG heading today
 *   node scripts/bump-version.mjs 26.11.0 --release 2026-11-03
 *
 * The five files (six sites) drift independently and nothing fails when one
 * does; see `release-preflight.mjs`, which this script runs on what it wrote.
 * With `--release` that is the full check CI's `preflight` job runs for the tag,
 * so a green bump means the tag will be accepted.
 *
 * Every edit is textual, so formatting, key order and line endings stay as
 * they are. Each JSON edit is verified by re-parsing and comparing everything
 * except the version fields. Cargo.lock gets the one-line change `cargo check`
 * would make to the crate's own entry, and nothing else.
 *
 * CHANGELOG: an existing `## [<old>] - Unreleased` heading is renamed to the new
 * version (unreleased notes belong to the next release); with none, an empty
 * `## [<new>] - Unreleased` section is inserted above the newest entry.
 * `--release` then replaces `Unreleased` with the date, and refuses when there
 * is no section for the version, since a dated empty section publishes nothing.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  cargoPackageName,
  checkReleaseContract,
  checkVersionFiles,
  isMainModule,
  readReleaseFiles,
} from "./release-preflight.mjs";

const CALVER = /^(\d{2})\.(\d{1,2})\.(\d+)$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

/**
 * The CalVer that follows `current` on `today`: the next MICRO within the same
 * month, else `.0` of the new month.
 *
 * @param {string} current
 * @param {Date} today
 * @returns {string}
 */
export function nextVersion(current, today) {
  const m = CALVER.exec(current);
  if (!m) throw new Error(`current version "${current}" is not CalVer YY.M.MICRO`);
  const yy = today.getFullYear() % 100;
  const month = today.getMonth() + 1;
  if (Number(m[1]) === yy && Number(m[2]) === month) {
    return `${yy}.${month}.${Number(m[3]) + 1}`;
  }
  if (Number(m[1]) * 100 + Number(m[2]) > yy * 100 + month) {
    throw new Error(`current version ${current} is later than today's month`);
  }
  return `${yy}.${month}.0`;
}

/**
 * Replace the capture group `value` of the first match of `re` at or after
 * `from`, failing when there is none.
 *
 * @param {string} text
 * @param {RegExp} re   Must have groups (prefix)(value)(suffix).
 * @param {string} value
 * @param {string} label
 * @param {number} [from]
 */
function replaceFirst(text, re, value, label, from = 0) {
  const flags = re.flags.includes("g") ? re.flags : `${re.flags}g`;
  const g = new RegExp(re.source, flags);
  g.lastIndex = from;
  const m = g.exec(text);
  if (!m) throw new Error(`${label}: version not found`);
  return text.slice(0, m.index) + m[1] + value + m[3] + text.slice(m.index + m[0].length);
}

/**
 * Bounds [start, end) of the TOML table whose header line matches `header`
 * and whose body satisfies `accept`.
 *
 * @param {string} toml
 * @param {RegExp} header   Anchored, multiline, e.g. /^\[package\]$/m.
 * @param {(body: string) => boolean} [accept]
 * @returns {[number, number][]}
 */
function tomlTables(toml, header, accept = () => true) {
  /** @type {[number, number][]} */
  const out = [];
  const g = new RegExp(header.source, "gm");
  for (let m; (m = g.exec(toml)); ) {
    const start = m.index + m[0].length;
    const next = /^\[/m.exec(toml.slice(start));
    const end = next ? start + next.index : toml.length;
    if (accept(toml.slice(start, end))) out.push([start, end]);
  }
  return out;
}

/**
 * JSON equality after setting the version fields of both sides to the same
 * value, so the only difference an edit may make is the version.
 *
 * @param {string} before
 * @param {string} after
 * @param {(o: any) => void} clearVersions
 * @param {string} label
 */
function assertOnlyVersionChanged(before, after, clearVersions, label) {
  const a = JSON.parse(before);
  const b = JSON.parse(after);
  clearVersions(a);
  clearVersions(b);
  if (JSON.stringify(a) !== JSON.stringify(b)) {
    throw new Error(`${label}: the edit changed more than the version`);
  }
}

/**
 * @typedef {ReturnType<typeof readReleaseFiles>} ReleaseFiles
 */

/**
 * Pure: the five version files and the changelog with `version` applied.
 *
 * @param {ReleaseFiles} files
 * @param {string} version
 * @param {{ releaseDate?: string }} [options]
 * @returns {ReleaseFiles}
 */
export function bumpVersionFiles(files, version, options = {}) {
  if (!CALVER.test(version)) throw new Error(`"${version}" is not CalVer YY.M.MICRO`);
  const { releaseDate } = options;
  if (releaseDate !== undefined && !DATE.test(releaseDate)) {
    throw new Error(`release date "${releaseDate}" is not YYYY-MM-DD`);
  }
  const VERSION_KEY = /("version"\s*:\s*")([^"]*)(")/;

  const packageJson = replaceFirst(files.packageJson, VERSION_KEY, version, "package.json");
  assertOnlyVersionChanged(files.packageJson, packageJson, (o) => (o.version = ""), "package.json");

  // The lockfile's top-level "version" comes first; packages[""] is the root
  // package's own entry, the first key under "packages".
  let packageLock = replaceFirst(files.packageLock, VERSION_KEY, version, "package-lock.json");
  const root = /"packages"\s*:\s*\{\s*""\s*:\s*\{/.exec(packageLock);
  if (!root) throw new Error('package-lock.json: no packages[""] entry');
  packageLock = replaceFirst(packageLock, VERSION_KEY, version, 'package-lock.json packages[""]', root.index);
  assertOnlyVersionChanged(
    files.packageLock,
    packageLock,
    (o) => {
      o.version = "";
      o.packages[""].version = "";
    },
    "package-lock.json",
  );

  const TOML_VERSION = /^(version\s*=\s*")([^"]*)(")/m;
  const [pkg] = tomlTables(files.cargoToml, /^\[package\]$/m);
  if (!pkg) throw new Error("src-tauri/Cargo.toml: no [package] table");
  const cargoToml =
    files.cargoToml.slice(0, pkg[0]) +
    replaceFirst(files.cargoToml.slice(pkg[0], pkg[1]), TOML_VERSION, version, "src-tauri/Cargo.toml") +
    files.cargoToml.slice(pkg[1]);

  const crate = cargoPackageName(files.cargoToml);
  const locked = tomlTables(
    files.cargoLock,
    /^\[\[package\]\]$/m,
    (body) => /^name\s*=\s*"([^"]+)"/m.exec(body)?.[1] === crate,
  );
  if (locked.length !== 1) {
    throw new Error(`src-tauri/Cargo.lock: expected one [[package]] entry for "${crate}", found ${locked.length}`);
  }
  const [[ls, le]] = locked;
  const cargoLock =
    files.cargoLock.slice(0, ls) +
    replaceFirst(files.cargoLock.slice(ls, le), TOML_VERSION, version, "src-tauri/Cargo.lock") +
    files.cargoLock.slice(le);

  const tauriConf = replaceFirst(files.tauriConf, VERSION_KEY, version, "src-tauri/tauri.conf.json");
  assertOnlyVersionChanged(files.tauriConf, tauriConf, (o) => (o.version = ""), "src-tauri/tauri.conf.json");

  return {
    packageJson,
    packageLock,
    cargoToml,
    cargoLock,
    tauriConf,
    changelog: bumpChangelog(files.changelog, version, releaseDate),
  };
}

/**
 * @param {string} changelog
 * @param {string} version
 * @param {string | undefined} releaseDate
 */
function bumpChangelog(changelog, version, releaseDate) {
  const esc = version.replace(/\./g, "\\.");
  const own = new RegExp(`^(## \\[${esc}\\] - )(.+)$`, "m");
  const unreleased = /^(## \[)(\d+\.\d+\.\d+)(\] - Unreleased)$/gm;
  const pending = [...changelog.matchAll(unreleased)];
  if (pending.length > 1) {
    throw new Error(`CHANGELOG.md has ${pending.length} "Unreleased" headings; expected at most one`);
  }

  let text = changelog;
  if (!own.test(text)) {
    if (pending.length === 1) {
      text = text.replace(unreleased, `$1${version}$3`);
    } else if (releaseDate !== undefined) {
      throw new Error(`CHANGELOG.md has no section for ${version}; write its notes before --release`);
    } else {
      const newest = /^## \[/m.exec(text);
      const at = newest ? newest.index : text.length;
      text = `${text.slice(0, at)}## [${version}] - Unreleased\n\n${text.slice(at)}`;
    }
  }
  if (releaseDate !== undefined) text = text.replace(own, `$1${releaseDate}`);
  return text;
}

// CLI. The guard is the preflight's; see its comment for why a path
// comparison is not enough.
if (isMainModule(import.meta)) {
  const args = process.argv.slice(2);
  const usage = "usage: node scripts/bump-version.mjs <YY.M.MICRO|next> [--release [YYYY-MM-DD]]";
  const target = args[0];
  if (!target || target.startsWith("-")) {
    console.error(usage);
    process.exit(2);
  }
  const r = args.indexOf("--release");
  /** @type {string | undefined} */
  let releaseDate;
  if (r >= 0) {
    const given = args[r + 1];
    const now = new Date();
    const pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
    releaseDate =
      given && !given.startsWith("-")
        ? given
        : `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  }

  const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
  const paths = {
    packageJson: "package.json",
    packageLock: "package-lock.json",
    cargoToml: "src-tauri/Cargo.toml",
    cargoLock: "src-tauri/Cargo.lock",
    tauriConf: "src-tauri/tauri.conf.json",
    changelog: "CHANGELOG.md",
  };
  const before = readReleaseFiles(repo, (p) => readFileSync(p, "utf8"));
  const current = JSON.parse(before.packageJson).version;
  let version;
  let after;
  try {
    version = target === "next" ? nextVersion(current, new Date()) : target;
    after = bumpVersionFiles(before, version, { releaseDate });
  } catch (e) {
    console.error(`[FAIL] ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }

  for (const [key, rel] of Object.entries(paths)) {
    const k = /** @type {keyof typeof paths} */ (key);
    if (after[k] !== before[k]) writeFileSync(join(repo, rel), after[k], "utf8");
  }

  // Read back what is now on disk, not what was computed.
  const written = readReleaseFiles(repo, (p) => readFileSync(p, "utf8"));
  const failures =
    releaseDate === undefined
      ? checkVersionFiles(version, written)
      : checkReleaseContract({ tag: `v${version}`, ...written });
  if (failures.length > 0) {
    console.error(`[FAIL] After bumping to ${version}:`);
    for (const f of failures) console.error(`  - ${f}`);
    process.exit(1);
  }
  const changed = Object.entries(paths)
    .filter(([k]) => after[/** @type {keyof typeof paths} */ (k)] !== before[/** @type {keyof typeof paths} */ (k)])
    .map(([, p]) => p);
  console.log(`[OK] ${current} -> ${version}; wrote ${changed.join(", ") || "nothing (already there)"}.`);
  console.log(
    releaseDate === undefined
      ? "     Version files agree. Re-run with --release to date the CHANGELOG heading."
      : `     CHANGELOG dated ${releaseDate}; release preflight passes for v${version}.`,
  );
}
