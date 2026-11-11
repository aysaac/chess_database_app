//! Every permission rule of `docs/server.md` (Roles and visibility),
//! against a running server.

mod common;

use common::{PASSWORD, TestServer, fixture_bytes};
use serde_json::{Value, json};

/// Every route that reads or writes a database's games, for `db`.
fn game_routes(db: &str) -> Vec<String> {
    let start_after_e4 = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR%20b%20KQkq%20-%200%201";
    vec![
        format!("/api/databases/{db}"),
        format!("/api/databases/{db}/games"),
        format!("/api/databases/{db}/games?white=Carlsen"),
        format!("/api/databases/{db}/games?sort=white&desc=true"),
        format!("/api/databases/{db}/games?fen={start_after_e4}&white=a"),
        format!("/api/databases/{db}/games?deleted=only"),
        format!("/api/databases/{db}/games/0"),
        format!("/api/databases/{db}/tree"),
        format!("/api/databases/{db}/tree?fen={start_after_e4}"),
        format!("/api/databases/{db}/lines"),
        format!("/api/databases/{db}/shares"),
    ]
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap().to_owned()).collect()
}

fn user_id(admin: &common::Client, name: &str) -> i64 {
    let users = admin.get("/api/admin/users").ok().json();
    users.as_array().unwrap().iter().find(|u| u["username"] == name).unwrap()["id"].as_i64().unwrap()
}

#[test]
fn a_private_database_is_its_owners_alone_not_even_the_admins() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let db = alice.create_db("Prep", "private");
    let import = alice.import_fixture(&db, "reference.pgn");
    let job = import["id"].as_u64().unwrap();
    for path in game_routes(&db) {
        alice.get(&path).ok();
    }
    let pgn = fixture_bytes("reference.pgn");

    for (who, c) in [("bob", &bob), ("the admin", &admin)] {
        assert!(!ids(&c.get("/api/databases").ok().json()).contains(&db), "{who} lists it");
        for path in game_routes(&db) {
            let r = c.get(&path);
            assert_eq!((r.status, r.code().as_str()), (404, "notFound"), "{who}: {path}: {}", r.body);
        }
        let refused = [
            c.upload(&format!("/api/databases/{db}/import"), &pgn),
            c.patch(&format!("/api/databases/{db}"), json!({ "visibility": "server" })),
            c.patch(&format!("/api/databases/{db}"), json!({ "name": "Mine" })),
            c.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "write" })),
            c.delete(&format!("/api/databases/{db}?confirm=Prep")),
            c.get(&format!("/api/jobs/{job}")),
            c.post(&format!("/api/jobs/{job}/cancel"), json!({})),
        ];
        for r in refused {
            assert_eq!(r.status, 404, "{who}: {}", r.body);
        }
    }
    // Searching several databases: naming it fails the whole search as "not
    // found", with a database of one's own beside it too; all=true leaves it out.
    let theirs = |c: &common::Client, name: &str| c.create_db(name, "private");
    for (who, c) in [("bob", &bob), ("the admin", &admin)] {
        let own = theirs(c, &format!("{who}'s"));
        for path in [
            format!("/api/search?db={db}"),
            format!("/api/search?db={own}&db={db}"),
            format!("/api/search?db={db}&db={own}&white=a&fen=x&sort=nonsense"),
            format!("/api/databases/{db}/export.pgn"),
            format!("/api/databases/{db}/export.pgn?fen=x"),
        ] {
            let r = c.get(&path);
            assert_eq!((r.status, r.code().as_str()), (404, "notFound"), "{who}: {path}: {}", r.body);
        }
        let all = c.get("/api/search?all=true&limit=500").ok().json();
        let searched: Vec<&str> =
            all["databases"].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
        assert!(!searched.contains(&db.as_str()) && searched.contains(&own.as_str()), "{who}: {all}");
        assert!(!all.to_string().contains("Prep"), "{who}: {all}");
    }
    // Maintenance: deleting games, duplicates and compaction.
    let dups = alice.post(&format!("/api/databases/{db}/duplicates"), json!({})).ok().json()["job"]
        .as_u64()
        .unwrap();
    alice.wait_job(dups);
    for (who, c) in [("bob", &bob), ("the admin", &admin)] {
        for r in [
            c.post(&format!("/api/databases/{db}/deleted"), json!({ "games": [0], "deleted": true })),
            c.post(&format!("/api/databases/{db}/deleted"), json!({ "nonsense": 1 })),
            c.post(&format!("/api/databases/{db}/duplicates"), json!({ "fuzzy": true })),
            c.get(&format!("/api/databases/{db}/duplicates/{dups}")),
            c.post(&format!("/api/databases/{db}/duplicates/{dups}/delete"), json!({})),
            c.post(&format!("/api/databases/{db}/compact"), json!({})),
            c.get(&format!("/api/jobs/{dups}")),
        ] {
            assert_eq!((r.status, r.code().as_str()), (404, "notFound"), "{who}: {}", r.body);
        }
    }
    assert_eq!(alice.get(&format!("/api/databases/{db}/games?deleted=only")).ok().json()["total"], 0);
    // The admin's own import route is permission-checked the same way.
    std::fs::write(s.data_dir().join("import/more.pgn"), &pgn).unwrap();
    let r = admin.post(&format!("/api/databases/{db}/import-file"), json!({ "path": "more.pgn" }));
    assert_eq!(r.status, 404, "{}", r.body);

    // The admin sees its metadata, and nothing of its games.
    let all = admin.get("/api/admin/databases").ok().json();
    let row = all.as_array().unwrap().iter().find(|d| d["id"] == db).unwrap().clone();
    assert_eq!(
        (row["name"].as_str(), row["owner"].as_str(), row["games"].as_u64()),
        (Some("Prep"), Some("alice"), Some(56))
    );
    assert!(row["sizeBytes"].as_u64().unwrap() > 0);
    assert_eq!(row["visibility"], "private");
    let jobs = admin.get("/api/admin/jobs").ok().json();
    let j = jobs.as_array().unwrap().iter().find(|j| j["id"] == job).unwrap();
    assert!(j.get("label").is_none() && j.get("message").is_none() && j.get("report").is_none(), "{j}");

    // Nor can the admin change who reads it, or who owns it.
    let admin_id = user_id(&admin, "admin");
    for body in [
        json!({ "visibility": "server" }),
        json!({ "visibility": "shared" }),
        json!({ "owner": "admin" }),
        json!({ "ownerId": admin_id }),
        json!({ "quotaBytes": null, "visibility": "server" }),
    ] {
        let r = admin.patch(&format!("/api/admin/databases/{db}"), body.clone());
        assert!(r.status == 422 || r.status == 400, "{body}: {} {}", r.status, r.body);
    }
    let info = alice.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!((info["visibility"].as_str(), info["owner"].as_str()), (Some("private"), Some("alice")));
    assert_eq!(bob.get(&format!("/api/databases/{db}")).status, 404);
    // The quota is the admin's to set.
    admin.patch(&format!("/api/admin/databases/{db}"), json!({ "quotaBytes": 1_000_000 })).ok();
    assert_eq!(alice.get(&format!("/api/databases/{db}")).ok().json()["quotaBytes"], 1_000_000);
    // Only the owner sees the import's report.
    assert_eq!(alice.get(&format!("/api/jobs/{job}")).ok().json()["report"]["imported"], 56);
}

#[test]
fn shares_give_read_or_read_and_write() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let carol = s.user(&admin, "carol");
    let db = alice.create_db("Team", "shared");
    alice.import_fixture(&db, "reference.pgn");
    assert_eq!(bob.get(&format!("/api/databases/{db}")).status, 404, "shared with nobody yet");

    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "read" })).ok();
    let listed = bob.get("/api/databases").ok().json();
    let row = listed.as_array().unwrap().iter().find(|d| d["id"] == db).unwrap();
    assert_eq!(row["access"], "read");
    for path in game_routes(&db).iter().filter(|p| !p.ends_with("/shares")) {
        bob.get(path).ok();
    }
    let pgn = fixture_bytes("variations.pgn");
    let import = format!("/api/databases/{db}/import");
    for r in [
        bob.upload(&import, &pgn),
        bob.patch(&format!("/api/databases/{db}"), json!({ "name": "Bob's" })),
        bob.delete(&format!("/api/databases/{db}?confirm=Team")),
        bob.get(&format!("/api/databases/{db}/shares")),
        bob.put(&format!("/api/databases/{db}/shares/carol"), json!({ "access": "read" })),
    ] {
        assert_eq!(r.status, 403, "{}", r.body);
    }
    for path in game_routes(&db) {
        assert_eq!(carol.get(&path).status, 404, "{path}");
    }

    alice.put(&format!("/api/databases/{db}/shares/bob"), json!({ "access": "write" })).ok();
    let job = bob.import_fixture(&db, "variations.pgn")["id"].as_u64().unwrap();
    assert_eq!(alice.get(&format!("/api/databases/{db}")).ok().json()["games"], 58);
    // The owner sees the report of an import into their database.
    assert_eq!(alice.get(&format!("/api/jobs/{job}")).ok().json()["report"]["imported"], 2);
    assert_eq!(carol.get(&format!("/api/jobs/{job}")).status, 404);
    assert_eq!(
        alice.get(&format!("/api/databases/{db}/shares")).ok().json(),
        json!([{ "username": "bob", "access": "write" }])
    );

    // Private again: the share stays but gives nothing.
    alice.patch(&format!("/api/databases/{db}"), json!({ "visibility": "private" })).ok();
    assert_eq!(bob.get(&format!("/api/databases/{db}/games")).status, 404);
    assert!(!ids(&bob.get("/api/databases").ok().json()).contains(&db));
    // Their own import's job goes with it.
    assert_eq!(bob.get(&format!("/api/jobs/{job}")).status, 404);
    assert_eq!(bob.post(&format!("/api/jobs/{job}/cancel"), json!({})).status, 404);
    alice.patch(&format!("/api/databases/{db}"), json!({ "visibility": "shared" })).ok();
    assert!(bob.get(&format!("/api/jobs/{job}")).ok().json()["report"].is_object());
    bob.get(&format!("/api/databases/{db}/games")).ok();
    alice.delete(&format!("/api/databases/{db}/shares/bob")).ok();
    assert_eq!(bob.get(&format!("/api/databases/{db}/games")).status, 404);
    // Sharing with nobody, or oneself.
    assert_eq!(
        alice.put(&format!("/api/databases/{db}/shares/nobody"), json!({ "access": "read" })).status,
        404
    );
    assert_eq!(
        alice.put(&format!("/api/databases/{db}/shares/alice"), json!({ "access": "read" })).status,
        400
    );
}

#[test]
fn server_wide_databases_are_read_by_every_user_and_made_by_the_admin() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    assert_eq!(alice.post("/api/databases", json!({ "name": "Mine", "visibility": "server" })).status, 403);
    let own = alice.create_db("Mine", "private");
    assert_eq!(alice.patch(&format!("/api/databases/{own}"), json!({ "visibility": "server" })).status, 403);

    let db = admin.create_db("Reference", "server");
    admin.import_fixture(&db, "reference.pgn");
    for c in [&alice, &bob] {
        let listed = c.get("/api/databases").ok().json();
        let row = listed.as_array().unwrap().iter().find(|d| d["id"] == db).unwrap().clone();
        assert_eq!((row["access"].as_str(), row["owner"].as_str()), (Some("read"), Some("admin")));
        for path in game_routes(&db).iter().filter(|p| !p.ends_with("/shares")) {
            c.get(path).ok();
        }
        assert_eq!(c.upload(&format!("/api/databases/{db}/import"), &fixture_bytes("setup.pgn")).status, 403);
    }
    // A share can still give write.
    admin.put(&format!("/api/databases/{db}/shares/alice"), json!({ "access": "write" })).ok();
    alice.import_fixture(&db, "setup.pgn");
    assert_eq!(bob.upload(&format!("/api/databases/{db}/import"), &fixture_bytes("setup.pgn")).status, 403);
}

#[test]
fn a_disabled_users_sessions_and_tokens_stop_working() {
    let s = TestServer::start();
    let admin = s.admin();
    let bob = s.user(&admin, "bob");
    let made = bob.post("/api/tokens", json!({ "name": "laptop" }));
    assert_eq!(made.status, 201, "{}", made.body);
    let token = made.json()["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("cdb_"));
    let api = bob.with_bearer(&token);
    assert_eq!(api.get("/api/me").ok().json()["username"], "bob");
    let db = api.create_db("Via token", "private");
    api.get(&format!("/api/databases/{db}/games")).ok();

    let id = user_id(&admin, "bob");
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "disabled": true })).ok();
    for c in [&api, &bob] {
        assert_eq!(c.get("/api/me").status, 401);
        assert_eq!(c.get(&format!("/api/databases/{db}/games")).status, 401);
    }
    let mut again = s.client();
    assert_eq!(again.login("bob", PASSWORD).status, 403);
    // The data stays, and the account works again once enabled (with a new session).
    let all = admin.get("/api/admin/databases").ok().json();
    assert!(ids(&all).contains(&db));
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "disabled": false })).ok();
    api.get(&format!("/api/databases/{db}/games")).ok();
    assert_eq!(bob.get("/api/me").status, 401, "disabling ended the session");
    again.login("bob", PASSWORD).ok();
    // A deleted token stops too.
    let tokens = again.get("/api/tokens").ok().json();
    let tid = tokens[0]["id"].as_i64().unwrap();
    assert!(tokens[0]["lastUsedAt"].is_number());
    again.delete(&format!("/api/tokens/{tid}")).ok();
    assert_eq!(api.get("/api/me").status, 401);
}

#[test]
fn browser_sessions_need_the_csrf_token_and_the_same_origin() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let mut no_csrf = alice.same();
    no_csrf.csrf = None;
    assert_eq!(no_csrf.post("/api/databases", json!({ "name": "x" })).status, 403);
    no_csrf.csrf = Some("0".repeat(64));
    assert_eq!(no_csrf.post("/api/databases", json!({ "name": "x" })).status, 403);
    no_csrf.get("/api/databases").ok();
    // A token isn't sent by a browser on its own: no CSRF token needed.
    let token = alice.post("/api/tokens", json!({ "name": "script" })).ok().json()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let api = alice.with_bearer(&token);
    assert_eq!(api.post("/api/databases", json!({ "name": "x" })).status, 201);
    // Another site's page can't post, even with the token.
    let mut evil = alice.same();
    evil.origin = Some("http://evil.example".into());
    assert_eq!(evil.post("/api/databases", json!({ "name": "x" })).status, 403);
    let mut same = alice.same();
    same.origin = Some(s.base.clone());
    assert_eq!(same.post("/api/databases", json!({ "name": "y" })).status, 201);
    // Logging out ends the session.
    alice.post("/api/logout", json!({})).ok();
    assert_eq!(alice.get("/api/me").status, 401);
}

#[test]
fn failed_logins_are_throttled() {
    let s = TestServer::with(|c| c.login_attempts = 3);
    let admin = s.admin();
    admin.post("/api/admin/users", json!({ "username": "bob", "password": PASSWORD })).ok();
    let mut c = s.client();
    for _ in 0..3 {
        assert_eq!(c.login("bob", "wrong password").status, 401);
    }
    let r = c.login("bob", PASSWORD);
    assert_eq!(r.status, 429, "{}", r.body);
    assert!(r.header("retry-after").is_some());
    // The address is throttled too: guessing other accounts from it waits.
    assert_eq!(c.login("admin", PASSWORD).status, 429);
    // Unknown names fail the same way as wrong passwords.
    let s = TestServer::start();
    s.admin();
    assert_eq!(s.client().login("nobody", PASSWORD).code(), "badLogin");
}

#[test]
fn the_setup_token_makes_the_first_admin_once() {
    let s = TestServer::start();
    let c = s.client();
    assert_eq!(c.get("/api/setup").ok().json()["needed"], true);
    let token = s.state.setup_token().unwrap();
    let setup =
        |t: &str, pw: &str| c.post("/api/setup", json!({ "token": t, "username": "root", "password": pw }));
    assert_eq!(setup("wrong", PASSWORD).status, 403);
    assert_eq!(setup(&token, "short").status, 400);
    assert_eq!(setup(&token, PASSWORD).status, 201);
    assert_eq!(setup(&token, PASSWORD).status, 409);
    assert_eq!(c.get("/api/setup").ok().json()["needed"], false);
    let mut c = s.client();
    assert_eq!(c.login("root", PASSWORD).ok().json()["role"], "admin");
    // Anything else needs a login.
    assert_eq!(s.client().get("/api/databases").status, 401);
}

#[test]
fn admin_routes_are_the_admins_and_keep_an_admin() {
    let s = TestServer::start();
    let admin = s.admin();
    let mut bob = s.user(&admin, "bob");
    for path in [
        "/api/admin/users",
        "/api/admin/databases",
        "/api/admin/jobs",
        "/api/admin/disk",
        "/api/admin/import-files",
    ] {
        assert_eq!(bob.get(path).status, 403, "{path}");
    }
    assert_eq!(bob.post("/api/admin/users", json!({ "username": "eve", "password": PASSWORD })).status, 403);
    let me = user_id(&admin, "admin");
    assert_eq!(admin.patch(&format!("/api/admin/users/{me}"), json!({ "disabled": true })).status, 409);
    assert_eq!(admin.patch(&format!("/api/admin/users/{me}"), json!({ "role": "user" })).status, 409);
    assert_eq!(
        admin.post("/api/admin/users", json!({ "username": "BOB", "password": PASSWORD })).status,
        409
    );
    assert_eq!(
        admin.post("/api/admin/users", json!({ "username": "../x", "password": PASSWORD })).status,
        400
    );

    // A new password ends the user's sessions.
    let id = user_id(&admin, "bob");
    admin.post(&format!("/api/admin/users/{id}/password"), json!({ "password": "another password" })).ok();
    assert_eq!(bob.get("/api/me").status, 401);
    assert_eq!(bob.login("bob", PASSWORD).status, 401);
    bob.login("bob", "another password").ok();
    // Their own password change keeps their session.
    bob.post("/api/me/password", json!({ "currentPassword": "another password", "newPassword": PASSWORD }))
        .ok();
    bob.get("/api/me").ok();
    let r = bob.post("/api/me/password", json!({ "currentPassword": "wrong", "newPassword": PASSWORD }));
    assert_eq!(r.status, 403);

    // Promoted, bob is an admin; the first admin can then be demoted.
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "role": "admin" })).ok();
    bob.get("/api/admin/users").ok();
    bob.patch(&format!("/api/admin/users/{me}"), json!({ "role": "user" })).ok();
    assert_eq!(admin.get("/api/admin/users").status, 403);
}

#[test]
fn deleting_a_database_needs_its_name() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Old games", "private");
    alice.import_fixture(&db, "reference.pgn");
    let folder = s.data_dir().join("dbs").join(format!("{db}.chessdb"));
    assert!(folder.is_dir());
    assert_eq!(alice.delete(&format!("/api/databases/{db}")).status, 400);
    assert_eq!(alice.delete(&format!("/api/databases/{db}?confirm=Old")).status, 400);
    alice.delete(&format!("/api/databases/{db}?confirm=Old%20games")).ok();
    assert!(!folder.exists());
    assert_eq!(alice.get(&format!("/api/databases/{db}")).status, 404);

    // The admin deletes any database, confirmed the same way.
    let db = alice.create_db("Secret", "private");
    assert_eq!(admin.delete(&format!("/api/admin/databases/{db}")).status, 400);
    admin.delete(&format!("/api/admin/databases/{db}?confirm=Secret")).ok();
    assert!(alice.get("/api/databases").ok().json().as_array().unwrap().is_empty());
}
