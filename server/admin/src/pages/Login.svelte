<script lang="ts">
  import { api } from "../lib/api";
  import type { Me } from "../lib/types";

  let {
    signedIn,
    openSignup = false,
    emailResets = false,
  }: { signedIn: (me: Me) => void; openSignup?: boolean; emailResets?: boolean } = $props();

  let username = $state("");
  let password = $state("");
  let error = $state("");
  let busy = $state(false);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    error = "";
    try {
      signedIn(await api.login(username.trim(), password));
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
      password = "";
    }
  }
</script>

<form class="panel box" onsubmit={submit}>
  <h1>Chess server</h1>
  <p class="muted small">Sign in to manage your account, or the server if you're its admin.</p>
  <label>Username <input bind:value={username} autocomplete="username" required /></label>
  <label>Password <input type="password" bind:value={password} autocomplete="current-password" required /></label>
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  <button class="primary" type="submit" disabled={busy}>{busy ? "Signing in…" : "Sign in"}</button>
  {#if emailResets || openSignup}
    <p class="links small">
      {#if emailResets}<a href="#/forgot">Forgot your password?</a>{/if}
      {#if openSignup}<a href="#/signup">Make an account</a>{/if}
    </p>
  {/if}
</form>

<style>
  .box {
    width: min(22rem, 100%);
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .links {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    margin: 0;
  }
</style>
