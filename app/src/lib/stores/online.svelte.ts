import { save } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, isAppError } from "../api";
import type { JobDto } from "../bindings/JobDto";
import type { OnlinePlayerDto } from "../bindings/OnlinePlayerDto";
import type { OnlineSiteDto } from "../bindings/OnlineSiteDto";
import type { PeriodDto } from "../bindings/PeriodDto";
import type { SpeedDto } from "../bindings/SpeedDto";
import {
  dbName,
  parseSpeeds,
  requestSpeeds,
  SITE_NAMES,
  SITES,
  sinceProblem,
  siteSpeeds,
  usernameProblem,
} from "../online";
import { db, inDocuments } from "./db.svelte";
import { jobs } from "./jobs.svelte";
import { prefs } from "./prefs.svelte";

/** Where the games go: a new database, or the one that's open. */
export type DownloadTarget = "new" | "open";

/** How the last download ended. */
export type DownloadResult = {
  state: "done" | "cancelled" | "failed";
  message: string;
  /** The database the games went into, and how many it has now. */
  name: string;
  games: number;
};

function today(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/**
 * Downloading a player's games from Lichess or Chess.com (NET-01): the
 * dialog's choices (the site, the name and the filters are remembered),
 * and the download this window runs. The download is a job in Rust: it
 * goes on with the dialog closed, in the status bar.
 */
class OnlineStore {
  /** The dialog is open (the ribbon shows it). */
  dialog = $state(false);
  site = $state<OnlineSiteDto>("lichess");
  username = $state("");
  speeds = $state<SpeedDto[]>([]);
  ratedOnly = $state(false);
  period = $state<PeriodDto>("all");
  since = $state("");
  into = $state<DownloadTarget>("new");
  /** A new database's path, once picked with Change… */
  target = $state("");
  /** Looking the player up, making the database, or downloading. */
  busy = $state(false);
  /** The player being downloaded, once looked up. */
  player = $state<OnlinePlayerDto | null>(null);
  /** The download's job, once it started. */
  job = $state<number | null>(null);
  result = $state<DownloadResult | null>(null);
  error = $state("");

  /** The running download's progress. */
  get progress(): JobDto | undefined {
    return this.job === null ? undefined : jobs.running.find((j) => j.id === this.job);
  }

  /** Fills the dialog for a new download: the remembered choices, into the open database if there is one. */
  prepare() {
    if (this.busy) return;
    const site = prefs.get<string>("onlineSite", "lichess");
    this.site = SITES.includes(site as OnlineSiteDto) ? (site as OnlineSiteDto) : "lichess";
    this.username = prefs.get<string>("onlineUsername", "");
    const speeds = parseSpeeds(prefs.get<string>("onlineSpeeds", ""));
    this.speeds = speeds.length ? speeds : siteSpeeds("lichess");
    this.ratedOnly = prefs.get<boolean>("onlineRatedOnly", false);
    this.period = "all";
    this.setInto(db.info ? "open" : "new");
    this.since = "";
    this.target = "";
    this.result = null;
    this.error = "";
  }

  /** New games only makes sense in a database that has some already. */
  setInto(into: DownloadTarget) {
    this.into = into;
    if (into === "open" && this.period === "all") this.period = "newOnly";
    if (into === "new" && this.period === "newOnly") this.period = "all";
  }

  toggleSpeed(speed: SpeedDto, on: boolean) {
    this.speeds = on
      ? [...new Set([...this.speeds, speed])]
      : this.speeds.filter((s) => s !== speed);
  }

  async chooseTarget() {
    const path = await save({
      title: "Save the new database as",
      defaultPath:
        this.target || (await inDocuments(`${dbName(this.username, this.site)}.chessdb`)),
    });
    if (path) this.target = path;
  }

  /** Looks the player up, makes the database if asked, and downloads. */
  async start() {
    if (this.busy) return;
    const name = this.username.trim();
    const speeds = requestSpeeds(this.site, this.speeds);
    const problem =
      usernameProblem(name) ??
      (speeds === null ? "Pick at least one time control." : null) ??
      (this.period === "since" ? sinceProblem(this.since, today()) : null) ??
      (this.into === "open" && !db.info ? "No database is open: download into a new one." : null);
    if (problem) {
      this.error = problem;
      return;
    }
    const site = this.site;
    this.busy = true;
    this.error = "";
    this.result = null;
    this.player = null;
    try {
      let player;
      try {
        player = await api.onlinePlayer(site, name);
      } catch (e) {
        this.error = errorMessage(e);
        return;
      }
      this.player = player;
      prefs.set("onlineSite", site);
      prefs.set("onlineUsername", player.username);
      prefs.set("onlineSpeeds", this.speeds.join(","));
      prefs.set("onlineRatedOnly", this.ratedOnly);
      if (this.into === "new") {
        let path = this.target;
        try {
          path ||= await api.suggestDbPath(dbName(player.username, site));
          await db.create(path);
        } catch (e) {
          this.error =
            isAppError(e) && e.code === "alreadyExists"
              ? "There's already a database with that name in that folder. Pick another name or folder with Change…"
              : `Couldn't make the database: ${errorMessage(e)}. Pick another folder with Change…`;
          return;
        }
      }
      const handle = db.info?.handle;
      const done = await db.download(
        {
          site,
          username: player.username,
          speeds: speeds ?? [],
          ratedOnly: this.ratedOnly,
          period: this.period,
          since: this.period === "since" ? this.since.trim() : null,
        },
        (id) => (this.job = id),
      );
      if (!done) return;
      const here = db.info?.handle === handle ? db.info : null;
      const state = done.state === "running" ? "done" : done.state;
      this.result = {
        state,
        message:
          state === "failed"
            ? (done.error ?? "")
            : state === "cancelled"
              ? `The download of ${player.username}'s games from ${SITE_NAMES[site]} was cancelled.`
              : done.message,
        name: here?.name ?? "",
        games: here?.games ?? 0,
      };
    } finally {
      this.busy = false;
      this.job = null;
    }
  }

  cancel() {
    if (this.job !== null) void jobs.cancel(this.job);
  }
}

export const online = new OnlineStore();
