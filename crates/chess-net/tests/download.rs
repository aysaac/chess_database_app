//! NET-01 end to end: a player's games from a local server standing in for
//! Lichess or Chess.com, into a database, then again for the new ones.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chess_db::{Database, NoProgress, Progress};
use chess_net::time::date_time_ms;
use chess_net::{Client, NetError, Period, Request, Site, Speed, download_into};
use serde_json::json;

/// What the server answers: a status and a body, or a body cut off after
/// that many bytes (the connection then closes), or one that stops sending.
enum Reply {
    Ok(u16, String),
    Bytes(Vec<u8>),
    Slow(String, Duration),
    CutOff(String, usize),
    Hang(String),
}

type Route = dyn Fn(&str, &str) -> Reply + Send + Sync;

/// A one-request-per-connection HTTP/1.1 server on a free port. Each
/// request's path and query are kept, in order.
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(route: impl Fn(&str, &str) -> Reply + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (log, route): (_, Arc<Route>) = (requests.clone(), Arc::new(route));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let (log, route) = (log.clone(), route.clone());
                std::thread::spawn(move || serve(stream.unwrap(), &log, &*route));
            }
        });
        Server { url, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>, route: &Route) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let target = line.split(' ').nth(1).unwrap_or("/").to_owned();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
            break;
        }
    }
    log.lock().unwrap().push(target.clone());
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let (status, body, send) = match route(path, query) {
        Reply::Ok(status, body) => {
            let n = body.len();
            (status, body.into_bytes(), n)
        }
        Reply::Bytes(body) => {
            let n = body.len();
            (200, body, n)
        }
        Reply::CutOff(body, n) => (200, body.into_bytes(), n),
        Reply::Slow(body, over) => {
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                .unwrap();
            let pieces = 30;
            for chunk in body.as_bytes().chunks(body.len().div_ceil(pieces)) {
                stream.write_all(chunk).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(over / pieces as u32);
            }
            return;
        }
        Reply::Hang(body) => {
            let n = body.len();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", n + 100_000)
                .unwrap();
            stream.write_all(body.as_bytes()).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
    };
    let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body[..send.min(body.len())]);
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn lichess_pgn(id: &str, white: &str, black: &str, date: &str, time: &str) -> String {
    format!(
        "[Event \"Rated blitz game\"]\n[Site \"https://lichess.org/{id}\"]\n[Date \"{date}\"]\n\
         [White \"{white}\"]\n[Black \"{black}\"]\n[Result \"1-0\"]\n[GameId \"{id}\"]\n\
         [UTCDate \"{date}\"]\n[UTCTime \"{time}\"]\n[Variant \"Standard\"]\n\n\
         1. e4 {{ [%eval 0.2] [%clk 0:03:00] }} 1... e5 {{ [%clk 0:03:00] }} 2. Qh5 1-0\n\n\n"
    )
}

/// A Lichess NDJSON line for the game.
fn lichess_line(id: &str, white: &str, black: &str, date: &str, time: &str) -> String {
    let at = date_time_ms(date, Some(time)).unwrap() + 345;
    let pgn = lichess_pgn(id, white, black, date, time);
    format!("{}\n", json!({ "id": id, "createdAt": at, "rated": true, "perf": "blitz", "pgn": pgn }))
}

/// The same for a game started at `at` (ms).
fn lichess_line_at(id: &str, white: &str, black: &str, at: i64) -> String {
    let day = chess_net::time::ymd(at).replace('-', ".");
    let secs = at.rem_euclid(86_400_000) / 1000;
    let time = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    let pgn = lichess_pgn(id, white, black, &day, &time);
    format!("{}\n", json!({ "id": id, "createdAt": at, "rated": true, "perf": "blitz", "pgn": pgn }))
}

fn profile(name: &str) -> Reply {
    Reply::Ok(200, json!({ "id": name.to_lowercase(), "username": name, "createdAt": 1_600_000_000_000i64, "count": { "all": 3 } }).to_string())
}

fn new_db() -> (tempfile::TempDir, Mutex<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(&dir.path().join("Online")).unwrap();
    (dir, Mutex::new(db))
}

fn request(site: Site, period: Period) -> Request {
    Request { site, username: "drnykterstein".into(), speeds: Vec::new(), rated_only: false, period }
}

/// The games as (White, Black, Date), in the database's order.
fn games(db: &Mutex<Database>) -> Vec<(String, String, String)> {
    let db = db.lock().unwrap();
    (0..db.count()).map(|i| db.row(i).unwrap()).map(|r| (r.white, r.black, r.date)).collect()
}

const GAMES: [(&str, &str, &str, &str, &str); 3] = [
    ("AAAAAAA1", "DrNykterstein", "Alice", "2026.09.01", "10:00:00"),
    ("AAAAAAA2", "Bob", "DrNykterstein", "2026.09.15", "11:30:00"),
    ("AAAAAAA3", "DrNykterstein", "Carol", "2026.10.02", "09:15:20"),
];

#[test]
fn lichess_games_then_only_the_new_ones() {
    // Like Lichess: the games started at `since` or later, and one played
    // since the first download.
    let server = Server::start(move |path, query| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        "/api/games/user/DrNykterstein" => {
            // Lichess's `since` (with `perfType`) leaves out a game created at it.
            let since: Option<i64> = query_value(query, "since").map(|s| s.parse::<i64>().unwrap() + 1);
            let mut body = String::new();
            for (id, w, b, d, t) in &GAMES {
                if since.is_none_or(|s| date_time_ms(d, Some(t)).unwrap() + 345 >= s) {
                    body += &lichess_line(id, w, b, d, t);
                }
            }
            if let Some(s) = since {
                body += &lichess_line_at("AAAAAAA4", "Dave", "DrNykterstein", s + 1000);
            }
            Reply::Ok(200, body)
        }
        _ => Reply::Ok(404, "<html>".into()),
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();

    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already, s.since, s.cancelled), (3, 0, None, false));
    let covered = s.covered.expect("up to the newest game sent");
    assert_eq!(
        covered,
        date_time_ms("2026.10.02", Some("09:15:20")).unwrap() + 345,
        "the server's time, not ours"
    );
    assert!(s.stopped.is_none());
    assert_eq!(s.player.username, "DrNykterstein");
    assert_eq!(
        games(&db),
        [
            ("DrNykterstein".into(), "Alice".into(), "2026.09.01".into()),
            ("Bob".into(), "DrNykterstein".into(), "2026.09.15".into()),
            ("DrNykterstein".into(), "Carol".into(), "2026.10.02".into()),
        ]
    );
    let game = db.lock().unwrap().read_game(0).unwrap();
    assert!(game.has_comments() || game.preorder().len() > 1, "clock times and evals kept");
    let reqs = server.requests();
    let games_query = reqs.iter().find(|r| r.starts_with("/api/games/")).unwrap();
    for part in ["sort=dateAsc", "pgnInJson=true", "clocks=true", "evals=true", "opening=true"] {
        assert!(games_query.contains(part), "{games_query}");
    }
    let perf = query_value(games_query.split_once('?').unwrap().1, "perfType").unwrap();
    assert_eq!(perf.replace("%2C", ","), "ultraBullet,bullet,blitz,rapid,classical,correspondence");

    // Again from where it got to: the newest game again (skipped) and the new one.
    let s =
        download_into(&client, &db, &request(Site::Lichess, Period::Since(covered)), &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already, s.since), (1, 1, Some(covered)));
    let q = server.requests().last().unwrap().clone();
    assert_eq!(query_value(&q, "since"), Some((covered - 1).to_string().as_str()), "1 ms before: {q}");
    assert_eq!(games(&db).len(), 4);
    assert_eq!(games(&db)[3].0, "Dave");

    // Everything again: nothing new.
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already), (0, 3));
}

#[test]
fn lichess_filters_and_dates() {
    let server = Server::start(|path, _| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        "/api/games/user/DrNykterstein" => Reply::Ok(200, String::new()),
        _ => Reply::Ok(404, String::new()),
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let since = date_time_ms("2026-01-01", None).unwrap();
    let mut r = request(Site::Lichess, Period::Since(since));
    r.speeds = vec![Speed::Blitz, Speed::Correspondence];
    r.rated_only = true;
    let s = download_into(&client, &db, &r, &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.since), (0, Some(since)));
    let q = server.requests().last().unwrap().clone();
    assert_eq!(
        query_value(&q, "perfType").map(|p| p.replace("%2C", ",")),
        Some("blitz,correspondence".into())
    );
    assert_eq!(query_value(&q, "rated"), Some("true"));
    assert_eq!(query_value(&q, "since"), Some((since - 1).to_string().as_str()));
}

fn chess_com_game(
    n: u64,
    white: &str,
    black: &str,
    date: &str,
    rules: &str,
    class: &str,
    rated: bool,
) -> serde_json::Value {
    let end = date_time_ms(date, Some("20:00:00")).unwrap() / 1000;
    let pgn = format!(
        "[Event \"Live Chess\"]\n[Site \"Chess.com\"]\n[Date \"{date}\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n\
         [Result \"0-1\"]\n[UTCDate \"{date}\"]\n[UTCTime \"19:55:00\"]\n\
         [Link \"https://www.chess.com/game/live/{n}\"]\n\n1. f3 {{[%clk 0:02:59.9]}} 1... e5 2. g4 Qh4# 0-1\n"
    );
    json!({ "url": format!("https://www.chess.com/game/live/{n}"), "pgn": pgn, "end_time": end,
            "rated": rated, "rules": rules, "time_class": class })
}

#[test]
fn chess_com_months_filters_and_new_ones() {
    let second = Arc::new(AtomicBool::new(false));
    let s2 = second.clone();
    let server = Server::start(move |path, _| {
        let later = s2.load(Ordering::SeqCst);
        match path {
            "/pub/player/drnykterstein" => Reply::Ok(
                200,
                json!({ "username": "drnykterstein", "url": "https://www.chess.com/member/DrNykterstein", "joined": 1_500_000_000 }).to_string(),
            ),
            "/pub/player/drnykterstein/games/archives" => {
                let base = "https://api.chess.com/pub/player/drnykterstein/games";
                Reply::Ok(200, json!({ "archives": [format!("{base}/2026/09"), format!("{base}/2026/08"), format!("{base}/2026/10")] }).to_string())
            }
            "/pub/player/drnykterstein/games/2026/08" => Reply::Ok(410, String::new()),
            "/pub/player/drnykterstein/games/2026/09" => {
                assert!(!later, "a month before the start isn't asked for");
                Reply::Ok(200, json!({ "games": [
                    // Newest last in the month, as Chess.com sends them; sorted anyway.
                    chess_com_game(3, "Bob", "DrNykterstein", "2026.09.20", "chess", "blitz", true),
                    chess_com_game(2, "DrNykterstein", "Alice", "2026.09.10", "chess", "bullet", true),
                    chess_com_game(4, "Eve", "DrNykterstein", "2026.09.21", "chess960", "blitz", true),
                    chess_com_game(5, "Eve", "DrNykterstein", "2026.09.22", "chess", "rapid", true),
                    chess_com_game(6, "Eve", "DrNykterstein", "2026.09.23", "chess", "blitz", false),
                ]}).to_string())
            }
            "/pub/player/drnykterstein/games/2026/10" => {
                let mut games = vec![chess_com_game(7, "DrNykterstein", "Carol", "2026.10.05", "chess", "blitz", true)];
                if later {
                    games.push(chess_com_game(8, "Dave", "DrNykterstein", "2026.10.21", "chess", "bullet", true));
                }
                Reply::Ok(200, json!({ "games": games }).to_string())
            }
            _ => Reply::Ok(404, json!({ "code": 0, "message": "not found" }).to_string()),
        }
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let mut r = request(Site::ChessCom, Period::All);
    r.username = "DrNykterstein".into();
    r.speeds = vec![Speed::Bullet, Speed::Blitz];
    r.rated_only = true;
    let s = download_into(&client, &db, &r, &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already), (3, 0));
    assert_eq!(s.player.username, "DrNykterstein", "as the profile's page writes it");
    let g = games(&db);
    assert_eq!(
        g.iter().map(|(w, b, _)| format!("{w}-{b}")).collect::<Vec<_>>(),
        ["DrNykterstein-Alice", "Bob-DrNykterstein", "DrNykterstein-Carol"],
        "oldest first; no Chess960, rapid or unrated game"
    );
    assert!(server.requests().iter().all(|r| !r.contains("DrNykterstein")), "usernames in lowercase");

    second.store(true, Ordering::SeqCst);
    let since = date_time_ms("2026-10-01", None).unwrap();
    r.period = Period::Since(since);
    let s = download_into(&client, &db, &r, &NoProgress).unwrap();
    assert_eq!((s.import.imported, s.already, s.since), (1, 1, Some(since)));
    assert_eq!(games(&db).len(), 4);
}

#[test]
fn an_unknown_player_is_an_error() {
    let server = Server::start(|_, _| Reply::Ok(404, "{}".into()));
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    for site in [Site::Lichess, Site::ChessCom] {
        let e = download_into(&client, &db, &request(site, Period::All), &NoProgress).unwrap_err();
        assert!(matches!(e, NetError::NoSuchPlayer { .. }), "{e}");
        assert!(e.to_string().contains("has no player called drnykterstein"), "{e}");
    }
    let mut r = request(Site::Lichess, Period::All);
    r.username = "bad name".into();
    assert!(matches!(download_into(&client, &db, &r, &NoProgress), Err(NetError::BadUsername(_))));
    assert!(server.requests().len() == 2, "no request for a name that can't be one");
}

#[test]
fn a_failure_before_any_game_is_an_error() {
    let server = Server::start(|path, _| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => Reply::Ok(503, String::new()),
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let e = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::Status { site: Site::Lichess, code: 503 }), "{e}");
}

#[test]
fn too_many_requests_waits_and_asks_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let server = Server::start(move |path, _| match path {
        "/api/user/drnykterstein" if c.fetch_add(1, Ordering::SeqCst) < 2 => Reply::Ok(429, String::new()),
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => Reply::Ok(200, lichess_line("AAAAAAA1", "DrNykterstein", "Alice", "2026.09.01", "10:00:00")),
    });
    let client = Client::with_servers(&server.url, &server.url).with_retry_wait(Duration::from_millis(50));
    let (_dir, db) = new_db();
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap();
    assert_eq!(s.import.imported, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 3);

    let server = Server::start(|_, _| Reply::Ok(429, String::new()));
    let client = Client::with_servers(&server.url, &server.url).with_retry_wait(Duration::from_millis(10));
    let e = download_into(&client, &db, &request(Site::ChessCom, Period::All), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::RateLimited(Site::ChessCom)), "{e}");
    assert_eq!(server.requests().len(), 4, "the first request and 3 more");

    // A lookup the user waits for gives up at once.
    let server = Server::start(|_, _| Reply::Ok(429, String::new()));
    let client = Client::with_servers(&server.url, &server.url);
    let e = chess_net::player(&client.for_lookup(), Site::Lichess, "someone", &|| false).unwrap_err();
    assert!(matches!(e, NetError::RateLimited(Site::Lichess)), "{e}");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn a_stream_that_breaks_off_goes_on_from_the_last_game() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let server = Server::start(move |path, query| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => {
            let call = c.fetch_add(1, Ordering::SeqCst);
            let from = if call == 0 { 0 } else { 1 };
            let body: String =
                GAMES[from..].iter().map(|(id, w, b, d, t)| lichess_line(id, w, b, d, t)).collect();
            if call == 0 {
                // Two whole games, then a cut in the third one.
                let two = lichess_line(GAMES[0].0, GAMES[0].1, GAMES[0].2, GAMES[0].3, GAMES[0].4).len()
                    + lichess_line(GAMES[1].0, GAMES[1].1, GAMES[1].2, GAMES[1].3, GAMES[1].4).len();
                Reply::CutOff(body, two + 40)
            } else {
                let since = date_time_ms(GAMES[1].3, Some(GAMES[1].4)).unwrap() + 345;
                assert_eq!(query_value(query, "since"), Some((since - 1).to_string().as_str()), "{query}");
                Reply::Ok(200, body)
            }
        }
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap();
    assert!(s.stopped.is_none(), "{:?}", s.stopped);
    assert_eq!((s.import.imported, s.already), (3, 0), "the game sent again isn't counted as there");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn every_break_after_new_games_is_resumed() {
    // Each request sends one game more, then breaks off: 6 breaks.
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let lines: Vec<String> = (0..7)
        .map(|i| {
            lichess_line(
                &format!("AAAAAAB{i}"),
                "DrNykterstein",
                "Alice",
                "2026.09.01",
                &format!("10:0{i}:00"),
            )
        })
        .collect();
    let server = Server::start(move |path, query| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => {
            let call = c.fetch_add(1, Ordering::SeqCst);
            let since: i64 = query_value(query, "since").map_or(0, |s| s.parse::<i64>().unwrap() + 1);
            let from = lines
                .iter()
                .position(|l| {
                    serde_json::from_str::<serde_json::Value>(l).unwrap()["createdAt"].as_i64() >= Some(since)
                })
                .unwrap();
            let body: String = lines[from..].concat();
            let upto: usize = lines[from..(from + 2).min(7)].iter().map(String::len).sum();
            if call < 6 { Reply::CutOff(body, upto + 10) } else { Reply::Ok(200, body) }
        }
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap();
    assert!(s.stopped.is_none(), "{:?}", s.stopped);
    assert_eq!(s.import.imported, 7);
}

#[test]
fn a_stream_that_isnt_text_is_bad_data_at_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let server = Server::start(move |path, _| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => {
            c.fetch_add(1, Ordering::SeqCst);
            Reply::Bytes(b"{\"id\":\"AAAAAAA1\",\"createdAt\":1,\"pgn\":\"\xff\"}\n".to_vec())
        }
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let e = download_into(&client, &db, &request(Site::Lichess, Period::All), &NoProgress).unwrap_err();
    assert!(matches!(e, NetError::BadData { site: Site::Lichess, .. }), "{e}");
    assert!(e.to_string().ends_with("Try again later"), "{e}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "not asked again as if the connection broke");
}

#[test]
fn a_slow_but_steady_answer_isnt_stalled() {
    // A month that takes 3 s to come, a few bytes at a time, with a stall
    // timeout of 1 s.
    let month = json!({ "games": [chess_com_game(7, "DrNykterstein", "Carol", "2026.10.05", "chess", "blitz", true)] })
        .to_string();
    let server = Server::start(move |path, _| match path {
        "/pub/player/drnykterstein" => Reply::Ok(200, json!({ "username": "drnykterstein" }).to_string()),
        "/pub/player/drnykterstein/games/archives" => {
            Reply::Ok(200, json!({ "archives": ["x/games/2026/10"] }).to_string())
        }
        _ => Reply::Slow(month.clone(), Duration::from_secs(3)),
    });
    let client = Client::with_servers(&server.url, &server.url).with_stall_timeout(Duration::from_secs(1));
    let (_dir, db) = new_db();
    let s = download_into(&client, &db, &request(Site::ChessCom, Period::All), &NoProgress).unwrap();
    assert!(s.stopped.is_none(), "{:?}", s.stopped);
    assert_eq!(s.import.imported, 1);
}

#[test]
fn a_lookup_gives_up_after_its_timeout() {
    let server = Server::start(|_, _| Reply::Hang("{".into()));
    let client =
        Client::with_servers(&server.url, &server.url).with_lookup_timeout(Duration::from_millis(500));
    let started = Instant::now();
    let e = chess_net::player(&client.for_lookup(), Site::Lichess, "someone", &|| false).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(matches!(e, NetError::Unreachable { .. }), "{e}");
    assert!(!e.to_string().contains("io:"), "{e}");
}

struct CancelAfter {
    at: Instant,
}

impl Progress for CancelAfter {
    fn report(&self, _done: u64, _total: u64) {}
    fn is_cancelled(&self) -> bool {
        Instant::now() >= self.at
    }
}

#[test]
fn a_cancel_keeps_every_game_it_counts_as_covered() {
    // 60 games coming over 6 s, cancelled after 3: the games the reader
    // had read ahead go in too, so all those up to `covered` are there.
    let start = date_time_ms("2026-09-01", None).unwrap();
    let body: String = (0..60)
        .map(|i| lichess_line_at(&format!("AAAAAA{i:02}"), "DrNykterstein", "Alice", start + i * 60_000))
        .collect();
    let server = Server::start(move |path, _| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => Reply::Slow(body.clone(), Duration::from_secs(6)),
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let progress = CancelAfter { at: Instant::now() + Duration::from_secs(3) };
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &progress).unwrap();
    assert!(s.cancelled && s.import.imported > 0 && s.import.imported < 60, "{s:?}");
    let covered = s.covered.unwrap();
    let up_to = (covered - start) / 60_000 + 1;
    assert_eq!(s.import.imported as i64, up_to, "every game up to {covered} is in the database");
    assert_eq!(games(&db).len() as u64, s.import.imported);
}

#[test]
fn cancel_takes_effect_while_the_server_keeps_us_waiting() {
    let server = Server::start(|path, _| match path {
        "/api/user/drnykterstein" => profile("DrNykterstein"),
        _ => Reply::Hang(lichess_line("AAAAAAA1", "DrNykterstein", "Alice", "2026.09.01", "10:00:00")),
    });
    let client = Client::with_servers(&server.url, &server.url);
    let (_dir, db) = new_db();
    let started = Instant::now();
    let progress = CancelAfter { at: started + Duration::from_millis(700) };
    let s = download_into(&client, &db, &request(Site::Lichess, Period::All), &progress).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(s.cancelled);
    assert_eq!(s.import.imported, 1, "the game that came is kept");
}
