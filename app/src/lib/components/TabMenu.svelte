<script lang="ts">
  import { tick } from "svelte";
  import { dock } from "../stores/dock.svelte";
  import {
    PANEL_TITLES,
    SIDES,
    SIDE_NAMES,
    allGroups,
    groupLabel,
    panels,
    type PanelId,
    type Side,
  } from "../stores/panels.svelte";

  // A panel tab's menu (UI-04), the keyboard's way to dock: right-click a
  // tab, or press the context-menu key or Shift+F10 on it. Move to ▸ a side
  // (a new group there; on its own side, next to its group) or Move into ▸
  // another group. The menu pattern: ↑/↓, Home/End, Enter; Escape closes it
  // and gives the focus back to the tab.
  const menu = $derived(dock.menu);
  const panel = $derived(menu?.panel ?? null);
  const own = $derived(panel ? panels.groupOf(panel) : null);
  const ownSide = $derived(panel ? panels.sideOf(panel) : null);
  const others = $derived(own ? allGroups(panels.layout).filter((g) => g.id !== own.id) : []);

  const tabOf = (p: PanelId) => document.querySelector<HTMLElement>(`[role='tab'][data-panel="${p}"]`);

  /** Closes the menu; the focus goes to the tab, wherever it is now. */
  async function close(p: PanelId | null) {
    dock.closeMenu();
    if (!p) return;
    await tick();
    tabOf(p)?.focus();
  }

  function moveTo(side: Side) {
    if (!panel || !own) return;
    const p = panel;
    const groups = panels.layout.zones[side].groups;
    // On its own side, the new group goes next to the one it leaves.
    const slot = side === ownSide ? groups.findIndex((g) => g.id === own.id) + 1 : groups.length;
    panels.move(p, { kind: "side", side, slot });
    void close(p);
  }

  function moveInto(group: string) {
    if (!panel) return;
    const p = panel;
    panels.move(p, { kind: "group", group, index: Infinity });
    void close(p);
  }

  function onKey(e: KeyboardEvent) {
    const items = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("[role='menuitem']:not(:disabled)")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    const to: Record<string, number> = {
      ArrowDown: i + 1,
      ArrowUp: i - 1,
      Home: 0,
      End: items.length - 1,
    };
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      void close(panel);
    } else if (e.key in to) {
      e.preventDefault();
      e.stopPropagation();
      items[(to[e.key] + items.length) % items.length]?.focus();
    } else if (!e.ctrlKey && !e.metaKey) {
      // Letters don't start a typed move (UI-02) while the menu is open.
      e.stopPropagation();
    }
  }

  /** Keeps the menu inside the window. */
  function place(el: HTMLElement) {
    if (!menu) return;
    const r = el.getBoundingClientRect();
    el.style.left = `${Math.max(4, Math.min(menu.x, window.innerWidth - r.width - 4))}px`;
    el.style.top = `${Math.max(4, Math.min(menu.y, window.innerHeight - r.height - 4))}px`;
    el.querySelector<HTMLElement>("[role='menuitem']:not(:disabled)")?.focus();
  }
</script>

<svelte:window
  onmousedown={(e) => {
    if (menu && !(e.target as HTMLElement).closest?.(".tab-menu")) dock.closeMenu();
  }}
/>

{#if menu && panel && own}
  <div
    class="tab-menu"
    role="menu"
    tabindex="-1"
    aria-label="Move {PANEL_TITLES[panel]}"
    data-popup
    onkeydown={onKey}
    {@attach place}
  >
    <div class="heading" role="presentation">Move to</div>
    {#each SIDES as side (side)}
      <button
        role="menuitem"
        data-side={side}
        disabled={side === ownSide && own.panels.length === 1}
        title={side === ownSide && own.panels.length === 1 ? `${PANEL_TITLES[panel]} is docked there` : undefined}
        onclick={() => moveTo(side)}>{SIDE_NAMES[side]}</button
      >
    {/each}
    <hr />
    <div class="heading" role="presentation">Move into</div>
    {#each others as g (g.id)}
      <button role="menuitem" data-group={g.id} onclick={() => moveInto(g.id)}>{groupLabel(g)}</button>
    {:else}
      <p class="none">No other group</p>
    {/each}
  </div>
{/if}

<style>
  .tab-menu {
    position: fixed;
    left: 0;
    top: 0;
    z-index: 60;
    display: flex;
    flex-direction: column;
    min-width: 190px;
    max-width: 360px;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .heading {
    padding: 2px 9px 1px;
    color: var(--ink-soft);
    font-size: 12px;
  }

  button {
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }

  hr {
    width: 100%;
    margin: 4px 0;
    border: 0;
    border-top: 1px solid var(--rule);
  }

  .none {
    margin: 2px 9px 4px;
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
