//! Running the server: rate limits, idle databases closing, metrics and
//! backups.

mod common;

use chess_server::housekeeping;
use common::TestServer;
use serde_json::json;

#[test]
fn idle_databases_close_and_open_again_never_while_in_use() {
    let s = TestServer::with(|c| c.idle_close = Some(std::time::Duration::from_millis(150)));
    let admin = s.admin();
    let db = admin.create_db("Idle", "private");
    admin.import_fixture(&db, "reference.pgn");
    admin.get(&format!("/api/databases/{db}/games")).ok();
    let folder = s.data_dir().join("dbs").join(format!("{db}.chessdb"));
    // Held (as by a request or a job): it stays, however long.
    let held = s.state.pool.get(&db).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(250));
    housekeeping::once(&s.state);
    assert!(s.state.pool.get_open(&db).is_some());
    assert!(matches!(chess_db::Database::open(&folder), Err(chess_db::DbError::Locked(_))));
    drop(held);
    // Idle: closed, its lock released.
    std::thread::sleep(std::time::Duration::from_millis(250));
    housekeeping::once(&s.state);
    assert!(s.state.pool.get_open(&db).is_none());
    drop(chess_db::Database::open(&folder).expect("closed: anyone may open it"));
    // The next request opens it again.
    assert_eq!(admin.get(&format!("/api/databases/{db}/games")).ok().json()["total"], 56);
    assert_eq!(admin.get(&format!("/api/databases/{db}/tree")).ok().json()["total"]["games"], 56);
    // Used just now: it stays.
    housekeeping::once(&s.state);
    assert!(s.state.pool.get_open(&db).is_some());
}

#[test]
fn metrics_need_their_token_and_name_no_database_or_user() {
    let off = TestServer::start();
    assert_eq!(off.client().get("/metrics").status, 404, "off without a token");
    let token = "a metrics token long enough";
    let s = TestServer::with(|c| c.metrics_token = Some(token.into()));
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Secret prep of Alice", "private");
    alice.import_fixture(&db, "reference.pgn");
    alice.get(&format!("/api/databases/{db}/games?white=Carlsen")).ok();
    alice.get(&format!("/api/databases/{db}/games/3")).ok();
    assert_eq!(alice.get(&format!("/api/databases/{db}/games/999")).status, 404);
    assert_eq!(s.client().get("/metrics").status, 401);
    assert_eq!(s.client().with_bearer("wrong").get("/metrics").status, 401);
    assert_eq!(alice.get("/metrics").status, 401, "a session isn't the token");
    let r = s.client().with_bearer(token).get("/metrics").ok();
    assert!(r.header("content-type").unwrap().starts_with("text/plain; version=0.0.4"));
    let text = r.body;
    for want in [
        "chess_server_http_requests_total{route=\"/api/databases/{db}/games\",method=\"GET\",status=\"200\"} 1",
        "chess_server_http_requests_total{route=\"/api/databases/{db}/games/{game}\",method=\"GET\",status=\"404\"} 1",
        "chess_server_http_request_duration_seconds_bucket{route=\"/api/databases/{db}/games\",method=\"GET\",le=\"+Inf\"} 1",
        "chess_server_open_databases 1",
        "chess_server_jobs_running{kind=\"import\"} 0",
        "chess_server_games 56",
        "chess_server_databases 1",
        "chess_server_users 2",
        "# TYPE chess_server_http_request_duration_seconds histogram",
    ] {
        assert!(text.contains(want), "{want} in\n{text}");
    }
    for leak in [db.as_str(), "Secret", "alice", "Alice", "Carlsen"] {
        assert!(!text.contains(leak), "{leak} in the metrics");
    }
}

/// Requests until one is refused; the refusal.
fn until_limited(mut request: impl FnMut() -> common::Res) -> (usize, common::Res) {
    for n in 0..100 {
        let r = request();
        if r.status == 429 {
            return (n, r);
        }
        assert!((200..300).contains(&r.status), "{}", r.body);
    }
    panic!("never limited");
}

#[test]
fn requests_are_limited_per_user_and_per_address_without_a_login() {
    let s = TestServer::with(|c| {
        c.rate_user = Some(60);
        c.rate_anon = Some(60);
    });
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let token =
        alice.post("/api/tokens", json!({ "name": "t" })).ok().json()["token"].as_str().unwrap().to_owned();
    let api = alice.with_bearer(&token);

    // A user: a burst of 15 (a quarter of a minute's), whichever way they come.
    let (n, r) = until_limited(|| alice.get("/api/me"));
    assert!((10..=16).contains(&n), "{n}");
    assert_eq!(r.code(), "rateLimited");
    assert!(r.header("retry-after").unwrap().parse::<u64>().unwrap() >= 1);
    assert_eq!(api.get("/api/me").status, 429, "the same user with a token");
    bob.get("/api/me").ok();

    // Without a login: per address, health checks aside.
    let anon = s.client();
    let (n, r) = until_limited(|| anon.get("/api/setup"));
    assert!(n <= 16, "{n}");
    assert!(r.header("retry-after").is_some());
    for _ in 0..30 {
        anon.get("/api/health").ok();
    }
    // Wrong credentials count against the address too.
    assert_eq!(anon.with_bearer("cdb_nothing").get("/api/me").status, 429);
    bob.get("/api/me").ok();
}
