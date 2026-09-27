<script lang="ts">
  import type { HeadersDto } from "../bindings/HeadersDto";
  import Modal from "./Modal.svelte";

  let {
    headers,
    onSave,
    onCancel,
  }: { headers: HeadersDto; onSave: (h: HeadersDto) => void; onCancel: () => void } = $props();

  // Edit a copy; Elo fields are strings while typing.
  // svelte-ignore state_referenced_locally
  let h = $state({
    ...headers,
    extra: headers.extra.map(([k, v]) => ({ k, v })),
    whiteElo: headers.whiteElo?.toString() ?? "",
    blackElo: headers.blackElo?.toString() ?? "",
  });

  function elo(s: string): number | null {
    const n = Number.parseInt(s, 10);
    return Number.isFinite(n) && n > 0 && n < 4000 ? n : null;
  }

  function save() {
    onSave({
      ...h,
      whiteElo: elo(h.whiteElo),
      blackElo: elo(h.blackElo),
      extra: h.extra.filter((t) => t.k.trim()).map((t) => [t.k.trim(), t.v] as [string, string]),
    });
  }
</script>

<Modal title="Game details" onClose={onCancel} width="620px">
  <form
    onsubmit={(e) => {
      e.preventDefault();
      save();
    }}
  >
    <div class="grid">
      <label>White <input bind:value={h.white} /></label>
      <label class="elo">Elo <input bind:value={h.whiteElo} inputmode="numeric" /></label>
      <label>Black <input bind:value={h.black} /></label>
      <label class="elo">Elo <input bind:value={h.blackElo} inputmode="numeric" /></label>
      <label>Event <input bind:value={h.event} /></label>
      <label class="elo">Round <input bind:value={h.round} /></label>
      <label>Site <input bind:value={h.site} /></label>
      <label class="elo">
        Result
        <select bind:value={h.result}>
          <option value="*">Unknown</option>
          <option value="1-0">1–0</option>
          <option value="0-1">0–1</option>
          <option value="1/2-1/2">½–½</option>
        </select>
      </label>
      <label>Date <input bind:value={h.date} placeholder="YYYY.MM.DD" /></label>
      <label class="elo">ECO <input bind:value={h.eco} maxlength="3" /></label>
      <label class="wide">Annotator <input bind:value={h.annotator} /></label>
    </div>

    <h3>Other tags</h3>
    {#each h.extra as tag, i (i)}
      <div class="tag">
        <input bind:value={tag.k} placeholder="Tag" aria-label="Tag name" />
        <input bind:value={tag.v} placeholder="Value" aria-label="Tag value" />
        <button type="button" title="Remove tag" onclick={() => h.extra.splice(i, 1)}>Remove</button>
      </div>
    {/each}
    <button type="button" onclick={() => h.extra.push({ k: "", v: "" })}>Add tag</button>
    <button type="submit" hidden aria-hidden="true"></button>
  </form>
  {#snippet actions()}
    <button onclick={onCancel}>Cancel</button>
    <button class="primary" onclick={save}>Save details</button>
  {/snippet}
</Modal>

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 120px;
    gap: 8px 12px;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  label.wide {
    grid-column: 1 / -1;
  }

  label input,
  label select {
    color: var(--ink);
    font-size: 14px;
  }

  h3 {
    margin: 18px 0 8px;
    font-size: 14px;
    font-weight: 600;
  }

  .tag {
    display: grid;
    grid-template-columns: 180px 1fr auto;
    gap: 6px;
    margin-bottom: 6px;
  }
</style>
