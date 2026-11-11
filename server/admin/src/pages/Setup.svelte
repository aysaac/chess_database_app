<script lang="ts">
  import { api } from "../lib/api";

  let { done }: { done: () => void } = $props();

  let token = $state("");
  let username = $state("admin");
  let password = $state("");
  let again = $state("");
  let error = $state("");
  let busy = $state(false);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    if (password !== again) {
      error = "The passwords differ.";
      return;
    }
    busy = true;
    try {
      await api.setup(token.trim(), username.trim(), password);
      done();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<form class="panel box" onsubmit={submit}>
  <h1>Set up the server</h1>
  <p class="small">
    Make the first admin account. The setup token is in the server's log from its start (for a container:
    <code>docker compose logs chess-server</code>).
  </p>
  <label>Setup token <input bind:value={token} autocomplete="off" spellcheck="false" required /></label>
  <label>Admin username <input bind:value={username} autocomplete="username" required /></label>
  <label
    >Password (8 characters or more)
    <input type="password" bind:value={password} autocomplete="new-password" minlength="8" required /></label
  >
  <label>Password again <input type="password" bind:value={again} autocomplete="new-password" required /></label>
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  <button class="primary" type="submit" disabled={busy}>{busy ? "Setting up…" : "Make the admin"}</button>
</form>

<style>
  .box {
    width: min(26rem, 100%);
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
</style>
