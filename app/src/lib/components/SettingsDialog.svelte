<script lang="ts">
  // App settings: appearance, for now; more sections join this dialog
  // later.
  import { APP_THEMES, appearance } from "../stores/appearance.svelte";
  import Modal from "./Modal.svelte";

  let { onClose }: { onClose: () => void } = $props();
</script>

<Modal title="Settings" {onClose} width="400px">
  <section>
    <h3>Appearance</h3>
    <div class="themes" role="radiogroup" aria-label="Colour theme">
      {#each APP_THEMES as t (t.id)}
        <label class="theme" class:on={appearance.theme === t.id}>
          <input
            type="radio"
            name="app-theme"
            value={t.id}
            checked={appearance.theme === t.id}
            onchange={() => (appearance.theme = t.id)}
          />
          {t.label}
        </label>
      {/each}
    </div>
  </section>
  {#snippet actions()}
    <button class="primary" onclick={onClose}>Done</button>
  {/snippet}
</Modal>

<style>
  h3 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-soft);
  }

  .themes {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .theme {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: var(--radius);
    cursor: pointer;
  }

  .theme.on {
    background: var(--felt-soft);
  }

  .theme:has(input:focus-visible) {
    outline: 2px solid var(--felt);
    outline-offset: 1px;
  }
</style>
