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

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn backups_are_made_online_and_restore_to_the_same_server() {
    let backups = tempfile::tempdir().unwrap();
    let dir = backups.path().join("backups");
    let s = TestServer::with(|c| {
        c.backup_dir = Some(dir.clone());
        c.backup_keep = 2;
    });
    let mut admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let private = alice.create_db("Alice's prep", "private");
    alice.import_fixture(&private, "reference.pgn");
    let shared = alice.create_db("Team", "shared");
    alice.import_fixture(&shared, "crosstable.pgn");
    alice.put(&format!("/api/databases/{shared}/shares/bob"), json!({ "access": "read" })).ok();
    let wide = admin.create_db("Reference", "server");
    admin.import_fixture(&wide, "variations.pgn");
    let token =
        alice.post("/api/tokens", json!({ "name": "t" })).ok().json()["token"].as_str().unwrap().to_owned();
    assert_eq!(bob.post("/api/admin/backups", json!({})).status, 403);

    // It waits for a write to the database to end (its writer slot).
    let open = s.state.pool.get(&private).unwrap();
    let user = s.state.store.user_by_name("alice").unwrap().unwrap().0;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let rx = std::sync::Mutex::new(rx);
    let writing = s.state.jobs.spawn("import", "Holding".into(), (&private, "x"), &user, move |_| {
        let _ = rx.lock().unwrap().recv();
        Ok(String::new())
    });
    *open.writer.lock().unwrap() = Some(writing);
    drop(open);
    let job = admin.post("/api/admin/backups", json!({})).ok().json()["job"].as_u64().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(700));
    assert_eq!(admin.get("/api/admin/jobs").ok().json()[0]["state"], "running", "waiting for the import");
    tx.send(()).unwrap();
    let done = admin.wait_job_admin(job);
    assert_eq!(done, "done");
    let listed = admin.get("/api/admin/backups").ok().json();
    assert_eq!(listed["backups"].as_array().unwrap().len(), 1, "{listed}");
    let backup = listed["backups"][0].clone();
    assert_eq!(backup["databases"], 3);
    assert!(backup["sizeBytes"].as_u64().unwrap() > 0);
    // Never downloadable: no route gives one.
    let name = backup["name"].as_str().unwrap();
    for path in [format!("/api/admin/backups/{name}"), format!("/api/admin/backups/{name}/server.sqlite")] {
        assert_eq!(admin.get(&path).status, 404);
    }
    // The newest two are kept.
    for _ in 0..2 {
        let job = admin.post("/api/admin/backups", json!({})).ok().json()["job"].as_u64().unwrap();
        assert_eq!(admin.wait_job_admin(job), "done");
    }
    let names: Vec<String> = admin.get("/api/admin/backups").ok().json()["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(!names.contains(&name.to_owned()), "the oldest went");
    let audit = admin.get("/api/admin/audit?action=backup").ok().json();
    assert!(audit["total"].as_u64().unwrap() >= 3);

    // Restored into a fresh data directory, the server is the same.
    let before: Vec<(String, String)> = [&alice, &bob, &admin]
        .iter()
        .flat_map(|c| {
            let dbs = c.get("/api/databases").ok().json();
            dbs.as_array()
                .unwrap()
                .iter()
                .map(|d| (d["id"].as_str().unwrap().to_owned(), d.to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    let games = |c: &common::Client, db: &str| c.get(&format!("/api/databases/{db}/export.pgn")).ok().body;
    let pgns = [games(&alice, &private), games(&bob, &shared), games(&bob, &wide)];
    let users = admin.get("/api/admin/users").ok().json();
    let restored = tempfile::tempdir().unwrap();
    let data = restored.path().join("data");
    let newest = dir.join(&names[0]);
    std::fs::create_dir_all(&data).unwrap();
    std::fs::copy(newest.join("server.sqlite"), data.join("server.sqlite")).unwrap();
    copy_dir(&newest.join("dbs"), &data.join("dbs"));
    drop(s);
    let r = TestServer::start_in(restored, |_| {});
    assert!(r.state.setup_token().is_none());
    let mut admin2 = r.client();
    admin2.login("admin", common::PASSWORD).ok();
    let mut alice2 = r.client();
    alice2.login("alice", common::PASSWORD).ok();
    let mut bob2 = r.client();
    bob2.login("bob", common::PASSWORD).ok();
    assert_eq!(admin2.get("/api/admin/users").ok().json().to_string().len(), users.to_string().len());
    let after: Vec<(String, String)> = [&alice2, &bob2, &admin2]
        .iter()
        .flat_map(|c| {
            let dbs = c.get("/api/databases").ok().json();
            dbs.as_array()
                .unwrap()
                .iter()
                .map(|d| (d["id"].as_str().unwrap().to_owned(), d.to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(after, before);
    assert_eq!([games(&alice2, &private), games(&bob2, &shared), games(&bob2, &wide)], pgns);
    // Private is still private; the token still works; sessions didn't survive.
    assert_eq!(bob2.get(&format!("/api/databases/{private}")).status, 404);
    assert_eq!(admin2.get(&format!("/api/databases/{private}/games")).status, 404);
    assert_eq!(r.client().with_bearer(&token).get("/api/me").ok().json()["username"], "alice");
    let mut old = r.client();
    old.cookie = admin.cookie.take();
    assert_eq!(old.get("/api/me").status, 401);
}

#[test]
fn scheduled_backups_start_when_one_is_due() {
    let backups = tempfile::tempdir().unwrap();
    let dir = backups.path().join("b");
    let s = TestServer::with(|c| {
        c.backup_dir = Some(dir.clone());
        c.backup_every = Some(std::time::Duration::from_secs(3600));
    });
    let admin = s.admin();
    housekeeping::once(&s.state);
    let jobs = admin.get("/api/admin/jobs").ok().json();
    let job = jobs.as_array().unwrap().iter().find(|j| j["kind"] == "backup").expect("a backup").clone();
    assert_eq!(job["startedBy"], "(schedule)");
    assert_eq!(admin.wait_job_admin(job["id"].as_u64().unwrap()), "done");
    housekeeping::once(&s.state);
    let jobs = admin.get("/api/admin/jobs").ok().json();
    assert_eq!(jobs.as_array().unwrap().iter().filter(|j| j["kind"] == "backup").count(), 1, "not due yet");
    assert_eq!(admin.get("/api/admin/backups").ok().json()["everyHours"], 1);
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

/// The routes anyone may call (login, setup, sign-up…) never look at
/// credentials: with some, working or not, a request there still counts
/// against the address.
#[test]
fn credentials_dont_get_round_the_address_limit() {
    let s = TestServer::with(|c| c.rate_anon = Some(60));
    let anon = s.client().with_bearer("cdb_nothing");
    let (n, r) = until_limited(|| anon.get("/api/setup"));
    assert!(n <= 16, "{n}");
    assert_eq!(r.code(), "rateLimited");
    // The same bucket as without credentials.
    assert_eq!(s.client().get("/api/setup").status, 429);
    let r = anon.post("/api/login", json!({ "username": "x", "password": "y" }));
    assert_eq!(r.status, 429, "{}", r.body);
}

/// A second server on the same data directory is refused: it would take
/// the first one's running imports for interrupted ones and delete their
/// uploads.
#[test]
fn one_server_per_data_directory() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Mine", "private");
    admin.import_fixture(&db, "lichess.pgn");
    let again = |dir: std::path::PathBuf| chess_server::AppState::new(chess_server::config::Config::new(dir));
    let err = again(s.data_dir()).err().expect("refused");
    assert!(err.contains("another chess-server"), "{err}");
    admin.get(&format!("/api/databases/{db}/games")).ok();
    // A stopped server lets it go: the next one starts there.
    let s = s.restart();
    assert!(again(s.data_dir()).is_err());
    s.client().get("/api/health").ok();
}

/// Every file under `dir`.
fn all_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            all_files(&e.path(), out);
        } else {
            out.push(e.path());
        }
    }
}

#[test]
fn encrypted_backups_hold_nothing_readable_and_restore_with_their_key() {
    let backups = tempfile::tempdir().unwrap();
    let dir = backups.path().join("backups");
    // The key made as the documentation says, with the real binary.
    let key_file = backups.path().join("backup.key");
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_chess-server"))
        .args(["backup", "key"])
        .arg(&key_file)
        .output()
        .unwrap();
    assert!(cli.status.success(), "{}", String::from_utf8_lossy(&cli.stderr));
    let key = chess_server::crypt::BackupKey::from_file(&key_file).unwrap();
    let s = TestServer::with(|c| {
        c.backup_dir = Some(dir.clone());
        c.backup_key = Some(key.clone());
    });
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let private = alice.create_db("Alice's prep", "private");
    alice.import_fixture(&private, "reference.pgn");
    let games = |c: &common::Client, db: &str| c.get(&format!("/api/databases/{db}/export.pgn")).ok().body;
    let pgn = games(&alice, &private);
    assert!(pgn.contains("Carlsen"));

    let job = admin.post("/api/admin/backups", json!({})).ok().json()["job"].as_u64().unwrap();
    assert_eq!(admin.wait_job_admin(job), "done");
    let listed = admin.get("/api/admin/backups").ok().json();
    assert_eq!(listed["keyId"], key.id().as_str());
    let name = listed["backups"][0]["name"].as_str().unwrap().to_owned();
    assert_eq!(listed["backups"][0]["keyId"], key.id().as_str());
    assert_eq!(listed["backups"][0]["databases"], 1);
    let backup = dir.join(&name);

    // Nothing in it can be read: every file but the marker is encrypted,
    // and neither the games nor the store show through.
    let mut files = Vec::new();
    all_files(&backup, &mut files);
    assert!(files.len() > 5, "{files:?}");
    for f in &files {
        let n = f.file_name().unwrap().to_str().unwrap();
        assert!(n.ends_with(".enc") || n == "ENCRYPTED", "{n}");
        let bytes = std::fs::read(f).unwrap();
        for plain in [&b"Carlsen"[..], b"SQLite format 3", b"alice", b"argon2"] {
            assert!(
                !bytes.windows(plain.len()).any(|w| w == plain),
                "{n} holds {:?}",
                String::from_utf8_lossy(plain)
            );
        }
    }
    let tmp: Vec<_> = std::fs::read_dir(s.data_dir().join("tmp")).unwrap().collect();
    assert!(tmp.is_empty(), "no plain copy of the store left behind: {tmp:?}");

    // Another key can't, nor a backup changed since.
    let other = backups.path().join("other.key");
    chess_server::crypt::BackupKey::write_new(&other).unwrap();
    let out = backups.path().join("plain");
    let wrong = std::process::Command::new(env!("CARGO_BIN_EXE_chess-server"))
        .args(["backup", "decrypt"])
        .args([&other, &backup, &out])
        .output()
        .unwrap();
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("was made with key"), "{wrong:?}");
    assert!(!out.exists());
    let tampered = backups.path().join("tampered");
    copy_dir(&backup, &tampered);
    let blob = tampered.join("dbs").join(format!("{private}.chessdb")).join("games.blob.enc");
    let mut bytes = std::fs::read(&blob).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x40;
    std::fs::write(&blob, bytes).unwrap();
    assert!(chess_server::crypt::decrypt_backup(&key, &tampered, &backups.path().join("t-out")).is_err());
    assert!(!backups.path().join("t-out").exists());

    // With its key: the plain backup, restored as any other.
    let ok = std::process::Command::new(env!("CARGO_BIN_EXE_chess-server"))
        .args(["backup", "decrypt"])
        .args([&key_file, &backup, &out])
        .output()
        .unwrap();
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    let restored = tempfile::tempdir().unwrap();
    let data = restored.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::copy(out.join("server.sqlite"), data.join("server.sqlite")).unwrap();
    copy_dir(&out.join("dbs"), &data.join("dbs"));
    drop(s);
    let r = TestServer::start_in(restored, |_| {});
    let mut alice2 = r.client();
    alice2.login("alice", common::PASSWORD).ok();
    assert_eq!(games(&alice2, &private), pgn);
}
