<script lang="ts">
  import { onMount } from "svelte";
  import { api, fullLink } from "../lib/api";
  import { bytes, parseBytes, when } from "../lib/format";
  import type { Invite, Link, Role } from "../lib/types";

  let invites = $state<Invite[]>([]);
  let error = $state("");
  let role = $state<Role>("user");
  let quota = $state("");
  let days = $state(7);
  let made = $state<Link | null>(null);
  let busy = $state(false);

  async function refresh() {
    try {
      invites = await api.invites();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function make(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    const q = parseBytes(quota);
    if (q === undefined) {
      error = "Write a quota such as 500 MB or 2 GB, or leave it empty for none.";
      return;
    }
    busy = true;
    try {
      made = await api.createInvite(role, q, days);
      quota = "";
      await refresh();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }

  async function withdraw(id: number) {
    error = "";
    try {
      await api.deleteInvite(id);
      if (made?.id === id) made = null;
      await refresh();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }

  const now = () => Date.now() / 1000;
  function status(i: Invite): string {
    if (i.usedAt) return `used by ${i.usedBy ?? "a deleted user"}`;
    return i.expiresAt <= now() ? "expired" : `open until ${when(i.expiresAt)}`;
  }

  onMount(refresh);
</script>

<section class="panel">
  <h2>Invite someone</h2>
  <p class="muted small">
    An invite is a link that makes one account, with the role and quota you choose, until it expires. Send it to
    the person yourself; it's shown only once.
  </p>
  <form class="form-row" onsubmit={make}>
    <label
      >Role
      <select bind:value={role}>
        <option value="user">User</option>
        <option value="admin">Admin</option>
      </select>
    </label>
    <label>Quota (empty: none) <input bind:value={quota} placeholder="e.g. 2 GB" /></label>
    <label>Works for (days) <input type="number" bind:value={days} min="1" max="30" required /></label>
    <button class="primary" type="submit" disabled={busy}>Make an invite link</button>
  </form>
  {#if made}
    <p class="note">
      Copy the link now; it isn't shown again. It works once, until {when(made.expiresAt)}:<br />
      <code class="token">{fullLink(made.url)}</code>
    </p>
  {/if}
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
</section>

<section class="panel">
  <h2>Invites</h2>
  <div class="table-wrap">
    <table>
      <thead><tr><th>Role</th><th class="num">Quota</th><th>Made</th><th>By</th><th>Status</th><th></th></tr></thead>
      <tbody>
        {#each invites as i (i.id)}
          <tr>
            <td><span class="badge" class:admin={i.role === "admin"}>{i.role}</span></td>
            <td class="num">{i.quotaBytes === null ? "none" : bytes(i.quotaBytes)}</td>
            <td class="muted small">{when(i.createdAt)}</td>
            <td>{i.createdBy ?? "—"}</td>
            <td class="small">{status(i)}</td>
            <td>
              {#if !i.usedAt && i.expiresAt > now()}
                <button class="danger" onclick={() => withdraw(i.id)}>Withdraw</button>
              {/if}
            </td>
          </tr>
        {:else}
          <tr><td colspan="6" class="muted">No invites yet.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .token {
    word-break: break-all;
  }
</style>
