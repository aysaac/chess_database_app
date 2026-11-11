//! Logging in to Lichess (NET-11): OAuth2's authorization code flow with
//! PKCE (RFC 7636) for a public client, which Lichess takes without
//! registering it, the browser coming back to a listener on 127.0.0.1 on a
//! port of its own; or a personal access token pasted in. Either way the
//! login is a token that can read and write the user's studies (and
//! downloads their own games faster). Logging out revokes it.
//!
//! The browser is opened by the caller (`open`), so tests and smoke runs
//! never start a real one: [`approve_in_background`] does what a browser
//! does when the user lets the app in.

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::http::{Client, Payload, Write, encode, error_message, query_pairs, read_json, status_error};
use crate::{NetError, Site, time};

/// What the app asks to do: read the user's studies (private ones too) and
/// write them.
pub const SCOPES: [&str; 2] = ["study:read", "study:write"];

/// A Lichess access token. Its `Debug` doesn't show it, so it never ends up
/// in a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(token: &str) -> Token {
        Token(token.trim().to_owned())
    }

    /// The token itself, to keep it (in the OS's credential store).
    pub fn secret(&self) -> &str {
        &self.0
    }

    pub(crate) fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(…)")
    }
}

/// A Lichess login: the token, and who it's for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub token: Token,
    /// The name as Lichess writes it.
    pub username: String,
    /// When the token stops working (ms since the epoch); `None`: never.
    pub expires: Option<i64>,
}

/// PKCE's pair: a random verifier, which stays here until the code is
/// exchanged, and its challenge, which goes with the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// A new pair: 64 characters of verifier (48 random bytes).
    pub fn new() -> Result<Pkce, NetError> {
        Ok(Pkce::from_verifier(random(48)?))
    }

    /// `BASE64URL(SHA256(verifier))`, without padding.
    pub fn from_verifier(verifier: String) -> Pkce {
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge }
    }
}

/// `n` random bytes as base64url.
fn random(n: usize) -> Result<String, NetError> {
    let mut bytes = vec![0; n];
    getrandom::fill(&mut bytes).map_err(|e| NetError::Login(format!("no random numbers ({e})")))?;
    Ok(base64url(&bytes))
}

/// Base64 with `-` and `_` (RFC 4648's URL alphabet), without padding.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Where the browser goes to let the app in: Lichess's authorization page.
pub fn authorize_url(
    client: &Client,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge),
        ("scope", &SCOPES.join(" ")),
        ("state", state),
    ];
    let query: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    format!("{}/oauth?{}", client.lichess, query.join("&"))
}

/// Logs in through the browser: `open` sends it to Lichess's authorization
/// page, and once the user lets the app in, Lichess sends it back to a
/// listener on 127.0.0.1 with a code, which is exchanged for a token. Waits
/// until then, until `stop` says to give up, or 10 minutes.
pub fn login(
    client: &Client,
    client_id: &str,
    open: &dyn Fn(&str) -> Result<(), String>,
    stop: &dyn Fn() -> bool,
) -> Result<Login, NetError> {
    let pkce = Pkce::new()?;
    let state = random(16)?;
    let cant_listen =
        |e: std::io::Error| NetError::Login(format!("the app can't listen for the browser ({e})"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(cant_listen)?;
    let redirect = format!("http://127.0.0.1:{}/", listener.local_addr().map_err(cant_listen)?.port());
    let url = authorize_url(client, client_id, &redirect, &pkce.challenge, &state);
    open(&url).map_err(|e| NetError::Login(format!("the browser didn't open ({e})")))?;
    let code = wait_for_code(&listener, &state, client.login_wait, stop)?;
    drop(listener);
    let (token, expires) = exchange(client, client_id, &code, &pkce.verifier, &redirect)?;
    let username = account(client, &token, stop)?;
    Ok(Login { token, username, expires })
}

/// What a browser does when the user lets the app in at once, for tests and
/// smoke runs against a server standing in for Lichess: asks for `url` and
/// follows its redirects (to the login's listener), on a thread of its own.
pub fn approve_in_background(url: &str) {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        if let Err(e) = agent.get(&url).call() {
            log::warn!("the stand-in browser failed: {e}");
        }
    });
}

/// Waits for the browser to come back with this login's code.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    wait: Duration,
    stop: &dyn Fn() -> bool,
) -> Result<String, NetError> {
    let failed =
        |e: std::io::Error| NetError::Login(format!("the app stopped listening for the browser ({e})"));
    listener.set_nonblocking(true).map_err(failed)?;
    let until = Instant::now() + wait;
    loop {
        if stop() {
            return Err(NetError::Cancelled);
        }
        if Instant::now() >= until {
            let minutes = wait.as_secs().div_ceil(60);
            return Err(NetError::Login(format!(
                "nothing came back from the browser in {minutes} minute{}. Try again",
                if minutes == 1 { "" } else { "s" }
            )));
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(answer) = answer(stream, state) {
                    return answer;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(failed(e)),
        }
    }
}

const PAGE_DONE: &str = "You're logged in to Lichess. You can close this tab and go back to Chess Database.";
const PAGE_REFUSED: &str =
    "Chess Database wasn't let in, so it isn't logged in to Lichess. You can close this tab.";
const PAGE_STALE: &str = "This answer is for another login to Lichess. Log in again from Chess Database.";

/// One request to the listener. The browser coming back for this login
/// (its `state`) with a code or an error ends the wait; anything else (a
/// favicon, a stale tab of an older login, another program) gets a page that
/// says so and is otherwise ignored.
fn answer(stream: TcpStream, state: &str) -> Option<Result<String, NetError>> {
    // Accepted sockets may take the listener's non-blocking mode.
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new((&stream).take(16 << 10));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    // The headers, which say nothing we need.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(n) if n > 0 && header != "\r\n" && header != "\n" => {}
            _ => break,
        }
    }
    let mut parts = line.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method != "GET" || path != "/" {
        respond(&stream, "404 Not Found", "Not found.");
        return None;
    }
    let pairs = query_pairs(query);
    let get = |k: &str| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if get("state") != Some(state) {
        respond(&stream, "400 Bad Request", PAGE_STALE);
        return None;
    }
    if let Some(code) = get("code").filter(|c| !c.is_empty()) {
        respond(&stream, "200 OK", PAGE_DONE);
        return Some(Ok(code.to_owned()));
    }
    respond(&stream, "200 OK", PAGE_REFUSED);
    let why = match (get("error"), get("error_description")) {
        (Some("access_denied"), _) => "the app wasn't let in on Lichess".to_owned(),
        (_, Some(d)) if !d.is_empty() => format!("Lichess said: {d}"),
        (Some(e), _) => format!("Lichess said: {e}"),
        (None, _) => "Lichess sent no code".to_owned(),
    };
    Some(Err(NetError::Login(why)))
}

fn respond(mut stream: &TcpStream, status: &str, text: &str) {
    let page = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Chess Database</title>\
         <body style=\"font: 16px/1.5 system-ui, sans-serif; margin: 3em\"><p>{text}</p></body></html>"
    );
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        page.len()
    );
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(page.as_bytes()));
    let _ = stream.flush();
}

/// The code and the verifier for a token.
fn exchange(
    client: &Client,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<(Token, Option<i64>), NetError> {
    #[derive(Deserialize)]
    struct Answer {
        access_token: String,
        expires_in: Option<i64>,
    }
    let url = format!("{}/api/token", client.lichess);
    let fields = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect),
        ("client_id", client_id),
    ];
    let write = Write { method: "POST", url: &url, token: None, body: Payload::Form(fields) };
    let mut resp = client.send(Site::Lichess, write)?;
    match resp.status().as_u16() {
        200 => {}
        400 | 401 => {
            let why = error_message(Site::Lichess, &mut resp).unwrap_or_else(|| "it refused the code".into());
            return Err(NetError::Login(format!("Lichess didn't give the app a token ({why})")));
        }
        429 => return Err(NetError::RateLimited(Site::Lichess)),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let answer: Answer = read_json(Site::Lichess, &mut resp, 1 << 16, &Arc::new(AtomicI64::new(0)))?;
    let expires = answer.expires_in.map(|s| time::now_ms() + s * 1000);
    Ok((Token::new(&answer.access_token), expires))
}

/// Who the token is for: their name as Lichess writes it.
pub fn account(client: &Client, token: &Token, stop: &dyn Fn() -> bool) -> Result<String, NetError> {
    #[derive(Deserialize)]
    struct Account {
        username: String,
    }
    let url = format!("{}/api/account", client.lichess);
    let auth = [("Authorization", token.bearer())];
    let mut resp = client.get_with(Site::Lichess, &url, &[], "application/json", &auth, stop)?;
    match resp.status().as_u16() {
        200 => {}
        401 | 403 => return Err(NetError::TokenRejected),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let account: Account = read_json(Site::Lichess, &mut resp, 1 << 20, &Arc::new(AtomicI64::new(0)))?;
    Ok(account.username)
}

/// A personal access token pasted in: whether Lichess knows it, that it can
/// read and write studies, and who it's for.
pub fn check_token(client: &Client, token: &Token, stop: &dyn Fn() -> bool) -> Result<Login, NetError> {
    #[derive(Deserialize)]
    struct Info {
        scopes: Option<String>,
        expires: Option<i64>,
    }
    let ok = !token.0.is_empty() && token.0.bytes().all(|b| b.is_ascii_graphic() && b != b',');
    if !ok {
        return Err(NetError::TokenRejected);
    }
    let url = format!("{}/api/token/test", client.lichess);
    let write = Write { method: "POST", url: &url, token: None, body: Payload::Text(token.0.clone()) };
    let mut resp = client.send(Site::Lichess, write)?;
    match resp.status().as_u16() {
        200 => {}
        429 => return Err(NetError::RateLimited(Site::Lichess)),
        _ => return Err(status_error(Site::Lichess, &resp)),
    }
    let mut answer: std::collections::HashMap<String, Option<Info>> =
        read_json(Site::Lichess, &mut resp, 1 << 16, &Arc::new(AtomicI64::new(0)))?;
    let Some(info) = answer.remove(&token.0).flatten() else { return Err(NetError::TokenRejected) };
    let has = info.scopes.unwrap_or_default();
    let has: Vec<&str> = has.split(',').map(str::trim).collect();
    let missing: Vec<&str> = SCOPES.iter().copied().filter(|s| !has.contains(s)).collect();
    if !missing.is_empty() {
        return Err(NetError::MissingScopes(missing.join(" and ")));
    }
    let username = account(client, token, stop)?;
    Ok(Login { token: token.clone(), username, expires: info.expires })
}

/// Revokes the token: it no longer works anywhere. One Lichess doesn't know
/// (revoked already, or expired) counts as revoked.
pub fn revoke(client: &Client, token: &Token) -> Result<(), NetError> {
    let url = format!("{}/api/token", client.lichess);
    let write = Write { method: "DELETE", url: &url, token: Some(token), body: Payload::Empty };
    let resp = client.send(Site::Lichess, write)?;
    match resp.status().as_u16() {
        200 | 204 | 401 => Ok(()),
        429 => Err(NetError::RateLimited(Site::Lichess)),
        _ => Err(status_error(Site::Lichess, &resp)),
    }
}

/// Where to make a personal access token that can do what the app needs,
/// on Lichess's site.
pub fn token_page(client: &Client) -> String {
    let scopes: String = SCOPES.iter().map(|s| format!("scopes[]={}&", encode(s))).collect();
    format!("{}/account/oauth/token/create?{scopes}description=Chess%20Database", client.lichess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_without_padding() {
        // RFC 4648's test vectors, in the URL alphabet and without `=`.
        for (bytes, b64) in
            [("", ""), ("f", "Zg"), ("fo", "Zm8"), ("foo", "Zm9v"), ("foob", "Zm9vYg"), ("fooba", "Zm9vYmE")]
        {
            assert_eq!(base64url(bytes.as_bytes()), b64);
        }
        assert_eq!(base64url(&[0xfb, 0xff, 0xbf]), "-_-_");
    }

    #[test]
    fn pkce_challenge_is_the_verifiers_sha256() {
        // RFC 7636, appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let a = Pkce::new().unwrap();
        let b = Pkce::new().unwrap();
        assert_eq!(a.verifier.len(), 64);
        assert!(a.verifier.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_ne!(a.verifier, b.verifier, "random");
        assert_eq!(Pkce::from_verifier(a.verifier.clone()), a);
    }

    #[test]
    fn a_token_never_shows_in_debug() {
        let t = Token::new("  lio_secret123 ");
        assert_eq!(t.secret(), "lio_secret123");
        assert_eq!(format!("{t:?}"), "Token(…)");
        let login = Login { token: t, username: "Bob".into(), expires: None };
        assert!(!format!("{login:?}").contains("secret"));
    }

    #[test]
    fn the_authorization_page_asks_for_studies() {
        let client = Client::with_servers("https://lichess.org", "https://api.chess.com");
        let url = authorize_url(&client, "dev.chessdb.desktop", "http://127.0.0.1:5000/", "ch", "st");
        assert_eq!(
            url,
            "https://lichess.org/oauth?response_type=code&client_id=dev.chessdb.desktop\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2F&code_challenge_method=S256&code_challenge=ch\
             &scope=study%3Aread%20study%3Awrite&state=st"
        );
        assert_eq!(
            token_page(&client),
            "https://lichess.org/account/oauth/token/create?scopes[]=study%3Aread&scopes[]=study%3Awrite&description=Chess%20Database"
        );
    }
}
