// Layout driver: checks the board, the panels around it and the ribbon at
// several window sizes, in three panel layouts (UI-04): the default one, the
// notation docked at the bottom, and every panel on the left. The webview
// can't resize its own window, so it sizes #app instead, which runs the
// same grid and ResizeObserver path. Screenshots of sizes larger than the
// harness window are cropped. Run with
// scripts/smoke/run.sh scripts/smoke/layout.js
(async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { invoke, game, panels } = window.__chessdb;
  // A native dialog would open on the user's screen, outside the harness's
  // hidden monitor: never ask about unsaved changes.
  game.confirmDiscard = async () => true;
  const log = (m) => invoke("dev_log", { message: String(m) });
  const expect = (ok, what) => {
    if (!ok) throw new Error("expected " + what);
  };
  const rect = (sel) => {
    const el = document.querySelector(sel);
    if (!el) throw new Error("missing " + sel);
    return el.getBoundingClientRect();
  };
  const overlap = (a, b) =>
    Math.min(a.right, b.right) - Math.max(a.left, b.left) > 0.5 &&
    Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0.5;
  const inside = (a, b) =>
    a.left >= b.left - 0.5 && a.right <= b.right + 0.5 && a.top >= b.top - 0.5 && a.bottom <= b.bottom + 0.5;
  const MINIMUM = { left: 220, right: 220, top: 120, bottom: 120 };

  // [width, height, label]: null keeps the real window size.
  const sizes = [
    [null, null, "window"],
    [960, 600, "tiled"],
    [1280, 800, "laptop"],
    [2048, 1240, "fullscreen"],
    [720, 1000, "narrow"],
    [1600, 520, "short"],
  ];

  // The panel layouts, made with the same moves as dragging tabs (dock.js
  // drags them for real).
  const layouts = {
    default: () => panels.reset(),
    "notation-bottom": () => {
      panels.reset();
      panels.move("notation", { kind: "side", side: "bottom", slot: 0 });
    },
    "all-left": () => {
      panels.reset();
      panels.move("notation", { kind: "side", side: "left", slot: 0 });
      const g = () => panels.groupOf("notation").id;
      panels.move("tree", { kind: "group", group: g(), index: 9 });
      panels.move("reference", { kind: "group", group: g(), index: 9 });
      panels.show("notation");
      panels.move("analysis", { kind: "side", side: "left", slot: 9 });
      panels.move("games", { kind: "side", side: "left", slot: 9 });
    },
  };

  try {
    await log("start");
    const app = document.getElementById("app");
    // The promotion picker stays open through all sizes: it must fit the board.
    await game.newGame("8/4P3/8/8/8/8/k7/4K3 w - - 0 1");
    await game.play("e7", "e8");
    expect(game.pendingPromotion, "promotion picker open");
    for (const [name, make] of Object.entries(layouts)) {
      make();
      await wait(300);
      const where = ["left", "right", "top", "bottom"]
        .map((s) => `${s} ${panels.layout.zones[s].groups.map((g) => g.panels.join("+")).join(" | ") || "-"}`)
        .join("; ");
      await log(`layout ${name}: ${where}`);
      if (name === "notation-bottom") expect(panels.sideOf("notation") === "bottom", "the notation at the bottom");
      if (name === "all-left") {
        expect(["right", "top", "bottom"].every((s) => !document.getElementById("dock-" + s)),
          "every side but the left empty and gone");
      }
      for (const [w, h, label] of sizes) {
        app.style.width = w ? w + "px" : "";
        app.style.height = h ? h + "px" : "";
        await wait(400);
        const tag = `${name} ${label}`;
        const board = rect(".board-square");
        const cg = rect(".board-square cg-board");
        const middle = rect(".dock-middle");
        const nav = rect(".board-column .nav");
        const whole = rect(".app");
        const dock = rect(".dock");
        const sides = [...document.querySelectorAll(".dock > .side")];
        await log(
          `${tag} ${Math.round(whole.width)}x${Math.round(whole.height)}: board ${board.width}, middle ` +
            `${Math.round(middle.width)}x${Math.round(middle.height)}, ` +
            sides.map((s) => `${s.dataset.side} ${Math.round(s.getBoundingClientRect()[["left", "right"].includes(s.dataset.side) ? "width" : "height"])}`).join(", "),
        );
        expect(board.width === board.height, `${tag}: a square board, got ${board.width}x${board.height}`);
        expect(Math.abs(cg.width - board.width) < 1, `${tag}: chessground to fill the board, got ${cg.width}x${cg.height} in ${board.width} (dpr ${devicePixelRatio})`);
        // The board and its nav bar inside the middle, clear of every panel and divider.
        expect(inside(board, middle), `${tag}: the board inside the middle`);
        expect(inside(nav, middle), `${tag}: the nav bar inside the middle`);
        for (const el of document.querySelectorAll(".dock > .side, .dock > [role='separator']")) {
          const r = el.getBoundingClientRect();
          const what = el.dataset.side ? `the ${el.dataset.side} side` : el.getAttribute("aria-label");
          expect(!overlap(board, r) && !overlap(nav, r), `${tag}: the board clear of ${what}`);
          expect(inside(r, dock), `${tag}: ${what} inside the dock`);
        }
        // Sides keep their minimum while the middle has more than its own.
        const roomy = middle.width > 206 + 2 && middle.height > 160 + 18 + 74 + 2;
        for (const s of sides) {
          const r = s.getBoundingClientRect();
          const size = ["left", "right"].includes(s.dataset.side) ? r.width : r.height;
          expect(!roomy || size >= MINIMUM[s.dataset.side] - 0.5, `${tag}: the ${s.dataset.side} side at least its minimum, got ${size}`);
        }
        // The board uses the room it has: the height or the width is nearly used up.
        const spareY = middle.height - (board.height + 18 + nav.height);
        const spareX = middle.width - (board.width + 46);
        expect(Math.min(spareY, spareX) < 16 || board.width === 160, `${tag}: board filling its space (spare ${spareX}x${spareY})`);
        const promo = rect(".board-square .picker");
        expect(inside(promo, board), `${tag}: promotion picker inside the board, got ${promo.width}px`);
        // Nothing is wider than the window, and the page never scrolls sideways.
        expect(document.documentElement.scrollWidth <= window.innerWidth + 0.5, `${tag}: no page-level horizontal scroll`);
        for (const sel of [".ribbon", ".dock", ".app > footer"]) {
          expect(rect(sel).right <= whole.right + 0.5, `${tag}: ${sel} inside the window, ends at ${rect(sel).right}`);
        }
        // Every group's panel tabs fit its strip, none cut off.
        for (const strip of document.querySelectorAll(".dock [data-dock-strip]")) {
          const box = strip.getBoundingClientRect();
          const last = [...strip.querySelectorAll("[role='tab']")].at(-1).getBoundingClientRect();
          expect(strip.scrollWidth <= strip.clientWidth + 1 && last.right <= box.right + 0.5,
            `${tag}: the panel tabs fit their strip (${strip.scrollWidth} in ${strip.clientWidth})`);
        }
        // The game list's columns fit it: the ones that don't are left out.
        const head = document.querySelector("#panel-games .row.head");
        if (head?.getClientRects().length) {
          const cells = [...head.querySelectorAll(".head-cell")];
          const hr = head.getBoundingClientRect();
          expect(cells.length >= 2 && cells.every((c) => c.getBoundingClientRect().right <= hr.right + 0.5),
            `${tag}: the game list's columns inside it`);
          await log(`  game list ${Math.round(hr.width)} px: ${cells.length} columns`);
        }
        // The ribbon (UI-18): one row, every group whole and inside it.
        const ribbon = rect(".ribbon");
        const groups = [...document.querySelectorAll(".ribbon > .groups > .group")];
        expect(groups.length === 6, `${tag}: six ribbon groups, got ${groups.length}`);
        for (const g of groups) {
          const r = g.getBoundingClientRect();
          expect(r.right <= ribbon.right + 0.5 && Math.abs(r.top - groups[0].getBoundingClientRect().top) < 1,
            `${tag}: ribbon group ${g.dataset.group} on the one row and inside it (${r.left}–${r.right} of ${ribbon.right})`);
          for (const b of g.querySelectorAll("button")) {
            const br = b.getBoundingClientRect();
            expect(br.right <= r.right + 0.5 && br.bottom <= r.bottom + 0.5, `${tag}: ${b.getAttribute("aria-label")} inside its group`);
          }
        }
        expect(ribbon.height < 110, `${tag}: the ribbon is one row high, got ${ribbon.height}`);
        if (name === "default") {
          await log(`  ribbon ${Math.round(ribbon.width)}x${Math.round(ribbon.height)}: ` +
            groups.map((g) => `${g.dataset.group} ${g.dataset.form}`).join(", "));
        }
        await wait(300);
        await log(`shot:layout-${name}-${label}`);
        await wait(1500);
      }
      app.style.width = app.style.height = "";
    }
    panels.reset();
    await game.choosePromotion(null);
    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e));
  }
})();
