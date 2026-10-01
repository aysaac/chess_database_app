<script lang="ts">
  import { tick } from "svelte";
  import { ROOT_NODE, game } from "../stores/game.svelte";
  import { panels } from "../stores/panels.svelte";
  import CommentEditor from "./CommentEditor.svelte";
  import MoveMenu from "./MoveMenu.svelte";
  import type { MoveTarget } from "../types";
  import NotationLine from "./NotationLine.svelte";

  // The variation popover and the next-move picker open under the current
  // move here, but they're drawn by MovePopovers.svelte, outside the
  // Notation tab, so they also open while another panel tab shows (UI-19).

  let pane: HTMLDivElement;
  let menu = $state<MoveTarget | null>(null);

  // Double-clicking a comment edits it in place (the move menu is the keyboard path).
  $effect(() => {
    const edit = (e: MouseEvent) => {
      const el = (e.target as HTMLElement).closest<HTMLElement>("[data-comment]");
      if (!el) return;
      const [node, slot] = el.dataset.comment!.split(":");
      game.editComment(Number(node), slot === "before" ? "before" : "after");
    };
    pane.addEventListener("dblclick", edit);
    return () => pane.removeEventListener("dblclick", edit);
  });

  // Keep the current move in view, also when the tab shows again after
  // moves made while another tab showed.
  $effect(() => {
    const cursor = game.view?.cursor;
    void game.notation;
    if (!panels.isShowing("notation")) return;
    tick().then(() => {
      pane?.querySelector(`[data-node="${cursor}"]`)?.scrollIntoView({ block: "nearest" });
    });
  });
</script>

<div class="notation" data-notation bind:this={pane}>
  {#if game.notation}
    {#if game.editingComment?.node === ROOT_NODE}
      <p class="intro"><CommentEditor initial={game.notation.rootComment} /></p>
    {:else if game.notation.rootComment}
      <p class="intro" data-comment="{ROOT_NODE}:after" title="Double-click to edit">
        {game.notation.rootComment}
      </p>
    {/if}
    {#if game.notation.moves.length}
      <div class="mainline">
        <NotationLine moves={game.notation.moves} depth={0} onContext={(t) => (menu = t)} />
        {#if game.view && game.view.headers.result !== "*"}
          <span class="result">{game.view.headers.result}</span>
        {/if}
      </div>
    {:else}
      <p class="empty">Make a move on the board to start recording.</p>
    {/if}
  {/if}
</div>

{#if menu}
  <MoveMenu target={menu} onClose={() => (menu = null)} />
{/if}

<style>
  .notation {
    height: 100%;
    overflow-y: auto;
    padding: 14px 18px 40px;
    font-family: var(--font-book);
    font-size: 16px;
    line-height: 1.75;
    user-select: text;
  }

  .mainline {
    max-width: 62ch;
    font-weight: 600;
  }

  .intro {
    margin: 0 0 10px;
    max-width: 62ch;
    font-style: italic;
    color: var(--ink-soft);
  }

  .result {
    margin-left: 6px;
    font-weight: 600;
  }

  .empty {
    color: var(--ink-soft);
    font-family: var(--font-ui);
    font-size: 14px;
  }
</style>
