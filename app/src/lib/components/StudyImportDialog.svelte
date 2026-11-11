<script lang="ts">
  // Import a Lichess study (IO-10): paste a study's or a chapter's link, or
  // pick one of a user's studies (their private and unlisted ones too when
  // it's the user logged in). Looking it up shows its chapters; importing
  // puts them into a new database named after the study, or into the open
  // one. Chapters the database has already (told by their ChapterURL) are
  // left out, so importing a study again adds only its new chapters. The
  // import is a job: closing the dialog leaves it running.
  import { save } from "@tauri-apps/plugin-dialog";
  import { onMount, untrack } from "svelte";
  import { api, errorMessage, isAppError } from "../api";
  import type { StudyInfoDto } from "../bindings/StudyInfoDto";
  import type { StudyPreviewDto } from "../bindings/StudyPreviewDto";
  import { formatCount } from "../format";
  import { sentence } from "../online";
  import { db } from "../stores/db.svelte";
  import { jobs } from "../stores/jobs.svelte";
  import { lichess } from "../stores/lichess.svelte";
  import { baseName, folderName } from "../stores/onboarding.svelte";
  import LichessAccount from "./LichessAccount.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let link = $state("");
  /** Whose studies the list shows. */
  let owner = $state("");
  let studies = $state<StudyInfoDto[] | null>(null);
  let listing = $state(false);
  let preview = $state<StudyPreviewDto | null>(null);
  let looking = $state(false);
  let into = $state<"new" | "open">("new");
  /** Making the database, or importing. */
  let busy = $state(false);
  let job = $state<number | null>(null);
  let result = $state<{ ok: boolean; message: string; games: number; name: string } | null>(null);
  let error = $state("");
  /** Bumped by each look-up and import, and by closing: a late answer does nothing. */
  let run = 0;
  let linkField: HTMLInputElement;

  onMount(() => {
    lichess.importTarget = "";
    linkField.focus();
    void lichess.load();
  });

  // Logged in (now, or before): the user's own studies, private ones too.
  $effect(() => {
    const user = lichess.username;
    if (!user) return;
    untrack(() => {
      if (owner.trim()) return;
      owner = user;
      void list();
    });
  });

  const progress = $derived(job === null ? undefined : jobs.running.find((j) => j.id === job));
  const percent = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.done / progress.total) * 100))
      : 0,
  );
  const newName = $derived(
    lichess.importTarget
      ? baseName(lichess.importTarget).replace(/\.chessdb$/i, "")
      : (preview?.name ?? "named after the study"),
  );

  async function list(e?: SubmitEvent) {
    e?.preventDefault();
    const name = owner.trim();
    if (!name) {
      error = "Type whose studies to list.";
      return;
    }
    listing = true;
    error = "";
    try {
      studies = await api.lichessStudies(name);
    } catch (err) {
      studies = null;
      error = errorMessage(err);
      void lichess.checkError(err);
    } finally {
      listing = false;
    }
  }

  async function lookUp(text = link) {
    const t = text.trim();
    if (!t) {
      error = "Paste the link of a study or chapter first.";
      return;
    }
    link = t;
    const mine = ++run;
    looking = true;
    error = "";
    result = null;
    preview = null;
    lichess.importTarget = "";
    try {
      const p = await api.lichessStudyLookup(t);
      if (mine === run) preview = p;
    } catch (err) {
      if (mine === run) {
        error = errorMessage(err);
        void lichess.checkError(err);
      }
    } finally {
      if (mine === run) looking = false;
    }
  }

  function submitLink(e: SubmitEvent) {
    e.preventDefault();
    void lookUp();
  }

  async function chooseTarget() {
    const name = preview?.name ?? "Lichess study";
    const path = await save({
      title: "Save the new database as",
      defaultPath: lichess.importTarget || (await api.suggestDbPath(name)),
    });
    if (path) lichess.importTarget = path;
  }

  async function start() {
    const p = preview;
    if (!p || busy) return;
    if (into === "open" && !db.info) {
      error = "No database is open: import into a new one.";
      return;
    }
    const mine = ++run;
    busy = true;
    error = "";
    result = null;
    try {
      if (into === "new") {
        try {
          await db.create(lichess.importTarget || (await api.suggestDbPath(p.name)));
        } catch (err) {
          error =
            isAppError(err) && err.code === "alreadyExists"
              ? "There's already a database with that name in that folder. Pick another name or folder with Change…"
              : `Couldn't make the database: ${errorMessage(err)}. Pick another folder with Change…`;
          return;
        }
      }
      if (mine !== run) return;
      const handle = db.info?.handle;
      const done = await db.importStudy(p.link, (id) => {
        if (mine === run) job = id;
        else void jobs.cancel(id);
      });
      if (!done || mine !== run) return;
      const here = db.info?.handle === handle ? db.info : null;
      const state = done.state === "running" ? "done" : done.state;
      result = {
        ok: state !== "failed",
        message:
          state === "failed"
            ? (done.error ?? "")
            : state === "cancelled"
              ? `The import of ${p.name} was cancelled.`
              : done.message,
        name: here?.name ?? "",
        games: here?.games ?? 0,
      };
      if (state === "failed") void lichess.load(true);
    } finally {
      if (mine === run) {
        busy = false;
        job = null;
      }
    }
  }

  /** An import goes on with the dialog closed; a look-up doesn't. */
  function close() {
    if (job === null) run++;
    onClose();
  }

  function shown(chapters: string[]): string {
    const first = chapters.slice(0, 6).map((c) => c || "(no name)");
    const more = chapters.length - first.length;
    return first.join(", ") + (more > 0 ? `, and ${formatCount(more)} more` : "");
  }
</script>

<Modal title="Import a Lichess study" onClose={close} width="600px">
  <LichessAccount
    why="Public studies import without a login. Private and unlisted ones need it, and so does the list of your own."
  />

  <form class="link" onsubmit={submitLink}>
    <label class="field">
      Study or chapter link
      <div class="row">
        <input
          bind:this={linkField}
          bind:value={link}
          placeholder="https://lichess.org/study/…"
          autocomplete="off"
          spellcheck="false"
          disabled={busy}
        />
        <button type="submit" disabled={busy || looking}
          >{looking ? "Looking up…" : "Look up"}</button
        >
      </div>
    </label>
  </form>

  <form class="owner" onsubmit={list}>
    <label class="field">
      Or pick one of a user's studies
      <div class="row">
        <input
          bind:value={owner}
          placeholder="Lichess username"
          autocomplete="off"
          spellcheck="false"
          disabled={busy}
        />
        <button type="submit" disabled={busy || listing}
          >{listing ? "Listing…" : "List studies"}</button
        >
      </div>
    </label>
  </form>
  {#if studies}
    {#if studies.length}
      <ul class="studies" aria-label="Studies">
        {#each studies as s (s.id)}
          <li>
            <button
              class="study"
              class:on={preview?.link === s.link}
              disabled={busy}
              onclick={() => lookUp(s.link)}
            >
              <span class="name">{s.name}</span><span class="soft">{s.updated}</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="soft">
        {owner.trim()} has no studies{lichess.username?.toLowerCase() === owner.trim().toLowerCase()
          ? ""
          : " anyone can see"}.
      </p>
    {/if}
  {/if}

  {#if preview}
    <div class="preview" aria-live="polite">
      <p>
        <strong>{preview.name}</strong>
        {#if preview.private}<span class="badge">private or unlisted</span>{/if}
        <span class="soft">
          {preview.oneChapter
            ? "one chapter"
            : `${formatCount(preview.chapters.length)} ${preview.chapters.length === 1 ? "chapter" : "chapters"}`}:
          {shown(preview.chapters)}</span
        >
      </p>
      <fieldset disabled={busy}>
        <legend>Into</legend>
        <label class="choice" class:on={into === "new"}>
          <input
            type="radio"
            name="study-into"
            checked={into === "new"}
            onchange={() => (into = "new")}
          />
          <span>
            A new database: <strong>{newName}</strong>
            <span class="soft folder" title={lichess.importTarget || undefined}
              >in {lichess.importTarget ? folderName(lichess.importTarget) : "Documents"}</span
            >
          </span>
          {#if into === "new"}<button type="button" onclick={chooseTarget}>Change…</button>{/if}
        </label>
        <label class="choice" class:on={into === "open"} class:off={!db.info}>
          <input
            type="radio"
            name="study-into"
            checked={into === "open"}
            disabled={!db.info}
            onchange={() => (into = "open")}
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
      <p class="soft">
        Comments, variations, arrows, clock times and the board's orientation come too. Chapters the
        database has already are left out.
      </p>
    </div>
  {/if}

  <div class="status" class:empty={!busy && !result && !error} aria-live="polite">
    {#if busy && job !== null}
      <div class="progress">
        <progress max="100" value={percent}></progress><span>{percent}%</span>
      </div>
    {:else if busy}
      <p class="soft">Making the database…</p>
    {/if}
    {#if result}
      {#if result.ok}
        <p class="done" role="status">
          {sentence(result.message)}{#if result.name}{" "}{result.name} has {formatCount(
              result.games,
            )}
            {result.games === 1 ? "game" : "games"} now.{/if}
        </p>
      {:else}
        <p class="error" role="alert">{sentence(result.message)}</p>
      {/if}
    {/if}
    {#if error}<p class="error" role="alert">{sentence(error)}</p>{/if}
  </div>

  {#snippet actions()}
    {#if busy && job !== null}
      <button onclick={() => job !== null && jobs.cancel(job)}>Stop import</button>
      <button onclick={close}>Close</button>
    {:else}
      <button onclick={close}>Close</button>
      <button class="primary" disabled={!preview || busy} onclick={start}>Import</button>
    {/if}
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 8px;
  }

  form {
    margin: 0 0 10px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  .row {
    display: flex;
    gap: 6px;
  }

  .row input {
    flex: 1;
    min-width: 0;
    color: var(--ink);
    font-size: 14px;
  }

  .studies {
    max-height: 180px;
    margin: 0 0 12px;
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

  .preview {
    padding-top: 10px;
    border-top: 1px solid var(--rule);
  }

  .badge {
    margin: 0 4px;
    padding: 0 6px;
    border-radius: var(--radius);
    background: var(--brass-soft);
    font-size: 12px;
  }

  fieldset {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 6px 0 10px;
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
