<script lang="ts">
  // The Lichess login (NET-11) at the top of the study dialogs: who is
  // logged in, or the ways to log in: through the browser (Lichess's page
  // asks the user to let the app in), or with a personal token pasted in.
  // The token itself never reaches the page: Rust keeps it, in the OS's
  // credential store when there is one.
  import { onMount } from "svelte";
  import { sentence } from "../online";
  import { lichess } from "../stores/lichess.svelte";

  /** Why the dialog wants a login, shown while logged out. */
  let { why }: { why: string } = $props();

  let pasting = $state(false);
  let token = $state("");

  onMount(() => {
    void lichess.load();
  });

  async function useToken(e: SubmitEvent) {
    e.preventDefault();
    if (await lichess.useToken(token)) {
      token = "";
      pasting = false;
    }
  }
</script>

<section class="account" aria-label="Lichess login">
  {#if lichess.username}
    <p class="who">
      <span>
        Logged in to Lichess as <strong>{lichess.username}</strong>
        <span class="soft"
          >{lichess.account.kept
            ? "(kept in the system's keyring)"
            : "(until the app closes: no keyring is running to keep the login in)"}</span
        >
      </span>
      <button
        title="Log out, and have Lichess revoke the app's token"
        onclick={() => lichess.logout()}>Log out</button
      >
    </p>
  {:else if lichess.loggingIn && !pasting}
    <p class="who">
      <span class="soft">Waiting for you to let the app in on Lichess, in your browser…</span>
      <button onclick={() => lichess.cancelLogin()}>Cancel login</button>
    </p>
  {:else}
    <p>{why}</p>
    <div class="ways">
      <button onclick={() => lichess.login()}>Log in to Lichess…</button>
      <button aria-expanded={pasting} onclick={() => (pasting = !pasting)}
        >Use a token instead</button
      >
    </div>
    {#if pasting}
      <form class="token" onsubmit={useToken}>
        <p class="soft">
          Make a token on Lichess that can read private studies and create studies, then paste it
          here.
          <button type="button" class="link" onclick={() => lichess.open()}
            >Make a token on Lichess</button
          >
        </p>
        <div class="row">
          <input
            type="password"
            aria-label="Lichess token"
            placeholder="lip_…"
            bind:value={token}
            autocomplete="off"
            spellcheck="false"
          />
          <button type="submit" disabled={lichess.loggingIn}>
            {lichess.loggingIn ? "Checking…" : "Log in with the token"}
          </button>
        </div>
      </form>
    {/if}
  {/if}
  {#if lichess.loginError}<p class="error" role="alert">{sentence(lichess.loginError)}</p>{/if}
  {#if lichess.note}<p class="soft" role="status">{sentence(lichess.note)}</p>{/if}
</section>

<style>
  .account {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0 0 14px;
    padding: 8px 10px;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  p {
    margin: 0;
  }

  .who {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .who button {
    flex: none;
  }

  .ways,
  .row {
    display: flex;
    gap: 6px;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .token {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--felt);
    text-decoration: underline;
  }

  .link:hover:not(:disabled) {
    background: none;
  }

  .soft {
    color: var(--ink-soft);
    font-size: 13px;
  }

  .error {
    color: var(--danger);
  }
</style>
