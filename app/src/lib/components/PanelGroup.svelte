<script lang="ts">
  import { dock } from "../stores/dock.svelte";
  import { PANEL_TITLES, panels, type Group, type PanelId } from "../stores/panels.svelte";
  import DropShelf from "./DropShelf.svelte";
  import Panel from "./Panel.svelte";

  // A group of panels shown as tabs (UI-19). These are panel tabs, not the
  // game tabs above the board (GameTabs.svelte). Every panel stays mounted,
  // so it keeps its scroll position and state; a hidden one does no work
  // (see `panels.isShowing`). Every group has a tab strip, even with one
  // tab: tabs are dragged from it to another side or group (UI-04), and
  // dropped on it to join the group.
  let {
    group,
    fit = false,
    grow = 1,
  }: {
    group: Group;
    /** It takes its content's size (see Dock.svelte's `fits`). */
    fit?: boolean;
    /** Its part of the side's free room (`flex-grow`). */
    grow?: number;
  } = $props();

  // The WAI-ARIA tabs pattern: ←/→ go to the previous or next tab and show
  // it, Home and End to the first and last. While the focus is in the tab
  // strip, these keys don't step through the game. The context-menu key or
  // Shift+F10 opens the focused tab's menu (Move to, Move into).
  function onKey(e: KeyboardEvent) {
    const tab = (e.target as HTMLElement).closest<HTMLElement>("[role='tab']");
    if (tab && (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10"))) {
      e.preventDefault();
      e.stopPropagation();
      const r = tab.getBoundingClientRect();
      dock.openMenu(tab.dataset.panel as PanelId, r.left, r.bottom + 2);
      return;
    }
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

<div
  class="group"
  class:fit
  data-panel-group
  data-group-id={group.id}
  style:flex={fit ? "none" : `${grow} 1 0`}
>
  <div
    class="tabs"
    class:drop-over={dock.target?.strip === group.id}
    role="tablist"
    aria-label="Panels"
    tabindex="-1"
    data-dock-strip={group.id}
    onkeydown={onKey}
  >
    {#each group.panels as id (id)}
      <button
        role="tab"
        id="panel-tab-{id}"
        data-panel={id}
        class:dragged={dock.panel === id}
        aria-selected={id === group.active}
        aria-controls="panel-{id}"
        tabindex={id === group.active ? 0 : -1}
        title="{PANEL_TITLES[id]}: drag the tab to another side or group, or right-click it to move it"
        onclick={() => panels.show(id)}
        onpointerdown={(e) => dock.arm(e, id)}
        oncontextmenu={(e) => {
          e.preventDefault();
          dock.openMenu(id, e.clientX, e.clientY);
        }}
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
  <!-- Databases to drop dragged games on (QOL-04), over the notation's group. -->
  {#if panels.shelfHost === group.id}<DropShelf />{/if}
</div>

<style>
  .group {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .group.fit {
    flex: none;
  }

  .tabs {
    flex: none;
    display: flex;
    gap: 2px;
    min-width: 0;
    padding: 3px 8px 0;
    overflow: hidden;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  /* Like the list's tabs (ListPane.svelte). They shrink, with an ellipsis,
     when the group is narrower than its tabs. */
  .tabs button {
    display: inline-grid;
    grid-template-columns: minmax(0, max-content);
    justify-items: center;
    flex: 0 1 auto;
    min-width: 0;
    padding: 2px 12px 3px;
    border-radius: 4px 4px 0 0;
    border-bottom: 2px solid transparent;
  }

  .tabs button[aria-selected="true"] {
    border-bottom-color: var(--felt);
    font-weight: 600;
  }

  .tabs button.dragged {
    opacity: 0.45;
  }

  /* Inside the strip, which clips what sticks out. */
  .tabs button:focus-visible {
    outline-offset: -2px;
  }

  .label {
    grid-area: 1 / 1;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .label.bold {
    visibility: hidden;
    font-weight: 600;
  }

  /* While a tab is dragged (UI-04), every tab strip is a drop target, above
     the edge bands it may overlap. */
  :global(body.dragging-tab) .tabs {
    position: relative;
    z-index: 310;
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--felt) 45%, transparent);
  }

  :global(body.dragging-tab) .tabs.drop-over {
    background: var(--felt-soft);
    box-shadow: inset 0 0 0 2px var(--felt);
  }

  /* Each panel scrolls inside; nothing in it widens the window. */
  .panel {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .panel[hidden] {
    display: none;
  }
</style>
