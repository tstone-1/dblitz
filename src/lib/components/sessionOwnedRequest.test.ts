import { describe, expect, it } from "vitest";
import { createSessionOwnedRequest } from "./sessionOwnedRequest";

/** A promise whose settlement this test controls, so ordering is explicit. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function harness(startGeneration = 1) {
  let generation = startGeneration;
  const published: string[][] = [];
  const errors: string[] = [];
  const request = createSessionOwnedRequest<string[]>({
    getGeneration: () => generation,
    publish: (value) => published.push(value),
    onError: (message) => errors.push(message),
  });
  return {
    request,
    published,
    errors,
    bumpGeneration: () => {
      generation++;
    },
  };
}

describe("createSessionOwnedRequest", () => {
  it("publishes a request that nothing supersedes", async () => {
    // Positive control. Without this, every assertion below is satisfied by a
    // guard that publishes nothing at all.
    const h = harness();
    await h.request(async () => ["ok"]);
    expect(h.published).toEqual([["ok"]]);
  });

  it("drops a superseded response that resolves last", async () => {
    // The reported defect: click table A, then B; B answers first, A answers
    // second, and A's columns land under B's label.
    const h = harness();
    const a = deferred<string[]>();
    const b = deferred<string[]>();

    const first = h.request(() => a.promise);
    const second = h.request(() => b.promise);

    b.resolve(["b-columns"]);
    await second;
    a.resolve(["a-columns"]);
    await first;

    expect(h.published).toEqual([["b-columns"]]);
  });

  it("drops a response whose database session has been replaced", async () => {
    // Start a read against database A, open database B mid-flight. The reset
    // clears the panel; A's answer must not repopulate it.
    const h = harness();
    const a = deferred<string[]>();
    const pending = h.request(() => a.promise);

    h.bumpGeneration();
    a.resolve(["stale-schema"]);
    await pending;

    expect(h.published).toEqual([]);
  });

  it("drops a superseded request's error instead of surfacing it", async () => {
    const h = harness();
    const a = deferred<string[]>();
    const b = deferred<string[]>();

    const first = h.request(() => a.promise);
    const second = h.request(() => b.promise);

    b.resolve(["b"]);
    await second;
    a.reject(new Error("no such table: gone"));
    await first;

    expect(h.errors).toEqual([]);
    expect(h.published).toEqual([["b"]]);
  });

  it("surfaces the error of a request that still owns publication", async () => {
    // Control for the case above: errors are gated, not swallowed.
    const h = harness();
    await h.request(async () => {
      throw new Error("boom");
    });
    expect(h.errors).toEqual(["Error: boom"]);
  });

  it("keeps two independent guards from superseding each other", async () => {
    // The raw-schema read and the table-columns read are separate concerns; a
    // table click must not cancel a schema load. One shared token would.
    let generation = 1;
    const deps = (sink: string[][]) => ({
      getGeneration: () => generation,
      publish: (value: string[]) => sink.push(value),
      onError: () => {},
    });
    const schemaSink: string[][] = [];
    const columnSink: string[][] = [];
    const requestSchema = createSessionOwnedRequest<string[]>(deps(schemaSink));
    const requestColumns = createSessionOwnedRequest<string[]>(deps(columnSink));

    const schema = deferred<string[]>();
    const columns = deferred<string[]>();
    const schemaCall = requestSchema(() => schema.promise);
    const columnCall = requestColumns(() => columns.promise);

    columns.resolve(["cols"]);
    await columnCall;
    schema.resolve(["schema"]);
    await schemaCall;

    expect(schemaSink).toEqual([["schema"]]);
    expect(columnSink).toEqual([["cols"]]);
  });
});
