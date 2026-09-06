import { describe, expect, it } from "vitest";
import { isMacPlatform, modKeyLabel } from "./platformKeys";

describe("isMacPlatform", () => {
  it("detects macOS from navigator.platform", () => {
    expect(isMacPlatform("MacIntel", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
  });

  it("rejects Windows and Linux", () => {
    expect(isMacPlatform("Win32", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false);
    expect(isMacPlatform("Linux x86_64", "Mozilla/5.0 (X11; Linux x86_64)")).toBe(false);
  });

  it("falls back to the user agent when platform is unavailable", () => {
    // `navigator.platform` is deprecated and a WebView may stop reporting it.
    expect(isMacPlatform(undefined, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(true);
    expect(isMacPlatform("", "Mozilla/5.0 (Windows NT 10.0)")).toBe(false);
  });

  it("does not read macOS out of a Windows user agent", () => {
    // Emptiness control for the regex: "Mac" appears inside plenty of strings
    // that are not macOS, so the fallback is word-anchored.
    expect(isMacPlatform(undefined, "Mozilla/5.0 (Windows NT 10.0) MacGyver/1.0")).toBe(false);
  });
});

describe("modKeyLabel", () => {
  it("names Cmd on macOS and Ctrl elsewhere", () => {
    expect(modKeyLabel(true)).toBe("Cmd");
    expect(modKeyLabel(false)).toBe("Ctrl");
  });
});
