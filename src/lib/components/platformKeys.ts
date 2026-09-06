/**
 * Which modifier key the UI should NAME in a shortcut hint.
 *
 * The bindings themselves accept both (`e.ctrlKey || e.metaKey` everywhere in
 * this app, and CodeMirror gets an explicit Ctrl-Enter *and* Meta-Enter), so
 * this is purely about the label. Getting it wrong is not cosmetic: "Ctrl+Enter
 * to execute" is, on a Mac, an instruction to press the one combination most
 * users there never press, and the discoverable one (Cmd+Enter) goes unnamed.
 *
 * `navigator.platform` is deprecated but is still the most reliable macOS
 * signal in a WebView, so it is consulted first and the user-agent string is
 * the fallback. Both are passed in rather than read here, so this stays a pure
 * function with a test.
 */

export function isMacPlatform(
  platform: string | undefined,
  userAgent: string | undefined,
): boolean {
  if (platform && /^(Mac|iPhone|iPad|iPod)/i.test(platform)) return true;
  if (platform) return false;
  return /\b(Macintosh|Mac OS X)\b/.test(userAgent ?? "");
}

/** "Cmd" on macOS, "Ctrl" everywhere else. */
export function modKeyLabel(isMac: boolean): string {
  return isMac ? "Cmd" : "Ctrl";
}

/** Reads the running environment; returns "Ctrl" when there is no navigator. */
export function currentModKeyLabel(): string {
  if (typeof navigator === "undefined") return "Ctrl";
  return modKeyLabel(isMacPlatform(navigator.platform, navigator.userAgent));
}
