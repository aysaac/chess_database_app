// Smoke-test driver for the panel-group minimize button (collapse.js,
// scratch): minimizing the Game list group to a status bar icon and
// restoring it, and that the layout pref round-trips after a reload.
(async () => {
  const ROOT = "__ROOT__";
  const OUT = "__OUT__";

  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { db, game, api, invoke, panels } = window.__chessdb;
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
  const until = async (cond, what) => {
    for (let i = 0; i < 40 && !cond(); i++) await wait(50);
    expect(cond(), what);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  const phase = sessionStorage.getItem("collapse-phase") || "start";

  try {
    await log("start, phase=" + phase);

    if (phase === "start") {
      await db.createAt(OUT + "/smoke.chessdb");
      await db.importFrom(ROOT + "/testdata/pgn/variations.pgn");
      await shot("01-imported");

      expect(!!document.querySelector('[data-group-id] .tabs button.collapse'), "a collapse button in a tab strip");

      const gamesGroup = panels.groupOf("games");
      expect(!!gamesGroup, "the games panel has a group");
      const gid = gamesGroup.id;

      const strip = document.querySelector(`[data-dock-strip="${gid}"]`);
      expect(!!strip, "the Game list's tab strip");
      const btn = strip.querySelector("button.collapse");
      expect(!!btn, "the Game list's collapse button");
      btn.click();
      await wait(100);

      expect(panels.layout.zones.bottom.groups.find((g) => g.id === gid)?.collapsed === true, "games group collapsed");
      expect(!document.querySelector(`[data-group-id="${gid}"]`), "the games group unmounted once collapsed");
      await shot("02-collapsed");

      const tray = document.querySelector(".minimized button");
      expect(!!tray, "a restore button in the status bar tray");
      expect(tray.textContent.includes("Game list"), "the tray button names the Game list");
      await shot("03-tray");

      sessionStorage.setItem("collapse-phase", "reloaded");
      await log("reloading");
      location.reload();
      return;
    }

    if (phase === "reloaded") {
      await until(() => db.info, "the database reopened");
      const gamesGroup = panels.groupOf("games");
      expect(!!gamesGroup, "the games panel still has a group after reload");
      expect(gamesGroup.collapsed === true, "the games group is still collapsed after reload");
      await shot("04-still-collapsed-after-reload");

      const tray = document.querySelector(".minimized button");
      expect(!!tray, "the restore tray survives the reload");
      tray.click();
      await wait(100);

      expect(panels.groupOf("games")?.collapsed === false, "the games group restored");
      expect(!!document.querySelector(`[data-group-id="${panels.groupOf("games").id}"]`), "the games group remounted");
      expect(!document.querySelector(".minimized button"), "the tray is empty again");
      await shot("05-restored");

      sessionStorage.removeItem("collapse-phase");
      await log("done");
      return;
    }
  } catch (e) {
    sessionStorage.removeItem("collapse-phase");
    await log("FAILED: " + (e && e.stack ? e.stack : e));
  }
})();
