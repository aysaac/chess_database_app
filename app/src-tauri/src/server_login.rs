//! The sign-in to the self-hosted server (NET-03): an API token the server
//! made for this computer, kept in the OS's credential store as the
//! Lichess login's is (`lichess.rs`), or for this session only where
//! there's none. The settings keep the server's address, who is signed in
//! and the token's id there (to revoke it), never the token. One server
//! account at a time.
//!
//! Debug builds take `CHESSDB_SERVER_URL` as the address the sign-in
//! dialog offers first (a smoke run's local server).

use std::sync::{Mutex, MutexGuard, OnceLock};

use chess_net::server::ServerSignIn;
use chess_net::{Client, Token};
use keyring_core::Entry;

use crate::dto::sync::ServerAccountDto;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::lichess::{Kept, store_entry};
use crate::settings::ServerUser;
use crate::state::AppState;

/// The credential store's entry for it (the service is the app's).
const ACCOUNT: &str = "server";

/// A sign-in, with its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLogin {
    pub url: String,
    pub username: String,
    pub token: Token,
    pub token_id: i64,
}

#[derive(Default)]
pub struct Server {
    client: OnceLock<Client>,
    /// The sign-in, once known this session, and where its token is kept.
    login: Mutex<Option<(ServerLogin, Kept)>>,
    /// The credential store's entry, opened the first time it's needed.
    entry: OnceLock<Option<Entry>>,
    /// Held while the sign-in is read, kept or forgotten.
    gate: Mutex<()>,
    /// The address of the last sign-in this session, offered again after
    /// signing out.
    last_url: Mutex<Option<String>>,
}

impl Server {
    /// The same with this credential store (tests).
    #[cfg(test)]
    pub fn with_store(store: std::sync::Arc<keyring_core::CredentialStore>) -> Server {
        let server = Server::default();
        let _ = server.entry.set(store.build("dev.chessdb.desktop", ACCOUNT, None).ok());
        server
    }

    /// The client for the server (kept, so connections are reused).
    pub fn client(&self) -> &Client {
        self.client.get_or_init(Client::new)
    }

    fn entry(&self) -> Option<&Entry> {
        self.entry.get_or_init(|| store_entry(ACCOUNT)).as_ref()
    }

    fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().expect("server gate")
    }
}

/// The address the sign-in offers first: the last one signed in to this
/// session, or (debug builds) `CHESSDB_SERVER_URL`.
fn suggested_url(state: &AppState) -> Option<String> {
    let last = state.server.last_url.lock().expect("server lock").clone();
    last.or_else(|| std::env::var("CHESSDB_SERVER_URL").ok().filter(|_| cfg!(debug_assertions)))
}

/// Who is signed in, without reading the credential store.
pub fn account(state: &AppState) -> ServerAccountDto {
    let known = state
        .server
        .login
        .lock()
        .expect("server lock")
        .as_ref()
        .map(|(l, kept)| (l.url.clone(), l.username.clone(), *kept == Kept::Store));
    let known = known.or_else(|| {
        let s = state.settings.lock().expect("settings lock");
        s.server.as_ref().map(|u| (u.url.clone(), u.username.clone(), true))
    });
    match known {
        Some((url, username, kept)) => {
            ServerAccountDto { url: Some(url), username: Some(username), kept, suggested_url: None }
        }
        None => {
            ServerAccountDto { url: None, username: None, kept: false, suggested_url: suggested_url(state) }
        }
    }
}

/// What a credential store that can't be read says.
fn unreadable(why: &str) -> AppError {
    AppError::new(
        ErrorCode::Login,
        format!(
            "the app couldn't read its sign-in to the server from the system's keyring ({why}). Unlock the keyring and try again, or sign out"
        ),
    )
}

/// The sign-in, reading the token from the credential store the first
/// time. `None` when nobody is signed in, or the store no longer has the
/// token (which signs out). A store that can't be read is an error and
/// leaves the sign-in as it is.
pub fn login(state: &AppState) -> AppResult<Option<ServerLogin>> {
    let _gate = state.server.gate();
    login_held(state)
}

fn login_held(state: &AppState) -> AppResult<Option<ServerLogin>> {
    if let Some((login, _)) = state.server.login.lock().expect("server lock").as_ref() {
        return Ok(Some(login.clone()));
    }
    let Some(user) = state.settings.lock().expect("settings lock").server.clone() else { return Ok(None) };
    let Some(entry) = state.server.entry() else {
        return Err(unreadable("it isn't running"));
    };
    match entry.get_password() {
        Ok(token) => {
            let login = ServerLogin {
                url: user.url,
                username: user.username,
                token: Token::new(&token),
                token_id: user.token_id,
            };
            *state.server.login.lock().expect("server lock") = Some((login.clone(), Kept::Store));
            Ok(Some(login))
        }
        Err(keyring_core::Error::NoEntry) => {
            log::info!("the server token is gone from the credential store: signed out");
            state.update_settings(|s| s.server = None);
            Ok(None)
        }
        Err(e) => {
            log::warn!("couldn't read the server token: {e}");
            Err(unreadable(&e.to_string()))
        }
    }
}

/// Keeps a new sign-in to `url`: the token in the credential store if
/// there's one (else for this session), who it's for in the settings.
/// Returns where it went, and the sign-in it replaced, whose token the
/// caller revokes.
pub fn keep(state: &AppState, url: &str, signed: ServerSignIn) -> (Kept, Option<ServerLogin>) {
    let _gate = state.server.gate();
    let before = login_held(state).ok().flatten();
    let login = ServerLogin {
        url: url.to_owned(),
        username: signed.username,
        token: signed.token,
        token_id: signed.token_id,
    };
    let kept = match state.server.entry().map(|e| e.set_password(login.token.secret())) {
        Some(Ok(())) => Kept::Store,
        Some(Err(e)) => {
            log::warn!("couldn't keep the server token in the credential store ({e}): it lasts this session");
            delete_stored(state);
            Kept::Session
        }
        None => Kept::Session,
    };
    let user = (kept == Kept::Store).then(|| ServerUser {
        url: login.url.clone(),
        username: login.username.clone(),
        token_id: login.token_id,
    });
    state.update_settings(|s| s.server = user);
    *state.server.last_url.lock().expect("server lock") = Some(login.url.clone());
    let replaced = before.filter(|b| b.token != login.token);
    *state.server.login.lock().expect("server lock") = Some((login, kept));
    (kept, replaced)
}

fn delete_stored(state: &AppState) {
    if let Some(entry) = state.server.entry() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => log::warn!("couldn't delete the server token from the credential store: {e}"),
        }
    }
}

/// Signs out: forgets the sign-in (the token too) and returns it, for its
/// token to be revoked. A store that can't be read signs out all the same.
pub fn forget(state: &AppState) -> Option<ServerLogin> {
    let _gate = state.server.gate();
    let known = login_held(state).ok().flatten();
    *state.server.login.lock().expect("server lock") = None;
    delete_stored(state);
    state.update_settings(|s| s.server = None);
    known
}

/// Forgets the sign-in whose token the server turned down (revoked there,
/// the account disabled), if it's still the one kept.
pub fn forget_rejected(state: &AppState, token: &Token) {
    let current = state.server.login.lock().expect("server lock").as_ref().map(|(l, _)| l.token.clone());
    if current.as_ref() == Some(token) {
        log::info!("the server no longer takes the sign-in: forgetting it");
        forget(state);
    }
}

/// Revokes a token the app no longer keeps, on a thread of its own: best
/// effort, nobody waits for it.
pub fn revoke_in_background(client: &Client, login: ServerLogin) {
    let client = client.clone();
    std::thread::spawn(move || {
        if let Err(e) = chess_net::server::sign_out(&client, &login.url, &login.token, login.token_id) {
            log::warn!("couldn't revoke a server token the app no longer keeps: {e}");
        }
    });
}

/// What the token is called on the server: this app on this computer, so
/// the user can tell it apart on their Account page.
pub fn token_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().chars().filter(|c| !c.is_control()).take(60).collect::<String>())
        .filter(|h| !h.is_empty());
    match host {
        Some(h) => format!("Chess Database app on {h}"),
        None => "Chess Database app".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keyring_core::CredentialStore;

    use super::*;

    fn signed(name: &str, token: &str, id: i64) -> ServerSignIn {
        ServerSignIn { username: name.into(), token: Token::new(token), token_id: id }
    }

    fn with_store() -> AppState {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        let mut state = AppState::default();
        state.server = Server::with_store(store);
        state
    }

    #[test]
    fn a_sign_in_is_kept_read_back_replaced_and_forgotten() {
        let state = with_store();
        assert_eq!(login(&state).unwrap(), None);
        let (kept, replaced) = keep(&state, "https://chess.example.com", signed("alice", "cdb_1", 1));
        assert_eq!((kept, replaced), (Kept::Store, None));
        let user = state.settings.lock().unwrap().server.clone().unwrap();
        assert_eq!(
            (user.url.as_str(), user.username.as_str(), user.token_id),
            ("https://chess.example.com", "alice", 1)
        );
        // After a restart: read from the store.
        *state.server.login.lock().unwrap() = None;
        let back = login(&state).unwrap().unwrap();
        assert_eq!((back.token.secret(), back.username.as_str()), ("cdb_1", "alice"));
        let acc = account(&state);
        assert_eq!((acc.username.as_deref(), acc.kept), (Some("alice"), true));
        // Another sign-in replaces it: the old one is handed back to revoke.
        let (_, replaced) = keep(&state, "https://chess.example.com", signed("bob", "cdb_2", 2));
        assert_eq!(replaced.map(|r| r.token_id), Some(1));
        // A rejected token that isn't the kept one changes nothing.
        forget_rejected(&state, &Token::new("cdb_1"));
        assert!(login(&state).unwrap().is_some());
        forget_rejected(&state, &Token::new("cdb_2"));
        assert_eq!(login(&state).unwrap(), None);
        assert!(state.settings.lock().unwrap().server.is_none());
        let (_, replaced) = keep(&state, "https://x.org", signed("carol", "cdb_3", 3));
        assert!(replaced.is_none());
        assert_eq!(forget(&state).map(|l| l.token_id), Some(3));
        assert_eq!(account(&state).username, None);
    }

    #[test]
    fn token_names_say_the_computer() {
        let name = token_name();
        assert!(name.starts_with("Chess Database app"), "{name}");
        assert!(name.chars().count() <= 100 && !name.chars().any(char::is_control));
    }
}
