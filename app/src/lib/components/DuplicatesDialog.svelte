<script lang="ts">
  // Find and remove duplicate games (DB-08, ChessBase's "Double game
  // killer"): choose what counts as a copy, search the open database in the
  // background, then mark the copies as deleted (restorable until the
  // database is compacted) or show them in the game list.
  import { onMount } from "svelte";
  import { formatCount, resultLabel } from "../format";
  import { db } from "../stores/db.svelte";
  import { duplicates } from "../stores/duplicates.svelte";
  import { prefs } from "../stores/prefs.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let fuzzy = $state(prefs.get<boolean>("duplicatesFuzzy", false));
  const report = $derived(duplicates.handle === db.info?.handle ? duplicates.report : null);
  /** The report, while its game numbers are still good. */
  const found = $derived(report && !report.stale ? report : null);
  const percent = $derived.by(() => {
    const job = duplicates.progress;
    return job && job.total > 0 ? Math.min(100, Math.round((job.done / job.total) * 100)) : 0;
  });

  onMount(() => void duplicates.load());

  function count(n: number, one: string, many: string) {
    return `${formatCount(n)} ${n === 1 ? one : many}`;
  }

  function find() {
    prefs.set("duplicatesFuzzy", fuzzy);
    void duplicates.find(fuzzy);
  }

  function show() {
    onClose();
    void duplicates.show();
  }
</script>

<Modal title="Find duplicate games" {onClose} width="660px">
  <p class="intro">
    Copies of the same game in <strong>{db.info?.name ?? "the database"}</strong>, as importing a
    file twice or merging two sources leaves. Each group keeps its best copy (the most variations,
    comments and symbols, then the longest, then the most complete headers); the others can be
    marked as deleted.
  </p>
  <fieldset disabled={duplicates.searching}>
    <legend>What counts as a copy</legend>
    <label class="choice" class:on={!fuzzy}>
      <input type="radio" name="duplicate-kind" checked={!fuzzy} onchange={() => (fuzzy = false)} />
      <span>
        <strong>Exact copies</strong>
        <span class="soft"
          >The same moves, players, ratings, event, site, date, round and result. Annotations may
          differ.</span
        >
      </span>
    </label>
    <label class="choice" class:on={fuzzy}>
      <input type="radio" name="duplicate-kind" checked={fuzzy} onchange={() => (fuzzy = true)} />
      <span>
        <strong>Exact and fuzzy copies</strong>
        <span class="soft">
          Also the same game from another source: the players written another way ("Carlsen, M."),
          another event name, or a copy that stops early. Dates, results, rounds and ratings must
          agree where both copies have them.
        </span>
      </span>
    </label>
  </fieldset>

  <div class="result" class:empty={!duplicates.searching && !report} aria-live="polite">
    {#if duplicates.searching}
      <div class="progress">
        <progress max="100" value={percent}></progress>
        <span>Looking for copies… {percent}%</span>
      </div>
    {:else if report?.stale}
      <p class="warn">
        The database was compacted after the last search, so its games were renumbered. Find the
        duplicates again.
      </p>
    {:else if found && found.groups === 0}
      <p class="summary">
        No {found.fuzzy ? "exact or fuzzy" : "exact"} copies among {count(
          found.games,
          "game",
          "games",
        )}.
      </p>
    {:else if found}
      <p class="summary">
        <strong>{count(found.toRemove, "copy", "copies")}</strong>
        {found.deleted ? "marked as deleted" : "to delete"}, in {count(
          found.groups,
          "group",
          "groups",
        )}
      </p>
      <p class="soft">
        {#if found.fuzzy}
          {count(found.exactGroups, "group", "groups")} of exact copies, {formatCount(
            found.groups - found.exactGroups,
          )} with fuzzy ones.
        {:else}
          Exact copies only.
        {/if}
        Searched {count(found.games, "game", "games")}.
      </p>
      <div class="table">
        <table>
          <thead>
            <tr>
              <th class="num">#</th>
              <th>Kept game</th>
              <th>Date</th>
              <th>Event</th>
              <th class="num">Copies</th>
            </tr>
          </thead>
          <tbody>
            {#each found.preview as g (g.keep.id)}
              <tr>
                <td class="num soft">{g.keep.id + 1}</td>
                <td class="players">
                  {g.keep.white || "?"} – {g.keep.black || "?"}
                  <span class="soft">{resultLabel(g.keep.result)}</span>
                </td>
                <td>{g.keep.date}</td>
                <td class="event">{g.keep.event}</td>
                <td class="num">
                  {#if !g.exact}<span class="tag" title="Some copies are fuzzy">fuzzy</span>{/if}
                  {g.copies}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if found.groups > found.preview.length}
        <p class="soft">
          And {count(found.groups - found.preview.length, "more group", "more groups")}: Show in
          game list lists them all.
        </p>
      {/if}
      {#if found.deleted}
        <p class="done">
          Marked {count(found.deleted, "copy", "copies")} as deleted. They stay in the database, struck
          through in the list, until you compact it (Compact… in the status bar). Until then, Restore
          brings them back.
        </p>
      {/if}
    {/if}
  </div>

  {#snippet actions()}
    {#if duplicates.searching}
      <button onclick={() => duplicates.cancel()}>Stop searching</button>
      <button onclick={onClose}>Close</button>
    {:else}
      <button onclick={onClose}>Close</button>
      {#if found?.groups}
        <button
          title="Show every group in the game list, each kept game followed by its copies"
          onclick={show}>Show in game list</button
        >
        {#if found.deleted}
          <button disabled={duplicates.busy} onclick={() => duplicates.restore()}
            >Restore {count(found.deleted, "copy", "copies")}</button
          >
        {:else}
          <button
            class="danger"
            title="Mark the copies as deleted; you can restore them until you compact the database"
            disabled={duplicates.busy}
            onclick={() => duplicates.remove()}
            >Delete {count(found.toRemove, "copy", "copies")}</button
          >
        {/if}
      {/if}
      <button class:primary={!found?.groups} disabled={!db.info} onclick={find}
        >{report ? "Find again" : "Find duplicates"}</button
      >
    {/if}
  {/snippet}
</Modal>

<style>
  p {
    margin: 0 0 10px;
  }

  .intro {
    color: var(--ink);
  }

  fieldset {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0 0 12px;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 6px;
    padding: 0;
    color: var(--ink-soft);
    font-size: 13px;
    font-weight: 600;
  }

  .choice {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 8px;
    border-radius: var(--radius);
    cursor: pointer;
  }

  .choice input {
    margin-top: 3px;
  }

  .choice.on {
    background: var(--felt-soft);
  }

  .choice > span {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  fieldset:disabled .choice {
    cursor: default;
    opacity: 0.6;
  }

  .soft {
    color: var(--ink-soft);
    font-size: 13px;
  }

  .result {
    min-height: 40px;
    padding-top: 10px;
    border-top: 1px solid var(--rule);
  }

  .result.empty {
    min-height: 0;
    padding: 0;
    border: 0;
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  progress {
    flex: 1;
    height: 6px;
    accent-color: var(--felt);
  }

  .summary {
    font-family: var(--font-book);
    font-size: 17px;
  }

  .warn {
    color: var(--danger);
  }

  .done {
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--brass-soft);
  }

  .table {
    max-height: 240px;
    margin-bottom: 10px;
    overflow: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  th {
    position: sticky;
    top: 0;
    padding: 4px 8px;
    background: var(--panel);
    color: var(--ink-soft);
    font-weight: 600;
    text-align: left;
  }

  td {
    padding: 3px 8px;
    border-top: 1px solid var(--rule);
    white-space: nowrap;
  }

  td.players,
  td.event {
    max-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  td.players {
    width: 48%;
  }

  td.event {
    width: 35%;
  }

  .num {
    text-align: right;
  }

  .tag {
    margin-right: 4px;
    padding: 0 6px;
    border-radius: 8px;
    background: var(--brass-soft);
    color: var(--brass);
    font-size: 11px;
    font-weight: 600;
  }

  button.danger {
    background: var(--danger);
    color: var(--paper);
  }

  button.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--danger) 85%, var(--ink));
  }
</style>
