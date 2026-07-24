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
