//! Accounts beyond phase 1: deleting users, transferring databases,
//! invites and open sign-up, reset links, e-mail and "Forgot password"
//! (against a stand-in SMTP server on 127.0.0.1).

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use chess_server::config::{SmtpConfig, SmtpTls};
use chess_server::secret::token_digest;
use chess_server::store::now;
use chess_server::wire::Role;
use common::{PASSWORD, TestServer};
use serde_json::json;

fn user_id(admin: &common::Client, name: &str) -> i64 {
    let users = admin.get("/api/admin/users").ok().json();
    users.as_array().unwrap().iter().find(|u| u["username"] == name).unwrap()["id"].as_i64().unwrap()
}

#[test]
fn deleting_a_user_deletes_their_databases_and_ends_everything_of_theirs() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let mine = alice.create_db("Alice's prep", "private");
    alice.import_fixture(&mine, "reference.pgn");
    let team = alice.create_db("Team", "shared");
    alice.put(&format!("/api/databases/{team}/shares/bob"), json!({ "access": "read" })).ok();
    let bobs = bob.create_db("Bob's", "shared");
    bob.put(&format!("/api/databases/{bobs}/shares/alice"), json!({ "access": "write" })).ok();
    bob.put(&format!("/api/databases/{bobs}/transfer"), json!({ "username": "alice" })).ok();
    let token =
        alice.post("/api/tokens", json!({ "name": "t" })).ok().json()["token"].as_str().unwrap().to_owned();
    let api = alice.with_bearer(&token);
    let id = user_id(&admin, "alice");

    // The admin sees what goes with them: names and sizes only.
    let dbs = admin.get(&format!("/api/admin/users/{id}/databases")).ok().json();
    let names: Vec<&str> = dbs.as_array().unwrap().iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"Alice's prep") && names.contains(&"Team"));
    assert!(
        dbs.as_array()
            .unwrap()
            .iter()
            .all(|d| d["sizeBytes"].as_u64().unwrap() > 0 && d.get("rows").is_none())
    );
    assert_eq!(bob.get(&format!("/api/admin/users/{id}/databases")).status, 403);

    assert_eq!(admin.delete(&format!("/api/admin/users/{id}")).status, 400);
    assert_eq!(admin.delete(&format!("/api/admin/users/{id}?confirm=bob")).status, 400);
    let me = user_id(&admin, "admin");
    assert_eq!(admin.delete(&format!("/api/admin/users/{me}?confirm=admin")).status, 409);
    admin.delete(&format!("/api/admin/users/{id}?confirm=alice")).ok();

    for path in ["/api/me", "/api/databases"] {
        assert_eq!(alice.get(path).status, 401);
        assert_eq!(api.get(path).status, 401);
    }
    assert_eq!(s.client().login("alice", PASSWORD).status, 401);
    for db in [&mine, &team] {
        assert!(!s.data_dir().join("dbs").join(format!("{db}.chessdb")).exists());
        assert_eq!(bob.get(&format!("/api/databases/{db}")).status, 404);
    }
    assert!(!admin.get("/api/admin/users").ok().body.contains("alice"));
    assert_eq!(bob.get(&format!("/api/databases/{bobs}/shares")).ok().json(), json!([]));
    assert_eq!(bob.get("/api/transfers").ok().json()["outgoing"], json!([]));
    // The name is free again, for a new account that gets nothing of hers.
    let again = s.user(&admin, "alice");
    assert!(
        again.get("/api/databases").ok().json().as_array().unwrap().iter().all(|d| d["id"] != mine.as_str())
    );
}

#[test]
fn ownership_is_offered_by_the_owner_and_accepted_by_the_recipient() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let bob = s.user(&admin, "bob");
    let carol = s.user(&admin, "carol");
    let db = alice.create_db("Gift", "private");
    alice.import_fixture(&db, "reference.pgn");
    let path = format!("/api/databases/{db}/transfer");
    // Only the owner offers; the admin can't move someone else's database.
    for c in [&bob, &admin] {
        assert_eq!(c.put(&path, json!({ "username": "bob" })).status, 404);
    }
    assert_eq!(alice.put(&path, json!({ "username": "nobody" })).code(), "noSuchUser");
    assert_eq!(alice.put(&path, json!({ "username": "alice" })).status, 400);
    let dave = s.user(&admin, "dave");
    admin.patch(&format!("/api/admin/users/{}", user_id(&admin, "dave")), json!({ "disabled": true })).ok();
    drop(dave);
    assert_eq!(alice.put(&path, json!({ "username": "dave" })).code(), "noSuchUser");
    alice.put(&path, json!({ "username": "bob" })).ok();

    let offers = bob.get("/api/transfers").ok().json();
    assert_eq!(offers["incoming"][0]["name"], "Gift");
    assert_eq!(offers["incoming"][0]["from"], "alice");
    assert_eq!(alice.get("/api/transfers").ok().json()["outgoing"][0]["to"], "bob");
    assert_eq!(carol.get("/api/transfers").ok().json(), json!({ "incoming": [], "outgoing": [] }));
    // Until accepted, bob reads nothing; nobody else can accept it.
    assert_eq!(bob.get(&format!("/api/databases/{db}/games")).status, 404);
    for c in [&carol, &admin] {
        assert_eq!(c.post(&format!("/api/transfers/{db}/accept"), json!({})).status, 404);
    }
    // It must fit in the recipient's quota.
    let bob_id = user_id(&admin, "bob");
    admin.patch(&format!("/api/admin/users/{bob_id}"), json!({ "quotaBytes": 1000 })).ok();
    let r = bob.post(&format!("/api/transfers/{db}/accept"), json!({}));
    assert_eq!((r.status, r.code().as_str()), (507, "quotaExceeded"));
    admin.patch(&format!("/api/admin/users/{bob_id}"), json!({ "quotaBytes": null })).ok();
    bob.post(&format!("/api/transfers/{db}/accept"), json!({})).ok();
    let info = bob.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!((info["owner"].as_str(), info["access"].as_str()), (Some("bob"), Some("owner")));
    assert_eq!(info["visibility"], "private");
    for path in [format!("/api/databases/{db}"), format!("/api/databases/{db}/games")] {
        assert_eq!(alice.get(&path).status, 404, "the old owner keeps nothing");
    }
    assert_eq!(bob.post(&format!("/api/transfers/{db}/accept"), json!({})).status, 404, "used up");
    // Declining and withdrawing.
    bob.put(&path, json!({ "username": "carol" })).ok();
    carol.post(&format!("/api/transfers/{db}/decline"), json!({})).ok();
    assert_eq!(carol.post(&format!("/api/transfers/{db}/accept"), json!({})).status, 404);
    bob.put(&path, json!({ "username": "carol" })).ok();
    bob.delete(&path).ok();
    assert_eq!(carol.post(&format!("/api/transfers/{db}/accept"), json!({})).status, 404);
    assert_eq!(bob.get(&format!("/api/databases/{db}")).ok().json()["owner"], "bob");
}

#[test]
fn invites_make_one_account_each_and_open_sign_up_is_off_by_default() {
    let s = TestServer::start();
    let admin = s.admin();
    let link = admin.post("/api/admin/invites", json!({ "quotaBytes": 5_000_000, "days": 2 })).ok().json();
    let token = link["token"].as_str().unwrap().to_owned();
    assert_eq!(link["url"].as_str().unwrap(), format!("/#/invite/{token}"));
    assert!(link["expiresAt"].as_i64().unwrap() - now() > 2 * 24 * 3600 - 60);
    let c = s.client();
    assert_eq!(c.get(&format!("/api/invites/{token}")).ok().json()["role"], "user");
    assert_eq!(c.get(&format!("/api/invites/{}", "0".repeat(64))).code(), "badInvite");
    let signup = |c: &common::Client, invite: Option<&str>, name: &str| {
        c.post("/api/signup", json!({ "invite": invite, "username": name, "password": PASSWORD }))
    };
    assert_eq!(signup(&c, None, "eve").status, 403, "no open sign-up");
    let r = signup(&c, Some(&token), "erin");
    assert_eq!(r.status, 201, "{}", r.body);
    assert!(r.header("set-cookie").unwrap().starts_with("chessdb_session="), "logged in");
    assert_eq!(r.json()["role"], "user");
    let erin = admin.get("/api/admin/users").ok().json();
    let erin = erin.as_array().unwrap().iter().find(|u| u["username"] == "erin").unwrap().clone();
    assert_eq!(erin["quotaBytes"], 5_000_000);
    assert_eq!(signup(&c, Some(&token), "frank").code(), "badInvite", "single use");
    let listed = admin.get("/api/admin/invites").ok().json();
    assert_eq!(listed[0]["usedBy"], "erin");
    assert!(!listed.to_string().contains(&token), "tokens are never shown again");
    // An admin invite; an expired one; a withdrawn one.
    let t = admin.post("/api/admin/invites", json!({ "role": "admin" })).ok().json()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(signup(&s.client(), Some(&t), "gina").ok().json()["role"], "admin");
    let old = "1".repeat(64);
    let me = user_id(&admin, "admin");
    s.state.store.create_invite(&token_digest(&old), Role::User, None, me, now() - 1).unwrap();
    assert_eq!(signup(&s.client(), Some(&old), "hank").code(), "badInvite");
    let t = admin.post("/api/admin/invites", json!({})).ok().json();
    admin.delete(&format!("/api/admin/invites/{}", t["id"])).ok();
    assert_eq!(signup(&s.client(), t["token"].as_str(), "ivan").code(), "badInvite");
    assert_eq!(admin.post("/api/admin/invites", json!({ "days": 31 })).status, 400);
    for (path, body) in [("/api/admin/invites", json!({})), ("/api/admin/invites/1", json!(null))] {
        let bob = s.client();
        let r = if body.is_null() { bob.delete(path) } else { bob.post(path, body) };
        assert_eq!(r.status, 401);
    }

    // Open sign-up: on by a setting, with the sign-up quota; throttled per address.
    let s = TestServer::with(|c| {
        c.open_signup = true;
        c.signup_quota = Some(123_456);
        c.login_attempts = 3;
    });
    let admin = s.admin();
    assert_eq!(s.client().get("/api/setup").ok().json()["openSignup"], true);
    let c = s.client();
    let r = signup(&c, None, "judy");
    assert_eq!(r.status, 201, "{}", r.body);
    let users = admin.get("/api/admin/users").ok().json();
    let judy = users.as_array().unwrap().iter().find(|u| u["username"] == "judy").unwrap();
    assert_eq!((judy["role"].as_str(), judy["quotaBytes"].as_u64()), (Some("user"), Some(123_456)));
    assert_eq!(signup(&c, None, "judy").status, 409);
    assert_eq!(signup(&c, None, "kate").status, 201);
    assert_eq!(signup(&c, None, "liam").status, 429, "three attempts from one address");
}

#[test]
fn reset_links_set_a_new_password_once_and_end_the_sessions() {
    let s = TestServer::with(|c| c.login_attempts = 4);
    let admin = s.admin();
    let mut bob = s.user(&admin, "bob");
    let id = user_id(&admin, "bob");
    assert_eq!(bob.post(&format!("/api/admin/users/{id}/reset-link"), json!({})).status, 403);
    let link = admin.post(&format!("/api/admin/users/{id}/reset-link"), json!({})).ok().json();
    let token = link["token"].as_str().unwrap().to_owned();
    assert_eq!(link["url"].as_str().unwrap(), format!("/#/reset/{token}"));
    let c = s.client();
    let reset = |token: &str, password: &str| {
        c.post("/api/password-reset", json!({ "token": token, "password": password }))
    };
    assert_eq!(reset(&token, "short").status, 400);
    reset(&token, "a new password").ok();
    assert_eq!(bob.get("/api/me").status, 401, "sessions ended");
    assert_eq!(bob.login("bob", PASSWORD).status, 401);
    bob.login("bob", "a new password").ok();
    assert_eq!(reset(&token, "another password").code(), "badReset", "used up");
    // A newer link replaces an older one.
    let first = admin.post(&format!("/api/admin/users/{id}/reset-link"), json!({})).ok().json();
    let second = admin.post(&format!("/api/admin/users/{id}/reset-link"), json!({})).ok().json();
    assert_eq!(reset(first["token"].as_str().unwrap(), "x password").code(), "badReset");
    reset(second["token"].as_str().unwrap(), "y password").ok();
    // Wrong tokens are throttled per address.
    for _ in 0..2 {
        assert_eq!(reset(&"2".repeat(64), "z password").status, 400);
    }
    assert_eq!(reset(&"2".repeat(64), "z password").status, 429);
}

/// A stand-in SMTP server: answers every command and hands over each
/// message's data.
fn smtp_server() -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut out) = stream else { return };
            let mut input = BufReader::new(out.try_clone().unwrap());
            let _ = out.write_all(b"220 stand-in ESMTP\r\n");
            let mut line = String::new();
            loop {
                line.clear();
                if input.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let cmd = line.trim_end().to_ascii_uppercase();
                if cmd.starts_with("DATA") {
                    let _ = out.write_all(b"354 go on\r\n");
                    let mut data = String::new();
                    loop {
                        line.clear();
                        if input.read_line(&mut line).unwrap_or(0) == 0 || line == ".\r\n" {
                            break;
                        }
                        data.push_str(&line);
                    }
                    let _ = tx.send(data);
                    let _ = out.write_all(b"250 queued\r\n");
                } else if cmd.starts_with("QUIT") {
                    let _ = out.write_all(b"221 bye\r\n");
                    break;
                } else {
                    let _ = out.write_all(b"250 ok\r\n");
                }
            }
        }
    });
    (port, rx)
}

#[test]
fn forgotten_passwords_are_reset_by_e_mail_without_telling_who_exists() {
    let (port, mails) = smtp_server();
    let s = TestServer::with(|c| {
        c.public_url = Some("http://chess.test".into());
        c.smtp = Some(SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            username: None,
            password: None,
            from: "Chess server <chess@chess.test>".into(),
            tls: SmtpTls::None,
        });
        c.login_attempts = 5;
    });
    let admin = s.admin();
    let mut bob = s.user(&admin, "bob");
    assert_eq!(s.client().get("/api/setup").ok().json()["emailResets"], true);
    let email = |c: &common::Client, pw: &str, e: serde_json::Value| {
        c.put("/api/me/email", json!({ "currentPassword": pw, "email": e }))
    };
    assert_eq!(email(&bob, "wrong", json!("bob@chess.test")).status, 403);
    assert_eq!(email(&bob, PASSWORD, json!("not an address")).status, 400);
    email(&bob, PASSWORD, json!("Bob@Chess.test")).ok();
    assert_eq!(bob.get("/api/me/account").ok().json()["email"], "Bob@Chess.test");
    assert_eq!(email(&admin, PASSWORD, json!("bob@chess.test")).status, 409, "taken, in any case");

    let c = s.client();
    let forgot = |login: &str| c.post("/api/password-reset/request", json!({ "login": login }));
    for login in ["bob@chess.test", "nobody", "nobody@chess.test", "admin"] {
        let r = forgot(login);
        assert_eq!((r.status, r.body.as_str()), (202, ""), "{login}: the same answer");
    }
    // One message: bob's (the admin has no address; nobody doesn't exist).
    let mail = mails.recv_timeout(Duration::from_secs(10)).expect("an e-mail");
    assert!(mails.recv_timeout(Duration::from_millis(500)).is_err(), "only one");
    let text = mail.replace("=\r\n", "");
    assert!(
        text.contains("To: Bob@Chess.test") || text.to_lowercase().contains("to: bob@chess.test"),
        "{text}"
    );
    let start = text.find("http://chess.test/#/reset/").expect("a link") + "http://chess.test/#/reset/".len();
    let token: String = text[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    assert_eq!(token.len(), 64);
    c.post("/api/password-reset", json!({ "token": token, "password": "a new password" })).ok();
    assert_eq!(bob.get("/api/me").status, 401);
    bob.login("bob", "a new password").ok();
    // Each request counts, per address and per name.
    assert_eq!(forgot("bob").status, 202, "the fifth from this address");
    assert_eq!(forgot("bob").status, 429, "the sixth");
    // Removing one's address: no more e-mails.
    email(&bob, "a new password", json!(null)).ok();
    assert!(bob.get("/api/me/account").ok().json()["email"].is_null());
}
