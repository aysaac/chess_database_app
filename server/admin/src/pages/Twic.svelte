<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { count, when } from "../lib/format";
  import type { Database, Twic } from "../lib/types";

  let twic = $state<Twic | null>(null);
  let mine = $state<Database[]>([]);
  let database = $state("");
  let everyDays = $state(7);
  let error = $state("");
  let notice = $state("");
  let busy = $state(false);

  async function refresh() {
    try {
      twic = await api.twic();
      database = twic.database ?? "";
      everyDays = twic.everyDays;
      mine = (await api.myDatabases()).filter((d) => d.access === "owner");
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    notice = "";
    try {
      twic = await api.setTwic(database || null, everyDays);
      notice = database ? "Saved." : "TWIC updates are off.";
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  async function checkNow() {
    error = "";
    notice = "";
    busy = true;
    try {
      const { job } = await api.twicCheck();
      notice = `Checking (job ${job})…`;
      for (;;) {
        await new Promise((r) => setTimeout(r, 1000));
        const j = await api.job(job);
        if (j.state !== "running") {
          notice = j.state === "done" ? j.message : `The check ended: ${j.error ?? j.state}.`;
          break;
        }
      }
      await refresh();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  onMount(refresh);
</script>

<section class="panel">
  <h2>The Week in Chess</h2>
  <p class="muted small">
    New issues of <a href="https://theweekinchess.com/twic" rel="noreferrer">The Week in Chess</a> (TWIC) go into
    a database of yours, once each. The first check takes the newest issue; each later one, the issues out since.
    TWIC is free to use: a server-wide database suits it.
  </p>
  <form class="form-row" onsubmit={save}>
    <label
      >Into
      <select bind:value={database}>
        <option value="">Nothing (off)</option>
        {#each mine as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
      </select>
    </label>
    <label>Check every (days; 0: only when asked) <input type="number" bind:value={everyDays} min="0" max="365" /></label>
    <button class="primary" type="submit">Save</button>
    <button type="button" onclick={checkNow} disabled={busy || !twic?.database}>{busy ? "Checking…" : "Check now"}</button>
  </form>
  {#if twic}
    <p class="muted small spaced">
      {twic.lastCheck ? `Last checked ${when(twic.lastCheck)}.` : "Not checked yet."}
      {twic.lastIssue ? ` The newest issue taken is ${twic.lastIssue}.` : ""}
    </p>
  {/if}
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  {#if notice}<p class="success small" role="status">{notice}</p>{/if}
</section>

{#if twic && twic.issues.length > 0}
  <section class="panel">
    <div class="table-wrap">
      <table>
        <thead><tr><th>Issue</th><th>Taken</th><th class="num">Games</th></tr></thead>
        <tbody>
          {#each twic.issues as i (i.issue)}
            <tr>
              <td>{i.issue}</td>
              <td class="muted small">{when(i.importedAt)}</td>
              <td class="num">{i.games === null ? "importing, or didn't finish" : count(i.games)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
{/if}

<style>
  .spaced {
    margin-top: 0.8rem;
  }
</style>
