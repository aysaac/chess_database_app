//! The audit log (NET-03): security-relevant events in the metadata store,
//! for the admin (`GET /api/admin/audit`), kept `CHESS_SERVER_AUDIT_DAYS`.
//!
//! Each entry is who did it (the user, or none), the action, what it was
//! done to (a user, a database, an invite, a token, a job, a backup), when,
//! and the client's address. **Never game content, search criteria,
//! positions or file names** (of uploads or imports): an entry says an
//! import started, not from what.

use crate::AppState;
use crate::store::{AuditRow, DbMeta, User, now};

/// What an action was done to.
pub enum Target<'a> {
    None,
    User(i64, &'a str),
    /// A username someone tried (a failed login), whether or not it exists.
    Name(&'a str),
    Database(&'a DbMeta),
    Invite(i64),
    Token(i64, &'a str),
    Job(u64),
    Backup(&'a str),
}

impl AppState {
    /// Notes an event (blocking: it writes the store). A failure to write is
    /// logged, never the request's failure.
    pub fn audit(
        &self,
        action: &str,
        actor: Option<&User>,
        target: Target<'_>,
        detail: Option<String>,
        client: Option<&str>,
    ) {
        let (kind, id, name) = match target {
            Target::None => (None, None, None),
            Target::User(id, name) => (Some("user"), Some(id.to_string()), Some(name.to_owned())),
            Target::Name(name) => (Some("user"), None, Some(name.chars().take(64).collect())),
            Target::Database(m) => (Some("database"), Some(m.id.clone()), Some(m.name.clone())),
            Target::Invite(id) => (Some("invite"), Some(id.to_string()), None),
            Target::Token(id, name) => (Some("token"), Some(id.to_string()), Some(name.to_owned())),
            Target::Job(id) => (Some("job"), Some(id.to_string()), None),
            Target::Backup(name) => (Some("backup"), None, Some(name.to_owned())),
        };
        let row = AuditRow {
            at: now(),
            actor_id: actor.map(|u| u.id),
            actor: actor.map(|u| u.username.clone()),
            action: action.to_owned(),
            target_kind: kind.map(str::to_owned),
            target_id: id,
            target: name,
            detail,
            client: client.map(str::to_owned),
        };
        if let Err(e) = self.store.audit(&row) {
            log::warn!("couldn't write the audit log: {}", e.message);
        }
    }
}

/// [`AppState::audit`] from async code: on the blocking pool.
pub async fn note(state: &std::sync::Arc<AppState>, f: impl FnOnce(&AppState) + Send + 'static) {
    let st = state.clone();
    let _ = tokio::task::spawn_blocking(move || f(&st)).await;
}

/// The client's address, put in each request's extensions by a middleware
/// (`lib.rs`), from the peer or a trusted proxy's `X-Forwarded-For`.
#[derive(Debug, Clone, Default)]
pub struct ClientAddr(pub String);
