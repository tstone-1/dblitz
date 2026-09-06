// @vitest-environment jsdom
//
// The only test in this repo that needs a DOM: it mounts a real EditorView with
// the same extension list `SqlEditor.svelte` mounts and presses the key. A
// source-level assertion ("`Prec.highest` appears in the file") would pass for a
// `Prec.highest` applied to the wrong keymap, and reading the order of an array
// proves nothing about which handler CodeMirror runs first.
//
// jsdom is a per-file environment (see the pragma above) rather than the global
// vitest environment: every other test here is pure logic and runs faster and
// with fewer surprises under `node`.

import { beforeAll, describe, expect, it } from "vitest";
import { EditorState, Compartment } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { buildEditorExtensions, buildColumnCompletions } from "./sqlEditorExtensions";

/** Mount an editor with the production extension list and return a probe. */
function mountEditor(doc = "SELECT 1") {
  const executed: number[] = [];
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    state: EditorState.create({
      doc,
      extensions: buildEditorExtensions({
        dark: false,
        placeholder: "Enter a SELECT query (read-only)...",
        themeCompartment: new Compartment(),
        getCompletions: () => [],
        onexecute: () => executed.push(1),
        onDocChanged: () => {},
      }),
    }),
    parent,
  });
  return {
    view,
    executed,
    text: () => view.state.doc.toString(),
    lineCount: () => view.state.doc.lines,
    press: (init: KeyboardEventInit) => {
      view.contentDOM.dispatchEvent(
        new window.KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          ...init,
        }),
      );
    },
    destroy: () => {
      view.destroy();
      parent.remove();
    },
  };
}

describe("jsdom environment", () => {
  // Assert the environment fact before blaming the application: under `node`
  // this whole file would fail with "document is not defined", and a
  // KeyboardEvent that jsdom did not construct would silently never reach
  // CodeMirror's handler -- which looks exactly like a broken keymap.
  it("provides a document and constructible KeyboardEvents", () => {
    expect(typeof document).toBe("object");
    const e = new window.KeyboardEvent("keydown", { key: "Enter", ctrlKey: true });
    expect(e.key).toBe("Enter");
    expect(e.ctrlKey).toBe(true);
  });
});

describe("buildEditorExtensions", () => {
  it("runs the execute binding on Ctrl+Enter without inserting a line", () => {
    // The Windows/Linux defect: `defaultKeymap` binds Mod-Enter (= Ctrl-Enter
    // there) to insertBlankLine, and the first handler that returns true wins.
    const ed = mountEditor("SELECT 1");
    try {
      ed.press({ key: "Enter", code: "Enter", keyCode: 13, ctrlKey: true });
      expect(ed.executed).toEqual([1]);
      expect(ed.lineCount()).toBe(1);
      expect(ed.text()).toBe("SELECT 1");
    } finally {
      ed.destroy();
    }
  });

  it("runs the execute binding on Meta+Enter without inserting a line", () => {
    // The macOS defect, same root cause: `Mod` is Meta there.
    const ed = mountEditor("SELECT 1");
    try {
      ed.press({ key: "Enter", code: "Enter", keyCode: 13, metaKey: true });
      expect(ed.executed).toEqual([1]);
      expect(ed.lineCount()).toBe(1);
      expect(ed.text()).toBe("SELECT 1");
    } finally {
      ed.destroy();
    }
  });

  it("leaves a plain Enter to the default keymap", () => {
    // Negative control. Without it, an extension list that swallowed every
    // Enter would satisfy both assertions above.
    const ed = mountEditor("SELECT 1");
    try {
      ed.press({ key: "Enter", code: "Enter", keyCode: 13 });
      expect(ed.executed).toEqual([]);
      expect(ed.lineCount()).toBe(2);
    } finally {
      ed.destroy();
    }
  });
});

describe("buildColumnCompletions", () => {
  it("dedupes column names across tables and keeps the first table as detail", () => {
    const completions = buildColumnCompletions({
      users: ["id", "name"],
      orders: ["id", "total"],
    });
    expect(completions.map((c) => c.label)).toEqual(["id", "name", "total"]);
    expect(completions[0].detail).toBe("users");
  });

  it("returns an empty list for an empty schema", () => {
    expect(buildColumnCompletions({})).toEqual([]);
  });
});
