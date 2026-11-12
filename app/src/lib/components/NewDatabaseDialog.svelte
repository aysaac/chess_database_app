<script lang="ts">
  // "New database": where its games come from (DB-02, NET-01, IO-10, NET-03).
  // From scratch makes an empty one; the other three make one that fills
  // itself from a Lichess or Chess.com account (or a Lichess study) or from
  // the user's own server, and can keep up to date (SyncDialog).
  import Icon from "./Icon.svelte";
  import Modal from "./Modal.svelte";

  export type NewSource = "scratch" | "server" | "chesscom" | "lichess";

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
      id: "chesscom",
      title: "From Chess.com",
      text: "Every game of a Chess.com account, by time control, and only the new ones each time you update.",
    },
    {
      id: "lichess",
      title: "From Lichess",
      text: "An account's games, or the chapters of a study, with clock times and comments. Update to get what's new.",
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
          {:else if c.id === "lichess"}
            <Icon name="chess-knight" size={28} />
          {:else}
            <!-- A plain pawn: a stand-in, not Chess.com's logo. -->
            <svg viewBox="0 0 24 24" width="28" height="28" fill="currentColor" aria-hidden="true">
              <path
                d="M12 3.2a3.4 3.4 0 0 0-2.2 6 5.6 5.6 0 0 0-2.4 4.1c0 1.5.6 2.8 1.6 3.7L7 19.6V21h10v-1.4l-2-2.6c1-.9 1.6-2.2 1.6-3.7a5.6 5.6 0 0 0-2.4-4.1 3.4 3.4 0 0 0-2.2-6z"
              />
            </svg>
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
    flex: none;
    width: 56px;
    height: 56px;
    display: grid;
    place-items: center;
    border-radius: 10px;
    color: #fff;
  }
  .mark.scratch {
    background: var(--felt);
  }
  .mark.server {
    background: #4a5f8a;
  }
  .mark.lichess {
    background: #2b2b2b;
  }
  .mark.chesscom {
    background: #5f8f3a;
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
