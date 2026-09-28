import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { progressLabel, startProgressPoller } from "./progressPoller";

describe("startProgressPoller", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("publishes each answer, one request at a time", async () => {
    const answers = [10, 55.5, null];
    const poll = vi.fn(() => Promise.resolve(answers.shift() ?? null));
    const seen: (number | null)[] = [];
    const stop = startProgressPoller(poll, (p) => seen.push(p), 100);

    expect(poll).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(100);
    await vi.advanceTimersByTimeAsync(100);
    await vi.advanceTimersByTimeAsync(100);
    expect(seen).toEqual([10, 55.5, null]);
    expect(poll).toHaveBeenCalledTimes(3);
    stop();
  });

  it("does not start a second request while one is outstanding", async () => {
    let release: (v: number) => void = () => {};
    const poll = vi.fn(
      () => new Promise<number | null>((resolve) => (release = resolve)),
    );
    const stop = startProgressPoller(poll, () => {}, 50);
    await vi.advanceTimersByTimeAsync(500);
    expect(poll).toHaveBeenCalledTimes(1);
    release(5);
    await vi.advanceTimersByTimeAsync(50);
    expect(poll).toHaveBeenCalledTimes(2);
    stop();
  });

  it("publishes nothing after stop, not even an answer already in flight", async () => {
    let release: (v: number) => void = () => {};
    const poll = () => new Promise<number | null>((resolve) => (release = resolve));
    const onUpdate = vi.fn();
    const stop = startProgressPoller(poll, onUpdate, 50);
    await vi.advanceTimersByTimeAsync(50);
    stop();
    release(99);
    await vi.advanceTimersByTimeAsync(500);
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it("shows no figure when a poll fails, and keeps polling", async () => {
    const poll = vi
      .fn<() => Promise<number | null>>()
      .mockRejectedValueOnce(new Error("ipc"))
      .mockResolvedValueOnce(30);
    const seen: (number | null)[] = [];
    const stop = startProgressPoller(poll, (p) => seen.push(p), 10);
    await vi.advanceTimersByTimeAsync(10);
    await vi.advanceTimersByTimeAsync(10);
    expect(seen).toEqual([null, 30]);
    stop();
  });
});

describe("progressLabel", () => {
  it("names the sort only when there is a figure", () => {
    expect(progressLabel(null)).toBe("Loading...");
    expect(progressLabel(0)).toBe("Sorting... 0%");
    expect(progressLabel(41.97)).toBe("Sorting... 41%");
  });
});
