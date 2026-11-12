<script lang="ts">
  // Where a database gets its games, and keeping it up to date (NET-01,
  // IO-10, NET-03): a player's games on Lichess or Chess.com, a Lichess
  // study, or a database on your own server. `mode` "open" sets the open
  // database's sources; "new" (from the library) makes a database from them
  // and opens it, where its first update starts. Updates bring only the new
  // games, when the database is opened (if it's set to) or on Update.
  import { save } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { api, errorMessage } from "../api";
  import type { OnlineSiteDto } from "../bindings/OnlineSiteDto";
  import type { ServerAccountDto } from "../bindings/ServerAccountDto";
  import type { ServerDbDto } from "../bindings/ServerDbDto";
  import type { SpeedDto } from "../bindings/SpeedDto";
  import type { SyncSourceDto } from "../bindings/SyncSourceDto";
  import {
    localMidnight,
    requestSpeeds,
    SITE_NAMES,
    SITES,
    siteSpeeds,
    sinceProblem,
    speedLabel,
    usernameProblem,
  } from "../online";
  import { describeSource, endOfDay, hostOf, nameFor } from "../sync";
  import type { ColorDto } from "../bindings/ColorDto";
  import type { LimitPolicyDto } from "../bindings/LimitPolicyDto";
  import { db, inDocuments } from "../stores/db.svelte";
  import LichessAccount from "./LichessAccount.svelte";
  import Modal from "./Modal.svelte";
  import type { StudyInfoDto } from "../bindings/StudyInfoDto";
  import { lichess } from "../stores/lichess.svelte";

  let {
    mode,
    onClose,
    only,
    onBack,
  }: {
    mode: "open" | "new";
    onClose: () => void;
    /** Limits the dialog to one place the games come from (the new-database chooser's). */
    only?: "online" | "study" | "server";
    /** Back to the chooser. */
    onBack?: () => void;
  } = $props();

  let sources = $state<SyncSourceDto[]>([]);
  let auto = $state(true);
  let kind = $state<"player" | "study" | "server">("player");

  type Kind = typeof kind;
  /**
   * What a database can mix: player accounts with each other (Lichess and
   * Chess.com), but a study or a server database only with its own kind,
   * so each stays the one thing that owns its games.
   */
  const allowed = $derived<Kind[]>(
    only
      ? [{ online: "player", study: "study", server: "server" }[only] as Kind]
      : sources.length
        ? [sources[0].kind]
        : ["player", "study", "server"],
  );
  $effect(() => {
    if (!allowed.includes(kind)) kind = allowed[0];
  });
  const KIND_LABELS: Record<Kind, string> = {
    player: "A player's games",
    study: "A Lichess study",
    server: "My server",
  };
  let error = $state("");
  let busy = $state(false);

  // A player
  let site = $state<OnlineSiteDto>("lichess");
  let username = $state("");
  let speeds = $state<SpeedDto[]>(siteSpeeds("lichess"));
  let ratedOnly = $state(false);
  let color = $state<ColorDto | "">("");
  let fromDay = $state("");
  let toDay = $state("");
  let latestOn = $state(false);
  let latestText = $state("100");
  // A limit on the games, over all the sources
  let limitOn = $state(false);
  let limitText = $state("1000");
  let policy = $state<LimitPolicyDto>("keepLatest");
  // A study
  let link = $state("");
  let studies = $state<StudyInfoDto[] | null>(null);
  let listing = $state(false);
  // The server
  let account = $state<ServerAccountDto | null>(null);
  let serverUrl = $state("");
  let serverUser = $state("");
  let password = $state("");
  let serverDbs = $state<ServerDbDto[] | null>(null);
  let pickedDb = $state("");

  onMount(() => {
    if (mode === "open" && db.syncInfo) {
      sources = db.syncInfo.sources.map((s) => ({ ...s }));
      auto = db.syncInfo.auto;
      limitOn = db.syncInfo.limit !== null;
      if (db.syncInfo.limit !== null) limitText = String(db.syncInfo.limit);
      policy = db.syncInfo.limitPolicy;
    }
    void loadAccount();
  });

  /** Logged in to Lichess: the user's own studies, private and unlisted ones too. */
  $effect(() => {
    const user = lichess.username;
    if (kind !== "study" || !user || studies !== null || listing) return;
    void listStudies(user);
  });

  async function listStudies(user: string) {
    listing = true;
    try {
      studies = await api.lichessStudies(user);
    } catch (e) {
      studies = [];
      error = errorMessage(e);
      void lichess.checkError(e);
    } finally {
      listing = false;
    }
  }

  /** Adds the study picked from the list. */
  function pick(s: StudyInfoDto) {
    link = s.link;
    void add();
  }

  async function loadAccount() {
    try {
      account = await api.serverAccount();
      serverUrl ||= account.suggestedUrl ?? "";
      if (account.username) await loadServerDbs();
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function loadServerDbs() {
    serverDbs = null;
    try {
      serverDbs = await api.serverDatabases();
      pickedDb = serverDbs[0]?.id ?? "";
    } catch (e) {
      error = errorMessage(e);
      account = await api.serverAccount().catch(() => account);
    }
  }

  async function signIn(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    busy = true;
    error = "";
    try {
      account = await api.serverSignIn(serverUrl, serverUser, password);
      password = "";
      await loadServerDbs();
    } catch (err) {
      error = errorMessage(err);
    } finally {
      busy = false;
    }
  }

  async function signOut() {
    account = await api.serverSignOut().catch(() => account);
    serverDbs = null;
    sources = sources.filter((s) => s.kind !== "server");
  }

  function toggleSpeed(s: SpeedDto, on: boolean) {
    speeds = on ? [...new Set([...speeds, s])] : speeds.filter((x) => x !== s);
  }

  /** Adds the source the form describes to the list. */
  async function add() {
    error = "";
    let source: SyncSourceDto;
    if (kind === "player") {
      const problem = usernameProblem(username);
      const chosen = requestSpeeds(site, speeds);
      if (problem || chosen === null) {
        error = problem ?? "Pick at least one time control.";
        return;
      }
      const dateProblem =
        (fromDay.trim() ? sinceProblem(fromDay, "9999-12-31") : null) ??
        (toDay.trim() ? sinceProblem(toDay, "9999-12-31") : null) ??
        (fromDay.trim() && toDay.trim() && fromDay.trim() > toDay.trim()
          ? "The first day is after the last day."
          : null);
      if (dateProblem) {
        error = dateProblem;
        return;
      }
      const latestN = Number(latestText.trim());
      if (latestOn && !(Number.isInteger(latestN) && latestN >= 1)) {
        error = "The latest games is a whole number of games, 1 or more.";
        return;
      }
      source = {
        kind: "player",
        site,
        username: username.trim(),
        speeds: chosen,
        ratedOnly,
        color: color || null,
        since: fromDay.trim() ? localMidnight(fromDay.trim()) : null,
        until: toDay.trim() ? endOfDay(toDay.trim()) : null,
        latest: latestOn ? latestN : null,
        reached: null,
      };
    } else if (kind === "study") {
      if (!link.trim()) {
        error = "Paste the study's link.";
        return;
      }
      busy = true;
      try {
        const study = await api.lichessStudyLookup(link);
        source = { kind: "study", study: study.link, chapter: null, name: study.name, lastModified: null };
      } catch (e) {
        error = errorMessage(e);
        void lichess.checkError(e);
        return;
      } finally {
        busy = false;
      }
    } else {
      const picked = serverDbs?.find((d) => d.id === pickedDb);
      if (!picked || !account?.url || !account.username) {
        error = "Pick a database on the server.";
        return;
      }
      source = {
        kind: "server",
        url: account.url,
        username: account.username,
        database: picked.id,
        name: picked.name,
        next: null,
        epoch: null,
      };
    }
    sources = [...sources.filter((s) => describeSource(s) !== describeSource(source)), source];
    username = "";
    link = "";
  }

  /** The limit as a number, null when off; or undefined when it isn't one. */
  function limit(): number | null | undefined {
    if (!limitOn || allowed[0] !== "player") return null;
    const n = Number(limitText.trim());
    return Number.isInteger(n) && n >= 1 && n <= 100_000_000 ? n : undefined;
  }

  async function saveOpen(updateNow: boolean) {
    const handle = db.info?.handle;
    if (handle === undefined || busy) return;
    busy = true;
    try {
      const max = limit();
      if (max === undefined) {
        error = "The limit is a whole number of games, 1 or more.";
        return;
      }
      await api.setDbSync(handle, sources, auto, max, policy);
      await db.loadSync();
      onClose();
      if (updateNow && sources.length) void db.update();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }

  /** Makes the database, with its sources; opening it starts the first update. */
  async function create() {
    if (!sources.length || busy) return;
    if (limit() === undefined) {
      error = "The limit is a whole number of games, 1 or more.";
      return;
    }
    const path = await save({
      title: "Save the new database as",
      defaultPath: await inDocuments(`${nameFor(sources[0])}.chessdb`),
    });
    if (!path) return;
    busy = true;
    error = "";
    try {
      const info = await api.createDb(path);
      await api.setDbSync(info.handle, sources, auto, limit() ?? null, policy);
      await api.openEditorWindow(path);
      onClose();
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal
  title={only
    ? `New database from ${only === "server" ? "my server" : only === "study" ? "a Lichess study" : "online games"}`
    : mode === "new"
      ? "New database from Lichess, Chess.com or my server"
      : "Keep this database up to date"}
  {onClose}
  width="600px"
>
  <p class="intro">
    {mode === "new"
      ? "Make a database that fills itself from where your games are, then keeps up with them."
      : "Games come from the sources below. An update brings only the new ones."}
  </p>

  <fieldset>
    <legend>Sources</legend>
    {#if sources.length === 0}
      <p class="soft">None yet. Add one below.</p>
    {/if}
    <ul class="sources">
      {#each sources as s (describeSource(s))}
        <li>
          <span>{describeSource(s)}</span>
          <button type="button" aria-label="Remove {describeSource(s)}" onclick={() => (sources = sources.filter((x) => x !== s))}
            >Remove</button
          >
        </li>
      {/each}
    </ul>
  </fieldset>

  <fieldset disabled={busy}>
    <legend>Add a source</legend>
    {#if allowed.length > 1}
      <div class="kinds" role="radiogroup" aria-label="Kind of source">
        {#each allowed as k (k)}
          <label class="check">
            <input type="radio" name="sync-kind" checked={kind === k} onchange={() => (kind = k)} />
            {KIND_LABELS[k]}
          </label>
        {/each}
      </div>
    {:else if !only && sources.length && kind !== "player"}
      <p class="soft">
        {kind === "study" ? "A study" : "A server database"} can't be mixed with other kinds of source: the games it
        owns have to stay clear.
      </p>
    {/if}

    {#if kind === "player"}
      <div class="row">
        {#each SITES as s (s)}
          <label class="check">
            <input
              type="radio"
              name="sync-site"
              checked={site === s}
              onchange={() => {
                site = s;
                speeds = siteSpeeds(s);
              }}
            />
            {SITE_NAMES[s]}
          </label>
        {/each}
      </div>
      {#if site === "lichess"}
        <div class="login">
          <LichessAccount
            why="Log in to Lichess so your own games download 3 times as fast. It's optional for public games, and the login stays in this app (in the system's keyring)."
          />
        </div>
      {/if}
      <label class="field">
        Username on {SITE_NAMES[site]}
        <input bind:value={username} autocomplete="off" autocapitalize="off" spellcheck="false" />
      </label>
      <div class="row">
        {#each siteSpeeds(site) as s (s)}
          <label class="check">
            <input type="checkbox" checked={speeds.includes(s)} onchange={(e) => toggleSpeed(s, e.currentTarget.checked)} />
            {speedLabel(site, s)}
          </label>
        {/each}
        <label class="check"><input type="checkbox" bind:checked={ratedOnly} /> Rated only</label>
      </div>
      <div class="row">
        <label class="field">
          Played as
          <select bind:value={color}>
            <option value="">White or Black</option>
            <option value="white">White only</option>
            <option value="black">Black only</option>
          </select>
        </label>
        <label class="field">
          From day <input bind:value={fromDay} placeholder="YYYY-MM-DD" inputmode="numeric" />
        </label>
        <label class="field">
          To day <input bind:value={toDay} placeholder="YYYY-MM-DD" inputmode="numeric" />
        </label>
      </div>
      <div class="row">
        <label class="check">
          <input type="checkbox" bind:checked={latestOn} />
          Only the latest
          <input class="num" bind:value={latestText} disabled={!latestOn} inputmode="numeric" aria-label="Latest games" />
          games
        </label>
      </div>
    {:else if kind === "study"}
      <LichessAccount
        why="Log in to list your own studies, private and unlisted ones too, and to keep them up to date. The login stays in this app (in the system's keyring); public studies need none."
      />
      {#if lichess.username}
        {#if listing}
          <p class="soft">Listing your studies…</p>
        {:else if studies?.length}
          <ul class="studies" aria-label="Your studies">
            {#each studies as s (s.id)}
              <li>
                <button type="button" onclick={() => pick(s)} title="Add {s.name}">
                  <span>{s.name}</span><span class="soft">{s.updated}</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if studies}
          <p class="soft">{lichess.username} has no studies.</p>
        {/if}
      {/if}
      <label class="field">
        Or paste a study's link
        <input bind:value={link} placeholder="https://lichess.org/study/…" spellcheck="false" />
      </label>
    {:else if account?.username}
      <p class="who">
        Signed in to {hostOf(account.url ?? "")} as <strong>{account.username}</strong>.
        <button type="button" onclick={signOut}>Sign out</button>
        {#if !account.kept}<span class="soft">Until the app closes: no system keyring.</span>{/if}
      </p>
      {#if serverDbs === null}
        <p class="soft">Loading its databases…</p>
      {:else if serverDbs.length === 0}
        <p class="soft">This account has no databases on the server yet.</p>
      {:else}
        <label class="field">
          Database
          <select bind:value={pickedDb}>
            {#each serverDbs as d (d.id)}
              <option value={d.id}>{d.name} ({d.games.toLocaleString()} games, {d.owner})</option>
            {/each}
          </select>
        </label>
      {/if}
    {:else}
      <form class="signin" onsubmit={signIn}>
        <label class="field">
          Server address
          <input bind:value={serverUrl} placeholder="chess.example.com" spellcheck="false" autocapitalize="off" />
        </label>
        <div class="row">
          <label class="field">
            Username <input bind:value={serverUser} autocomplete="username" autocapitalize="off" />
          </label>
          <label class="field">
            Password <input type="password" bind:value={password} autocomplete="current-password" />
          </label>
        </div>
        <p class="soft">
          Your password is sent once; the app keeps only a token the server makes for this computer, and you can revoke it
          on the server's Account page.
        </p>
        <button class="primary" disabled={!serverUrl || !serverUser || !password}>Sign in</button>
      </form>
    {/if}

    {#if kind !== "server" || account?.username}
      <button type="button" class="add" onclick={add} disabled={kind === "server" && !pickedDb}
        >+ Add this source</button
      >
    {/if}
  </fieldset>

  {#if allowed[0] === "player"}
    <fieldset>
      <legend>Limit</legend>
      <label class="check">
        <input type="checkbox" bind:checked={limitOn} />
        Hold at most
        <input class="num" bind:value={limitText} disabled={!limitOn} inputmode="numeric" aria-label="Most games" />
        games, over all the accounts together
      </label>
      {#if limitOn}
        <label class="check">
          <input type="radio" name="sync-policy" checked={policy === "keepLatest"} onchange={() => (policy = "keepLatest")} />
          Keep only the latest: older games over the limit are marked as deleted (Compact frees the space)
        </label>
        <label class="check">
          <input type="radio" name="sync-policy" checked={policy === "stop"} onchange={() => (policy = "stop")} />
          Stop updating when the limit is hit
        </label>
      {/if}
    </fieldset>
  {/if}

  <label class="check">
    <input type="checkbox" bind:checked={auto} />
    Update on its own when the database is opened
  </label>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#snippet actions()}
    {#if onBack}<button onclick={onBack}>Back</button>{/if}
    <button onclick={onClose}>Cancel</button>
    {#if mode === "new"}
      <button class="primary" disabled={busy || sources.length === 0} onclick={create}>Create and download…</button>
    {:else}
      <button disabled={busy} onclick={() => saveOpen(false)}>Save</button>
      <button class="primary" disabled={busy || sources.length === 0} onclick={() => saveOpen(true)}
        >Save and update now</button
      >
    {/if}
  {/snippet}
</Modal>

<style>
  .intro {
    margin: 0 0 10px;
  }
  fieldset {
    border: 1px solid var(--rule);
    border-radius: 6px;
    margin: 0 0 10px;
    padding: 8px 10px;
    display: grid;
    gap: 8px;
  }
  legend {
    padding: 0 4px;
    font-weight: 600;
  }
  .soft {
    margin: 0;
    color: var(--ink-soft, inherit);
    opacity: 0.8;
    font-size: 0.92em;
  }
  .login {
    padding: 10px 12px;
    border: 1px solid var(--brass);
    border-radius: 8px;
    background: var(--brass-soft);
  }
  .num {
    width: 7em;
  }
  .add {
    justify-self: start;
    font-weight: 600;
  }
  .studies {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 160px;
    overflow: auto;
    border: 1px solid var(--rule);
    border-radius: 6px;
  }
  .studies button {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    border: 0;
    border-radius: 0;
    background: transparent;
    text-align: left;
  }
  .studies button:hover {
    background: var(--brass-soft);
  }
  .sources {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  .sources li {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    align-items: center;
  }
  .kinds,
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    align-items: end;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .field {
    display: grid;
    gap: 3px;
    flex: 1;
  }
  .signin {
    display: grid;
    gap: 8px;
  }
  .who {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
  }
  .error {
    color: var(--danger, #b3261e);
    margin: 6px 0 0;
  }
</style>
