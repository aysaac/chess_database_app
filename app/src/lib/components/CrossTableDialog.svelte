<script lang="ts">
  // Tournament cross-table (DB-16): the standings of one event's edition,
  // as a grid of results by opponent (round robins) or a row of rounds
  // (Swiss, knock-out). Clicking a result opens that game. Opened from the
  // Events list (NameList) and from the game on screen (GameHeader).
  import { onMount, tick } from "svelte";
  import type { CrossTableDto } from "../bindings/CrossTableDto";
  import type { CrossResultDto } from "../bindings/CrossResultDto";
  import { api } from "../api";
  import {
    formatPoints,
    formatTieBreak,
    gridCells,
    rankLabel,
    resultSymbol,
    resultTitle,
    slotCells,
    slotColumns,
    slotHeading,
  } from "../crosstable";
  import { formatCount } from "../format";
  import { db } from "../stores/db.svelte";
  import { game } from "../stores/game.svelte";
  import { notices } from "../stores/notify.svelte";
  import Modal from "./Modal.svelte";

  let {
    handle,
    event = null,
    gameId = null,
    onClose,
    onShowGames,
  }: {
    handle: number;
    /** An event's name id (the Events list), or… */
    event?: number | null;
    /** …a game whose event to show; its results are highlighted. */
    gameId?: number | null;
    onClose: () => void;
    /** After Show games has limited the list to the event. */
    onShowGames?: () => void;
  } = $props();

  let data = $state<CrossTableDto | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let view = $state<"grid" | "rounds">("grid");
  let scroller = $state<HTMLDivElement>();
  let seq = 0;

  const rows = $derived(data?.rows ?? []);
  const grid = $derived(view === "grid" && !!data?.grid);
  const cells = $derived(grid ? gridCells(rows) : []);
  const columns = $derived(data ? slotColumns(data.slots, data.unslotted) : []);
  const canShowGames = $derived(!!data && db.info?.handle === handle);

  const summary = $derived.by(() => {
    if (!data) return "";
    const parts = [];
    if (data.kind) parts.push(data.kind);
    parts.push(`${formatCount(data.players)} ${data.players === 1 ? "player" : "players"}`);
    if (data.byRound && data.slots.length) {
      parts.push(`${data.slots.length} ${data.slots.length === 1 ? "round" : "rounds"}`);
    }
    parts.push(`${formatCount(data.games)} ${data.games === 1 ? "game" : "games"}`);
    if (data.site) parts.push(data.site);
    if (data.dates) parts.push(data.dates);
    return parts.join(" · ");
  });

  const notes = $derived.by(() => {
    if (!data) return [];
    // "1 forfeit counts", "2 forfeits count".
    const n = (count: number, one: string, many: string, rest: string, verbs: [string, string]) =>
      `${formatCount(count)} ${count === 1 ? one : many}${rest} ${count === 1 ? verbs[0] : verbs[1]}`;
    const counts: [string, string] = ["counts", "count"];
    const out = [];
    if (data.unfinished) {
      out.push(`${n(data.unfinished, "unfinished game", "unfinished games", " (*)", counts)} for nothing.`);
    }
    if (data.forfeits) {
      out.push(`${n(data.forfeits, "forfeit", "forfeits", " (+ −)", counts)} for points, not for the performance.`);
    }
    if (data.byes) out.push(`${n(data.byes, "bye", "byes", "", counts)} for points, not for the performance.`);
    if (data.skipped) {
      out.push(`${n(data.skipped, "game", "games", " without both players' names", ["is", "are"])} left out.`);
    }
    return out;
  });

  async function load(edition: number | null) {
    const mine = ++seq;
    loading = true;
    try {
      const next = await api.crossTable(handle, data?.eventId ?? event, gameId, edition);
      if (mine !== seq) return;
      const first = !data;
      data = next;
      error = null;
      if (first) view = next.pairing === "roundRobin" && next.grid ? "grid" : "rounds";
      await tick();
      // Show the game on screen, else start at the top.
      const current = scroller?.querySelector<HTMLElement>("button.current");
      if (current) current.focus();
      else scroller?.scrollTo(0, 0);
    } catch (e) {
      if (mine !== seq) return;
      // The first load shows its error in the dialog; a later one (another edition) in a notice.
      if (data) notices.error(e);
      else error = (e as { message?: string })?.message ?? String(e);
    } finally {
      if (mine === seq) loading = false;
    }
  }

  onMount(() => {
    void load(null);
  });

  async function open(r: CrossResultDto) {
    // Props are live getters: copy them before closing.
    const h = handle;
    onClose();
    await game.open(h, r.game);
  }

  async function showGames() {
    const id = data?.eventId;
    const after = onShowGames;
    onClose();
    if (id === undefined) return;
    await db.showNameGames("event", id);
    after?.();
  }

  function sharedTitle(rank: number, rankTo: number): string | undefined {
    return rank === rankTo ? undefined : `Shares places ${rank}–${rankTo}: level on points and every tie-break`;
  }
</script>

{#snippet result(r: CrossResultDto, withOpponent: boolean)}
  {@const title = resultTitle(r, rows, data?.byRound ?? false)}
  <button
    class="res {r.outcome}"
    class:current={r.game === gameId}
    class:forfeit={r.forfeit}
    {title}
    aria-label={title}
    onclick={() => open(r)}
  >
    {#if withOpponent}
      {#if r.opponent === null}<span class="opp bye">bye</span>{:else}<span class="opp">{r.opponent + 1}</span
        ><span class="colour" class:white={r.white}></span>{/if}
    {/if}<span class="sym">{resultSymbol(r)}</span>
  </button>
{/snippet}

<Modal title={data?.event || "Cross-table"} {onClose} width="auto">
  <div class="cross">
    {#if data}
      <div class="bar">
        <p class="summary">{summary}</p>
        <span class="spacer"></span>
        {#if data.editions.length > 1}
          <label class="edition">
            Edition
            <select value={data.edition} onchange={(e) => load(Number(e.currentTarget.value))}>
              {#each data.editions as ed, i (i)}
                <option value={i}>{ed.label} · {formatCount(ed.games)} {ed.games === 1 ? "game" : "games"}</option>
              {/each}
            </select>
          </label>
        {/if}
        {#if data.grid && !data.tooBig}
          <div class="views" role="group" aria-label="Layout">
            <button aria-pressed={view === "grid"} onclick={() => (view = "grid")}>Grid</button>
            <button aria-pressed={view === "rounds"} onclick={() => (view = "rounds")}
              >{data.byRound ? "Rounds" : "Games"}</button
            >
          </div>
        {/if}
      </div>

      {#if data.tooBig}
        <p class="message">{data.tooBig}</p>
      {:else}
        <div class="scroll" class:loading bind:this={scroller}>
          <table aria-label="Standings">
            <thead>
              <tr>
                <th class="pos num" title="Place">#</th>
                <th class="name">Player</th>
                <th class="num" title="Elo in this event">Elo</th>
                {#if grid}
                  {#each rows as _, j (j)}
                    <th class="g" title={rows[j].name}>{j + 1}</th>
                  {/each}
                {:else}
                  {#each columns as slot (slot)}
                    <th class="g" title={slot === 0 ? "Games without a round" : undefined}
                      >{slotHeading(slot, data.byRound)}</th
                    >
                  {/each}
                {/if}
                <th class="num first-stat" title="Points">Pts</th>
                <th class="num" title="Games with a result">Games</th>
                <th class="num" title="Score">%</th>
                <th class="num" title="Average Elo of the opponents met over the board">Opp. Elo</th>
                <th class="num" title="Performance rating (FIDE), over the games against opponents with an Elo"
                  >Perf</th
                >
                {#each data.tieBreaks as t (t.short)}
                  <th class="num" title={t.title}>{t.short}</th>
                {/each}
              </tr>
            </thead>
            <tbody>
              {#each rows as row, i (i)}
                <tr>
                  <td class="pos num" title={sharedTitle(row.rank, row.rankTo)}
                    >{i + 1}{#if row.rank !== row.rankTo}<span class="tie" aria-label="shared place {rankLabel(row)}"
                        >=</span
                      >{/if}</td
                  >
                  <td class="name" title={row.name}>{row.name}</td>
                  <td class="num soft">{row.elo ?? ""}</td>
                  {#if grid}
                    {#each cells[i] as cell, j (j)}
                      <td class="g" class:self={i === j}>
                        {#each cell as r (r.game)}{@render result(r, false)}{/each}
                      </td>
                    {/each}
                  {:else}
                    {#each slotCells(row, columns) as cell, c (c)}
                      <td class="g rounds">
                        {#each cell as r (r.game)}{@render result(r, true)}{/each}
                      </td>
                    {/each}
                  {/if}
                  <td class="num pts first-stat">{formatPoints(row.points)}</td>
                  <td class="num">{row.games}</td>
                  <td class="num">{row.percent === null ? "" : `${Math.round(row.percent)}%`}</td>
                  <td
                    class="num soft"
                    title={row.ratedGames
                      ? `Over ${row.ratedGames} ${row.ratedGames === 1 ? "game" : "games"}`
                      : undefined}>{row.avgOpponent ?? ""}</td
                  >
                  <td class="num">{row.performance ?? ""}</td>
                  {#each row.tieBreaks as v, t (t)}
                    <td class="num">{formatTieBreak(v)}</td>
                  {/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if notes.length}
          <p class="notes">{notes.join(" ")}</p>
        {/if}
      {/if}
    {:else if error}
      <p class="message">{error}</p>
    {:else}
      <p class="message soft">Building the cross-table…</p>
    {/if}
  </div>

  {#snippet actions()}
    <button disabled={!canShowGames} title="List the event's games in the game list" onclick={showGames}
      >Show games</button
    >
    <button class="primary" onclick={onClose}>Close</button>
  {/snippet}
</Modal>

<style>
  .cross {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: min(560px, calc(100vw - 72px));
    font-size: 13px;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
  }

  .summary {
    margin: 0;
    color: var(--ink-soft);
  }

  .spacer {
    flex: 1;
  }

  .edition {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--ink-soft);
  }

  .edition select {
    padding: 2px 6px;
  }

  .views {
    display: flex;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .views button {
    border-radius: 0;
    padding: 2px 12px;
  }

  .views button[aria-pressed="true"] {
    background: var(--felt-soft);
    font-weight: 600;
  }

  .message {
    margin: 8px 0;
    max-width: 60ch;
  }

  .soft {
    color: var(--ink-soft);
  }

  /* Hugs a narrow table; a wide one scrolls inside. */
  .scroll {
    width: fit-content;
    max-width: 100%;
    max-height: calc(100vh - 260px);
    overflow: auto;
    border: 1px solid var(--rule);
    border-radius: var(--radius);
    transition: opacity 0.15s;
  }

  .scroll.loading {
    opacity: 0.55;
    pointer-events: none;
  }

  table {
    border-collapse: separate;
    border-spacing: 0;
    font-variant-numeric: tabular-nums;
  }

  th,
  td {
    padding: 2px 8px;
    white-space: nowrap;
    background: var(--paper);
    border-bottom: 1px solid color-mix(in srgb, var(--rule) 60%, transparent);
  }

  thead th {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--panel);
    color: var(--ink-soft);
    font-weight: 500;
    text-align: center;
    border-bottom: 1px solid var(--rule);
  }

  /* The place and the name stay in view while the table scrolls sideways. */
  .pos {
    position: sticky;
    left: 0;
    z-index: 1;
    width: 40px;
    min-width: 40px;
    color: var(--ink-soft);
  }

  .name {
    position: sticky;
    left: 40px;
    z-index: 1;
    max-width: 16em;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
    font-weight: 500;
    border-right: 1px solid var(--rule);
  }

  thead .pos,
  thead .name {
    z-index: 3;
  }

  thead .name {
    text-align: left;
  }

  .num {
    text-align: right;
  }

  .tie {
    margin-left: 1px;
    color: var(--brass);
  }

  .pts {
    font-weight: 700;
  }

  .first-stat {
    border-left: 1px solid var(--rule);
  }

  tbody tr:hover td {
    background: color-mix(in srgb, var(--felt-soft) 45%, var(--paper));
  }

  .g {
    min-width: 30px;
    padding: 1px 2px;
    text-align: center;
    border-left: 1px solid color-mix(in srgb, var(--rule) 60%, transparent);
  }

  tbody td.self {
    background: repeating-linear-gradient(
      135deg,
      var(--panel),
      var(--panel) 3px,
      color-mix(in srgb, var(--rule) 70%, var(--panel)) 3px,
      color-mix(in srgb, var(--rule) 70%, var(--panel)) 5px
    );
  }

  .res {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 22px;
    justify-content: center;
    padding: 1px 4px;
    border-radius: 3px;
    line-height: 1.3;
  }

  .rounds .res {
    min-width: 54px;
    justify-content: flex-end;
  }

  .opp {
    min-width: 2.2em;
    text-align: right;
    color: var(--ink-soft);
  }

  .opp.bye {
    font-style: italic;
  }

  .colour {
    width: 8px;
    height: 8px;
    border: 1px solid var(--ink-soft);
    border-radius: 1px;
    background: var(--eval-black);
  }

  .colour.white {
    background: var(--eval-white);
  }

  .sym {
    min-width: 0.8em;
    text-align: center;
  }

  .win .sym {
    color: var(--felt);
    font-weight: 700;
  }

  .loss .sym {
    color: var(--ink-soft);
  }

  .unfinished .sym {
    color: var(--brass);
  }

  .forfeit .sym {
    font-style: italic;
  }

  .res.current {
    outline: 2px solid var(--brass);
    outline-offset: -2px;
    background: var(--brass-soft);
  }

  .res:focus-visible {
    outline-offset: -2px;
  }

  .notes {
    margin: 0;
    max-width: 90ch;
    color: var(--ink-soft);
  }
</style>
