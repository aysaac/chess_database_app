// Typed calls to the server's /api. The session is the HttpOnly cookie;
// state-changing calls send the session's CSRF token.

import type {
  Account,
  AuditPage,
  Backups,
  Invite,
  Link,
  OwnedDatabase,
  AdminDatabase,
  AdminJob,
  Database,
  Disk,
  ImportFile,
  JobInfo,
  Me,
  NewToken,
  Role,
  SetupStatus,
  Token,
  User,
  Visibility,
} from "./types";

export class ApiError extends Error {
  status: number;
  code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

let csrf: string | null = null;

export function setCsrf(token: string | null): void {
  csrf = token;
}

/** Called when a request finds the session gone (401). */
let onSignedOut: () => void = () => {};

export function whenSignedOut(f: () => void): void {
  onSignedOut = f;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET" && csrf) headers["X-CSRF-Token"] = csrf;
  const res = await fetch(`/api${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  const text = await res.text();
  let data: unknown = undefined;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = undefined;
    }
  }
  if (!res.ok) {
    const err = (data as { error?: { code?: string; message?: string } } | undefined)?.error;
    if (res.status === 401 && path !== "/me" && path !== "/login") onSignedOut();
    throw new ApiError(res.status, err?.code ?? "error", err?.message ?? `${res.status} ${res.statusText}`);
  }
  return data as T;
}

const enc = encodeURIComponent;

/** A link the server made (an invite, a reset link) as a full address. */
export function fullLink(url: string): string {
  return /^https?:\/\//.test(url) ? url : `${location.origin}${url}`;
}

/** Where a database's games download as PGN (the session cookie goes along). */
export function exportUrl(db: string): string {
  return `/api/databases/${encodeURIComponent(db)}/export.pgn`;
}

export const api = {
  me: () => request<Me>("GET", "/me"),
  setupStatus: () => request<SetupStatus>("GET", "/setup"),
  setup: (token: string, username: string, password: string) =>
    request<Me>("POST", "/setup", { token, username, password }),
  login: (username: string, password: string) => request<Me>("POST", "/login", { username, password }),
  logout: () => request<void>("POST", "/logout"),
  changePassword: (currentPassword: string, newPassword: string) =>
    request<void>("POST", "/me/password", { currentPassword, newPassword }),
  account: () => request<Account>("GET", "/me/account"),
  changeEmail: (currentPassword: string, email: string | null) =>
    request<void>("PUT", "/me/email", { currentPassword, email }),
  signup: (invite: string | null, username: string, password: string, email: string | null) =>
    request<Me>("POST", "/signup", { invite, username, password, email }),
  invite: (token: string) => request<{ role: Role }>("GET", `/invites/${enc(token)}`),
  resetPassword: (token: string, password: string) =>
    request<void>("POST", "/password-reset", { token, password }),
  forgotPassword: (login: string) => request<void>("POST", "/password-reset/request", { login }),
  tokens: () => request<Token[]>("GET", "/tokens"),
  createToken: (name: string) => request<NewToken>("POST", "/tokens", { name }),
  deleteToken: (id: number) => request<void>("DELETE", `/tokens/${id}`),

  users: () => request<User[]>("GET", "/admin/users"),
  createUser: (username: string, password: string, role: Role) =>
    request<User>("POST", "/admin/users", { username, password, role }),
  updateUser: (id: number, change: { role?: Role; disabled?: boolean; quotaBytes?: number | null }) =>
    request<User>("PATCH", `/admin/users/${id}`, change),
  setPassword: (id: number, password: string) =>
    request<void>("POST", `/admin/users/${id}/password`, { password }),
  userDatabases: (id: number) => request<OwnedDatabase[]>("GET", `/admin/users/${id}/databases`),
  deleteUser: (id: number, confirm: string) =>
    request<void>("DELETE", `/admin/users/${id}?confirm=${enc(confirm)}`),
  resetLink: (id: number) => request<Link>("POST", `/admin/users/${id}/reset-link`),
  invites: () => request<Invite[]>("GET", "/admin/invites"),
  createInvite: (role: Role, quotaBytes: number | null, days: number) =>
    request<Link>("POST", "/admin/invites", { role, quotaBytes, days }),
  deleteInvite: (id: number) => request<void>("DELETE", `/admin/invites/${id}`),

  allDatabases: () => request<AdminDatabase[]>("GET", "/admin/databases"),
  setDatabaseQuota: (id: string, quotaBytes: number | null) =>
    request<AdminDatabase>("PATCH", `/admin/databases/${enc(id)}`, { quotaBytes }),
  deleteDatabase: (id: string, confirm: string) =>
    request<void>("DELETE", `/admin/databases/${enc(id)}?confirm=${enc(confirm)}`),

  myDatabases: () => request<Database[]>("GET", "/databases"),
  createDatabase: (name: string, visibility: Visibility) =>
    request<Database>("POST", "/databases", { name, visibility }),
  importFiles: () => request<ImportFile[]>("GET", "/admin/import-files"),
  importFile: (db: string, path: string) =>
    request<{ job: number }>("POST", `/databases/${enc(db)}/import-file`, { path }),

  job: (id: number) => request<JobInfo>("GET", `/jobs/${id}`),
  jobs: () => request<AdminJob[]>("GET", "/admin/jobs"),
  cancelJob: (id: number) => request<void>("POST", `/admin/jobs/${id}/cancel`),
  disk: () => request<Disk>("GET", "/admin/disk"),
  backups: () => request<Backups>("GET", "/admin/backups"),
  backUp: () => request<{ job: number }>("POST", "/admin/backups"),
  audit: (q: { action: string; who: string; from: number | null; to: number | null; offset: number; limit: number }) => {
    const params = new URLSearchParams({ offset: String(q.offset), limit: String(q.limit) });
    if (q.action) params.set("action", q.action);
    if (q.who) params.set("who", q.who);
    if (q.from !== null) params.set("from", String(q.from));
    if (q.to !== null) params.set("to", String(q.to));
    return request<AuditPage>("GET", `/admin/audit?${params}`);
  },
};
