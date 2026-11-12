<script lang="ts">
  // "New database": where its games come from (DB-02, NET-01, IO-10, NET-03).
  // From scratch makes an empty one; the other three make one that fills
  // itself from a Lichess or Chess.com account (or a Lichess study) or from
  // the user's own server, and can keep up to date (SyncDialog).
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  export type NewSource = "scratch" | "server" | "online" | "study";

  let { onPick, onClose }: { onPick: (source: NewSource) => void; onClose: () => void } = $props();

  const choices: { id: NewSource; title: string; text: string }[] = [
    {
      id: "scratch",
      title: "From scratch",
      text: "An empty database on this computer. Add games by importing a PGN file, pasting, or saving games you play and analyse.",
    },
    {
      id: "server",
      title: "From my server",
      text: "A copy of a database on your own chess server, kept in step as you add games there.",
    },
    {
      id: "online",
      title: "From online games",
      text: "The games of your Lichess and Chess.com accounts, in one database. Add as many accounts as you like; each update brings only the new games.",
    },
    {
      id: "study",
      title: "From a Lichess study",
      text: "The chapters of a study, with comments, arrows and clock times. Update to get the chapters added since.",
    },
  ];
</script>

<Modal title="New database" {onClose} width="min(820px, 92vw)">
  <p class="intro">Where do the games come from?</p>
  <div class="grid">
    {#each choices as c (c.id)}
      <button type="button" class="card" onclick={() => onPick(c.id)}>
        <span class="mark {c.id}" aria-hidden="true">
          {#if c.id === "scratch"}
            <Icon name="database-plus" size={28} />
          {:else if c.id === "server"}
            <Icon name="cloud-download" size={28} />
          {:else if c.id === "study"}
            <Icon name="book-down" size={28} />
          {:else}
            <!-- Plain stand-ins for the two sites, not their logos: a pawn and a knight. -->
            <svg viewBox="0 0 24 24" width="28" height="28" fill="currentColor" aria-hidden="true">
              <path
                d="M7 3.5a2.4 2.4 0 0 0-1.6 4.2A4 4 0 0 0 3.7 11c0 1.1.4 2 1.1 2.7L3.4 16v1.2h7V16L9 13.7c.7-.7 1.1-1.6 1.1-2.7a4 4 0 0 0-1.7-3.3A2.4 2.4 0 0 0 7 3.5z"
              />
            </svg>
            <Icon name="chess-knight" size={22} />
          {/if}
        </span>
        <span class="text">
          <strong>{c.title}</strong>
          <span>{c.text}</span>
        </span>
      </button>
    {/each}
  </div>

  {#snippet actions()}
    <button onclick={onClose}>Cancel</button>
  {/snippet}
</Modal>

<style>
  .intro {
    margin: 0 0 12px;
    color: var(--ink-soft);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 14px;
  }
  .card {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    text-align: left;
    padding: 18px;
    min-height: 120px;
    min-width: 0;
    width: 100%;
    height: auto;
    white-space: normal;
    justify-content: flex-start;
    align-items: flex-start;
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--paper);
    color: var(--ink);
    cursor: pointer;
  }
  .card:hover,
  .card:focus-visible {
    border-color: var(--brass);
    background: var(--brass-soft);
  }
  .mark {
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 56px;
    height: 56px;
    border-radius: 10px;
    color: #fff;
  }
  .mark.scratch {
    background: var(--felt);
  }
  .mark.server {
    background: #4a5f8a;
  }
  .mark.study {
    background: #2b2b2b;
  }
  .mark.online {
    gap: 2px;
    background: linear-gradient(135deg, #5f8f3a 50%, #2b2b2b 50%);
  }
  .text {
    display: grid;
    gap: 6px;
    min-width: 0;
    font-size: 0.95em;
    line-height: 1.4;
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .text strong {
    font-family: var(--font-book);
    font-size: 1.1em;
  }
  .text span {
    color: var(--ink-soft);
  }
  @media (max-width: 560px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
