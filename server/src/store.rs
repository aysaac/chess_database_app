//! The metadata store (`server.sqlite` in the data directory): users,
//! browser sessions, API tokens, databases and their shares. Games are
//! never in it; they stay in each database's own files.
//!
//! One connection behind a mutex; every query is small. Callers run it off
//! the async threads (`spawn_blocking`). WAL mode lets `chess-server admin`
//! commands write while the server runs.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{ApiError, ApiResult};
use crate::wire::{Role, ShareAccess, Visibility};

/// Schema version, in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 2;

/// From version 1 to 2: game id epochs (compaction), e-mail addresses,
/// ownership transfers, invites, password resets, jobs, the audit log,
/// settings and the TWIC issues imported (NET-06).
const MIGRATE_2: &str = "
ALTER TABLE databases ADD COLUMN id_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN email TEXT;
CREATE UNIQUE INDEX users_email ON users(email COLLATE NOCASE) WHERE email IS NOT NULL;
CREATE TABLE transfers (
    database_id TEXT PRIMARY KEY REFERENCES databases(id) ON DELETE CASCADE,
    from_user INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_user INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
CREATE INDEX transfers_to ON transfers(to_user);
CREATE TABLE invites (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    quota_bytes INTEGER,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at INTEGER,
    used_by INTEGER REFERENCES users(id) ON DELETE SET NULL
);
CREATE TABLE password_resets (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX password_resets_user ON password_resets(user_id);
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    database_id TEXT REFERENCES databases(id) ON DELETE CASCADE,
    database_name TEXT NOT NULL,
    user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    username TEXT NOT NULL,
    state TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    message TEXT NOT NULL DEFAULT '',
    error TEXT,
    report TEXT,
    resume TEXT,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX jobs_database ON jobs(database_id);
CREATE TABLE audit (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    actor_id INTEGER,
    actor TEXT,
    action TEXT NOT NULL,
    target_kind TEXT,
    target_id TEXT,
    target TEXT,
    detail TEXT,
    client TEXT
);
CREATE INDEX audit_at ON audit(at);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE twic_issues (
    issue INTEGER PRIMARY KEY,
    database_id TEXT,
    imported_at INTEGER NOT NULL,
    games INTEGER NOT NULL
);
";

const SCHEMA: &str = "
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'user')),
    disabled INTEGER NOT NULL DEFAULT 0,
    quota_bytes INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE api_tokens (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE TABLE databases (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    owner_id INTEGER NOT NULL REFERENCES users(id),
    visibility TEXT NOT NULL CHECK (visibility IN ('private', 'shared', 'server')),
    quota_bytes INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX databases_owner ON databases(owner_id);
CREATE TABLE shares (
    database_id TEXT NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    access TEXT NOT NULL CHECK (access IN ('read', 'write')),
    PRIMARY KEY (database_id, user_id)
);
CREATE INDEX shares_user ON shares(user_id);
";

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    pub quota_bytes: Option<u64>,
    pub created_at: i64,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

/// A database's row: who owns it and who else may read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbMeta {
    pub id: String,
    pub name: String,
    pub owner_id: i64,
    pub owner: String,
    pub visibility: Visibility,
    pub quota_bytes: Option<u64>,
    pub created_at: i64,
    /// Bumped by each compaction, which renumbers the games: a game id
    /// read before one means another game after it.
    pub id_epoch: u64,
}

/// Who and what a job is for, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobMetaRow {
    pub db: String,
    pub db_name: String,
    pub user: i64,
    pub username: String,
    pub started_at: i64,
}

/// A job as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub id: u64,
    pub kind: String,
    pub label: String,
    /// None once its database is gone (then the row goes too).
    pub db: Option<String>,
    pub db_name: String,
    /// None once the user who started it was deleted.
    pub user: Option<i64>,
    pub username: String,
    /// `running`, `done`, `failed`, `cancelled` or `interrupted`.
    pub state: String,
    pub done: u64,
    pub total: u64,
    pub message: String,
    pub error: Option<String>,
    pub report: Option<String>,
    pub resume: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

const JOB_COLUMNS: &str = "id, kind, label, database_id, database_name, user_id, username, state, done, total, \
                           message, error, report, resume, started_at, finished_at";

fn job_row(r: &Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get::<_, i64>(0)?.max(0) as u64,
        kind: r.get(1)?,
        label: r.get(2)?,
        db: r.get(3)?,
        db_name: r.get(4)?,
        user: r.get(5)?,
        username: r.get(6)?,
        state: r.get(7)?,
        done: r.get::<_, i64>(8)?.max(0) as u64,
        total: r.get::<_, i64>(9)?.max(0) as u64,
        message: r.get(10)?,
        error: r.get(11)?,
        report: r.get(12)?,
        resume: r.get(13)?,
        started_at: r.get(14)?,
        finished_at: r.get(15)?,
    })
}

/// An invite, as the admin sees it (never its token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteRow {
    pub id: i64,
    pub role: Role,
    pub quota_bytes: Option<u64>,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
    pub used_by: Option<String>,
}

pub fn invalid_invite() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_REQUEST,
        "badInvite",
        "This invite is used up, has expired or doesn't exist: ask for another",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

const USER_COLUMNS: &str = "id, username, role, disabled, quota_bytes, created_at";
const DB_COLUMNS: &str =
    "d.id, d.name, d.owner_id, u.username, d.visibility, d.quota_bytes, d.created_at, d.id_epoch";

fn user_row(r: &Row<'_>) -> rusqlite::Result<User> {
    let role: String = r.get(2)?;
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        role: Role::parse(&role).unwrap_or(Role::User),
        disabled: r.get::<_, i64>(3)? != 0,
        quota_bytes: r.get::<_, Option<i64>>(4)?.map(|q| q.max(0) as u64),
        created_at: r.get(5)?,
    })
}

fn db_row(r: &Row<'_>) -> rusqlite::Result<DbMeta> {
    let visibility: String = r.get(4)?;
    Ok(DbMeta {
        id: r.get(0)?,
        name: r.get(1)?,
        owner_id: r.get(2)?,
        owner: r.get(3)?,
        // The CHECK constraint keeps it one of the three; private is the safe reading.
        visibility: Visibility::parse(&visibility).unwrap_or(Visibility::Private),
        quota_bytes: r.get::<_, Option<i64>>(5)?.map(|q| q.max(0) as u64),
        created_at: r.get(6)?,
        id_epoch: r.get::<_, i64>(7)?.max(0) as u64,
    })
}

fn quota(q: Option<u64>) -> Option<i64> {
    q.map(|q| i64::try_from(q).unwrap_or(i64::MAX))
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(&format!(
                    "BEGIN; {SCHEMA} {MIGRATE_2} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))?;
            }
            1 => {
                log::info!("updating the metadata store to schema version {SCHEMA_VERSION}");
                conn.execute_batch(&format!(
                    "BEGIN; {MIGRATE_2} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))?;
            }
            SCHEMA_VERSION => {}
            newer => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "{} has schema version {newer}; this server knows {SCHEMA_VERSION}: run a newer one",
                    path.display()
                )));
            }
        }
        Ok(Store { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("metadata store lock")
    }

    // ---- users -------------------------------------------------------------

    pub fn user_count(&self) -> ApiResult<u64> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// Fails with a conflict when the name is taken (in any case).
    pub fn create_user(&self, username: &str, password_hash: &str, role: Role) -> ApiResult<User> {
        Store::insert_user(&self.conn(), username, password_hash, role)
    }

    /// Makes the first user, an admin, unless there is a user already
    /// (checked under the same lock, so two setups can't both succeed).
    pub fn create_first_admin(&self, username: &str, password_hash: &str) -> ApiResult<Option<User>> {
        let conn = self.conn();
        if conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0))? > 0 {
            return Ok(None);
        }
        Store::insert_user(&conn, username, password_hash, Role::Admin).map(Some)
    }

    fn insert_user(conn: &Connection, username: &str, password_hash: &str, role: Role) -> ApiResult<User> {
        let created = now();
        match conn.execute(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![username, password_hash, role.as_str(), created],
        ) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(ApiError::conflict(format!("The username {username} is taken")));
            }
            Err(e) => return Err(e.into()),
        }
        Ok(User {
            id: conn.last_insert_rowid(),
            username: username.to_owned(),
            role,
            disabled: false,
            quota_bytes: None,
            created_at: created,
        })
    }

    pub fn user(&self, id: i64) -> ApiResult<Option<User>> {
        Ok(self
            .conn()
            .query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"), [id], user_row)
            .optional()?)
    }

    /// The user (any case of the name) and their password hash.
    pub fn user_by_name(&self, username: &str) -> ApiResult<Option<(User, String)>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {USER_COLUMNS}, password_hash FROM users WHERE username = ?1"),
                [username],
                |r| Ok((user_row(r)?, r.get(6)?)),
            )
            .optional()?)
    }

    pub fn users(&self) -> ApiResult<Vec<User>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY username"))?;
        let rows = stmt.query_map([], user_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Enabled admins.
    pub fn active_admins(&self) -> ApiResult<u64> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    pub fn update_user(
        &self,
        id: i64,
        role: Option<Role>,
        disabled: Option<bool>,
        quota_bytes: Option<Option<u64>>,
    ) -> ApiResult<()> {
        let conn = self.conn();
        if let Some(role) = role {
            conn.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![role.as_str(), id])?;
        }
        if let Some(disabled) = disabled {
            conn.execute("UPDATE users SET disabled = ?1 WHERE id = ?2", params![disabled as i64, id])?;
        }
        if let Some(q) = quota_bytes {
            conn.execute("UPDATE users SET quota_bytes = ?1 WHERE id = ?2", params![quota(q), id])?;
        }
        Ok(())
    }

    pub fn set_password(&self, id: i64, password_hash: &str) -> ApiResult<()> {
        self.conn()
            .execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![password_hash, id])?;
        Ok(())
    }

    // ---- sessions ----------------------------------------------------------

    pub fn create_session(
        &self,
        token_hash: &[u8],
        user: i64,
        csrf: &str,
        lifetime_secs: i64,
    ) -> ApiResult<()> {
        let created = now();
        let conn = self.conn();
        // Sessions that ran out go now and then, here.
        conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [created])?;
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![token_hash, user, csrf, created, created + lifetime_secs],
        )?;
        Ok(())
    }

    /// The session's user and CSRF token, if it hasn't run out and the user
    /// isn't disabled.
    pub fn session(&self, token_hash: &[u8]) -> ApiResult<Option<(User, String)>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {}, s.csrf FROM sessions s JOIN users u ON u.id = s.user_id
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
                    USER_COLUMNS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                ),
                params![token_hash, now()],
                |r| Ok((user_row(r)?, r.get(6)?)),
            )
            .optional()?)
    }

    pub fn delete_session(&self, token_hash: &[u8]) -> ApiResult<()> {
        self.conn().execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    /// Logs the user out everywhere, except the session `keep`.
    pub fn delete_sessions_of(&self, user: i64, keep: Option<&[u8]>) -> ApiResult<()> {
        self.conn().execute(
            "DELETE FROM sessions WHERE user_id = ?1 AND token_hash IS NOT ?2",
            params![user, keep],
        )?;
        Ok(())
    }

    // ---- API tokens --------------------------------------------------------

    pub fn create_token(&self, user: i64, name: &str, token_hash: &[u8]) -> ApiResult<TokenRow> {
        let conn = self.conn();
        let created = now();
        conn.execute(
            "INSERT INTO api_tokens (user_id, token_hash, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![user, token_hash, name, created],
        )?;
        Ok(TokenRow {
            id: conn.last_insert_rowid(),
            name: name.to_owned(),
            created_at: created,
            last_used_at: None,
        })
    }

    pub fn tokens(&self, user: i64) -> ApiResult<Vec<TokenRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, last_used_at FROM api_tokens WHERE user_id = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([user], |r| {
                Ok(TokenRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    last_used_at: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// False when the user has no such token.
    pub fn delete_token(&self, user: i64, id: i64) -> ApiResult<bool> {
        Ok(self.conn().execute("DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2", params![id, user])?
            > 0)
    }

    /// The token's user, if they aren't disabled; notes when it was used.
    pub fn token_user(&self, token_hash: &[u8]) -> ApiResult<Option<User>> {
        let conn = self.conn();
        let user = conn
            .query_row(
                &format!(
                    "SELECT {} FROM api_tokens t JOIN users u ON u.id = t.user_id
                     WHERE t.token_hash = ?1 AND u.disabled = 0",
                    USER_COLUMNS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                ),
                [token_hash],
                user_row,
            )
            .optional()?;
        if user.is_some() {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?1 WHERE token_hash = ?2",
                params![now(), token_hash],
            )?;
        }
        Ok(user)
    }

    // ---- databases ---------------------------------------------------------

    pub fn create_database(
        &self,
        id: &str,
        name: &str,
        owner: i64,
        visibility: Visibility,
    ) -> ApiResult<DbMeta> {
        self.conn().execute(
            "INSERT INTO databases (id, name, owner_id, visibility, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, owner, visibility.as_str(), now()],
        )?;
        self.database(id)?.ok_or_else(|| ApiError::internal("a database row vanished"))
    }

    pub fn database(&self, id: &str) -> ApiResult<Option<DbMeta>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {DB_COLUMNS} FROM databases d JOIN users u ON u.id = d.owner_id WHERE d.id = ?1"
                ),
                [id],
                db_row,
            )
            .optional()?)
    }

    pub fn all_databases(&self) -> ApiResult<Vec<DbMeta>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DB_COLUMNS} FROM databases d JOIN users u ON u.id = d.owner_id ORDER BY d.name, d.id"
        ))?;
        let rows = stmt.query_map([], db_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every database `user` could read, with their share in it: those they
    /// own, those shared with them (unless private), and the server-wide
    /// ones. Access is decided by [`crate::access::access`] all the same.
    pub fn databases_for(&self, user: i64) -> ApiResult<Vec<(DbMeta, Option<ShareAccess>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DB_COLUMNS}, s.access FROM databases d
             JOIN users u ON u.id = d.owner_id
             LEFT JOIN shares s ON s.database_id = d.id AND s.user_id = ?1
             WHERE d.owner_id = ?1
                OR d.visibility = 'server'
                OR (d.visibility = 'shared' AND s.user_id IS NOT NULL)
             ORDER BY d.name, d.id"
        ))?;
        let rows = stmt
            .query_map([user], |r| {
                let access: Option<String> = r.get(8)?;
                Ok((db_row(r)?, access.as_deref().and_then(ShareAccess::parse)))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn databases_owned_by(&self, user: i64) -> ApiResult<Vec<DbMeta>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DB_COLUMNS} FROM databases d JOIN users u ON u.id = d.owner_id WHERE d.owner_id = ?1"
        ))?;
        let rows = stmt.query_map([user], db_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn rename_database(&self, id: &str, name: &str) -> ApiResult<()> {
        self.conn().execute("UPDATE databases SET name = ?1 WHERE id = ?2", params![name, id])?;
        Ok(())
    }

    pub fn set_visibility(&self, id: &str, visibility: Visibility) -> ApiResult<()> {
        self.conn().execute(
            "UPDATE databases SET visibility = ?1 WHERE id = ?2",
            params![visibility.as_str(), id],
        )?;
        Ok(())
    }

    /// Notes a compaction: the games were renumbered.
    pub fn bump_id_epoch(&self, id: &str) -> ApiResult<()> {
        self.conn().execute("UPDATE databases SET id_epoch = id_epoch + 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_database_quota(&self, id: &str, quota_bytes: Option<u64>) -> ApiResult<()> {
        self.conn().execute(
            "UPDATE databases SET quota_bytes = ?1 WHERE id = ?2",
            params![quota(quota_bytes), id],
        )?;
        Ok(())
    }

    /// Deletes the row and its shares; false when there was none.
    pub fn delete_database(&self, id: &str) -> ApiResult<bool> {
        Ok(self.conn().execute("DELETE FROM databases WHERE id = ?1", [id])? > 0)
    }

    // ---- shares ------------------------------------------------------------

    pub fn share(&self, db: &str, user: i64) -> ApiResult<Option<ShareAccess>> {
        let access: Option<String> = self
            .conn()
            .query_row(
                "SELECT access FROM shares WHERE database_id = ?1 AND user_id = ?2",
                params![db, user],
                |r| r.get(0),
            )
            .optional()?;
        Ok(access.as_deref().and_then(ShareAccess::parse))
    }

    pub fn shares(&self, db: &str) -> ApiResult<Vec<(String, ShareAccess)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT u.username, s.access FROM shares s JOIN users u ON u.id = s.user_id
             WHERE s.database_id = ?1 ORDER BY u.username",
        )?;
        let rows = stmt
            .query_map([db], |r| {
                let access: String = r.get(1)?;
                Ok((r.get(0)?, ShareAccess::parse(&access).unwrap_or(ShareAccess::Read)))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_share(&self, db: &str, user: i64, access: ShareAccess) -> ApiResult<()> {
        self.conn().execute(
            "INSERT INTO shares (database_id, user_id, access) VALUES (?1, ?2, ?3)
             ON CONFLICT (database_id, user_id) DO UPDATE SET access = excluded.access",
            params![db, user, access.as_str()],
        )?;
        Ok(())
    }

    pub fn remove_share(&self, db: &str, user: i64) -> ApiResult<bool> {
        Ok(self
            .conn()
            .execute("DELETE FROM shares WHERE database_id = ?1 AND user_id = ?2", params![db, user])?
            > 0)
    }

    // ---- e-mail addresses ----------------------------------------------------

    pub fn email_of(&self, user: i64) -> ApiResult<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT email FROM users WHERE id = ?1", [user], |r| r.get(0))
            .optional()?
            .flatten())
    }

    /// Sets (or with `None` removes) a user's address; a conflict when
    /// another account has it (in any case).
    pub fn set_email(&self, user: i64, email: Option<&str>) -> ApiResult<()> {
        match self.conn().execute("UPDATE users SET email = ?1 WHERE id = ?2", params![email, user]) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(ApiError::conflict("Another account has that address"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The enabled user with this username or e-mail address (any case),
    /// and their address.
    pub fn user_by_login(&self, login: &str) -> ApiResult<Option<(User, Option<String>)>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {USER_COLUMNS}, email FROM users
                     WHERE (username = ?1 OR email = ?1 COLLATE NOCASE) AND disabled = 0 ORDER BY id LIMIT 1"
                ),
                [login],
                |r| Ok((user_row(r)?, r.get(6)?)),
            )
            .optional()?)
    }

    /// Deletes a user: their sessions, tokens, shares, transfers and reset
    /// links go with them. Their databases must be gone first. False when
    /// there was none.
    pub fn delete_user(&self, user: i64) -> ApiResult<bool> {
        Ok(self.conn().execute("DELETE FROM users WHERE id = ?1", [user])? > 0)
    }

    // ---- invites -------------------------------------------------------------

    pub fn create_invite(
        &self,
        token_hash: &[u8],
        role: Role,
        quota_bytes: Option<u64>,
        by: i64,
        expires_at: i64,
    ) -> ApiResult<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO invites (token_hash, role, quota_bytes, created_by, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![token_hash, role.as_str(), quota(quota_bytes), by, now(), expires_at],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn invites(&self) -> ApiResult<Vec<InviteRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT i.id, i.role, i.quota_bytes, c.username, i.created_at, i.expires_at, i.used_at, u.username
             FROM invites i LEFT JOIN users c ON c.id = i.created_by LEFT JOIN users u ON u.id = i.used_by
             ORDER BY i.id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let role: String = r.get(1)?;
                Ok(InviteRow {
                    id: r.get(0)?,
                    role: Role::parse(&role).unwrap_or(Role::User),
                    quota_bytes: r.get::<_, Option<i64>>(2)?.map(|q| q.max(0) as u64),
                    created_by: r.get(3)?,
                    created_at: r.get(4)?,
                    expires_at: r.get(5)?,
                    used_at: r.get(6)?,
                    used_by: r.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_invite(&self, id: i64) -> ApiResult<bool> {
        Ok(self.conn().execute("DELETE FROM invites WHERE id = ?1", [id])? > 0)
    }

    /// The role and quota of an invite that's unused and hasn't expired.
    pub fn open_invite(&self, token_hash: &[u8]) -> ApiResult<Option<(Role, Option<u64>)>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT role, quota_bytes FROM invites WHERE token_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![token_hash, now()],
                |r| {
                    let role: String = r.get(0)?;
                    Ok((Role::parse(&role).unwrap_or(Role::User), r.get::<_, Option<i64>>(1)?.map(|q| q.max(0) as u64)))
                },
            )
            .optional()?)
    }

    /// Makes a user, with an invite (used up by it, in the same
    /// transaction, so it makes one account only) or without one (open
    /// sign-up: a user with `quota`).
    pub fn sign_up(
        &self,
        invite: Option<&[u8]>,
        username: &str,
        password_hash: &str,
        email: Option<&str>,
        quota_bytes: Option<u64>,
    ) -> ApiResult<User> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (invite_id, role, quota_bytes) = match invite {
            Some(hash) => {
                let found = tx
                    .query_row(
                        "SELECT id, role, quota_bytes FROM invites
                         WHERE token_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                        params![hash, now()],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)),
                    )
                    .optional()?;
                let (id, role, q) = found.ok_or_else(invalid_invite)?;
                (Some(id), Role::parse(&role).unwrap_or(Role::User), q.map(|q| q.max(0) as u64))
            }
            None => (None, Role::User, quota_bytes),
        };
        let mut user = Store::insert_user(&tx, username, password_hash, role)?;
        if let Some(q) = quota_bytes {
            tx.execute("UPDATE users SET quota_bytes = ?1 WHERE id = ?2", params![quota(Some(q)), user.id])?;
            user.quota_bytes = Some(q);
        }
        if let Some(email) = email {
            match tx.execute("UPDATE users SET email = ?1 WHERE id = ?2", params![email, user.id]) {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    return Err(ApiError::conflict("Another account has that address"));
                }
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(id) = invite_id {
            tx.execute(
                "UPDATE invites SET used_at = ?1, used_by = ?2 WHERE id = ?3",
                params![now(), user.id, id],
            )?;
        }
        tx.commit()?;
        Ok(user)
    }

    // ---- password resets -----------------------------------------------------

    /// A new one-time reset link for `user`; earlier ones stop working.
    pub fn create_reset(&self, user: i64, token_hash: &[u8], expires_at: i64) -> ApiResult<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM password_resets WHERE user_id = ?1 OR expires_at <= ?2",
            params![user, now()],
        )?;
        conn.execute(
            "INSERT INTO password_resets (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![token_hash, user, now(), expires_at],
        )?;
        Ok(())
    }

    /// Sets a new password with a reset link: the user's links go, and so do
    /// their sessions. None when the link is wrong, used or expired.
    pub fn reset_password(&self, token_hash: &[u8], password_hash: &str) -> ApiResult<Option<User>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let user: Option<i64> = tx
            .query_row(
                "SELECT r.user_id FROM password_resets r JOIN users u ON u.id = r.user_id
                 WHERE r.token_hash = ?1 AND r.expires_at > ?2 AND u.disabled = 0",
                params![token_hash, now()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(user) = user else { return Ok(None) };
        tx.execute("UPDATE users SET password_hash = ?1 WHERE id = ?2", params![password_hash, user])?;
        tx.execute("DELETE FROM password_resets WHERE user_id = ?1", [user])?;
        tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user])?;
        let found =
            tx.query_row(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"), [user], user_row)?;
        tx.commit()?;
        Ok(Some(found))
    }

    // ---- ownership transfers -------------------------------------------------

    /// Offers database `db` to user `to` (replacing an earlier offer).
    pub fn offer(&self, db: &str, from: i64, to: i64) -> ApiResult<()> {
        self.conn().execute(
            "INSERT INTO transfers (database_id, from_user, to_user, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (database_id) DO UPDATE SET from_user = excluded.from_user,
                 to_user = excluded.to_user, created_at = excluded.created_at",
            params![db, from, to, now()],
        )?;
        Ok(())
    }

    pub fn withdraw_offer(&self, db: &str) -> ApiResult<bool> {
        Ok(self.conn().execute("DELETE FROM transfers WHERE database_id = ?1", [db])? > 0)
    }

    /// The offers made to `user` and by them: (database, from, to, when),
    /// with the database's metadata.
    pub fn offers_for(&self, user: i64) -> ApiResult<Vec<(DbMeta, String, String, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DB_COLUMNS}, f.username, t.username, x.created_at FROM transfers x
             JOIN databases d ON d.id = x.database_id JOIN users u ON u.id = d.owner_id
             JOIN users f ON f.id = x.from_user JOIN users t ON t.id = x.to_user
             WHERE (x.to_user = ?1 AND t.disabled = 0) OR x.from_user = ?1 ORDER BY x.created_at"
        ))?;
        let rows = stmt
            .query_map([user], |r| Ok((db_row(r)?, r.get(8)?, r.get(9)?, r.get(10)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The offer of `db` to `to`, if there is one: who made it.
    pub fn offer_to(&self, db: &str, to: i64) -> ApiResult<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT from_user FROM transfers WHERE database_id = ?1 AND to_user = ?2",
                params![db, to],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Makes `to` the owner of `db`, if it's still offered to them by its
    /// owner: the offer goes, and so does any share `to` had (they own it
    /// now). The old owner keeps nothing of it.
    pub fn accept_offer(&self, db: &str, to: i64) -> ApiResult<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let moved = tx.execute(
            "UPDATE databases SET owner_id = ?2 WHERE id = ?1
             AND owner_id = (SELECT from_user FROM transfers WHERE database_id = ?1 AND to_user = ?2)",
            params![db, to],
        )?;
        if moved == 0 {
            return Ok(false);
        }
        tx.execute("DELETE FROM transfers WHERE database_id = ?1", [db])?;
        tx.execute("DELETE FROM shares WHERE database_id = ?1 AND user_id = ?2", params![db, to])?;
        tx.commit()?;
        Ok(true)
    }

    // ---- jobs ------------------------------------------------------------------

    pub fn max_job_id(&self) -> ApiResult<u64> {
        Ok(self.conn().query_row("SELECT COALESCE(MAX(id), 0) FROM jobs", [], |r| r.get::<_, i64>(0))?.max(0)
            as u64)
    }

    pub fn insert_job(&self, id: u64, kind: &str, label: &str, meta: &JobMetaRow) -> ApiResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO jobs (id, kind, label, database_id, database_name, user_id, username, state, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8)",
            params![id as i64, kind, label, meta.db, meta.db_name, meta.user, meta.username, meta.started_at],
        )?;
        // Finished jobs are kept, the newest thousand.
        conn.execute(
            "DELETE FROM jobs WHERE state != 'running' AND id <= (SELECT id FROM jobs ORDER BY id DESC LIMIT 1 OFFSET 1000)",
            [],
        )?;
        Ok(())
    }

    pub fn job_progress(&self, id: u64, done: u64, total: u64, message: &str) -> ApiResult<()> {
        self.conn().execute(
            "UPDATE jobs SET done = ?2, total = ?3, message = ?4 WHERE id = ?1",
            params![id as i64, done as i64, total as i64, message],
        )?;
        Ok(())
    }

    /// A job's end: its state (`done`, `failed`, `cancelled`, `interrupted`),
    /// and its report as JSON; how to resume it is forgotten.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_job(
        &self,
        id: u64,
        state: &str,
        error: Option<&str>,
        message: &str,
        done: u64,
        total: u64,
        report: Option<&str>,
    ) -> ApiResult<()> {
        self.conn().execute(
            "UPDATE jobs SET state = ?2, error = ?3, message = ?4, done = ?5, total = ?6,
                 report = COALESCE(?7, report), resume = NULL, finished_at = ?8 WHERE id = ?1",
            params![id as i64, state, error, message, done as i64, total as i64, report, now()],
        )?;
        Ok(())
    }

    /// How to go on with an import if the server stops (JSON).
    pub fn job_resume(&self, id: u64, resume: &str) -> ApiResult<()> {
        self.conn().execute("UPDATE jobs SET resume = ?2 WHERE id = ?1", params![id as i64, resume])?;
        Ok(())
    }

    pub fn job(&self, id: u64) -> ApiResult<Option<JobRow>> {
        Ok(self
            .conn()
            .query_row(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"), [id as i64], job_row)
            .optional()?)
    }

    /// The newest jobs, newest first.
    pub fn recent_jobs(&self, limit: u32) -> ApiResult<Vec<JobRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs ORDER BY id DESC LIMIT ?1"))?;
        let rows = stmt.query_map([limit], job_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Jobs the store says are running: at start, those a stop interrupted.
    pub fn running_jobs(&self) -> ApiResult<Vec<JobRow>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE state = 'running' ORDER BY id"))?;
        let rows = stmt.query_map([], job_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- settings ----------------------------------------------------------------

    pub fn setting(&self, key: &str) -> ApiResult<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> ApiResult<()> {
        let conn = self.conn();
        match value {
            Some(v) => conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, v],
            )?,
            None => conn.execute("DELETE FROM settings WHERE key = ?1", [key])?,
        };
        Ok(())
    }

    /// The size of the store's files.
    pub fn size_bytes(path: &Path) -> u64 {
        ["", "-wal", "-shm"]
            .iter()
            .filter_map(|suffix| {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                std::fs::metadata(std::path::PathBuf::from(p)).ok()
            })
            .map(|m| m.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("s.sqlite")).unwrap();
        (dir, store)
    }

    #[test]
    fn users_are_unique_in_any_case() {
        let (_dir, s) = store();
        let a = s.create_user("Alice", "h", Role::Admin).unwrap();
        assert_eq!(s.create_user("alice", "h", Role::User).unwrap_err().status, 409);
        let (found, hash) = s.user_by_name("ALICE").unwrap().unwrap();
        assert_eq!((found.id, hash.as_str()), (a.id, "h"));
        assert_eq!(s.active_admins().unwrap(), 1);
        s.update_user(a.id, None, Some(true), Some(Some(10))).unwrap();
        let a = s.user(a.id).unwrap().unwrap();
        assert!(a.disabled);
        assert_eq!(a.quota_bytes, Some(10));
        assert_eq!(s.active_admins().unwrap(), 0);
    }

    #[test]
    fn sessions_end_when_disabled_or_expired() {
        let (_dir, s) = store();
        let u = s.create_user("bob", "h", Role::User).unwrap();
        s.create_session(b"t1", u.id, "c1", 3600).unwrap();
        s.create_session(b"t2", u.id, "c2", -1).unwrap();
        assert_eq!(s.session(b"t1").unwrap().unwrap().1, "c1");
        assert!(s.session(b"t2").unwrap().is_none(), "expired");
        s.create_token(u.id, "laptop", b"k").unwrap();
        assert!(s.token_user(b"k").unwrap().is_some());
        s.update_user(u.id, None, Some(true), None).unwrap();
        assert!(s.session(b"t1").unwrap().is_none());
        assert!(s.token_user(b"k").unwrap().is_none());
        s.update_user(u.id, None, Some(false), None).unwrap();
        s.delete_sessions_of(u.id, None).unwrap();
        assert!(s.session(b"t1").unwrap().is_none());
        assert!(s.tokens(u.id).unwrap()[0].last_used_at.is_some());
    }

    #[test]
    fn databases_for_follows_visibility() {
        let (_dir, s) = store();
        let a = s.create_user("a", "h", Role::User).unwrap();
        let b = s.create_user("b", "h", Role::User).unwrap();
        s.create_database("p", "Private", a.id, Visibility::Private).unwrap();
        s.create_database("s", "Shared", a.id, Visibility::Shared).unwrap();
        s.create_database("w", "Wide", a.id, Visibility::Server).unwrap();
        s.set_share("p", b.id, ShareAccess::Write).unwrap();
        s.set_share("s", b.id, ShareAccess::Read).unwrap();
        let ids = |u: i64| s.databases_for(u).unwrap().into_iter().map(|(d, _)| d.id).collect::<Vec<_>>();
        assert_eq!(ids(a.id), ["p", "s", "w"]);
        assert_eq!(ids(b.id), ["s", "w"], "a share in a private database gives nothing");
        assert_eq!(s.shares("s").unwrap(), [("b".to_owned(), ShareAccess::Read)]);
        assert!(s.delete_database("s").unwrap());
        assert!(s.share("s", b.id).unwrap().is_none());
    }
}
