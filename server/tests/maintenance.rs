//! Keeping a database in order on the server: games marked as deleted and
//! restored, duplicates found and deleted (DB-08), compaction (DB-06).

mod common;

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use chess_db::{Database, DuplicateOptions, NoProgress, find_duplicates, import_pgn};
use common::{TestServer, fixture, many_games};
use serde_json::json;

fn total(c: &common::Client, db: &str, params: &str) -> u64 {
    c.get(&format!("/api/databases/{db}/games?{params}")).ok().json()["total"].as_u64().unwrap()
}

#[test]
fn games_are_marked_deleted_and_restored() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let db = alice.create_db("Team", "shared");
    alice.import_fixture(&db, "reference.pgn");
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "read" })).ok();
    let path = format!("/api/databases/{db}/deleted");
    let r = alice.post(&path, json!({ "games": [0, 1, 2, 2], "deleted": true })).ok();
    assert_eq!(r.json()["changed"], 3);
    assert_eq!(alice.post(&path, json!({ "games": [1], "deleted": true })).ok().json()["changed"], 0);
    assert_eq!(total(&alice, &db, ""), 53);
    assert_eq!(total(&alice, &db, "deleted=show"), 56);
    assert_eq!(total(&alice, &db, "deleted=only"), 3);
    assert_eq!(total(&alice, &db, "deleted=only&sort=white"), 3);
    let rows = alice.get(&format!("/api/databases/{db}/games?deleted=only")).ok().json();
    assert!(rows["rows"].as_array().unwrap().iter().all(|r| r["deleted"] == true));
    // Reading isn't enough; ids past the end and stale epochs are refused.
    assert_eq!(bob.post(&path, json!({ "games": [3], "deleted": true })).status, 403);
    assert_eq!(alice.post(&path, json!({ "games": [56], "deleted": true })).status, 400);
    let r = alice.post(&path, json!({ "games": [3], "deleted": true, "idEpoch": 1 }));
    assert_eq!((r.status, r.code().as_str()), (409, "staleIds"));
    assert_eq!(
        alice.post(&path, json!({ "games": [0], "deleted": false, "idEpoch": 0 })).ok().json()["changed"],
        1
    );
    assert_eq!(total(&bob, &db, "deleted=only"), 2);
}

#[test]
fn duplicates_are_found_as_a_job_and_their_copies_deleted() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let db = alice.create_db("Copies", "shared");
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "read" })).ok();
    alice.import_fixture(&db, "duplicates.pgn");
    alice.import_fixture(&db, "reference.pgn");
    alice.import_fixture(&db, "reference.pgn");
    let tmp = tempfile::tempdir().unwrap();
    let local = Mutex::new(Database::create(&tmp.path().join("L")).unwrap());
    for f in ["duplicates.pgn", "reference.pgn", "reference.pgn"] {
        import_pgn(&local, &fixture(f), &NoProgress).unwrap();
    }
    for fuzzy in [false, true] {
        let want = find_duplicates(&local, DuplicateOptions { fuzzy }, &NoProgress).unwrap();
        let r = alice.post(&format!("/api/databases/{db}/duplicates"), json!({ "fuzzy": fuzzy }));
        assert_eq!(r.status, 202, "{}", r.body);
        let job = r.json()["job"].as_u64().unwrap();
        let done = alice.wait_job(job);
        assert_eq!(done["state"], "done", "{done}");
        let report = &done["duplicates"];
        assert_eq!(report["groups"].as_u64(), Some(want.groups.len() as u64), "{report}");
        assert_eq!(report["copies"].as_u64(), Some(want.removable() as u64));
        let page = alice.get(&format!("/api/databases/{db}/duplicates/{job}?limit=1000")).ok().json();
        let groups: Vec<(u64, Vec<u64>)> = page["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let remove = g["remove"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
                (g["keep"].as_u64().unwrap(), remove)
            })
            .collect();
        let expected: Vec<(u64, Vec<u64>)> = want
            .groups
            .iter()
            .map(|g| (u64::from(g.keep), g.remove.iter().map(|&x| u64::from(x)).collect()))
            .collect();
        assert_eq!(groups, expected, "fuzzy {fuzzy}");
        // Bob reads the database but didn't start the search: no report, no groups.
        assert!(bob.get(&format!("/api/jobs/{job}")).ok().json().get("duplicates").is_none());
        assert_eq!(bob.get(&format!("/api/databases/{db}/duplicates/{job}")).status, 403);
    }
    let r = alice.post(&format!("/api/databases/{db}/duplicates"), json!({}));
    let job = alice.wait_job(r.json()["job"].as_u64().unwrap())["id"].as_u64().unwrap();
    let copies = alice.get(&format!("/api/jobs/{job}")).ok().json()["duplicates"]["copies"].as_u64().unwrap();
    assert!(copies >= 56, "every reference game twice: {copies}");
    // Bob may read but not write.
    assert_eq!(bob.post(&format!("/api/databases/{db}/duplicates/{job}/delete"), json!({})).status, 403);
    let before = total(&alice, &db, "");
    let done = alice.post(&format!("/api/databases/{db}/duplicates/{job}/delete"), json!({})).ok().json();
    assert_eq!(done["marked"].as_u64(), Some(copies));
    assert_eq!(total(&alice, &db, ""), before - copies);
    // The database changed since: the same report can't delete again.
    assert_eq!(alice.post(&format!("/api/databases/{db}/duplicates/{job}/delete"), json!({})).status, 409);
    // A job of another kind, or of another database, isn't a report.
    let other = alice.create_db("Other", "private");
    assert_eq!(alice.get(&format!("/api/databases/{other}/duplicates/{job}")).status, 404);
}

#[test]
fn compaction_is_the_owners_renumbers_games_and_rebuilds_the_index() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let db = alice.create_db("Old", "shared");
    alice.import_fixture(&db, "reference.pgn");
    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "write" })).ok();
    let compact = format!("/api/databases/{db}/compact");
    assert_eq!(bob.post(&compact, json!({})).status, 403, "write access isn't enough");
    // A search cached before.
    let carlsen = total(&alice, &db, "white=Carlsen");
    let first =
        alice.get(&format!("/api/databases/{db}/games?limit=1&offset=5")).ok().json()["rows"][0].clone();
    alice
        .post(&format!("/api/databases/{db}/deleted"), json!({ "games": [0, 1, 2, 3, 4], "deleted": true }))
        .ok();

    // Refused while another write job runs on it.
    let open = s.state.pool.get(&db).unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let user = s.state.store.user_by_name("alice").unwrap().unwrap().0;
    let rx = Mutex::new(rx);
    let blocker = s.state.jobs.spawn("import", "Holding".into(), (&db, "Old"), &user, move |_| {
        let _ = rx.lock().unwrap().recv();
        Ok(String::new())
    });
    *open.writer.lock().unwrap() = Some(blocker);
    assert_eq!(alice.post(&compact, json!({})).status, 409);
    tx.send(()).unwrap();
    s.state.jobs.manager.wait(blocker);

    // While a compaction runs, requests answer "busy" rather than wait.
    open.compacting.store(true, Ordering::Release);
    let r = alice.get(&format!("/api/databases/{db}/games"));
    assert_eq!((r.status, r.code().as_str()), (503, "busy"));
    assert!(r.header("retry-after").is_some());
    assert!(alice.get("/api/databases").ok().json()[0]["games"].as_u64().is_some(), "the list still answers");
    open.compacting.store(false, Ordering::Release);
    drop(open);

    let r = alice.post(&compact, json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    let job = alice.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    let info = alice.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!((info["games"].as_u64(), info["idEpoch"].as_u64()), (Some(51), Some(1)));
    assert!(info["index"]["upToDate"].as_bool().unwrap(), "{info}");
    // Game 5 is game 0 now; the cached search is made again.
    let now = alice.get(&format!("/api/databases/{db}/games?limit=1")).ok().json()["rows"][0].clone();
    assert_eq!((now["white"].clone(), now["id"].as_u64()), (first["white"].clone(), Some(0)));
    assert!(total(&alice, &db, "white=Carlsen") <= carlsen);
    assert_eq!(total(&alice, &db, "deleted=only"), 0);
    assert_eq!(alice.get(&format!("/api/databases/{db}/tree")).ok().json()["total"]["games"], 51);
    // Ids read before are refused.
    let r = alice.post(
        &format!("/api/databases/{db}/deleted"),
        json!({ "games": [0], "deleted": true, "idEpoch": 0 }),
    );
    assert_eq!(r.status, 409);
}

/// A change taking ids that waited for the database while a compaction
/// ran (it checked the epoch before) is refused: the ids are other games
/// now.
#[test]
fn marking_games_after_a_compaction_refuses_the_old_ids() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Raced", "private");
    alice.import_fixture(&db, "reference.pgn");
    let path = format!("/api/databases/{db}/deleted");
    alice.post(&path, json!({ "games": [0], "deleted": true, "idEpoch": 0 })).ok();

    // The database held, as by a slow query; the request waits for it with epoch 0 checked.
    let open = s.state.pool.get(&db).unwrap();
    let mut held = open.lock();
    let (c, p) = (alice.same(), path.clone());
    let request =
        std::thread::spawn(move || c.post(&p, json!({ "games": [5], "deleted": true, "idEpoch": 0 })));
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Then a compaction takes it, as its job does: the epoch first, then the games renumbered.
    s.state.store.bump_id_epoch(&db).unwrap();
    held.compact(&NoProgress).unwrap();
    drop(held);
    let r = request.join().unwrap();
    assert_eq!((r.status, r.code().as_str()), (409, "staleIds"), "{}", r.body);
    assert_eq!(total(&alice, &db, "deleted=only"), 0, "no game marked by an old id");
}

/// An export a compaction comes in the middle of stops with an error: the
/// client gets a broken download, not a file mixing games from before and
/// after the renumbering.
#[test]
fn an_export_stops_when_a_compaction_renumbers_the_games() {
    use std::io::Read;
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Big", "private");
    let r = admin.upload(&format!("/api/databases/{db}/import?filename=many.pgn"), &many_games(30_000));
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(admin.wait_job(r.json()["job"].as_u64().unwrap())["state"], "done");
    admin.post(&format!("/api/databases/{db}/deleted"), json!({ "games": [0], "deleted": true })).ok();

    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let resp = agent
        .get(admin.url(&format!("/api/databases/{db}/export.pgn")))
        .header("Cookie", format!("chessdb_session={}", admin.cookie.as_deref().unwrap()))
        .call()
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let mut body = resp.into_body().into_with_config().limit(1 << 30).reader();
    let mut start = vec![0u8; 64 * 1024];
    body.read_exact(&mut start).unwrap();
    assert!(String::from_utf8_lossy(&start).contains("[White \"White 1\"]"), "game 0 is left out, deleted");

    let r = admin.post(&format!("/api/databases/{db}/compact"), json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    assert_eq!(admin.wait_job(r.json()["job"].as_u64().unwrap())["state"], "done");
    let mut rest = Vec::new();
    let read = body.read_to_end(&mut rest);
    assert!(read.is_err(), "the export went on across the compaction ({} more bytes)", rest.len());
    // What came before the error is games 1, 2, 3… with none skipped (game
    // n + 1 read by id n after the renumbering).
    start.extend_from_slice(&rest);
    let text = String::from_utf8_lossy(&start);
    let players: Vec<u32> =
        text.split("[White \"White ").skip(1).filter_map(|p| p.split('"').next()?.parse().ok()).collect();
    assert!(players.len() > 100, "{}", players.len());
    let skipped = players.iter().zip(1..).find(|&(&p, n)| p != n);
    assert!(skipped.is_none(), "(game sent, game due): {skipped:?}");
}
