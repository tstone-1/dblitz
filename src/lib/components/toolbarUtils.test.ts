import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  fileName,
  parentDir,
  safeTint,
  TINT_PRESETS,
  tintPillStyle,
  toolbarTintStyle,
} from "./toolbarUtils";

describe("toolbar utilities", () => {
  it("accepts only configured tint values", () => {
    expect(safeTint("#d94040")).toBe("#d94040");
    expect(safeTint("#ffffff")).toBeNull();
    expect(safeTint(null)).toBeNull();
  });

  it("keeps tint presets in sync with the backend whitelist", () => {
    const rustConfig = readFileSync("src-tauri/src/config.rs", "utf8");
    const match = rustConfig.match(/pub const TINT_PRESETS: &\[&str\] = &\[(?<values>[^\]]+)\]/);
    expect(match?.groups?.values).toBeTruthy();
    const backendValues = [...match!.groups!.values.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect(TINT_PRESETS.map((preset) => preset.value).filter(Boolean)).toEqual(backendValues);
  });

  it("builds tint styles only for safe tint values", () => {
    expect(toolbarTintStyle("#d94040")).toContain("#d94040");
    expect(tintPillStyle("#d94040")).toContain("border-color: #d94040");
    expect(toolbarTintStyle("#ffffff")).toBe("");
    expect(tintPillStyle(null)).toBe("");
  });

  it("extracts filenames from POSIX and Windows paths", () => {
    expect(fileName("/tmp/example.sqlite")).toBe("example.sqlite");
    expect(fileName("C:\\data\\example.sqlite")).toBe("example.sqlite");
    expect(fileName(null)).toBe("No file open");
  });

  it("extracts parent directories from POSIX and Windows paths", () => {
    expect(parentDir("/tmp/example.sqlite")).toBe("/tmp");
    expect(parentDir("C:\\data\\example.sqlite")).toBe("C:/data");
    expect(parentDir("example.sqlite")).toBe("");
  });
});
