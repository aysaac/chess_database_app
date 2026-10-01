<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { formatCount } from "../format";
  import { analysis } from "../stores/analysis.svelte";
  import { inDocuments } from "../stores/db.svelte";
  import { jobs } from "../stores/jobs.svelte";
  import { baseName, folderName, onboarding, type WelcomeStep } from "../stores/onboarding.svelte";
  import Modal from "./Modal.svelte";

  // The welcome guide (QOL-08): how to start (import, sample games or an
  // existing database), the engine, and a few tips. It opens on a first run
  // and from the "?" button in the status bar.
  const TITLES: Record<WelcomeStep, string> = {
    start: "Welcome to Chess Database",
    import: "Import your games",
    engine: "Your engine",
    tips: "A few things to try",
  };
  const PAGE: Record<WelcomeStep, number> = { start: 1, import: 1, engine: 2, tips: 3 };

  let busy = $state(false);
  const importJob = $derived(jobs.running.find((j) => j.kind === "import"));
  const percent = $derived(
    importJob && importJob.total > 0 ? Math.min(100, Math.round((importJob.done / importJob.total) * 100)) : 0,
  );
  const engine = $derived(analysis.defaultEngine);
  const targetName = $derived(baseName(onboarding.target).replace(/\.chessdb$/i, ""));

  async function run(f: () => Promise<unknown>) {
    busy = true;
    try {
      await f();
    } finally {
      busy = false;
    }
  }

  async function addEngine() {
    const path = await open({
      multiple: false,
      directory: false,
      title: "Choose a UCI engine program",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await run(() => analysis.add(path));
  }

  /** Focuses an element when it appears (the dialog must keep focus for Escape to work). */
  function focusOnMount(node: HTMLElement) {
    node.focus();
  }
</script>

{#key onboarding.step}
  <Modal title={TITLES[onboarding.step]} onClose={() => onboarding.close()} width="620px">
    {#if onboarding.step === "start"}
      <p class="lead">
        Keep your games in a database, find them again in a moment, and check them with a chess engine. How would
        you like to start?
      </p>
      <div class="choices">
        <button class="choice" use:focusOnMount disabled={busy} onclick={() => onboarding.goto("import")}>
          <strong>Import your games</strong>
          <span>Make a database from PGN files, such as a download from Lichess, Chess.com or TWIC.</span>
        </button>
        <button
          class="choice"
          disabled={busy}
          title={onboarding.samplePath}
          onclick={() => run(() => onboarding.openSample())}
        >
          <strong>Explore the sample games</strong>
          <span
            >{onboarding.sampleGames || "Twelve"} famous games, from the Immortal Game to Kasparov's Immortal, with a
            few notes. A good place to try the board and the engine.</span
          >
        </button>
        <button class="choice" disabled={busy} onclick={() => run(() => onboarding.openExisting())}>
          <strong>Open a database</strong>
          <span>A .chessdb folder you already have.</span>
        </button>
      </div>
    {:else if onboarding.step === "import"}
      <section>
        <h3>PGN files</h3>
        {#if onboarding.files.length}
          <ul class="files">
            {#each onboarding.files as f, i (f)}
              <li title={f} class:now={onboarding.importing && i === onboarding.current}>{baseName(f)}</li>
            {/each}
          </ul>
        {:else}
          <p class="soft">None chosen yet. Pick one or several: they all go into one new database.</p>
        {/if}
        <button
          class:primary={!onboarding.files.length}
          class:plain={onboarding.files.length > 0}
          disabled={onboarding.importing}
          onclick={() => onboarding.chooseFiles()}
          >{onboarding.files.length ? "Choose other files…" : "Choose PGN files…"}</button
        >
      </section>

      {#if onboarding.files.length}
        <section>
          <h3>New database</h3>
          {#if onboarding.target}
            <p class="target" title={onboarding.target}>
              <strong>{targetName}</strong> <span class="soft">in {folderName(onboarding.target)}</span>
            </p>
          {/if}
          <button
            class="plain"
            disabled={onboarding.importing || !!onboarding.result}
            onclick={() => onboarding.chooseTarget()}>Change…</button
          >
        </section>
      {/if}

      {#if onboarding.importing}
        <div class="progress" aria-live="polite">
          <span>
            Importing {baseName(onboarding.files[onboarding.current] ?? "")}{onboarding.files.length > 1
              ? ` (${onboarding.current + 1} of ${onboarding.files.length})`
              : ""}
          </span>
          <progress max="100" value={percent}></progress>
          <span class="soft">{percent}%</span>
          {#if importJob}
            <button class="plain" onclick={() => jobs.cancel(importJob.id)}>Cancel</button>
          {/if}
        </div>
      {/if}

      {#if onboarding.result}
        {@const r = onboarding.result}
        <p class:ok={r.complete} role="status">
          {#if r.complete}
            Imported {formatCount(r.games)} {r.games === 1 ? "game" : "games"} into {r.name}.
          {:else}
            The import stopped early: {r.name} has the {formatCount(r.games)}
            {r.games === 1 ? "game" : "games"} read so far.
          {/if}
        </p>
        {#if r.warnings}
          <p class="soft">
            {formatCount(r.warnings)}
            {r.warnings === 1 ? "problem was" : "problems were"} found while reading the files{#if r.skipped}, and
              {formatCount(r.skipped)}
              {r.skipped === 1 ? "game couldn't be read, so it was" : "games couldn't be read, so they were"} skipped{/if}.
            "Import warnings" in the status bar lists them.
          </p>
        {/if}
      {/if}

      {#if onboarding.error}<p class="error" role="alert">{onboarding.error}</p>{/if}
    {:else if onboarding.step === "engine"}
      {#if analysis.loadingEngines}
        <p class="soft" aria-live="polite">Looking for engines on this computer…</p>
      {:else if engine}
        <p class="ok"><strong>{engine.name}</strong> is ready.</p>
        <p class="soft path" title={engine.path}>
          {engine.detected ? "Found automatically" : "Added by you"}: {engine.path}
        </p>
        <p>
          It analyses the position on the board (<strong>Analyze</strong> in the toolbar, or <kbd>Ctrl</kbd>+<kbd
            >Shift</kbd
          >+<kbd>A</kbd>) and checks whole games for mistakes (<strong>Blunder check…</strong>).
        </p>
        {#if analysis.engines.length > 1}
          <p class="soft">
            You have {analysis.engines.length} engines; <strong>Engines…</strong> in the toolbar chooses the default.
          </p>
        {/if}
      {:else}
        <p>
          No engine was found. The database works without one, but analysis and the blunder check need a UCI
          engine.
        </p>
        <p class="soft">
          Stockfish is free and one of the strongest: download it from stockfishchess.org, then add the program
          here.
        </p>
        <div class="row">
          <button class="plain" disabled={busy} onclick={addEngine}>Add engine…</button>
          <button class="plain" disabled={busy} onclick={() => run(() => analysis.detect())}>Look again</button>
        </div>
      {/if}
    {:else}
      <ul class="tips">
        <li>
          <strong>Type a move</strong> with the board focused: <kbd>e4</kbd>, <kbd>Nf3</kbd> or <kbd>O-O</kbd>, then
          Enter.
        </li>
        <li>
          <strong>Arrow keys</strong> step through a game, <kbd>Home</kbd> and <kbd>End</kbd> jump to its ends, and
          <kbd>Ctrl</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> open the previous or next game in the list.
        </li>
        <li><strong>Right-click a move</strong> to comment on it, add symbols, or promote and delete variations.</li>
        <li>
          The <strong>Opening tree</strong> tab, next to Notation, shows what was played from the position on the board,
          and how it scored.
        </li>
        <li><strong>Search…</strong> above the game list finds games by player, event, year or rating.</li>
      </ul>
      <p class="soft">This guide opens again from the <strong>?</strong> button at the right end of the status bar.</p>
    {/if}

    {#snippet actions()}
      <span class="page">Step {PAGE[onboarding.step]} of 3</span>
      {#if onboarding.step === "start"}
        <button onclick={() => onboarding.close()}>Skip for now</button>
      {:else if onboarding.step === "import"}
        <button disabled={onboarding.importing} onclick={() => onboarding.goto("start")}>Back</button>
        {#if onboarding.result}
          <button class="primary" use:focusOnMount onclick={() => onboarding.goto("engine")}>Next</button>
        {:else}
          <button
            class:primary={onboarding.files.length > 0}
            disabled={!onboarding.files.length || !onboarding.target || onboarding.importing}
            onclick={() => onboarding.runImport()}>{onboarding.importing ? "Importing…" : "Import"}</button
          >
        {/if}
      {:else if onboarding.step === "engine"}
        <button onclick={() => onboarding.goto("start")}>Back</button>
        <button class="primary" onclick={() => onboarding.goto("tips")}>Next</button>
      {:else}
        <button onclick={() => onboarding.goto("engine")}>Back</button>
        <button class="primary" onclick={() => onboarding.close()}>Start</button>
      {/if}
    {/snippet}
  </Modal>
{/key}

<style>
  p {
    margin: 0 0 10px;
  }

  .lead {
    margin-bottom: 14px;
  }

  .soft {
    color: var(--ink-soft);
  }

  .choices {
    display: grid;
    gap: 8px;
  }

  .choice {
    display: grid;
    gap: 2px;
    padding: 10px 14px;
    text-align: left;
    white-space: normal;
    border-color: var(--rule);
    background: var(--panel);
  }

  .choice strong {
    font-size: 15px;
  }

  .choice span {
    color: var(--ink-soft);
  }

  .choice:hover:not(:disabled) {
    border-color: var(--felt);
  }

  section {
    margin-bottom: 14px;
  }

  h3 {
    margin: 0 0 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-soft);
  }

  .files {
    margin: 0 0 8px;
    padding: 0;
    list-style: none;
    max-height: 120px;
    overflow-y: auto;
  }

  .files li {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .files li.now {
    color: var(--felt);
    font-weight: 600;
  }

  .target {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-bottom: 6px;
  }

  .plain {
    border-color: var(--rule);
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 10px;
  }

  .progress > span:first-child {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  progress {
    width: 140px;
    height: 6px;
    accent-color: var(--felt);
  }

  .ok {
    color: var(--felt);
    font-weight: 500;
  }

  .ok::before {
    content: "✓ ";
  }

  .error {
    color: var(--danger);
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .tips {
    margin: 0 0 12px;
    padding-left: 18px;
    display: grid;
    gap: 6px;
  }

  kbd {
    padding: 0 4px;
    border: 1px solid var(--rule);
    border-radius: 3px;
    background: var(--panel);
    font: inherit;
    font-size: 12px;
  }

  .page {
    margin-right: auto;
    align-self: center;
    color: var(--ink-soft);
    font-size: 13px;
  }
</style>
