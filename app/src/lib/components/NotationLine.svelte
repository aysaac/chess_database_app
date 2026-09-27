<script lang="ts">
  import type { CommentSlotDto } from "../bindings/CommentSlotDto";
  import type { NotationMoveDto } from "../bindings/NotationMoveDto";
  import { figurine, isMoveNag, nagSymbol } from "../format";
  import { game } from "../stores/game.svelte";
  import type { MoveTarget } from "../types";
  import CommentEditor from "./CommentEditor.svelte";
  import NotationLine from "./NotationLine.svelte";

  let {
    moves,
    depth,
    paren = false,
    onContext,
  }: {
    moves: NotationMoveDto[];
    depth: number;
    /** Wrap the line in parentheses (nested variations). */
    paren?: boolean;
    onContext: (t: MoveTarget) => void;
  } = $props();

  // Black moves repeat their number at the start of a line and after a
  // comment or variation interrupted the flow.
  function showNumber(i: number): boolean {
    const m = moves[i];
    if (m.white || i === 0) return true;
    const prev = moves[i - 1];
    return prev.commentAfter !== "" || prev.variations.length > 0;
  }

  function title(m: NotationMoveDto): string | undefined {
    const parts = [];
    if (m.clock) parts.push(`Clock ${m.clock}`);
    if (m.eval) parts.push(`Eval ${m.eval}`);
    return parts.length ? parts.join(", ") : undefined;
  }

  const editing = (id: number, slot: CommentSlotDto) =>
    game.editingComment?.node === id && game.editingComment.slot === slot;

  // A folded variation (UI-03) shows as a stub. Depth 0 is the main line.
  const folded = $derived(depth > 0 && game.isCollapsed(moves[0].id));

  function count(line: NotationMoveDto[]): number {
    return line.reduce((n, m) => n + 1 + m.variations.reduce((k, v) => k + count(v), 0), 0);
  }

  function onMenu(e: MouseEvent, id: number) {
    e.preventDefault();
    game.goto(id);
    onContext({ id, inVariation: depth > 0, x: e.clientX, y: e.clientY });
  }
</script>

<!--
  Spacing is explicit ({" "}) and the markup has no whitespace between nodes,
  so parentheses hug their moves: "(8...Nb6 9.Ng3)".
-->
{#if folded}<button
    class="stub"
    title="Show this variation ({count(moves)} {count(moves) === 1 ? 'move' : 'moves'})"
    onclick={() => game.expand(moves[0].id)}
    >{paren ? "(" : ""}{moves[0].moveNumber}{moves[0].white ? "." : "…"}{figurine(moves[0].san)} …{paren
      ? ")"
      : ""}</button
  >{:else}{#each moves as m, i (m.id)}{#if i === 0 && paren}<button
      class="paren open"
      aria-label="Collapse this variation"
      title="Collapse this variation"
      onclick={() => game.collapse([moves[0].id])}>(</button
    >{/if}{#if editing(m.id, "before")}<CommentEditor
      initial={m.commentBefore}
    />{" "}{:else if m.commentBefore}<span
      class="comment"
      data-comment="{m.id}:before">{m.commentBefore}</span
    >{" "}{/if}<button
    class="move"
    class:current={game.view?.cursor === m.id}
    data-node={m.id}
    title={title(m)}
    onclick={() => game.goto(m.id)}
    oncontextmenu={(e) => onMenu(e, m.id)}
    >{#if showNumber(i)}<span class="number">{m.moveNumber}{m.white ? "." : "…"}</span>{/if}<span
      class="san">{figurine(m.san)}</span
    >{#each m.nags.filter(isMoveNag) as nag (nag)}<span class="nag">{nagSymbol(nag)}</span
      >{/each}</button
  >{#each m.nags.filter((n) => !isMoveNag(n)) as nag (nag)}{" "}<span class="nag"
      >{nagSymbol(nag)}</span
    >{/each}{#if editing(m.id, "after")}{" "}<CommentEditor initial={m.commentAfter} />{:else if m.commentAfter}{" "}<span
      class="comment"
      data-comment="{m.id}:after">{m.commentAfter}</span
    >{/if}{#if m.variations.length}{#if depth === 0}<div class="variations">
        {#each m.variations as line (line[0].id)}
          <div class="variation">
            {#if !game.isCollapsed(line[0].id)}<button
                class="fold"
                aria-label="Collapse this variation"
                title="Collapse this variation"
                onclick={() => game.collapse([line[0].id])}
              ></button>{/if}<NotationLine moves={line} depth={1} {onContext} />
          </div>
        {/each}
      </div>{:else}{#each m.variations as line (line[0].id)}{" "}<NotationLine
          moves={line}
          depth={depth + 1}
          paren
          {onContext}
        />{/each}{/if}{/if}{#if i === moves.length - 1}{#if paren}<span class="paren close">)</span
      >{/if}{:else}{" "}{/if}{/each}{/if}

<style>
  .move {
    display: inline;
    padding: 1px 3px;
    margin: 0 1px;
    border: 0;
    border-radius: 3px;
    font: inherit;
    line-height: inherit;
    white-space: nowrap;
  }

  .move:hover {
    background: var(--felt-soft);
  }

  .move.current {
    background: var(--felt);
    color: var(--paper);
  }

  .move.current .nag,
  .move.current .number {
    color: inherit;
  }

  .number {
    color: var(--ink-soft);
    margin-right: 2px;
    font-variant-numeric: tabular-nums;
  }

  /* Figurine glyphs fall back per character to a font that has them. */
  .san {
    font-family: "Source Serif 4 Variable", var(--font-figurine), serif;
  }

  .nag {
    color: var(--brass);
    font-weight: 600;
  }

  .comment {
    font-style: italic;
    color: var(--ink-soft);
    white-space: normal;
  }

  .variations {
    margin: 4px 0 6px;
  }

  .variation {
    position: relative;
    font-weight: 400;
    font-size: 0.94em;
    margin: 2px 0 2px 10px;
    padding-left: 10px;
    border-left: 2px solid color-mix(in srgb, var(--brass) 55%, transparent);
  }

  .paren {
    color: var(--ink-soft);
  }

  /* The opening parenthesis of a nested variation folds it. */
  button.paren {
    display: inline;
    padding: 0;
    margin: 0;
    border: 0;
    background: none;
    font: inherit;
    line-height: inherit;
    cursor: pointer;
  }

  button.paren:hover {
    color: var(--brass);
  }

  /* The left rule of a top-level variation folds it. */
  .fold {
    position: absolute;
    left: -8px;
    top: 0;
    bottom: 0;
    width: 12px;
    padding: 0;
    border: 0;
    border-radius: 2px;
    background: transparent;
    cursor: pointer;
  }

  .fold:hover {
    background: color-mix(in srgb, var(--brass) 30%, transparent);
  }

  .stub {
    display: inline;
    padding: 0 4px;
    margin: 0 1px;
    border: 1px dashed color-mix(in srgb, var(--brass) 60%, transparent);
    border-radius: 3px;
    background: none;
    font: inherit;
    line-height: inherit;
    color: var(--ink-soft);
    white-space: nowrap;
  }

  .stub:hover {
    background: var(--felt-soft);
  }

  /* Cancel the move button's side padding where a parenthesis touches it. */
  .paren.open + .move {
    margin-left: -3px;
  }

  .move + .paren.close {
    margin-left: -3px;
  }
</style>
