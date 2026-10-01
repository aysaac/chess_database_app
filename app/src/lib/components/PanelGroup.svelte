<script lang="ts">
  import { PANEL_TITLES, fitsContent, panels, type Group } from "../stores/panels.svelte";
  import Panel from "./Panel.svelte";

  // A group of panels shown as tabs (UI-19). These are panel tabs, not the
  // game tabs above the board (GameTabs.svelte). Every panel stays mounted,
  // so it keeps its scroll position and state; a hidden one does no work
  // (see `panels.isShowing`). A group of one panel has no tab strip.
  let { group }: { group: Group } = $props();

  // The WAI-ARIA tabs pattern: ←/→ go to the previous or next tab and show
  // it, Home and End to the first and last. While the focus is in the tab
  // strip, these keys don't step through the game.
  function onKey(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return;
    const ids = group.panels;
    const i = ids.indexOf(group.active);
    const to: Record<string, number> = { ArrowLeft: i - 1, ArrowRight: i + 1, Home: 0, End: ids.length - 1 };
    if (!(e.key in to)) return;
    e.preventDefault();
    e.stopPropagation();
    const id = ids[(to[e.key] + ids.length) % ids.length];
    panels.show(id);
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-panel="${id}"]`)?.focus();
  }
</script>

<div class="group" class:fit={fitsContent(group)} data-panel-group>
  {#if group.panels.length > 1}
    <div class="tabs" role="tablist" aria-label="Panels" tabindex="-1" onkeydown={onKey}>
      {#each group.panels as id (id)}
        <button
          role="tab"
          id="panel-tab-{id}"
          data-panel={id}
          aria-selected={id === group.active}
          aria-controls="panel-{id}"
          tabindex={id === group.active ? 0 : -1}
          onclick={() => panels.show(id)}
        >
          <span class="label">{PANEL_TITLES[id]}</span>
          <!-- Keeps the width of the bold label, so the tabs don't shift when one is chosen. -->
          <span class="label bold" aria-hidden="true">{PANEL_TITLES[id]}</span>
        </button>
      {/each}
    </div>
    {#each group.panels as id (id)}
      <div class="panel" role="tabpanel" id="panel-{id}" aria-labelledby="panel-tab-{id}" hidden={id !== group.active}>
        <Panel {id} />
      </div>
    {/each}
  {:else}
    <div class="panel" id="panel-{group.active}"><Panel id={group.active} /></div>
  {/if}
</div>

<style>
  .group {
    display: flex;
    flex-direction: column;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
  }

  .group.fit {
    flex: none;
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 3px 8px 0;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  /* Like the list's tabs (ListPane.svelte). */
  .tabs button {
    display: inline-grid;
    justify-items: center;
    padding: 2px 12px 3px;
    border-radius: 4px 4px 0 0;
    border-bottom: 2px solid transparent;
  }

  .tabs button[aria-selected="true"] {
    border-bottom-color: var(--felt);
    font-weight: 600;
  }

  .label {
    grid-area: 1 / 1;
  }

  .label.bold {
    visibility: hidden;
    font-weight: 600;
  }

  .panel {
    flex: 1;
    min-height: 0;
  }

  .panel[hidden] {
    display: none;
  }
</style>
