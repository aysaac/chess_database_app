import { open, save } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, isAppError } from "../api";
import { analysis } from "./analysis.svelte";
import { db, inDocuments, PGN_FILTER } from "./db.svelte";
import { game } from "./game.svelte";
import { notices } from "./notify.svelte";

/** The welcome guide's pages: how to start, importing games, the engine, tips. */
export type WelcomeStep = "start" | "import" | "engine" | "tips";

/** `/a/b/Games.chessdb` → `Games.chessdb` (either separator). */
export function baseName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
}

/** The folder part of a path, for display. */
export function folderName(path: string): string {
  return path.slice(0, Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")));
}

/**
 * Onboarding (QOL-08): the welcome guide that opens on a first run, the
 * sample games, and the import wizard that makes a database from PGN files.
 */
class Onboarding {
  open = $state(false);
  step = $state<WelcomeStep>("start");
  /** Games in the sample database, and where it lives. */
  sampleGames = $state(0);
  samplePath = $state("");

  /** PGN files chosen in the import wizard. */
  files = $state<string[]>([]);
  /** Where the new database goes. */
  target = $state("");
  importing = $state(false);
  /** The file being imported (index into `files`). */
  current = $state(0);
  /** What the finished import made. */
  result = $state<{ name: string; games: number; warnings: number; skipped: number; complete: boolean } | null>(
    null,
  );
  error = $state("");

  /** Opens the guide on a first run. */
  async check() {
    try {
      const status = await api.onboardingStatus();
      this.sampleGames = status.sampleGames;
      this.samplePath = status.samplePath;
      if (status.showWelcome && !this.open) this.show();
    } catch (e) {
      notices.error(e);
    }
  }

  show(step: WelcomeStep = "start") {
    if (!this.importing) this.resetImport();
    this.step = step;
    this.open = true;
    if (step === "engine") void analysis.loadEngines();
  }

  /** Closes the guide; it doesn't open by itself again. */
  async close() {
    this.open = false;
    await api.finishWelcome().catch(notices.error.bind(notices));
  }

  goto(step: WelcomeStep) {
    this.error = "";
    // A finished import: choosing "Import your games" again starts over.
    if (step === "import" && this.result && !this.importing) this.resetImport();
    this.step = step;
    if (step === "engine" && !analysis.engines.length) void analysis.loadEngines();
  }

  resetImport() {
    this.files = [];
    this.target = "";
    this.result = null;
    this.error = "";
    this.current = 0;
  }

  /** Shows the first game of the database that was just opened, unless the board has unsaved work. */
  async #showFirstGame() {
    const info = db.info;
    if (info && info.games > 0 && !game.view?.dirty) await game.open(info.handle, 0, 0);
  }

  async openSample() {
    if (await db.openSample()) {
      await this.#showFirstGame();
      this.goto("engine");
    }
  }

  async openExisting() {
    const before = db.info?.handle;
    await db.openDialog();
    if (db.info && db.info.handle !== before) {
      await this.#showFirstGame();
      this.goto("engine");
    }
  }

  async chooseFiles() {
    const picked = await open({
      multiple: true,
      filters: PGN_FILTER,
      title: "Choose PGN files to import",
      defaultPath: await inDocuments(),
    });
    if (picked?.length) await this.setFiles(picked);
  }

  /** Sets the files to import; the database is named after the first one. */
  async setFiles(paths: string[]) {
    this.files = paths;
    this.result = null;
    this.error = "";
    const name = baseName(paths[0]).replace(/\.pgn$/i, "");
    try {
      this.target = await api.suggestDbPath(name);
    } catch (e) {
      this.target = "";
      this.error = `Couldn't find a folder for the new database (${errorMessage(e)}). Pick one with Change…`;
    }
  }

  async chooseTarget() {
    const path = await save({
      title: "Save the new database as",
      defaultPath: this.target || (await inDocuments("My games.chessdb")),
    });
    if (path) this.target = path;
  }

  /** Creates the database and imports the files into it, one after another. */
  async runImport() {
    if (this.importing || !this.files.length || !this.target) return;
    this.importing = true;
    this.error = "";
    this.result = null;
    try {
      try {
        await db.create(this.target);
      } catch (e) {
        const name = baseName(this.target).replace(/\.chessdb$/i, "");
        this.error =
          isAppError(e) && e.code === "alreadyExists"
            ? `There's already a database called ${name} in that folder. Pick another name or folder with Change…`
            : `Couldn't create ${name}: ${errorMessage(e)}. Pick another folder with Change…`;
        return;
      }
      const handle = db.info?.handle;
      let warnings = 0;
      let skipped = 0;
      let complete = true;
      for (let i = 0; i < this.files.length; i++) {
        this.current = i;
        const ok = await db.importFrom(this.files[i]);
        warnings += db.importReport?.warningCount ?? 0;
        skipped += db.importReport?.skipped ?? 0;
        if (!ok || db.info?.handle !== handle) {
          complete = false;
          break;
        }
      }
      if (db.info && db.info.handle === handle) {
        this.result = { name: db.info.name, games: db.info.games, warnings, skipped, complete };
        await this.#showFirstGame();
      }
    } finally {
      this.importing = false;
    }
  }
}

export const onboarding = new Onboarding();
