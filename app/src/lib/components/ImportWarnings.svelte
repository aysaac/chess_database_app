<script lang="ts">
  import type { ImportReportDto } from "../bindings/ImportReportDto";
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import Modal from "./Modal.svelte";

  /** What went wrong in the last import, game by game. */
  let { report, onClose }: { report: ImportReportDto; onClose: () => void } = $props();

  const plural = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

  function open(id: number) {
    if (db.info) void game.open(db.info.handle, id);
    onClose();
  }
</script>

<Modal title="Import warnings for {report.file}" {onClose} width="640px">
  <p>
    Imported {plural(report.imported, "game", "games")}{#if report.skipped}, and skipped
      {plural(report.skipped, "game", "games")} that couldn't be read{/if}. Games with a problem were kept as
    far as they could be read: an illegal move ends the line it's in.
  </p>
  {#if report.warningCount > report.warnings.length}
    <p class="soft">
      Showing the first {formatCount(report.warnings.length)} of {formatCount(report.warningCount)} warnings.
    </p>
  {/if}
  <ul>
    {#each report.warnings as w, i (i)}
      <li>
        <div class="where">
          <span class="number">Game {formatCount(w.number)} in the file</span>
          {#if w.label}<span class="soft">{w.label}</span>{/if}
          {#if w.game !== null}
            {@const id = w.game}
            <button title="Open this game on the board" onclick={() => open(id)}>Open</button>
          {:else}
            <span class="soft skipped">Not imported</span>
          {/if}
        </div>
        <div class="message">{w.message}</div>
      </li>
    {/each}
  </ul>
  {#snippet actions()}
    <button
      onclick={() => {
        db.importReport = null;
        onClose();
      }}>Dismiss warnings</button
    >
    <button class="primary" onclick={onClose}>Close</button>
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 10px;
    line-height: 1.45;
  }

  .soft {
    color: var(--ink-soft);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border-top: 1px solid var(--rule);
  }

  li {
    padding: 7px 2px;
    border-bottom: 1px solid var(--rule);
  }

  .where {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .number {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .where button,
  .skipped {
    margin-left: auto;
  }

  .where button {
    padding: 0 10px;
  }

  .message {
    margin-top: 2px;
    color: var(--ink-soft);
    overflow-wrap: anywhere;
  }
</style>
