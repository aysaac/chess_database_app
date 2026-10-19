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
  /** The report, while the database hasn't changed since. */
  const found = $derived(report && !report.stale ? report : null);
  const working = $derived(duplicates.working);
  /** Another window deletes or restores copies of this database. */
  const elsewhere = $derived(!working && !!report?.busy);
  const percent = $derived.by(() => {
    const job = duplicates.progress;
    return job && job.total > 0 ? Math.min(100, Math.round((job.done / job.total) * 100)) : 0;
  });
  const doing: Record<string, string> = {
    find: "Looking for copies…",
    delete: "Marking the copies as deleted…",
    restore: "Restoring the copies…",
  };

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
    file twice or merging two sources leaves. Each group keeps its best copy (the longest, then the
    one with the most variations, comments and symbols, then clock times and evals, then the most
    complete headers); the others can be marked as deleted. A copy with anything the kept game lacks
    (annotations, clock times, ratings, a board, an ECO code) is never one of them.
  </p>
  <fieldset disabled={!!working}>
    <legend>What counts as a copy</legend>
    <label class="choice" class:on={!fuzzy}>
      <input type="radio" name="duplicate-kind" checked={!fuzzy} onchange={() => (fuzzy = false)} />
      <span>
        <strong>Exact copies</strong>
        <span class="soft"
          >The same moves, players, ratings, event, site, date, round and result. Games of one
          move or none only between named players in a known round.</span
        >
      </span>
    </label>
    <label class="choice" class:on={fuzzy}>
      <input type="radio" name="duplicate-kind" checked={fuzzy} onchange={() => (fuzzy = true)} />
      <span>
        <strong>Exact and fuzzy copies</strong>
        <span class="soft">
          Also the same game from another source: the players written another way ("Carlsen, M."),
          another event name, or a copy that stops early (after 10 moves at least). Dates, results,
          rounds and ratings must agree where both copies have them; a game that could be a copy of
          two different games stays.
        </span>
      </span>
    </label>
  </fieldset>

  <div class="result" class:empty={!working && !report} aria-live="polite">
    {#if working}
      <div class="progress">
        <progress max="100" value={percent}></progress>
        <span>{doing[working]} {percent}%</span>
      </div>
    {:else if elsewhere}
      <p class="soft">Copies of this database are being deleted or restored in another window.</p>
    {:else if report?.stale}
      <p class="warn">
        {#if report.compacted}
          The database was compacted after the last search, so its games were renumbered. Find the
          duplicates again.
        {:else}
          The database changed after the last search (games were added, edited, deleted or
          restored), so a copy may now be the only one left. Find the duplicates again.
        {/if}
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
        {found.applied ? "marked as deleted" : "to delete"}, in {count(
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
    {/if}
    {#if !working && report?.deleted}
      <p class="done">
        Marked {count(report.deleted, "copy", "copies")} as deleted. They stay in the database, struck
        through in the list, until you compact it (Compact… in the status bar). Until then, Restore
        brings them back while the database stays open, and so does Restore in the game list's menu.
      </p>
    {/if}
  </div>

  {#snippet actions()}
    {#if working}
      <button onclick={() => duplicates.cancel()}
        >{working === "find" ? "Stop searching" : "Stop"}</button
      >
      <button onclick={onClose}>Close</button>
    {:else}
      <button onclick={onClose}>Close</button>
      {#if found?.groups}
        <button
          title="Show every group in the game list; in the list's own order, each kept game comes before its copies"
          onclick={show}>Show in game list</button
        >
        {#if !found.applied}
          <button
            class="danger"
            title="Mark the copies as deleted; you can restore them until you compact the database"
            disabled={elsewhere}
            onclick={() => duplicates.remove()}
            >Delete {count(found.toRemove, "copy", "copies")}</button
          >
        {/if}
      {/if}
      {#if report?.deleted}
        <button disabled={elsewhere} onclick={() => duplicates.restore()}
          >Restore {count(report.deleted, "copy", "copies")}</button
        >
      {/if}
      <button class:primary={!found?.groups} disabled={!db.info || elsewhere} onclick={find}
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
