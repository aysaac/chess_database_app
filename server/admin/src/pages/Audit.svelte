<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { count, when } from "../lib/format";
  import type { AuditPage } from "../lib/types";

  const PAGE = 100;
  const GROUPS = [
    ["", "Everything"],
    ["login", "Logins"],
    ["user", "Users"],
    ["password", "Passwords"],
    ["email", "E-mail addresses"],
    ["invite", "Invites"],
    ["token", "API tokens"],
    ["database", "Databases"],
    ["share", "Shares"],
    ["transfer", "Transfers"],
    ["import", "Imports"],
    ["export", "Exports"],
    ["games", "Games deleted or restored"],
    ["duplicates", "Duplicates deleted"],
    ["compact", "Compactions"],
    ["backup", "Backups"],
    ["twic", "TWIC updates"],
    ["job", "Jobs cancelled"],
  ];

  let page = $state<AuditPage | null>(null);
  let error = $state("");
  let action = $state("");
  let who = $state("");
  let from = $state("");
  let to = $state("");
  let offset = $state(0);

  /** A date field (yyyy-mm-dd, local) as Unix seconds; `end` for the day after. */
  function seconds(day: string, end = false): number | null {
    if (!day) return null;
    const d = new Date(`${day}T00:00:00`);
    if (end) d.setDate(d.getDate() + 1);
    return Math.floor(d.getTime() / 1000);
  }

  async function load() {
    error = "";
    try {
      page = await api.audit({ action, who: who.trim(), from: seconds(from), to: seconds(to, true), offset, limit: PAGE });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function search(e: SubmitEvent) {
    e.preventDefault();
    offset = 0;
    load();
  }

  function go(by: number) {
    offset = Math.max(0, offset + by);
    load();
  }

  function target(e: AuditPage["entries"][number]): string {
    if (!e.targetKind) return "";
    return `${e.targetKind} ${e.target ?? ""}`.trim() || e.targetKind;
  }

  onMount(load);
</script>

<section class="panel">
  <h2>Audit log</h2>
  <p class="muted small">
    Who did what, when and from where: logins, accounts, databases, shares, imports, exports, compactions and
    backups. Never games, searches, positions or file names. Entries are kept {page?.days ?? "…"} days.
  </p>
  <form class="form-row" onsubmit={search}>
    <label
      >What
      <select bind:value={action}>
        {#each GROUPS as [value, label] (value)}<option {value}>{label}</option>{/each}
      </select>
    </label>
    <label>User <input bind:value={who} placeholder="who did it, or to whom" /></label>
    <label>From <input type="date" bind:value={from} /></label>
    <label>To <input type="date" bind:value={to} /></label>
    <button class="primary" type="submit">Show</button>
  </form>
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
</section>

{#if page}
  <section class="panel">
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th>When</th><th>Who</th><th>What</th><th>To</th><th>Detail</th><th>From</th></tr>
        </thead>
        <tbody>
          {#each page.entries as e, i (i)}
            <tr>
              <td class="small nowrap">{when(e.at)}</td>
              <td>{e.actor ?? "—"}</td>
              <td><code>{e.action}</code></td>
              <td class="wrap">{target(e)}</td>
              <td class="small wrap">{e.detail ?? ""}</td>
              <td class="muted small">{e.client ?? ""}</td>
            </tr>
          {:else}
            <tr><td colspan="6" class="muted">Nothing matches.</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div class="pager">
      <span class="muted small"
        >{page.total === 0 ? "" : `${count(offset + 1)}–${count(offset + page.entries.length)} of ${count(page.total)}`}</span
      >
      <button disabled={offset === 0} onclick={() => go(-PAGE)}>Newer</button>
      <button disabled={offset + PAGE >= page.total} onclick={() => go(PAGE)}>Older</button>
    </div>
  </section>
{/if}

<style>
  .pager {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    justify-content: flex-end;
    margin-top: 0.75rem;
  }
  .nowrap {
    white-space: nowrap;
  }
</style>
