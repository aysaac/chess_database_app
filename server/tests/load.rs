//! Many users at once (NET-03): reads in parallel on one database, edits of
//! one game racing each other (exactly one wins each round, the others are
//! told it changed), and copies kept in step while others edit. Release
//! builds and real data are measured in `docs/server.md` (Measured); this
//! checks correctness under concurrency, on the test server.

mod common;

use std::collections::HashMap;
use std::sync::{Barrier, Mutex};

use chess_core::pgn::write_game;
use chess_db::{Database, NoProgress};
use chess_net::server::{self, CopySource};
use common::{PASSWORD, TestServer, many_games};
use serde_json::json;

const USERS: usize = 16;

fn upload(c: &common::Client, db: &str, pgn: &[u8]) {
    let r = c.upload(&format!("/api/databases/{db}/import?filename=games.pgn"), pgn);
    assert_eq!(r.status, 202, "{}", r.body);
    let job = c.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
}

fn edited(by: usize, round: usize) -> String {
    format!(
        "[Event \"Round {round}\"]\n[White \"User {by}\"]\n[Black \"Someone\"]\n[Result \"*\"]\n\n1. e4 e5 2. Nf3 *\n"
    )
}

#[test]
fn many_users_read_edit_and_copy_at_once() {
    let s = TestServer::start();
    let admin = s.admin();
    let team = admin.create_db("Team", "server");
    upload(&admin, &team, &many_games(1500));
    let users: Vec<common::Client> = (0..USERS)
        .map(|i| {
            let name = format!("user{i}");
            let c = s.user(&admin, &name);
            admin.put(&format!("/api/databases/{team}/shares/{name}"), json!({ "access": "write" })).ok();
            c
        })
        .collect();
    let uid_of = |n: u32| -> String {
        admin.get(&format!("/api/databases/{team}/games/{n}")).ok().json()["uid"].as_str().unwrap().to_owned()
    };
    let contested = uid_of(7);
    let errors = Mutex::new(Vec::<String>::new());

    // 1. Reads, all at once: every one answers, with the same answers.
    let barrier = Barrier::new(USERS);
    let answers: Vec<(u64, String, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = users
            .iter()
            .map(|c| {
                let (barrier, team) = (&barrier, &team);
                scope.spawn(move || {
                    barrier.wait();
                    let mut last = (0, String::new(), String::new());
                    for i in 0..5 {
                        let page = c.get(&format!("/api/databases/{team}/games?sort=white&offset={}&limit=50", i * 50));
                        let tree = c.get(&format!("/api/databases/{team}/tree?fen=rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR%20b%20KQkq%20-%200%201"));
                        let game = c.get(&format!("/api/databases/{team}/games/{}", i * 100));
                        let list = c.get("/api/databases");
                        for r in [&page, &tree, &game, &list] {
                            assert!((200..300).contains(&r.status), "{} {}", r.status, r.body);
                        }
                        last = (page.json()["total"].as_u64().unwrap(), tree.json()["moves"].to_string(), game.body.clone());
                    }
                    last
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(answers.iter().all(|a| *a == answers[0]), "every user saw the same");
    assert_eq!(answers[0].0, 1500);

    // 2. Edits of one game racing: each round every user sends one from the
    // same base, and exactly one is taken.
    for round in 0..4 {
        let base = admin.get(&format!("/api/databases/{team}")).ok().json()["latestChange"].as_u64().unwrap();
        let barrier = Barrier::new(USERS);
        let statuses: Vec<(usize, u16, String)> = std::thread::scope(|scope| {
            let handles: Vec<_> = users
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let (barrier, team, contested) = (&barrier, &team, &contested);
                    scope.spawn(move || {
                        barrier.wait();
                        let r = c.put(
                            &format!("/api/databases/{team}/by-uid/{contested}"),
                            json!({ "pgn": edited(i, round), "baseSeq": base }),
                        );
                        (i, r.status, if r.status == 200 { String::new() } else { r.code() })
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let won: Vec<_> = statuses.iter().filter(|(_, st, _)| *st == 200).collect();
        assert_eq!(won.len(), 1, "round {round}: {statuses:?}");
        assert!(
            statuses.iter().all(|(_, st, code)| *st == 200 || (*st == 409 && code == "changedOnServer")),
            "round {round}: {statuses:?}"
        );
        let there = admin.get(&format!("/api/databases/{team}/by-uid/{contested}")).ok().json();
        assert_eq!(
            there["headers"]["white"],
            format!("User {}", won[0].0),
            "the winner's edit is the one kept"
        );
    }

    // 3. Copies kept in step while others edit, delete and add: after a
    // last update each copy is the server's database, game for game.
    let client = chess_net::Client::new();
    let logins: Vec<_> = (0..4)
        .map(|i| server::sign_in(&client, &s.base, &format!("user{i}"), PASSWORD, "load test").unwrap())
        .collect();
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let copies: Vec<Mutex<Database>> =
        dirs.iter().map(|d| Mutex::new(Database::create(&d.path().join("Copy")).unwrap())).collect();
    let uids: Vec<String> = (0..60).map(|n| uid_of(n * 20)).collect();
    let base_url = s.base.as_str();
    let marks: Vec<_> = std::thread::scope(|scope| {
        let copiers: Vec<_> = (0..4)
            .map(|i| {
                let (client, login, db, team, errors) = (&client, &logins[i], &copies[i], &team, &errors);
                scope.spawn(move || {
                    let source = CopySource { url: base_url, token: &login.token, id: team };
                    let mut mark = None;
                    for _ in 0..6 {
                        match server::copy_into(
                            client,
                            source,
                            mark,
                            db,
                            &NoProgress,
                            &|_| false,
                            &mut |_| {},
                        ) {
                            Ok(c) => {
                                if let Some(e) = c.stopped {
                                    errors.lock().unwrap().push(format!("copy {i} stopped: {e}"));
                                }
                                mark = Some(c.reached);
                            }
                            Err(e) => errors.lock().unwrap().push(format!("copy {i}: {e}")),
                        }
                    }
                    mark
                })
            })
            .collect();
        let editors: Vec<_> = users[4..]
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (team, uids, errors) = (&team, &uids, &errors);
                scope.spawn(move || {
                    for k in 0..5 {
                        let uid = &uids[(i * 5 + k) % uids.len()];
                        let r = match k % 3 {
                            0 => c.put(
                                &format!("/api/databases/{team}/by-uid/{uid}"),
                                json!({ "pgn": edited(i, k) }),
                            ),
                            1 => c.post(
                                &format!("/api/databases/{team}/deleted"),
                                json!({ "uids": [uid], "deleted": k % 2 == 1 }),
                            ),
                            _ => c.post(
                                &format!("/api/databases/{team}/games"),
                                json!({ "pgn": edited(i, k) }),
                            ),
                        };
                        // Adding is refused while an index build writes: that's allowed.
                        if !(200..300).contains(&r.status) && !(r.status == 409 && k % 3 == 2) {
                            errors.lock().unwrap().push(format!("editor {i}: {} {}", r.status, r.body));
                        }
                    }
                })
            })
            .collect();
        for e in editors {
            e.join().unwrap();
        }
        copiers.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let errors = errors.into_inner().unwrap();
    assert!(errors.is_empty(), "{errors:#?}");

    let open = s.state.pool.get(&team).unwrap();
    let theirs: HashMap<String, (String, bool)> = {
        let mut db = open.lock();
        let _ = db.game_stats();
        (0..db.count())
            .map(|g| {
                let state = (write_game(&db.read_game(g).unwrap()), db.record(g).unwrap().is_deleted());
                (db.uid(g).unwrap().to_string(), state)
            })
            .collect()
    };
    drop(open);
    for (i, (db, mark)) in copies.iter().zip(marks).enumerate() {
        let source = CopySource { url: &s.base, token: &logins[i].token, id: &team };
        server::copy_into(&client, source, mark, db, &NoProgress, &|_| false, &mut |_| {}).unwrap();
        let db = db.lock().unwrap();
        let by_origin = db.games_by_origin().unwrap();
        assert_eq!(by_origin.len(), theirs.len(), "copy {i} has every game once");
        for (uid, slot) in by_origin {
            let ours = (write_game(&db.read_game(slot).unwrap()), db.record(slot).unwrap().is_deleted());
            assert_eq!(Some(&ours), theirs.get(&uid.to_string()), "copy {i}, game {uid}");
        }
    }
}
