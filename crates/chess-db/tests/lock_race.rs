//! A database closed while another thread starts a process must reopen at
//! once. A child process holds copies of the parent's file descriptors from
//! the fork until it runs its program, and an flock stays held while any copy
//! of its descriptor is open, so closing the lock file alone left the lock on
//! for that window and the next open reported `Locked`.

#![cfg(unix)]

use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chess_db::{Database, DbError};

#[test]
fn a_closed_database_reopens_while_other_threads_start_processes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Race.chessdb");
    drop(Database::create(&path).unwrap());

    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..4)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = Command::new("true").status();
                }
            })
        })
        .collect();

    let mut locked = 0;
    for _ in 0..2000 {
        match Database::open(&path) {
            Ok(db) => drop(db),
            Err(DbError::Locked(_)) => locked += 1,
            Err(e) => panic!("{e}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    for s in spawners {
        s.join().unwrap();
    }
    assert_eq!(locked, 0, "reported Locked {locked} times out of 2000");
}
