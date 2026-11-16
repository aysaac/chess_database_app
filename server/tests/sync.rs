//! Editing games on the server and the routes copies keep in step with
//! (NET-03, `routes/sync.rs`): games by permanent id, the journal of changed
//! games, parts of a copy, and finding games again after a compaction.

mod common;

use common::{TestServer, many_games};
use serde_json::{Value, json};

fn upload(c: &common::Client, db: &str, pgn: &[u8]) {
    let r = c.upload(&format!("/api/databases/{db}/import?filename=games.pgn"), pgn);
    assert_eq!(r.status, 202, "{}", r.body);
    let job = c.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
}

fn uid_of(c: &common::Client, db: &str, game: u32) -> String {
    c.get(&format!("/api/databases/{db}/games/{game}")).ok().json()["uid"].as_str().unwrap().to_owned()
}

fn changes(c: &common::Client, db: &str, since: u64) -> Value {
    c.get(&format!("/api/databases/{db}/sync/changes?since={since}")).ok().json()
}

const EDITED: &str = "[Event \"Edited\"]\n[White \"Kasparov, G.\"]\n[Black \"Karpov, A.\"]\n[Result \"1-0\"]\n\n\
                      1. d4 {A comment} d5 2. c4 (2. Nf3) e6 1-0\n";

#[test]
fn games_are_edited_by_permanent_id_and_the_journal_has_them() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let carol = s.user(&admin, "carol");
    let db = alice.create_db("Shared", "shared");
    upload(&alice, &db, &many_games(5));
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "write" })).ok();
    alice.put(&format!("/api/databases/{db}/shares/carol"), json!({ "access": "read" })).ok();

    let uid = uid_of(&alice, &db, 2);
    let by_uid = format!("/api/databases/{db}/by-uid/{uid}");
    let g = alice.get(&by_uid).ok().json();
    assert_eq!((g["id"].as_u64(), g["headers"]["white"].as_str()), (Some(2), Some("White 2")));
    let info = alice.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!(info["latestChange"], 0, "{info}");
    assert_eq!(changes(&alice, &db, 0)["games"], json!([]));

    // Who may: write access edits, read access doesn't, others see nothing.
    let edit = json!({ "pgn": EDITED, "baseSeq": 0 });
    let r = carol.put(&by_uid, edit.clone());
    assert_eq!(r.status, 403, "{}", r.body);
    let dave = s.user(&admin, "dave");
    assert_eq!(dave.put(&by_uid, edit.clone()).status, 404);
    assert_eq!(dave.get(&by_uid).status, 404);
    // Not a game, or not one game.
    let r = bob.put(&by_uid, json!({ "pgn": "" }));
    assert_eq!(r.status, 400, "{}", r.body);
    let r = bob.put(&by_uid, json!({ "pgn": format!("{EDITED}\n{EDITED}") }));
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(bob.put(&format!("/api/databases/{db}/by-uid/not-a-uid"), edit.clone()).status, 404);
    let other = "1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b";
    let r = bob.put(&format!("/api/databases/{db}/by-uid/{other}"), edit.clone());
    assert_eq!((r.status, r.code().as_str()), (404, "noSuchGame"));

    let saved = bob.put(&by_uid, edit.clone()).ok().json();
    assert_eq!((saved["id"].as_u64(), saved["uid"].as_str()), (Some(2), Some(uid.as_str())));
    let seq = saved["seq"].as_u64().unwrap();
    assert!(seq > 0);
    let g = carol.get(&by_uid).ok().json();
    assert_eq!(g["headers"]["white"], "Kasparov, G.");
    assert!(g["pgn"].as_str().unwrap().contains("{A comment}"), "{}", g["pgn"]);
    assert_eq!(uid_of(&carol, &db, 2), uid, "the permanent id stays with an edit");
    let row = carol.get(&format!("/api/databases/{db}/games?white=Kasparov")).ok().json();
    assert_eq!(row["total"], 1, "the list follows the edit: {row}");

    // An edit from a copy older than that change is refused, a newer one isn't.
    let r = alice.put(&by_uid, edit.clone());
    assert_eq!((r.status, r.code().as_str()), (409, "changedOnServer"), "{}", r.body);
    let r = alice.put(&by_uid, json!({ "pgn": EDITED, "baseSeq": seq, "deleted": true })).ok().json();
    let seq2 = r["seq"].as_u64().unwrap();
    assert!(seq2 > seq);
    // Without a base, it's written whatever happened.
    alice.put(&by_uid, json!({ "pgn": EDITED, "deleted": false })).ok();

    // The journal: each game once, in its state now.
    let c = changes(&carol, &db, 0);
    assert_eq!(c["games"].as_array().unwrap().len(), 1, "{c}");
    assert_eq!(c["games"][0]["uid"], uid.as_str());
    assert_eq!(c["games"][0]["game"]["deleted"], false);
    assert_eq!(c["games"][0]["game"]["id"], 2);
    assert!(!c["more"].as_bool().unwrap());
    let latest = c["latest"].as_u64().unwrap();
    assert!(latest > seq2);
    assert_eq!(changes(&carol, &db, latest)["games"], json!([]));
    assert_eq!(changes(&carol, &db, latest)["latest"].as_u64(), Some(latest));

    // Deleting by permanent id goes in the journal, games by number too;
    // a game already deleted isn't noted again.
    let (u0, u4) = (uid_of(&alice, &db, 0), uid_of(&alice, &db, 4));
    let r = bob.post(
        &format!("/api/databases/{db}/deleted"),
        json!({ "uids": [u0], "games": [4], "deleted": true }),
    );
    assert_eq!(r.ok().json()["changed"], 2);
    let c = changes(&carol, &db, latest);
    let uids: Vec<_> = c["games"].as_array().unwrap().iter().map(|g| g["uid"].as_str().unwrap()).collect();
    assert_eq!(uids.len(), 2, "{c}");
    assert!(uids.contains(&u0.as_str()) && uids.contains(&u4.as_str()));
    assert!(c["games"].as_array().unwrap().iter().all(|g| g["game"]["deleted"] == true));
    let after = c["latest"].as_u64().unwrap();
    bob.post(&format!("/api/databases/{db}/deleted"), json!({ "uids": [u0], "deleted": true })).ok();
    assert_eq!(changes(&carol, &db, after)["games"], json!([]));
    let r = bob.post(&format!("/api/databases/{db}/deleted"), json!({ "uids": [other], "deleted": true }));
    assert_eq!(r.status, 400);

    // In parts: at most `limit` at a time, with where to go on from.
    let page = carol.get(&format!("/api/databases/{db}/sync/changes?since=0&limit=1")).ok().json();
    assert_eq!(page["games"].as_array().unwrap().len(), 1);
    assert!(page["more"].as_bool().unwrap());
    let next =
        carol.get(&format!("/api/databases/{db}/sync/changes?since={}&limit=5", page["latest"])).ok().json();
    assert_eq!(next["games"].as_array().unwrap().len(), 2, "{next}");
    assert!(!next["more"].as_bool().unwrap());

    // Adding a game: at the end, with its own permanent id; not in the journal.
    let r = carol.post(&format!("/api/databases/{db}/games"), json!({ "pgn": EDITED }));
    assert_eq!(r.status, 403);
    let r = bob.post(&format!("/api/databases/{db}/games"), json!({ "pgn": EDITED }));
    assert_eq!(r.status, 201, "{}", r.body);
    let added = r.json();
    assert_eq!(added["id"], 5);
    assert_eq!(uid_of(&carol, &db, 5), added["uid"].as_str().unwrap());
    assert_eq!(changes(&carol, &db, after)["games"], json!([]));

    // The audit log says a game was edited or added, not which.
    let audit = admin.get("/api/admin/audit?action=games").ok().json();
    let actions: Vec<_> =
        audit["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
    assert!(actions.contains(&"games.edit") && actions.contains(&"games.add"), "{actions:?}");
    assert!(!audit.to_string().contains("Kasparov"));
}

#[test]
fn a_copy_takes_parts_and_finds_its_place_after_a_compaction() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Games", "private");
    upload(&alice, &db, &many_games(10));
    alice.post(&format!("/api/databases/{db}/deleted"), json!({ "games": [1, 3], "deleted": true })).ok();

    let part = alice.get(&format!("/api/databases/{db}/sync/games?idFrom=0&idTo=6&idEpoch=0")).ok().json();
    assert_eq!((part["epoch"].as_u64(), part["next"].as_u64()), (Some(0), Some(6)));
    let games = part["games"].as_array().unwrap();
    assert_eq!(games.len(), 6, "deleted games too");
    assert_eq!(games[1]["deleted"], true);
    assert_eq!(games[2]["deleted"], false);
    assert!(games[2]["pgn"].as_str().unwrap().contains("White 2"));
    let seq = part["seq"].as_u64().unwrap();
    assert!(seq > 0, "the deletions are in the journal");
    let uids: Vec<String> = games.iter().map(|g| g["uid"].as_str().unwrap().to_owned()).collect();
    let end = alice.get(&format!("/api/databases/{db}/sync/games?idFrom=6&idTo=100")).ok().json();
    assert_eq!((end["next"].as_u64(), end["games"].as_array().unwrap().len()), (Some(10), 4));
    let past = alice.get(&format!("/api/databases/{db}/sync/games?idFrom=10")).ok().json();
    assert_eq!((past["next"].as_u64(), past["games"].as_array().unwrap().len()), (Some(10), 0));

    let r = alice.post(&format!("/api/databases/{db}/compact"), json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(alice.wait_job(r.json()["job"].as_u64().unwrap())["state"], "done");

    // The old numbering is refused; the permanent ids say where games are now.
    let r = alice.get(&format!("/api/databases/{db}/sync/games?idFrom=6&idEpoch=0"));
    assert_eq!((r.status, r.code().as_str()), (409, "staleIds"));
    let found = alice.post(&format!("/api/databases/{db}/sync/locate"), json!({ "uids": uids })).ok().json();
    assert_eq!(found["epoch"], 1);
    assert_eq!(found["ids"], json!([0, null, 1, null, 2, 3]));
    let r = alice.post(&format!("/api/databases/{db}/sync/locate"), json!({ "uids": ["nope"] }));
    assert_eq!(r.status, 400);
    // The games removed come back as gone in the journal.
    let c = changes(&alice, &db, 0);
    let gone: Vec<_> = c["games"].as_array().unwrap().iter().filter(|g| g["game"].is_null()).collect();
    assert_eq!(gone.len(), 2, "{c}");
    assert_eq!(c["epoch"], 1);
    // An edit by permanent id works across it.
    let saved = alice.put(&format!("/api/databases/{db}/by-uid/{}", uids[5]), json!({ "pgn": EDITED })).ok();
    assert_eq!(saved.json()["id"], 3);
}
