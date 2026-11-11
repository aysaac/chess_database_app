//! The audit log: security-relevant events with who, what, when and from
//! where; the admin's alone; never game content, criteria, positions or
//! file names; forgotten after its retention.

mod common;

use chess_server::housekeeping;
use chess_server::store::{AuditRow, now};
use common::{PASSWORD, TestServer, fixture_bytes};
use serde_json::{Value, json};

fn actions(page: &Value) -> Vec<String> {
    page["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_owned()).collect()
}

const AFTER_E4: &str = "rnbqkbnr%2Fpppppppp%2F8%2F8%2F4P3%2F8%2FPPPP1PPP%2FRNBQKBNR%20b%20KQkq%20-%200%201";

#[test]
fn security_events_are_logged_without_content() {
    let s = TestServer::start();
    let admin = s.admin();
    let mut alice = s.user(&admin, "alice");
    let users = admin.get("/api/admin/users").ok().json();
    let alice_id = users.as_array().unwrap().iter().find(|u| u["username"] == "alice").unwrap()["id"].clone();
    assert_eq!(s.client().login("alice", "wrong password").status, 401);
    let db = alice.create_db("Alice's prep", "private");
    let r = alice.upload(
        &format!("/api/databases/{db}/import?filename=secret-opening-file.pgn"),
        &fixture_bytes("reference.pgn"),
    );
    alice.wait_job(r.json()["job"].as_u64().unwrap());
    alice.get(&format!("/api/databases/{db}/games?white=Nepomniachtchi&fen={AFTER_E4}")).ok();
    alice.get(&format!("/api/search?db={db}&text=Caruana")).ok();
    alice.get(&format!("/api/databases/{db}/export.pgn?white=Nepomniachtchi")).ok();
    alice.patch(&format!("/api/databases/{db}"), json!({ "name": "Prep 2", "visibility": "shared" })).ok();
    alice.put(&format!("/api/databases/{db}/shares/admin"), json!({ "access": "read" })).ok();
    let t = alice.post("/api/tokens", json!({ "name": "laptop" })).ok().json();
    alice.delete(&format!("/api/tokens/{}", t["id"])).ok();
    alice.post(&format!("/api/databases/{db}/compact"), json!({})).ok();
    admin.patch(&format!("/api/admin/users/{alice_id}"), json!({ "role": "admin" })).ok();
    admin.patch(&format!("/api/admin/users/{alice_id}"), json!({ "disabled": true })).ok();
    admin.patch(&format!("/api/admin/users/{alice_id}"), json!({ "disabled": false })).ok();
    admin.post(&format!("/api/admin/users/{alice_id}/reset-link"), json!({})).ok();
    admin.post("/api/admin/invites", json!({})).ok();
    alice.login("alice", PASSWORD).ok();

    let page = admin.get("/api/admin/audit?limit=500").ok().json();
    let all = actions(&page);
    for want in [
        "user.create",
        "login",
        "login.failed",
        "database.create",
        "import.start",
        "export",
        "database.visibility",
        "token.create",
        "token.delete",
        "compact",
        "user.role",
        "user.disable",
        "user.enable",
        "password.reset_link",
        "invite.create",
    ] {
        assert!(all.iter().any(|a| a == want), "{want} in {all:?}");
    }
    let entry = page["entries"].as_array().unwrap().iter().find(|e| e["action"] == "import.start").unwrap();
    assert_eq!(
        (entry["actor"].as_str(), entry["targetKind"].as_str(), entry["detail"].as_str()),
        (Some("alice"), Some("database"), Some("an upload"))
    );
    assert_eq!(entry["client"], "127.0.0.1");
    let failed = page["entries"].as_array().unwrap().iter().find(|e| e["action"] == "login.failed").unwrap();
    assert_eq!((failed["actor"].clone(), failed["target"].as_str()), (Value::Null, Some("alice")));
    // Nothing of the games, the criteria, the position or the file.
    let text = page.to_string();
    for leak in ["secret-opening-file", "Nepomniachtchi", "Caruana", "Carlsen", "4P3", "e4", "rnbqkbnr"] {
        assert!(!text.contains(leak), "{leak} is in the audit log");
    }
    // Filters: a group of actions, a user, a time.
    let logins = actions(&admin.get("/api/admin/audit?action=login").ok().json());
    assert!(logins.iter().all(|a| a == "login" || a == "login.failed") && logins.len() >= 3, "{logins:?}");
    let about_alice = admin.get("/api/admin/audit?who=ALICE&limit=500").ok().json();
    assert!(
        about_alice["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["actor"] == "alice" || e["target"] == "alice")
    );
    let future = admin.get(&format!("/api/admin/audit?from={}", now() + 100)).ok().json();
    assert_eq!(future["total"], 0);
    // The admin's alone.
    let bob = s.user(&admin, "bob");
    assert_eq!(bob.get("/api/admin/audit").status, 403);
    // Old entries go after the retention (365 days by default).
    let old = AuditRow { at: now() - 400 * 24 * 3600, action: "login".into(), ..AuditRow::default() };
    s.state.store.audit(&old).unwrap();
    let before = admin.get("/api/admin/audit").ok().json()["total"].as_u64().unwrap();
    housekeeping::once(&s.state);
    assert_eq!(admin.get("/api/admin/audit").ok().json()["total"].as_u64().unwrap(), before - 1);
}
