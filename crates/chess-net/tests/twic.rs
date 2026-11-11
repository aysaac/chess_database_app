//! The TWIC client (NET-06) against a server on 127.0.0.1 standing in for
//! theweekinchess.com: its TWIC page, issue zips, and a missing issue.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chess_db::{Database, NoProgress};
use chess_net::twic::{Twic, TwicError};

/// A zip holding `name` with `contents`, deflated.
pub fn zip_of(name: &str, contents: &[u8]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut out);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file(name, options).unwrap();
    zip.write_all(contents).unwrap();
    zip.finish().unwrap();
    out.into_inner()
}

const ISSUE: &str = "[Event \"Stand-in Open\"]\n[White \"A\"]\n[Black \"B\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7# 1-0\n\n\
[Event \"Stand-in Open\"]\n[White \"C\"]\n[Black \"D\"]\n[Result \"1/2-1/2\"]\n\n1. d4 d5 1/2-1/2\n";

/// Serves the stand-in site; the requests it got (path, user agent).
fn serve() -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut out) = stream else { return };
            let mut input = BufReader::new(out.try_clone().unwrap());
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let mut agent = String::new();
            loop {
                let mut h = String::new();
                if input.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("user-agent:") {
                    agent = v.trim().to_owned();
                }
            }
            log.lock().unwrap().push((path.clone(), agent));
            let (status, body): (&str, Vec<u8>) = match path.as_str() {
                "/twic" => (
                    "200 OK",
                    b"<html><a href='/zips/twic1599g.zip'>PGN</a> <a href='/zips/twic1601g.zip'>PGN</a> \
                      <a href='/zips/twic1601c6.zip'>CBV</a></html>"
                        .to_vec(),
                ),
                "/zips/twic1601g.zip" => ("200 OK", zip_of("twic1601.pgn", ISSUE.as_bytes())),
                "/zips/twic1599g.zip" => ("200 OK", zip_of("readme.txt", b"no games")),
                _ => ("404 Not Found", b"not here".to_vec()),
            };
            let _ = write!(
                out,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = out.write_all(&body);
            let _ = out.flush();
            let mut rest = Vec::new();
            let _ = input.read_to_end(&mut rest);
        }
    });
    (base, seen)
}

#[test]
fn the_newest_issue_is_found_and_imported() {
    let (base, seen) = serve();
    let twic = Twic::with_base(&base);
    assert_eq!(twic.latest_issue().unwrap(), 1601);
    let pgn = twic.issue_pgn(1601).unwrap();
    assert_eq!(pgn, ISSUE.as_bytes());
    let dir = tempfile::tempdir().unwrap();
    let db = Mutex::new(Database::create(&dir.path().join("TWIC")).unwrap());
    let summary = twic.import_issue(&db, 1601, &NoProgress).unwrap();
    assert_eq!(summary.imported, 2);
    assert_eq!(db.lock().unwrap().row(0).unwrap().event, "Stand-in Open");
    assert!(matches!(twic.issue_pgn(1600), Err(TwicError::NoSuchIssue(1600))));
    assert!(matches!(twic.issue_pgn(1599), Err(TwicError::BadZip(1599, _))));
    // It says what it is.
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter()
            .all(|(_, agent)| agent.starts_with("chess-database-app/") && agent.contains("twic updater")),
        "{seen:?}"
    );
    assert_eq!(seen[0].0, "/twic");
}

#[test]
fn a_server_that_isnt_there_is_said_so() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let twic = Twic::with_base(&format!("http://127.0.0.1:{port}"));
    assert!(matches!(twic.latest_issue(), Err(TwicError::Unreachable(_))));
}
