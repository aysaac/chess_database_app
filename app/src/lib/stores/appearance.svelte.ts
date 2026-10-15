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
