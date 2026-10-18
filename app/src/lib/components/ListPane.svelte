<script lang="ts">
  import type { NameKindDto } from "../bindings/NameKindDto";
  import { db } from "../stores/db.svelte";
  import GameList from "./GameList.svelte";
  import NameList from "./NameList.svelte";

  // The bottom pane: the game list, or one of the name lists (DB-04).
  const tabs: { id: "games" | NameKindDto; label: string }[] = [
    { id: "games", label: "Games" },
    { id: "player", label: "Players" },
    { id: "event", label: "Events" },
    { id: "site", label: "Sites" },
    { id: "annotator", label: "Annotators" },
  ];
  let tab = $state<"games" | NameKindDto>("games");
  // Show games from elsewhere (a cross-table, DB-16) brings the Games tab forward.
  let gamesShown = db.gamesShown;
  $effect(() => {
    if (db.gamesShown === gamesShown) return;
    gamesShown = db.gamesShown;
    tab = "games";
  });

  function onTabKey(e: KeyboardEvent) {
    const i = tabs.findIndex((t) => t.id === tab);
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = tabs[(i + step + tabs.length) % tabs.length];
    if (next.id === "games" || db.info) tab = next.id;
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-tab="${tab}"]`)?.focus();
  }
</script>

<div class="pane">
  <div class="tabs" role="tablist" aria-label="Lists" tabindex="-1" onkeydown={onTabKey}>
    {#each tabs as t (t.id)}
      <button
        role="tab"
        data-tab={t.id}
        aria-selected={tab === t.id}
        tabindex={tab === t.id ? 0 : -1}
        disabled={t.id !== "games" && !db.info}
        onclick={() => (tab = t.id)}>{t.label}</button
      >
    {/each}
  </div>
  <div class="body" hidden={tab !== "games"}><GameList /></div>
  {#if tab !== "games"}
    <div class="body"><NameList kind={tab} onShowGames={() => (tab = "games")} /></div>
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  /* In a narrow list (docked on a side, UI-04) they scroll, like the game tabs. */
  .tabs {
    display: flex;
    gap: 2px;
    min-width: 0;
    padding: 3px 8px 0;
    overflow-x: auto;
    scrollbar-width: thin;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  .tabs button {
    flex: none;
    padding: 2px 12px 3px;
    border-radius: 4px 4px 0 0;
    border-bottom: 2px solid transparent;
  }

  .tabs button:focus-visible {
    outline-offset: -2px;
  }

  .tabs button[aria-selected="true"] {
    border-bottom-color: var(--felt);
    font-weight: 600;
  }

  .body {
    flex: 1;
    min-height: 0;
  }

  .body[hidden] {
    display: none;
  }
</style>
