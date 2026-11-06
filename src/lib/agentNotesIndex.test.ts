import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

// Repo root, two levels up from src/lib.
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function readText(file: string): string {
  return readFileSync(join(root, file), "utf8").replace(/\r\n/g, "\n");
}

/**
 * AGENTS.md keeps a one-line index of `docs/agent-notes.md`, matched by title.
 * A heading renamed in the notes without its index line (or the reverse)
 * breaks nothing else, so this test is the only thing that notices: the Node
 * 24 -> 26 move renamed the heading and left the index line naming 24.
 *
 * The rules match the session-start index check the maintainer runs:
 * - every index title is a prefix of exactly one `##`/`###` heading
 *   (case-insensitive), so a hook may shorten a long heading;
 * - every `###` entry is reached by an index line, or sits under a `##` that is;
 * - every `##` is reached, or is a container whose `###` children are;
 * - an index bullet stays under 250 characters.
 */
const INDEX_MAX_CHARS = 250;

type Heading = { level: 2 | 3; title: string };

/** Index bullets `- *Title* — hook`, joined across wrapped lines. */
function indexBullets(text: string): string[] {
  const out: string[] = [];
  let cur: string | null = null;
  for (const line of text.split("\n")) {
    if (/^\s*- \*(?!\*)/.test(line)) {
      if (cur !== null) out.push(cur);
      cur = line;
    } else if (cur !== null && /^\s+\S/.test(line) && !/^\s*[-*] /.test(line)) {
      cur += " " + line.trim();
    } else if (cur !== null) {
      out.push(cur);
      cur = null;
    }
  }
  if (cur !== null) out.push(cur);
  return out.map((b) => b.replace(/\s+/g, " ").trim());
}

function indexTitles(text: string): { titles: string[]; unparsed: string[] } {
  const titles: string[] = [];
  const unparsed: string[] = [];
  for (const b of indexBullets(text)) {
    const m = /^- (\*(?!\*).+?\*(?!\*)(?: \/ \*(?!\*).+?\*(?!\*))*) — /.exec(b);
    if (!m) {
      unparsed.push(b);
      continue;
    }
    for (const part of m[1].split(" / ")) titles.push(part.replace(/^\*|\*$/g, "").trim());
  }
  return { titles, unparsed };
}

function headings(corpus: string): Heading[] {
  return [...corpus.matchAll(/^(#{2,3}) (.+?)\s*$/gm)].map((m) => ({
    level: m[1].length as 2 | 3,
    title: m[2],
  }));
}

const norm = (s: string) => s.replace(/\s+/g, " ").trim().toLowerCase();

/** Every way the index and the corpus disagree; empty means they agree. */
function indexProblems(index: string, corpus: string): string[] {
  const problems: string[] = [];
  const { titles, unparsed } = indexTitles(index);
  const heads = headings(corpus);
  if (titles.length === 0) problems.push("the index names no entries");
  if (heads.length === 0) problems.push("the corpus has no headings");
  for (const b of unparsed) problems.push(`index bullet does not parse as \`- *Title* — hook\`: ${b}`);
  for (const b of indexBullets(index)) {
    if (b.length > INDEX_MAX_CHARS) problems.push(`index bullet is ${b.length} chars: ${b.slice(0, 80)}`);
  }

  const reached = new Set<Heading>();
  for (const title of titles) {
    const n = norm(title);
    const hits = heads.filter((h) => norm(h.title).startsWith(n));
    if (hits.length === 1) reached.add(hits[0]);
    else if (hits.length === 0) problems.push(`index names an entry not in the corpus: ${title}`);
    else problems.push(`index title matches ${hits.length} headings: ${title}`);
  }

  let parent: Heading | null = null;
  let parentHasReachedChild = false;
  const closeParent = () => {
    if (parent && !reached.has(parent) && !parentHasReachedChild) {
      problems.push(`\`##\` entry no index line reaches: ${parent.title}`);
    }
  };
  for (const h of heads) {
    if (h.level === 2) {
      closeParent();
      parent = h;
      parentHasReachedChild = false;
    } else if (reached.has(h)) {
      parentHasReachedChild = true;
    } else if (!parent || !reached.has(parent)) {
      problems.push(`\`###\` entry no index line reaches: ${h.title}`);
    }
  }
  closeParent();
  return problems;
}

describe("AGENTS.md index of docs/agent-notes.md", () => {
  it("declares docs/agent-notes.md as its corpus", () => {
    expect(readText("AGENTS.md")).toContain("<!-- docs-index: docs/agent-notes.md -->");
  });

  it("agrees with the corpus in both directions", () => {
    const agents = readText("AGENTS.md");
    const index = agents.slice(agents.indexOf("\n## Index"));
    expect(indexProblems(index, readText("docs/agent-notes.md"))).toEqual([]);
  });

  // Controls: each direction must be able to fail, or a green run proves nothing.
  const corpus = "## Area\n\n### Node is pinned to 26\n\nbody\n\n### Other entry\n\nbody\n";
  const index = "- *Node is pinned to 26* — hook\n- *Other entry* — hook\n";

  it("passes a matching fixture", () => {
    expect(indexProblems(index, corpus)).toEqual([]);
  });

  it("fails an index line whose heading was renamed", () => {
    const stale = index.replace("pinned to 26", "pinned to 24");
    expect(indexProblems(stale, corpus)).toEqual([
      "index names an entry not in the corpus: Node is pinned to 24",
      "`###` entry no index line reaches: Node is pinned to 26",
    ]);
  });

  it("fails a corpus entry with no index line", () => {
    expect(indexProblems(index, corpus + "\n### New entry\n\nbody\n")).toEqual([
      "`###` entry no index line reaches: New entry",
    ]);
  });

  it("fails a `##` entry that is neither indexed nor a container", () => {
    expect(indexProblems(index, corpus + "\n## Loose section\n\nbody\n")).toEqual([
      "`##` entry no index line reaches: Loose section",
    ]);
  });

  it("fails an ambiguous title and an over-long bullet", () => {
    const twoNodes = corpus + "\n### Node is pinned to 26 on CI\n\nbody\n";
    expect(indexProblems(index, twoNodes)).toContain("index title matches 2 headings: Node is pinned to 26");
    const long = index + `- *Other entry* — ${"x".repeat(INDEX_MAX_CHARS)}\n`;
    expect(indexProblems(long, corpus).some((p) => p.startsWith("index bullet is "))).toBe(true);
  });
});
