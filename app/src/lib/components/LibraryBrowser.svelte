<script lang="ts">
  // The database manager's browser (DB-07): folders/favourites, a grid of
  // recent databases and a preview of the selected one. Shared by the
  // library window (LibraryHome.svelte) and the in-editor modal
  // (LibraryView.svelte). Opening a database always spawns a separate
  // editor window (`onOpen`) rather than changing the current one.
  import { api } from "../api";
  import { formatBytes, formatCount, resultLabel } from "../format";
  import { db } from "../stores/db.svelte";
  import { inferDbIcon, library } from "../stores/library.svelte";
  import { prefs } from "../stores/prefs.svelte";
  import type { GameLineDto } from "../bindings/GameLineDto";
  import type { GameRowDto } from "../bindings/GameRowDto";
  import type { RecentDbDto } from "../bindings/RecentDbDto";
  import Icon from "./Icon.svelte";
  import LibraryMiniBoard from "./LibraryMiniBoard.svelte";

  let { onOpen, onSettings }: { onOpen: (path: string) => void; onSettings?: () => void } =
    $props();

  const SAMPLE_SIZE = 50;

  // ---- the preview panel's size, dragged from its border -----------------
  // Side layout: the border is on the left and the size is a width. Bottom
  // layout (library.previewLayout, a Settings choice, DB-07/ChessBase-style):
  // the preview sits under the grid instead, the border is on top, and the
  // same number is a height. One pref covers both; switching layouts keeps
  // whatever size was last set, which is close enough for either axis.
  const DEFAULT_PREVIEW_SIZE = 360;
  const MIN_PREVIEW_SIZE = 220;
  const MAX_PREVIEW_SIZE = 640;

  function clampPreview(n: number): number {
    return Math.max(MIN_PREVIEW_SIZE, Math.min(MAX_PREVIEW_SIZE, n));
  }

  const bottom = $derived(library.previewLayout === "bottom");

  /** Size while dragging (and after), before the pref has it. */
  let localPreviewSize = $state<number | null>(null);
  const previewSize = $derived(
    localPreviewSize ?? clampPreview(prefs.get("libraryPreviewSize", DEFAULT_PREVIEW_SIZE)),
  );

  /** Drags the preview's border; window listeners, like the game list's
   * column edges, so a drag past the divider keeps going. */
  function startDrag(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const start = previewSize;
    const horizontal = bottom;
    const p0 = horizontal ? e.clientY : e.clientX;
    const move = (ev: PointerEvent) => {
      // Released outside the window: no pointerup came.
      if ((ev.buttons & 1) === 0) return end();
      // The preview sits after the grid (right, or below), so dragging
      // toward the grid (up or left) widens/heightens it.
      const p = horizontal ? ev.clientY : ev.clientX;
      localPreviewSize = clampPreview(start + (p0 - p));
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      if (localPreviewSize != null) prefs.set("libraryPreviewSize", localPreviewSize);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  }

  function dividerKey(e: KeyboardEvent) {
    const grow = bottom ? { ArrowUp: 16, ArrowDown: -16 } : { ArrowLeft: 16, ArrowRight: -16 };
    const dn = grow[e.key as "ArrowLeft"];
    if (!dn || e.ctrlKey || e.metaKey || e.altKey) return;
    e.preventDefault();
    e.stopPropagation();
    localPreviewSize = clampPreview(previewSize + dn);
    prefs.set("libraryPreviewSize", localPreviewSize);
  }

  function resetPreviewSize() {
    localPreviewSize = DEFAULT_PREVIEW_SIZE;
    prefs.set("libraryPreviewSize", DEFAULT_PREVIEW_SIZE);
  }

  let selected = $state<string | null>(null);
  let sample = $state<GameRowDto[] | null>(null);
  let sampleTotal = $state(0);
  let loading = $state(false);
  let peekError = $state<string | null>(null);

  let boardGame = $state<number | null>(null);
  let line = $state<GameLineDto | null>(null);
  let lineLoading = $state(false);

  $effect(() => {
    void db.loadRecent();
  });

  const favoriteEntries = $derived(db.recent.filter((e) => library.isFavorite(e.path)));
  const current = $derived(db.recent.find((e) => e.path === selected) ?? null);

  async function pick(entry: RecentDbDto) {
    selected = entry.path;
    sample = null;
    peekError = null;
    line = null;
    boardGame = null;
    if (!entry.exists) return;
    loading = true;
    try {
      const page = await api.peekDatabase(entry.path, SAMPLE_SIZE);
      sample = page.rows;
      sampleTotal = page.total;
      if (page.rows.length) void pickGame(page.rows[0].id);
    } catch (e) {
      peekError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function pickGame(id: number) {
    if (!current) return;
    boardGame = id;
    const path = current.path;
    const seq = ++lineSeq;
    lineLoading = true;
    try {
      const got = await api.peekGameLine(path, id);
      if (seq === lineSeq) line = got;
    } catch {
      if (seq === lineSeq) line = null;
    } finally {
      if (seq === lineSeq) lineLoading = false;
    }
  }
  let lineSeq = 0;
</script>

<div class="browser">
  <aside class="folders">
    <div class="folders-top">
      {#if favoriteEntries.length}
        <div class="label">FAVOURITES</div>
        {#each favoriteEntries as entry (entry.path)}
          {@render miniRow(entry)}
        {/each}
      {/if}

      <div class="label">FOLDERS</div>
      <div class="folder-row row active">
        <Icon name="folder-open" size={14} />
        <span>My databases</span>
      </div>
      <p class="note small">Folders aren't filtered by yet; every database below is listed here.</p>
    </div>

    {#if onSettings}
      <div class="folders-footer">
        <button class="cog" aria-label="Settings" title="Settings" onclick={onSettings}>
          <Icon name="settings" size={14} />
        </button>
      </div>
    {/if}
  </aside>

  <div class="content" class:bottom>
    <main class="grid-area">
      <div class="grid-head">My databases</div>
      {#if db.recent.length === 0}
        <p class="note">No databases yet. Create or open one above.</p>
      {:else}
        <div class="grid">
          {#each db.recent as entry (entry.path)}
            {@render tile(entry)}
          {/each}
        </div>
      {/if}
    </main>

    <!-- A focusable separator is the WAI-ARIA window splitter (arrows move it). -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="divider"
      role="separator"
      aria-orientation={bottom ? "horizontal" : "vertical"}
      aria-label="Resize the preview panel"
      aria-valuenow={previewSize}
      aria-valuemin={MIN_PREVIEW_SIZE}
      aria-valuemax={MAX_PREVIEW_SIZE}
      tabindex="0"
      title="Drag to resize; double-click for the default size"
      onpointerdown={startDrag}
      onkeydown={dividerKey}
      ondblclick={resetPreviewSize}
    ></div>

    <section class="preview" style:flex-basis="{previewSize}px">
      {#if !current}
        <p class="note">Pick a database to see its details.</p>
      {:else}
        <div class="pv-header row">
          <div class="pv-icon"><Icon name={inferDbIcon(current.name)} size={22} /></div>
          <div class="pv-title">
            <div class="pv-name">{current.name}</div>
            <div class="pv-path" title={current.path}>{current.path}</div>
          </div>
          <button
            class="star"
            aria-label={library.isFavorite(current.path)
              ? "Remove from favourites"
              : "Add to favourites"}
            title={library.isFavorite(current.path)
              ? "Remove from favourites"
              : "Add to favourites"}
            onclick={() => library.toggleFavorite(current!.path)}
          >
            <Icon name="star" size={16} />
          </button>
          <button class="primary" disabled={!current.exists} onclick={() => onOpen(current!.path)}
            >Open</button
          >
        </div>

        {#if !current.exists}
          <p class="note">This database's folder can't be found any more.</p>
        {:else}
          <div class="stats row">
            <span>{current.games != null ? `${formatCount(current.games)} games` : "—"}</span>
            <span class="soft"
              >{current.sizeBytes != null ? formatBytes(current.sizeBytes) : ""}</span
            >
          </div>

          <div class="games-title">
            SAMPLE GAMES{sampleTotal
              ? ` (first ${formatCount(sample?.length ?? 0)} of ${formatCount(sampleTotal)})`
              : ""}
          </div>
          {#if loading}
            <p class="note">Loading…</p>
          {:else if peekError}
            <p class="note">Couldn't read this database: {peekError}</p>
          {:else if sample}
            <div class="games-and-board">
              <div class="games-scroll">
                <table>
                  <thead>
                    <tr>
                      <th class="num">#</th>
                      <th>White</th>
                      <th class="num">Elo</th>
                      <th>Black</th>
                      <th class="num">Elo</th>
                      <th>Result</th>
                      <th class="num">Moves</th>
                      <th>ECO</th>
                      <th>Event</th>
                      <th>Date</th>
                      <th>Round</th>
                      <th>Annotator</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each sample as g (g.id)}
                      <tr class:selected={g.id === boardGame} onclick={() => pickGame(g.id)}>
                        <td class="num">{g.id}</td>
                        <td>{g.white}</td>
                        <td class="num">{g.whiteElo ?? ""}</td>
                        <td>{g.black}</td>
                        <td class="num">{g.blackElo ?? ""}</td>
                        <td>{resultLabel(g.result)}</td>
                        <td class="num">{Math.ceil(g.plies / 2)}</td>
                        <td>{g.eco}</td>
                        <td>{g.event}</td>
                        <td>{g.date}</td>
                        <td>{g.round}</td>
                        <td>{g.annotator}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>

              {#if sample.length}
                <div class="mini-wrap">
                  <LibraryMiniBoard {line} loading={lineLoading} />
                </div>
              {/if}
            </div>
          {/if}
        {/if}
      {/if}
    </section>
  </div>
</div>

{#snippet miniRow(entry: RecentDbDto)}
  <button class="mini-row row" class:selected={entry.path === selected} onclick={() => pick(entry)}>
    <Icon name={inferDbIcon(entry.name)} size={15} />
    <span class="name">{entry.name}</span>
  </button>
{/snippet}

{#snippet tile(entry: RecentDbDto)}
  <button
    class="card"
    class:selected={entry.path === selected}
    class:missing={!entry.exists}
    onclick={() => pick(entry)}
  >
    {#if library.isFavorite(entry.path)}
      <span class="fav-badge"><Icon name="star" size={12} /></span>
    {/if}
    <div class="badge" style:opacity={entry.exists ? 1 : 0.5}>
      <Icon name={inferDbIcon(entry.name)} size={22} />
    </div>
    <div class="card-name">{entry.name}</div>
    <div class="card-meta">
      {entry.exists
        ? entry.games != null
          ? `${formatCount(entry.games)} games`
          : ""
        : "Not found"}
    </div>
  </button>
{/snippet}

<style>
  .browser {
    /* Tiling window managers routinely ignore the app's requested minimum
       window size (AGENTS.md), so this can get much narrower than the
       three-column layout below needs: stack instead of letting anything
       overflow or get clipped. */
    container: library / inline-size;
    flex: 1;
    display: flex;
    min-width: 0;
    min-height: 0;
  }

  .row {
    display: flex;
    align-items: center;
  }

  /* ---- left: favourites + folders ---- */
  .folders {
    width: 220px;
    flex: 0 0 220px;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border-right: 1px solid var(--rule);
    min-height: 0;
  }

  .folders-top {
    flex: 1;
    min-height: 0;
    padding: 12px 8px;
    overflow-y: auto;
  }

  /* Pinned at the bottom of the sidebar, like a PyCharm-style welcome screen. */
  .folders-footer {
    flex: none;
    padding: 8px;
    border-top: 1px solid var(--rule);
  }

  .cog {
    width: 26px;
    height: 26px;
    padding: 0;
    display: grid;
    place-items: center;
    border-radius: 50%;
    border-color: var(--rule);
    color: var(--ink-soft);
  }

  .cog:hover {
    color: var(--ink);
  }

  .label {
    font-size: 11px;
    letter-spacing: 0.06em;
    font-weight: 600;
    color: var(--ink-soft);
    padding: 10px 6px 4px;
  }

  .label:first-child {
    padding-top: 0;
  }

  .note {
    margin: 8px;
    color: var(--ink-soft);
    font-size: 13px;
  }

  .note.small {
    margin: 6px 6px 0;
    font-size: 11.5px;
  }

  .mini-row {
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid transparent;
    background: none;
    color: var(--ink);
    text-align: left;
    cursor: pointer;
    font-size: 13px;
  }

  .mini-row .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mini-row:hover {
    background: var(--felt-soft);
  }

  .mini-row.selected {
    border-color: var(--felt);
    background: var(--felt-soft);
  }

  .folder-row {
    gap: 7px;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 13px;
    color: var(--ink-soft);
  }

  .folder-row.active {
    background: var(--felt-soft);
    border-left: 3px solid var(--felt);
    font-weight: 600;
    color: var(--ink);
  }

  /* ---- middle: the grid + divider + preview, side by side or stacked ---- */
  .content {
    flex: 1;
    display: flex;
    min-width: 0;
    min-height: 0;
  }

  .content.bottom {
    flex-direction: column;
  }

  .grid-area {
    flex: 1 1 auto;
    min-width: 0;
    min-height: 0;
    padding: 18px 20px;
    overflow-y: auto;
  }

  .grid-head {
    font-family: var(--font-book);
    font-weight: 700;
    font-size: 18px;
    margin-bottom: 14px;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    gap: 12px;
  }

  .card {
    position: relative;
    background: var(--panel);
    border: 1px solid var(--rule);
    border-radius: 10px;
    padding: 14px;
    text-align: left;
    cursor: pointer;
    transition:
      box-shadow 0.15s,
      border-color 0.15s;
  }

  .card:hover {
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.08);
  }

  .card.selected {
    border-color: var(--felt);
    box-shadow: 0 0 0 2px var(--felt-soft);
  }

  .card.missing .card-name {
    color: var(--ink-soft);
  }

  .fav-badge {
    position: absolute;
    top: 10px;
    right: 10px;
    color: var(--brass);
  }

  .badge {
    width: 40px;
    height: 40px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: var(--brass-soft);
    color: var(--brass);
  }

  .card-name {
    font-family: var(--font-book);
    font-weight: 700;
    font-size: 14px;
    margin-top: 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-meta {
    font-size: 11.5px;
    color: var(--ink-soft);
    margin-top: 2px;
  }

  /* ---- the preview: to the side (default) or along the bottom ---- */
  .preview {
    /* The table and the mini board sit side by side (below this width,
       stacked instead, like the Reference tab does at its own breakpoint). */
    container: lib-preview / inline-size;
    flex: 0 0 auto;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--panel);
    border-left: 1px solid var(--rule);
    padding: 16px 18px;
    overflow-y: auto;
  }

  .content.bottom .preview {
    border-left: none;
    border-top: 1px solid var(--rule);
  }

  /* Between the grid and the preview, dragged to resize the preview: a
     vertical bar to the side, a horizontal one along the bottom. */
  .divider {
    flex: none;
    width: 5px;
    cursor: col-resize;
    background: var(--rule);
    border-inline: 2px solid var(--paper);
  }

  .content.bottom .divider {
    width: auto;
    height: 5px;
    cursor: row-resize;
    border-inline: none;
    border-block: 2px solid var(--paper);
  }

  .divider:hover,
  .divider:focus-visible {
    background: var(--felt);
  }

  .pv-header {
    gap: 10px;
  }

  .pv-icon {
    width: 40px;
    height: 40px;
    flex: 0 0 40px;
    display: grid;
    place-items: center;
    border-radius: 9px;
    background: var(--brass-soft);
    color: var(--brass);
  }

  .pv-title {
    min-width: 0;
    flex: 1;
  }

  .pv-name {
    font-family: var(--font-book);
    font-weight: 700;
    font-size: 16px;
  }

  .pv-path {
    font-size: 11.5px;
    color: var(--ink-soft);
    font-family: monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .star {
    width: 30px;
    height: 30px;
    padding: 0;
    display: grid;
    place-items: center;
    border-color: var(--rule);
    border-radius: 6px;
    color: var(--ink-soft);
  }

  .stats {
    gap: 10px;
    margin-top: 12px;
    font-size: 13px;
  }

  .soft {
    color: var(--ink-soft);
  }

  .games-title {
    font-size: 11px;
    letter-spacing: 0.06em;
    font-weight: 600;
    color: var(--ink-soft);
    margin-top: 18px;
    margin-bottom: 6px;
  }

  /* The point of this screen's feedback: more columns than fit, scrolled to
     rather than hidden (unlike the main game list, which drops columns when
     narrow). */
  /* The sample games table and the mini board, side by side. */
  .games-and-board {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    min-height: 0;
  }

  .games-scroll {
    flex: 1 1 auto;
    min-width: 0;
    overflow-x: auto;
    border: 1px solid var(--rule);
    border-radius: 4px;
  }

  table {
    border-collapse: collapse;
    font-size: 12.5px;
    white-space: nowrap;
  }

  th,
  td {
    padding: 5px 10px;
    text-align: left;
    border-bottom: 1px solid var(--rule);
  }

  th {
    position: sticky;
    top: 0;
    background: var(--panel);
    color: var(--ink-soft);
    font-weight: 600;
  }

  th.num,
  td.num {
    text-align: right;
  }

  tbody tr {
    cursor: pointer;
  }

  tbody tr:hover td {
    background: var(--felt-soft);
  }

  tbody tr.selected td {
    background: var(--felt-soft);
    font-weight: 600;
  }

  .mini-wrap {
    flex: 0 0 auto;
    width: clamp(160px, 42%, 240px);
    padding-left: 14px;
    border-left: 1px solid var(--rule);
    display: flex;
    justify-content: center;
  }

  /* Too narrow for the table and the board side by side: stack them, the
     board under the table (the Reference tab falls back the same way). */
  @container lib-preview (width < 300px) {
    .games-and-board {
      flex-direction: column;
    }

    .mini-wrap {
      width: 100%;
      padding-left: 0;
      padding-top: 12px;
      border-left: none;
      border-top: 1px solid var(--rule);
    }
  }

  /* Narrower than the three columns need (580px of it fixed, plus room for
     one grid column): stack them instead of clipping or overflowing. */
  @container library (width < 820px) {
    .browser {
      flex-direction: column;
      overflow-y: auto;
    }

    /* Regardless of the side/bottom setting: no room for two columns either way. */
    .content {
      flex-direction: column;
    }

    .folders,
    .grid-area,
    .preview {
      flex: none;
      width: 100%;
      overflow-y: visible;
    }

    .folders {
      border-right: none;
      border-bottom: 1px solid var(--rule);
    }

    .folders-top {
      overflow-y: visible;
    }

    .preview {
      border-left: none;
      border-top: 1px solid var(--rule);
      /* Overrides the drag-resize width, which is an inline style (higher
         specificity than any selector); in column mode it would otherwise
         set this section's height instead. */
      flex-basis: auto !important;
    }

    /* A column-resize handle makes no sense once the preview is stacked
       below the grid instead of beside it. */
    .divider {
      display: none;
    }
  }
</style>
