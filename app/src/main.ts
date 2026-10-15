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

// The library (DB-07) is the app's main window; opening a database or
// starting a new game spawns a separate "editor-…" window for it
// (commands::window::open_editor_window), so several can be open side by
// side. Any window that isn't the main one is an editor.
const isMain = getCurrentWindow().label === "main";
const app = mount(isMain ? LibraryHome : App, { target: document.getElementById("app")! });

// Dev builds expose the stores so smoke-test scripts can drive the UI
// (see CHESSDB_SMOKE in src-tauri/src/lib.rs).
if (import.meta.env.DEV) {
  void (async () => {
    const [
      { db },
      { game },
      { jobs },
      { analysis },
      { prefs },
      { tree },
      { panels },
      { dock },
      { reference },
      { onboarding },
      { notices },
      { api },
      { invoke },
    ] = await Promise.all([
      import("./lib/stores/db.svelte"),
      import("./lib/stores/game.svelte"),
      import("./lib/stores/jobs.svelte"),
      import("./lib/stores/analysis.svelte"),
      import("./lib/stores/prefs.svelte"),
      import("./lib/stores/tree.svelte"),
      import("./lib/stores/panels.svelte"),
      import("./lib/stores/dock.svelte"),
      import("./lib/stores/reference.svelte"),
      import("./lib/stores/onboarding.svelte"),
      import("./lib/stores/notify.svelte"),
      import("./lib/api"),
      import("@tauri-apps/api/core"),
    ]);
    Object.assign(window, {
      __chessdb: {
        db, game, jobs, analysis, prefs, tree, panels, dock, reference, onboarding, notices, api, invoke,
        win: getCurrentWindow(),
        LogicalSize,
      },
    });
  })();
}

export default app;
