<script lang="ts">
  import { onMount } from "svelte";

  let { color, onPick }: { color: "white" | "black"; onPick: (role: string | null) => void } =
    $props();

  const roles = [
    { role: "q", name: "queen", label: "Queen" },
    { role: "r", name: "rook", label: "Rook" },
    { role: "b", name: "bishop", label: "Bishop" },
    { role: "n", name: "knight", label: "Knight" },
  ];

  let picker: HTMLDivElement;
  onMount(() => (picker.querySelector("button") as HTMLButtonElement | null)?.focus());

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") onPick(null);
    const hit = roles.find((r) => r.role === e.key.toLowerCase());
    if (hit) onPick(hit.role);
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="scrim" role="presentation" onclick={() => onPick(null)}>
  <!-- cg-wrap: chessground's piece images are scoped to it. -->
  <div class="picker cg-wrap" role="dialog" aria-label="Promote to" bind:this={picker}>
    {#each roles as r (r.role)}
      <button
        title={`${r.label} (${r.role.toUpperCase()})`}
        onclick={(e) => {
          e.stopPropagation();
          onPick(r.role);
        }}
      >
        <piece class={`${color} ${r.name}`}></piece>
      </button>
    {/each}
  </div>
</div>

<style>
  .scrim {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--paper) 55%, transparent);
    z-index: 10;
  }

  .picker {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 6px;
    padding: 8px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
  }

  button {
    width: 72px;
    height: 72px;
    padding: 6px;
    border-color: var(--rule);
  }

  .picker piece {
    position: static;
    display: block;
    width: 100%;
    height: 100%;
    transform: none;
  }
</style>
