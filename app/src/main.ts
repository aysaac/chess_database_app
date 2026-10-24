import { mount } from "svelte";
import { LogicalSize, getCurrentWindow } from "@tauri-apps/api/window";
import "@fontsource-variable/source-serif-4/opsz.css";
import "@fontsource-variable/source-serif-4/opsz-italic.css";
import "@fontsource/ibm-plex-sans-condensed/400.css";
import "@fontsource/ibm-plex-sans-condensed/500.css";
import "@fontsource/ibm-plex-sans-condensed/600.css";
import "@lichess-org/chessground/assets/chessground.base.css";
import "@lichess-org/chessground/assets/chessground.cburnett.css";
import "./app.css";
import "./board-theme.css";
import App from "./App.svelte";
import LibraryHome from "./lib/components/LibraryHome.svelte";
import { applyAppearance } from "./lib/stores/appearance.svelte";
import { prefs } from "./lib/stores/prefs.svelte";
import { handleClose } from "./lib/windowClose";

// The library (DB-07) is the app's main window; opening a database or
// starting a new game spawns a separate "editor-…" window for it
// (commands::window::open_editor_window), so several can be open side by
// side. Any window that isn't the main one is an editor.
const isMain = getCurrentWindow().label === "main";

// Every window starts the same way, whichever shell it shows: the saved
// preferences first (kept in step with the other windows from then on), then
// the theme from them, then the shell. So no shell reads a default where a
// preference is saved, and no window flashes the wrong theme. If the read
// fails or doesn't answer, `init` gives up waiting after two seconds and
// the window shows anyway; the preferences fill in when a later read
// succeeds (prefsLedger.ts). Closing any window saves its last changes
// (windowClose.ts).
async function start() {
  void handleClose().catch((e) => console.error("couldn't watch for this window closing:", e));
  await prefs.init();
  applyAppearance();
  mount(isMain ? LibraryHome : App, { target: document.getElementById("app")! });
  if (import.meta.env.DEV) await exposeStores();
}

// Dev builds expose the stores so smoke-test scripts can drive the UI
// (see CHESSDB_SMOKE in src-tauri/src/lib.rs). Only once the shell is
// mounted: drivers look at the page as soon as they find these.
async function exposeStores() {
  const [
    { db },
    { game },
    { jobs },
    { analysis },
    { tree },
    { panels },
    { dock },
    { reference },
    { onboarding },
    { notices },
    { library },
    { appearance },
    { online },
    { api },
    { invoke },
  ] = await Promise.all([
    import("./lib/stores/db.svelte"),
    import("./lib/stores/game.svelte"),
    import("./lib/stores/jobs.svelte"),
    import("./lib/stores/analysis.svelte"),
    import("./lib/stores/tree.svelte"),
    import("./lib/stores/panels.svelte"),
    import("./lib/stores/dock.svelte"),
    import("./lib/stores/reference.svelte"),
    import("./lib/stores/onboarding.svelte"),
    import("./lib/stores/notify.svelte"),
    import("./lib/stores/library.svelte"),
    import("./lib/stores/appearance.svelte"),
    import("./lib/stores/online.svelte"),
    import("./lib/api"),
    import("@tauri-apps/api/core"),
  ]);
  Object.assign(window, {
    __chessdb: {
      db, game, jobs, analysis, prefs, tree, panels, dock, reference, onboarding, notices, library, appearance,
      online, api, invoke,
      win: getCurrentWindow(),
      LogicalSize,
    },
  });
}

void start();
