<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { api } from "../lib/api";
  import { fraction, when } from "../lib/format";
  import type { AdminJob } from "../lib/types";

  let jobs = $state<AdminJob[]>([]);
  let error = $state("");
  let onlyRunning = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;

  let shown = $derived(onlyRunning ? jobs.filter((j) => j.state === "running") : jobs);
  let running = $derived(jobs.filter((j) => j.state === "running").length);

  const KINDS: Record<string, string> = {
    import: "Import",
    index: "Position index",
    lines: "Counting lines",
  };

  async function refresh() {
    try {
      jobs = await api.jobs();
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function cancel(id: number) {
    try {
      await api.cancelJob(id);
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 2000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<section class="panel">
  <div class="head">
    <h2>Jobs</h2>
    <span class="muted small">{running} running</span>
    <label class="inline small"><input type="checkbox" bind:checked={onlyRunning} /> Running only</label>
  </div>
  <p class="muted small">
    What runs where, for whom, and how far it is. A job's file names and results are its user's, so they aren't
    shown here.
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th class="num">#</th>
          <th>Kind</th>
          <th>Database</th>
          <th>Started by</th>
          <th>Started</th>
          <th>State</th>
          <th>Progress</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each shown as j (j.id)}
          {@const f = fraction(j.done, j.total)}
          <tr>
            <td class="num">{j.id}</td>
            <td>{KINDS[j.kind] ?? j.kind}</td>
            <td>{j.databaseName}</td>
            <td>{j.startedBy}</td>
            <td class="muted small">{when(j.startedAt)}</td>
            <td><span class="badge {j.state}">{j.state}</span></td>
            <td>
              {#if j.state === "running"}
                {#if f !== null}<progress value={f}></progress>{:else}<progress></progress>{/if}
              {/if}
            </td>
            <td>{#if j.state === "running"}<button class="danger" onclick={() => cancel(j.id)}>Cancel</button>{/if}</td>
          </tr>
        {:else}
          <tr><td colspan="8" class="muted">No jobs since the server started.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .head {
    display: flex;
    align-items: baseline;
    gap: 1rem;
  }
  .head h2 {
    margin-right: auto;
  }
</style>
