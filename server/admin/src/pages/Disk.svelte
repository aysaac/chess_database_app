<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { bytes, count } from "../lib/format";
  import type { Disk } from "../lib/types";

  let disk = $state<Disk | null>(null);
  let error = $state("");

  async function refresh() {
    try {
      disk = await api.disk();
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  onMount(refresh);

  let usedShare = $derived(
    disk && disk.fsTotalBytes ? (disk.fsTotalBytes - (disk.fsFreeBytes ?? 0)) / disk.fsTotalBytes : null,
  );
</script>

<section class="panel">
  <div class="head">
    <h2>Disk</h2>
    <button onclick={refresh}>Refresh</button>
  </div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if disk}
    <p class="small">Data directory: <code>{disk.dataDir}</code></p>
    <div class="stats">
      <div class="stat">
        <div class="value">{bytes(disk.fsFreeBytes)}</div>
        <div class="label">free of {bytes(disk.fsTotalBytes)}</div>
        {#if usedShare !== null}<progress value={usedShare}></progress>{/if}
      </div>
      <div class="stat">
        <div class="value">{bytes(disk.databasesBytes)}</div>
        <div class="label">{count(disk.databases)} databases (with their position indexes)</div>
      </div>
      <div class="stat">
        <div class="value">{bytes(disk.importBytes)}</div>
        <div class="label">import folder</div>
      </div>
      <div class="stat">
        <div class="value">{bytes(disk.uploadsBytes)}</div>
        <div class="label">uploads being imported</div>
      </div>
      <div class="stat">
        <div class="value">{bytes(disk.metadataBytes)}</div>
        <div class="label">users, sessions and permissions</div>
      </div>
    </div>
    <p class="muted small spaced">
      The data directory must be on a local disk, not a network share (NFS, SMB): databases rely on file locks
      and on writes reaching the disk when asked.
    </p>
  {/if}
</section>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .stat progress {
    width: 100%;
    margin-top: 0.4rem;
  }
  .spaced {
    margin-top: 1rem;
  }
</style>
