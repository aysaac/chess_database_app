import { board } from "./board.svelte";
import { prefs } from "./prefs.svelte";

/** The app's colour theme, independent of the board's own colours (UI-01). */
export const APP_THEMES = [
  { id: "system", label: "Match system" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
] as const;

/** How the app looks, saved with the preferences. */
class Appearance {
  get theme(): string {
    const id = prefs.get("appTheme", "system");
    return APP_THEMES.some((t) => t.id === id) ? id : "system";
  }

  set theme(id: string) {
    prefs.set("appTheme", id);
  }
}

export const appearance = new Appearance();

/**
 * Keeps <html> showing the app's theme and the board colours, where app.css
 * and board-theme.css read them, as the preferences change (here or in
 * another window). main.ts starts it once in every window, whichever shell
 * that shows (the library or an editor), once the preferences are loaded.
 */
export function applyAppearance() {
  $effect.root(() => {
    // The board colours (UI-01) apply to every board through <html>.
    $effect(() => {
      document.documentElement.dataset.boardTheme = board.theme;
    });
    // The app's colour theme (Settings): "system" leaves app.css to the OS.
    $effect(() => {
      const theme = appearance.theme;
      if (theme === "system") delete document.documentElement.dataset.theme;
      else document.documentElement.dataset.theme = theme;
    });
  });
}
