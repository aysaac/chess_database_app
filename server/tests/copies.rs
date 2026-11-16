//! The desktop app's side of the server (NET-03, `chess_net::server`)
//! against a real server: signing in, the databases listed, and a local copy
//! of one taken in parts and kept up to date, up to a compaction there.

mod common;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chess_core::pgn::write_game;
use chess_db::{Database, GameUid, Progress};
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
    let copy = server::copy_into(&client, source, None, &local, &Cancel::default(), &|_| false, &mut |m| {
        marks.push(m)
    })
    .unwrap();
    assert_eq!(copy.import.imported, first as u64);
    assert_eq!(marks.iter().map(|m| m.next).collect::<Vec<_>>(), [PART_GAMES, PART_GAMES * 2, first as u32]);
    assert_eq!(copy.reached, CopyMark { next: first as u32, epoch: 0, seq: Some(0) });
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
    let again = server::copy_into(
        &client,
        source,
        Some(copy.reached),
        &local,
        &Cancel::default(),
        &|_| false,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!((again.import.imported, again.reached), (0, copy.reached));

    // New games on the server, one deleted there before the update: they
    // come, the deleted one marked as deleted.
    upload(&alice, &db, &many_games(5));
    let deleted = first as u32 + 1;
    alice.post(&format!("/api/databases/{db}/deleted"), json!({ "games": [deleted], "deleted": true })).ok();
    let update = server::copy_into(
        &client,
        source,
        Some(copy.reached),
        &local,
        &Cancel::default(),
        &|_| false,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(update.import.imported, 5);
    assert_eq!(update.reached.next, first as u32 + 5);
    assert_eq!(local.lock().unwrap().count(), first as u32 + 5);
    assert!(local.lock().unwrap().record(deleted).unwrap().is_deleted());
    // The deletion is in the journal, after the mark: the next update
    // looks at it again and finds nothing to change.
    let quiet = server::copy_into(
        &client,
        source,
        Some(update.reached),
        &local,
        &Cancel::default(),
        &|_| false,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!((quiet.import.imported, quiet.changed), (0, 0));
    let update = quiet;

    // A Cancel takes effect between parts: the parts before stay, noted.
    let tmp2 = tempfile::tempdir().unwrap();
    let partial = Mutex::new(Database::create(&tmp2.path().join("Partial")).unwrap());
    let cancel = Cancel::default();
    let mut noted = None;
    let stopped = server::copy_into(&client, source, None, &partial, &cancel, &|_| false, &mut |m| {
        noted = Some(m);
        cancel.0.store(true, Ordering::Relaxed);
    })
    .unwrap();
    assert!(stopped.cancelled);
    assert_eq!((stopped.import.imported, stopped.reached.next), (PART_GAMES as u64, PART_GAMES));
    assert_eq!(noted, Some(stopped.reached));
    let rest =
        server::copy_into(&client, source, noted, &partial, &Cancel::default(), &|_| false, &mut |_| {})
            .unwrap();
    assert_eq!(partial.lock().unwrap().count(), local.lock().unwrap().count());
    assert_eq!(rest.reached, update.reached);

    // A compaction there numbers the games again (the deleted game goes):
    // the copy finds where it got to by its games' permanent ids, and goes on.
    let r = alice.post(&format!("/api/databases/{db}/compact"), json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    alice.wait_job(r.json()["job"].as_u64().unwrap());
    upload(&alice, &db, &many_games(3));
    let after = server::copy_into(
        &client,
        source,
        Some(update.reached),
        &local,
        &Cancel::default(),
        &|_| false,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(after.reached.epoch, 1);
    assert_eq!(after.reached.next, first as u32 + 4 + 3);
    assert_eq!(after.import.imported, 3, "only the new games");
    assert_eq!(local.lock().unwrap().count(), first as u32 + 5 + 3);
    // A copy whose games are none of them there any more is made again.
    let stranger = tempfile::tempdir().unwrap();
    let other = Mutex::new(Database::create(&stranger.path().join("Other")).unwrap());
    let stale = CopyMark { next: 10, epoch: 0, seq: Some(0) };
    let r =
        server::copy_into(&client, source, Some(stale), &other, &Cancel::default(), &|_| false, &mut |_| {});
    assert!(matches!(r, Err(NetError::ServerCompacted)), "{r:?}");

    // A database that's gone, and signing out.
    let gone = CopySource { id: "nope", ..source };
    let r = server::copy_into(&client, gone, None, &local, &Cancel::default(), &|_| false, &mut |_| {});
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

const EDITED: &str = "[Event \"Edited\"]\n[White \"Kasparov, G.\"]\n[Black \"Karpov, A.\"]\n[Result \"1-0\"]\n\n\
                      1. d4 {A comment} d5 2. c4 e6 1-0\n";

/// The local copy's game that is game `uid` on the server.
fn local_of(db: &Mutex<Database>, uid: GameUid) -> u32 {
    *db.lock().unwrap().games_by_origin().unwrap().get(&uid).expect("in the copy")
}

/// White in the local copy's game that is game `uid` on the server.
fn white_of(db: &Mutex<Database>, uid: GameUid) -> String {
    let slot = local_of(db, uid);
    db.lock().unwrap().read_game(slot).unwrap().headers.white
}

#[test]
fn edits_and_deletions_follow_both_ways() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    s.user(&admin, "bob");
    let db = alice.create_db("Team", "shared");
    upload(&alice, &db, &many_games(20));
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "write" })).ok();

    let client = Client::new();
    let login = server::sign_in(&client, &s.base, "bob", PASSWORD, "Bob's laptop").unwrap();
    let source = CopySource { url: &s.base, token: &login.token, id: &db };
    let tmp = tempfile::tempdir().unwrap();
    let local = Mutex::new(Database::create(&tmp.path().join("Copy")).unwrap());
    let none = |_: GameUid| false;
    let copy =
        server::copy_into(&client, source, None, &local, &Cancel::default(), &none, &mut |_| {}).unwrap();
    let uid = |n: u32| -> GameUid {
        alice.get(&format!("/api/databases/{db}/games/{n}")).ok().json()["uid"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };

    // Edited, deleted and restored on the server: the copy follows.
    let (u3, u5, u7) = (uid(3), uid(5), uid(7));
    alice.put(&format!("/api/databases/{db}/by-uid/{u3}"), json!({ "pgn": EDITED })).ok();
    alice
        .post(
            &format!("/api/databases/{db}/deleted"),
            json!({ "uids": [u5.to_string(), u7.to_string()], "deleted": true }),
        )
        .ok();
    alice
        .post(&format!("/api/databases/{db}/deleted"), json!({ "uids": [u7.to_string()], "deleted": false }))
        .ok();
    let mut marks = Vec::new();
    let update =
        server::copy_into(&client, source, Some(copy.reached), &local, &Cancel::default(), &none, &mut |m| {
            marks.push(m)
        })
        .unwrap();
    assert_eq!((update.changed, update.import.imported), (2, 0), "game 7 is as it was");
    let latest = update.reached.seq.unwrap();
    assert!(latest > copy.reached.seq.unwrap());
    {
        let l = local.lock().unwrap();
        assert_eq!(l.read_game(3).unwrap().headers.white, "Kasparov, G.");
        assert!(l.record(5).unwrap().is_deleted());
        assert!(!l.record(7).unwrap().is_deleted());
    }

    // Edited here: sent back, and the server has it.
    let edit = |slot: u32, white: &str| {
        let mut l = local.lock().unwrap();
        let mut g = l.read_game(slot).unwrap();
        g.headers.white = white.to_owned();
        l.replace(slot, &g).unwrap();
    };
    let (u9, u11) = (uid(9), uid(11));
    edit(local_of(&local, u9), "Edited on the laptop");
    let pushed =
        server::push_edits(&client, source, &local, &[server::LocalEdit { uid: u9, base_seq: latest }])
            .unwrap();
    assert_eq!((pushed.sent, pushed.kept_as_new, pushed.done.as_slice()), (1, 0, [u9].as_slice()));
    assert!(pushed.stopped.is_none());
    let there = alice.get(&format!("/api/databases/{db}/by-uid/{u9}")).ok().json();
    assert_eq!(there["headers"]["white"], "Edited on the laptop");
    // The next update hears of it, and finds the copy has it already.
    let update = server::copy_into(
        &client,
        source,
        Some(update.reached),
        &local,
        &Cancel::default(),
        &none,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(update.changed, 0);

    // Edited on both sides: the server's edit stays, and the copy keeps
    // its own as a new game; until it's sent, an update leaves it alone.
    let base = update.reached.seq.unwrap();
    edit(local_of(&local, u11), "Laptop's version");
    alice.put(&format!("/api/databases/{db}/by-uid/{u11}"), json!({ "pgn": EDITED })).ok();
    let pending = |u: GameUid| u == u11;
    let kept = server::copy_into(
        &client,
        source,
        Some(update.reached),
        &local,
        &Cancel::default(),
        &pending,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!((kept.kept, kept.changed), (1, 0));
    assert_eq!(white_of(&local, u11), "Laptop's version");
    let count = local.lock().unwrap().count();
    let pushed =
        server::push_edits(&client, source, &local, &[server::LocalEdit { uid: u11, base_seq: base }])
            .unwrap();
    assert_eq!((pushed.sent, pushed.kept_as_new), (0, 1));
    let there = alice.get(&format!("/api/databases/{db}/by-uid/{u11}")).ok().json();
    assert_eq!(there["headers"]["white"], "Kasparov, G.", "the server's edit stays");
    {
        let mut l = local.lock().unwrap();
        assert_eq!(l.count(), count + 1);
        assert_eq!(l.read_game(count).unwrap().headers.white, "Laptop's version", "kept as a new game");
        assert_eq!(l.origin(count).unwrap(), None, "which isn't the server's");
    }
    assert_eq!(white_of(&local, u11), "Kasparov, G.");

    // A read-only account's edit isn't sent either, nor lost.
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "read" })).ok();
    let u13 = uid(13);
    edit(local_of(&local, u13), "Read-only edit");
    let pushed =
        server::push_edits(&client, source, &local, &[server::LocalEdit { uid: u13, base_seq: latest }])
            .unwrap();
    assert_eq!((pushed.sent, pushed.kept_as_new), (0, 1));
    assert_eq!(white_of(&local, u13), "White 13");
}
