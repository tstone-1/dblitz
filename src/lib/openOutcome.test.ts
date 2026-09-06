import { describe, expect, it } from "vitest";
import { openSucceeded } from "./openOutcome";

describe("openSucceeded", () => {
  it("is true when the requested file is the one now open and nothing failed", () => {
    // Positive control: a guard that always answered false would satisfy every
    // other case here and would never switch to the Browse tab at all.
    expect(openSucceeded({ requestedPath: "/db.sqlite", dbPath: "/db.sqlite", error: null })).toBe(true);
  });

  it("is false when the open reported an error", () => {
    expect(
      openSucceeded({ requestedPath: "/db.sqlite", dbPath: null, error: "unable to open database file" }),
    ).toBe(false);
  });

  it("is false when a failed reopen left the previous file open", () => {
    // `error` is set, but `dbPath` still names a file -- the one that was
    // already open. Checking the path alone would pass this.
    expect(
      openSucceeded({ requestedPath: "/new.sqlite", dbPath: "/old.sqlite", error: "not a database" }),
    ).toBe(false);
  });

  it("is false for a superseded open that published nothing", () => {
    // A second openDatabase() started while the first was in flight: the first
    // returns silently, with no error at all. Checking `error` alone would
    // pass this and switch tabs for a request that never landed.
    expect(openSucceeded({ requestedPath: "/a.sqlite", dbPath: "/b.sqlite", error: null })).toBe(false);
  });
});
