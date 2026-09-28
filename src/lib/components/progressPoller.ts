/**
 * Polls a progress source while something is pending and hands each answer to
 * `onUpdate`. Browse Data runs one while a page request is outstanding, so a
 * Parquet sort that takes seconds shows how far it is.
 *
 * One request at a time: the next poll is scheduled only after the previous
 * answer arrives, so a slow backend cannot pile up requests. After `stop()`
 * nothing more is published, including an answer already in flight - a late
 * figure would otherwise overwrite the cleared display.
 */
export function startProgressPoller(
  poll: () => Promise<number | null>,
  onUpdate: (percent: number | null) => void,
  intervalMs = 250,
): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const tick = async () => {
    let value: number | null = null;
    try {
      value = await poll();
    } catch {
      // A failed poll shows no figure rather than ending the poller; the
      // request it describes reports its own errors.
      value = null;
    }
    if (stopped) return;
    onUpdate(value);
    timer = setTimeout(tick, intervalMs);
  };
  timer = setTimeout(tick, intervalMs);

  return () => {
    stopped = true;
    if (timer !== undefined) clearTimeout(timer);
  };
}

/** The pending label for a progress figure. */
export function progressLabel(percent: number | null): string {
  return percent === null ? "Loading..." : `Sorting... ${Math.floor(percent)}%`;
}
