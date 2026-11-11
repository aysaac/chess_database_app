//! A server standing in for Lichess's OAuth and study API (NET-11, IO-10,
//! NET-12), for `tests/studies.rs` and, through `examples/lichess_standin.rs`,
//! for smoke runs of the app. It keeps its users, tokens and studies in
//! memory and answers like Lichess does where the app depends on it:
//!
//! - `/oauth` sends the browser straight back to `redirect_uri` with a code
//!   (or `error=access_denied` when told to), as if the user let the app
//!   in; `POST /api/token` checks the code, `redirect_uri`, `client_id` and
//!   the PKCE verifier against the challenge; `DELETE /api/token` revokes.
//! - Private and unlisted studies need a member's token: 401 without one,
//!   403 with someone else's. Lists leave them out without the owner's.
//! - A study's export has `Last-Modified`, and `If-Modified-Since` with it
//!   gets a 304 until the study changes. A chapter's export is always new.
//! - `POST /api/study` makes a study with an empty first chapter, up to
//!   `daily_limit` a day (then 429); `import-pgn` makes a chapter per game,
//!   64 at most in a study, and `initial=true` replaces the empty first one.
//!   Bodies over 512 KiB get a 413.
//!
//! Every request is logged (method, path and query, and whether it had a
//! token) in `requests`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use chess_net::oauth::Pkce;
use chess_net::study::split_games;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Study {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// public, unlisted or private.
    pub visibility: String,
    /// (id, PGN as exported), in order.
    pub chapters: Vec<(String, String)>,
    /// The empty chapter a new study starts with is still there.
    pub empty_first: bool,
    /// Bumped by every change: the export's `Last-Modified`.
    pub version: i64,
    pub settings: HashMap<String, String>,
}

struct Grant {
    challenge: String,
    redirect: String,
    client_id: String,
    user: String,
    scopes: String,
}

#[derive(Default)]
pub struct State {
    /// token → (username, scopes).
    pub tokens: HashMap<String, (String, String)>,
    pub studies: Vec<Study>,
    /// Studies made per user today.
    pub made_today: HashMap<String, usize>,
    pub daily_limit: usize,
    /// Who the browser logs in as.
    pub approve_as: String,
    /// The browser refuses the app.
    pub deny: bool,
    /// "METHOD /path?query [token]", in order.
    pub requests: Vec<String>,
    codes: HashMap<String, Grant>,
    next: u64,
    clock: i64,
}

impl State {
    fn next_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{:06}", self.next)
    }

    fn tick(&mut self) -> i64 {
        self.clock += 1;
        self.clock
    }

    pub fn study(&self, id: &str) -> Option<&Study> {
        self.studies.iter().find(|s| s.id == id)
    }
}

pub struct Lichess {
    pub url: String,
    pub state: Arc<Mutex<State>>,
}

impl Lichess {
    /// A server on a free port (`port` 0) or that one.
    pub fn start(port: u16) -> Lichess {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let state = Arc::new(Mutex::new(State {
            daily_limit: 30,
            approve_as: "Owner".into(),
            clock: 1_793_000_000,
            ..State::default()
        }));
        let (s, base) = (state.clone(), url.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (s, base) = (s.clone(), base.clone());
                std::thread::spawn(move || serve(stream, &s, &base));
            }
        });
        Lichess { url, state }
    }

    /// A user with a token of these scopes (comma-separated).
    pub fn add_token(&self, user: &str, token: &str, scopes: &str) {
        self.state.lock().unwrap().tokens.insert(token.into(), (user.into(), scopes.into()));
    }

    /// A study of `owner` with these chapters (PGN, tags and all).
    pub fn add_study(&self, id: &str, name: &str, owner: &str, visibility: &str, chapters: &[&str]) {
        let mut st = self.state.lock().unwrap();
        let version = st.tick();
        let mut study = Study {
            id: id.into(),
            name: name.into(),
            owner: owner.into(),
            visibility: visibility.into(),
            chapters: Vec::new(),
            empty_first: false,
            version,
            settings: HashMap::new(),
        };
        for pgn in chapters {
            let ch = st.next_id("ch");
            study.chapters.push((ch.clone(), tagged(pgn, &self.url, &study, &ch)));
        }
        st.studies.push(study);
    }

    pub fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn study(&self, id: &str) -> Option<Study> {
        self.state.lock().unwrap().study(id).cloned()
    }

    /// The studies of `owner`, oldest first.
    pub fn studies_of(&self, owner: &str) -> Vec<Study> {
        self.state.lock().unwrap().studies.iter().filter(|s| s.owner == owner).cloned().collect()
    }
}

/// A chapter's PGN as the export writes it: the game's tags without the
/// study ones, then the study's.
fn tagged(pgn: &str, base: &str, study: &Study, chapter: &str) -> String {
    let (mut tags, mut moves) = (Vec::new(), String::new());
    let mut in_tags = true;
    for line in pgn.trim().lines() {
        if in_tags && line.trim_start().starts_with('[') {
            tags.push(line.trim().to_owned());
        } else {
            in_tags = false;
            moves.push_str(line);
            moves.push('\n');
        }
    }
    let value = |name: &str| {
        tags.iter().find_map(|t| {
            let rest = t.strip_prefix('[')?.strip_prefix(name)?.trim_start().strip_prefix('"')?;
            Some(rest.trim_end_matches(']').trim_end_matches('"').to_owned())
        })
    };
    let name = match (value("White"), value("Black")) {
        (Some(w), Some(b)) => format!("{w} - {b}"),
        _ => value("Event").unwrap_or_else(|| "Chapter".into()),
    };
    let orientation = value("Orientation").unwrap_or_else(|| "white".into());
    tags.retain(|t| {
        !["[StudyName ", "[ChapterName ", "[ChapterURL ", "[Orientation "].iter().any(|p| t.starts_with(p))
    });
    tags.push(format!("[StudyName \"{}\"]", study.name));
    tags.push(format!("[ChapterName \"{name}\"]"));
    tags.push(format!("[ChapterURL \"https://lichess.org/study/{}/{chapter}\"]", study.id));
    tags.push(format!("[Orientation \"{orientation}\"]"));
    let _ = base;
    format!("{}\n\n{}", tags.join("\n"), moves.trim())
}

/// An HTTP date of the study's version (seconds since the epoch).
fn http_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (y, m, d) = chess_net::time::civil_from_days(days);
    let rest = secs.rem_euclid(86_400);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let month =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][m as usize - 1];
    format!("{weekday}, {d:02} {month} {y} {:02}:{:02}:{:02} GMT", rest / 3600, rest / 60 % 60, rest % 60)
}

fn decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
                out.push(hex.and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(b'?'));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pairs(text: &str) -> HashMap<String, String> {
    text.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect()
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn token(&self) -> Option<&str> {
        self.headers.get("authorization")?.strip_prefix("Bearer ")
    }
    fn form(&self) -> HashMap<String, String> {
        pairs(&String::from_utf8_lossy(&self.body))
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn reply(status: u16, body: impl Into<String>) -> Reply {
    Reply { status, headers: Vec::new(), body: body.into() }
}

fn json_reply(status: u16, v: serde_json::Value) -> Reply {
    reply(status, v.to_string())
}

fn serve(stream: TcpStream, state: &Mutex<State>, base: &str) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split(' ');
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let len: usize = headers.get("content-length").and_then(|n| n.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let req = Request { method, path: path.to_owned(), query: pairs(query), headers, body };
    let r = if len > 512 << 10 { reply(413, "Request Entity Too Large") } else { route(&req, state, base) };
    {
        let mut st = state.lock().unwrap();
        let auth = if req.token().is_some() { " [token]" } else { "" };
        st.requests.push(format!("{} {target}{auth}", req.method));
    }
    let mut out = stream;
    let mut head =
        format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(r.body.as_bytes());
}

fn route(req: &Request, state: &Mutex<State>, base: &str) -> Reply {
    let mut st = state.lock().unwrap();
    let who = req.token().and_then(|t| st.tokens.get(t).cloned());
    let can = |scope: &str| who.as_ref().is_some_and(|(_, s)| s.split(',').any(|x| x == scope));
    let segs: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), segs.as_slice()) {
        ("GET", ["oauth"]) => {
            let q = &req.query;
            let ok = q.get("response_type").map(String::as_str) == Some("code")
                && q.get("code_challenge_method").map(String::as_str) == Some("S256")
                && q.contains_key("client_id")
                && q.contains_key("code_challenge");
            let Some(redirect) = q.get("redirect_uri").filter(|_| ok) else {
                return reply(400, "bad request");
            };
            let state_param = q.get("state").cloned().unwrap_or_default();
            let location = if st.deny {
                format!("{redirect}?error=access_denied&error_description=user+cancelled&state={state_param}")
            } else {
                let code = st.next_id("liu_");
                let grant = Grant {
                    challenge: q["code_challenge"].clone(),
                    redirect: redirect.clone(),
                    client_id: q["client_id"].clone(),
                    user: st.approve_as.clone(),
                    scopes: q.get("scope").cloned().unwrap_or_default().replace(' ', ","),
                };
                st.codes.insert(code.clone(), grant);
                format!("{redirect}?code={code}&state={state_param}")
            };
            Reply { status: 302, headers: vec![("Location".into(), location)], body: String::new() }
        }
        ("POST", ["api", "token"]) => {
            let f = req.form();
            let grant = f.get("code").and_then(|c| st.codes.remove(c));
            let Some(g) = grant else { return json_reply(400, json!({ "error": "invalid_grant" })) };
            let verifier = f.get("code_verifier").cloned().unwrap_or_default();
            if Pkce::from_verifier(verifier).challenge != g.challenge {
                return json_reply(
                    400,
                    json!({ "error": "invalid_grant", "error_description": "hash of code_verifier does not match code_challenge" }),
                );
            }
            if f.get("redirect_uri") != Some(&g.redirect) || f.get("client_id") != Some(&g.client_id) {
                return json_reply(
                    400,
                    json!({ "error": "invalid_grant", "error_description": "redirect_uri or client_id mismatch" }),
                );
            }
            if f.get("grant_type").map(String::as_str) != Some("authorization_code") {
                return json_reply(400, json!({ "error": "unsupported_grant_type" }));
            }
            let token = st.next_id("lio_standin");
            st.tokens.insert(token.clone(), (g.user, g.scopes));
            json_reply(
                200,
                json!({ "token_type": "Bearer", "access_token": token, "expires_in": 31_536_000 }),
            )
        }
        ("DELETE", ["api", "token"]) => match req.token() {
            Some(t) if st.tokens.remove(t).is_some() => reply(204, ""),
            _ => reply(401, "{\"error\":\"No such token\"}"),
        },
        ("POST", ["api", "token", "test"]) => {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let mut out = serde_json::Map::new();
            for t in body.split(',') {
                let v = match st.tokens.get(t) {
                    Some((u, s)) => json!({ "userId": u.to_lowercase(), "scopes": s, "expires": null }),
                    None => serde_json::Value::Null,
                };
                out.insert(t.to_owned(), v);
            }
            json_reply(200, serde_json::Value::Object(out))
        }
        ("GET", ["api", "account"]) => match &who {
            Some((u, _)) => json_reply(200, json!({ "id": u.to_lowercase(), "username": u })),
            None => json_reply(401, json!({ "error": "No such token" })),
        },
        ("GET", ["api", "study", "by", user]) => {
            if req.token().is_some() && who.is_none() {
                return json_reply(401, json!({ "error": "No such token" }));
            }
            let own = who.as_ref().is_some_and(|(u, _)| u.eq_ignore_ascii_case(user)) && can("study:read");
            let mut lines = String::new();
            for s in st.studies.iter().rev().filter(|s| s.owner.eq_ignore_ascii_case(user)) {
                if own || s.visibility == "public" {
                    let line = json!({ "id": s.id, "name": s.name, "createdAt": s.version * 1000, "updatedAt": s.version * 1000 });
                    lines.push_str(&format!("{line}\n"));
                }
            }
            reply(200, lines)
        }
        ("GET", ["api", "study", rest @ ..]) => {
            let (id, chapter) = match rest {
                [s] => (s.strip_suffix(".pgn").unwrap_or(s), None),
                [s, c] => (*s, Some(c.strip_suffix(".pgn").unwrap_or(c))),
                _ => return reply(404, "<html>"),
            };
            let Some(study) = st.study(id).cloned() else { return reply(404, "<!DOCTYPE html><html>") };
            if req.token().is_some() && who.is_none() {
                return reply(401, "{\"error\":\"No such token\"}");
            }
            if study.visibility != "public" {
                match &who {
                    None => return reply(401, "This study is now private"),
                    Some((u, _)) if *u != study.owner || !can("study:read") => {
                        return reply(403, "This study is now private");
                    }
                    _ => {}
                }
            }
            let strip = |pgn: &str| pgn.to_owned();
            match chapter {
                Some(c) => {
                    let Some((_, pgn)) = study.chapters.iter().find(|(id, _)| id == c) else {
                        return reply(404, "Study or chapter not found");
                    };
                    let now = st.tick();
                    let mut r = reply(200, format!("{}\n\n\n", strip(pgn)));
                    r.headers.push(("Last-Modified".into(), http_date(now)));
                    r
                }
                None => {
                    let modified = http_date(study.version);
                    if req.headers.get("if-modified-since") == Some(&modified) {
                        return Reply { status: 304, headers: Vec::new(), body: String::new() };
                    }
                    let mut body = String::new();
                    if study.empty_first {
                        body.push_str(&format!(
                            "[Event \"{0}: Chapter 1\"]\n[Result \"*\"]\n[StudyName \"{0}\"]\n[ChapterName \"Chapter 1\"]\n\
                             [ChapterURL \"https://lichess.org/study/{1}/emptyone\"]\n[Orientation \"white\"]\n\n *\n\n\n",
                            study.name, study.id
                        ));
                    }
                    for (_, pgn) in &study.chapters {
                        body.push_str(&strip(pgn));
                        body.push_str("\n\n\n");
                    }
                    let mut r = reply(200, body);
                    r.headers.push(("Last-Modified".into(), modified));
                    r
                }
            }
        }
        ("POST", ["api", "study"]) => {
            let Some((user, _)) = who.clone() else {
                return json_reply(401, json!({ "error": "No such token" }));
            };
            if !can("study:write") {
                return json_reply(403, json!({ "error": "Missing scope" }));
            }
            let f = req.form();
            for k in ["name", "visibility", "computer", "explorer", "cloneable", "shareable", "chat"] {
                if !f.contains_key(k) {
                    return json_reply(400, json!({ "error": { k: ["This field is required"] } }));
                }
            }
            let name_len = f["name"].chars().count();
            if !(2..=100).contains(&name_len) {
                return json_reply(
                    400,
                    json!({ "error": { "name": ["Must be between 2 and 100 characters"] } }),
                );
            }
            let made = st.made_today.get(&user).copied().unwrap_or(0);
            if made >= st.daily_limit {
                return json_reply(429, json!({ "error": "Too many requests. Try again later." }));
            }
            st.made_today.insert(user.clone(), made + 1);
            let id = st.next_id("st");
            let version = st.tick();
            let settings =
                f.iter().filter(|(k, _)| *k != "name").map(|(k, v)| (k.clone(), v.clone())).collect();
            let study = Study {
                id: id.clone(),
                name: f["name"].clone(),
                owner: user,
                visibility: f["visibility"].clone(),
                chapters: Vec::new(),
                empty_first: true,
                version,
                settings,
            };
            st.studies.push(study);
            json_reply(200, json!({ "id": id }))
        }
        ("POST", ["api", "study", id, "import-pgn"]) => {
            let Some((user, _)) = who.clone() else {
                return json_reply(401, json!({ "error": "No such token" }));
            };
            if !can("study:write") {
                return json_reply(403, json!({ "error": "Missing scope" }));
            }
            let f = req.form();
            let Some(pgn) = f.get("pgn").filter(|p| !p.trim().is_empty()) else {
                return json_reply(400, json!({ "error": { "pgn": ["This field is required"] } }));
            };
            let initial = f.get("initial").map(String::as_str) == Some("true");
            let version = st.tick();
            let mut ids = Vec::new();
            for g in split_games(pgn).into_iter().take(64) {
                ids.push((st.next_id("ch"), g.to_owned()));
            }
            let Some(study) = st.studies.iter_mut().find(|s| s.id == *id) else {
                return reply(404, "<html>");
            };
            if study.owner != user {
                return json_reply(403, json!({ "error": "No permission to add chapter" }));
            }
            let mut made = Vec::new();
            let mut error = None;
            for (n, (ch, g)) in ids.into_iter().enumerate() {
                if n == 0 && initial && study.empty_first && study.chapters.is_empty() {
                    study.empty_first = false;
                }
                let count = study.chapters.len() + usize::from(study.empty_first);
                if count >= 64 {
                    error = Some("Too many chapters".to_owned());
                    break;
                }
                let text = tagged(&g, base, study, &ch);
                made.push(json!({ "id": ch, "name": "chapter", "players": [], "status": "*" }));
                study.chapters.push((ch, text));
            }
            study.version = version;
            json_reply(200, json!({ "chapters": made, "error": error }))
        }
        // NET-01, enough to see who sends a token: a profile and one game.
        ("GET", ["api", "user", name]) => json_reply(
            200,
            json!({ "id": name.to_lowercase(), "username": name, "createdAt": 1_600_000_000_000i64 }),
        ),
        ("GET", ["api", "games", "user", name]) => {
            if req.token().is_some() && who.is_none() {
                return json_reply(401, json!({ "error": "No such token" }));
            }
            let pgn = format!(
                "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/GaMe0001\"]\n[White \"{name}\"]\n\
                 [Black \"Someone\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Qh5 1-0\n"
            );
            json_reply(200, json!({ "id": "GaMe0001", "createdAt": 1_700_000_000_000i64, "pgn": pgn }))
        }
        _ => reply(404, "<!DOCTYPE html><html>"),
    }
}
