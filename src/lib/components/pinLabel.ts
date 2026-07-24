/** Pin state shared by column filters and the global filter. */
export type PinState = "none" | "pinned" | "modified";

/**
 * Context-menu label for a pin toggle across its three states. `noun` is the
 * thing being pinned -- "filter" for a column filter, "global filter" for the
 * table-wide filter -- so one helper serves both the DataGrid column-pin menu
 * and the BrowseData global-pin menu, which previously duplicated this ternary.
 */
export function pinToggleLabel(state: PinState, noun: string): string {
  if (state === "pinned") return `Unpin ${noun}`;
  if (state === "modified") return `Re-pin ${noun} (save current value)`;
  return `Pin ${noun} (save as default)`;
}
