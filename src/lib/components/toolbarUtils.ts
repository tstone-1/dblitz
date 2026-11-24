export const TINT_PRESETS: Array<{ value: string | null; name: string }> = [
  { value: null, name: "None" },
  { value: "#d94040", name: "Red" },
  { value: "#e0a030", name: "Amber" },
  { value: "#4aa84a", name: "Green" },
  { value: "#3080d0", name: "Blue" },
  { value: "#8050c0", name: "Purple" },
  { value: "#c04090", name: "Pink" },
];

const TINT_VALUES = new Set(
  TINT_PRESETS.map((preset) => preset.value).filter((value): value is string => value != null),
);

export function safeTint(value: string | null): string | null {
  return value && TINT_VALUES.has(value) ? value : null;
}

export function toolbarTintStyle(value: string | null): string {
  const tint = safeTint(value);
  return tint ? `background: color-mix(in srgb, ${tint} 22%, var(--bg-secondary));` : "";
}

export function tintPillStyle(value: string | null): string {
  const tint = safeTint(value);
  return tint ? `background: ${tint}; color: white; border-color: ${tint};` : "";
}

function displayPath(path: string, windows: boolean) {
  return windows ? path.replace(/\\/g, "/") : path;
}

const onWindows = typeof navigator !== "undefined" && /^Win/i.test(navigator.platform);

export function fileName(path: string | null, windows = onWindows): string {
  if (!path) return "No file open";
  const normalized = displayPath(path, windows);
  return normalized.slice(normalized.lastIndexOf("/") + 1);
}

export function parentDir(path: string, windows = onWindows): string {
  const normalized = displayPath(path, windows);
  const index = normalized.lastIndexOf("/");
  if (index === 0 || (windows && index === 2 && normalized[1] === ":")) {
    return normalized.slice(0, index + 1);
  }
  return index < 0 ? "" : normalized.slice(0, index);
}
