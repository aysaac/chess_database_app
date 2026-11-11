//! The TWIC updater (NET-06) against a stand-in for theweekinchess.com on
//! 127.0.0.1: the newest issue taken first, then each new one, none twice;
//! into a database the admin owns only; on a schedule.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use chess_server::housekeeping;
use common::TestServer;
use serde_json::json;

fn zip_of(name: &str, contents: &[u8]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(name, options).unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap();
    out.into_inner()
}

/// Issue `n`: `n % 5 + 1` games of its own.
fn issue(n: u32) -> String {
    (0..n % 5 + 1)
        .map(|i| format!("[Event \"TWIC {n}\"]\n[White \"W{n}-{i}\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 1-0\n\n"))
        .collect()
}

/// A stand-in site whose newest issue is `latest`.
fn site(latest: Arc<AtomicU32>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut out) = stream else { return };
            let mut input = BufReader::new(out.try_clone().unwrap());
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            loop {
                let mut h = String::new();
                if input.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let newest = latest.load(Ordering::SeqCst);
            let zip = path
                .strip_prefix("/zips/twic")
                .and_then(|p| p.strip_suffix("g.zip"))
                .and_then(|n| n.parse().ok());
            let (status, body): (&str, Vec<u8>) = match (path.as_str(), zip) {
                ("/twic", _) => {
                    let links: String = (newest - 3..=newest)
                        .map(|n| format!("<a href='/zips/twic{n}g.zip'>{n}</a>"))
                        .collect();
                    ("200 OK", links.into_bytes())
                }
                (_, Some(n)) if n <= newest && n != 1602 => {
                    ("200 OK", zip_of(&format!("twic{n}.pgn"), issue(n).as_bytes()))
                }
                _ => ("404 Not Found", Vec::new()),
            };
            let _ = write!(
                out,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = out.write_all(&body);
            let mut rest = Vec::new();
            let _ = input.read_to_end(&mut rest);
        }
    });
    base
}

fn check(admin: &common::Client) -> serde_json::Value {
    let r = admin.post("/api/admin/twic/check", json!({}));
    assert_eq!(r.status, 202, "{}", r.body);
    let job = r.json()["job"].as_u64().unwrap();
    admin.wait_job(job)
}

#[test]
fn new_issues_go_into_the_admins_database_once_each() {
    let latest = Arc::new(AtomicU32::new(1600));
    let base = site(latest.clone());
    let s = TestServer::with(|c| c.twic_url = base.clone());
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let theirs = alice.create_db("Alice's", "shared");
    alice.put(&format!("/api/databases/{theirs}/shares/admin"), json!({ "access": "write" })).ok();
    let private = alice.create_db("Alice's prep", "private");
    let db = admin.create_db("TWIC", "server");

    // Not configured yet; only into a database the admin owns.
    assert_eq!(admin.post("/api/admin/twic/check", json!({})).status, 409);
    assert_eq!(admin.put("/api/admin/twic", json!({ "database": private })).status, 404);
    assert_eq!(admin.put("/api/admin/twic", json!({ "database": theirs })).status, 403);
    assert_eq!(alice.put("/api/admin/twic", json!({ "database": theirs })).status, 403);
    let set = admin.put("/api/admin/twic", json!({ "database": db })).ok().json();
    assert_eq!((set["databaseName"].as_str(), set["everyDays"].as_u64()), (Some("TWIC"), Some(7)));

    // The first check takes the newest issue only.
    let job = check(&admin);
    assert_eq!(job["state"], "done", "{job}");
    let games = |s: &str| admin.get(&format!("/api/databases/{s}")).ok().json()["games"].as_u64().unwrap();
    assert_eq!(games(&db), 1, "issue 1600: one game");
    // Nothing new: nothing twice.
    assert_eq!(check(&admin)["message"], "No new issue (the newest is 1600)");
    assert_eq!(games(&db), 1);
    // Three new issues (1602 missing from the site): each one once.
    latest.store(1604, Ordering::SeqCst);
    let job = check(&admin);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(games(&db), 1 + 2 + 4 + 5, "1601, 1603 and 1604");
    let view = admin.get("/api/admin/twic").ok().json();
    let taken: Vec<u64> =
        view["issues"].as_array().unwrap().iter().map(|i| i["issue"].as_u64().unwrap()).collect();
    assert_eq!(taken, [1604, 1603, 1601, 1600]);
    assert_eq!(view["issues"][0]["games"], 5);
    assert_eq!(view["lastIssue"], 1604);
    let events = admin.get(&format!("/api/databases/{db}/games?limit=50&sort=event")).ok().json();
    let mut names: Vec<String> =
        events["rows"].as_array().unwrap().iter().map(|r| r["event"].as_str().unwrap().to_owned()).collect();
    names.dedup();
    assert_eq!(names, ["TWIC 1600", "TWIC 1601", "TWIC 1603", "TWIC 1604"]);
    assert!(s.data_dir().join("tmp").read_dir().unwrap().next().is_none(), "no download left behind");
    let audit = admin.get("/api/admin/audit?action=twic&limit=50").ok().json();
    assert!(audit["total"].as_u64().unwrap() >= 6, "{audit}");

    // On a schedule: due once the days passed (none passed yet).
    housekeeping::once(&s.state);
    let jobs = admin.get("/api/admin/jobs").ok().json();
    assert_eq!(jobs.as_array().unwrap().iter().filter(|j| j["kind"] == "twic").count(), 3);
    admin.put("/api/admin/twic", json!({ "database": db, "everyDays": 7 })).ok();
    s.state.store.set_setting("twic.last_check", Some("0")).unwrap();
    latest.store(1605, Ordering::SeqCst);
    housekeeping::once(&s.state);
    let jobs = admin.get("/api/admin/jobs").ok().json();
    let scheduled =
        jobs.as_array().unwrap().iter().find(|j| j["kind"] == "twic").unwrap()["id"].as_u64().unwrap();
    assert_eq!(admin.wait_job(scheduled)["state"], "done");
    assert_eq!(games(&db), 12 + 1);
    // Turned off.
    admin.put("/api/admin/twic", json!({ "database": null })).ok();
    assert_eq!(admin.post("/api/admin/twic/check", json!({})).status, 409);
}
