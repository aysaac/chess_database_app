<script lang="ts">
  import { onMount } from "svelte";
  import type { NotationMoveDto } from "../bindings/NotationMoveDto";
  import { MOVE_NAGS, POSITION_NAGS, nagSymbol } from "../format";
  import { commentBefore, game } from "../stores/game.svelte";
  import type { MoveTarget } from "../types";

  let { target, onClose }: { target: MoveTarget; onClose: () => void } = $props();

  let menu: HTMLDivElement | undefined = $state();

  function find(lines: NotationMoveDto[][], id: number): NotationMoveDto | undefined {
    for (const line of lines) {
      for (const m of line) {
        if (m.id === id) return m;
        const hit = find(m.variations, id);
        if (hit) return hit;
      }
    }
    return undefined;
  }

  const move = $derived(game.notation ? find([game.notation.moves], target.id) : undefined);
  const nags = $derived(new Set(move?.nags ?? []));
  // Before a main-line move, that's the previous move's comment (or the intro).
  const before = $derived(game.notation ? commentBefore(game.notation, target.id) : undefined);

  // Keep the menu on screen.
  const left = $derived(Math.min(target.x, window.innerWidth - 260));
  const top = $derived(Math.min(target.y, window.innerHeight - 330));

  onMount(() => {
    const close = (e: MouseEvent) => {
      if (menu && !menu.contains(e.target as Node)) onClose();
    };
    setTimeout(() => window.addEventListener("mousedown", close));
    return () => window.removeEventListener("mousedown", close);
  });

  function run(f: () => unknown) {
    f();
    onClose();
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<div class="menu" role="menu" style:left="{left}px" style:top="{top}px" bind:this={menu}>
  <!-- The comment is typed in place in the notation (CommentEditor). -->
  <button role="menuitem" onclick={() => run(() => game.editComment(target.id, "after"))}
    >{move?.commentAfter ? "Edit" : "Add"} comment after move</button
  >
  <button role="menuitem" onclick={() => run(() => before && game.editComment(before.node, before.slot))}
    >{before?.text ? "Edit" : "Add"} comment before move</button
  >
  <div class="nags" role="group" aria-label="Move assessment">
    {#each MOVE_NAGS as n (n.nag)}
      <button
        class:on={nags.has(n.nag)}
        title={n.label}
        onclick={() => run(() => game.toggleNag(target.id, n.nag))}>{nagSymbol(n.nag)}</button
      >
    {/each}
  </div>
  <div class="nags" role="group" aria-label="Position assessment">
    {#each POSITION_NAGS as n (n.nag)}
      <button
        class:on={nags.has(n.nag)}
        title={n.label}
        onclick={() => run(() => game.toggleNag(target.id, n.nag))}>{nagSymbol(n.nag)}</button
      >
    {/each}
  </div>
  <hr />
  <button
    role="menuitem"
    disabled={!target.inVariation}
    onclick={() => run(() => game.promoteVariation(target.id))}>Promote variation</button
  >
  <button
    role="menuitem"
    disabled={!target.inVariation}
    onclick={() => run(() => game.deleteVariation(target.id))}>Delete variation</button
  >
  <button role="menuitem" onclick={() => run(() => game.deleteRemaining(target.id))}
    >Delete moves after this one</button
  >
</div>

<style>
  .menu {
    position: fixed;
    z-index: 50;
    width: 244px;
    display: flex;
    flex-direction: column;
    padding: 5px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  .menu > button {
    text-align: left;
  }

  .nags {
    display: grid;
    grid-template-columns: repeat(8, 1fr);
    gap: 2px;
    margin: 4px 0;
  }

  .nags button {
    padding: 3px 0;
    font-family: var(--font-book);
    font-weight: 600;
    color: var(--brass);
  }

  .nags button.on {
    background: var(--brass-soft);
    border-color: var(--brass);
  }

  hr {
    width: 100%;
    border: 0;
    border-top: 1px solid var(--rule);
    margin: 4px 0;
  }
</style>
