<script lang="ts">
  import { onMount } from "svelte";
  import { api, exportUrl } from "../lib/api";
  import { bytes, count, parseBytes, visibilityLabel, when } from "../lib/format";
  import type { AdminDatabase } from "../lib/types";

  let dbs = $state<AdminDatabase[]>([]);
  let error = $state("");
  let notice = $state("");

  let dialog: HTMLDialogElement | undefined = $state();
  let target = $state<AdminDatabase | null>(null);
  let mode = $state<"quota" | "delete">("quota");
  let field = $state("");
  let dialogError = $state("");
  let busy = $state(false);

  let total = $derived(dbs.reduce((s, d) => s + d.sizeBytes, 0));

  async function refresh() {
    try {
      dbs = await api.allDatabases();
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function open(d: AdminDatabase, m: "quota" | "delete") {
    target = d;
    mode = m;
    field = m === "quota" && d.quotaBytes !== null ? bytes(d.quotaBytes) : "";
    dialogError = "";
    dialog?.showModal();
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    const d = target;
    if (!d) return;
    dialogError = "";
    busy = true;
    try {
      if (mode === "quota") {
        const quota = parseBytes(field);
        if (quota === undefined) {
          dialogError = "Write a size such as 500 MB or 2 GB, or leave it empty for none.";
          return;
        }
        await api.setDatabaseQuota(d.id, quota);
        notice = `Set the quota of ${d.name}.`;
      } else {
        await api.deleteDatabase(d.id, field);
        notice = `Deleted ${d.name} (${d.owner}).`;
      }
      dialog?.close();
      await refresh();
    } catch (err) {
      dialogError = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  onMount(refresh);
</script>

<section class="panel">
  <h2>Databases</h2>
  <p class="note">
    Metadata only. Nobody but its owner can read a private database's games, positions or searches, the admin
    included; and only the owner changes who can read it. The files on the server's disk aren't encrypted:
    whoever can read the data folder can read them.
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="success" role="status">{notice}</p>{/if}
  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th>Name</th>
          <th>Owner</th>
          <th>Visibility</th>
          <th class="num">Games</th>
          <th class="num">Size</th>
          <th class="num">Quota</th>
          <th>Made</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each dbs as d (d.id)}
          <tr>
            <td class="wrap">{d.name}<br /><code class="muted small">{d.id}</code></td>
            <td>{d.owner}</td>
            <td><span class="badge" class:server={d.visibility === "server"}>{visibilityLabel(d.visibility)}</span></td>
            <td class="num">{count(d.games)}</td>
            <td class="num">{bytes(d.sizeBytes)}</td>
            <td class="num">{d.quotaBytes === null ? "none" : bytes(d.quotaBytes)}</td>
            <td class="muted small">{when(d.createdAt)}</td>
            <td><div class="actions">
              {#if d.access}
                <a class="button" href={exportUrl(d.id)} download title="Download its games as PGN">Export PGN</a>
              {/if}
              <button onclick={() => open(d, "quota")}>Quota</button>
              <button class="danger" onclick={() => open(d, "delete")}>Delete…</button>
            </div></td>
          </tr>
        {:else}
          <tr><td colspan="8" class="muted">No databases yet. Import one from the server folder, under Import.</td></tr>
        {/each}
      </tbody>
      {#if dbs.length > 1}
        <tfoot>
          <tr>
            <td colspan="4" class="muted">{dbs.length} databases</td>
            <td class="num">{bytes(total)}</td>
            <td colspan="3"></td>
          </tr>
        </tfoot>
      {/if}
    </table>
  </div>
</section>

<dialog bind:this={dialog}>
  {#if target}
    <form onsubmit={save}>
      {#if mode === "quota"}
        <h3>Quota for {target.name}</h3>
        <label>Most this database may take (e.g. 2 GB; empty for none) <input bind:value={field} /></label>
        <p class="muted small">An import stops when it's reached; the games imported so far stay.</p>
      {:else}
        <h3>Delete {target.name}?</h3>
        <p>
          This deletes {target.owner}'s database and its {count(target.games)} games for good. It can't be undone.
        </p>
        <label>Type its name to confirm <input bind:value={field} autocomplete="off" required /></label>
      {/if}
      {#if dialogError}<p class="error small" role="alert">{dialogError}</p>{/if}
      <div class="dialog-buttons">
        <button type="button" onclick={() => dialog?.close()}>Cancel</button>
        {#if mode === "quota"}
          <button class="primary" type="submit" disabled={busy}>Save</button>
        {:else}
          <button class="danger solid" type="submit" disabled={busy || field.trim() !== target.name}>Delete</button>
        {/if}
      </div>
    </form>
  {/if}
</dialog>
