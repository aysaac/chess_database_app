import { api, errorMessage, isAppError } from "../api";
import type { LichessAccountDto } from "../bindings/LichessAccountDto";
import type { Ranges } from "../selection";
import { notices } from "./notify.svelte";

/**
 * Lichess (NET-11, IO-10, NET-12): who is logged in, logging in and out,
 * and which study dialog is open. Rust keeps the token (in the OS's
 * credential store when there is one); the page only ever knows the name.
 */
class LichessStore {
  account = $state<LichessAccountDto>({ username: null, kept: false });
  /** Waiting for the browser, or checking a pasted token. */
  loggingIn = $state(false);
  /** Why the last login didn't happen. */
  loginError = $state("");
  /** What logging out did, when there's something to say. */
  note = $state("");
  /** The "Import a Lichess study" dialog is open. */
  importDialog = $state(false);
  /** Where that dialog makes a new database, once picked with Change… (empty: in Documents). */
  importTarget = $state("");
  /** The "Send games to Lichess" dialog is open: for these rows, or (null) the list's games. */
  exportDialog = $state<{ ranges: Ranges | null } | null>(null);

  #loaded = false;
  /** Bumped by each login and by a cancel: a login that returns after either is ignored. */
  #run = 0;

  get username(): string | null {
    return this.account.username;
  }

  /** Who is logged in (once; it doesn't read the credential store). */
  async load(force = false) {
    if (this.#loaded && !force) return;
    this.#loaded = true;
    try {
      this.account = await api.lichessAccount();
    } catch (e) {
      notices.error(e);
    }
  }

  /** After an error: a "login" error means Lichess no longer takes the login, which Rust forgot. */
  async checkError(e: unknown) {
    if (isAppError(e) && e.code === "login") await this.load(true);
  }

  /** Logs in through the browser: Lichess's page asks the user to let the app in. */
  async login() {
    if (this.loggingIn) return;
    const run = ++this.#run;
    this.loggingIn = true;
    this.loginError = "";
    this.note = "";
    try {
      const account = await api.lichessLogin();
      if (run === this.#run) this.account = account;
    } catch (e) {
      if (run === this.#run) this.loginError = errorMessage(e);
    } finally {
      if (run === this.#run) this.loggingIn = false;
    }
  }

  /** Stops waiting for the browser. */
  cancelLogin() {
    if (!this.loggingIn) return;
    this.#run++;
    this.loggingIn = false;
    void api.lichessLoginCancel();
  }

  /** Logs in with a personal access token; resolves with whether it worked. */
  async useToken(token: string): Promise<boolean> {
    if (this.loggingIn) return false;
    if (!token.trim()) {
      this.loginError = "Paste the token first.";
      return false;
    }
    const run = ++this.#run;
    this.loggingIn = true;
    this.loginError = "";
    this.note = "";
    try {
      const account = await api.lichessUseToken(token.trim());
      if (run !== this.#run) return false;
      this.account = account;
      return true;
    } catch (e) {
      if (run === this.#run) this.loginError = errorMessage(e);
      return false;
    } finally {
      if (run === this.#run) this.loggingIn = false;
    }
  }

  /** Logs out: the app forgets the token, and Lichess revokes it. */
  async logout() {
    this.cancelLogin();
    try {
      const done = await api.lichessLogout();
      this.account = { username: null, kept: false };
      this.note = done.problem ?? (done.revoked ? "Logged out: Lichess revoked the token." : "");
    } catch (e) {
      notices.error(e);
    }
  }

  /** Opens a Lichess page in the browser: `url`, or where to make a token. */
  async open(url = "") {
    try {
      await api.lichessOpen(url);
    } catch (e) {
      notices.error(e);
    }
  }
}

export const lichess = new LichessStore();
