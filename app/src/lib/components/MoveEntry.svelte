<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../api";
  import type { LegalMoveDto } from "../bindings/LegalMoveDto";
  import { figurine } from "../format";
  import { matchMove } from "../moveInput";
  import { game } from "../stores/game.svelte";
  import { notices } from "../stores/notify.svelte";

  /**
   * Typing a move on the board (UI-02). Opened by typing a letter or digit
   * while the board or notation has focus; Enter plays, Escape or clicking
   * away closes.
   */
  let input: HTMLInputElement;
  let moves = $state<LegalMoveDto[] | null>(null);
  let text = $state(game.typing ?? "");
  /** Enter was pressed before the legal moves arrived. */
  let submitted = false;
  const match = $derived(moves ? matchMove(text, moves) : null);
  const MAX_SHOWN = 8;
  const shown = $derived(match?.candidates.slice(0, MAX_SHOWN) ?? []);

  onMount(() => {
    input.focus();
    input.setSelectionRange(text.length, text.length);
    const session = game.session;
    if (session === null) return close();
    api
      .legalMoves(session)
      .then((m) => {
        moves = m;
        if (submitted) submit();
      })
      .catch((e) => {
        notices.error(e);
        close();
      });
  });

  function close() {
    game.typing = null;
  }

  function play(move: LegalMoveDto) {
    close();
    void game.playUci(move.uci);
  }

  function submit() {
    submitted = !moves;
    const move = match?.move ?? (match?.candidates.length === 1 ? match.candidates[0] : null);
    if (move) play(move);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }

  const status = $derived.by(() => {
    if (!moves || !match) return "";
    if (!text.trim()) return "Type a move, such as Nf3 or g1f3";
    const only = match.move ?? (match.candidates.length === 1 ? match.candidates[0] : null);
    if (only) return `Enter plays ${figurine(only.san)}`;
    if (!match.candidates.length) return "No legal move matches";
    return "Keep typing to choose one";
  });
</script>

<div class="entry" role="group" aria-label="Type a move">
  {#if shown.length > 1}
    <div class="candidates">
      {#each shown as m (m.uci)}
        <button
          tabindex="-1"
          title="Play {m.san}"
          onmousedown={(e) => e.preventDefault()}
          onclick={() => play(m)}>{figurine(m.san)}</button
        >
      {/each}
      {#if match && match.candidates.length > MAX_SHOWN}<span class="more">…</span>{/if}
    </div>
  {/if}
  <input
    bind:this={input}
    bind:value={text}
    aria-label="Move"
    aria-describedby="move-entry-status"
    spellcheck="false"
    autocomplete="off"
    onkeydown={onKey}
    onblur={close}
  />
  <div id="move-entry-status" class="status" aria-live="polite">{status}</div>
</div>

<style>
  .entry {
    position: absolute;
    left: 50%;
    bottom: 12%;
    transform: translateX(-50%);
    z-index: 12;
    width: min(260px, 90%);
    padding: 8px;
    background: var(--paper);
    border: 1px solid var(--rule);
    border-radius: 6px;
    box-shadow: 0 6px 20px color-mix(in srgb, var(--ink) 25%, transparent);
  }

  input {
    width: 100%;
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
    font-size: 18px;
    font-weight: 600;
  }

  .candidates {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 6px;
  }

  .candidates button {
    padding: 0 7px;
    border-color: var(--rule);
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
    font-weight: 600;
  }

  .more {
    color: var(--ink-soft);
  }

  .status {
    margin-top: 4px;
    font-size: 12px;
    color: var(--ink-soft);
  }
</style>
