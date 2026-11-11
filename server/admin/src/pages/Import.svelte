<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { api } from "../lib/api";
  import { bytes, count, fraction, visibilityLabel } from "../lib/format";
  import type { Database, ImportFile, JobInfo, Visibility } from "../lib/types";

  let files = $state<ImportFile[]>([]);
  let writable = $state<Database[]>([]);
  let error = $state("");

  let file = $state("");
  let into = $state<"new" | "existing">("new");
  let name = $state("");
  let nameTouched = $state(false);
  // Never server-wide unless chosen: a licensed database (Mega Database) is
  // sold per user and must stay private.
  let visibility = $state<Visibility>("private");
  let existing = $state("");
  let busy = $state(false);

  let job = $state<JobInfo | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;

  function suggestedName(path: string): string {
    const last = path.split("/").pop() ?? path;
    return last.replace(/\.(pgn|cbh)$/i, "");
  }

  $effect(() => {
    if (!nameTouched) name = file ? suggestedName(file) : "";
  });

  async function refresh() {
    try {
      [files, writable] = await Promise.all([
        api.importFiles(),
        api.myDatabases().then((dbs) => dbs.filter((d) => d.access !== "read")),
      ]);
      if (!files.some((f) => f.path === file)) file = files[0]?.path ?? "";
      if (!writable.some((d) => d.id === existing)) existing = writable[0]?.id ?? "";
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function poll(id: number) {
    clearInterval(timer);
    const tick = async () => {
      try {
        job = await api.job(id);
        if (job.state !== "running") {
          clearInterval(timer);
          busy = false;
          await refresh();
        }
      } catch (e) {
        clearInterval(timer);
        busy = false;
        error = e instanceof Error ? e.message : String(e);
      }
    };
    tick();
    timer = setInterval(tick, 700);
  }

  async function start(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    job = null;
    busy = true;
    try {
      let db = existing;
      if (into === "new") db = (await api.createDatabase(name.trim(), visibility)).id;
      poll((await api.importFile(db, file)).job);
    } catch (err) {
      busy = false;
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function cancel() {
    if (job) await api.cancelJob(job.id).catch(() => {});
  }

  onMount(refresh);
  onDestroy(() => clearInterval(timer));

  let progress = $derived(job ? fraction(job.done, job.total) : null);
</script>

<section class="panel">
  <h2>Import from the server folder</h2>
  <p class="small">
    Put PGN files, or ChessBase databases (the <code>.cbh</code> with its <code>.cbg</code>, <code>.cbp</code>,
    <code>.cbt</code> and <code>.cbc</code> beside it), in the <code>import</code> folder of the server's data
    directory, then import them here. Users import their own games by upload, through the API.
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if files.length === 0}
    <p class="muted">
      The import folder has no <code>.pgn</code> or <code>.cbh</code> files.
      <button class="link" onclick={refresh}>Look again</button>
    </p>
  {:else}
    <form class="stack" onsubmit={start}>
      <label
        >File
        <select bind:value={file}>
          {#each files as f (f.path)}
            <option value={f.path}>{f.path} ({f.kind === "cbh" ? "ChessBase" : "PGN"}, {bytes(f.sizeBytes)})</option>
          {/each}
        </select>
      </label>

      <fieldset>
        <legend>Into</legend>
        <label class="inline"><input type="radio" bind:group={into} value="new" /> A new database</label>
        <label class="inline"
          ><input type="radio" bind:group={into} value="existing" disabled={writable.length === 0} /> A database of yours</label
        >
      </fieldset>

      {#if into === "new"}
        <div class="form-row">
          <label class="grow"
            >Name <input bind:value={name} oninput={() => (nameTouched = true)} required maxlength="100" /></label
          >
          <label
            >Who can read it
            <select bind:value={visibility}>
              <option value="private">Private: only you</option>
              <option value="server">Server-wide: every user</option>
            </select>
          </label>
        </div>
        <p class="note">
          {#if visibility === "server"}
            Every user of this server will read these games. Make a database server-wide only when its licence
            allows it: free reference data such as TWIC or the Lichess database dumps, yes; a ChessBase Mega
            Database, no (it's sold per user).
          {:else}
            Private: no other user can read it. Keep a licensed database such as ChessBase's Mega Database
            private, since it's sold per user. You can make a database server-wide later, through the API.
          {/if}
        </p>
      {:else}
        <label
          >Database
          <select bind:value={existing}>
            {#each writable as d (d.id)}
              <option value={d.id}>{d.name} ({visibilityLabel(d.visibility)}, {count(d.games)} games)</option>
            {/each}
          </select>
        </label>
      {/if}

      <div>
        <button class="primary" type="submit" disabled={busy || !file || (into === "existing" && !existing)}
          >Import</button
        >
      </div>
    </form>
  {/if}
</section>

{#if job}
  <section class="panel" aria-live="polite">
    <h3>{job.label}</h3>
    {#if job.state === "running"}
      <p>
        {#if progress !== null}<progress value={progress}></progress>{:else}<progress></progress>{/if}
        <span class="muted small">{job.message}</span>
        <button class="danger" onclick={cancel}>Cancel</button>
      </p>
    {:else if job.state === "done"}
      <p class="success">{job.message}</p>
    {:else if job.state === "cancelled"}
      <p class="muted">Cancelled. The games imported until then stay.</p>
    {:else}
      <p class="error">{job.error}</p>
    {/if}
    {#if job.report && job.report.warningCount > 0}
      <p class="muted small">
        {count(job.report.warningCount)} warnings, {count(job.report.skipped)} games skipped.
      </p>
    {/if}
  </section>
{/if}

<style>
  .stack {
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
    max-width: 44rem;
  }

  fieldset {
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 0.5rem 0.8rem 0.6rem;
    display: flex;
    gap: 1.2rem;
    flex-wrap: wrap;
  }

  legend {
    font-size: 0.85rem;
    color: var(--muted);
    padding: 0 0.3rem;
  }

  fieldset label {
    color: var(--text);
    font-size: 0.95rem;
  }

  .grow {
    flex: 1;
    min-width: 14rem;
  }
</style>
