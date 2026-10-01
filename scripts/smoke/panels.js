// Smoke-test driver for the panel tabs (UI-19): Notation and Opening tree as
// tabs beside the board, the eval graph (ANL-04) inside the Notation tab, the
// tree querying only while its tab shows, the move popovers opening while
// the tree shows, the keyboard (the WAI-ARIA tabs pattern), and the active
// tab kept in the `layout` pref and restored at start (QOL-01), also from the
// older `treeVisible` pref. To test the restore it reloads the page; the
// harness runs the driver again after every load, so the phase it's in is
// kept in sessionStorage.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, tree, panels, prefs, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden monitor: never ask about unsaved changes.
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
  const until = async (cond, what, ms = 5000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const untilAsync = async (cond, what, ms = 5000) => {
    const end = performance.now() + ms;
    while (performance.now() < end) {
      if (await cond()) return;
      await wait(100);
    }
    expect(await cond(), what);
  };
  const $ = (s) => document.querySelector(s);
  const tab = (id) => $(`[role='tab'][data-panel='${id}']`);
  const panel = (id) => $(`#panel-${id}`);
  const shown = (el) => !!el && el.getClientRects().length > 0;
  const key = (el, k) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
  const saved = () => api.uiPrefs();
  const savedActive = (p) => p.layout?.zones?.right?.groups?.find((g) => g.panels.includes("tree"))?.active;
  // Which tab shows, as the store, the tab strip and the panels say it.
  const showing = (id) =>
    panels.isShowing(id) &&
    tab(id)?.getAttribute("aria-selected") === "true" &&
    tab(id)?.tabIndex === 0 &&
    shown(panel(id)) &&
    ["notation", "tree"].filter((o) => o !== id).every((o) => !shown(panel(o)) && tab(o).tabIndex === -1);
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  // Every tree query the app makes.
  let queries = 0;
  const openingTree = api.openingTree;
  api.openingTree = (...args) => {
    queries++;
    return openingTree(...args);
  };

  const PHASE = "smoke-panels-phase";
  const phase = sessionStorage.getItem(PHASE) ?? "tabs";
  const reload = async (next) => {
    sessionStorage.setItem(PHASE, next);
    await log(`reloading the page (${next})`);
    location.reload();
  };

  try {
    if (phase === "tabs") {
      await log("start");
      // Fresh settings: Notation shows, the Opening tree tab is next to it.
      const strip = $(".game-column [role='tablist']");
      expect(strip?.getAttribute("aria-label") === "Panels", "a panel tab strip in the game column");
      const tabs = [...strip.querySelectorAll("[role='tab']")];
      const labels = tabs.map((t) => t.querySelector(".label:not([aria-hidden])")?.textContent.trim()).join(" | ");
      expect(labels === "Notation | Opening tree", "Notation and Opening tree tabs, got " + labels);
      for (const t of tabs) {
        const p = document.getElementById(t.getAttribute("aria-controls"));
        expect(p?.getAttribute("role") === "tabpanel" && p.getAttribute("aria-labelledby") === t.id,
          `${t.dataset.panel}: tab and panel point at each other`);
      }
      expect(showing("notation"), "the Notation tab shows by default");
      expect(prefs.raw("layout") === undefined && prefs.raw("treeVisible") === undefined, "no saved layout yet");
      expect(!$(".tree header button[aria-label^='Close']"), "the tree has no close button");

      await db.createAt(OUT + "/panels.chessdb");
      for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
        await db.importFrom(ROOT + "/testdata/pgn/" + f);
      }
      await until(() => db.info?.index.built && db.info.index.upToDate, "the index built after the imports", 20000);

      // The Lichess game has evaluations: the graph is at the bottom of the Notation tab.
      const rows = (await api.listGames(db.info.handle, db.sort, 0, 6)).rows;
      const lichess = rows.findIndex((r) => r.white === "player1");
      expect(lichess >= 0, "the Lichess game in the list");
      await game.open(db.info.handle, rows[lichess].id, lichess);
      for (let i = 0; i < 6; i++) await game.step("forward");
      await until(() => shown($("figure.graph")), "the eval graph");
      const graph = $("figure.graph");
      expect(document.querySelectorAll("figure.graph").length === 1 && panel("notation").contains(graph),
        "the eval graph inside the Notation tab");
      const notationBox = $("[data-notation]").getBoundingClientRect();
      expect(graph.getBoundingClientRect().top >= notationBox.bottom - 1, "the graph under the notation");
      expect(panel("notation").contains($(".game-column header h1")), "the game header inside the Notation tab");
      await wait(300);
      expect(queries === 0 && tree.data === null, "no tree query while the Notation tab shows, got " + queries);
      await shot("p01-notation-tab");

      // Clicking the Opening tree tab shows it; it queries now, and on each move.
      tab("tree").click();
      await until(() => showing("tree"), "the Opening tree tab shows after a click");
      expect(!shown(graph), "the eval graph hidden with the Notation tab");
      await until(() => tree.current && tree.data.fen === game.view.fen, "the tree of the position");
      expect(queries === 1, "one query when the tab showed, got " + queries);
      await game.step("forward");
      await until(() => tree.current && queries === 2, "a query after a move, got " + queries);
      await shot("p02-tree-tab");

      // A move from the tree where the game has another one: the variation
      // popover opens under the tab strip, though the notation is hidden.
      await game.step("start");
      await until(() => tree.current?.total.games === 4, "the tree of the starting position");
      await tree.play("d2d4");
      expect(game.pendingChoice?.dest === "d4", "the variation choice for 1.d4 (the game has 1.e4)");
      await wait(150);
      const pop = $("[role='dialog'][aria-modal='true']")?.getBoundingClientRect();
      const stripBox = strip.getBoundingClientRect();
      expect(pop && pop.width > 0 && pop.top >= stripBox.bottom && pop.top - stripBox.bottom < 12 &&
        pop.left >= stripBox.left, "the popover just under the panel tabs, got " + JSON.stringify(pop));
      expect(document.activeElement?.textContent === "New variation", "New variation focused");
      await shot("p03-variation-from-tree");
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      expect(!game.pendingChoice && game.view.cursorPath.length === 0, "Escape cancels it");

      // The next-move picker opens there too (Kasparov–Topalov splits at 8.Bh6 / 8.g4).
      await game.open(db.info.handle, 0, 0);
      await game.step("start");
      for (let i = 0; i < 14; i++) await game.forward();
      const picker = () => $("[role='listbox'][aria-label='Next move']");
      game.forward();
      await until(() => picker() && document.activeElement === picker(), "the next-move picker, focused");
      const pick = picker().getBoundingClientRect();
      expect(pick.top >= stripBox.bottom && pick.left >= stripBox.left, "the picker under the panel tabs");
      await shot("p04-next-move-from-tree");
      key(picker(), "Escape");
      await until(() => !picker(), "Escape closes the picker");

      // Keyboard: ←/→ and Home/End move between the tabs and show them; the
      // game doesn't move meanwhile. Outside the strip the arrows step again.
      const cursor = game.view.cursor;
      tab("tree").focus();
      expect(document.activeElement === tab("tree"), "the Opening tree tab focused");
      for (const [k, id, what] of [
        ["ArrowLeft", "notation", "← shows Notation"],
        ["ArrowRight", "tree", "→ shows Opening tree"],
        ["Home", "notation", "Home shows Notation"],
        ["End", "tree", "End shows Opening tree"],
        ["ArrowRight", "notation", "→ wraps to Notation"],
      ]) {
        key(document.activeElement, k);
        await until(() => showing(id) && document.activeElement === tab(id), what);
      }
      await wait(200);
      expect(game.view.cursor === cursor, "the arrow keys in the tab strip don't step through the game");
      document.activeElement.blur();
      const ply = game.view.cursorPath.length;
      key(document.body, "ArrowLeft");
      await until(() => game.view.cursorPath.length === ply - 1, "← outside the tab strip steps back");

      // The tree doesn't query while it's hidden, and does as soon as it shows.
      expect(showing("notation"), "Notation showing");
      queries = 0;
      for (let i = 0; i < 3; i++) await game.step("back");
      await wait(400);
      expect(queries === 0, "no tree query while hidden, got " + queries);
      // The ribbon's View ▸ "Opening tree" shows the tab.
      [...document.querySelectorAll(".ribbon > .groups button")].find((b) => b.getAttribute("aria-label") === "Opening tree").click();
      await until(() => showing("tree"), "the ribbon button shows the Opening tree tab");
      await until(() => tree.current && tree.data.fen === game.view.fen && queries === 1,
        "one query for the position on the board, got " + queries);

      // The active tab is saved in the layout pref.
      await untilAsync(async () => savedActive(await saved()) === "tree", "the layout saved with the tree showing");
      const p = await saved();
      expect(p.layout.version === 1 && ["left", "right", "top", "bottom"].every((s) => Array.isArray(p.layout.zones[s]?.groups)),
        "a version 1 layout with four sides, got " + JSON.stringify(p.layout));
      expect(p.layout.zones.bottom.groups[0].panels.join() === "games", "the game list at the bottom");
      expect(!("treeVisible" in p), "no treeVisible pref");
      await reload("restore");
      return;
    }

    if (phase === "restore") {
      // After a restart (here, a reload) the Opening tree tab shows again,
      // and queries for the reopened database.
      await log("after the reload: the saved layout");
      await until(() => showing("tree"), "the Opening tree tab restored");
      await until(() => db.info && tree.current && tree.data.fen === game.view?.fen && tree.data.total.games === 4,
        "the tree of the reopened database", 10000);
      await shot("p05-restored");
      // An older settings file: no layout, the tree visible (QOL-01).
      await api.setUiPref("layout", null);
      await api.setUiPref("treeVisible", true);
      await reload("migrate");
      return;
    }

    if (phase === "migrate") {
      await log("after the reload: treeVisible and no layout");
      await until(() => showing("tree"), "treeVisible shows the Opening tree tab");
      tab("notation").click();
      await until(() => showing("notation"), "Notation shows after a click");
      await untilAsync(async () => {
        const p = await saved();
        return savedActive(p) === "notation" && !("treeVisible" in p);
      }, "the layout saved with Notation showing, and treeVisible gone");
      // A layout that can't be read gives the default.
      await api.setUiPref("layout", { version: 1, zones: { right: "nope" } });
      await reload("unreadable");
      return;
    }

    if (phase === "unreadable") {
      await log("after the reload: an unreadable layout");
      await until(() => prefs.raw("layout") !== undefined, "the prefs loaded");
      await wait(300);
      expect(showing("notation"), "the default layout, with Notation showing");
      expect(tab("tree") && tab("notation"), "both tabs there");
      sessionStorage.removeItem(PHASE);
      await log("done");
      return;
    }

    throw new Error("unknown phase " + phase);
  } catch (e) {
    sessionStorage.removeItem(PHASE);
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
