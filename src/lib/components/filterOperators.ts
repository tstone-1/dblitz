export const OPERAND_REQUIRED_OPS = ["<", ">", ">=", "<=", "="] as const;

export const INCOMPLETE_OPS = new RegExp(`^(${OPERAND_REQUIRED_OPS.join("|")})$`);

// A column filter value is semicolon-separated (OR/AND list of segments; see
// the filter-syntax help in BrowseData). A single segment that is a bare
// operand-requiring operator (e.g. ">" with no number yet) is INCOMPLETE: the
// backend would reject or misinterpret it, and it's the transient state while
// the user is still typing an operator expression.

/** True if ANY non-empty segment of `value` is a bare operand-requiring operator. */
export function hasIncompleteSegment(value: string): boolean {
  return value.split(";").some((seg) => {
    const trimmed = seg.trim();
    return trimmed !== "" && INCOMPLETE_OPS.test(trimmed);
  });
}

/**
 * Regex-aware per-filter incompleteness check. A regex-mode filter is never
 * "incomplete" -- `<` is a legal regex pattern, not a pending operator -- and
 * an all-whitespace value is inert. Otherwise defer to `hasIncompleteSegment`.
 */
export function hasIncompleteOperator(value: string, isRegex: boolean): boolean {
  if (isRegex || value.trim() === "") return false;
  return hasIncompleteSegment(value);
}

/**
 * Drop bare operand-requiring operator segments from a (non-regex) filter
 * value, keeping the complete ones. `"foo;<"` -> `"foo"`, `"<"` -> `""`
 * (an empty result is then dropped entirely by buildActiveFilters). Lets a
 * discrete action (a sort click) reload with the still-valid segments instead
 * of being blocked outright by one half-typed operator.
 */
export function stripIncompleteSegments(value: string): string {
  return value
    .split(";")
    .filter((seg) => {
      const trimmed = seg.trim();
      return trimmed !== "" && !INCOMPLETE_OPS.test(trimmed);
    })
    .join(";");
}

// Characters with a meaning in Rust's `regex` crate outside a character class.
// `-` is only special inside `[...]`, so a part number like GAN111-650WSB is
// left readable in the resulting pattern.
const REGEX_META = /[\\^$.*+?()[\]{}|]/g;

export function escapeRegex(text: string): string {
  return text.replace(REGEX_META, "\\$&");
}

/**
 * A column filter after pasting clipboard text, or null when the paste should
 * be left to the browser.
 *
 * An `<input>` silently deletes the line breaks of a pasted list, so a column
 * copied out of Excel would become one long run-together string that matches
 * nothing. Two or more non-empty lines are instead turned into a regex that
 * matches any of them: each line trimmed, escaped and joined with `|`,
 * duplicates dropped. In regex mode the pattern replaces the selection, like
 * any paste. In text mode the existing text would change meaning under regex
 * mode, so the pasted list becomes the whole filter.
 */
export function filterAfterListPaste(
  current: { value: string; is_regex: boolean } | undefined,
  selectionStart: number | null,
  selectionEnd: number | null,
  pasted: string,
): { value: string; is_regex: boolean } | null {
  const lines = pasted
    .split(/\r\n|\r|\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "");
  if (lines.length < 2) return null;
  const pattern = [...new Set(lines)].map(escapeRegex).join("|");
  if (!current?.is_regex) return { value: pattern, is_regex: true };
  const start = selectionStart ?? current.value.length;
  const end = selectionEnd ?? start;
  return {
    value: current.value.slice(0, start) + pattern + current.value.slice(end),
    is_regex: true,
  };
}
