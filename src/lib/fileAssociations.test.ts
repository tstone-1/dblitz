import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * File associations differ by platform on purpose. The installer associates
 * SQLite and Parquet everywhere; CSV and JSON belong to spreadsheets and
 * editors, so on Windows they are an opt-in "Open with" entry the app writes
 * (`src-tauri/src/platform/open_with.rs`), and on macOS a declaration ranked
 * `Alternate`, which lists dblitz in Finder's "Open With" without ever making
 * it the default. A platform config replaces the whole `fileAssociations`
 * array (JSON merge patch), so the macOS file repeats the shared entries.
 */
type Association = { ext: string[]; role?: string; rank?: string; contentTypes?: string[] };

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
const associations = (path: string): Association[] => JSON.parse(read(path)).bundle.fileAssociations;

const main = associations("src-tauri/tauri.conf.json");
const macos = associations("src-tauri/tauri.macos.conf.json");
// The extensions the Windows setting writes, read from the Rust source.
const openWith = [...read("src-tauri/src/platform/open_with.rs").matchAll(/^\s*\("(\w+)", "[^"]+"\),$/gm)].map(
  (m) => m[1],
);

describe("file associations", () => {
  it("associates no CSV or JSON extension on any platform by default", () => {
    expect(openWith).toEqual(["csv", "tsv", "json", "jsonl", "ndjson"]);
    const everywhere = main.flatMap((a) => a.ext);
    expect(everywhere).toContain("parquet");
    for (const ext of openWith) expect(everywhere).not.toContain(ext);
  });

  it("repeats the shared entries unchanged on macOS", () => {
    expect(macos.slice(0, main.length)).toEqual(main);
  });

  it("adds exactly the CSV and JSON extensions on macOS, as an alternate viewer", () => {
    const extra = macos.slice(main.length);
    expect(extra.flatMap((a) => a.ext).sort()).toEqual([...openWith].sort());
    for (const a of extra) {
      expect(a.rank).toBe("Alternate");
      expect(a.role).toBe("Viewer");
    }
  });

  it("gives a macOS entry with a content type one extension only", () => {
    // macOS ignores CFBundleTypeExtensions when LSItemContentTypes is set, and
    // the bundler infers public.json for "json": grouping jsonl with it would
    // drop jsonl silently.
    for (const a of macos.slice(main.length)) {
      if (a.contentTypes || a.ext.includes("json")) expect(a.ext).toHaveLength(1);
    }
  });
});
