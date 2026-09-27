<script lang="ts">
  import { analysis } from "../stores/analysis.svelte";
  import { game } from "../stores/game.svelte";
  import { prefs } from "../stores/prefs.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  const choices = [
    { ms: 300, label: "Quick (0.3 s per move)" },
    { ms: 1000, label: "Normal (1 s per move)" },
    { ms: 3000, label: "Thorough (3 s per move)" },
  ];
  let movetime = $state(prefs.get("blunderCheckMs", 1000));
  const plies = $derived(game.notation?.moves.length ?? 0);
  const minutes = $derived(Math.ceil(((plies + 1) * movetime) / 60000));

  function start() {
    prefs.set("blunderCheckMs", movetime);
    onClose();
    analysis.blunderCheck(movetime);
  }
</script>

<Modal title="Blunder check" {onClose} width="460px">
  <p>
    The engine evaluates every move of the main line and marks inaccuracies (?!), mistakes (?) and
    blunders (??), adding its better line as a variation. It runs in the background; you can undo the
    result in one step.
  </p>
  <label>
    Engine time
    <select bind:value={movetime}>
      {#each choices as c (c.ms)}<option value={c.ms}>{c.label}</option>{/each}
    </select>
  </label>
  <p class="soft">
    {analysis.defaultEngine?.name ?? "The default engine"}, about {minutes}
    {minutes === 1 ? "minute" : "minutes"} for {Math.ceil(plies / 2)} moves.
  </p>
  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
    <button class="primary" disabled={!plies || !analysis.engines.length} onclick={start}>Start blunder check</button>
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 12px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  select {
    font-size: 14px;
  }

  .soft {
    margin-top: 10px;
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
