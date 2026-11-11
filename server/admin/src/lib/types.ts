// The JSON of server/src/wire.rs that the admin UI uses (written by hand:
// keep in step with it).

export type Role = "admin" | "user";
export type Visibility = "private" | "shared" | "server";
export type Access = "read" | "write" | "owner";
export type JobState = "running" | "done" | "failed" | "cancelled" | "interrupted";

export interface Me {
  id: number;
  username: string;
  role: Role;
  csrfToken: string | null;
}

export interface SetupStatus {
  needed: boolean;
  /** Anyone may make an account. */
  openSignup: boolean;
  /** "Forgot password" can e-mail a reset link. */
  emailResets: boolean;
}

/** One's own account (`GET /api/me/account`). */
export interface Account {
  username: string;
  role: Role;
  email: string | null;
  quotaBytes: number | null;
  usedBytes: number;
  /** "Forgot password" can e-mail a link. */
  emailResets: boolean;
}

/** A link with a one-time token: an invite or a reset link, shown once. */
export interface Link {
  token: string;
  /** The page taking it: absolute with a public URL set, else a path. */
  url: string;
  expiresAt: number;
  id?: number;
}

export interface Invite {
  id: number;
  role: Role;
  quotaBytes: number | null;
  createdBy: string | null;
  createdAt: number;
  expiresAt: number;
  usedAt: number | null;
  usedBy: string | null;
}

export interface AuditEntry {
  at: number;
  actor: string | null;
  action: string;
  targetKind: string | null;
  targetId: string | null;
  target: string | null;
  detail: string | null;
  client: string | null;
}

export interface AuditPage {
  total: number;
  offset: number;
  days: number;
  entries: AuditEntry[];
}

export interface Backup {
  name: string;
  sizeBytes: number;
  databases: number;
  createdAt: number;
}

export interface Backups {
  configured: boolean;
  everyHours: number | null;
  keep: number;
  backups: Backup[];
}

export interface Twic {
  database: string | null;
  databaseName: string | null;
  everyDays: number;
  lastCheck: number | null;
  lastIssue: number | null;
  issues: { issue: number; importedAt: number; games: number | null }[];
}

/** A database deleted with its owner (metadata only). */
export interface OwnedDatabase {
  id: string;
  name: string;
  games: number;
  sizeBytes: number;
}

export interface User {
  id: number;
  username: string;
  role: Role;
  disabled: boolean;
  quotaBytes: number | null;
  createdAt: number;
  databases: number;
  usedBytes: number;
}

/** A database as the admin sees any of them: metadata only. */
export interface AdminDatabase {
  id: string;
  name: string;
  owner: string;
  ownerId: number;
  visibility: Visibility;
  games: number;
  sizeBytes: number;
  quotaBytes: number | null;
  createdAt: number;
  /** What the admin may do with it as any user; null for another user's private database. */
  access: Access | null;
}

/** A database the caller can read (`GET /api/databases`). */
export interface Database {
  id: string;
  name: string;
  owner: string;
  visibility: Visibility;
  access: Access;
  games: number;
  sizeBytes: number;
  quotaBytes: number | null;
  createdAt: number;
}

export interface AdminJob {
  id: number;
  kind: string;
  state: JobState;
  done: number;
  total: number;
  database: string;
  databaseName: string;
  startedBy: string;
  startedAt: number;
}

export interface ImportReport {
  file: string;
  imported: number;
  skipped: number;
  warningCount: number;
}

export interface JobInfo {
  id: number;
  kind: string;
  label: string;
  state: JobState;
  done: number;
  total: number;
  message: string;
  error: string | null;
  database: string;
  report: ImportReport | null;
}

export interface Disk {
  dataDir: string;
  fsTotalBytes: number | null;
  fsFreeBytes: number | null;
  databases: number;
  databasesBytes: number;
  metadataBytes: number;
  uploadsBytes: number;
  importBytes: number;
}

export interface ImportFile {
  path: string;
  sizeBytes: number;
  kind: "pgn" | "cbh";
}

export interface Token {
  id: number;
  name: string;
  createdAt: number;
  lastUsedAt: number | null;
}

export interface NewToken {
  id: number;
  name: string;
  token: string;
}
