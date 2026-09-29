<script lang="ts">
  import { formatBytes, formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { jobs } from "../stores/jobs.svelte";
  import { notices } from "../stores/notify.svelte";
  import { onboarding } from "../stores/onboarding.svelte";
  import ImportWarnings from "./ImportWarnings.svelte";
  import Modal from "./Modal.svelte";

  let confirmCompact = $state(false);
  let showWarnings = $state(false);

  function percent(done: number, total: number) {
    return total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
  }
</script>

<footer>
  <span class="db">
    {#if db.info}
      <strong>{db.info.name}</strong>
      <span>{formatCount(db.info.games)} games</span>
      <span class="soft">{formatBytes(db.info.sizeBytes)}</span>
      {#if db.info.deleted}
        <span class="soft">{formatCount(db.info.deleted)} deleted</span>
      {/if}
      {#if db.importReport}
        <button
          class="close warn"
          title="Problems found while importing {db.importReport.file}"
          onclick={() => (showWarnings = true)}
          >{formatCount(db.importReport.warningCount)} import {db.importReport.warningCount === 1
            ? "warning"
            : "warnings"}</button
        >
      {/if}
      <button class="close" title="Remove deleted games for good and reclaim space" onclick={() => (confirmCompact = true)}
        >Compact…</button
      >
      <button class="close" title="Close this database" onclick={() => db.close()}>Close</button>
    {:else}
      <span class="soft">No database open</span>
    {/if}
  </span>

  <span class="jobs">
    {#each jobs.running as job (job.id)}
      <span class="job">
        <span>{job.label}</span>
        <progress max="100" value={percent(job.done, job.total)}></progress>
        <span class="soft">{percent(job.done, job.total)}%</span>
        <button onclick={() => jobs.cancel(job.id)}>Cancel</button>
      </span>
    {/each}
  </span>

  <span class="notice" class:error={notices.current?.kind === "error"} aria-live="polite">
    {notices.current?.text ?? ""}
  </span>

  <button
    class="help"
    aria-label="Getting started"
    title="Getting started: sample games, importing, the engine and tips"
    onclick={() => onboarding.show()}>?</button
  >
</footer>

{#if showWarnings && db.importReport}
  <ImportWarnings report={db.importReport} onClose={() => (showWarnings = false)} />
{/if}

{#if confirmCompact && db.info}
  <Modal title="Compact {db.info.name}" onClose={() => (confirmCompact = false)} width="480px">
    <p>
      {#if db.info.deleted}
        This removes the {formatCount(db.info.deleted)}
        {db.info.deleted === 1 ? "game" : "games"} marked as deleted for good, and the space old versions of
        edited games still take.
      {:else}
        No games are marked as deleted. Compacting reclaims the space old versions of edited games still take.
      {/if}
    </p>
    <p class="soft">
      Games are renumbered and the position index is rebuilt afterwards. Deleted games can't be restored once
      the database is compacted.
    </p>
    {#snippet actions()}
      <button onclick={() => (confirmCompact = false)}>Cancel</button>
      <button
        class="primary"
        onclick={() => {
          confirmCompact = false;
          db.compact();
        }}>Compact database</button
      >
    {/snippet}
  </Modal>
{/if}

<style>
  footer {
    display: grid;
    grid-template-columns: auto 1fr minmax(0, auto) auto;
    align-items: center;
    gap: 16px;
    height: 30px;
    padding: 0 12px;
    border-top: 1px solid var(--rule);
    background: var(--panel);
    font-size: 13px;
  }

  .db {
    display: flex;
    gap: 10px;
  }

  .soft {
    color: var(--ink-soft);
  }

  .jobs {
    display: flex;
    gap: 18px;
    min-width: 0;
  }

  .job {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .close {
    padding: 0 8px;
    color: var(--ink-soft);
  }

  .close.warn {
    color: var(--brass);
  }

  .job button {
    padding: 0 8px;
    border-color: var(--rule);
  }

  progress {
    width: 140px;
    height: 6px;
    accent-color: var(--felt);
  }

  .notice {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--felt);
    font-weight: 500;
  }

  .notice.error {
    color: var(--danger);
  }

  /* Opens the welcome guide (QOL-08). */
  .help {
    width: 20px;
    height: 20px;
    padding: 0;
    border-color: var(--rule);
    border-radius: 50%;
    font-size: 12px;
    font-weight: 600;
    line-height: 1;
    color: var(--ink-soft);
  }
</style>
