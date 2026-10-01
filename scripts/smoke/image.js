// Smoke-test driver for board images (QOL-06): the "Board image" menu (in the
// ribbon's Clipboard group, UI-18), copying the board as PNG and SVG (the
// harness saves the private display's clipboard on `clip:<name>` as
// <out>/clips/), saving files (no native dialog: the path is given directly)
// and a failed save.
(async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  while (!window.__chessdb) await wait(100);
  const { game, prefs, invoke } = window.__chessdb;
  // A native dialog would hang the run on the private display.
  game.confirmDiscard = async () => true;
  const log = (m) => invoke("dev_log", { message: String(m) });
  const shot = async (name) => {
    await wait(800);
    await log("shot:" + name);
    await wait(1500);
  };
  // The harness reads the clipboard after the log line; don't replace it meanwhile.
  const clip = async (name) => {
    await log("clip:" + name);
    await wait(1500);
  };
  const expect = (ok, what) => {
    if (!ok) throw new Error("expected " + what);
  };
  const until = async (cond, what, ms = 6000) => {
    const end = performance.now() + ms;
    while (!cond() && performance.now() < end) await wait(50);
    expect(cond(), what);
  };
  const $ = (s) => document.querySelector(s);
  const button = (text) =>
    [...document.querySelectorAll("button")].find((b) => b.textContent.replace(/\s+/g, " ").trim() === text);
  const notice = () => $(".notice")?.textContent.trim() ?? "";
  const noticeError = () => $(".notice")?.classList.contains("error") ?? false;
  const menu = () => $(".image-menu [role='menu']");
  const openMenu = async () => {
    if (!menu()) button("Board image").click();
    await until(() => menu(), "the board image menu opens");
  };
  const choose = async (text) => {
    await openMenu();
    const item = [...menu().querySelectorAll("[role^='menuitem']")].find((b) => b.textContent.includes(text));
    expect(item, "menu item " + text);
    item.click();
    await wait(50);
  };
  // Picks a copy item, waits for its notice and has the harness save the clipboard.
  const copy = async (item, text, name) => {
    const start = performance.now();
    await choose(item);
    expect(!menu(), "the menu closes after an action");
    await until(() => notice().startsWith(text) && !noticeError(), `notice "${text}", got "${notice()}"`);
    await log(`  ${name}: ${Math.round(performance.now() - start)} ms from the click to the notice`);
    await clip(name);
  };
  window.addEventListener("error", (e) => log("window error: " + e.message));
  window.addEventListener("unhandledrejection", (e) =>
    log("unhandled rejection: " + (e.reason?.message ?? JSON.stringify(e.reason))),
  );

  try {
    await log("start");
    prefs.set("boardImageSize", 480);
    prefs.set("boardTheme", "walnut");
    await game.newGame();
    for (const [o, d] of [["e2", "e4"], ["e7", "e5"], ["g1", "f3"], ["b8", "c6"], ["f1", "b5"]]) {
      await game.play(o, d);
    }
    await game.setShapes([
      { orig: "a7", dest: "a6", brush: "green" },
      { orig: "c6", brush: "red" },
    ]);
    expect(game.view.shapes.length === 2, "two drawings on the position");

    await openMenu();
    const sizes = [...menu().querySelectorAll("[role='menuitemradio']")];
    expect(sizes.length === 3, "three sizes");
    expect(sizes.filter((b) => b.getAttribute("aria-checked") === "true").length === 1, "one size chosen");
    expect(sizes.find((b) => b.getAttribute("aria-checked") === "true").textContent.includes("480"), "480 px chosen");
    await shot("i01-menu");

    await copy("Copy as PNG", "Copied the board as an image (480 × 480 pixels)", "i02-png-480");

    // Flipped and large.
    game.flip();
    await choose("Large");
    expect(menu(), "choosing a size keeps the menu open");
    await until(() => prefs.get("boardImageSize", 0) === 800, "the size is remembered");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await until(() => !menu(), "Escape closes the menu");
    await copy("Copy as PNG", "Copied the board as an image (800 × 800 pixels)", "i03-png-800-black");
    await copy("Copy as SVG", "Copied the board as SVG", "i04-svg");
    game.flip();

    // The name "Save as file…" suggests.
    expect(game.boardImageName === "Board after 3.Bb5.png", "file name, got " + game.boardImageName);
    await game.setHeaders({ ...game.view.headers, white: "Carlsen, M", black: "Caruana: F" });
    expect(game.boardImageName === "Carlsen, M - Caruana_ F after 3.Bb5.png", "file name, got " + game.boardImageName);
    await game.step("back");
    expect(game.boardImageName === "Carlsen, M - Caruana_ F after 2...Nc6.png", "file name, got " + game.boardImageName);
    await game.step("forward");

    const out = "__OUT__";
    await game.saveBoardImageTo(out + "/saved.png");
    await until(() => notice() === "Saved the board image as saved.png", "saved PNG, got " + notice());
    await game.saveBoardImageTo(out + "/saved.svg");
    await until(() => notice() === "Saved the board image as saved.svg", "saved SVG, got " + notice());
    await game.saveBoardImageTo(out + "/no-extension");
    await until(() => notice() === "Saved the board image as no-extension.png", ".png added, got " + notice());

    await game.saveBoardImageTo(out + "/missing-folder/board.png");
    await until(() => noticeError() && notice().startsWith("Couldn't save"), "a failed save explained: " + notice());
    await log("  error: " + notice());
    await shot("i05-save-error");

    // A new game in another tab: the start position, no last move.
    await game.newGame(null, "tab");
    expect(game.boardImageName === "Board.png", "file name at the start, got " + game.boardImageName);
    await copy("Copy as PNG", "Copied the board as an image", "i06-start");

    await log("done");
  } catch (e) {
    await log("FAILED: " + (e?.message ?? JSON.stringify(e)));
  }
})();
