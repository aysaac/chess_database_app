<script lang="ts">
  // The Sync button (NET-13): the open database kept in step with a Lichess
  // study. Linked, opening the dialog looks at both sides at once and shows
  // what a sync would do ("3 new games here to send, 1 changed on
  // Lichess"); one confirm does it, as a job (closing the dialog leaves it
  // running). Not linked, it links the database first: to a new private
  // study, or to one of the user's (the one a static copy came from is
  // offered), following it only or both ways. Games both sides changed in
  // ways that don't merge are never overwritten: they wait here, each with
  // Keep this database's, Keep Lichess's or Keep both.
  import { onMount, untrack } from "svelte";
  import { api, errorMessage } from "../api";
  import type { StudyInfoDto } from "../bindings/StudyInfoDto";
  import type { StudyVisibilityDto } from "../bindings/StudyVisibilityDto";
  import type { SyncChoiceDto } from "../bindings/SyncChoiceDto";
  import type { SyncConflictDto } from "../bindings/SyncConflictDto";
  import type { SyncModeDto } from "../bindings/SyncModeDto";
  import type { SyncPreviewDto } from "../bindings/SyncPreviewDto";
  import type { SyncReportDto } from "../bindings/SyncReportDto";
  import type { SyncTargetDto } from "../bindings/SyncTargetDto";
  import { sentence } from "../online";
  import { AutoListing, MAX_STUDY_NAME, studyNameProblem } from "../studies";
  import { countOf, pendingText, stepLabel, summary } from "../studySync";
  import { db } from "../stores/db.svelte";
  import { jobs } from "../stores/jobs.svelte";
  import { lichess } from "../stores/lichess.svelte";
  import { studySync } from "../stores/studySync.svelte";
  import LichessAccount from "./LichessAccount.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // Copied once: the dialog is about the database open when it opened.
  const handle = db.info?.handle ?? null;
  const dbName = db.info?.name ?? "Games";

  type Phase = "choose" | "planning" | "preview" | "applying" | "done";
  let phase = $state<Phase>("choose");
  let job = $state<number | null>(null);
  let preview = $state<SyncPreviewDto | null>(null);
  let report = $state<SyncReportDto | null>(null);
  let outcome = $state<{ ok: boolean; message: string } | null>(null);
  let error = $state("");
  let deleteChapters = $state(false);

  // Linking a database that isn't.
  let into = $state<"new" | "existing">("new");
  let name = $state(dbName);
  let visibility = $state<StudyVisibilityDto>("private");
  let mode = $state<SyncModeDto>("twoWay");
  let study = $state("");
  let mine = $state<StudyInfoDto[] | null>(null);
  let listing = $state(false);
  let listError = $state("");
  const auto = new AutoListing();

  // A game waiting for the user, shown in place of the rest.
  let resolving = $state<SyncConflictDto | null>(null);
  let resolveBusy = $state(false);
  let resolveError = $state("");
  let unlinking = $state(false);

  const linked = $derived(studySync.linked);
  const pending = $derived(linked?.pending ?? []);
  const progress = $derived(job === null ? undefined : jobs.running.find((j) => j.id === job));
  const percent = $derived(
    progress && progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0,
  );

  onMount(() => {
    void lichess.load();
    void start();
  });

  async function start() {
    await studySync.load(handle);
    const l = studySync.linked;
    if (l) {
      void look({ kind: "study", study: l.study, mode: l.mode });
    } else if (studySync.status?.copiedFrom) {
      into = "existing";
      study = studySync.status.copiedFrom.link;
    }
  }

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

  // The user's studies, once per login while "One of your studies" shows
  // (see AutoListing: a failed listing waits for Try again).
  $effect(() => {
    const user = lichess.username;
    const shown = phase === "choose" && !linked && into === "existing";
    untrack(() => {
      if (auto.take(user, shown)) void listMine();
    });
  });

  /** The target the linking form describes, or why it can't be one. */
  function formTarget(): SyncTargetDto | string {
    if (into === "new") {
      const n = name.trim();
      return studyNameProblem(n) ?? { kind: "new", name: n, visibility, mode };
    }
    if (!study.trim()) return "Pick one of your studies, or paste its link.";
    return { kind: "study", study: study.trim(), mode };
  }

  /** Looks at both sides: what a sync with `target` would do. */
  async function look(target: SyncTargetDto) {
    if (handle === null || phase === "planning" || phase === "applying") return;
    phase = "planning";
    error = "";
    preview = null;
    report = null;
    outcome = null;
    deleteChapters = false;
    try {
      const id = await api.studySyncPlan(handle, target);
      job = id;
      const done = await jobs.waitFor(id);
      if (done.state === "done") {
        preview = await api.studySyncPreview(handle);
        phase = preview ? "preview" : "choose";
      } else {
        error = done.state === "cancelled" ? "Stopped." : (done.error ?? "");
        phase = "choose";
        void lichess.load(true);
      }
    } catch (e) {
      error = errorMessage(e);
      void lichess.checkError(e);
      phase = "choose";
    } finally {
      job = null;
    }
  }

  function lookFromForm() {
    const t = formTarget();
    if (typeof t === "string") {
      error = t;
      return;
    }
    void look(t);
  }

  async function confirm() {
    if (handle === null || !preview) return;
    phase = "applying";
    error = "";
    try {
      const id = await api.studySyncApply(handle, deleteChapters);
      job = id;
      const done = await jobs.waitFor(id);
      report = await api.studySyncReport(handle);
      outcome = {
        ok: done.state === "done",
        message:
          done.state === "failed"
            ? (done.error ?? "")
            : done.state === "cancelled"
              ? "Stopped. What was done before stays done; Sync again does the rest."
              : done.message,
      };
      if (done.state === "failed") void lichess.load(true);
    } catch (e) {
      error = errorMessage(e);
      void lichess.checkError(e);
    } finally {
      job = null;
      phase = "done";
      await Promise.all([studySync.reload(), db.refresh()]);
    }
  }

  async function setMode(next: SyncModeDto) {
    const l = linked;
    if (handle === null || !l || l.mode === next) return;
    try {
      studySync.status = await api.studySyncSetMode(handle, l.study, next);
      void look({ kind: "study", study: l.study, mode: next });
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function unlink() {
    const l = linked;
    if (handle === null || !l) return;
    try {
      studySync.status = await api.studySyncUnlink(handle, l.study);
      unlinking = false;
      preview = null;
      outcome = null;
      phase = "choose";
      into = "existing";
      study = `https://lichess.org/study/${l.study}`;
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function openConflict(uid: string) {
    if (handle === null) return;
    resolveError = "";
    try {
      resolving = await api.studySyncConflict(handle, uid);
      if (!resolving) await studySync.reload();
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function resolve(choice: SyncChoiceDto) {
    const r = resolving;
    if (handle === null || !r || resolveBusy) return;
    resolveBusy = true;
    resolveError = "";
    try {
      const id = await api.studySyncResolve(handle, r.uid, choice);
      job = id;
      const done = await jobs.waitFor(id);
      if (done.state === "done") {
        resolving = null;
        await Promise.all([studySync.reload(), db.refresh()]);
        // What's left to sync, with what the answer did.
        const l = studySync.linked;
        if (l && phase !== "applying") await look({ kind: "study", study: l.study, mode: l.mode });
        outcome = { ok: true, message: done.message };
      } else {
        resolveError = done.state === "cancelled" ? "Stopped." : (done.error ?? "");
        // It may have learnt Lichess's newer version: show it.
        resolving = await api.studySyncConflict(handle, r.uid);
      }
    } catch (e) {
      resolveError = errorMessage(e);
      void lichess.checkError(e);
    } finally {
      job = null;
      resolveBusy = false;
    }
  }

  const modeWords = (m: SyncModeDto) => (m === "twoWay" ? "both ways" : "following Lichess only");
  const MODES: { value: SyncModeDto; label: string; hint: string }[] = [
    { value: "twoWay", label: "Both ways", hint: "games added or changed here go to the study, and its changes come here" },
    { value: "follow", label: "Follow only", hint: "the study's changes come here; nothing goes to Lichess" },
  ];
  const VISIBILITIES: { value: StudyVisibilityDto; label: string }[] = [
    { value: "private", label: "Private" },
    { value: "unlisted", label: "Unlisted" },
    { value: "public", label: "Public" },
  ];
  const listed = $derived(preview?.items.filter((i) => i.step !== "unchanged") ?? []);
  const deletedHere = $derived(preview ? countOf(preview.counts, "deletedHere") : 0);
</script>

<Modal title={linked ? "Sync with a Lichess study" : "Link to a Lichess study"} {onClose} width="640px">
  <LichessAccount
    why="Sending games to a study, and reading private ones, needs your Lichess login. A public study can be followed without it."
  />

  {#if resolving}
    {@const r = resolving}
    <section class="resolve" aria-label="A game that waits for you">
      <h3>{r.name}</h3>
      <p>{pendingText(r.kind)} <span class="soft">Found {r.found}.</span></p>
      {#if r.questions.length}
        <table class="questions">
          <thead><tr><th>Where</th><th>Here</th><th>On Lichess</th></tr></thead>
          <tbody>
            {#each r.questions as q, i (i)}
              <tr><td>{q.what}</td><td>{q.mine || "—"}</td><td>{q.theirs || "—"}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <dl class="sides">
        <dt>Here</dt>
        <dd>
          <span class="soft">{r.mine.moves} moves, {r.mine.comments} comments:</span>
          <span class="line">{r.mine.line || "no moves"}</span>
        </dd>
        <dt>On Lichess</dt>
        <dd>
          {#if r.theirs}
            <span class="soft">{r.theirs.moves} moves, {r.theirs.comments} comments:</span>
            <span class="line">{r.theirs.line || "no moves"}</span>
          {:else}
            <span class="soft">removed from the study</span>
          {/if}
          <button class="link" onclick={() => lichess.open(r.url)}>Open on Lichess</button>
        </dd>
      </dl>
      <div class="choices">
        <button disabled={resolveBusy} onclick={() => resolve("mine")}>
          {r.kind === "removedThere"
            ? r.twoWay
              ? "Send it to the study again"
              : "Keep it here, unlinked"
            : r.twoWay
              ? "Keep this database's"
              : "Keep this database's here"}
        </button>
        <button disabled={resolveBusy} onclick={() => resolve("theirs")}>
          {r.kind === "removedThere" ? "Mark it as deleted here" : "Keep Lichess's"}
        </button>
        <button disabled={resolveBusy} onclick={() => resolve("both")}>Keep both</button>
      </div>
      <p class="soft">
        {#if r.kind === "removedThere"}
          Keep both keeps the game here, unlinked and marked as a copy.
        {:else if r.twoWay}
          This database's goes to Lichess in place of the chapter; Lichess's comes here in place of
          this game (evals and what only this database holds stay); Keep both takes Lichess's here
          and keeps this database's as a new game, marked as a copy, which no sync sends.
        {:else}
          Following only, nothing goes to Lichess: this database's version stays here, or
          Lichess's takes its place, or Keep both keeps both here.
        {/if}
      </p>
      {#if resolveBusy}<p class="soft" aria-live="polite">Working…</p>{/if}
      {#if resolveError}<p class="error" role="alert">{sentence(resolveError)}</p>{/if}
    </section>
  {:else}
    {#if linked}
      <section class="linked" aria-label="The linked study">
        <p>
          Linked to <strong>{linked.name ?? linked.study}</strong>
          <span class="soft">({linked.games} {linked.games === 1 ? "game" : "games"}, {modeWords(linked.mode)})</span>
          <button class="link" onclick={() => lichess.open(linked.url)}>Open on Lichess</button>
        </p>
        <div class="row">
          <div class="modes" role="radiogroup" aria-label="How the database follows the study">
            {#each MODES as m (m.value)}
              <label class="check" title={m.hint}>
                <input
                  type="radio"
                  name="sync-mode"
                  checked={linked.mode === m.value}
                  disabled={phase === "planning" || phase === "applying"}
                  onchange={() => setMode(m.value)}
                />
                {m.label}
              </label>
            {/each}
          </div>
          {#if unlinking}
            <span class="confirm">
              <span class="soft">The games stay here as they are; nothing changes on Lichess.</span>
              <button onclick={unlink}>Unlink</button>
              <button onclick={() => (unlinking = false)}>Keep the link</button>
            </span>
          {:else}
            <button disabled={phase === "planning" || phase === "applying"} onclick={() => (unlinking = true)}
              >Unlink…</button
            >
          {/if}
        </div>
        {#if pending.length}
          <div class="pending" role="group" aria-label="Games that wait for you">
            <p>
              <strong>{pending.length} {pending.length === 1 ? "game waits" : "games wait"} for you:</strong>
              both sides changed, and the sync doesn't overwrite either.
            </p>
            <ul>
              {#each pending as p (p.uid)}
                <li>
                  <span class="name">{p.name}</span>
                  <button onclick={() => openConflict(p.uid)}>Resolve…</button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </section>
    {:else if phase === "choose" || phase === "planning"}
      <p class="intro">
        Link <strong>{dbName}</strong> to a study: its games become the study's chapters, and Sync keeps
        them in step. Evals, clock details and anything else Lichess can't hold stay here.
      </p>
      <fieldset disabled={phase !== "choose"}>
        <legend>The study</legend>
        <label class="choice" class:on={into === "new"}>
          <input type="radio" name="sync-into" checked={into === "new"} onchange={() => (into = "new")} />
          <span>A new study for these games</span>
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
                    name="sync-visibility"
                    checked={visibility === v.value}
                    onchange={() => (visibility = v.value)}
                  />
                  {v.label}
                </label>
              {/each}
            </div>
          </div>
        {/if}
        <label class="choice" class:on={into === "existing"}>
          <input
            type="radio"
            name="sync-into"
            checked={into === "existing"}
            onchange={() => (into = "existing")}
          />
          <span
            >One of your studies <span class="soft">(its chapters and these games are matched up first)</span
            ></span
          >
        </label>
        {#if into === "existing"}
          <div class="details">
            {#if studySync.status?.copiedFrom}
              {@const c = studySync.status.copiedFrom}
              <p class="soft">
                These games came from <strong>{c.name ?? c.study}</strong>: linking to it keeps them in step
                with it.
              </p>
            {/if}
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
            {/if}
            <label class="field">
              Or its link
              <input bind:value={study} placeholder="https://lichess.org/study/…" autocomplete="off" spellcheck="false" />
            </label>
          </div>
        {/if}
      </fieldset>
      <fieldset disabled={phase !== "choose"}>
        <legend>Keep them in step</legend>
        {#each MODES as m (m.value)}
          <label class="check">
            <input type="radio" name="sync-link-mode" checked={mode === m.value} onchange={() => (mode = m.value)} />
            {m.label} <span class="soft">({m.hint})</span>
          </label>
        {/each}
      </fieldset>
    {/if}

    <div class="status" aria-live="polite">
      {#if phase === "planning"}
        <p class="soft">Looking at the study on Lichess and at the games here…</p>
      {:else if phase === "preview" && preview}
        <div class="preview">
          <p class="summary">
            <strong>{sentence(summary(preview.counts))}</strong>
            {#if !preview.study}
              <span class="soft"
                >They go to {preview.name}, a new {visibility} study on Lichess, {modeWords(preview.mode)}.</span
              >
            {/if}
          </p>
          {#if listed.length}
            <ul class="items" aria-label="What the sync does">
              {#each listed as i, n (n)}
                <li class={i.step}>
                  <span class="name">{i.name}</span>
                  <span class="what">{stepLabel(i.step)}{i.note ? `: ${i.note}` : ""}</span>
                </li>
              {/each}
            </ul>
          {/if}
          {#if deletedHere > 0 && preview.mode === "twoWay"}
            <label class="check">
              <input type="checkbox" bind:checked={deleteChapters} />
              Also remove the {deletedHere === 1 ? "chapter" : `${deletedHere} chapters`} of games deleted here
              from the study
            </label>
          {/if}
          {#if countOf(preview.counts, "noRoom") > 0}
            <p class="soft">A study holds 64 chapters: the games that don't fit stay here.</p>
          {/if}
          {#if preview.unlisted > 0}
            <p class="soft">
              {preview.unlisted.toLocaleString("en-US")} other games here aren't in the study (following only,
              they stay here).
            </p>
          {/if}
        </div>
      {:else if phase === "applying"}
        <div class="progress">
          <progress max="100" value={percent}></progress><span>{percent}%</span>
        </div>
        <p class="soft">Syncing; closing this window leaves it going.</p>
      {/if}
      {#if outcome}
        <p class={outcome.ok ? "done" : "error"} role={outcome.ok ? "status" : "alert"}>
          {sentence(outcome.message)}
        </p>
      {/if}
      {#if report?.skipped.length}
        <ul class="items" aria-label="Games left as they were">
          {#each report.skipped as s, n (n)}
            <li><span class="name">{s.name}</span><span class="what">{s.note}</span></li>
          {/each}
        </ul>
      {/if}
      {#if error}<p class="error" role="alert">{sentence(error)}</p>{/if}
    </div>
  {/if}

  {#snippet actions()}
    {#if resolving}
      <button disabled={resolveBusy} onclick={() => (resolving = null)}>Back</button>
    {:else if phase === "planning" || phase === "applying"}
      {#if job !== null}
        <button onclick={() => job !== null && jobs.cancel(job)}>Stop</button>
      {/if}
      <button onclick={onClose}>Close</button>
    {:else if phase === "preview" && preview}
      <button onclick={onClose}>{preview.nothing ? "Close" : "Cancel"}</button>
      {#if !preview.nothing}
        <button class="primary" onclick={confirm}>{linked ? "Sync now" : "Link and sync"}</button>
      {/if}
    {:else if phase === "done"}
      <button onclick={onClose}>Close</button>
      {#if linked}
        <button onclick={() => linked && look({ kind: "study", study: linked.study, mode: linked.mode })}
          >Look again</button
        >
      {/if}
    {:else}
      <button onclick={onClose}>Close</button>
      {#if linked}
        <button class="primary" onclick={() => linked && look({ kind: "study", study: linked.study, mode: linked.mode })}
          >Look again</button
        >
      {:else}
        <button class="primary" disabled={handle === null} onclick={lookFromForm}>Show what it does</button>
      {/if}
    {/if}
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 8px;
  }

  h3 {
    margin: 0 0 6px;
    font-size: 15px;
  }

  .intro {
    margin-bottom: 12px;
  }

  .linked {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 12px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex-wrap: wrap;
  }

  .modes {
    display: flex;
    gap: 14px;
  }

  .confirm {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .pending {
    padding: 8px 10px;
    border: 1px solid var(--brass);
    border-radius: var(--radius);
    background: var(--brass-soft);
  }

  .pending ul,
  .items,
  .studies {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .pending li,
  .items li {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    padding: 2px 0;
  }

  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .items {
    max-height: 220px;
    margin-bottom: 8px;
    padding: 4px 8px;
    overflow-y: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  .items .what {
    flex: none;
    max-width: 60%;
    color: var(--ink-soft);
    font-size: 13px;
    text-align: right;
  }

  .items .conflict .what,
  .items .pending .what {
    color: var(--danger);
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
    gap: 14px;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .studies {
    max-height: 140px;
    overflow-y: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
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

  .list-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .status {
    padding-top: 10px;
    border-top: 1px solid var(--rule);
  }

  .summary {
    display: flex;
    flex-direction: column;
    gap: 2px;
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

  .resolve {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .questions {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  .questions th,
  .questions td {
    padding: 3px 6px;
    border-bottom: 1px solid var(--rule);
    text-align: left;
    vertical-align: top;
  }

  .questions th {
    color: var(--ink-soft);
    font-weight: 600;
  }

  .sides {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 10px;
    margin: 0;
  }

  .sides dt {
    color: var(--ink-soft);
    font-weight: 600;
  }

  .sides dd {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    margin: 0;
  }

  .sides .line {
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
    font-family: var(--font-book);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .choices {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--felt);
    text-decoration: underline;
  }

  .link:hover:not(:disabled) {
    background: none;
  }

  .soft {
    color: var(--ink-soft);
    font-size: 13px;
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
