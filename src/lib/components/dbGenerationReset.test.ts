import { describe, expect, it } from "vitest";
import { createDbGenerationReset } from "./dbGenerationReset.svelte";

describe("createDbGenerationReset", () => {
  // The factory imports nothing from the store, so these tests need no store,
  // no mocked invoke and no Svelte runtime -- just a plain generation counter.
  function harness(startGen = 0) {
    let generation = startGen;
    let resets = 0;
    const check = createDbGenerationReset({
      getGeneration: () => generation,
      onReset: () => { resets++; },
    });
    return {
      check,
      open: () => { generation++; },
      setGeneration: (g: number) => { generation = g; },
      get resets() { return resets; },
    };
  }

  it("resets exactly once per generation bump", () => {
    const h = harness();

    h.open();
    h.check();
    expect(h.resets).toBe(1);

    h.open();
    h.check();
    h.open();
    h.check();
    expect(h.resets).toBe(3);
  });

  it("does not reset on repeated checks with an unchanged generation", () => {
    // Consumers call check() from a `$effect`, which re-runs for reasons that
    // have nothing to do with a database switch. Only the transition counts.
    const h = harness();

    h.open();
    h.check();
    h.check();
    h.check();

    expect(h.resets).toBe(1);
  });

  it("does not reset on the first check while no database has been opened", () => {
    // Seeded with 0 to match appState.dbOpenGeneration's initial value: a
    // freshly mounted component with no database open has nothing to clear.
    const h = harness();

    h.check();
    h.check();

    expect(h.resets).toBe(0);
  });

  it("resets on the first check when mounted into an already-open database", () => {
    // Pinning the pre-existing behaviour of the three hand-rolled copies: a
    // component that mounts late (a lazily-rendered tab) cannot tell that it
    // never showed the previous session, so it resets its empty state once.
    const h = harness(4);

    h.check();

    expect(h.resets).toBe(1);

    h.check();
    expect(h.resets).toBe(1);
  });

  it("reports true only on the check that reset", () => {
    const h = harness();

    expect(h.check()).toBe(false);

    h.open();
    expect(h.check()).toBe(true);
    expect(h.check()).toBe(false);
  });

  it("keeps each consumer's bookkeeping independent", () => {
    // Three components hold their own instance; one having already observed a
    // generation must not stop another from resetting for the same open.
    let generation = 0;
    const resets = [0, 0];
    const checks = resets.map((_, i) =>
      createDbGenerationReset({
        getGeneration: () => generation,
        onReset: () => { resets[i]++; },
      }),
    );

    generation = 1;
    checks[0]();
    expect(resets).toEqual([1, 0]);

    checks[1]();
    expect(resets).toEqual([1, 1]);

    generation = 2;
    checks[1]();
    expect(resets).toEqual([1, 2]);
    checks[0]();
    expect(resets).toEqual([2, 2]);
  });
});
