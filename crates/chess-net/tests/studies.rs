//! NET-11, IO-10 and NET-12 end to end against a local server standing in
//! for Lichess (`standin/mod.rs`): logging in through the "browser" with
//! PKCE, a pasted token, logging out; a study's chapters into a database,
//! again for the new ones only, a 304, private studies; games out to new
//! studies of 64 chapters, the daily limit, an existing study, and a round
//! trip.

mod standin;

use std::io::{Read, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chess_db::{Database, NoProgress, Progress};
use chess_net::oauth::{self, Token};
use chess_net::study::{
    ExportTarget, StudyRef, Visibility, download_study_into, export_to_studies, fetch_study,
    fetch_study_outline, parse_study_link, refresh_study_into, split_games, studies_of,
};
use chess_net::{Client, NetError, Period, Request, Site, download_into};
use standin::Lichess;

fn fixture() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/lichess-study.pgn");
    std::fs::read_to_string(path).unwrap()
}

/// The fixture's chapters, as PGN texts.
fn chapters() -> Vec<String> {
    split_games(&fixture()).into_iter().map(|g| g.trim().to_owned()).collect()
}

fn client(server: &Lichess) -> Client {
    Client::with_servers(&server.url, &server.url).with_retry_wait(Duration::from_millis(20))
}

fn new_db() -> (tempfile::TempDir, Mutex<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(&dir.path().join("Study")).unwrap();
    (dir, Mutex::new(db))
}

/// The games as (White, Black, Event), in the database's order.
fn games(db: &Mutex<Database>) -> Vec<(String, String, String)> {
    let db = db.lock().unwrap();
    (0..db.count()).map(|i| db.row(i).unwrap()).map(|r| (r.white, r.black, r.event)).collect()
}

fn study(id: &str) -> StudyRef {
    StudyRef { study: id.into(), chapter: None }
}

fn public_study(server: &Lichess) {
    let ch = chapters();
    let refs: Vec<&str> = ch.iter().map(String::as_str).collect();
    server.add_study("FxStudy1", "Fixture study", "Owner", "public", &refs);
}

// ---- IO-10: a study into a database -----------------------------------------

#[test]
fn a_public_study_then_only_its_new_chapters() {
    let server = Lichess::start(0);
    public_study(&server);
    let client = client(&server);
    let (_dir, db) = new_db();
    let link = parse_study_link("https://lichess.org/study/FxStudy1").unwrap();
    let s = download_study_into(&client, &db, &link, None, None, &NoProgress).unwrap();
    assert_eq!((s.chapters, s.import.imported, s.already, s.unchanged), (4, 4, 0, false));
    assert_eq!(s.name, "Fixture study");
    assert_eq!(s.import.warning_count, 0, "{:?}", s.import.warnings);
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1, "one request: {reqs:?}");
    for part in ["clocks=true", "comments=true", "variations=true", "orientation=true"] {
        assert!(reqs[0].contains(part), "{reqs:?}");
    }
    assert!(!reqs[0].contains("[token]"), "no token for a public study");
    let g = games(&db);
    assert_eq!(g[0], ("Student".into(), "Coach".into(), "Fixture study: Italian ideas".into()));
    assert_eq!(g[3].0, "Alpha");
    let game = db.lock().unwrap().read_game(0).unwrap();
    let url = game.headers.get_extra("ChapterURL").unwrap();
    assert!(url.starts_with("https://lichess.org/study/FxStudy1/"), "{url}");
    assert!(game.has_variations() && game.has_comments(), "comments, arrows and variations kept");

    // A chapter added on Lichess: only it comes the second time.
    let added = "[Event \"Fixture study: New\"]\n[White \"Gamma\"]\n[Black \"Delta\"]\n[Result \"*\"]\n\
                 [StudyName \"Fixture study\"]\n[ChapterURL \"https://lichess.org/study/FxStudy1/NewChap1\"]\n\n1. c4 *";
    {
        let mut st = server.state.lock().unwrap();
        let study = st.studies.iter_mut().find(|s| s.id == "FxStudy1").unwrap();
        study.chapters.push(("NewChap1".into(), added.into()));
        study.version += 1;
    }
    let s = download_study_into(&client, &db, &link, None, None, &NoProgress).unwrap();
    assert_eq!((s.chapters, s.import.imported, s.already), (5, 1, 4));
    assert_eq!(games(&db).len(), 5);
    assert_eq!(games(&db)[4].0, "Gamma");
}

#[test]
fn a_study_looked_up_is_asked_again_with_if_modified_since() {
    let server = Lichess::start(0);
    public_study(&server);
    let client = client(&server);
    let link = study("FxStudy1");
    // The lookup the dialog makes, to show the name and the chapters.
    let looked = fetch_study(&client, &link, None, None, &|| false).unwrap().unwrap();
    assert_eq!((looked.name.as_str(), looked.chapters.len()), ("Fixture study", 4));
    let modified = looked.last_modified.clone().expect("Last-Modified");
    assert_eq!(fetch_study(&client, &link, None, Some(&modified), &|| false).unwrap(), None, "a 304");

    let (_dir, db) = new_db();
    let s = download_study_into(&client, &db, &link, None, Some(&looked), &NoProgress).unwrap();
    assert!(s.unchanged, "from the lookup");
    assert_eq!(s.import.imported, 4);
    // Changed since: downloaded again.
    server.state.lock().unwrap().studies[0].version += 1;
    let s = download_study_into(&client, &db, &link, None, Some(&looked), &NoProgress).unwrap();
    assert!(!s.unchanged);
    assert_eq!((s.import.imported, s.already), (0, 4));
}

#[test]
fn a_study_a_database_keeps_up_with_is_downloaded_only_when_it_changed() {
    let server = Lichess::start(0);
    public_study(&server);
    let client = client(&server);
    let link = study("FxStudy1");
    let (_dir, db) = new_db();
    let first = refresh_study_into(&client, &db, &link, None, None, &NoProgress).unwrap();
    assert_eq!(first.import.as_ref().map(|i| i.import.imported), Some(4));
    let modified = first.last_modified.clone().expect("Last-Modified");
    // Unchanged: a 304, nothing imported, the same mark kept.
    let again = refresh_study_into(&client, &db, &link, None, Some(&modified), &NoProgress).unwrap();
    assert!(again.import.is_none());
    assert_eq!(again.last_modified.as_deref(), Some(modified.as_str()));
    assert_eq!(games(&db).len(), 4);
    // Changed: downloaded, and only the chapter it lacks goes in.
    let added = "[Event \"Fixture study: New\"]\n[White \"Gamma\"]\n[Black \"Delta\"]\n[Result \"*\"]\n\
                 [StudyName \"Fixture study\"]\n[ChapterURL \"https://lichess.org/study/FxStudy1/NewChap1\"]\n\n1. c4 *";
    {
        let mut st = server.state.lock().unwrap();
        st.studies[0].chapters.push(("NewChap1".into(), added.into()));
        st.studies[0].version += 1;
    }
    let changed = refresh_study_into(&client, &db, &link, None, Some(&modified), &NoProgress).unwrap();
    let import = changed.import.expect("downloaded");
    assert_eq!((import.import.imported, import.already), (1, 4));
    assert_ne!(changed.last_modified, Some(modified));
    assert_eq!(games(&db).len(), 5);
}

/// The dialog's look-up takes the light export (names and main lines), not
/// the full one the import downloads: a large annotated study on a slow line
/// would otherwise fail the look-up's 30 seconds though its import wouldn't.
#[test]
fn a_look_up_takes_the_light_export() {
    let server = Lichess::start(0);
    public_study(&server);
    let client = client(&server);
    let outline = fetch_study_outline(&client, &study("FxStudy1"), None, &|| false).unwrap();
    assert_eq!((outline.name.as_str(), outline.chapters.len()), ("Fixture study", 4));
    assert_eq!(outline.chapters[0].name, "Student - Coach");
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    for part in ["clocks=false", "comments=false", "variations=false", "orientation=true"] {
        assert!(reqs[0].contains(part), "{reqs:?}");
    }
    let e = fetch_study_outline(&client, &study("Nothing1"), None, &|| false).unwrap_err();
    assert!(matches!(e, NetError::NoSuchStudy), "{e}");
}

#[test]
fn one_chapter_then_the_rest_of_the_study() {
    let server = Lichess::start(0);
    public_study(&server);
    let client = client(&server);
    let (_dir, db) = new_db();
    let chapter = server.study("FxStudy1").unwrap().chapters[1].0.clone();
    let link = parse_study_link(&format!("lichess.org/study/FxStudy1/{chapter}#5")).unwrap();
    assert_eq!(link.chapter.as_deref(), Some(chapter.as_str()));
    let s = download_study_into(&client, &db, &link, None, None, &NoProgress).unwrap();
    assert_eq!((s.chapters, s.import.imported), (1, 1));
    assert!(server.requests()[0].starts_with(&format!("GET /api/study/FxStudy1/{chapter}.pgn?")));
    let s = download_study_into(&client, &db, &study("FxStudy1"), None, None, &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already), (3, 1));
}

#[test]
fn private_studies_need_their_owners_token() {
    let server = Lichess::start(0);
    let ch = chapters();
    server.add_study("Private1", "Mine", "Owner", "private", &[&ch[0]]);
    server.add_study("Unlistd1", "Hidden", "Owner", "unlisted", &[&ch[1]]);
    server.add_study("Public01", "Open", "Owner", "public", &[&ch[3]]);
    server.add_token("Owner", "lio_owner", "study:read,study:write");
    server.add_token("Other", "lio_other", "study:read,study:write");
    let client = client(&server);
    let (_dir, db) = new_db();
    let (owner, other) = (Token::new("lio_owner"), Token::new("lio_other"));
    for id in ["Private1", "Unlistd1"] {
        let e = download_study_into(&client, &db, &study(id), None, None, &NoProgress).unwrap_err();
        assert!(matches!(e, NetError::StudyNeedsLogin), "{e}");
        assert!(e.to_string().contains("Log in to Lichess"), "{e}");
        let e = download_study_into(&client, &db, &study(id), Some(&other), None, &NoProgress).unwrap_err();
        assert!(matches!(e, NetError::StudyForbidden), "{e}");
        let s = download_study_into(&client, &db, &study(id), Some(&owner), None, &NoProgress).unwrap();
        assert_eq!(s.import.imported, 1);
    }
    let e = download_study_into(
        &client,
        &db,
        &study("Private1"),
        Some(&Token::new("lio_gone")),
        None,
        &NoProgress,
    )
    .unwrap_err();
    assert!(matches!(e, NetError::TokenRejected), "{e}");
    let e = download_study_into(&client, &db, &study("Nothing1"), None, None, &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::NoSuchStudy), "{e}");

    // "My studies": the public ones without the token, all with it.
    let names = |t: Option<&Token>| -> Vec<String> {
        studies_of(&client, "owner", t, &|| false).unwrap().into_iter().map(|s| s.name).collect()
    };
    assert_eq!(names(None), ["Open"]);
    assert_eq!(names(Some(&owner)), ["Open", "Hidden", "Mine"], "newest first");
    assert_eq!(names(Some(&other)), ["Open"]);
    assert!(
        server.requests().iter().any(|r| r.starts_with("GET /api/study/by/owner") && r.ends_with("[token]"))
    );
}

struct CancelAfter(Instant);

impl Progress for CancelAfter {
    fn report(&self, _: u64, _: u64) {}
    fn is_cancelled(&self) -> bool {
        Instant::now() >= self.0
    }
}

#[test]
fn cancel_takes_effect_while_lichess_keeps_us_waiting() {
    // A server that accepts and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(4).collect();
        std::thread::sleep(Duration::from_secs(30));
        drop(held);
    });
    let client = Client::with_servers(&url, &url);
    let (_dir, db) = new_db();
    let started = Instant::now();
    let progress = CancelAfter(started + Duration::from_millis(500));
    let e = download_study_into(&client, &db, &study("FxStudy1"), None, None, &progress).unwrap_err();
    assert!(matches!(e, NetError::Cancelled), "{e}");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

// ---- NET-11: logging in -----------------------------------------------------

/// The browser's part: checks what the app asks for, then lets it in.
fn browser(url: &str) -> Result<(), String> {
    assert!(url.contains(
        "/oauth?response_type=code&client_id=dev.chessdb.test&redirect_uri=http%3A%2F%2F127.0.0.1%3A"
    ));
    assert!(url.contains("&code_challenge_method=S256&code_challenge="));
    assert!(url.contains("&scope=study%3Aread%20study%3Awrite&state="));
    oauth::approve_in_background(url);
    Ok(())
}

#[test]
fn logs_in_through_the_browser_with_pkce() {
    let server = Lichess::start(0);
    server.state.lock().unwrap().approve_as = "DrNykterstein".into();
    let client = client(&server);
    let login = oauth::login(&client, "dev.chessdb.test", &browser, &|| false).unwrap();
    assert_eq!(login.username, "DrNykterstein");
    assert!(login.expires.is_some_and(|e| e > chess_net::time::now_ms()));
    // The stand-in gave the token only for the right verifier.
    let st = server.state.lock().unwrap();
    assert_eq!(st.tokens.get(login.token.secret()).map(|t| t.1.as_str()), Some("study:read,study:write"));
    drop(st);
    assert_eq!(oauth::account(&client, &login.token, &|| false).unwrap(), "DrNykterstein");

    // Logging out revokes it.
    oauth::revoke(&client, &login.token).unwrap();
    assert!(server.state.lock().unwrap().tokens.is_empty());
    let e = oauth::account(&client, &login.token, &|| false).unwrap_err();
    assert!(matches!(e, NetError::TokenRejected), "{e}");
    oauth::revoke(&client, &login.token).expect("a token Lichess doesn't know counts as revoked");
}

/// Cancel or Log out after the browser came back with the code: the token
/// Lichess gave for it is revoked, not left working with nobody holding it.
#[test]
fn a_login_stopped_once_the_code_came_revokes_its_token() {
    let server = Lichess::start(0);
    let client = client(&server);
    let exchanged = || server.requests().iter().any(|r| r == "POST /api/token");
    let e = oauth::login(&client, "dev.chessdb.test", &browser, &exchanged).unwrap_err();
    assert!(matches!(e, NetError::Cancelled), "{e}");
    assert!(server.state.lock().unwrap().tokens.is_empty(), "revoked");
    let reqs = server.requests();
    assert!(reqs.iter().any(|r| r == "DELETE /api/token [token]"), "{reqs:?}");
    assert!(!reqs.iter().any(|r| r.starts_with("GET /api/account")), "stopped before asking whose: {reqs:?}");
}

/// Lichess failing to say whose the new token is: the login fails, and the
/// token goes.
#[test]
fn a_login_whose_account_fails_revokes_its_token() {
    let server = Lichess::start(0);
    server.state.lock().unwrap().fail.push(("GET /api/account".into(), 503));
    let e = oauth::login(&client(&server), "dev.chessdb.test", &browser, &|| false).unwrap_err();
    assert!(matches!(e, NetError::Status { code: 503, .. }), "{e}");
    assert!(server.state.lock().unwrap().tokens.is_empty(), "revoked");
}

#[test]
fn a_login_the_user_refuses() {
    let server = Lichess::start(0);
    server.state.lock().unwrap().deny = true;
    let e = oauth::login(&client(&server), "dev.chessdb.test", &browser, &|| false).unwrap_err();
    assert!(matches!(&e, NetError::Login(why) if why.contains("wasn't let in")), "{e}");
    assert!(server.state.lock().unwrap().tokens.is_empty());
}

/// A raw request to the login's listener; the answer's status line.
fn knock(port: &str, target: &str) -> String {
    let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    write!(s, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out.lines().next().unwrap_or("").to_owned()
}

#[test]
fn the_listener_ignores_other_requests_and_a_wrong_state() {
    let server = Lichess::start(0);
    let client = client(&server);
    let knocked = std::sync::Arc::new(Mutex::new(Vec::new()));
    let open = |url: &str| {
        let (url, knocked) = (url.to_owned(), knocked.clone());
        // While the login listens: other requests first, then the browser.
        std::thread::spawn(move || {
            let port = url.split("127.0.0.1%3A").nth(1).unwrap().split('%').next().unwrap().to_owned();
            let state = url.split("&state=").nth(1).unwrap().to_owned();
            let mut k = knocked.lock().unwrap();
            k.push(knock(&port, "/favicon.ico"));
            k.push(knock(&port, "/?code=forged&state=not-this-login"));
            k.push(knock(&port, &format!("/?code=forged&state={state}x")));
            drop(k);
            oauth::approve_in_background(&url);
        });
        Ok(())
    };
    let login = oauth::login(&client, "dev.chessdb.test", &open, &|| false).unwrap();
    assert_eq!(login.username, "Owner");
    let k = knocked.lock().unwrap().clone();
    assert!(k[0].contains("404"), "{k:?}");
    assert!(k[1].contains("400") && k[2].contains("400"), "a code for another state is turned away: {k:?}");
}

#[test]
fn a_login_gives_up_when_the_browser_never_comes_back() {
    let server = Lichess::start(0);
    let client = client(&server).with_login_wait(Duration::from_millis(400));
    let e = oauth::login(&client, "dev.chessdb.test", &|_| Ok(()), &|| false).unwrap_err();
    assert!(matches!(&e, NetError::Login(why) if why.contains("nothing came back")), "{e}");
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let e = oauth::login(
        &client.clone().with_login_wait(Duration::from_secs(60)),
        "dev.chessdb.test",
        &|_| {
            stop.store(true, Ordering::Relaxed);
            Ok(())
        },
        &|| stop.load(Ordering::Relaxed),
    )
    .unwrap_err();
    assert!(matches!(e, NetError::Cancelled), "{e}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let e = oauth::login(&client, "x", &|_| Err("no browser".into()), &|| false).unwrap_err();
    assert!(e.to_string().contains("no browser"), "{e}");
}

#[test]
fn a_pasted_token_is_checked() {
    let server = Lichess::start(0);
    server.add_token("Owner", "lio_full", "study:read,study:write,puzzle:read");
    server.add_token("Owner", "lio_read", "study:read");
    let client = client(&server);
    let login = oauth::check_token(&client, &Token::new(" lio_full\n"), &|| false).unwrap();
    assert_eq!((login.username.as_str(), login.token.secret()), ("Owner", "lio_full"));
    let e = oauth::check_token(&client, &Token::new("lio_read"), &|| false).unwrap_err();
    assert!(matches!(&e, NetError::MissingScopes(s) if s == "study:write"), "{e}");
    for bad in ["lio_unknown", "", "two,tokens"] {
        let e = oauth::check_token(&client, &Token::new(bad), &|| false).unwrap_err();
        assert!(matches!(e, NetError::TokenRejected), "{bad}: {e}");
    }
}

#[test]
fn own_games_are_downloaded_with_the_token() {
    let server = Lichess::start(0);
    server.add_token("DrNykterstein", "lio_mine", "study:read,study:write");
    let client = client(&server).with_lichess_login("drnykterstein", Token::new("lio_mine"));
    let (_dir, db) = new_db();
    let request = |name: &str| Request {
        site: Site::Lichess,
        username: name.into(),
        speeds: vec![],
        rated_only: false,
        period: Period::All,
        until: None,
        color: None,
        max_games: None,
    };
    for name in ["DrNykterstein", "Someone"] {
        download_into(&client, &db, &request(name), &NoProgress).unwrap();
    }
    let games = || -> Vec<String> {
        server.requests().into_iter().filter(|r| r.starts_with("GET /api/games/")).collect()
    };
    let g = games();
    assert_eq!(g.len(), 2, "{g:?}");
    assert!(g[0].contains("/DrNykterstein?") && g[0].ends_with("[token]"), "{g:?}");
    assert!(!g[1].ends_with("[token]"), "someone else's games without it: {g:?}");
    assert!(!client.lichess_login_rejected());

    // Revoked since: the games come without it, and the caller learns to
    // forget the login.
    server.state.lock().unwrap().tokens.clear();
    let s = download_into(&client, &db, &request("DrNykterstein"), &NoProgress).unwrap();
    assert!(s.stopped.is_none() && s.already == 1, "{s:?}");
    let g = games();
    assert!(g[2].ends_with("[token]") && !g[3].ends_with("[token]"), "{g:?}");
    assert!(client.lichess_login_rejected(), "the 401 is reported");
}

// ---- NET-12: games to studies -----------------------------------------------

/// `n` small games, each with a number.
fn numbered(n: usize) -> Vec<String> {
    (1..=n)
        .map(|i| {
            format!("[Event \"Game {i}\"]\n[White \"W{i}\"]\n[Black \"B{i}\"]\n[Result \"*\"]\n\n1. e4 e5 *")
        })
        .collect()
}

fn writer(server: &Lichess) -> Token {
    server.add_token("Owner", "lio_owner", "study:read,study:write");
    Token::new("lio_owner")
}

#[test]
fn games_go_to_new_private_studies_of_64_chapters() {
    let server = Lichess::start(0);
    let token = writer(&server);
    let client = client(&server);
    let target = ExportTarget::New { name: "Club games".into(), visibility: Visibility::Private };
    let s = export_to_studies(&client, &token, &target, &numbered(130), &NoProgress).unwrap();
    assert!(s.stopped.is_none() && !s.cancelled, "{:?}", s.stopped);
    assert_eq!((s.games, s.sent(), s.chapters(), s.not_sent()), (130, 130, 130, 0));
    let names: Vec<&str> = s.studies.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Club games (1/3)", "Club games (2/3)", "Club games (3/3)"]);
    assert_eq!(s.studies.iter().map(|s| s.sent).collect::<Vec<_>>(), [64, 64, 2]);
    assert!(s.studies.iter().all(|s| s.made && s.url == format!("{}/study/{}", server.url, s.id)));
    let made = server.studies_of("Owner");
    assert_eq!(made.len(), 3);
    for (study, want) in made.iter().zip([64, 64, 2]) {
        assert_eq!(study.chapters.len(), want);
        assert!(!study.empty_first, "the empty first chapter went");
        assert_eq!(study.visibility, "private");
        assert_eq!(study.settings.get("cloneable").map(String::as_str), Some("owner"));
        assert_eq!(study.settings.get("shareable").map(String::as_str), Some("owner"));
    }
    assert!(made[2].chapters[1].1.contains("[Event \"Game 130\"]"), "in order");
    let writes: Vec<String> = server.requests().into_iter().filter(|r| r.starts_with("POST")).collect();
    assert_eq!(writes.len(), 6, "a study and its games, one request each: {writes:?}");
}

#[test]
fn the_daily_limit_stops_the_export_and_says_where_the_games_went() {
    let server = Lichess::start(0);
    let token = writer(&server);
    server.state.lock().unwrap().daily_limit = 1;
    let client = client(&server);
    let target = ExportTarget::New { name: "Club games".into(), visibility: Visibility::Unlisted };
    let s = export_to_studies(&client, &token, &target, &numbered(130), &NoProgress).unwrap();
    assert!(matches!(s.stopped, Some(NetError::StudyLimit)), "{:?}", s.stopped);
    assert_eq!((s.sent(), s.not_sent()), (64, 66));
    assert_eq!(s.studies.len(), 1);
    assert_eq!(s.studies[0].name, "Club games (1/3)");
    let message = NetError::StudyLimit.to_string();
    assert!(message.contains("30 new studies a day") && message.contains("tomorrow"), "{message}");
    let creates = server.requests().into_iter().filter(|r| r.starts_with("POST /api/study ")).count();
    assert_eq!(creates, 2, "the refused one isn't asked again");

    // Already at the limit: nothing sent, so it's an error.
    let e = export_to_studies(&client, &token, &target, &numbered(3), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::StudyLimit), "{e}");
    // More than a day's studies are refused before asking.
    let before = server.requests().len();
    let e = export_to_studies(&client, &token, &target, &numbered(30 * 64 + 1), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::TooManyStudies { games: 1921, studies: 31 }), "{e}");
    assert_eq!(server.requests().len(), before);
    let e = export_to_studies(
        &client,
        &token,
        &ExportTarget::New { name: " x ".into(), visibility: Visibility::Private },
        &numbered(1),
        &NoProgress,
    )
    .unwrap_err();
    assert!(matches!(e, NetError::BadStudyName), "{e}");
}

#[test]
fn games_go_to_an_existing_study_that_has_room() {
    let server = Lichess::start(0);
    let token = writer(&server);
    let sixty = numbered(60);
    let refs: Vec<&str> = sixty.iter().map(String::as_str).collect();
    server.add_study("Exists01", "Repertoire", "Owner", "private", &refs);
    server.add_study("Theirs01", "Not mine", "Other", "public", &refs[..1]);
    let client = client(&server);
    let target = ExportTarget::Existing("Exists01".into());
    let e = export_to_studies(&client, &token, &target, &numbered(5), &NoProgress).unwrap_err();
    assert!(matches!(&e, NetError::StudyFull { name, room: 4 } if name == "Repertoire"), "{e}");
    assert!(e.to_string().contains("room for 4 more chapters"), "{e}");
    assert!(server.requests().iter().all(|r| !r.starts_with("POST")), "nothing sent");
    let s = export_to_studies(&client, &token, &target, &numbered(4), &NoProgress).unwrap();
    assert_eq!((s.sent(), s.chapters()), (4, 4));
    assert!(!s.studies[0].made);
    let study = server.study("Exists01").unwrap();
    assert_eq!(study.chapters.len(), 64);
    assert!(study.chapters[0].1.contains("[Event \"Game 1\"]"), "its own chapters stay");
    let e = export_to_studies(
        &client,
        &token,
        &ExportTarget::Existing("Theirs01".into()),
        &numbered(1),
        &NoProgress,
    )
    .unwrap_err();
    assert!(matches!(e, NetError::NotContributor), "{e}");
    let e =
        export_to_studies(&client, &Token::new("lio_gone"), &target, &numbered(1), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::TokenRejected), "{e}");
}

/// A token Lichess turns down after the export made a study: the summary
/// says so (`stopped`), for the app to forget the login.
#[test]
fn a_token_turned_down_partway_stops_the_export() {
    let server = Lichess::start(0);
    let token = writer(&server);
    server.state.lock().unwrap().fail.push(("POST /api/study/".into(), 401));
    let target = ExportTarget::New { name: "Club games".into(), visibility: Visibility::Private };
    let s = export_to_studies(&client(&server), &token, &target, &numbered(3), &NoProgress).unwrap();
    assert!(matches!(s.stopped, Some(NetError::TokenRejected)), "{:?}", s.stopped);
    assert_eq!((s.studies.len(), s.sent()), (1, 0));
    assert!(s.studies[0].made);
}

#[test]
fn a_cancel_stops_between_two_requests() {
    struct CancelAfterReports(std::cell::Cell<u32>);
    impl Progress for CancelAfterReports {
        fn report(&self, _: u64, _: u64) {
            self.0.set(self.0.get() + 1);
        }
        fn is_cancelled(&self) -> bool {
            self.0.get() >= 1
        }
    }
    let server = Lichess::start(0);
    let token = writer(&server);
    let target = ExportTarget::New { name: "Club games".into(), visibility: Visibility::Private };
    let s =
        export_to_studies(&client(&server), &token, &target, &numbered(130), &CancelAfterReports(0.into()))
            .unwrap();
    assert!(s.cancelled);
    assert_eq!((s.sent(), s.studies.len()), (64, 1), "the first study's games, and what Lichess has");
    assert_eq!(server.studies_of("Owner").len(), 1);
}

#[test]
fn a_round_trip_through_a_study_keeps_the_moves_and_comments() {
    let server = Lichess::start(0);
    let token = writer(&server);
    let client = client(&server);
    let (_dir, first) = new_db();
    chess_db::import_pgn(
        &first,
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pgn/lichess-study.pgn"),
        &NoProgress,
    )
    .unwrap();
    let pgns: Vec<String> = {
        let db = first.lock().unwrap();
        (0..db.count()).map(|i| chess_core_pgn(&db.read_game(i).unwrap())).collect()
    };
    let target = ExportTarget::New { name: "Round trip".into(), visibility: Visibility::Private };
    let s = export_to_studies(&client, &token, &target, &pgns, &NoProgress).unwrap();
    assert_eq!(s.chapters(), 4);
    let (_dir2, second) = new_db();
    let link = study(&s.studies[0].id);
    let back = download_study_into(&client, &second, &link, Some(&token), None, &NoProgress).unwrap();
    assert_eq!(back.import.imported, 4);
    let (a, b) = (first.lock().unwrap(), second.lock().unwrap());
    for i in 0..4 {
        let (ga, gb) = (a.read_game(i).unwrap(), b.read_game(i).unwrap());
        assert_eq!(chess_core::pgn::write_movetext(&ga), chess_core::pgn::write_movetext(&gb), "game {i}");
        assert_eq!(gb.headers.get_extra("StudyName"), Some("Round trip"));
    }
}

fn chess_core_pgn(game: &chess_core::game::Game) -> String {
    chess_core::pgn::write_game(game)
}
