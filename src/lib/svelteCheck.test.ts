import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { checkVerdict } from "../../scripts/svelte-check.mjs";

const completed = "1790240993419 COMPLETED 12 FILES 0 ERRORS 0 WARNINGS 0 FILES_WITH_PROBLEMS\n";

describe("svelte-check wrapper", () => {
  it("passes a run that finished clean", () => {
    expect(checkVerdict(0, `1 START "x"\n${completed}`)).toBe(0);
  });

  it("fails a zero exit with no summary, which is how svelte-check reports a crash", () => {
    const crash = "Error: svelte-check --tsgo requires TypeScript 7 to be installed\nsvelte-check failed\n";
    expect(checkVerdict(0, crash)).toBe(1);
  });

  it("keeps svelte-check's own failure code", () => {
    expect(checkVerdict(1, completed.replace("0 ERRORS", "2 ERRORS"))).toBe(1);
    expect(checkVerdict(null, "")).toBe(1);
  });

  it("is what npm run check runs", () => {
    const pkg = JSON.parse(readFileSync("package.json", "utf8"));
    expect(pkg.scripts.check).toContain("node scripts/svelte-check.mjs");
    expect(pkg.scripts.check).not.toMatch(/&&\s*svelte-check\b/);
  });
});
