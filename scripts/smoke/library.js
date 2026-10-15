// Smoke-test driver for the library-first shell (DB-07): the main window is
// now the database manager, not the editor; opening a database spawns a
// separate "editor-…" OS window for it. Also covers the settings dialog's
// theme toggle, which lives on both shells' status areas.
//
// CHESSDB_SMOKE re-runs this same script in EVERY window that loads a page,
// including the editor window this driver causes to open (see AGENTS.md).
// Each run tells which part to play from what's on screen (sessionStorage
// doesn't cross windows, so it can't carry a "phase" between them).
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, invoke, prefs, api } = window.__chessdb;
  game.confirmDiscard = async () => true;
  const log = (m) => invoke("dev_log", { message: String(m) });
  const shot = async (name) => {
    await wait(800);
    await log("shot:" + name);
    await wait(1500);
  };
  const expect = (ok, what) => {
    if (!ok) throw new Error("expected " + what);
  };
  const until = async (cond, what, tries = 80) => {
    for (let i = 0; i < tries && !(await cond()); i++) await wait(50);
    expect(await cond(), what);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  const DB_PATH = OUT + "/smoke-lib.chessdb";

  // The editor window this driver opens has no query string or label this
  // script can read ahead of time from a fresh load, but it DOES have the
  // same CHESSDB_SMOKE script re-run inside it: tell the two apart by what's
  // on screen (the library shows ".home", an editor shows ".ribbon").
  const inLibrary = () => !!document.querySelector(".home");
  const inEditor = () => !!document.querySelector(".ribbon");

  try {
    if (inLibrary()) {
      await log("start (library window)");
      await shot("01-library-home");

      // No native dialogs in a driver (scripts/smoke/README.md): create the database and
      // import a few real games directly through the API, the way the
      // New/Open buttons (and a PGN import) would.
      const info = await api.createDb(DB_PATH);
      await api.importPgn(info.handle, ROOT + "/testdata/pgn/reference.pgn");
      await until(async () => (await api.peekDatabase(DB_PATH, 1)).total > 0, "the import finished");
      await db.loadRecent();
      await until(() => db.recent.some((e) => e.path.endsWith("smoke-lib.chessdb")), "the new database listed");

      // Pick its card, confirm the preview, then open it: this must spawn a
      // separate OS window, not replace the library's own content.
      const card = () => [...document.querySelectorAll(".card")].find((c) => c.textContent.includes("smoke-lib"));
      await until(() => !!card(), "a card for smoke-lib");
      card().click();
      await until(() => !!document.querySelector(".preview .pv-name"), "the preview showing the picked database");

      // The mini board (this message's feature): a real game's moves,
      // stepped with an interactive, view-only chessground board.
      await until(() => !!document.querySelector(".mini-wrap .cg-wrap"), "the mini board mounted");
      await until(() => document.querySelectorAll(".mini-wrap piece").length > 0, "pieces drawn on the mini board");
      const forward = () => document.querySelector('.nav button[aria-label="Next move"]');
      expect(forward() && !forward().disabled, "a usable Next-move button");
      forward().click();
      await until(
        () => !document.querySelector(".mini-wrap .caption")?.textContent.startsWith("Start"),
        "the mini board stepped forward",
      );
      await shot("02-library-preview");

      // The preview panel's border drags to resize it (synthetic pointer
      // events at the divider's real position, like dock.js does).
      const divider = document.querySelector(".divider");
      const preview = document.querySelector(".preview");
      expect(divider, "the preview's resize divider");
      const widthBefore = preview.getBoundingClientRect().width;
      const dRect = divider.getBoundingClientRect();
      const x0 = dRect.left + dRect.width / 2;
      const y0 = dRect.top + dRect.height / 2;
      divider.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, clientX: x0, clientY: y0 }));
      window.dispatchEvent(
        new PointerEvent("pointermove", { bubbles: true, buttons: 1, clientX: x0 - 80, clientY: y0 }),
      );
      await wait(50);
      window.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, clientX: x0 - 80, clientY: y0 }));
      await until(
        () => Math.round(preview.getBoundingClientRect().width - widthBefore) >= 70,
        "dragging the divider left widened the preview by about 80px, got " +
          (preview.getBoundingClientRect().width - widthBefore),
      );
      await until(() => prefs.raw("libraryPreviewSize") != null, "the new size saved to prefs");
      await shot("02a-preview-resized");

      // Settings: switching to the bottom (ChessBase-style horizontal) layout.
      const cog2 = document.querySelector(".folders-footer button.cog");
      cog2.click();
      const dialog2 = () => document.querySelector("[role='dialog'][aria-label='Settings']");
      await until(() => !!dialog2(), "the settings dialog");
      const bottomRadio = [...dialog2().querySelectorAll("label.theme")].find(
        (l) => l.textContent.trim() === "Bottom",
      );
      expect(bottomRadio, "a Bottom preview-layout option");
      bottomRadio.querySelector("input").click();
      dialog2().querySelector("button.primary").click();
      await until(() => !dialog2(), "the settings dialog closed");
      await until(() => document.querySelector(".content")?.classList.contains("bottom"), "layout switched to bottom");
      const previewAfter = document.querySelector(".preview").getBoundingClientRect();
      const gridAfter = document.querySelector(".grid-area").getBoundingClientRect();
      expect(Math.abs(previewAfter.left - gridAfter.left) < 2, "the preview now sits under the grid, not beside it");
      expect(previewAfter.top > gridAfter.top, "the preview is below the grid");
      await shot("02b-preview-bottom-layout");

      const before = await api.listWindows();
      document.querySelector(".preview button.primary").click(); // "Open"
      await until(async () => (await api.listWindows()).length > before.length, "a new OS window opened");
      const labels = await api.listWindows();
      expect(labels.some((l) => l.startsWith("editor-")), "an editor-* window, got " + labels.join(", "));
      await shot("03-after-open-spawns-window");

      // The settings dialog: basic light/dark/system theme (more options later).
      const cog = document.querySelector(".folders-footer button.cog");
      expect(cog, "the settings cog pinned at the bottom of the folders sidebar");
      cog.click();
      const dialog = () => document.querySelector("[role='dialog'][aria-label='Settings']");
      await until(() => !!dialog(), "the settings dialog");
      const radio = (label) => [...dialog().querySelectorAll("label.theme")].find((l) => l.textContent.trim() === label);
      const DARK_PANEL_RGB = "rgb(31, 42, 44)";
      const panelBg = () => getComputedStyle(document.querySelector("header")).backgroundColor;
      radio("Dark").querySelector("input").click();
      await until(() => document.documentElement.dataset.theme === "dark", "dark theme applied to <html>");
      await until(() => panelBg() === DARK_PANEL_RGB, "the library header itself (not just the dialog) repainted dark");
      await shot("04-settings-dark");
      dialog().querySelector("button.primary").click();
      await until(() => !dialog(), "the settings dialog closed");
      await until(() => prefs.raw("appTheme") === "dark", "appTheme saved to prefs");

      // The editor window spawned above re-runs this same script and logs
      // its own "done" (see the harness note at the top); don't log one
      // here, or the harness stops watching before that happens.
      await log("library checks passed; waiting for the editor window's own checks");
    } else if (inEditor()) {
      // This is the editor window the library window just spawned.
      await log("start (editor window)");
      await until(() => !!db.info, "a database open on arrival");
      expect(db.info.path.endsWith("smoke-lib.chessdb") || db.info.path.includes("smoke-lib"),
        "the editor opened the database the library sent it, got " + db.info.path);
      await shot("05-editor-opened-with-target");
      await log("done");
    } else {
      await log("FAILED: neither the library nor an editor shell is on screen");
    }
  } catch (e) {
    await log("FAILED: " + (e?.message || e) + "\n" + (e?.stack || ""));
  }
})();
