<script lang="ts">
  // The ribbon (UI-18): every command of the old toolbar in labelled groups,
  // like ChessBase's Home ribbon. It never wraps: as the window narrows,
  // groups from the right first drop their small buttons' labels (icons
  // stay, with the label as tooltip and accessible name), then collapse into
  // one button that opens the group as a dropdown. Widths come from a hidden
  // copy of every group in each form, so choosing a form never moves the
  // live buttons around (no flicker, no ResizeObserver loop).
  import { onMount, type Snippet } from "svelte";
  import type { IconName } from "../icons/lucide";
  import { pasteGame, pastePosition } from "../paste";
  import { analysis } from "../stores/analysis.svelte";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { panels } from "../stores/panels.svelte";
  import { tree } from "../stores/tree.svelte";
  import BlunderCheckDialog from "./BlunderCheckDialog.svelte";
  import BoardImageMenu from "./BoardImageMenu.svelte";
  import EngineDialog from "./EngineDialog.svelte";
  import FenDialog from "./FenDialog.svelte";
  import RecentMenu from "./RecentMenu.svelte";
  import ReferenceMenu from "./ReferenceMenu.svelte";
  import RibbonButton from "./RibbonButton.svelte";
  import RibbonDropdown from "./RibbonDropdown.svelte";
  import SetupEditor from "./SetupEditor.svelte";

  let setup = $state(false);
  let gotoDialog = $state(false);
  let engineDialog = $state(false);
  let blunderDialog = $state(false);

  type Form = "full" | "compact" | "collapsed";
  type Group = { id: string; label: string; icon: IconName; body: Snippet<[boolean]> };

  const groups: Group[] = [
    { id: "clipboard", label: "Clipboard", icon: "clipboard", body: clipboardGroup },
    { id: "game", label: "Game", icon: "chess-knight", body: gameGroup },
    { id: "reference", label: "Reference", icon: "library", body: referenceGroup },
    { id: "analysis", label: "Analysis", icon: "cpu", body: analysisGroup },
    { id: "database", label: "Database", icon: "database", body: databaseGroup },
    { id: "view", label: "View", icon: "panel-right", body: viewGroup },
  ];
  const N = groups.length;

  /**
   * How narrow the ribbon is: at level L the last min(L, N) groups are
   * compact, and of those the last L - N are collapsed.
   */
  let level = $state(0);

  function formAt(l: number, i: number): Form {
    const fromRight = N - 1 - i;
    if (fromRight < l - N) return "collapsed";
    return fromRight < l ? "compact" : "full";
  }

  let bar: HTMLElement;
  let measure: HTMLElement;

  /** The roomiest level whose groups fit the ribbon's width. */
  function fit() {
    const width = (id: string, form: Form) =>
      measure.querySelector(`[data-group="${id}"][data-form="${form}"]`)?.getBoundingClientRect().width ?? 0;
    const widths = groups.map((g) => ({
      full: width(g.id, "full"),
      compact: width(g.id, "compact"),
      collapsed: width(g.id, "collapsed"),
    }));
    const style = getComputedStyle(bar);
    const room = bar.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight) - 1;
    for (let l = 0; l <= 2 * N; l++) {
      const total = widths.reduce((sum, w, i) => sum + w[formAt(l, i)], 0);
      if (total <= room) {
        level = l;
        return;
      }
    }
    level = 2 * N;
  }

  onMount(() => {
    // The window's width, and the groups' own widths (fonts loading, a label
    // changing) both change the fit.
    const observer = new ResizeObserver(fit);
    observer.observe(bar);
    observer.observe(measure.firstElementChild as Element);
    return () => observer.disconnect();
  });

  /** Find games in the reference database, or the open one when none is set. */
  const findTarget = $derived(tree.referenceTarget);
</script>

{#snippet clipboardGroup(compact: boolean)}
  <RibbonButton
    size="large"
    icon="clipboard-paste"
    label="Paste game"
    title="Open the PGN game on the clipboard as a new game (Ctrl+V outside a text box)"
    onclick={pasteGame}
  />
  <div class="col">
    <RibbonButton
      icon="grid-3x3"
      label="Paste position"
      title="Go to the position of the FEN or Lichess link on the clipboard (Ctrl+V outside a text box)"
      {compact}
      onclick={pastePosition}
    />
    <RibbonButton
      icon="copy"
      label="Copy game"
      title="Copy the game as PGN"
      {compact}
      disabled={!game.view}
      onclick={() => game.copyPgn()}
    />
    <RibbonButton
      icon="clipboard-copy"
      label="Copy position"
      title="Copy the position as FEN"
      {compact}
      disabled={!game.view}
      onclick={() => game.copyFen()}
    />
  </div>
  <div class="col"><BoardImageMenu {compact} /></div>
{/snippet}

{#snippet gameGroup(compact: boolean)}
  <RibbonDropdown
    class="new-game"
    icon="file-plus"
    label="More ways to start a game"
    title="New game or set up a position"
    menuLabel="New game"
    mainLabel="New game"
    mainTitle="New game (Ctrl+N)"
    onmain={() => game.newGame()}
  >
    <button role="menuitem" onclick={() => game.newGame()}>New game (Ctrl+N)</button>
    <button role="menuitem" title="Set up a position, or paste a FEN" onclick={() => (setup = true)}
      >Set up position…</button
    >
  </RibbonDropdown>
  <div class="col">
    <RibbonButton
      icon="save"
      label="Save"
      title="Save (Ctrl+S)"
      {compact}
      primary={game.view?.dirty}
      disabled={!game.view}
      onclick={() => game.save()}
    />
    <RibbonButton
      icon="save-plus"
      label="Save as new"
      title="Save as new (Ctrl+Shift+S)"
      {compact}
      disabled={!game.view || !db.info}
      onclick={() => game.saveAsNew()}
    />
  </div>
  <div class="col">
    <RibbonButton
      icon="undo-2"
      label="Undo"
      title="Undo (Ctrl+Z)"
      {compact}
      disabled={!game.view?.canUndo}
      onclick={() => game.undo()}
    />
    <RibbonButton
      icon="redo-2"
      label="Redo"
      title="Redo (Ctrl+Shift+Z)"
      {compact}
      disabled={!game.view?.canRedo}
      onclick={() => game.redo()}
    />
  </div>
{/snippet}

{#snippet referenceGroup(compact: boolean)}
  <div class="col">
    <ReferenceMenu {compact} />
    <div class="row" class:stack={!compact}>
      <RibbonButton
        icon="database-search"
        label="Find games"
        title={findTarget?.reference && findTarget.handle !== db.info?.handle
          ? `Open ${findTarget.name} in the game list with the games that reach this position`
          : "Show the games that reach this position in the game list"}
        {compact}
        disabled={!findTarget || !game.view || db.searching}
        onclick={() => tree.findGames(false, findTarget)}
      />
      <RibbonButton
        icon="crosshair"
        label="Go to position…"
        title="Paste a FEN or Lichess link to see its position and opening tree (Ctrl+V outside a text box)"
        {compact}
        onclick={() => (gotoDialog = true)}
      />
    </div>
  </div>
{/snippet}

{#snippet analysisGroup(compact: boolean)}
  <div class="col">
    <RibbonButton
      icon="cpu"
      label={analysis.starting ? "Starting…" : "Analyze"}
      title="Start or stop the engine (Ctrl+Shift+A)"
      {compact}
      aria-pressed={analysis.status.running}
      disabled={!game.view || analysis.starting}
      onclick={() => analysis.toggle()}
    />
    <RibbonButton
      icon="badge-alert"
      label="Blunder check…"
      title="Check every move of the game with the engine"
      {compact}
      disabled={!game.notation?.moves.length}
      onclick={() => (blunderDialog = true)}
    />
    <RibbonButton
      icon="settings-2"
      label="Engines…"
      title="Add engines, choose the default and set its options"
      {compact}
      onclick={() => (engineDialog = true)}
    />
  </div>
{/snippet}

{#snippet databaseGroup(compact: boolean)}
  <div class="col">
    <RibbonButton
      icon="database-plus"
      label="New database"
      title="Create a database"
      {compact}
      onclick={() => db.createDialog()}
    />
    <RibbonButton
      icon="folder-open"
      label="Open database"
      title="Open a database (.chessdb folder)"
      {compact}
      onclick={() => db.openDialog()}
    />
    <RecentMenu {compact} />
  </div>
  <div class="col">
    <RibbonButton
      icon="file-down"
      label="Import games"
      title="Add the games of a PGN file or a ChessBase database (.cbh) to the open database"
      {compact}
      disabled={!db.info}
      onclick={() => db.importDialog()}
    />
    <RibbonButton
      icon="file-up"
      label="Export PGN"
      title="Save the games of the list (filtered or all) as a PGN file"
      {compact}
      disabled={!db.info || db.info.games === 0}
      onclick={() => db.exportDialog()}
    />
  </div>
{/snippet}

{#snippet viewGroup(compact: boolean)}
  <div class="col">
    <RibbonButton
      icon="history"
      label="Notation"
      title="Show the Notation tab: the game header, its moves and the eval graph"
      {compact}
      aria-pressed={panels.isShowing("notation")}
      onclick={() => panels.show("notation")}
    />
    <RibbonButton
      icon="network"
      label="Opening tree"
      title="Show the Opening tree tab: the moves played from this position"
      {compact}
      aria-pressed={panels.isShowing("tree")}
      onclick={() => panels.show("tree")}
    />
    <RibbonButton
      icon="library"
      label="Reference"
      title="Show the Reference tab: the moves, main variations and top games of the reference database"
      {compact}
      aria-pressed={panels.isShowing("reference")}
      onclick={() => panels.show("reference")}
    />
  </div>
  <div class="col">
    <RibbonButton
      icon="layout-dashboard"
      label="Reset layout"
      title="Put the panels back where they start: Notation, Opening tree and Reference on the right, the game list at the bottom"
      {compact}
      onclick={() => panels.reset()}
    />
  </div>
{/snippet}

{#snippet group(g: Group, form: Form)}
  {#if form === "collapsed"}
    <div class="group collapsed" data-group={g.id} data-form={form}>
      <RibbonDropdown size="group" kind="panel" icon={g.icon} label={g.label} title="{g.label} commands">
        <div class="body in-panel">{@render g.body(false)}</div>
      </RibbonDropdown>
    </div>
  {:else}
    <div class="group" role="group" aria-label={g.label} data-group={g.id} data-form={form}>
      <div class="body">{@render g.body(form === "compact")}</div>
      <div class="caption" aria-hidden="true">{g.label}</div>
    </div>
  {/if}
{/snippet}

<section class="ribbon" aria-label="Ribbon" bind:this={bar}>
  <div class="groups">
    {#each groups as g, i (g.id)}
      {@render group(g, formAt(level, i))}
    {/each}
  </div>
  <!-- Every group in every form, unseen, to measure. -->
  <div class="measure" aria-hidden="true" inert bind:this={measure}>
    <div class="groups">
      {#each groups as g (g.id)}
        {@render group(g, "full")}
        {@render group(g, "compact")}
        {@render group(g, "collapsed")}
      {/each}
    </div>
  </div>
</section>

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
  .ribbon {
    position: relative;
    /* Never widens the window's grid, whatever the groups need. Not
       clipped: the dropdowns hang out of it. */
    min-width: 0;
    padding: 0 4px;
    border-bottom: 1px solid var(--rule);
    background: var(--panel);
  }

  .groups {
    display: flex;
    height: 96px;
  }

  .group {
    flex: none;
    display: flex;
    flex-direction: column;
    padding: 4px 5px 0;
    border-right: 1px solid var(--rule);
  }

  .group:last-child {
    border-right: 0;
  }

  .group.collapsed {
    padding: 4px 3px;
  }

  .body {
    display: flex;
    gap: 2px;
    height: 76px;
  }

  /* In a collapsed group's dropdown without a large button: as tall as its
     tallest column. */
  .body.in-panel:not(:has(:global(.large))) {
    height: auto;
  }

  .col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .col .row {
    display: flex;
    gap: 2px;
  }

  .col .row.stack {
    flex-direction: column;
  }

  .caption {
    padding: 1px 0 2px;
    color: var(--ink-soft);
    font-size: 12px;
    line-height: 13px;
    text-align: center;
    white-space: nowrap;
  }

  .measure {
    position: absolute;
    top: 0;
    left: 0;
    width: 0;
    height: 0;
    overflow: hidden;
    visibility: hidden;
    pointer-events: none;
  }

  .measure .groups {
    width: max-content;
  }
</style>
