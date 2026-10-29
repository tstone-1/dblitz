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

import { describe, expect, it } from "vitest";
import { EditorState, Compartment, EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { currentCompletions, startCompletion } from "@codemirror/autocomplete";
import { forEachDiagnostic, type Diagnostic } from "@codemirror/lint";
import type { SqlCheck } from "#lib/ipc.ts";
import { buildEditorExtensions } from "./sqlEditorExtensions";
import type { CheckReport, RunnableRange } from "./sqlLint";

const schema = {
  columns: { users: ["id", "name"], customer: ["id", "email"] },
  types: {},
  engine: "sqlite" as const,
};

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Mount an editor with the production extension list and return a probe.
 *  `check` stands in for the backend's validate_sql. */
function mountEditor(doc = "SELECT 1", check: (sql: string) => Promise<SqlCheck | null> = async () => null) {
  const executed: number[] = [];
  const checked: string[] = [];
  const reports: CheckReport[] = [];
  const runnables: RunnableRange[] = [];
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    state: EditorState.create({
      doc,
      extensions: buildEditorExtensions({
        dark: false,
        placeholder: "Enter a SELECT query (read-only)...",
        themeCompartment: new Compartment(),
        getSchema: () => schema,
        check: (sql) => {
          checked.push(sql);
          return check(sql);
        },
        getGeneration: () => 1,
        onCheck: (r) => reports.push(r),
        onexecute: () => executed.push(1),
        onDocChanged: () => {},
        onRunnableChanged: (r) => runnables.push(r),
        lintDelay: 10,
      }),
    }),
    parent,
  });
  return {
    view,
    executed,
    checked,
    reports,
    runnables,
    highlighted: () => [...parent.querySelectorAll(".cm-activeStatement")].map((e) => e.textContent).join(""),
    diagnostics: () => {
      const out: Diagnostic[] = [];
      forEachDiagnostic(view.state, (d) => out.push(d));
      return out;
    },
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

describe("editor comforts", () => {
  it("closes a bracket when one is typed", () => {
    // Through the registered input handlers, which is where closeBrackets()
    // hooks in. The keymap for it was wired before the extension ever was, so
    // typing "(" used to insert one character.
    const ed = mountEditor("SELECT count");
    try {
      const pos = ed.text().length;
      ed.view.dispatch({ selection: { anchor: pos } });
      const handled = ed.view.state
        .facet(EditorView.inputHandler)
        .some((h) => h(ed.view, pos, pos, "(", () => ed.view.state.update({ changes: { from: pos, insert: "(" } })));
      expect(handled).toBe(true);
      expect(ed.text()).toBe("SELECT count()");
    } finally {
      ed.destroy();
    }
  });

  it("accepts an open completion with Tab, and leaves Tab alone otherwise", async () => {
    const ed = mountEditor("SELECT * FROM cust");
    try {
      ed.view.dispatch({ selection: { anchor: ed.text().length } });
      ed.press({ key: "Tab", code: "Tab", keyCode: 9 });
      expect(ed.text()).toBe("SELECT * FROM cust"); // control: nothing open
      startCompletion(ed.view);
      for (let i = 0; i < 50 && currentCompletions(ed.view.state).length === 0; i++) await wait(10);
      expect(currentCompletions(ed.view.state)[0]?.label).toBe("customer");
      // The popup counts as open only once CodeMirror has drawn it.
      await wait(100);
      ed.press({ key: "Tab", code: "Tab", keyCode: 9 });
      expect(ed.text()).toBe("SELECT * FROM customer");
    } finally {
      ed.destroy();
    }
  });
});

describe("live check", () => {
  it("underlines the problem the check reports and fixes it from the suggestion", async () => {
    const ed = mountEditor("SELECT * FROM custmer", async () => ({
      error: "no such table: custmer",
      offset: 14,
    }));
    try {
      ed.view.dispatch({ changes: { from: 0, insert: " " } }); // an edit schedules a check
      for (let i = 0; i < 50 && ed.diagnostics().length === 0; i++) await wait(10);
      const [d] = ed.diagnostics();
      expect(ed.view.state.sliceDoc(d.from, d.to)).toBe("custmer");
      expect(d.message).toBe("no such table: custmer");
      expect(ed.reports.at(-1)).toEqual({
        sql: "SELECT * FROM custmer",
        generation: 1,
        check: { error: "no such table: custmer", offset: 14 },
      });
      d.actions![0].apply(ed.view, d.from, d.to);
      expect(ed.text()).toBe(" SELECT * FROM customer");
    } finally {
      ed.destroy();
    }
  });

  it("checks only the selection when there is one, and reports it", async () => {
    // One statement, so the selection is the only thing that can narrow what
    // gets checked.
    const ed = mountEditor("SELECT name FROM users");
    try {
      // Let the check scheduled at mount run first, so what follows can only
      // pass through the selection-change re-check.
      for (let i = 0; i < 50 && ed.checked.length === 0; i++) await wait(10);
      expect(ed.checked).toEqual(["SELECT name FROM users"]);
      ed.view.dispatch({ selection: EditorSelection.single(7, 11) });
      expect(ed.runnables.at(-1)).toMatchObject({ text: "name", kind: "selection" });
      for (let i = 0; i < 50 && !ed.checked.includes("name"); i++) await wait(10);
      expect(ed.checked).toContain("name");
      // Collapsing the selection makes the whole text the runnable one again.
      ed.view.dispatch({ selection: EditorSelection.cursor(3) });
      expect(ed.runnables.at(-1)).toMatchObject({ text: "SELECT name FROM users", kind: "all" });
    } finally {
      ed.destroy();
    }
  });
});

describe("statement at the cursor", () => {
  it("runs, checks and highlights the statement the cursor is in", async () => {
    const ed = mountEditor("SELECT 1;\nSELECT name FROM users;\n");
    try {
      ed.view.dispatch({ selection: EditorSelection.cursor(15) });
      expect(ed.runnables.at(-1)).toMatchObject({ text: "SELECT name FROM users", kind: "statement" });
      expect(ed.highlighted()).toBe("SELECT name FROM users");
      for (let i = 0; i < 50 && !ed.checked.includes("SELECT name FROM users"); i++) await wait(10);
      expect(ed.checked).toContain("SELECT name FROM users");
      expect(ed.checked).not.toContain("SELECT 1;\nSELECT name FROM users;\n");
    } finally {
      ed.destroy();
    }
  });

  it("highlights nothing and runs all of it when there is one statement", () => {
    // Control for the test above: the tint must mean "only this part runs".
    const ed = mountEditor("SELECT 1;");
    try {
      ed.view.dispatch({ selection: EditorSelection.cursor(3) });
      expect(ed.runnables.at(-1)).toMatchObject({ text: "SELECT 1;", kind: "all" });
      expect(ed.highlighted()).toBe("");
    } finally {
      ed.destroy();
    }
  });
});
