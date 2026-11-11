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
const SCHEMA_VERSION: i64 = 1;

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
const DB_COLUMNS: &str = "d.id, d.name, d.owner_id, u.username, d.visibility, d.quota_bytes, d.created_at";

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
    })
}

fn quota(q: Option<u64>) -> Option<i64> {
    q.map(|q| i64::try_from(q).unwrap_or(i64::MAX))
}

fn active_admins(conn: &Connection) -> rusqlite::Result<u64> {
    conn.query_row("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n as u64)
}

/// A token's `last_used_at` is written when it's older than this (seconds),
/// not at every request (each write is an fsync under the store's lock).
pub const TOKEN_USE_PRECISION: i64 = 60;

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
                    "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
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
        Ok(active_admins(&self.conn())?)
    }

    /// Changes a user. Refused (409) when it would leave no enabled admin
    /// where there was one: counted and changed in one transaction, so two
    /// admins demoting or disabling each other at once can't both succeed.
    pub fn update_user(
        &self,
        id: i64,
        role: Option<Role>,
        disabled: Option<bool>,
        quota_bytes: Option<Option<u64>>,
    ) -> ApiResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let admins = active_admins(&tx)?;
        if let Some(role) = role {
            tx.execute("UPDATE users SET role = ?1 WHERE id = ?2", params![role.as_str(), id])?;
        }
        if let Some(disabled) = disabled {
            tx.execute("UPDATE users SET disabled = ?1 WHERE id = ?2", params![disabled as i64, id])?;
        }
        if let Some(q) = quota_bytes {
            tx.execute("UPDATE users SET quota_bytes = ?1 WHERE id = ?2", params![quota(q), id])?;
        }
        if admins > 0 && active_admins(&tx)? == 0 {
            // Dropping the transaction rolls it back.
            return Err(ApiError::conflict("The server needs an enabled admin"));
        }
        tx.commit()?;
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

    /// The token's user, if they aren't disabled; notes when it was used (to
    /// the minute: [`TOKEN_USE_PRECISION`]).
    pub fn token_user(&self, token_hash: &[u8]) -> ApiResult<Option<User>> {
        let conn = self.conn();
        let found = conn
            .query_row(
                &format!(
                    "SELECT {}, t.last_used_at FROM api_tokens t JOIN users u ON u.id = t.user_id
                     WHERE t.token_hash = ?1 AND u.disabled = 0",
                    USER_COLUMNS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                ),
                [token_hash],
                |r| Ok((user_row(r)?, r.get::<_, Option<i64>>(6)?)),
            )
            .optional()?;
        let Some((user, last_used)) = found else { return Ok(None) };
        let now = now();
        if last_used.is_none_or(|t| now - t >= TOKEN_USE_PRECISION) {
            conn.execute(
                "UPDATE api_tokens SET last_used_at = ?1 WHERE token_hash = ?2",
                params![now, token_hash],
            )?;
        }
        Ok(Some(user))
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
                let access: Option<String> = r.get(7)?;
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
        // The only enabled admin stays, with nothing of the change made.
        assert_eq!(s.update_user(a.id, None, Some(true), Some(Some(10))).unwrap_err().status, 409);
        assert_eq!(s.update_user(a.id, Some(Role::User), None, None).unwrap_err().status, 409);
        assert_eq!(s.user(a.id).unwrap().unwrap(), a);
        let b = s.create_user("Bea", "h", Role::Admin).unwrap();
        s.update_user(a.id, None, Some(true), Some(Some(10))).unwrap();
        let a = s.user(a.id).unwrap().unwrap();
        assert!(a.disabled);
        assert_eq!(a.quota_bytes, Some(10));
        assert_eq!(s.active_admins().unwrap(), 1);
        assert_eq!(s.update_user(b.id, Some(Role::User), None, None).unwrap_err().status, 409);
    }

    #[test]
    fn token_use_is_noted_to_the_minute() {
        let (_dir, s) = store();
        let u = s.create_user("bob", "h", Role::User).unwrap();
        s.create_token(u.id, "laptop", b"k").unwrap();
        let last_used = || s.tokens(u.id).unwrap()[0].last_used_at.unwrap();
        let set = |t: i64| {
            s.conn().execute("UPDATE api_tokens SET last_used_at = ?1", [t]).unwrap();
        };
        s.token_user(b"k").unwrap().unwrap();
        assert!(now() - last_used() <= 1);
        let recent = now() - TOKEN_USE_PRECISION / 2;
        set(recent);
        s.token_user(b"k").unwrap().unwrap();
        assert_eq!(last_used(), recent, "not written again within the minute");
        set(now() - TOKEN_USE_PRECISION);
        s.token_user(b"k").unwrap().unwrap();
        assert!(now() - last_used() <= 1);
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
