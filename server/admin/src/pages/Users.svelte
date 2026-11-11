<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { bytes, parseBytes, when } from "../lib/format";
  import type { Me, Role, User } from "../lib/types";

  let { me }: { me: Me } = $props();

  let users = $state<User[]>([]);
  let error = $state("");
  let notice = $state("");

  let newName = $state("");
  let newPassword = $state("");
  let newRole = $state<Role>("user");
  let creating = $state(false);

  // The dialog's user and what it edits.
  let dialog: HTMLDialogElement | undefined = $state();
  let editing = $state<User | null>(null);
  let mode = $state<"password" | "quota">("password");
  let field = $state("");
  let dialogError = $state("");

  async function refresh() {
    try {
      users = await api.users();
      error = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function act(f: () => Promise<unknown>, done: string) {
    error = "";
    notice = "";
    try {
      await f();
      notice = done;
      await refresh();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function create(e: SubmitEvent) {
    e.preventDefault();
    creating = true;
    const name = newName.trim();
    await act(() => api.createUser(name, newPassword, newRole), `Made ${name}.`);
    if (!error) {
      newName = "";
      newPassword = "";
      newRole = "user";
    }
    creating = false;
  }

  function open(u: User, m: "password" | "quota") {
    editing = u;
    mode = m;
    field = m === "quota" && u.quotaBytes !== null ? bytes(u.quotaBytes) : "";
    dialogError = "";
    dialog?.showModal();
  }

  async function save(e: SubmitEvent) {
    e.preventDefault();
    const u = editing;
    if (!u) return;
    dialogError = "";
    try {
      if (mode === "password") {
        await api.setPassword(u.id, field);
        notice = `Set a new password for ${u.username}; their sessions ended.`;
      } else {
        const quota = parseBytes(field);
        if (quota === undefined) {
          dialogError = "Write a size such as 500 MB or 2 GB, or leave it empty for none.";
          return;
        }
        await api.updateUser(u.id, { quotaBytes: quota });
        notice = `Set the quota of ${u.username}.`;
      }
      dialog?.close();
      await refresh();
    } catch (err) {
      dialogError = err instanceof Error ? err.message : String(err);
    }
  }

  onMount(refresh);
</script>

<section class="panel">
  <h2>Users</h2>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="success" role="status">{notice}</p>{/if}
  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th>Username</th>
          <th>Role</th>
          <th>Status</th>
          <th class="num">Databases</th>
          <th class="num">Used</th>
          <th class="num">Quota</th>
          <th>Made</th>
          <th></th>
        </tr>
      </thead>
      <tbody>
        {#each users as u (u.id)}
          <tr>
            <td>{u.username}{#if u.id === me.id}<span class="muted you">(you)</span>{/if}</td>
            <td><span class="badge" class:admin={u.role === "admin"}>{u.role}</span></td>
            <td>
              {#if u.disabled}<span class="badge off">disabled</span>{:else}<span class="muted">active</span>{/if}
            </td>
            <td class="num">{u.databases}</td>
            <td class="num">{bytes(u.usedBytes)}</td>
            <td class="num">{u.quotaBytes === null ? "none" : bytes(u.quotaBytes)}</td>
            <td class="muted small">{when(u.createdAt)}</td>
            <td><div class="actions">
              {#if u.id !== me.id}
                <button
                  onclick={() =>
                    act(
                      () => api.updateUser(u.id, { disabled: !u.disabled }),
                      u.disabled ? `Enabled ${u.username}.` : `Disabled ${u.username}: their sessions and tokens stopped.`,
                    )}>{u.disabled ? "Enable" : "Disable"}</button
                >
                <button
                  onclick={() =>
                    act(
                      () => api.updateUser(u.id, { role: u.role === "admin" ? "user" : "admin" }),
                      `${u.username} is ${u.role === "admin" ? "a user" : "an admin"} now.`,
                    )}>{u.role === "admin" ? "Make user" : "Make admin"}</button
                >
              {/if}
              <button onclick={() => open(u, "password")}>New password</button>
              <button onclick={() => open(u, "quota")}>Quota</button>
            </div></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<section class="panel">
  <h2>New user</h2>
  <form class="form-row" onsubmit={create}>
    <label>Username <input bind:value={newName} required maxlength="32" autocomplete="off" /></label>
    <label
      >Password <input type="password" bind:value={newPassword} required minlength="8" autocomplete="new-password" /></label
    >
    <label
      >Role
      <select bind:value={newRole}>
        <option value="user">User</option>
        <option value="admin">Admin</option>
      </select>
    </label>
    <button class="primary" type="submit" disabled={creating}>Make the user</button>
  </form>
  <p class="muted small spaced">
    Disabling a user stops their sessions and API tokens at once and keeps their databases. A user's quota covers
    all the databases they own; an import stops when it's reached.
  </p>
</section>

<dialog bind:this={dialog}>
  {#if editing}
    <form onsubmit={save}>
      {#if mode === "password"}
        <h3>New password for {editing.username}</h3>
        <label
          >Password (8 characters or more)
          <input type="password" bind:value={field} minlength="8" required autocomplete="new-password" /></label
        >
        <p class="muted small">Their browser sessions end; their API tokens keep working.</p>
      {:else}
        <h3>Quota for {editing.username}</h3>
        <label>Most their databases may take together (e.g. 2 GB; empty for none) <input bind:value={field} /></label>
      {/if}
      {#if dialogError}<p class="error small" role="alert">{dialogError}</p>{/if}
      <div class="dialog-buttons">
        <button type="button" onclick={() => dialog?.close()}>Cancel</button>
        <button class="primary" type="submit">Save</button>
      </div>
    </form>
  {/if}
</dialog>

<style>
  .you {
    margin-left: 0.3rem;
  }
  .spaced {
    margin-top: 0.8rem;
  }
</style>
