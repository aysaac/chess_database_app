<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { when } from "../lib/format";
  import type { NewToken, Token } from "../lib/types";

  let current = $state("");
  let next = $state("");
  let again = $state("");
  let pwError = $state("");
  let pwDone = $state("");

  let tokens = $state<Token[]>([]);
  let tokenName = $state("");
  let made = $state<NewToken | null>(null);
  let tokError = $state("");

  async function refreshTokens() {
    try {
      tokens = await api.tokens();
    } catch (e) {
      tokError = e instanceof Error ? e.message : String(e);
    }
  }

  async function changePassword(e: SubmitEvent) {
    e.preventDefault();
    pwError = "";
    pwDone = "";
    if (next !== again) {
      pwError = "The new passwords differ.";
      return;
    }
    try {
      await api.changePassword(current, next);
      pwDone = "Changed. Your other sessions ended.";
      current = next = again = "";
    } catch (err) {
      pwError = err instanceof Error ? err.message : String(err);
    }
  }

  async function makeToken(e: SubmitEvent) {
    e.preventDefault();
    tokError = "";
    try {
      made = await api.createToken(tokenName.trim());
      tokenName = "";
      await refreshTokens();
    } catch (err) {
      tokError = err instanceof Error ? err.message : String(err);
    }
  }

  async function revoke(id: number) {
    tokError = "";
    try {
      await api.deleteToken(id);
      if (made?.id === id) made = null;
      await refreshTokens();
    } catch (err) {
      tokError = err instanceof Error ? err.message : String(err);
    }
  }

  onMount(refreshTokens);
</script>

<section class="panel">
  <h2>Password</h2>
  <form class="form-row" onsubmit={changePassword}>
    <label>Current <input type="password" bind:value={current} required autocomplete="current-password" /></label>
    <label>New <input type="password" bind:value={next} required minlength="8" autocomplete="new-password" /></label>
    <label>Again <input type="password" bind:value={again} required autocomplete="new-password" /></label>
    <button class="primary" type="submit">Change</button>
  </form>
  {#if pwError}<p class="error small" role="alert">{pwError}</p>{/if}
  {#if pwDone}<p class="success small" role="status">{pwDone}</p>{/if}
</section>

<section class="panel">
  <h2>API tokens</h2>
  <p class="muted small">
    For scripts and, later, the desktop app: send <code>Authorization: Bearer &lt;token&gt;</code>. A token acts
    as you until it's revoked or your account is disabled.
  </p>
  {#if made}
    <p class="note">
      Copy the token now; it isn't shown again:<br /><code class="token">{made.token}</code>
    </p>
  {/if}
  {#if tokError}<p class="error small" role="alert">{tokError}</p>{/if}
  {#if tokens.length > 0}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Name</th><th>Made</th><th>Last used</th><th></th></tr></thead>
        <tbody>
          {#each tokens as t (t.id)}
            <tr>
              <td>{t.name}</td>
              <td class="muted small">{when(t.createdAt)}</td>
              <td class="muted small">{when(t.lastUsedAt)}</td>
              <td><button class="danger" onclick={() => revoke(t.id)}>Revoke</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  <form class="form-row spaced" onsubmit={makeToken}>
    <label>Name <input bind:value={tokenName} required maxlength="100" placeholder="e.g. laptop" /></label>
    <button type="submit">Make a token</button>
  </form>
</section>

<style>
  .token {
    word-break: break-all;
  }
  .spaced {
    margin-top: 0.8rem;
  }
</style>
