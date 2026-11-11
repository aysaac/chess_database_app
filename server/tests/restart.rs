//! Jobs survive the server being killed: the real binary is SIGKILLed in the
//! middle of an import, started again on the same data directory, and the
//! import goes on from what it committed; the games end up those of a clean
//! import of the same file, none twice and none missing.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chess_core::pgn::write_game;
use chess_db::{Database, NoProgress, import_pgn};
use common::{Client, PASSWORD};
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_chess-server");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Server {
    child: Child,
    base: String,
}

impl Server {
    fn start(data: &Path, port: u16) -> Server {
        let child = Command::new(BIN)
            .args(["serve", "--data"])
            .arg(data)
            .args(["--bind", &format!("127.0.0.1:{port}")])
            .env("RUST_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the server");
        let base = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Client::new(&base).try_get("/api/health").is_none() {
            assert!(Instant::now() < deadline, "the server didn't start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Server { child, base }
    }

    fn admin(&self) -> Client {
        let mut c = Client::new(&self.base);
        c.login("admin", PASSWORD).ok();
        c
    }

    /// SIGKILL: no shutdown, nothing flushed or tidied.
    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A PGN of `n` short games, one in 997 skipped (a bad FEN).
fn many_games(path: &Path, n: usize) {
    let mut pgn = String::with_capacity(n * 80);
    for i in 0..n {
        if i % 997 == 13 {
            pgn.push_str("[Event \"bad\"]\n[SetUp \"1\"]\n[FEN \"nonsense\"]\n\n1. e4 *\n\n");
        } else {
            let moves = ["1. e4 e5 2. Nf3 Nc6 3. Bb5", "1. d4 d5 2. c4 e6", "1. c4 e5 2. Nc3"][i % 3];
            pgn.push_str(&format!(
                "[Event \"Club {}\"]\n[White \"W{i}\"]\n[Black \"B{}\"]\n\n{moves} *\n\n",
                i / 50,
                i * 7
            ));
        }
    }
    std::fs::write(path, pgn).unwrap();
}

fn uploads(data: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(data.join("tmp")).unwrap().map(|e| e.unwrap().path()).collect()
}

/// Kills the server once the import's first batches are in, before it ends.
fn kill_mid_import(server: Server, admin: &Client, db: &str, job: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let games = admin.get(&format!("/api/databases/{db}")).ok().json()["games"].as_u64().unwrap();
        let state = admin.get(&format!("/api/jobs/{job}")).ok().json()["state"].clone();
        assert_eq!(state, "running", "the import ended before it could be interrupted ({games} games)");
        if games > 0 {
            server.kill();
            return games;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_killed_import_goes_on_after_a_restart_with_no_game_twice_or_missing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let status = Command::new(BIN)
        .args(["--data"])
        .arg(&data)
        .args(["admin", "create", "admin"])
        .env("CHESS_SERVER_PASSWORD", PASSWORD)
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    let pgn = dir.path().join("many.pgn");
    many_games(&pgn, 150_000);
    let port = free_port();

    let server = Server::start(&data, port);
    let admin = server.admin();
    let db = admin.create_db("Many", "private");
    let r =
        admin.upload(&format!("/api/databases/{db}/import?filename=many.pgn"), &std::fs::read(&pgn).unwrap());
    assert_eq!(r.status, 202, "{}", r.body);
    let job = r.json()["job"].as_u64().unwrap();
    let at_kill = kill_mid_import(server, &admin, &db, job);
    assert_eq!(uploads(&data).len(), 1, "the upload stays for the import to go on with");

    // Started again: the same job goes on, and ends.
    let server = Server::start(&data, port);
    let admin = server.admin();
    let done = admin.wait_job(job);
    assert_eq!(done["state"], "done", "{done}");
    assert!(uploads(&data).is_empty(), "the upload goes once the import is done");
    let tmp = tempfile::tempdir().unwrap();
    let clean = Mutex::new(Database::create(&tmp.path().join("Clean")).unwrap());
    let summary = import_pgn(&clean, &pgn, &NoProgress).unwrap();
    let info = admin.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!(info["games"].as_u64(), Some(summary.imported), "killed at {at_kill} games");
    assert!(at_kill < summary.imported);
    assert_eq!(done["report"]["imported"].as_u64(), Some(summary.imported));
    assert!(info["index"]["upToDate"].as_bool().unwrap(), "{info}");
    let expected: String = {
        let db = clean.lock().unwrap();
        (0..db.count()).map(|id| write_game(&db.read_game(id).unwrap())).collect()
    };
    let exported = admin.get(&format!("/api/databases/{db}/export.pgn")).ok().body;
    assert!(exported == expected, "the games differ from a clean import's");

    // Killed again in another import: the admin cancels it once it goes on.
    let r = admin
        .upload(&format!("/api/databases/{db}/import?filename=again.pgn"), &std::fs::read(&pgn).unwrap());
    let again = r.json()["job"].as_u64().unwrap();
    kill_mid_import(server, &admin, &db, again);
    let server = Server::start(&data, port);
    let admin = server.admin();
    let listed = admin.get("/api/admin/jobs").ok().json();
    let row = listed.as_array().unwrap().iter().find(|j| j["id"] == again).unwrap().clone();
    assert_eq!(row["state"], "running", "{row}");
    admin.post(&format!("/api/admin/jobs/{again}/cancel"), json!({})).ok();
    assert_eq!(admin.wait_job(again)["state"], "cancelled");
    assert!(uploads(&data).is_empty(), "a cancelled import's upload goes");
    // The first job is still there, done, after all this.
    assert_eq!(admin.get(&format!("/api/jobs/{job}")).ok().json()["state"], "done");
}
