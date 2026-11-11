<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { Me, Role } from "../lib/types";

  // Making an account: with an invite's link (`#/invite/TOKEN`), or, when
  // the server allows it, without one (`#/signup`).
  let { invite, signedIn }: { invite: string | null; signedIn: (me: Me) => void } = $props();

  let role = $state<Role | null>(null);
  let checking = $state(true);
  let linkError = $state("");
  let username = $state("");
  let password = $state("");
  let again = $state("");
  let email = $state("");
  let error = $state("");
  let busy = $state(false);

  onMount(async () => {
    if (invite === null) {
      checking = false;
      return;
    }
    try {
      role = (await api.invite(invite)).role;
    } catch (e) {
      linkError = e instanceof Error ? e.message : String(e);
    } finally {
      checking = false;
    }
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    error = "";
    if (password !== again) {
      error = "The passwords differ.";
      return;
    }
    busy = true;
    try {
      const me = await api.signup(invite, username.trim(), password, email.trim() || null);
      history.replaceState(null, "", location.pathname);
      signedIn(me);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="panel box">
  <h1>Make an account</h1>
  {#if checking}
    <p class="muted">Checking the invite…</p>
  {:else if linkError}
    <p class="error" role="alert">{linkError}</p>
    <p><a href="#/">Sign in</a></p>
  {:else}
    <p class="muted small">
      {#if invite}
        You're invited to the chess server{role === "admin" ? " as an admin" : ""}. Choose a username and a
        password.
      {:else}
        Choose a username and a password for the chess server.
      {/if}
    </p>
    <form onsubmit={submit}>
      <label
        >Username <span class="muted small">(letters, digits, . _ -)</span>
        <input bind:value={username} required maxlength="32" autocomplete="username" /></label
      >
      <label>Password <input type="password" bind:value={password} required minlength="8" autocomplete="new-password" /></label>
      <label>Again <input type="password" bind:value={again} required minlength="8" autocomplete="new-password" /></label>
      <label
        >E-mail address <span class="muted small">(optional: for reset links)</span>
        <input type="email" bind:value={email} maxlength="254" autocomplete="email" /></label
      >
      {#if error}<p class="error small" role="alert">{error}</p>{/if}
      <button class="primary" type="submit" disabled={busy}>{busy ? "Making it…" : "Make the account"}</button>
    </form>
    <p class="small"><a href="#/">I have an account</a></p>
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
