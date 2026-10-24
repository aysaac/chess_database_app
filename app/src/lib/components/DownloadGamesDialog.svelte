<script lang="ts">
  // Download a player's games from Lichess or Chess.com (NET-01), into a
  // new database or the open one. Games the database has already are left
  // out, so downloading again adds only the new ones. The download runs as
  // a job: closing the dialog leaves it running in the status bar.
  import { onMount } from "svelte";
  import { formatCount } from "../format";
  import {
    SITE_NAMES,
    SITES,
    dbName,
    lichessTime,
    sentence,
    siteSpeeds,
    speedLabel,
  } from "../online";
  import { db } from "../stores/db.svelte";
  import { baseName, folderName } from "../stores/onboarding.svelte";
  import { online } from "../stores/online.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let username: HTMLInputElement;

  onMount(() => {
    online.prepare();
    // After the Modal's own focus (its first field is a site's radio button).
    username.focus();
  });

  const siteName = $derived(SITE_NAMES[online.site]);
  const job = $derived(online.progress);
  const percent = $derived(
    job && job.total > 0 ? Math.min(100, Math.round((job.done / job.total) * 100)) : 0,
  );
  const newName = $derived(
    online.target
      ? baseName(online.target).replace(/\.chessdb$/i, "")
      : dbName(online.username, online.site),
  );
  const locked = $derived(online.busy);

  function submit(e: SubmitEvent) {
    e.preventDefault();
    void online.start();
  }
</script>

<Modal title="Download games from Lichess or Chess.com" {onClose} width="560px">
  <!-- The store checks the fields: WebKit's own checks stop the submit at an
       empty date field (it shows today's date as a hint) and focus it. -->
  <form id="download-games" novalidate onsubmit={submit}>
    <p class="intro">
      A player's games, with their clock times{online.site === "lichess"
        ? ", and evals where Lichess analysed them"
        : ""}. Games the database has already are left out, so downloading again adds only the new
      ones.
    </p>

    <fieldset disabled={locked}>
      <legend>Player</legend>
      <div class="sites" role="radiogroup" aria-label="Site">
        {#each SITES as s (s)}
          <label class="site" class:on={online.site === s}>
            <input
              type="radio"
              name="online-site"
              checked={online.site === s}
              onchange={() => (online.site = s)}
            />
            {SITE_NAMES[s]}
          </label>
        {/each}
      </div>
      <label class="field">
        Username on {siteName}
        <input
          bind:this={username}
          bind:value={online.username}
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
        />
      </label>
    </fieldset>

    <fieldset disabled={locked}>
      <legend>Games</legend>
      <div class="speeds" role="group" aria-label="Time controls">
        {#each siteSpeeds(online.site) as s (s)}
          <label class="check">
            <input
              type="checkbox"
              checked={online.speeds.includes(s)}
              onchange={(e) => online.toggleSpeed(s, e.currentTarget.checked)}
            />
            {speedLabel(online.site, s)}
          </label>
        {/each}
      </div>
      <label class="check"
        ><input type="checkbox" bind:checked={online.ratedOnly} /> Rated games only</label
      >
      <div class="periods">
        <label class="check">
          <input
            type="radio"
            name="online-period"
            checked={online.period === "all"}
            onchange={() => (online.period = "all")}
          />
          All of them
        </label>
        {#if online.into === "open"}
          <label class="check">
            <input
              type="radio"
              name="online-period"
              checked={online.period === "newOnly"}
              onchange={() => (online.period = "newOnly")}
            />
            New ones only: from the newest of the player's games in {db.info?.name ??
              "the database"}
          </label>
        {/if}
        <label class="check">
          <input
            type="radio"
            name="online-period"
            checked={online.period === "since"}
            onchange={() => (online.period = "since")}
          />
          From
          <input
            class="date"
            type="date"
            aria-label="First day"
            bind:value={online.since}
            oninput={() => (online.period = "since")}
          />
          on
        </label>
      </div>
    </fieldset>

    <fieldset disabled={locked}>
      <legend>Into</legend>
      <label class="choice" class:on={online.into === "new"}>
        <input
          type="radio"
          name="online-into"
          checked={online.into === "new"}
          onchange={() => online.setInto("new")}
        />
        <span>
          A new database: <strong>{newName}</strong>
          <span class="soft folder" title={online.target || undefined}
            >in {online.target ? folderName(online.target) : "Documents"}</span
          >
        </span>
        {#if online.into === "new"}
          <button type="button" class="plain" onclick={() => online.chooseTarget()}>Change…</button>
        {/if}
      </label>
      <label class="choice" class:on={online.into === "open"} class:off={!db.info}>
        <input
          type="radio"
          name="online-into"
          checked={online.into === "open"}
          disabled={!db.info}
          onchange={() => online.setInto("open")}
        />
        <span>
          {#if db.info}
            The open database: <strong>{db.info.name}</strong>
            <span class="soft"
              >({formatCount(db.info.games)} {db.info.games === 1 ? "game" : "games"})</span
            >
          {:else}
            The open database <span class="soft">(none is open)</span>
          {/if}
        </span>
      </label>
    </fieldset>
    <button type="submit" hidden aria-hidden="true"></button>
  </form>

  <div
    class="status"
    class:empty={!online.busy && !online.result && !online.error}
    aria-live="polite"
  >
    {#if online.busy && online.job === null}
      <p class="soft">Looking up {online.username.trim()} on {siteName}…</p>
    {:else if online.busy}
      <div class="progress">
        <progress max="100" value={percent}></progress>
        <span>{percent}%</span>
      </div>
      <p class="soft">
        {#if online.site === "lichess" && online.player?.games && online.period === "all"}
          {online.player.username} has played {formatCount(online.player.games)} games on Lichess, all
          kinds. Lichess sends about 20 a second: all of them take {lichessTime(
            online.player.games,
          )}.
        {:else if online.site === "lichess"}
          Lichess sends about 20 games a second, so a few thousand take a few minutes.
        {:else}
          Chess.com sends the games a month at a time.
        {/if}
        The games appear in the list as they come, and the download goes on if you close this window.
      </p>
    {:else if online.result}
      {@const r = online.result}
      {#if r.state === "done"}
        <p class="done" role="status">
          {r.message}.{#if r.name}{" "}{r.name} has {formatCount(r.games)}
            {r.games === 1 ? "game" : "games"} now.{/if}
        </p>
      {:else if r.state === "cancelled"}
        <p class="done" role="status">
          {r.message}{#if r.name}{" "}The games downloaded before stay in {r.name} ({formatCount(
              r.games,
            )}
            {r.games === 1 ? "game" : "games"}).{/if}
        </p>
      {:else}
        <p class="error" role="alert">{sentence(r.message)}</p>
      {/if}
    {/if}
    {#if online.error}<p class="error" role="alert">{sentence(online.error)}</p>{/if}
  </div>

  {#snippet actions()}
    {#if online.busy}
      <button disabled={online.job === null} onclick={() => online.cancel()}>Stop download</button>
      <button onclick={onClose}>Close</button>
    {:else}
      <button onclick={onClose}>Close</button>
      <button class="primary" type="submit" form="download-games">Download</button>
    {/if}
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 10px;
  }

  .intro {
    color: var(--ink);
  }

  fieldset {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0 0 14px;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 6px;
    padding: 0;
    color: var(--ink-soft);
    font-size: 13px;
    font-weight: 600;
  }

  fieldset:disabled {
    opacity: 0.6;
  }

  .sites {
    display: flex;
    gap: 6px;
  }

  .site {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px 4px 8px;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    cursor: pointer;
  }

  .site.on {
    border-color: var(--felt);
    background: var(--felt-soft);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  .field input {
    color: var(--ink);
    font-size: 14px;
  }

  .speeds {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .periods {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 2px;
  }

  .date {
    font-size: 13px;
  }

  .choice {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: var(--radius);
    cursor: pointer;
  }

  .choice.on {
    background: var(--felt-soft);
  }

  .choice.off {
    cursor: default;
    opacity: 0.6;
  }

  .choice > span {
    flex: 1;
    min-width: 0;
  }

  .folder {
    overflow-wrap: anywhere;
  }

  .choice button {
    flex: none;
  }

  .soft {
    color: var(--ink-soft);
    font-size: 13px;
  }

  .status {
    padding-top: 10px;
    border-top: 1px solid var(--rule);
  }

  .status.empty {
    display: none;
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }

  progress {
    flex: 1;
    height: 6px;
    accent-color: var(--felt);
  }

  .done {
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--brass-soft);
  }

  .error {
    color: var(--danger);
  }
</style>
