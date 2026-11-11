<script lang="ts">
  // Send games to Lichess (NET-12): the selected games, or the list's (its
  // filter, or all of them), each as a chapter of a study of the user's:
  // a new one (private unless they pick otherwise; several of 64 chapters
  // each when there are more games, "Name (1/3)"…), or one of theirs that
  // has room. Lichess makes 30 new studies a day per account: past that the
  // export stops and says which games went where. The games in the
  // database don't change. The export is a job: closing the dialog leaves
  // it running.
  import { onMount, untrack } from "svelte";
  import { api, errorMessage } from "../api";
  import type { StudyExportReportDto } from "../bindings/StudyExportReportDto";
  import type { StudyInfoDto } from "../bindings/StudyInfoDto";
  import type { StudyVisibilityDto } from "../bindings/StudyVisibilityDto";
  import { formatCount } from "../format";
  import { sentence } from "../online";
  import type { Ranges } from "../selection";
  import { count } from "../selection";
  import { AutoListing, MAX_STUDY_NAME, studyNameProblem } from "../studies";
  import { db } from "../stores/db.svelte";
  import { jobs } from "../stores/jobs.svelte";
  import { lichess } from "../stores/lichess.svelte";
  import LichessAccount from "./LichessAccount.svelte";
  import Modal from "./Modal.svelte";

  /** The rows to send; null: the list's games. */
  let { ranges, onClose }: { ranges: Ranges | null; onClose: () => void } = $props();

  // Copied once: the props are live, and the list's selection can change.
  const rows = untrack(() => ranges)?.map(([a, b]) => [a, b] as [number, number]) ?? null;
  const selected = rows ? count(rows) : 0;
  const listed = db.info ? (db.info.filter ? db.info.filter.count : db.info.games) : 0;
  const games = rows ? selected : listed;
  const what = rows
    ? `The ${selected === 1 ? "selected game" : `${formatCount(selected)} selected games`}`
    : db.info?.filter
      ? `The ${formatCount(listed)} ${listed === 1 ? "game" : "games"} of the list's filter`
      : `All ${formatCount(listed)} ${listed === 1 ? "game" : "games"} of ${db.info?.name ?? "the database"}`;

  const VISIBILITIES: { value: StudyVisibilityDto; label: string; hint: string }[] = [
    { value: "private", label: "Private", hint: "only you, and members you invite" },
    { value: "unlisted", label: "Unlisted", hint: "anyone with its link" },
    { value: "public", label: "Public", hint: "anyone, and it's listed on Lichess" },
  ];

  let into = $state<"new" | "existing">("new");
  let name = $state(db.info?.filter ? `${db.info.name} (filtered)` : (db.info?.name ?? "Games"));
  let visibility = $state<StudyVisibilityDto>("private");
  let mine = $state<StudyInfoDto[] | null>(null);
  let listing = $state(false);
  /** Why listing the user's studies failed; "Try again" asks again. */
  let listError = $state("");
  const auto = new AutoListing();
  let study = $state("");
  let busy = $state(false);
  let job = $state<number | null>(null);
  let report = $state<StudyExportReportDto | null>(null);
  let outcome = $state<{ ok: boolean; message: string } | null>(null);
  let error = $state("");

  onMount(() => {
    void lichess.load();
  });

  const studies = $derived(Math.max(1, Math.ceil(games / 64)));
  const progress = $derived(job === null ? undefined : jobs.running.find((j) => j.id === job));
  const percent = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.done / progress.total) * 100))
      : 0,
  );

  async function listMine() {
    const user = lichess.username;
    if (!user || listing) return;
    listing = true;
    listError = "";
    mine = null;
    try {
      mine = await api.lichessStudies(user);
    } catch (e) {
      listError = errorMessage(e);
      void lichess.checkError(e);
    } finally {
      listing = false;
    }
  }

  // The user's studies, once per login while "One of your studies" shows.
  // Only the login and the choice are read here (the listing itself is
  // untracked): a failed listing waits for "Try again", never a loop of
  // requests while Lichess is down or limits the app.
  $effect(() => {
    const user = lichess.username;
    const shown = into === "existing";
    untrack(() => {
      if (auto.take(user, shown)) void listMine();
    });
  });

  async function send() {
    const info = db.info;
    if (busy || !info) return;
    if (!lichess.username) {
      error = "Log in to Lichess first: the games go to studies of yours.";
      return;
    }
    const n = name.trim();
    const problem = into === "new" ? studyNameProblem(n) : null;
    if (problem) {
      error = problem;
      return;
    }
    if (into === "existing" && !study.trim()) {
      error = "Pick one of your studies, or paste its link.";
      return;
    }
    busy = true;
    error = "";
    report = null;
    outcome = null;
    try {
      const target =
        into === "new"
          ? { kind: "new" as const, name: n, visibility }
          : { kind: "existing" as const, study: study.trim() };
      let id: number;
      try {
        id = await api.sendToStudy(info.handle, db.selectionOf(rows), { ...db.sort }, target);
      } catch (e) {
        error = errorMessage(e);
        void lichess.checkError(e);
        return;
      }
      job = id;
      const done = await jobs.waitFor(id);
      report = await api.studyExportReport();
      const state = done.state === "running" ? "done" : done.state;
      outcome = {
        ok: state !== "failed",
        message:
          state === "failed"
            ? (done.error ?? "")
            : state === "cancelled"
              ? `Stopped. ${report ? `${formatCount(report.sent)} of the ${formatCount(report.games)} games went to Lichess before.` : ""}`
              : done.message,
      };
      if (state === "failed") void lichess.load(true);
    } finally {
      busy = false;
      job = null;
    }
  }
</script>

<Modal title="Send games to a Lichess study" {onClose} width="600px">
  <LichessAccount
    why="The games go to studies in your Lichess account, so the app needs your login."
  />

  <p class="games">
    <strong>{what}</strong>, each as a chapter. The games in the database don't change.
  </p>

  <fieldset disabled={busy || !lichess.username}>
    <legend>To</legend>
    <label class="choice" class:on={into === "new"}>
      <input
        type="radio"
        name="study-to"
        checked={into === "new"}
        onchange={() => (into = "new")}
      />
      <span>A new study</span>
    </label>
    {#if into === "new"}
      <div class="details">
        <label class="field">
          Name
          <input bind:value={name} maxlength={MAX_STUDY_NAME} autocomplete="off" />
        </label>
        <div class="visibility" role="radiogroup" aria-label="Who can see it">
          {#each VISIBILITIES as v (v.value)}
            <label class="check">
              <input
                type="radio"
                name="study-visibility"
                checked={visibility === v.value}
                onchange={() => (visibility = v.value)}
              />
              {v.label} <span class="soft">({v.hint})</span>
            </label>
          {/each}
        </div>
        {#if studies > 1}
          <p class="soft">
            A study holds 64 chapters, so {formatCount(games)} games make {formatCount(studies)} studies:
            “{name.trim()}
            (1/{studies})” and on. Lichess makes 30 new studies a day per account{studies > 30
              ? ": send at most 1,920 games at a time"
              : ""}.
          </p>
        {/if}
      </div>
    {/if}
    <label class="choice" class:on={into === "existing"}>
      <input
        type="radio"
        name="study-to"
        checked={into === "existing"}
        onchange={() => (into = "existing")}
      />
      <span
        >One of your studies <span class="soft">(it must have room: a study holds 64 chapters)</span
        ></span
      >
    </label>
    {#if into === "existing"}
      <div class="details">
        {#if listing}
          <p class="soft">Listing your studies…</p>
        {:else if listError}
          <p class="list-error" role="alert">
            <span class="error">{sentence(listError)}</span>
            <button type="button" onclick={listMine}>Try again</button>
          </p>
        {:else if mine?.length}
          <ul class="studies" aria-label="Your studies">
            {#each mine as s (s.id)}
              <li>
                <button class="study" class:on={study === s.link} onclick={() => (study = s.link)}>
                  <span class="name">{s.name}</span><span class="soft">{s.updated}</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if mine}
          <p class="soft">You have no studies yet: send the games to a new one.</p>
        {/if}
        <label class="field">
          Or its link
          <input
            bind:value={study}
            placeholder="https://lichess.org/study/…"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </div>
    {/if}
  </fieldset>

  <div class="status" class:empty={!busy && !outcome && !error} aria-live="polite">
    {#if busy}
      <div class="progress">
        <progress max="100" value={percent}></progress><span>{percent}%</span>
      </div>
      <p class="soft">The games go one study at a time; closing this window leaves it going.</p>
    {/if}
    {#if outcome}
      <p class={outcome.ok ? "done" : "error"} role={outcome.ok ? "status" : "alert"}>
        {sentence(outcome.message)}
      </p>
    {/if}
    {#if report?.studies.length}
      <ul class="sent" aria-label="Studies the games went to">
        {#each report.studies as s (s.url)}
          <li>
            <span
              >{s.name}: {formatCount(s.sent)}
              {s.sent === 1 ? "game" : "games"}{s.made ? " (new)" : ""}</span
            >
            <button onclick={() => lichess.open(s.url)}>Open on Lichess</button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if error}<p class="error" role="alert">{sentence(error)}</p>{/if}
  </div>

  {#snippet actions()}
    {#if busy && job !== null}
      <button onclick={() => job !== null && jobs.cancel(job)}>Stop sending</button>
      <button onclick={onClose}>Close</button>
    {:else}
      <button onclick={onClose}>Close</button>
      <button class="primary" disabled={busy || !lichess.username || games === 0} onclick={send}>
        Send {formatCount(games)}
        {games === 1 ? "game" : "games"}
      </button>
    {/if}
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 8px;
  }

  .games {
    margin-bottom: 12px;
  }

  fieldset {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0 0 12px;
    padding: 0;
    border: 0;
  }

  fieldset:disabled {
    opacity: 0.6;
  }

  legend {
    margin-bottom: 4px;
    padding: 0;
    color: var(--ink-soft);
    font-size: 13px;
    font-weight: 600;
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

  .details {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 2px 0 6px 30px;
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

  .visibility {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .studies {
    max-height: 160px;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    list-style: none;
  }

  .study {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    padding: 4px 8px;
    border: 0;
    border-radius: 0;
    text-align: left;
  }

  .study.on {
    background: var(--felt-soft);
  }

  .study .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .study .soft {
    flex: none;
  }

  .sent {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0 0 8px;
    padding: 0;
    list-style: none;
  }

  .sent li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .sent button {
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

  .list-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .list-error button {
    flex: none;
  }
</style>
