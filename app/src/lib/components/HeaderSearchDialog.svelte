<script lang="ts">
  import type { EloModeDto } from "../bindings/EloModeDto";
  import type { HeaderQueryDto } from "../bindings/HeaderQueryDto";
  import type { ResultFilterDto } from "../bindings/ResultFilterDto";
  import { db } from "../stores/db.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();

  // Numbers are typed as text and checked on search.
  const blank = {
    white: "",
    black: "",
    ignoreColors: false,
    event: "",
    site: "",
    annotator: "",
    yearFrom: "",
    yearTo: "",
    ecoFrom: "",
    ecoTo: "",
    result: "" as ResultFilterDto | "",
    eloMin: "",
    eloMax: "",
    eloMode: "both" as EloModeDto,
    movesMin: "",
    movesMax: "",
    annotatedOnly: false,
    skipDeleted: false,
  };
  const str = (n: number | null | undefined) => (n == null ? "" : String(n));
  const last = db.lastQuery;
  let q = $state({
    ...blank,
    ...last,
    yearFrom: str(last.yearFrom),
    yearTo: str(last.yearTo),
    eloMin: str(last.eloMin),
    eloMax: str(last.eloMax),
    movesMin: str(last.movesMin),
    movesMax: str(last.movesMax),
    result: (last.result ?? "") as ResultFilterDto | "",
  });
  let within = $state(false);
  let error = $state("");

  function num(s: string, what: string, max: number): number | null {
    if (!s.trim()) return null;
    const n = Number(s.trim());
    if (!Number.isInteger(n) || n < 0 || n > max) throw new Error(`${what} must be a whole number up to ${max}.`);
    return n;
  }

  function eco(s: string, what: string): string {
    const v = s.trim().toUpperCase();
    if (v && !/^[A-E]\d\d$/.test(v)) throw new Error(`${what} must look like B20 (a letter A–E and two digits).`);
    return v;
  }

  function search() {
    let query: Partial<HeaderQueryDto>;
    try {
      query = {
        white: q.white.trim(),
        black: q.black.trim(),
        ignoreColors: q.ignoreColors,
        event: q.event.trim(),
        site: q.site.trim(),
        annotator: q.annotator.trim(),
        yearFrom: num(q.yearFrom, "The first year", 9999),
        yearTo: num(q.yearTo, "The last year", 9999),
        ecoFrom: eco(q.ecoFrom, "The first ECO code"),
        ecoTo: eco(q.ecoTo, "The last ECO code"),
        result: q.result || null,
        eloMin: num(q.eloMin, "The lowest Elo", 4000),
        eloMax: num(q.eloMax, "The highest Elo", 4000),
        eloMode: q.eloMode,
        movesMin: num(q.movesMin, "The fewest moves", 9999),
        movesMax: num(q.movesMax, "The most moves", 9999),
        annotatedOnly: q.annotatedOnly,
        skipDeleted: q.skipDeleted,
      };
    } catch (e) {
      error = (e as Error).message;
      return;
    }
    onClose();
    db.quickText = "";
    db.search(query, within);
  }
</script>

<Modal title="Search games" {onClose} width="640px">
  <form
    onsubmit={(e) => {
      e.preventDefault();
      search();
    }}
  >
    <div class="grid">
      <label>White <input bind:value={q.white} placeholder="Name or part of it" /></label>
      <label>Black <input bind:value={q.black} placeholder="Name or part of it" /></label>
      <label class="check wide"
        ><input type="checkbox" bind:checked={q.ignoreColors} /> Either player may have White</label
      >
      <label>Event <input bind:value={q.event} /></label>
      <label>Site <input bind:value={q.site} /></label>
      <label>Annotator <input bind:value={q.annotator} /></label>
      <label>
        Result
        <select bind:value={q.result}>
          <option value="">Any result</option>
          <option value="whiteWins">White won (1–0)</option>
          <option value="blackWins">Black won (0–1)</option>
          <option value="draw">Draw (½–½)</option>
          <option value="decisive">Decisive (1–0 or 0–1)</option>
        </select>
      </label>
      <div class="range">
        <label>Year from <input bind:value={q.yearFrom} inputmode="numeric" placeholder="1990" /></label>
        <label>to <input bind:value={q.yearTo} inputmode="numeric" placeholder="2024" /></label>
      </div>
      <div class="range">
        <label>ECO from <input bind:value={q.ecoFrom} maxlength="3" placeholder="B20" /></label>
        <label>to <input bind:value={q.ecoTo} maxlength="3" placeholder="B99" /></label>
      </div>
      <div class="range">
        <label>Elo from <input bind:value={q.eloMin} inputmode="numeric" placeholder="2500" /></label>
        <label>to <input bind:value={q.eloMax} inputmode="numeric" /></label>
      </div>
      <label>
        Elo applies to
        <select bind:value={q.eloMode}>
          <option value="both">Both players</option>
          <option value="either">At least one player</option>
          <option value="average">The average of both</option>
        </select>
      </label>
      <div class="range">
        <label>Moves from <input bind:value={q.movesMin} inputmode="numeric" /></label>
        <label>to <input bind:value={q.movesMax} inputmode="numeric" /></label>
      </div>
      <label class="check"
        ><input type="checkbox" bind:checked={q.annotatedOnly} /> Only annotated games</label
      >
      <label class="check"><input type="checkbox" bind:checked={q.skipDeleted} /> Leave out deleted games</label>
      <label class="check wide" class:disabled={!db.info?.filter}>
        <input type="checkbox" bind:checked={within} disabled={!db.info?.filter} /> Search within the current
        results
      </label>
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <button type="submit" hidden aria-hidden="true"></button>
  </form>
  {#snippet actions()}
    <button onclick={() => (q = { ...blank })}>Clear fields</button>
    <button onclick={onClose}>Cancel</button>
    <button class="primary" onclick={search}>Search</button>
  {/snippet}
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px 14px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  label input,
  label select {
    color: var(--ink);
    font-size: 14px;
  }

  label.check {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    color: var(--ink);
  }

  label.disabled {
    opacity: 0.5;
  }

  .wide {
    grid-column: 1 / -1;
  }

  .range {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }

  .range input {
    width: 100%;
  }

  .error {
    margin: 12px 0 0;
    color: var(--danger);
  }
</style>
