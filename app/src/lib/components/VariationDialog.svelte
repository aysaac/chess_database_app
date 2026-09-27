<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { VariationPolicyDto } from "../bindings/VariationPolicyDto";
  import { placeUnder } from "../popover";
  import { game } from "../stores/game.svelte";

  /** The move the popover opens under: the current move in the notation. */
  let { anchor }: { anchor: HTMLElement } = $props();

  const options: { policy: VariationPolicyDto; label: string; hint: string }[] = [
    {
      policy: "newVariation",
      label: "New variation",
      hint: "Keep the game as it is and add this move as a side line.",
    },
    {
      policy: "newMainLine",
      label: "New main line",
      hint: "Make this move the main line; the old one becomes a variation.",
    },
    { policy: "overwrite", label: "Overwrite", hint: "Replace the recorded move and everything after it." },
  ];

  let box: HTMLDivElement;
  let pos = $state<{ left: number; top: number } | null>(null);

  const choose = (p: VariationPolicyDto | null) => game.chooseVariation(p);

  onMount(() => {
    pos = placeUnder(anchor, box);
    // After the position is applied: a hidden button can't take focus.
    tick().then(() => box?.querySelector("button")?.focus());

    // A click anywhere else cancels, and doesn't reach the board underneath.
    const outside = (e: MouseEvent) => {
      if (box.contains(e.target as Node)) return;
      e.stopPropagation();
      e.preventDefault();
      choose(null);
    };
    window.addEventListener("mousedown", outside, true);
    return () => window.removeEventListener("mousedown", outside, true);
  });

  // Escape cancels; arrows and Tab cycle through the buttons.
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") return choose(null);
    const steps: Record<string, number> = { ArrowDown: 1, ArrowUp: -1, Tab: e.shiftKey ? -1 : 1 };
    const step = steps[e.key];
    if (!step) return;
    e.preventDefault();
    const buttons = [...box.querySelectorAll("button")];
    const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
    buttons[(i + step + buttons.length) % buttons.length].focus();
  }
</script>

<svelte:window onkeydown={onKey} />

<div
  class="popover"
  role="dialog"
  aria-modal="true"
  aria-label="A move is already recorded here"
  style:left="{pos?.left ?? 0}px"
  style:top="{pos?.top ?? 0}px"
  style:visibility={pos ? null : "hidden"}
  bind:this={box}
>
  <p>A move is already recorded here</p>
  {#each options as o, i (o.policy)}
    <button class:primary={i === 0} title={o.hint} onclick={() => choose(o.policy)}>{o.label}</button>
  {/each}
  <hr />
  <button onclick={() => choose(null)}>Cancel</button>
</div>

<style>
  .popover {
    position: fixed;
    z-index: 100;
    width: 200px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 18%, transparent);
  }

  p {
    margin: 2px 4px 4px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  button {
    text-align: left;
  }

  hr {
    width: 100%;
    border: 0;
    border-top: 1px solid var(--rule);
    margin: 3px 0;
  }
</style>
