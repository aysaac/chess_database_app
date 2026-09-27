<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import type { UciOptionDto } from "../bindings/UciOptionDto";
  import { analysis } from "../stores/analysis.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // Set by the analysis pane and always on, so not offered here.
  const MANAGED = new Set(["multipv", "uci_showwdl", "uci_analysemode", "ponder"]);
  const COMMON = ["Threads", "Hash", "SyzygyPath"];

  let selectedId = $state<string | null>(analysis.defaultEngine?.id ?? null);
  const selected = $derived(analysis.engines.find((e) => e.id === selectedId) ?? analysis.engines[0]);
  let values = $state<Record<string, string>>({});
  let busy = $state(false);
  let showAll = $state(false);

  // Edit a copy of the selected engine's values.
  $effect(() => {
    const e = selected;
    values = e ? Object.fromEntries(e.declared.map((o) => [o.name, current(o, e.options)])) : {};
  });

  function current(o: UciOptionDto, set: [string, string][]): string {
    const hit = set.find(([n]) => n.toLowerCase() === o.name.toLowerCase());
    if (hit) return hit[1];
    return o.type === "button" ? "" : String(o.default);
  }

  const offered = $derived(
    (selected?.declared ?? []).filter((o) => o.type !== "button" && !MANAGED.has(o.name.toLowerCase())),
  );
  const common = $derived(offered.filter((o) => COMMON.includes(o.name)));
  const others = $derived(offered.filter((o) => !COMMON.includes(o.name)));

  async function addEngine() {
    const path = await open({ multiple: false, directory: false, title: "Choose a UCI engine program" });
    if (typeof path !== "string") return;
    busy = true;
    const before = new Set(analysis.engines.map((e) => e.id));
    if (await analysis.add(path)) selectedId = analysis.engines.find((e) => !before.has(e.id))?.id ?? selectedId;
    busy = false;
  }

  async function detect() {
    busy = true;
    await analysis.detect();
    busy = false;
  }

  async function save() {
    if (!selected) return onClose();
    const options = offered.map((o) => [o.name, String(values[o.name] ?? "")] as [string, string]);
    busy = true;
    const ok = await analysis.setOptions(selected.id, options);
    busy = false;
    if (ok) onClose();
  }
</script>

{#snippet field(o: UciOptionDto)}
  <label class="field">
    <span>{o.name}</span>
    {#if o.type === "check"}
      <input
        type="checkbox"
        checked={values[o.name] === "true"}
        onchange={(e) => (values[o.name] = String(e.currentTarget.checked))}
      />
    {:else if o.type === "spin"}
      <input type="number" min={o.min} max={o.max} bind:value={values[o.name]} />
    {:else if o.type === "combo"}
      <select bind:value={values[o.name]}>
        {#each o.vars as v (v)}<option value={v}>{v}</option>{/each}
      </select>
    {:else if o.type === "string"}
      <input bind:value={values[o.name]} spellcheck="false" />
    {/if}
  </label>
{/snippet}

<Modal title="Engines" {onClose} width="720px">
  <div class="layout">
    <div class="list">
      {#if !analysis.engines.length}
        <p class="empty">
          No engines yet. Add a UCI engine such as Stockfish, or find the ones installed on this computer.
        </p>
      {/if}
      <ul role="listbox" aria-label="Engines">
        {#each analysis.engines as e (e.id)}
          <li>
            <button
              role="option"
              aria-selected={selected?.id === e.id}
              class:active={selected?.id === e.id}
              onclick={() => (selectedId = e.id)}
            >
              <strong>{e.name}</strong>
              {#if e.isDefault}<span class="tag">Default</span>{/if}
              <span class="path" title={e.path}>{e.path}</span>
            </button>
          </li>
        {/each}
      </ul>
      <div class="list-actions">
        <button disabled={busy} onclick={addEngine}>Add engine…</button>
        <button disabled={busy} onclick={detect}>Find engines</button>
      </div>
    </div>

    <div class="options">
      {#if selected}
        <div class="engine-actions">
          <button disabled={busy || selected.isDefault} onclick={() => analysis.setDefault(selected.id)}>
            Use by default
          </button>
          <button disabled={busy} onclick={() => analysis.remove(selected.id)}>Remove from list</button>
        </div>
        {#each common as o (o.name)}{@render field(o)}{/each}
        {#if others.length}
          <button class="toggle" onclick={() => (showAll = !showAll)}>
            {showAll ? "Hide" : "Show"} all options ({others.length})
          </button>
          {#if showAll}
            <div class="all">
              {#each others as o (o.name)}{@render field(o)}{/each}
            </div>
          {/if}
        {/if}
      {/if}
    </div>
  </div>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button class="primary" disabled={busy || !selected} onclick={save}>Save options</button>
  {/snippet}
</Modal>

<style>
  .layout {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 18px;
    min-height: 260px;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li button {
    display: grid;
    width: 100%;
    gap: 1px;
    text-align: left;
    white-space: normal;
    border-color: var(--rule);
    margin-bottom: 4px;
  }

  li button.active {
    background: var(--felt-soft);
    border-color: var(--felt);
  }

  .tag {
    justify-self: start;
    padding: 0 6px;
    border-radius: 8px;
    background: var(--brass-soft);
    color: var(--brass);
    font-size: 11px;
    font-weight: 600;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-soft);
    font-size: 12px;
  }

  .empty {
    margin: 0 0 8px;
    color: var(--ink-soft);
  }

  .list-actions,
  .engine-actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .list-actions button,
  .engine-actions button,
  .toggle {
    border-color: var(--rule);
  }

  .engine-actions {
    margin-bottom: 12px;
  }

  .field {
    display: grid;
    grid-template-columns: 150px 1fr;
    align-items: center;
    gap: 10px;
    margin-bottom: 6px;
    font-size: 13px;
  }

  .field span {
    color: var(--ink-soft);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .field input[type="checkbox"] {
    justify-self: start;
  }

  .toggle {
    margin: 6px 0;
  }

  .all {
    max-height: 220px;
    overflow-y: auto;
    padding-right: 4px;
  }
</style>
