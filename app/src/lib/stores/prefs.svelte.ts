import { api, onUiPref } from "../api";
import { PrefSync } from "../prefsLedger";

export type { PrefValue } from "../prefsLedger";

/**
 * Preferences that describe one window rather than the app (DB-07: several
 * editor windows can be open at once). A window saves them as usual, so
 * one opened later starts from the last saved, but a change doesn't reach
 * the other windows already open: dragging a divider or moving a panel in
 * one editor shouldn't rearrange the others, nor flipping its board turn
 * theirs. Every other preference belongs to the app (the theme, the
 * board's look, the engine lines, the library's favourites, layout and
 * preview size…) and reaches every open window as soon as it's saved.
 */
const PER_WINDOW: ReadonlySet<string> = new Set([
  // The dock's groups, tabs and sizes (UI-04, UI-19), and the older prefs
  // it replaced, which are removed when it's saved.
  "layout",
  "treeVisible",
  "listShare",
  // The Reference tab's dividers (OPEN-14).
  "referenceSizes",
  // Which side of the board is at the bottom.
  "orientation",
  // Which database the opening tree reads.
  "treeSource",
]);

/** This window's copy of the preferences; reactive, so whatever reads them follows changes. */
const values = $state<Record<string, unknown>>({});

/**
 * Frontend preferences (board orientation, pane sizes…), saved in the app's
 * settings file by the Rust side (INF-02). Each window holds a copy, which
 * `init` loads and keeps in step with the changes other windows save
 * (prefsLedger.ts). Saving goes through `api` at the time of the call, so
 * smoke drivers can wrap it.
 */
export const prefs = new PrefSync(values, {
  load: () => api.uiPrefs(),
  save: (key, value) => api.setUiPref(key, value),
  listen: (take) =>
    onUiPref((change) => {
      if (!PER_WINDOW.has(change.key)) take(change.key, change.value, change.revision);
    }),
});

// Send what's waiting for its debounce when the window loses focus (the next
// window may show it at once) or the page goes away (a reload). Neither gets
// a save out before a Tauri window closes, so closing a window saves too
// (windowClose.ts).
window.addEventListener("blur", () => void prefs.flush());
window.addEventListener("pagehide", () => void prefs.flush());
