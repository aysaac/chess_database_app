//! The Lichess login (NET-11): its token kept in the OS's credential store
//! (the Secret Service on Linux, the Credential Manager on Windows, the
//! keychain on macOS), or for this session only where there's none. The
//! settings keep only who is logged in, so start-up never touches the
//! store: it's read the first time the token is needed (which may ask the
//! user to unlock it). A store that can't be read then (locked, its prompt
//! dismissed) keeps the login for a later try; only a store without the
//! token logs out. Test runs (`CHESSDB_CONFIG_DIR`) never use it.
//!
//! Keeping a login, forgetting it and stopping logins happen one at a time
//! (`Lichess::gate`), so a login a Cancel or a Log out stopped is never
//! kept, and one kept before a Log out is the one it forgets and revokes.
//! A token the app no longer keeps (a stopped login's, one a new login
//! replaces) is revoked, never left working on Lichess.
//!
//! Debug builds take two test hooks: `CHESSDB_LICHESS_URL` points the app
//! at a server standing in for Lichess, and `CHESSDB_BROWSER` replaces the
//! system browser: `approve` lets the login in at once (what
//! `chess_net::oauth::approve_in_background` does), `none` opens nothing.
//! A smoke run (`CHESSDB_SMOKE`) never opens a real browser.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use chess_net::study::StudyRef;
use chess_net::{Client, Login, Token};
use keyring_core::{CredentialStore, Entry};

use crate::dto::lichess::{LichessAccountDto, StudyExportReportDto};
use crate::error::{AppError, ErrorCode};
use crate::settings::LichessUser;
use crate::state::AppState;

/// The app's OAuth client id: Lichess takes any that's unique to the app.
pub const CLIENT_ID: &str = "dev.chessdb.desktop";
/// The credential store's entry: this service and user.
const SERVICE: &str = "dev.chessdb.desktop";
const ACCOUNT: &str = "lichess";

/// Where the token is kept (the server sign-in's too).
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
    /// Bumped to stop a login waiting for the browser (or checking a token).
    attempt: AtomicU64,
    /// Held while a login is read from the store, kept or forgotten, and
    /// while logins are stopped: see the module's comment.
    gate: Mutex<()>,
    /// The study looked up last (IO-10): its import's name, and whether it
    /// took the login.
    pub looked_up: Mutex<Option<LookedUp>>,
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

    /// Stops the logins under way: none of them is kept after this returns
    /// (one kept before stays).
    pub fn stop_logins(&self) {
        let _gate = self.gate();
        self.attempt.fetch_add(1, Ordering::SeqCst);
    }

    fn gate(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().expect("lichess gate")
    }
}

/// A study looked up (IO-10), for its import.
#[derive(Debug, Clone)]
pub struct LookedUp {
    pub link: StudyRef,
    pub name: String,
    /// It took the login to see it (private or unlisted).
    pub private: bool,
}

/// The credential store couldn't be read. The login stays, for a later try.
#[derive(Debug)]
pub enum Unreadable {
    /// Locked, its unlock prompt dismissed, or another failure: what it said.
    Failed(String),
    /// None is running this time (one was when the login was kept).
    NoStore,
}

impl Unreadable {
    /// Why, in a few words.
    pub fn reason(&self) -> &str {
        match self {
            Unreadable::Failed(e) => e,
            Unreadable::NoStore => "it isn't running",
        }
    }
}

impl fmt::Display for Unreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = "the app couldn't read its Lichess login from the system's keyring";
        match self {
            Unreadable::Failed(e) => write!(f, "{what} ({e}). Unlock the keyring and try again, or log out"),
            Unreadable::NoStore => {
                write!(f, "{what}: it isn't running. Start it and open the app again, or log out")
            }
        }
    }
}

impl From<Unreadable> for AppError {
    fn from(e: Unreadable) -> AppError {
        AppError::new(ErrorCode::Login, e.to_string())
    }
}

/// The OS's credential store, if it has one the app can use.
fn open_entry() -> Option<Entry> {
    store_entry(ACCOUNT)
}

/// The app's entry `account` in the OS's credential store, if it has one
/// the app can use (never in a test run): the Lichess login's, or the
/// server sign-in's (`server_login.rs`).
pub(crate) fn store_entry(account: &str) -> Option<Entry> {
    if std::env::var_os("CHESSDB_CONFIG_DIR").is_some() {
        log::info!("a test run: the {account} login lasts this session");
        return None;
    }
    let store = match platform_store() {
        Ok(store) => store,
        Err(e) => {
            log::info!("no credential store ({e}): the {account} login lasts this session");
            return None;
        }
    };
    store.build(SERVICE, account, None).inspect_err(|e| log::warn!("credential store: {e}")).ok()
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
/// `None` when nobody is logged in, or the store no longer has the token
/// (which logs out). A store that can't be read leaves the login as it is.
pub fn login(state: &AppState) -> Result<Option<Login>, Unreadable> {
    let _gate = state.lichess.gate();
    login_held(state)
}

/// [`login`], with the gate held.
fn login_held(state: &AppState) -> Result<Option<Login>, Unreadable> {
    if let Some((login, _)) = state.lichess.login.lock().expect("lichess lock").as_ref() {
        return Ok(Some(login.clone()));
    }
    let Some(user) = state.settings.lock().expect("settings lock").lichess.clone() else { return Ok(None) };
    // The settings name a user only while the token is in the store, so a
    // store missing this time (not running) is one that can't be read.
    let Some(entry) = state.lichess.entry() else {
        log::warn!("no credential store this time: the Lichess login can't be read");
        return Err(Unreadable::NoStore);
    };
    match entry.get_password() {
        Ok(token) => {
            let login = Login { token: Token::new(&token), username: user.username, expires: user.expires };
            *state.lichess.login.lock().expect("lichess lock") = Some((login.clone(), Kept::Store));
            Ok(Some(login))
        }
        Err(keyring_core::Error::NoEntry) => {
            log::info!("the Lichess token is gone from the credential store: logged out");
            state.update_settings(|s| s.lichess = None);
            Ok(None)
        }
        Err(e) => {
            log::warn!("couldn't read the Lichess token: {e}");
            Err(Unreadable::Failed(e.to_string()))
        }
    }
}

/// The login of `username` (any case), if they're the one logged in.
pub fn login_of(state: &AppState, username: &str) -> Result<Option<Login>, Unreadable> {
    let same = |name: &str| name.eq_ignore_ascii_case(username.trim());
    let known = match state.lichess.login.lock().expect("lichess lock").as_ref() {
        Some((login, _)) => same(&login.username),
        None => {
            state.settings.lock().expect("settings lock").lichess.as_ref().is_some_and(|u| same(&u.username))
        }
    };
    if known { login(state) } else { Ok(None) }
}

/// Keeps the login of `attempt` (see [`Lichess::start_login`]) unless a
/// Cancel, a Log out or another login stopped it: then it's handed back,
/// and nothing changes. Otherwise where its token went, and the token of
/// the login it replaced (if another), which the caller revokes.
pub fn keep(state: &AppState, attempt: u64, login: Login) -> Result<(Kept, Option<Token>), Login> {
    let _gate = state.lichess.gate();
    if state.lichess.login_stopped(attempt) {
        return Err(login);
    }
    Ok(remember(state, login))
}

/// Keeps a new login: the token in the credential store if there's one
/// (else for this session), who it's for in the settings. Returns where it
/// went, and the token of the login before if it was another (the one in
/// memory, or the store's), which no longer has a place: it's for the
/// caller to revoke. The gate is held.
fn remember(state: &AppState, login: Login) -> (Kept, Option<Token>) {
    let entry = state.lichess.entry();
    let before = state.lichess.login.lock().expect("lichess lock").as_ref().map(|(l, _)| l.token.clone());
    let before = before.or_else(|| match entry.map(Entry::get_password) {
        Some(Ok(t)) => Some(Token::new(&t)),
        Some(Err(keyring_core::Error::NoEntry)) | None => None,
        Some(Err(e)) => {
            log::warn!("couldn't read the Lichess token before this login: {e}");
            None
        }
    });
    let kept = match entry.map(|e| e.set_password(login.token.secret())) {
        Some(Ok(())) => Kept::Store,
        Some(Err(e)) => {
            log::warn!(
                "couldn't keep the Lichess token in the credential store ({e}): it lasts this session"
            );
            // Nor the token of a login before, which would come back.
            delete_stored(state);
            Kept::Session
        }
        None => Kept::Session,
    };
    let user = (kept == Kept::Store)
        .then(|| LichessUser { username: login.username.clone(), expires: login.expires });
    state.update_settings(|s| s.lichess = user);
    let replaced = before.filter(|t| *t != login.token);
    *state.lichess.login.lock().expect("lichess lock") = Some((login, kept));
    (kept, replaced)
}

/// Deletes the token from the credential store, if there's one.
fn delete_stored(state: &AppState) {
    if let Some(entry) = state.lichess.entry() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => log::warn!("couldn't delete the Lichess token from the credential store: {e}"),
        }
    }
}

/// Forgets the login (the token too, from the credential store), and
/// returns it to be revoked. The gate is held. Logged out even when the
/// store can't be read; the error says so, as the token can't be revoked.
fn forget_held(state: &AppState) -> Result<Option<Login>, Unreadable> {
    let known = login_held(state);
    *state.lichess.login.lock().expect("lichess lock") = None;
    delete_stored(state);
    state.update_settings(|s| s.lichess = None);
    known
}

/// Logs out: stops the logins under way and forgets the login, returning it
/// to be revoked. A store that can't be read logs out all the same, and the
/// error says the token couldn't be revoked.
pub fn logout(state: &AppState) -> Result<Option<Login>, Unreadable> {
    let _gate = state.lichess.gate();
    state.lichess.attempt.fetch_add(1, Ordering::SeqCst);
    forget_held(state)
}

/// Forgets the login whose token Lichess turned down (revoked on its site,
/// expired), if it's still the one logged in: not one made since.
pub fn forget_rejected(state: &AppState, token: &Token) {
    let _gate = state.lichess.gate();
    if matches!(login_held(state), Ok(Some(login)) if login.token == *token) {
        log::info!("Lichess no longer takes the login: forgetting it");
        let _ = forget_held(state);
    }
}

/// Revokes a token the app no longer keeps, on a thread of its own: it's
/// best effort, and nobody waits for it.
pub fn revoke_in_background(client: &Client, token: Token) {
    let client = client.clone();
    std::thread::spawn(move || {
        if let Err(e) = chess_net::oauth::revoke(&client, &token) {
            log::warn!("couldn't revoke a Lichess token the app no longer keeps: {e}");
        }
    });
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

    /// A state with a mock credential store.
    fn with_store() -> AppState {
        let store: Arc<CredentialStore> = keyring_core::mock::Store::new().unwrap();
        let mut state = AppState::default();
        state.lichess = Lichess::with_store(store);
        state
    }

    fn entry(state: &AppState) -> &Entry {
        state.lichess.entry.get().unwrap().as_ref().unwrap()
    }

    /// The next call on the mock store fails as a locked keyring does.
    fn lock_store(state: &AppState) {
        let cred: &keyring_core::mock::Cred = entry(state).as_any().downcast_ref().unwrap();
        cred.set_error(keyring_core::Error::NoStorageAccess("the keyring is locked".into()));
    }

    /// Keeps `l` as a login that wasn't stopped.
    fn keep_now(state: &AppState, l: Login) -> (Kept, Option<Token>) {
        let attempt = state.lichess.start_login();
        keep(state, attempt, l).unwrap()
    }

    /// After a restart: the settings say who, the store has the token. The
    /// mock store's entries are per entry object, so the same one stands for
    /// the store across the restart.
    fn restart(state: &AppState) {
        *state.lichess.login.lock().unwrap() = None;
    }

    #[test]
    fn a_login_is_kept_in_the_store_and_read_back_lazily() {
        let state = with_store();
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert_eq!(keep_now(&state, login("Bob", "lio_1")), (Kept::Store, None));
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), true));
        assert_eq!(
            state.settings.lock().unwrap().lichess,
            Some(LichessUser { username: "Bob".into(), expires: Some(5) })
        );
        assert_eq!(entry(&state).get_password().unwrap(), "lio_1");
        restart(&state);
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), true), "without reading the store");
        assert_eq!(super::login(&state).unwrap().unwrap().token.secret(), "lio_1");
        assert!(login_of(&state, " BOB ").unwrap().is_some());
        assert!(login_of(&state, "Alice").unwrap().is_none());

        assert_eq!(logout(&state).unwrap().unwrap().token.secret(), "lio_1");
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert!(state.settings.lock().unwrap().lichess.is_none());
        assert!(super::login(&state).unwrap().is_none());
        assert!(matches!(entry(&state).get_password(), Err(keyring_core::Error::NoEntry)));
    }

    #[test]
    fn without_a_store_a_login_lasts_the_session() {
        let state = AppState::default();
        let _ = state.lichess.entry.set(None);
        assert_eq!(keep_now(&state, login("Bob", "lio_2")).0, Kept::Session);
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), false));
        assert!(state.settings.lock().unwrap().lichess.is_none(), "nothing claims a login after a restart");
        assert_eq!(super::login(&state).unwrap().unwrap().token.secret(), "lio_2");
    }

    #[test]
    fn a_token_gone_from_the_store_logs_out() {
        let state = with_store();
        state.update_settings(|s| s.lichess = Some(LichessUser { username: "Bob".into(), expires: None }));
        assert!(super::login(&state).unwrap().is_none());
        assert!(state.settings.lock().unwrap().lichess.is_none());
    }

    /// A locked keyring (or its prompt dismissed) isn't a logout: the login
    /// stays for a later try, and the token isn't orphaned.
    #[test]
    fn a_store_that_cant_be_read_keeps_the_login() {
        let state = with_store();
        keep_now(&state, login("Bob", "lio_3"));
        restart(&state);
        lock_store(&state);
        let e = super::login(&state).unwrap_err();
        assert!(
            e.to_string().contains("couldn't read its Lichess login") && e.to_string().contains("locked")
        );
        assert_eq!(AppError::from(e).code, ErrorCode::Login);
        assert!(state.settings.lock().unwrap().lichess.is_some(), "still logged in");
        assert_eq!(account(&state), LichessAccountDto::new(Some("Bob"), true));
        assert_eq!(super::login(&state).unwrap().unwrap().token.secret(), "lio_3", "read once unlocked");

        // No store this time (not running): the same.
        let state = AppState::default();
        let _ = state.lichess.entry.set(None);
        state.update_settings(|s| s.lichess = Some(LichessUser { username: "Bob".into(), expires: None }));
        let e = super::login(&state).unwrap_err();
        assert!(e.to_string().contains("it isn't running"), "{e}");
        assert!(state.settings.lock().unwrap().lichess.is_some());
    }

    /// Log out while the store can't be read: logged out all the same, and
    /// the error says the token couldn't be revoked (the command then says
    /// where to revoke it).
    #[test]
    fn a_logout_with_a_store_that_cant_be_read_says_so() {
        let state = with_store();
        keep_now(&state, login("Bob", "lio_4"));
        restart(&state);
        lock_store(&state);
        assert!(logout(&state).is_err(), "nothing to revoke: it says so");
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert!(state.settings.lock().unwrap().lichess.is_none());
        // The store answered again for the delete: the token went.
        assert!(matches!(entry(&state).get_password(), Err(keyring_core::Error::NoEntry)));
    }

    /// A login a Cancel or a Log out stopped is never kept, even when its
    /// token came: it's handed back to be revoked.
    #[test]
    fn a_stopped_login_is_not_kept() {
        let state = with_store();
        let a = state.lichess.start_login();
        state.lichess.stop_logins();
        let back = keep(&state, a, login("Bob", "lio_5")).unwrap_err();
        assert_eq!(back.token.secret(), "lio_5");
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert!(state.settings.lock().unwrap().lichess.is_none());
        assert!(entry(&state).get_password().is_err(), "not in the store");

        // Kept, then a Log out: it's the one forgotten.
        let b = state.lichess.start_login();
        keep(&state, b, login("Bob", "lio_6")).unwrap();
        let c = state.lichess.start_login();
        assert_eq!(logout(&state).unwrap().unwrap().token.secret(), "lio_6");
        assert!(keep(&state, c, login("Bob", "lio_7")).is_err(), "the logout stopped the next one too");
        assert!(super::login(&state).unwrap().is_none());
    }

    /// A new login hands back the token of the login before (in memory, or
    /// the store's) to be revoked; one that can't go in the store (it lasts
    /// the session) takes the earlier token out of it.
    #[test]
    fn a_new_login_replaces_the_tokens_before() {
        let state = with_store();
        keep_now(&state, login("Bob", "lio_8"));
        assert_eq!(keep_now(&state, login("Ann", "lio_9")), (Kept::Store, Some(Token::new("lio_8"))));
        assert_eq!(keep_now(&state, login("Ann", "lio_9")), (Kept::Store, None), "the same token");
        restart(&state);
        assert_eq!(keep_now(&state, login("Ann", "lio_10")), (Kept::Store, Some(Token::new("lio_9"))));
        // The store can't be read: the token before is unknown.
        restart(&state);
        lock_store(&state);
        assert_eq!(keep_now(&state, login("Ann", "lio_11")), (Kept::Store, None));
        assert_eq!(entry(&state).get_password().unwrap(), "lio_11");

        // The store refusing the new token (the next call on it, as the
        // token before is in memory): the session keeps it, and the store
        // keeps nothing, so no older login comes back after a restart.
        lock_store(&state);
        assert_eq!(keep_now(&state, login("Cid", "lio_12")), (Kept::Session, Some(Token::new("lio_11"))));
        assert!(matches!(entry(&state).get_password(), Err(keyring_core::Error::NoEntry)));
        assert!(state.settings.lock().unwrap().lichess.is_none());
        assert_eq!(account(&state), LichessAccountDto::new(Some("Cid"), false));
    }

    /// A 401 forgets the login whose token it was, not one made since.
    #[test]
    fn a_rejected_token_forgets_only_its_login() {
        let state = with_store();
        keep_now(&state, login("Bob", "lio_12"));
        forget_rejected(&state, &Token::new("lio_old"));
        assert_eq!(account(&state).username.as_deref(), Some("Bob"), "another token: kept");
        forget_rejected(&state, &Token::new("lio_12"));
        assert_eq!(account(&state), LichessAccountDto::new(None, false));
        assert!(entry(&state).get_password().is_err(), "out of the store too");
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
