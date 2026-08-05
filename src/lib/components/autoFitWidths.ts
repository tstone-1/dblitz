/**
 * The one decision behind "auto-fit this table's column widths on first open".
 *
 * `BrowseData.selectTable()` awaits the first chunk before it can decide
 * anything about widths, and `applyAutoWidths()` measures and persists through
 * the component's LIVE `selectedTable` -- not the table the awaiting call was
 * opening. So the decision has two halves, and only one of them is about
 * widths:
 *
 *  1. Is this continuation still the current selection? Clicking table A (slow
 *     first load) and then table B before A's chunk lands resumes A's tail with
 *     `selectedTable === "B"`. A's tail saw A's widths were unset and called
 *     `applyAutoWidths()`, which measured B's grid -- or, if B's first chunk had
 *     not arrived either, B's bare headers -- and wrote the result into B's
 *     config, silently discarding the user's hand-tuned saved widths for B.
 *
 *  2. Does the requested table already have saved widths? Auto-fit is a
 *     first-open convenience; a table the user has sized by hand keeps its
 *     sizes.
 *
 * Both live here rather than in the component so the race can be exercised by a
 * plain function call -- the component's own version was only reachable through
 * a real click sequence against a slow query.
 *
 * NOTE for future edits: this is used as the guard on the post-await tail of
 * `selectTable`, and `false` covers both "superseded" and "nothing to do".
 * Tail work that must still run for a table with saved widths has to be
 * sequenced ABOVE the guard, under its own `requestedTable === currentTable`
 * check.
 */

export interface AutoFitWidthsInput {
  /** Table the in-flight `selectTable()` call was opening. */
  requestedTable: string;
  /** Table selected right now, after the await -- `null` when none is. */
  currentTable: string | null;
  /** Saved widths for `requestedTable`; may be absent in an older config. */
  savedWidths: Record<string, number> | null | undefined;
}

export function shouldAutoFitWidths({
  requestedTable,
  currentTable,
  savedWidths,
}: AutoFitWidthsInput): boolean {
  // The selection moved on while the reload was in flight: this continuation
  // no longer owns the grid it would measure and the config it would write.
  if (currentTable !== requestedTable) return false;
  return !savedWidths || Object.keys(savedWidths).length === 0;
}
