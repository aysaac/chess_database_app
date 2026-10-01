<script lang="ts">
  // The Reference tab's top games (OPEN-14 3c): the games reaching the
  // position, strongest first (average Elo of both players). Selecting one
  // (click, ↑/↓) shows it on the mini board, where ←/→ step through it;
  // Enter or a double-click opens it on the main board where it reaches the
  // position, as opening from the list does. The menu (right-click, the
  // Menu key or Shift+F10) also opens it in a new tab.
  import type { TopGameDto } from "../bindings/TopGameDto";
  import { resultLabel } from "../format";
  import { reference } from "../stores/reference.svelte";

  let { games }: { games: TopGameDto[] } = $props();

  let grid: HTMLElement;
  let menu = $state<{ x: number; y: number; game: TopGameDto } | null>(null);

  const index = $derived(games.findIndex((g) => g.id === reference.selected));

  function select(i: number) {
    const g = games[Math.max(0, Math.min(games.length - 1, i))];
    if (!g) return;
    void reference.selectGame(g);
    queueMicrotask(() => document.getElementById(`top-game-${g.id}`)?.scrollIntoView({ block: "nearest" }));
  }

  function openMenu(g: TopGameDto, x: number, y: number) {
    menu = { x, y, game: g };
  }

  function onKey(e: KeyboardEvent) {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const g = games[index];
    const actions: Record<string, () => void> = {
      ArrowDown: () => select(index < 0 ? 0 : index + 1),
      ArrowUp: () => select(index < 0 ? 0 : index - 1),
      Home: () => select(0),
      End: () => select(games.length - 1),
      PageDown: () => select(index + 10),
      PageUp: () => select(index - 10),
      // The selected game steps on the mini board, not on the main one.
      ArrowLeft: () => reference.step("back"),
      ArrowRight: () => reference.step("forward"),
      Enter: () => g && void reference.openGame(g),
    };
    const contextKey = e.key === "ContextMenu" || (e.shiftKey && e.key === "F10");
    if (contextKey && g) {
      const r = document.getElementById(`top-game-${g.id}`)?.getBoundingClientRect();
      if (r) openMenu(g, r.left + 40, r.bottom);
    } else if (e.shiftKey || !(e.key in actions)) {
      return;
    } else {
      actions[e.key]();
    }
    e.preventDefault();
    e.stopPropagation();
  }

  function closeMenu(refocus = true) {
    menu = null;
    if (refocus) grid?.focus();
  }

  function menuKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    e.stopPropagation();
    const items = [...(e.currentTarget as HTMLElement).querySelectorAll<HTMLElement>("[role='menuitem']")];
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
  }

  function open(newTab: boolean) {
    const g = menu?.game;
    closeMenu(false);
    if (g) void reference.openGame(g, newTab);
  }
</script>

<svelte:window onmousedown={() => menu && closeMenu(false)} />

<div class="top-games">
  <div class="title" id="top-games-title">Top games</div>
  <div class="scroll">
    <table
      class="games"
      role="grid"
      aria-labelledby="top-games-title"
      aria-activedescendant={index >= 0 ? `top-game-${games[index].id}` : undefined}
      tabindex="0"
      bind:this={grid}
      onkeydown={onKey}
    >
      <thead>
        <tr>
          <th scope="col">White</th>
          <th scope="col" class="num">Elo</th>
          <th scope="col">Black</th>
          <th scope="col" class="num">Elo</th>
          <th scope="col">Result</th>
          <th scope="col" class="num">Year</th>
          <th scope="col" class="event">Event</th>
        </tr>
      </thead>
      <tbody>
        {#each games as g, i (g.id)}
          <tr
            id="top-game-{g.id}"
            aria-selected={i === index}
            class:selected={i === index}
            onclick={() => select(i)}
            ondblclick={() => reference.openGame(g)}
            oncontextmenu={(e) => {
              e.preventDefault();
              select(i);
              openMenu(g, e.clientX, e.clientY);
            }}
          >
            <td>{g.white}</td>
            <td class="num">{g.whiteElo ?? ""}</td>
            <td>{g.black}</td>
            <td class="num">{g.blackElo ?? ""}</td>
            <td>{resultLabel(g.result)}</td>
            <td class="num">{g.year ?? ""}</td>
            <td class="event">{g.event}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

{#if menu}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    aria-label="Top game"
    style:left="{Math.min(menu.x, window.innerWidth - 200)}px"
    style:top="{Math.min(menu.y, window.innerHeight - 80)}px"
    onmousedown={(e) => e.stopPropagation()}
    onkeydown={menuKey}
    {@attach (el) => el.querySelector<HTMLElement>("[role='menuitem']")?.focus()}
  >
    <button role="menuitem" onclick={() => open(false)}>Open game</button>
    <button role="menuitem" onclick={() => open(true)}>Open in a new tab</button>
  </div>
{/if}

<style>
  .top-games {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-width: 0;
    min-height: 0;
  }

  .title {
    padding: 3px 10px 3px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
    color: var(--ink-soft);
  }

  .scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  table:focus-visible {
    outline-offset: -2px;
  }

  th,
  td {
    padding: 1px 8px 1px 0;
    text-align: left;
    white-space: nowrap;
    max-width: 14em;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  th:first-child,
  td:first-child {
    padding-left: 14px;
  }

  thead th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--paper);
    color: var(--ink-soft);
    font-weight: 500;
  }

  tbody tr {
    cursor: default;
  }

  tbody tr:hover {
    background: color-mix(in srgb, var(--felt-soft) 50%, transparent);
  }

  tbody tr.selected {
    background: var(--felt-soft);
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  /* The event takes what's left. */
  .event {
    width: 100%;
    max-width: 0;
  }

  .menu {
    position: fixed;
    z-index: 50;
    display: flex;
    flex-direction: column;
    min-width: 180px;
    padding: 4px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.18);
  }

  .menu button {
    text-align: left;
  }
</style>
