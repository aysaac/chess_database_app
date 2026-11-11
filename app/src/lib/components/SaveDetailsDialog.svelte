<script lang="ts">
  import type { HeadersDto } from "../bindings/HeadersDto";
  import Modal from "./Modal.svelte";

  // The details asked for when a game is saved as new (DB-01), laid out like
  // ChessBase's save dialog: the players and the tournament, the result as
  // buttons, Elo, round and ECO, and the date in three parts with Today and
  // Reset. Everything can be left empty. Headers the dialog doesn't show
  // (annotator, other tags) stay as the game has them.
  let {
    headers,
    database,
    onSave,
    onCancel,
  }: {
    headers: HeadersDto;
    database: string;
    onSave: (h: HeadersDto) => void;
    onCancel: () => void;
  } = $props();

  /** "2025.12.07", "2025.??.??" or "" as its three parts (empty where unknown). */
  function parts(date: string): [string, string, string] {
    const [y = "", m = "", d = ""] = date
      .split(".")
      .map((p) => (/^\d+$/.test(p) ? String(Number(p)) : ""));
    return [y, m, d];
  }

  // Edit a copy: the Elo fields are strings while typing.
  /* svelte-ignore state_referenced_locally */
  const start = parts(headers.date);
  /* svelte-ignore state_referenced_locally */
  let h = $state({
    white: headers.white,
    black: headers.black,
    event: headers.event,
    site: headers.site,
    round: headers.round,
    eco: headers.eco,
    result: headers.result,
    whiteElo: headers.whiteElo?.toString() ?? "",
    blackElo: headers.blackElo?.toString() ?? "",
    year: start[0],
    month: start[1],
    day: start[2],
  });

  const RESULTS: { value: string; label: string }[] = [
    { value: "1-0", label: "1–0" },
    { value: "1/2-1/2", label: "½–½" },
    { value: "0-1", label: "0–1" },
    { value: "*", label: "Unknown" },
  ];

  function elo(s: string): number | null {
    const n = Number.parseInt(s, 10);
    return Number.isFinite(n) && n > 0 && n < 4000 ? n : null;
  }

  /** A date part as PGN writes it, or "??" (4 times "?" for the year) when empty or out of range. */
  function part(s: string, max: number, width: number): string {
    const n = Number.parseInt(s, 10);
    return Number.isFinite(n) && n >= 1 && n <= max
      ? String(n).padStart(width, "0")
      : "?".repeat(width);
  }

  function date(): string {
    const [y, m, d] = [part(h.year, 9999, 4), part(h.month, 12, 2), part(h.day, 31, 2)];
    return y === "????" && m === "??" && d === "??" ? "" : `${y}.${m}.${d}`;
  }

  function today() {
    const now = new Date();
    [h.year, h.month, h.day] = [
      String(now.getFullYear()),
      String(now.getMonth() + 1),
      String(now.getDate()),
    ];
  }

  function reset() {
    [h.year, h.month, h.day] = ["", "", ""];
  }

  function save() {
    onSave({
      ...headers,
      white: h.white.trim(),
      black: h.black.trim(),
      event: h.event.trim(),
      site: h.site.trim(),
      round: h.round.trim(),
      eco: h.eco.trim().toUpperCase(),
      result: h.result,
      whiteElo: elo(h.whiteElo),
      blackElo: elo(h.blackElo),
      date: date(),
    });
  }

  function focus(el: HTMLInputElement) {
    el.focus();
    el.select();
  }
</script>

<Modal title="Save as new in {database}" onClose={onCancel} width="600px">
  <form
    onsubmit={(e) => {
      e.preventDefault();
      save();
    }}
  >
    <div class="names">
      <label for="sd-white">White</label>
      <input id="sd-white" bind:value={h.white} placeholder="Last name, first name" use:focus />
      <label for="sd-black">Black</label>
      <input id="sd-black" bind:value={h.black} placeholder="Last name, first name" />
      <label for="sd-event">Tournament</label>
      <input id="sd-event" bind:value={h.event} />
      <label for="sd-site">Site</label>
      <input id="sd-site" bind:value={h.site} />
    </div>

    <div class="lower">
      <div class="numbers">
        <label for="sd-eco">ECO code</label>
        <input id="sd-eco" bind:value={h.eco} maxlength="3" class="short" />
        <label for="sd-welo">Elo White</label>
        <input id="sd-welo" bind:value={h.whiteElo} inputmode="numeric" class="short" />
        <label for="sd-belo">Elo Black</label>
        <input id="sd-belo" bind:value={h.blackElo} inputmode="numeric" class="short" />
        <label for="sd-round">Round</label>
        <input id="sd-round" bind:value={h.round} class="short" />
      </div>

      <fieldset class="result">
        <legend>Result</legend>
        {#each RESULTS as r (r.value)}
          <label class="radio"
            ><input type="radio" name="result" value={r.value} bind:group={h.result} />
            {r.label}</label
          >
        {/each}
      </fieldset>

      <fieldset class="date">
        <legend>Date</legend>
        <label for="sd-year">Year</label>
        <input id="sd-year" bind:value={h.year} inputmode="numeric" maxlength="4" class="short" />
        <label for="sd-month">Month</label>
        <input id="sd-month" bind:value={h.month} inputmode="numeric" maxlength="2" class="short" />
        <label for="sd-day">Day</label>
        <input id="sd-day" bind:value={h.day} inputmode="numeric" maxlength="2" class="short" />
        <div class="date-buttons">
          <button type="button" onclick={today}>Today</button>
          <button type="button" onclick={reset}>Reset</button>
        </div>
      </fieldset>
    </div>
    <button type="submit" hidden aria-hidden="true"></button>
  </form>
  {#snippet actions()}
    <button onclick={onCancel}>Cancel</button>
    <button class="primary" onclick={save}>Save game</button>
  {/snippet}
</Modal>

<style>
  .names {
    display: grid;
    grid-template-columns: 90px 1fr;
    align-items: center;
    gap: 8px 12px;
  }

  .lower {
    display: grid;
    grid-template-columns: auto auto auto;
    justify-content: space-between;
    gap: 16px;
    margin-top: 16px;
  }

  .numbers,
  .date {
    display: grid;
    grid-template-columns: auto 72px;
    align-content: start;
    align-items: center;
    gap: 6px 10px;
  }

  fieldset {
    margin: 0;
    padding: 8px 12px 10px;
    border: 1px solid var(--rule, currentColor);
    border-radius: 6px;
  }

  legend {
    padding: 0 4px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  .result {
    display: grid;
    align-content: start;
    gap: 6px;
  }

  label {
    font-size: 13px;
    color: var(--ink-soft);
  }

  .radio {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--ink);
    font-size: 14px;
  }

  input:not([type="radio"]) {
    color: var(--ink);
    font-size: 14px;
  }

  .short {
    width: 72px;
  }

  .date-buttons {
    grid-column: 1 / -1;
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
</style>
