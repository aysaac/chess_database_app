<script lang="ts">
  import { api } from "../lib/api";

  // "Forgot password": asks for a reset link by e-mail. The answer is the
  // same whether the account (or its address) exists or not.
  let login = $state("");
  let error = $state("");
  let sent = $state(false);
  let busy = $state(false);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    busy = true;
    try {
      await api.forgotPassword(login.trim());
      sent = true;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="panel box">
  <h1>Forgot your password?</h1>
  {#if sent}
    <p class="success" role="status">
      If an account with that name or address has an e-mail address, a link to set a new password is on its way.
      It works for an hour.
    </p>
  {:else}
    <form onsubmit={submit}>
      <label>Username or e-mail address <input bind:value={login} required maxlength="254" autocomplete="username" /></label>
      {#if error}<p class="error small" role="alert">{error}</p>{/if}
      <button class="primary" type="submit" disabled={busy}>Send me a link</button>
    </form>
    <p class="muted small">Without an e-mail address on your account, ask the server's admin for a reset link.</p>
  {/if}
  <p class="small"><a href="#/">Sign in</a></p>
</div>

<style>
  .box,
  form {
    width: min(24rem, 100%);
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
</style>
