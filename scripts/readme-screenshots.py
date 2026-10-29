"""Capture the README images from a running dblitz over CDP (Windows).

1. Build the demo database and copy it to a neutral path, because the toolbar
   shows the full path and it lands in every image:
       uv run scripts/make-demo-db.py C:/demo/roastery.sqlite
2. Start Vite (`npm run dev`) and a debug build with CDP enabled:
       $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
       $env:WEBVIEW2_USER_DATA_FOLDER = "$env:TEMP\\dblitz-shots-udf"
       Start-Process src-tauri\\target\\debug\\dblitz.exe -ArgumentList '"C:\\demo\\roastery.sqlite"'
3. uv run --with playwright --with pillow scripts/readme-screenshots.py [browse|sqlgif|lint ...]

With no step names it runs all three and writes docs/images/.
"""

import base64
import io
import sys
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

OUT = Path(__file__).resolve().parent.parent / "docs" / "images"
WIDTH, HEIGHT = 1200, 760


class App:
    def __init__(self, page):
        self.pg = page
        self.cdp = page.context.new_cdp_session(page)

    def scale(self, factor):
        # Playwright's own screenshot ignores this override; Page.captureScreenshot honours it.
        self.cdp.send("Emulation.setDeviceMetricsOverride",
                      {"width": WIDTH, "height": HEIGHT, "deviceScaleFactor": factor, "mobile": False})
        self.pg.wait_for_timeout(500)

    def png(self, clip=None):
        args = {"format": "png"}
        if clip:
            args["clip"] = {**clip, "scale": 1}
        return base64.b64decode(self.cdp.send("Page.captureScreenshot", args)["data"])

    def tab(self, name):
        self.pg.get_by_role("button", name=name, exact=True).click()
        self.pg.wait_for_timeout(600)

    def click_at(self, js, arg):
        """Click the centre of the element a JS expression returns. Coordinates,
        because Playwright reports some of the sidebar buttons as not visible."""
        r = self.pg.evaluate(
            "([f, a]) => { const e = (new Function('a', f))(a); if (!e) return null;"
            " const b = e.getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; }",
            [js, arg])
        assert r, (js, arg)
        self.pg.mouse.click(*r)

    def paste(self, text):
        """Replace the editor text with a synthetic paste: CodeMirror reads clipboardData."""
        self.pg.evaluate(
            """t => { const c = document.querySelector('.tab-panel:not(.hidden) .cm-content');
                 c.focus(); document.execCommand('selectAll');
                 const dt = new DataTransfer(); dt.setData('text/plain', t);
                 c.dispatchEvent(new ClipboardEvent('paste', {clipboardData: dt, bubbles: true, cancelable: true})); }""",
            text)

    def editor_text(self):
        return self.pg.evaluate("document.querySelector('.tab-panel:not(.hidden) .cm-content').innerText")


SIDEBAR_TABLE = ("return [...document.querySelectorAll('button.table-btn')]"
                 ".find(x => x.textContent.trim().startsWith(a));")
HEADER = ("return [...document.querySelectorAll('.tab-panel:not(.hidden) .header-row [role=columnheader]')]"
          ".find(x => x.querySelector('.cell-text')?.textContent.trim() === a);")
FILTER = ("const h = [...document.querySelectorAll('.tab-panel:not(.hidden) .header-row [role=columnheader]')]"
          ".find(x => x.querySelector('.cell-text')?.textContent.trim() === a);"
          "const row = document.querySelector('.tab-panel:not(.hidden) .filter-row').getBoundingClientRect();"
          "return document.elementFromPoint(h.getBoundingClientRect().x + 25, row.y + row.height / 2);")
CARET_AFTER = ("const c = document.querySelector('.tab-panel:not(.hidden) .cm-content');"
               "const w = document.createTreeWalker(c, NodeFilter.SHOW_TEXT);"
               "for (let t; (t = w.nextNode());) { const i = t.data.indexOf(a);"
               " if (i >= 0) { const r = document.createRange(); r.setStart(t, i + a.length);"
               " r.collapse(true); return { getBoundingClientRect: () => r.getBoundingClientRect() }; } }"
               "return null;")

SKELETON = ("SELECT \nFROM customers c\nJOIN orders o ON o.customer_id = c.id\n"
            "WHERE o.status <> 'cancelled'\nGROUP BY c.id\nORDER BY revenue_eur DESC;")
# (typed text, key pressed after it). Tab accepts the open suggestion.
TYPED = [("c.na", "Tab"), (", c.ci", "Tab"), (", count(*) AS orders, round(sum(o.tot", "Tab"),
         ("), 2) AS revenue_eur", None)]
FINAL = SKELETON.replace("SELECT ", "SELECT c.name, c.city, count(*) AS orders, "
                                    "round(sum(o.total_eur), 2) AS revenue_eur", 1)


def browse(app):
    app.scale(2)
    app.tab("Browse Data")
    app.click_at(SIDEBAR_TABLE, "customers")
    app.pg.wait_for_timeout(1200)
    app.click_at(FILTER, "country")
    app.pg.keyboard.press("Control+a")
    app.pg.keyboard.type("NL", delay=40)
    app.pg.wait_for_timeout(1000)
    # dblitz saves the sort per file, so a click toggles from whatever the last
    # run left behind; click until the header says descending.
    for _ in range(3):
        if app.pg.evaluate("a => (new Function('a', a[0]))(a[1])?.getAttribute('aria-sort')",
                           [HEADER, "joined"]) == "descending":
            break
        app.click_at(HEADER, "joined")
        app.pg.wait_for_timeout(1000)
    else:
        raise AssertionError("joined never sorted descending")
    app.pg.mouse.move(560, 22)  # empty toolbar space: no hover highlight in the grid
    app.pg.wait_for_timeout(300)
    (OUT / "browse.png").write_bytes(app.png())


class Recorder:
    def __init__(self, app, clip):
        self.app, self.clip, self.frames = app, clip, []

    def snap(self, hold=0):
        self.frames.append((time.monotonic(), self.app.png(self.clip), hold))

    def save(self, name):
        from PIL import Image
        imgs, durs = [], []
        for i, (t, png, hold) in enumerate(self.frames):
            nxt = self.frames[i + 1][0] if i + 1 < len(self.frames) else t + 0.1
            im = Image.open(io.BytesIO(png)).convert("RGB")
            ms = max(60, min(int((nxt - t) * 1000), 400)) + hold
            if imgs and im.tobytes() == imgs[-1].tobytes():
                durs[-1] += ms
                continue
            imgs.append(im)
            durs.append(ms)
        pal = [im.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
               for im in imgs]
        pal[0].save(OUT / name, save_all=True, append_images=pal[1:], duration=durs, loop=0,
                    optimize=True)
        print(f"{name}: {len(imgs)} frames, {sum(durs) / 1000:.1f}s")


def sqlgif(app):
    app.scale(1)
    app.tab("Execute SQL")
    rec = Recorder(app, {"x": 0, "y": 0, "width": WIDTH, "height": 560})
    app.paste(SKELETON)
    app.pg.wait_for_timeout(700)
    app.click_at(CARET_AFTER, "SELECT")
    app.pg.keyboard.press("End")
    rec.snap(hold=900)
    for text, key in TYPED:
        for ch in text:
            app.pg.keyboard.type(ch)
            app.pg.wait_for_timeout(70)
            rec.snap()
        app.pg.wait_for_timeout(450)
        rec.snap(hold=500)
        if key:
            app.pg.keyboard.press(key)
            app.pg.wait_for_timeout(150)
            rec.snap(hold=250)
    app.pg.wait_for_timeout(700)
    rec.snap(hold=600)
    app.pg.keyboard.press("Control+Enter")
    for _ in range(6):
        app.pg.wait_for_timeout(150)
        rec.snap()
    app.pg.wait_for_timeout(800)
    rec.snap(hold=3500)
    # A suggestion list that changed would type a different query; refuse to save it.
    assert app.editor_text().replace("\n", "") == FINAL.replace("\n", ""), app.editor_text()
    rec.save("sql-editor.gif")


def lint(app):
    app.scale(2)
    app.tab("Execute SQL")
    app.paste(FINAL.replace("o.total_eur", "o.totl_eur"))
    app.pg.wait_for_timeout(1500)
    r = app.pg.evaluate("""() => { const b = document.querySelector('.tab-panel:not(.hidden) .cm-lintRange-error')
                                     .getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; }""")
    app.pg.mouse.move(*r)
    app.pg.wait_for_timeout(1200)
    tip = app.pg.evaluate("document.querySelector('.cm-tooltip')?.innerText") or ""
    assert "Replace with total_eur" in tip, tip
    # Toolbar and editor only: a result grid below would be left over from an earlier run.
    (OUT / "sql-check.png").write_bytes(app.png({"x": 0, "y": 0, "width": WIDTH, "height": 200}))


STEPS = {"browse": browse, "sqlgif": sqlgif, "lint": lint}

if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.connect_over_cdp("http://127.0.0.1:9222")
        # A debug build also exposes its devtools window as a page target.
        page = next(x for x in browser.contexts[0].pages if x.url.startswith("http://localhost"))
        for name in sys.argv[1:] or list(STEPS):
            STEPS[name](App(page))
            print(f"[OK] {name}")
