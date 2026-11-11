// The JSON of server/src/wire.rs that the admin UI uses (written by hand:
// keep in step with it).

export type Role = "admin" | "user";
export type Visibility = "private" | "shared" | "server";
export type Access = "read" | "write" | "owner";
export type JobState = "running" | "done" | "failed" | "cancelled";

export interface Me {
  id: number;
  username: string;
  role: Role;
  csrfToken: string | null;
}

export interface SetupStatus {
  needed: boolean;
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
