<script lang="ts">
  import { onMount } from "svelte";
  import { api, ApiError, setCsrf, whenSignedOut } from "./lib/api";
  import type { Me } from "./lib/types";
  import Account from "./pages/Account.svelte";
  import Databases from "./pages/Databases.svelte";
  import Disk from "./pages/Disk.svelte";
  import Import from "./pages/Import.svelte";
  import Jobs from "./pages/Jobs.svelte";
  import Login from "./pages/Login.svelte";
  import Setup from "./pages/Setup.svelte";
  import Users from "./pages/Users.svelte";

  const PAGES = [
    { id: "users", label: "Users" },
    { id: "databases", label: "Databases" },
    { id: "import", label: "Import" },
    { id: "jobs", label: "Jobs" },
    { id: "disk", label: "Disk" },
    { id: "account", label: "Account" },
  ] as const;
  type PageId = (typeof PAGES)[number]["id"];

  let phase = $state<"loading" | "setup" | "login" | "ready" | "error">("loading");
  let me = $state<Me | null>(null);
  let page = $state<PageId>(fromHash());
  let loadError = $state("");

  function fromHash(): PageId {
    const h = location.hash.replace(/^#\/?/, "");
    return (PAGES.find((p) => p.id === h)?.id ?? "users") as PageId;
  }

  async function load() {
    try {
      const m = await api.me();
      setCsrf(m.csrfToken);
      me = m;
      phase = "ready";
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) {
        try {
          phase = (await api.setupStatus()).needed ? "setup" : "login";
        } catch (e2) {
          loadError = String(e2);
          phase = "error";
        }
      } else {
        loadError = e instanceof Error ? e.message : String(e);
        phase = "error";
      }
    }
  }

  function signedIn(m: Me) {
    setCsrf(m.csrfToken);
    me = m;
    phase = "ready";
  }

  async function logout() {
    try {
      await api.logout();
    } finally {
      setCsrf(null);
      me = null;
      phase = "login";
    }
  }

  onMount(() => {
    const onHash = () => (page = fromHash());
    window.addEventListener("hashchange", onHash);
    whenSignedOut(() => {
      setCsrf(null);
      me = null;
      phase = "login";
    });
    load();
    return () => window.removeEventListener("hashchange", onHash);
  });
</script>

{#if phase === "loading"}
  <main class="center"><p class="muted">Loading…</p></main>
{:else if phase === "error"}
  <main class="center">
    <div class="panel narrow">
      <h1>Can't reach the server</h1>
      <p class="error">{loadError}</p>
      <button onclick={load}>Try again</button>
    </div>
  </main>
{:else if phase === "setup"}
  <main class="center"><Setup done={() => (phase = "login")} /></main>
{:else if phase === "login" || !me}
  <main class="center"><Login {signedIn} /></main>
{:else if me.role !== "admin"}
  <main class="center">
    <div class="panel narrow">
      <h1>For admins only</h1>
      <p>
        You're signed in as <strong>{me.username}</strong>, who isn't an admin. This page manages the server; a
        page for players comes later.
      </p>
      <button onclick={logout}>Log out</button>
    </div>
  </main>
{:else}
  <header>
    <div class="brand">Chess server <span class="muted">admin</span></div>
    <nav>
      {#each PAGES as p (p.id)}
        <a href={`#/${p.id}`} class:active={page === p.id} aria-current={page === p.id ? "page" : undefined}
          >{p.label}</a
        >
      {/each}
    </nav>
    <div class="who">
      <span class="muted">{me.username}</span>
      <button class="link" onclick={logout}>Log out</button>
    </div>
  </header>
  <main>
    {#if page === "users"}
      <Users {me} />
    {:else if page === "databases"}
      <Databases />
    {:else if page === "import"}
      <Import />
    {:else if page === "jobs"}
      <Jobs />
    {:else if page === "disk"}
      <Disk />
    {:else}
      <Account />
    {/if}
  </main>
{/if}

<style>
  header {
    display: flex;
    align-items: center;
    gap: 1.5rem;
    padding: 0.6rem 1.25rem;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }

  .brand {
    font-weight: 600;
  }

  nav {
    display: flex;
    gap: 0.25rem;
    flex: 1;
    flex-wrap: wrap;
  }

  nav a {
    text-decoration: none;
    color: var(--muted);
    padding: 0.3rem 0.65rem;
    border-radius: 6px;
  }

  nav a:hover {
    color: var(--text);
    background: var(--row-hover);
  }

  nav a.active {
    color: var(--text);
    background: var(--row-hover);
    font-weight: 600;
  }

  .who {
    display: flex;
    gap: 0.7rem;
    align-items: center;
  }

  main {
    padding: 1.25rem;
    max-width: 78rem;
    margin: 0 auto;
  }

  @media (max-width: 720px) {
    header {
      gap: 0.5rem 1rem;
      padding: 0.6rem 1rem;
    }
    nav {
      order: 3;
      flex-basis: 100%;
      flex-wrap: nowrap;
      overflow-x: auto;
    }
    .who {
      margin-left: auto;
    }
    main {
      padding: 1rem;
    }
  }

  main.center {
    display: flex;
    justify-content: center;
    padding-top: 12vh;
  }

  .narrow {
    width: min(26rem, 100%);
  }
</style>
