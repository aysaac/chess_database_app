// Layout driver: checks the board, notation and game list at several window
// sizes. The webview can't resize its own window, so it sizes #app instead,
// which runs the same grid and ResizeObserver path. Screenshots of sizes
// larger than the harness window are cropped. Run with
// scripts/smoke/run.sh scripts/smoke/layout.js
(async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { invoke, game } = window.__chessdb;
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

  // [width, height, label]: null keeps the real window size.
  const sizes = [
    [null, null, "window"],
    [960, 600, "tiled"],
    [1280, 800, "laptop"],
    [2048, 1240, "fullscreen"],
    [720, 1000, "narrow"],
    [1600, 520, "short"],
  ];

  try {
    await log("start");
    const app = document.getElementById("app");
    // The promotion picker stays open through all sizes: it must fit the board.
    await game.newGame("8/4P3/8/8/8/8/k7/4K3 w - - 0 1");
    await game.play("e7", "e8");
    expect(game.pendingPromotion, "promotion picker open");
    for (const [w, h, label] of sizes) {
      app.style.width = w ? w + "px" : "";
      app.style.height = h ? h + "px" : "";
      await wait(400);
      const board = rect(".board-square");
      const cg = rect(".board-square cg-board");
      const column = rect(".board-column");
      const nav = rect(".board-column .nav");
      const notation = rect(".game-column");
      const work = rect(".workspace");
      const list = rect(".games");
      const whole = rect(".app");
      await log(
        `${label} ${Math.round(whole.width)}x${Math.round(whole.height)}: board ${board.width}, ` +
          `notation ${Math.round(notation.width)}, list ${Math.round(list.height)}, ` +
          `workspace ${Math.round(work.height)}`,
      );
      expect(board.width === board.height, `${label}: a square board, got ${board.width}x${board.height}`);
      expect(Math.abs(cg.width - board.width) < 1, `${label}: chessground to fill the board, got ${cg.width}x${cg.height} in ${board.width} (dpr ${devicePixelRatio})`);
      expect(board.left >= column.left && board.right <= column.right, `${label}: board inside its column`);
      expect(board.right <= notation.left, `${label}: board not overlapping the notation`);
      expect(nav.bottom <= work.bottom + 0.5, `${label}: nav bar inside the workspace`);
      expect(notation.width >= 320, `${label}: notation at least 320px, got ${notation.width}`);
      expect(list.bottom <= whole.bottom, `${label}: game list inside the window`);
      // The panel tabs (UI-19) fit the game column, none cut off.
      const strip = document.querySelector(".game-column [role='tablist']");
      const lastTab = rect(".game-column [role='tab']:last-child");
      expect(strip && strip.scrollWidth <= strip.clientWidth && lastTab.right <= notation.right,
        `${label}: the panel tabs fit the game column`);
      const promo = rect(".board-square .picker");
      expect(promo.left >= board.left && promo.right <= board.right && promo.top >= board.top &&
        promo.bottom <= board.bottom, `${label}: promotion picker inside the board, got ${promo.width}px`);
      // The board should use the space it has: either the height (under the
      // nav) or the width (next to a 320px notation) is nearly used up.
      const spareY = work.bottom - nav.bottom - 4;
      const spareX = notation.width - 320;
      expect(Math.min(spareY, spareX) < 16 || board.width === 160, `${label}: board filling its space`);
      // Nothing is wider than the window (the game list's columns used to
      // widen everything in narrow windows).
      for (const sel of [".ribbon", ".workspace", ".games", ".tabs"]) {
        expect(rect(sel).right <= whole.right + 0.5, `${label}: ${sel} inside the window, ends at ${rect(sel).right}`);
      }
      // The ribbon (UI-18): one row, every group whole and inside it.
      const ribbon = rect(".ribbon");
      const groups = [...document.querySelectorAll(".ribbon > .groups > .group")];
      expect(groups.length === 6, `${label}: six ribbon groups, got ${groups.length}`);
      for (const g of groups) {
        const r = g.getBoundingClientRect();
        expect(r.right <= ribbon.right + 0.5 && Math.abs(r.top - groups[0].getBoundingClientRect().top) < 1,
          `${label}: ribbon group ${g.dataset.group} on the one row and inside it (${r.left}–${r.right} of ${ribbon.right})`);
        for (const b of g.querySelectorAll("button")) {
          const br = b.getBoundingClientRect();
          expect(br.right <= r.right + 0.5 && br.bottom <= r.bottom + 0.5, `${label}: ${b.getAttribute("aria-label")} inside its group`);
        }
      }
      expect(ribbon.height < 110, `${label}: the ribbon is one row high, got ${ribbon.height}`);
      await log(`  ribbon ${Math.round(ribbon.width)}x${Math.round(ribbon.height)}: ` +
        groups.map((g) => `${g.dataset.group} ${g.dataset.form}`).join(", "));
      await wait(300);
      await log("shot:layout-" + label);
      await wait(1500);
    }
    app.style.width = app.style.height = "";
    await game.choosePromotion(null);
    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? e));
  }
})();
