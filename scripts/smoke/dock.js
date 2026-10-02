// Smoke-test driver for docking (UI-04): every panel tab dragged to each
// side of the window and into another group with synthetic pointer events
// at the elements' real positions (the same hit testing and threshold as a
// mouse), next to the nearest group, reordering a strip, Escape and a
// release elsewhere cancelling; the tab menu (right-click, Shift+F10, the
// context-menu key); empty groups and sides going; the dividers with the
// keys, the pointer and a double-click, stopping at the minimums; View ▸
// Reset layout; and the `layout` pref written and read back after a reload,
// also from the older prefs (a version 1 layout, `listShare`) and from
// unreadable or incomplete ones. The harness runs the driver again after
// every page load, so the phase it's in is kept in sessionStorage.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, panels, dock, prefs, api, invoke } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden display: never ask about unsaved changes.
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
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const rect = (sel) => {
    const el = typeof sel === "string" ? $(sel) : sel;
    if (!el) throw new Error("missing " + sel);
    return el.getBoundingClientRect();
  };
  const tab = (id) => $(`[role='tab'][data-panel='${id}']`);
  const center = (el) => {
    const r = el.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  };
  const pointer = (type, x, y, mods = {}) => {
    const el = document.elementFromPoint(x, y) ?? document.body;
    el.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        composed: true,
        clientX: x,
        clientY: y,
        pointerId: 1,
        pointerType: "mouse",
        isPrimary: true,
        button: type === "pointermove" ? -1 : 0,
        buttons: type === "pointerup" ? 0 : 1,
        ...mods,
      }),
    );
  };
  const key = (el, k, mods = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...mods }));
  const hint = () => $(".dock-ghost span")?.textContent.trim() ?? "";
  const rb = (label) =>
    [...document.querySelectorAll(".ribbon > .groups button")].find((b) => b.getAttribute("aria-label") === label);
  const overlap = (a, b) =>
    Math.min(a.right, b.right) - Math.max(a.left, b.left) > 0.5 &&
    Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0.5;
  const inside = (a, b) =>
    a.left >= b.left - 0.5 && a.right <= b.right + 0.5 && a.top >= b.top - 0.5 && a.bottom <= b.bottom + 0.5;
  const SIDES = ["left", "right", "top", "bottom"];
  /** The layout as "left: -; right: notation+tree+reference | analysis; …". */
  const where = () =>
    SIDES.map((s) => `${s}: ${panels.layout.zones[s].groups.map((g) => g.panels.join("+")).join(" | ") || "-"}`).join("; ");
  const DEFAULT = "left: -; right: notation+tree+reference | analysis; top: -; bottom: games";
  const boardSize = () => rect(".board-square").width;
  /** The board square, in the middle, clear of every side; nothing scrolls sideways. */
  const checkBoard = (what) => {
    const b = rect(".board-square");
    const m = rect(".dock-middle");
    expect(Math.abs(b.width - b.height) < 0.5 && inside(b, m), `${what}: the board square and inside the middle`);
    for (const s of $$(".dock > .side")) {
      expect(!overlap(b, s.getBoundingClientRect()), `${what}: the board clear of the ${s.dataset.side} side`);
    }
    expect(document.documentElement.scrollWidth <= window.innerWidth + 0.5, `${what}: no sideways scroll`);
  };
  const sizeOf = (side) => {
    const r = rect("#dock-" + side);
    return side === "left" || side === "right" ? r.width : r.height;
  };
  const appBox = () => rect(".app");

  // ---- dragging --------------------------------------------------------

  /** Presses on a tab and drags it past the threshold (not released). */
  const startDrag = async (id) => {
    const a = center(tab(id));
    pointer("pointerdown", a.x, a.y);
    pointer("pointermove", a.x + 2, a.y + 1);
    await wait(30);
    expect(!dock.panel && !$("[data-dock-edge]"), "no drag before the pointer moves 4 px");
    pointer("pointermove", a.x + 6, a.y + 5);
    await until(() => dock.panel === id && $("[data-dock-edge]"), "the edge bands once dragging " + id);
    return a;
  };
  const moveTo = async (p) => {
    pointer("pointermove", p.x, p.y);
    await wait(80);
    return hint();
  };
  const release = async (p) => {
    pointer("pointerup", p.x, p.y);
    await wait(300);
  };
  /**
   * A point on a side's edge band: its middle, or near its start or end
   * (on the left and right, below the tab strips at the top, which are
   * drop targets of their own).
   */
  const onBand = (side, at = "middle") => {
    const r = rect(`[data-dock-edge='${side}']`);
    const stacked = side === "left" || side === "right";
    const along = (lo, hi) => (at === "start" ? lo + (stacked ? 40 : 12) : at === "end" ? hi - 12 : (lo + hi) / 2);
    return stacked
      ? { x: r.left + r.width / 2, y: along(r.top, r.bottom) }
      : { x: along(r.left, r.right), y: r.top + r.height / 2 };
  };
  /** A point on a tab strip: before a tab (its left quarter), or past the last one. */
  const onStrip = (beforeTab) => {
    const r = rect(tab(beforeTab));
    return { x: r.left + r.width * 0.25, y: r.top + r.height / 2 };
  };
  const pastStrip = (lastTab) => {
    const r = rect(tab(lastTab));
    return { x: r.right + 8, y: r.top + r.height / 2 };
  };
  /** Drags `id` to the point `to()` gives once the drag has started, and drops it there. */
  const dragTo = async (id, to) => {
    await startDrag(id);
    const p = to();
    const label = await moveTo(p);
    await release(p);
    expect(!dock.panel && !$("[data-dock-edge]") && !$(".dock-ghost"), "the drag over");
    return label;
  };

  const PHASE = "smoke-dock-phase";
  const phase = sessionStorage.getItem(PHASE) ?? "drag";
  const reload = async (next) => {
    sessionStorage.setItem(PHASE, next);
    await log(`reloading the page (${next})`);
    location.reload();
  };

  try {
    if (phase === "drag") {
      await log("start");
      // ---- The default layout.
      expect(where() === DEFAULT, "the default layout, got " + where());
      for (const id of ["notation", "tree", "reference", "analysis", "games"]) {
        const t = tab(id);
        expect(t && t.closest("[role='tablist']")?.dataset.dockStrip, `${id}: a tab in a group's tab strip`);
      }
      expect($("#dock-right") && $("#dock-bottom") && !$("#dock-left") && !$("#dock-top"), "the right and bottom sides only");
      for (const [side, orientation] of [["right", "vertical"], ["bottom", "horizontal"]]) {
        const d = $(`[role='separator'][aria-controls='dock-${side}']`);
        const [now, lo, hi] = ["valuenow", "valuemin", "valuemax"].map((a) => Number(d?.getAttribute("aria-" + a)));
        expect(d && d.getAttribute("aria-orientation") === orientation && d.tabIndex === 0 && lo <= now && now <= hi && hi > lo,
          `the ${side} side's divider: a focusable ${orientation} separator with its value, got ${now} in ${lo}–${hi}`);
      }
      // The engine is off: its group takes only its tab and a line, so no
      // divider between it and the notation.
      expect(rect("[data-group-id='" + panels.groupOf("analysis").id + "']").height < 80, "the engine's group small while it's off");
      expect(!$("#dock-right > [role='separator']"), "no divider next to the engine's group while it's off");
      expect($("#panel-analysis").textContent.includes("The engine is off"), "the engine's tab says it's off");

      await db.createAt(OUT + "/dock.chessdb");
      for (const f of ["variations.pgn", "setup.pgn", "lichess.pgn"]) {
        await db.importFrom(ROOT + "/testdata/pgn/" + f);
      }
      await game.open(db.info.handle, 0, 0);
      for (let i = 0; i < 8; i++) await game.forward();
      await wait(300);
      checkBoard("default");
      await shot("d01-default");

      // ---- Each panel to each side, as a group of its own.
      for (const id of ["notation", "tree", "reference", "analysis", "games"]) {
        for (const side of ["left", "top", "right", "bottom"]) {
          panels.reset();
          await wait(250);
          const before = panels.layout.zones[side].groups.length;
          const label = await dragTo(id, () => onBand(side, before ? "end" : "middle"));
          const expected = {
            left: "Dock on the left",
            top: "Dock at the top",
            right: "Dock on the right, below",
            bottom: "Dock at the bottom, right of",
          }[side];
          expect(label.startsWith(expected), `${id} to the ${side}: the label "${expected}…", got "${label}"`);
          expect(panels.sideOf(id) === side && panels.groupOf(id).panels.join() === id,
            `${id} on the ${side} side, in a group of its own: ${where()}`);
          const groups = panels.layout.zones[side].groups;
          expect(groups.at(-1).panels[0] === id, `${id} after the ${side} side's other groups: ${where()}`);
          expect(panels.isShowing(id) && tab(id).closest("#dock-" + side), `${id}'s tab showing on the ${side} side`);
          await wait(200);
          checkBoard(`${id} on the ${side}`);
          await log(`  ${id} → ${side}: "${label}"; ${where()}`);
        }
      }
      // A side that opens takes its room from the side across the board,
      // so the board keeps its size; its only tab dropped back on it
      // changes nothing.
      panels.reset();
      await wait(250);
      const [board0, right0] = [boardSize(), sizeOf("right")];
      await dragTo("notation", () => onBand("left"));
      await wait(250);
      expect(boardSize() === board0 && Math.abs(sizeOf("left") + sizeOf("right") - right0) < 1.5,
        `the left side's room from the right side: board ${board0} → ${boardSize()}, right ${right0} → ` +
        `${sizeOf("left")} + ${sizeOf("right")}`);
      panels.resizeSide("right", 0.5);
      await wait(250);
      const right1 = sizeOf("right");
      await dragTo("notation", () => onBand("left", "end"));
      await wait(250);
      expect(where() === "left: notation; right: tree+reference | analysis; top: -; bottom: games" &&
        Math.abs(sizeOf("right") - right1) < 1.5, `nothing changes: ${where()}, right ${right1} → ${sizeOf("right")}`);
      // The game list docked on the left: the bottom side goes, and the board grows.
      panels.reset();
      await wait(250);
      const tall = boardSize();
      await dragTo("games", () => onBand("left"));
      await until(() => !$("#dock-bottom"), "the bottom side gone with its last group");
      await wait(300);
      expect(boardSize() > tall, `the board grew with the bottom side gone: ${tall} → ${boardSize()}`);
      const list = rect("#panel-games .row.head");
      expect(list.width <= sizeOf("left") + 0.5 && $$("#panel-games .head-cell").length >= 2,
        "the game list's columns fit the left side");
      await shot("d02-list-on-the-left");

      // ---- While dragging: the bands and the label; Escape cancels.
      panels.reset();
      await wait(250);
      await startDrag("notation");
      expect($$("[data-dock-edge]").length === 4, "a band on each edge");
      expect(hint() === "Move Notation here", "over its own strip: reorder, got " + hint());
      const nowhere = await moveTo(center($(".board-square")));
      expect(nowhere.startsWith("Drop on an edge"), "the label away from a target, got " + nowhere);
      const lp = onBand("left");
      const leftHint = await moveTo(lp);
      expect(leftHint === "Dock on the left" && $("[data-dock-edge='left']").classList.contains("over"),
        "over the left band: " + leftHint);
      await shot("d03-dragging");
      key(document.body, "Escape");
      await until(() => !dock.panel && !$("[data-dock-edge]"), "Escape cancels");
      pointer("pointerup", lp.x, lp.y);
      await wait(200);
      expect(where() === DEFAULT, "nothing moved after Escape: " + where());
      // A release away from any target cancels too (here, on the board).
      await startDrag("tree");
      const board = center($(".board-square"));
      expect((await moveTo(board)).startsWith("Drop on an edge"), "no target over the board");
      await release(board);
      expect(where() === DEFAULT, "nothing moved after a release on the board: " + where());
      // Under the threshold it's a click: the tab shows.
      const t = center(tab("reference"));
      pointer("pointerdown", t.x, t.y);
      pointer("pointermove", t.x + 2, t.y + 2);
      pointer("pointerup", t.x + 2, t.y + 2);
      tab("reference").click();
      await wait(100);
      expect(where() === DEFAULT && panels.isShowing("reference"), "a click shows the tab and moves nothing");
      panels.show("notation");

      // ---- Next to the nearest group: above the notation.
      const above = await dragTo("tree", () => onBand("right", "start"));
      expect(above === "Dock on the right, above Notation", "the label above the notation, got " + above);
      expect(where() === "left: -; right: tree | notation+reference | analysis; top: -; bottom: games",
        "the tree above the notation: " + where());

      // ---- Into another group: the label names it; it joins where the pointer is.
      panels.reset();
      await wait(250);
      const into = await dragTo("tree", () => pastStrip("games"));
      expect(into === "Move into Game list", "the label into the game list's group, got " + into);
      expect(panels.groupOf("tree") === panels.groupOf("games") && panels.groupOf("tree").panels.join() === "games,tree" &&
        panels.isShowing("tree"), "the tree after the game list in its group, showing: " + where());
      const intoNotation = await dragTo("games", () => onStrip("notation"));
      expect(intoNotation === "Move into Notation", "the label into the notation's group, got " + intoNotation);
      expect(where() === "left: -; right: games+notation+reference | analysis; top: -; bottom: tree",
        "the game list first in the notation's group: " + where());
      const withBottom = boardSize();
      await dragTo("tree", () => onStrip("analysis"));
      expect(where() === "left: -; right: games+notation+reference | tree+analysis; top: -; bottom: -",
        "the bottom side gone: " + where());
      await until(() => !$("#dock-bottom"), "no bottom side");
      await wait(300);
      expect(boardSize() > withBottom, `the board grew: ${withBottom} → ${boardSize()}`);
      checkBoard("everything on the right");
      await shot("d04-everything-right");

      // ---- Reordering along its own strip.
      panels.reset();
      await wait(250);
      const here = await dragTo("reference", () => onStrip("notation"));
      expect(here === "Move Reference here", "the reorder label, got " + here);
      expect(panels.groupOf("reference").panels.join() === "reference,notation,tree", "Reference first: " + where());
      await dragTo("notation", () => pastStrip("tree"));
      expect(panels.groupOf("reference").panels.join() === "reference,tree,notation", "Notation last: " + where());

      // ---- The tab menu: right-click, then Move to ▸ Left.
      panels.reset();
      await wait(250);
      const at = center(tab("tree"));
      tab("tree").dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: at.x, clientY: at.y }));
      await until(() => $(".tab-menu"), "the tab menu");
      const menu = $(".tab-menu");
      const items = $$("[role='menuitem']", menu).map((b) => b.textContent.trim() + (b.disabled ? " (off)" : ""));
      expect(items.join(", ") === "Left, Right, Top, Bottom, Engine analysis, Game list",
        "Move to the four sides and into the two other groups, got " + items.join(", "));
      expect(menu.getAttribute("aria-label") === "Move Opening tree" && document.activeElement === $$("[role='menuitem']", menu)[0],
        "the menu named and its first item focused");
      await shot("d05-tab-menu");
      document.activeElement.click();
      await until(() => !$(".tab-menu") && panels.sideOf("tree") === "left", "Move to ▸ Left");
      await until(() => document.activeElement === tab("tree"), "the focus on the moved tab");
      expect(where() === "left: tree; right: notation+reference | analysis; top: -; bottom: games", "the tree on the left: " + where());

      // Shift+F10 on a focused tab; ↓ to Move into ▸ the notation's group; Enter.
      tab("games").focus();
      key(tab("games"), "F10", { shiftKey: true });
      await until(() => $(".tab-menu"), "the menu from Shift+F10");
      const bottomItem = $(".tab-menu [data-side='bottom']");
      expect(bottomItem.disabled, "Bottom off: the game list is there on its own");
      const target = panels.groupOf("notation").id;
      for (let i = 0; i < 8 && document.activeElement?.dataset.group !== target; i++) key(document.activeElement, "ArrowDown");
      expect(document.activeElement?.dataset.group === target &&
        document.activeElement.textContent.trim() === "Notation · Reference", "↓ reaches the notation's group");
      document.activeElement.click();
      await until(() => panels.groupOf("games")?.id === target, "the game list in the notation's group");
      await until(() => document.activeElement === tab("games"), "the focus on the moved tab");
      expect(!$("#dock-bottom"), "the bottom side gone");
      // The context-menu key, then Escape: closed, the focus back on the tab.
      tab("notation").focus();
      key(tab("notation"), "ContextMenu");
      await until(() => $(".tab-menu"), "the menu from the context-menu key");
      key(document.activeElement, "Escape");
      await until(() => !$(".tab-menu") && document.activeElement === tab("notation"), "Escape closes it, focus back on the tab");
      // A click elsewhere closes it.
      key(tab("notation"), "ContextMenu");
      await until(() => $(".tab-menu"), "the menu again");
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      await until(() => !$(".tab-menu"), "a click elsewhere closes it");

      // ---- The dividers: keys, the pointer, the limits, a double-click.
      panels.reset();
      await wait(300);
      const right = $("[role='separator'][aria-controls='dock-right']");
      const w0 = sizeOf("right");
      const cursor = game.view.cursor;
      right.focus();
      for (let i = 0; i < 3; i++) key(right, "ArrowLeft");
      await wait(200);
      expect(Math.abs(sizeOf("right") - (w0 + 48)) < 1.5, `← ×3 widens the right side by 48 px: ${w0} → ${sizeOf("right")}`);
      key(right, "ArrowRight");
      await wait(200);
      expect(Math.abs(sizeOf("right") - (w0 + 32)) < 1.5, `→ narrows it by 16 px: ${sizeOf("right")}`);
      expect(game.view.cursor === cursor, "the arrows on a divider don't step through the game");
      expect(Number(right.getAttribute("aria-valuenow")) === Math.round((sizeOf("right") / appBox().width) * 100),
        "aria-valuenow follows: " + right.getAttribute("aria-valuenow"));
      // The pointer: 300 px to the left, and the board gets smaller.
      const big = boardSize();
      let p = center(right);
      pointer("pointerdown", p.x, p.y);
      pointer("pointermove", p.x - 150, p.y);
      pointer("pointermove", p.x - 300, p.y);
      pointer("pointerup", p.x - 300, p.y);
      await wait(300);
      expect(Math.abs(sizeOf("right") - (w0 + 332)) < 1.5, `dragged 300 px: ${sizeOf("right")}`);
      expect(boardSize() < big, `a smaller board: ${big} → ${boardSize()}`);
      checkBoard("the right side wider");
      await shot("d06-big-notation");
      // As far as it goes: the middle keeps a 160 px board with its chrome.
      p = center(right);
      pointer("pointerdown", p.x, p.y);
      pointer("pointermove", p.x - 5000, p.y);
      pointer("pointerup", p.x - 5000, p.y);
      await wait(300);
      expect(Math.abs(rect(".dock-middle").width - 206) < 1.5 && boardSize() < 161,
        `the divider stops at the middle's minimum: middle ${rect(".dock-middle").width}, board ${boardSize()}`);
      checkBoard("the smallest middle");
      // The other way: the right side keeps 220 px.
      p = center(right);
      pointer("pointerdown", p.x, p.y);
      pointer("pointermove", p.x + 5000, p.y);
      pointer("pointerup", p.x + 5000, p.y);
      await wait(300);
      expect(Math.abs(sizeOf("right") - 220) < 1.5, `the divider stops at the side's minimum: ${sizeOf("right")}`);
      await shot("d07-big-board");
      right.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      await wait(300);
      expect(Math.abs(sizeOf("right") - w0) < 1.5, `a double-click resets it: ${sizeOf("right")} (was ${w0})`);
      // The bottom side's divider, with ↑.
      const bottom = $("[role='separator'][aria-controls='dock-bottom']");
      const h0 = sizeOf("bottom");
      bottom.focus();
      key(bottom, "ArrowUp");
      key(bottom, "ArrowUp");
      await wait(200);
      expect(Math.abs(sizeOf("bottom") - (h0 + 32)) < 1.5, `↑ ×2 raises the bottom side by 32 px: ${h0} → ${sizeOf("bottom")}`);
      // Between two groups: the tree split off below the notation.
      tab("tree").focus();
      key(tab("tree"), "F10", { shiftKey: true });
      await until(() => $(".tab-menu [data-side='right']"), "the tree's menu");
      $(".tab-menu [data-side='right']").click();
      await until(() => where() === "left: -; right: notation+reference | tree | analysis; top: -; bottom: games",
        "the tree in a group of its own below the notation: " + where());
      await wait(200);
      const between = $("#dock-right > [role='separator']");
      expect(between && between.getAttribute("aria-orientation") === "horizontal", "a divider between the two groups");
      const g = (id) => rect(`[data-group-id='${panels.groupOf(id).id}']`).height;
      const [a0, b0] = [g("notation"), g("tree")];
      expect(Math.abs(a0 - b0) < 2, `the two groups split the room evenly: ${a0}, ${b0}`);
      between.focus();
      key(between, "ArrowDown");
      key(between, "ArrowDown");
      await wait(200);
      expect(Math.abs(g("notation") - (a0 + 32)) < 2 && Math.abs(g("tree") - (b0 - 32)) < 2,
        `↓ ×2 moves the boundary 32 px: ${g("notation")}, ${g("tree")}`);
      p = center(between);
      pointer("pointerdown", p.x, p.y);
      pointer("pointermove", p.x, p.y - 5000);
      pointer("pointerup", p.x, p.y - 5000);
      await wait(200);
      expect(Math.abs(g("notation") - 80) < 1.5, `the notation's group keeps 80 px: ${g("notation")}`);
      between.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      await wait(200);
      expect(Math.abs(g("notation") - g("tree")) < 2, "a double-click splits them evenly again");

      // ---- View ▸ Reset layout.
      rb("Reset layout").click();
      await wait(300);
      expect(where() === DEFAULT && Math.abs(sizeOf("right") - w0) < 1.5 && Math.abs(sizeOf("bottom") - h0) < 1.5,
        "Reset layout restores the default: " + where());

      // ---- Saved: notation at the bottom, the right side at 40%.
      await dragTo("notation", () => onBand("bottom", "start"));
      expect(where() === "left: -; right: tree+reference | analysis; top: -; bottom: notation | games",
        "the notation left of the game list: " + where());
      panels.resizeSide("right", 0.4);
      await untilAsync(async () => {
        const l = (await api.uiPrefs()).layout;
        return l?.version === 2 && l.zones.right.size === 0.4 && l.zones.bottom.groups.map((x) => x.panels.join("+")).join(" | ") === "notation | games";
      }, "the layout saved");
      const saved = await api.uiPrefs();
      expect(!("treeVisible" in saved) && !("listShare" in saved), "no older prefs");
      expect(saved.layout.zones.bottom.groups.every((x) => !("id" in x)), "group ids aren't saved");
      sessionStorage.setItem("smoke-dock-right", String(sizeOf("right")));
      await shot("d08-notation-bottom");
      await reload("restore");
      return;
    }

    if (phase === "restore") {
      await log("after the reload: the saved layout");
      await until(() => prefs.raw("layout") !== undefined && where() === "left: -; right: tree+reference | analysis; top: -; bottom: notation | games",
        "the layout restored, got " + where());
      await wait(500);
      const before = Number(sessionStorage.getItem("smoke-dock-right"));
      expect(Math.abs(sizeOf("right") - before) < 1.5 && Math.abs(sizeOf("right") - 0.4 * appBox().width) < 1.5,
        `the right side's size restored: ${sizeOf("right")}`);
      checkBoard("restored");
      // An older settings file: a version 1 layout (UI-19) with the tree
      // showing, and the game list's old divider.
      await api.setUiPref("layout", {
        version: 1,
        zones: {
          left: { size: 0, groups: [] },
          right: { size: 0.6, groups: [
            { panels: ["notation", "tree", "reference"], active: "tree", share: 0.75 },
            { panels: ["analysis"], active: "analysis", share: 0.25 },
          ] },
          top: { size: 0, groups: [] },
          bottom: { size: 0.28, groups: [{ panels: ["games"], active: "games", share: 1 }] },
        },
      });
      await api.setUiPref("listShare", 0.4);
      await reload("migrate");
      return;
    }

    if (phase === "migrate") {
      await log("after the reload: a version 1 layout and listShare");
      await until(() => prefs.raw("listShare") === 0.4, "the prefs loaded");
      await wait(500);
      expect(where() === DEFAULT && panels.isShowing("tree"), "the default layout with the tree showing: " + where());
      expect(Math.abs(sizeOf("bottom") - 0.4 * appBox().height) < 1.5,
        `listShare carried over to the bottom side: ${sizeOf("bottom")} of ${appBox().height}`);
      tab("notation").click();
      await untilAsync(async () => {
        const p = await api.uiPrefs();
        return p.layout?.version === 2 && p.layout.zones.bottom.size === 0.4 && !("listShare" in p);
      }, "the layout saved with the bottom side at 0.4, and listShare gone");
      // A version 2 layout without the Reference tab (a panel added later):
      // it joins its default neighbours.
      await api.setUiPref("layout", {
        version: 2,
        zones: {
          left: { size: 0.25, groups: [{ panels: ["tree", "notation"], active: "notation", share: 1 }] },
          right: { size: 0, groups: [] },
          top: { size: 0.2, groups: [{ panels: ["games"], active: "games", share: 0.6 }, { panels: ["analysis"], active: "analysis", share: 0.4 }] },
          bottom: { size: 0, groups: [] },
        },
      });
      await reload("incomplete");
      return;
    }

    if (phase === "incomplete") {
      await log("after the reload: a layout without the Reference tab");
      await until(() => prefs.raw("layout") !== undefined, "the prefs loaded");
      await wait(500);
      expect(where() === "left: tree+notation+reference; right: -; top: games | analysis; bottom: -",
        "Reference added to its neighbours' group: " + where());
      checkBoard("left and top");
      await shot("d09-left-and-top");
      // Unreadable: a side's size out of range.
      await api.setUiPref("layout", { version: 2, zones: { left: { size: 2, groups: [] } } });
      await reload("unreadable");
      return;
    }

    if (phase === "unreadable") {
      await log("after the reload: an unreadable layout");
      await until(() => prefs.raw("layout") !== undefined, "the prefs loaded");
      await wait(500);
      expect(where() === DEFAULT && panels.isShowing("notation"), "the default layout: " + where());
      checkBoard("default after an unreadable layout");
      sessionStorage.removeItem(PHASE);
      sessionStorage.removeItem("smoke-dock-right");
      await log("done");
      return;
    }

    throw new Error("unknown phase " + phase);
  } catch (e) {
    sessionStorage.removeItem(PHASE);
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
