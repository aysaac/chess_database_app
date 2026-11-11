<script lang="ts">
  import { api } from "../lib/api";

  // Setting a new password with a reset link (`#/reset/TOKEN`), made by the
  // admin or e-mailed by "Forgot password".
  let { token }: { token: string } = $props();

  let password = $state("");
  let again = $state("");
  let error = $state("");
  let done = $state(false);
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
      await api.resetPassword(token, password);
      done = true;
      history.replaceState(null, "", location.pathname);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="panel box">
  <h1>Set a new password</h1>
  {#if done}
    <p class="success" role="status">Your password is set. You were signed out everywhere.</p>
    <p><a href="#/">Sign in</a></p>
  {:else}
    <form onsubmit={submit}>
      <label>New password <input type="password" bind:value={password} required minlength="8" autocomplete="new-password" /></label>
      <label>Again <input type="password" bind:value={again} required minlength="8" autocomplete="new-password" /></label>
      {#if error}<p class="error small" role="alert">{error}</p>{/if}
      <button class="primary" type="submit" disabled={busy}>Set the password</button>
    </form>
    <p class="muted small">The link works once. Setting the password signs you out everywhere.</p>
  {/if}
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
