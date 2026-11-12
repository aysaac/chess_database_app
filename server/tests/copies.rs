//! The desktop app's side of the server (NET-03, `chess_net::server`)
//! against a real server: signing in, the databases listed, and a local copy
//! of one taken in parts and kept up to date, up to a compaction there.

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chess_core::pgn::write_game;
use chess_db::{Database, Progress};
use chess_net::server::{self, CopyMark, CopySource, PART_GAMES};
use chess_net::{Client, NetError};
use common::{PASSWORD, TestServer, many_games};
use serde_json::json;

/// A progress that cancels once told to.
#[derive(Default)]
struct Cancel(AtomicBool);

impl Progress for Cancel {
    fn report(&self, _done: u64, _total: u64) {}
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn upload(c: &common::Client, db: &str, pgn: &[u8]) {
    let r = c.upload(&format!("/api/databases/{db}/import?filename=games.pgn"), pgn);
    assert_eq!(r.status, 202, "{}", r.body);
    let job = c.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
}

#[test]
fn a_copy_takes_the_games_in_parts_and_then_the_new_ones() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Alice's games", "private");
    let first = PART_GAMES as usize * 2 + 37;
    upload(&alice, &db, &many_games(first));

    let client = Client::new();
    let bad = server::sign_in(&client, &s.base, "alice", "wrong password", "Test");
    assert!(matches!(bad, Err(NetError::ServerLoginRefused(_))), "{bad:?}");
    let login = server::sign_in(&client, &s.base, "alice", PASSWORD, "Test computer").unwrap();
    assert_eq!(login.username, "alice");
    assert_eq!(server::whoami(&client, &s.base, &login.token).unwrap(), "alice");
    let tokens = alice.get("/api/tokens").ok().json();
    assert_eq!(tokens[0]["name"], "Test computer", "{tokens}");
    let listed = server::databases(&client, &s.base, &login.token).unwrap();
    let info = listed.iter().find(|d| d.id == db).expect("listed");
    assert_eq!(
        (info.name.as_str(), info.games, info.access.as_str()),
        ("Alice's games", first as u32, "owner")
    );

    // A new copy: every game, a part at a time, each noted.
    let tmp = tempfile::tempdir().unwrap();
    let local = Mutex::new(Database::create(&tmp.path().join("Copy")).unwrap());
    let source = CopySource { url: &s.base, token: &login.token, id: &db };
    let mut marks = Vec::new();
    let copy =
        server::copy_into(&client, source, None, &local, &Cancel::default(), &mut |m| marks.push(m)).unwrap();
    assert_eq!(copy.import.imported, first as u64);
    assert_eq!(marks.iter().map(|m| m.next).collect::<Vec<_>>(), [PART_GAMES, PART_GAMES * 2, first as u32]);
    assert_eq!(copy.reached, CopyMark { next: first as u32, epoch: 0 });
    assert!(copy.stopped.is_none() && !copy.cancelled);
    {
        let open = s.state.pool.get(&db).unwrap();
        let theirs = open.lock();
        let ours = local.lock().unwrap();
        assert_eq!(ours.count(), theirs.count());
        for id in [0, 1, PART_GAMES - 1, PART_GAMES, first as u32 - 1] {
            assert_eq!(write_game(&ours.read_game(id).unwrap()), write_game(&theirs.read_game(id).unwrap()));
        }
    }

    // Nothing new: nothing comes.
    let again =
        server::copy_into(&client, source, Some(copy.reached), &local, &Cancel::default(), &mut |_| {})
            .unwrap();
    assert_eq!((again.import.imported, again.reached), (0, copy.reached));

    // New games on the server, one deleted there before the update: the
    // others come, the deleted one doesn't.
    upload(&alice, &db, &many_games(5));
    let deleted = first as u32 + 1;
    alice.post(&format!("/api/databases/{db}/deleted"), json!({ "games": [deleted], "deleted": true })).ok();
    let update =
        server::copy_into(&client, source, Some(copy.reached), &local, &Cancel::default(), &mut |_| {})
            .unwrap();
    assert_eq!(update.import.imported, 4);
    assert_eq!(update.reached.next, first as u32 + 5);
    assert_eq!(local.lock().unwrap().count(), first as u32 + 4);

    // A Cancel takes effect between parts: the parts before stay, noted.
    let tmp2 = tempfile::tempdir().unwrap();
    let partial = Mutex::new(Database::create(&tmp2.path().join("Partial")).unwrap());
    let cancel = Cancel::default();
    let mut noted = None;
    let stopped = server::copy_into(&client, source, None, &partial, &cancel, &mut |m| {
        noted = Some(m);
        cancel.0.store(true, Ordering::Relaxed);
    })
    .unwrap();
    assert!(stopped.cancelled);
    assert_eq!((stopped.import.imported, stopped.reached.next), (PART_GAMES as u64, PART_GAMES));
    assert_eq!(noted, Some(stopped.reached));
    let rest = server::copy_into(&client, source, noted, &partial, &Cancel::default(), &mut |_| {}).unwrap();
    assert_eq!(partial.lock().unwrap().count(), local.lock().unwrap().count());
    assert_eq!(rest.reached, update.reached);

    // A compaction there numbers the games again: the copy can't go on.
    let r = alice.post(&format!("/api/databases/{db}/compact"), json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    alice.wait_job(r.json()["job"].as_u64().unwrap());
    let after =
        server::copy_into(&client, source, Some(update.reached), &local, &Cancel::default(), &mut |_| {});
    assert!(matches!(after, Err(NetError::ServerCompacted)), "{after:?}");

    // A database that's gone, and signing out.
    let gone = CopySource { id: "nope", ..source };
    let r = server::copy_into(&client, gone, None, &local, &Cancel::default(), &mut |_| {});
    assert!(matches!(r, Err(NetError::NoSuchServerDb)), "{r:?}");
    server::sign_out(&client, &s.base, &login.token, login.token_id).unwrap();
    let r = server::whoami(&client, &s.base, &login.token);
    assert!(matches!(r, Err(NetError::ServerSignInRejected)), "{r:?}");
    // Signing out again is fine: the token is gone already.
    server::sign_out(&client, &s.base, &login.token, login.token_id).unwrap();
}

#[test]
fn an_address_that_isnt_a_chess_server_says_so() {
    let client = Client::new();
    let r = server::sign_in(&client, "http://127.0.0.1:9", "a", "b", "Test");
    assert!(matches!(r, Err(NetError::ServerUnreachable { .. })), "{r:?}");
}
