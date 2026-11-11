//! The Lichess login (NET-11): its token kept in the OS's credential store
//! (the Secret Service on Linux, the Credential Manager on Windows, the
//! keychain on macOS), or for this session only where there's none. The
//! settings keep only who is logged in, so start-up never touches the
//! store: it's read the first time the token is needed (which may ask the
//! user to unlock it). Test runs (`CHESSDB_CONFIG_DIR`) never use it.
//!
//! Debug builds take two test hooks: `CHESSDB_LICHESS_URL` points the app
//! at a server standing in for Lichess, and `CHESSDB_BROWSER` replaces the
//! system browser: `approve` lets the login in at once (what
//! `chess_net::oauth::approve_in_background` does), `none` opens nothing.
//! A smoke run (`CHESSDB_SMOKE`) never opens a real browser.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chess_net::study::StudyPgn;
use chess_net::{Client, Login, Token};
use keyring_core::{CredentialStore, Entry};

use crate::dto::lichess::{LichessAccountDto, StudyExportReportDto};
use crate::settings::LichessUser;
use crate::state::AppState;

/// The app's OAuth client id: Lichess takes any that's unique to the app.
pub const CLIENT_ID: &str = "dev.chessdb.desktop";
/// The credential store's entry: this service and user.
const SERVICE: &str = "dev.chessdb.desktop";
const ACCOUNT: &str = "lichess";

/// Where the token is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// The OS's credential store: still logged in after a restart.
    Store,
    /// Memory: until the app closes.
    Session,
}

#[derive(Default)]
pub struct Lichess {
    client: OnceLock<Client>,
    /// The login, once known this session, and where its token is kept.
    login: Mutex<Option<(Login, Kept)>>,
    /// The credential store's entry, opened the first time it's needed;
    /// `None` when there's no store.
    entry: OnceLock<Option<Entry>>,
    /// Bumped to stop a login waiting for the browser.
    attempt: AtomicU64,
    /// The study looked up last (IO-10), for its import to ask Lichess only
    /// whether it changed since, and whether it took the login.
    pub looked_up: Mutex<Option<(StudyPgn, bool)>>,
    /// What the last export to studies (NET-12) did.
    pub export_report: Mutex<Option<StudyExportReportDto>>,
}

impl Lichess {
    /// The same with this credential store (tests).
    #[cfg(test)]
    pub fn with_store(store: Arc<CredentialStore>) -> Lichess {
        let lichess = Lichess::default();
        let _ = lichess.entry.set(store.build(SERVICE, ACCOUNT, None).ok());
        lichess
    }

    /// The client for Lichess (kept, so connections are reused).
    pub fn client(&self) -> &Client {
        self.client.get_or_init(|| {
            let stand_in = std::env::var("CHESSDB_LICHESS_URL").ok().filter(|_| cfg!(debug_assertions));
            match stand_in {
                Some(url) => {
                    log::info!("Lichess stands in at {url}");
                    Client::with_servers(&url, "https://api.chess.com")
                }
                None => Client::new(),
            }
        })
    }

    fn entry(&self) -> Option<&Entry> {
        self.entry.get_or_init(open_entry).as_ref()
    }

    /// Starts a login attempt, stopping any before it; the attempt is
    /// stopped once [`stop_logins`](Lichess::stop_logins) or another start
    /// bumps the count.
    pub fn start_login(&self) -> u64 {
        self.attempt.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn login_stopped(&self, attempt: u64) -> bool {
        self.attempt.load(Ordering::SeqCst) != attempt
    }

    pub fn stop_logins(&self) {
        self.attempt.fetch_add(1, Ordering::SeqCst);
    }
}

/// The OS's credential store, if it has one the app can use.
fn open_entry() -> Option<Entry> {
    if std::env::var_os("CHESSDB_CONFIG_DIR").is_some() {
        log::info!("a test run: a Lichess login lasts this session");
        return None;
    }
    let store = match platform_store() {
        Ok(store) => store,
        Err(e) => {
            log::info!("no credential store ({e}): a Lichess login lasts this session");
            return None;
        }
    };
    store.build(SERVICE, ACCOUNT, None).inspect_err(|e| log::warn!("credential store: {e}")).ok()
}

#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore("no credential store on this system".into()))
}

/// Who is logged in, without reading the credential store.
pub fn account(state: &AppState) -> LichessAccountDto {
    if let Some((login, kept)) = state.lichess.login.lock().expect("lichess lock").as_ref() {
        return LichessAccountDto::new(Some(&login.username), *kept == Kept::Store);
    }
    let user = state.settings.lock().expect("settings lock").lichess.clone();
    LichessAccountDto::new(user.as_ref().map(|u| u.username.as_str()), user.is_some())
}

/// The login, reading the token from the credential store the first time.
/// `None` when nobody is logged in (or the store lost the token, which then
/// logs out).
pub fn login(state: &AppState) -> Option<Login> {
    if let Some((login, _)) = state.lichess.login.lock().expect("lichess lock").as_ref() {
        return Some(login.clone());
    }
    let user = state.settings.lock().expect("settings lock").lichess.clone()?;
    let token = state.lichess.entry().map(|e| e.get_password());
    match token {
        Some(Ok(token)) => {
            let login = Login { token: Token::new(&token), username: user.username, expires: user.expires };
            *state.lichess.login.lock().expect("lichess lock") = Some((login.clone(), Kept::Store));
            Some(login)
        }
        other => {
            match other {
                Some(Err(keyring_core::Error::NoEntry)) => {
                    log::info!("the Lichess token is gone from the credential store: logged out")
                }
                Some(Err(e)) => log::warn!("couldn't read the Lichess token: {e}"),
                _ => log::info!("no credential store this time: logged out of Lichess"),
            }
            state.update_settings(|s| s.lichess = None);
            None
        }
    }
}

/// The login of `username` (any case), if they're the one logged in.
pub fn login_of(state: &AppState, username: &str) -> Option<Login> {
    let same = |name: &str| name.eq_ignore_ascii_case(username.trim());
    let known = match state.lichess.login.lock().expect("lichess lock").as_ref() {
        Some((login, _)) => same(&login.username),
        None => {
            state.settings.lock().expect("settings lock").lichess.as_ref().is_some_and(|u| same(&u.username))
        }
    };
    if known { login(state) } else { None }
}

/// Keeps a new login: the token in the credential store if there's one
/// (else for this session), who it's for in the settings. Replaces the
/// login before, whose token isn't revoked (it's the same account's or the
/// user chose to log in again).
pub fn remember(state: &AppState, login: Login) -> Kept {
    let saved = state.lichess.entry().map(|e| e.set_password(login.token.secret()));
    let kept = match saved {
        Some(Ok(())) => Kept::Store,
        Some(Err(e)) => {
            log::warn!(
                "couldn't keep the Lichess token in the credential store ({e}): it lasts this session"
            );
            Kept::Session
        }
        None => Kept::Session,
    };
    let user = (kept == Kept::Store)
        .then(|| LichessUser { username: login.username.clone(), expires: login.expires });
    state.update_settings(|s| s.lichess = user);
    *state.lichess.login.lock().expect("lichess lock") = Some((login, kept));
    kept
}

/// Forgets the login (the token too, from the credential store), and
/// returns it to be revoked.
pub fn forget(state: &AppState) -> Option<Login> {
    let known = login(state);
    *state.lichess.login.lock().expect("lichess lock") = None;
    if let Some(entry) = state.lichess.entry() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => log::warn!("couldn't delete the Lichess token from the credential store: {e}"),
        }
    }
    state.update_settings(|s| s.lichess = None);
    known
}

/// Opens `url` in the system browser (or what a test run uses instead).
pub fn open_browser(url: &str) -> Result<(), String> {
    if cfg!(debug_assertions) {
        match std::env::var("CHESSDB_BROWSER").ok().as_deref() {
            Some("approve") => {
                chess_net::oauth::approve_in_background(url);
                return Ok(());
            }
            Some(other) => {
                log::info!("CHESSDB_BROWSER={other}: no browser opened");
                return Ok(());
            }
            None if std::env::var_os("CHESSDB_SMOKE").is_some() => {
                return Err(
                    "a smoke run never opens a browser (CHESSDB_BROWSER=approve lets a stand-in in)".into()
                );
            }
            None => {}
        }
    }
    open::that_detached(url).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(name: &str, token: &str) -> Login {
        Login { token: Token::new(token), username: name.into(), expires: Some(5) }
    }

    #[test]
    fn a_login_is_kept_in_the_store_and_read_back_lazily() {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        let mut state = AppState::default();
        state.lichess = Lichess::with_store(store);
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert_eq!(remember(&state, login("Bob", "lio_1")), Kept::Store);
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), true));
        assert_eq!(
            state.settings.lock().unwrap().lichess,
            Some(LichessUser { username: "Bob".into(), expires: Some(5) })
        );

        // After a restart: the settings say who, the store has the token.
        // The mock store's entries are per entry object, so the same one
        // stands for the store across the restart.
        let entry = state.lichess.entry.get().unwrap().as_ref().unwrap().get_password().unwrap();
        assert_eq!(entry, "lio_1");
        *state.lichess.login.lock().unwrap() = None;
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), true), "without reading the store");
        assert_eq!(super::login(&state).unwrap().token.secret(), "lio_1");
        assert!(login_of(&state, " BOB ").is_some());
        assert!(login_of(&state, "Alice").is_none());

        assert_eq!(forget(&state).unwrap().token.secret(), "lio_1");
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert!(state.settings.lock().unwrap().lichess.is_none());
        assert!(super::login(&state).is_none());
    }

    #[test]
    fn without_a_store_a_login_lasts_the_session() {
        let state = AppState::default();
        let _ = state.lichess.entry.set(None);
        assert_eq!(remember(&state, login("Bob", "lio_2")), Kept::Session);
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), false));
        assert!(state.settings.lock().unwrap().lichess.is_none(), "nothing claims a login after a restart");
        assert_eq!(super::login(&state).unwrap().token.secret(), "lio_2");
    }

    #[test]
    fn a_token_gone_from_the_store_logs_out() {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        let mut state = AppState::default();
        state.lichess = Lichess::with_store(store);
        state.update_settings(|s| s.lichess = Some(LichessUser { username: "Bob".into(), expires: None }));
        assert!(super::login(&state).is_none());
        assert!(state.settings.lock().unwrap().lichess.is_none());
    }

    #[test]
    fn logins_stop() {
        let l = Lichess::default();
        let a = l.start_login();
        assert!(!l.login_stopped(a));
        let b = l.start_login();
        assert!(l.login_stopped(a) && !l.login_stopped(b));
        l.stop_logins();
        assert!(l.login_stopped(b));
    }
}
