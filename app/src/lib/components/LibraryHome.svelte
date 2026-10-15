<script lang="ts">
  // The app's main window (DB-07): the database manager, shown first
  // instead of an editor. Opening or creating a database spawns a separate
  // editor window for it (main.ts picks this shell for the "main" window,
  // App.svelte for every "editor-…" one it opens).
  import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
  import { api } from "../api";
  import { db, inDocuments } from "../stores/db.svelte";
  import { notices } from "../stores/notify.svelte";
  import Icon from "./Icon.svelte";
  import LibraryBrowser from "./LibraryBrowser.svelte";
  import SettingsDialog from "./SettingsDialog.svelte";

  let showSettings = $state(false);
  let busy = $state(false);

  async function open(path: string) {
    try {
      await api.openEditorWindow(path);
      await db.loadRecent();
    } catch (e) {
      notices.error(e);
    }
  }

  async function newDatabase() {
    const path = await save({
      title: "Create a database",
      defaultPath: await inDocuments("My games.chessdb"),
    });
    if (!path || busy) return;
    busy = true;
    try {
      await api.createDb(path);
      await open(path);
    } finally {
      busy = false;
    }
  }

  async function openExisting() {
    const path = await openDialog({
      directory: true,
      title: "Open a database (.chessdb folder)",
      defaultPath: await inDocuments(),
    });
    if (typeof path === "string") await open(path);
  }
</script>

<div class="home">
  <header>
    <div class="row title">
      <Icon name="database" size={18} />
      <span>Database manager</span>
    </div>
    <div class="spacer"></div>
    <button class="btn" disabled={busy} onclick={openExisting}>Open database…</button>
    <button class="btn primary" disabled={busy} onclick={newDatabase}>+ New database</button>
  </header>

  <LibraryBrowser onOpen={open} onSettings={() => (showSettings = true)} />
</div>

{#if showSettings}
  <SettingsDialog onClose={() => (showSettings = false)} />
{/if}

<style>
  .home {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--paper);
    color: var(--ink);
  }

  .row {
    display: flex;
    align-items: center;
  }

  header {
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 10px;
    min-height: 48px;
    flex: none;
    padding: 8px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--rule);
  }

  .title {
    gap: 8px;
    font-family: var(--font-book);
    font-weight: 700;
    font-size: 16px;
  }

  .spacer {
    flex: 1;
  }

  .btn {
    height: 30px;
    padding: 0 12px;
    border-radius: 6px;
    border: 1px solid var(--rule);
    background: var(--paper);
    color: var(--ink);
    font-size: 13px;
  }

  .btn.primary {
    border-color: var(--brass);
    background: var(--brass);
    color: #fff;
    font-weight: 600;
  }
</style>
