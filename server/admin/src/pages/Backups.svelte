<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { bytes, when } from "../lib/format";
  import type { Backups } from "../lib/types";

  let info = $state<Backups | null>(null);
  let error = $state("");
  let notice = $state("");
  let busy = $state(false);

  async function refresh() {
    try {
      info = await api.backups();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function backUp() {
    error = "";
    notice = "";
    busy = true;
    try {
      const { job } = await api.backUp();
      notice = `Backing up (job ${job}): it shows under Jobs, and here once it's done.`;
      for (;;) {
        await new Promise((r) => setTimeout(r, 1000));
        const jobs = await api.jobs();
        const j = jobs.find((x) => x.id === job);
        if (!j || j.state !== "running") {
          notice = j?.state === "done" ? "Backed up." : `The backup ended: ${j?.state ?? "gone"}.`;
          break;
        }
      }
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  onMount(refresh);
</script>

<section class="panel">
  <h2>Backups</h2>
  <p class="muted small">
    A backup is the server's accounts and every database, copied while it runs into the backup folder on the
    server's disk. Backups can't be downloaded here: they hold private databases, which only their owners may
    read. To restore one, see "Backups" in the server's documentation.
  </p>
  {#if info && !info.configured}
    <p class="note">
      No backup folder is set. Set <code>CHESS_SERVER_BACKUP_DIR</code> (on another volume than the data, ideally),
      and <code>CHESS_SERVER_BACKUP_EVERY_HOURS</code> for backups on a schedule.
    </p>
  {:else if info}
    <p>
      {info.everyHours ? `Every ${info.everyHours} hours, and when you ask.` : "Only when you ask (no schedule set)."}
      The newest {info.keep} are kept.
    </p>
    <button class="primary" onclick={backUp} disabled={busy}>{busy ? "Backing up…" : "Back up now"}</button>
  {/if}
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  {#if notice}<p class="success small" role="status">{notice}</p>{/if}
</section>

{#if info?.configured}
  <section class="panel">
    <div class="table-wrap">
      <table>
        <thead><tr><th>Backup</th><th>Made</th><th class="num">Databases</th><th class="num">Size</th></tr></thead>
        <tbody>
          {#each info.backups as b (b.name)}
            <tr>
              <td><code>{b.name}</code></td>
              <td class="muted small">{when(b.createdAt)}</td>
              <td class="num">{b.databases}</td>
              <td class="num">{bytes(b.sizeBytes)}</td>
            </tr>
          {:else}
            <tr><td colspan="4" class="muted">No backups yet.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
{/if}
