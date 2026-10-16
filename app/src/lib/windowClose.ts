// What every window does when it's asked to close (DB-07), whichever shell
// it shows (the library, an editor or a future one): main.ts sets it up in
// each. The shell's own checks run first (an editor asks about unsaved
// games); then the preferences still waiting for their debounce are saved.
// A Tauri window's page gets no blur, pagehide, visibilitychange,
// beforeunload or unload in time to send anything when the window closes,
// so without this a change made in the last 400 ms was lost.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { prefs } from "./stores/prefs.svelte";

/** The most a closing window waits for its preferences to be saved. */
const SAVE_WAIT_MS = 1000;

/** Answers whether the window may close (false keeps it open). */
type CloseCheck = () => Promise<boolean>;

const checks = new Set<CloseCheck>();

/** Runs `check` before this window closes; returns a function that stops it. */
export function beforeClose(check: CloseCheck): () => void {
  checks.add(check);
  return () => checks.delete(check);
}

/**
 * Handles this window's close requests. Tauri holds the close while the
 * handler runs and closes the window (`destroy`, granted to every window)
 * once it returns without `preventDefault`; if it throws or never returns,
 * the window stays open. So a save that fails or doesn't answer within
 * SAVE_WAIT_MS doesn't hold the close up. A check that throws keeps the
 * window open, as one that answers false does: an editor's guards unsaved
 * games.
 */
export function handleClose() {
  return getCurrentWindow().onCloseRequested(async (event) => {
    for (const check of checks) {
      let ok = false;
      try {
        ok = await check();
      } catch (e) {
        console.error(e);
      }
      if (!ok) {
        event.preventDefault();
        return;
      }
    }
    const saved = prefs.flush().catch((e) => console.error(e));
    await Promise.race([saved, new Promise((r) => setTimeout(r, SAVE_WAIT_MS))]);
  });
}
