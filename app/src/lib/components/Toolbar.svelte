<script lang="ts">
  import { analysis } from "../stores/analysis.svelte";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { tree } from "../stores/tree.svelte";
  import BlunderCheckDialog from "./BlunderCheckDialog.svelte";
  import BoardImageMenu from "./BoardImageMenu.svelte";
  import EngineDialog from "./EngineDialog.svelte";
  import FenDialog from "./FenDialog.svelte";
  import SetupEditor from "./SetupEditor.svelte";
  import RecentMenu from "./RecentMenu.svelte";

  let setup = $state(false);
  let gotoDialog = $state(false);
  let engineDialog = $state(false);
  let blunderDialog = $state(false);
</script>

<nav aria-label="Toolbar">
  <div class="group" role="group" aria-label="Database">
    <button onclick={() => db.createDialog()}>New database</button>
    <button onclick={() => db.openDialog()}>Open database</button>
    <RecentMenu />
    <button disabled={!db.info} onclick={() => db.importDialog()}>Import PGN</button>
    <button disabled={!db.info || db.info.games === 0} onclick={() => db.exportDialog()}>
      Export PGN
    </button>
  </div>
  <div class="group" role="group" aria-label="Game">
    <button onclick={() => game.newGame()}>New game</button>
    <button title="Set up a position, or paste a FEN" onclick={() => (setup = true)}>Set up position…</button>
    <button
      title="Paste a FEN or Lichess link to see its position and opening tree (Ctrl+V outside a text box)"
      onclick={() => (gotoDialog = true)}>Go to position…</button
    >
    <button
      class:primary={game.view?.dirty}
      disabled={!game.view}
      title="Save (Ctrl+S)"
      onclick={() => game.save()}>Save</button
    >
    <button disabled={!game.view || !db.info} title="Ctrl+Shift+S" onclick={() => game.saveAsNew()}>
      Save as new
    </button>
  </div>
  <div class="group" role="group" aria-label="Edit">
    <button disabled={!game.view?.canUndo} title="Undo (Ctrl+Z)" onclick={() => game.undo()}>Undo</button>
    <button disabled={!game.view?.canRedo} title="Redo (Ctrl+Shift+Z)" onclick={() => game.redo()}>Redo</button>
  </div>
  <div class="group" role="group" aria-label="Share">
    <button disabled={!game.view} onclick={() => game.copyPgn()}>Copy PGN</button>
    <button disabled={!game.view} onclick={() => game.copyFen()}>Copy FEN</button>
    <BoardImageMenu />
  </div>
  <div class="group" role="group" aria-label="Analysis">
    <button
      class:primary={analysis.status.running}
      aria-pressed={analysis.status.running}
      disabled={!game.view || analysis.starting}
      title="Start or stop the engine (Ctrl+Shift+A)"
      onclick={() => analysis.toggle()}>{analysis.starting ? "Starting…" : "Analyze"}</button
    >
    <button disabled={!game.notation?.moves.length} onclick={() => (blunderDialog = true)}>Blunder check…</button>
    <button class:primary={tree.visible} aria-pressed={tree.visible} onclick={() => tree.toggle()}>Opening tree</button>
    <button onclick={() => (engineDialog = true)}>Engines…</button>
  </div>
</nav>

{#if engineDialog}
  <EngineDialog onClose={() => (engineDialog = false)} />
{/if}

{#if blunderDialog}
  <BlunderCheckDialog onClose={() => (blunderDialog = false)} />
{/if}

{#if setup}
  <SetupEditor onClose={() => (setup = false)} />
{/if}

{#if gotoDialog}
  <FenDialog onClose={() => (gotoDialog = false)} />
{/if}

<style>
  nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 14px;
    padding: 5px 8px;
    border-bottom: 1px solid var(--rule);
    background: var(--panel);
  }

  .group {
    display: flex;
    gap: 0;
  }

  .group button {
    padding: 4px 8px;
  }
</style>
