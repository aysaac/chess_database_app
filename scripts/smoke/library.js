// Smoke-test driver for the library-first shell (DB-07): the main window is
// now the database manager, not the editor; opening a database spawns a
// separate "editor-…" OS window for it. Also covers the preferences every
// window shares:
// - favourites, the preview layout and its size surviving a reload of the
//   library window (it loads the saved prefs before it shows);
// - the Settings dialog's theme applying to the library window itself
//   (Light over the private display's dark GTK theme, then Dark);
// - changes reaching the other open window both ways (the theme, a star,
//   the preview layout), a star made in each window keeping the other's
//   (no stale copy saved over a newer list), and the board orientation
//   staying with its own window (a per-window pref, prefs.svelte.ts);
// - a change made just before a window closes being saved (windowClose.ts):
//   the editor sets a pref and closes at once, and the library finds it
//   saved and in its copy; then the library does the same and a second
//   editor window checks.
//
// CHESSDB_SMOKE re-runs this same script in EVERY window that loads a page,
// including the editor window this driver causes to open (see AGENTS.md).
// Each run tells which part to play from what's on screen (sessionStorage
// doesn't cross windows, so it can't carry a "phase" between them). The two
// windows take turns through a `smokeStep` pref read straight from the Rust
// side (api.uiPrefs), so the turns don't depend on the sync being tested.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, invoke, prefs, api, library, appearance, win } = window.__chessdb;
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
    expect(await cond(), typeof what === "function" ? what() : what);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  const DB_PATH = OUT + "/smoke-lib.chessdb";
  const DB2_PATH = OUT + "/smoke-lib-2.chessdb";
  const saved = async () => (await api.uiPrefs()).values;
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  // Whose turn it is (see the header): set and awaited through the Rust side.
  const step = (name) => api.setUiPref("smokeStep", name);
  const turn = (name) => until(async () => (await saved()).smokeStep === name, "the other window to reach " + name, 600);

  // The editor window this driver opens has no query string or label this
  // script can read ahead of time from a fresh load, but it DOES have the
  // same CHESSDB_SMOKE script re-run inside it: tell the two apart by what's
  // on screen (the library shows ".home", an editor shows ".ribbon").
  const inLibrary = () => !!document.querySelector(".home");
  const inEditor = () => !!document.querySelector(".ribbon");

  // The library header's background: the panel colour of each theme (app.css).
  const LIGHT_PANEL_RGB = "rgb(232, 236, 231)";
  const DARK_PANEL_RGB = "rgb(31, 42, 44)";
  const headerBg = () => getComputedStyle(document.querySelector(".home header")).backgroundColor;
  const settingsDialog = () => document.querySelector("[role='dialog'][aria-label='Settings']");
  const openSettings = async () => {
    document.querySelector(".folders-footer button.cog").click();
    await until(() => !!settingsDialog(), "the settings dialog");
  };
  const pickSetting = (label) => {
    const option = [...settingsDialog().querySelectorAll("label.theme")].find((l) => l.textContent.trim() === label);
    expect(option, "a " + label + " option in Settings");
    option.querySelector("input").click();
  };
  const closeSettings = async () => {
    settingsDialog().querySelector("button.primary").click();
    await until(() => !settingsDialog(), "the settings dialog closed");
  };
  // Closing (DB-07): a change made just before a window closes, while it
  // still waits for its debounce, must be saved.
  const CLOSE_EDITOR = "set just before the editor closed";
  const CLOSE_LIBRARY = "set just before the library closed";
  // Asks this window to close, as its title-bar button does: Tauri then
  // holds the close and sends the window `tauri://close-requested`, since
  // its page listens for it (windowClose.ts), so this sends that.
  // (`win.close()` needs `core:window:allow-close`, which the app doesn't.)
  const requestClose = () =>
    invoke("plugin:event|emit_to", {
      target: { kind: "Window", label: win.label },
      event: "tauri://close-requested",
      payload: null,
    });
  // Waits for `label`'s window to go, then checks that `key` was saved as
  // `value` before it went, and that the change reached this window's copy.
  const checkSavedOnClose = async (label, key, value) => {
    await until(async () => !(await api.listWindows()).includes(label), "the " + label + " window closed", 200);
    // No waiting here: the window saves before it closes.
    const got = (await saved())[key];
    expect(got === value, "the change made just before " + label + " closed saved, got " + JSON.stringify(got));
    await until(() => prefs.raw(key) === value, () => "it reached this window's copy, got " + JSON.stringify(prefs.raw(key)));
  };
  const card = (name) => [...document.querySelectorAll(".card")].find((c) => c.querySelector(".card-name")?.textContent.trim() === name);
  const favouriteRows = () => [...document.querySelectorAll(".folders .mini-row .name")].map((n) => n.textContent.trim());

  try {
    if (inLibrary() && sessionStorage.getItem("library-phase") !== "reloaded") {
      await log("start (library window)");
      await shot("01-library-home");

      // No native dialogs in a driver (scripts/smoke/README.md): create the database and
      // import a few real games directly through the API, the way the
      // New/Open buttons (and a PGN import) would.
      const info = await api.createDb(DB_PATH);
      await api.importPgn(info.handle, ROOT + "/testdata/pgn/reference.pgn");
      await until(async () => (await api.peekDatabase(DB_PATH, 1)).total > 0, "the import finished");
      // A second one, to star from the editor window later.
      await api.createDb(DB2_PATH);
      await db.loadRecent();
      await until(() => !!card("smoke-lib") && !!card("smoke-lib-2"), "cards for both new databases");
      const listed = db.recent.find((e) => e.path.endsWith("/smoke-lib.chessdb"));
      expect(listed?.games > 0 && listed.sizeBytes > 0, "a game count and size for smoke-lib, got " + JSON.stringify(listed));
      expect(card("smoke-lib").querySelector(".card-meta").textContent.includes("games"), "the card shows the game count");

      // Pick its card, confirm the preview.
      card("smoke-lib").click();
      await until(() => !!document.querySelector(".preview .pv-name"), "the preview showing the picked database");

      // The mini board: a real game's moves, stepped with an interactive,
      // view-only chessground board.
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
        () => "dragging the divider left widened the preview by about 80px, got " +
          (preview.getBoundingClientRect().width - widthBefore),
      );
      const previewSize = prefs.raw("libraryPreviewSize");
      expect(typeof previewSize === "number", "the new size saved to prefs");
      await shot("02a-preview-resized");

      // Settings: switching to the bottom (ChessBase-style horizontal) layout.
      await openSettings();
      pickSetting("Bottom");
      await closeSettings();
      await until(() => document.querySelector(".content")?.classList.contains("bottom"), "layout switched to bottom");
      const previewAfter = document.querySelector(".preview").getBoundingClientRect();
      const gridAfter = document.querySelector(".grid-area").getBoundingClientRect();
      expect(Math.abs(previewAfter.left - gridAfter.left) < 2, "the preview now sits under the grid, not beside it");
      expect(previewAfter.top > gridAfter.top, "the preview is below the grid");
      await shot("02b-preview-bottom-layout");

      // Star it: it joins the favourites in the sidebar.
      document.querySelector(".preview button.star").click();
      await until(() => same(favouriteRows(), ["smoke-lib"]), () => "smoke-lib under favourites, got " + favouriteRows());

      // A restart of the library window (a reload) must bring all of that
      // back, once the debounced writes are saved.
      await until(async () => {
        const p = await saved();
        return same(p.libraryFavorites, [listed.path]) && p.libraryPreviewLayout === "bottom" &&
          p.libraryPreviewSize === previewSize;
      }, "the favourite, preview layout and size saved");
      sessionStorage.setItem("library-phase", "reloaded");
      sessionStorage.setItem("library-preview-size", String(previewSize));
      await log("reloading the library window");
      location.reload();
      return;
    }

    if (inLibrary()) {
      await log("library window reloaded");
      const previewSize = Number(sessionStorage.getItem("library-preview-size"));
      sessionStorage.removeItem("library-phase");
      const recent = await api.recentDatabases();
      const lib = recent.find((e) => e.path.endsWith("/smoke-lib.chessdb")).path;
      const lib2 = recent.find((e) => e.path.endsWith("/smoke-lib-2.chessdb")).path;

      // Bug 1: the prefs are loaded before the library shows, so nothing
      // came back as a default.
      expect(same(prefs.raw("libraryFavorites"), [lib]), "the favourites loaded, got " + JSON.stringify(prefs.raw("libraryFavorites")));
      await until(() => same(favouriteRows(), ["smoke-lib"]), () => "smoke-lib still under favourites, got " + favouriteRows());
      await until(() => !!card("smoke-lib")?.querySelector(".fav-badge"), "the star badge on its card");
      expect(document.querySelector(".content")?.classList.contains("bottom"), "the bottom preview layout after the reload");
      const height = document.querySelector(".preview").getBoundingClientRect().height;
      expect(Math.abs(height - previewSize) < 2, "the preview " + previewSize + "px high after the reload, got " + height);
      await shot("03-library-after-reload");

      // Bug 2: the theme applies to the library window itself. The private
      // display's GTK theme is dark, so Light is a visible change.
      expect(matchMedia("(prefers-color-scheme: dark)").matches, "a dark system theme to test against");
      expect(headerBg() === DARK_PANEL_RGB, "the dark system theme at first, got " + headerBg());
      await openSettings();
      pickSetting("Light");
      await until(() => document.documentElement.dataset.theme === "light", "light theme applied to <html>");
      await until(() => headerBg() === LIGHT_PANEL_RGB, () => "the library header itself repainted light, got " + headerBg());
      await shot("04-settings-light");
      pickSetting("Dark");
      await until(() => document.documentElement.dataset.theme === "dark", "dark theme applied to <html>");
      await until(() => headerBg() === DARK_PANEL_RGB, () => "the library header repainted dark, got " + headerBg());
      await shot("04a-settings-dark");
      await closeSettings();
      await until(async () => (await saved()).appTheme === "dark", "appTheme saved");

      // Open smoke-lib: this must spawn a separate OS window, not replace
      // the library's own content.
      card("smoke-lib").click();
      await until(() => document.querySelector(".preview .pv-name")?.textContent === "smoke-lib", "smoke-lib picked");
      const before = await api.listWindows();
      document.querySelector(".preview button.primary").click(); // "Open"
      await until(async () => (await api.listWindows()).length > before.length, "a new OS window opened");
      const labels = await api.listWindows();
      const editorLabel = labels.find((l) => !before.includes(l));
      expect(editorLabel?.startsWith("editor-"), "an editor-* window, got " + labels.join(", "));
      await shot("05-after-open-spawns-window");

      // Bug 3, editor to library: the editor switches to Light, stars
      // smoke-lib-2 and flips its board (see the editor's part below).
      await turn("editor-changed");
      await until(() => document.documentElement.dataset.theme === "light", "the editor's Light theme reached the library");
      await until(() => headerBg() === LIGHT_PANEL_RGB, () => "the library repainted light, got " + headerBg());
      await until(() => same(library.favorites, [lib, lib2]), () => "both favourites in the library's copy, got " + JSON.stringify(library.favorites));
      // Listed in the recent databases' order.
      await until(() => same(favouriteRows().sort(), ["smoke-lib", "smoke-lib-2"]), () => "both under favourites, got " + favouriteRows());
      // The orientation is the editor's own: saved, but not taken here.
      expect((await saved()).orientation === "black", "the editor's flipped board saved");
      await wait(500);
      expect(prefs.raw("orientation") === undefined, "the library's copy without the editor's orientation, got " + prefs.raw("orientation"));
      await shot("06-library-after-editor-changes");

      // Library to editor: unstar smoke-lib here, from a copy that has the
      // editor's star (before the fix it held only its own, and saving it
      // dropped smoke-lib-2), and go back to the side preview.
      document.querySelector(".preview button.star").click();
      await until(() => same(library.favorites, [lib2]), "smoke-lib unstarred");
      await openSettings();
      pickSetting("Side");
      await closeSettings();
      await until(async () => {
        const p = await saved();
        return same(p.libraryFavorites, [lib2]) && p.libraryPreviewLayout === "side";
      }, "the library's changes saved, keeping the editor's star");
      await log("library checks passed; waiting for the editor window's own checks");
      await step("library-changed");

      // The editor sets a pref and closes at once (below).
      await turn("editor-closing");
      await checkSavedOnClose(editorLabel, "smokeCloseEditor", CLOSE_EDITOR);
      await shot("09-library-after-editor-closed");

      // The library does the same; a second editor window checks.
      await step("second-editor");
      await api.openEditorWindow(null);
      await turn("second-editor-ready");
      await step("library-closing");
      await log("closing the library window right after a change");
      prefs.set("smokeCloseLibrary", CLOSE_LIBRARY);
      await requestClose();
    } else if (inEditor() && (await saved()).smokeStep === "second-editor") {
      // The second editor window, opened to see the library close.
      await log("start (second editor window)");
      await step("second-editor-ready");
      await turn("library-closing");
      await checkSavedOnClose("main", "smokeCloseLibrary", CLOSE_LIBRARY);
      await shot("10-editor-after-library-closed");
      await log("done");
    } else if (inEditor()) {
      // This is the editor window the library window just spawned.
      await log("start (editor window)");
      await until(() => !!db.info, "a database open on arrival");
      expect(db.info.path.endsWith("smoke-lib.chessdb") || db.info.path.includes("smoke-lib"),
        "the editor opened the database the library sent it, got " + db.info.path);
      // It started from the saved prefs: the library's Dark and its star.
      expect(document.documentElement.dataset.theme === "dark", "the saved Dark theme in the new editor window");
      const recent = await api.recentDatabases();
      const lib = recent.find((e) => e.path.endsWith("/smoke-lib.chessdb")).path;
      const lib2 = recent.find((e) => e.path.endsWith("/smoke-lib-2.chessdb")).path;
      expect(same(library.favorites, [lib]), "the saved favourites in the editor, got " + JSON.stringify(library.favorites));
      await shot("07-editor-opened-with-target");

      appearance.theme = "light";
      library.toggleFavorite(lib2);
      game.flip();
      await until(async () => {
        const p = await saved();
        return p.appTheme === "light" && same(p.libraryFavorites, [lib, lib2]) && p.orientation === "black";
      }, "the editor's changes saved");
      await step("editor-changed");

      await turn("library-changed");
      await until(() => same(library.favorites, [lib2]), () => "the library's unstar reached the editor, got " + JSON.stringify(library.favorites));
      await until(() => library.previewLayout === "side", "the library's Side layout reached the editor");
      expect(game.orientation === "black" && prefs.raw("orientation") === "black", "the editor's own board still flipped");
      expect(document.documentElement.dataset.theme === "light", "the editor still light");
      await shot("08-editor-after-library-changes");

      // A change, then the window closes before its debounce is over.
      await step("editor-closing");
      await log("closing the editor window right after a change");
      prefs.set("smokeCloseEditor", CLOSE_EDITOR);
      await requestClose();
    } else {
      await log("FAILED: neither the library nor an editor shell is on screen");
    }
  } catch (e) {
    sessionStorage.removeItem("library-phase");
    await log("FAILED: " + (e?.message || e) + "\n" + (e?.stack || ""));
  }
})();
