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

export function fileName(path: string | null): string {
  if (!path) return "No file open";
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1];
}

export function parentDir(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const index = normalized.lastIndexOf("/");
  return index > 0 ? normalized.slice(0, index) : "";
}
