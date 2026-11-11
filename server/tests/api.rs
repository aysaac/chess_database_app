//! The read API against chess-db itself, upload limits, quotas, the
//! import folder, bad ids, and a restart.

mod common;

use std::sync::{Arc, Mutex};

use chess_core::pgn::write_game;
use chess_core::position;
use chess_core::shakmaty::{Chess, Position};
use chess_db::{
    Database, HeaderQuery, NoProgress, ResultFilter, SortKey, SortSpec, build_sort, find_position,
    import_pgn, page_ids, reference_lines, reference_overview, search_headers, sorted_subset,
    update_position_index,
};
use chess_server::wire::{GameDto, GamePage, ReferenceLinesDto, lines_dto, reference_dto};
use common::{PASSWORD, TestServer, fixture, fixture_bytes};
use serde_json::json;

/// The fixtures imported into a database of our own, as the server does.
fn local_db(dir: &std::path::Path, fixtures: &[&str]) -> Mutex<Database> {
    let db = Mutex::new(Database::create(&dir.join("Local")).unwrap());
    for f in fixtures {
        import_pgn(&db, &fixture(f), &NoProgress).unwrap();
    }
    update_position_index(&db, &NoProgress).unwrap();
    db
}

/// `a == b`, numbers within 1e-9 (a float sent as JSON can come back a
/// unit in the last place off); panics naming the first difference.
fn assert_same(a: &serde_json::Value, b: &serde_json::Value, at: &str) {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!((x - y).abs() < 1e-9, "{at}: {x} != {y}");
        }
        (Value::Array(x), Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{at}: lengths");
            for (i, (x, y)) in x.iter().zip(y).enumerate() {
                assert_same(x, y, &format!("{at}[{i}]"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            assert_eq!(x.keys().collect::<Vec<_>>(), y.keys().collect::<Vec<_>>(), "{at}: keys");
            for (k, v) in x {
                assert_same(v, &y[k], &format!("{at}.{k}"));
            }
        }
        _ => assert_eq!(a, b, "{at}"),
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[test]
fn searches_and_sorts_give_what_chess_db_gives() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let fixtures = ["crosstable.pgn", "reference.pgn", "variations.pgn"];
    let db = alice.create_db("Mixed", "private");
    for f in fixtures {
        alice.import_fixture(&db, f);
    }
    let tmp = tempfile::tempdir().unwrap();
    let local = local_db(tmp.path(), &fixtures);

    let cases: Vec<(&str, HeaderQuery)> = vec![
        ("", HeaderQuery::default()),
        ("white=Carlsen", HeaderQuery { white: "Carlsen".into(), ..HeaderQuery::default() }),
        ("text=pine", HeaderQuery { text: "pine".into(), ..HeaderQuery::default() }),
        (
            "white=Oak&black=Pine&ignoreColors=true",
            HeaderQuery {
                white: "Oak".into(),
                black: "Pine".into(),
                ignore_colors: true,
                ..HeaderQuery::default()
            },
        ),
        ("event=spring%20round", HeaderQuery { event: "spring round".into(), ..HeaderQuery::default() }),
        (
            "yearFrom=2015&yearTo=2020",
            HeaderQuery { year_from: Some(2015), year_to: Some(2020), ..HeaderQuery::default() },
        ),
        ("result=whiteWins", HeaderQuery { result: Some(ResultFilter::WhiteWins), ..HeaderQuery::default() }),
        (
            "eloMin=2200&eloMode=either",
            HeaderQuery {
                elo_min: Some(2200),
                elo_mode: chess_db::EloMode::Either,
                ..HeaderQuery::default()
            },
        ),
        ("movesMin=20", HeaderQuery { moves_min: Some(20), ..HeaderQuery::default() }),
        ("annotatedOnly=true", HeaderQuery { annotated_only: true, ..HeaderQuery::default() }),
    ];
    let sorts = [
        ("", SortSpec::NATURAL),
        ("&sort=id&desc=true", SortSpec::new(SortKey::Id, true)),
        ("&sort=white", SortSpec::new(SortKey::White, false)),
        ("&sort=whiteElo&desc=true", SortSpec::new(SortKey::WhiteElo, true)),
        ("&sort=date", SortSpec::new(SortKey::Date, false)),
        ("&sort=event&desc=true", SortSpec::new(SortKey::Event, true)),
        ("&sort=plies", SortSpec::new(SortKey::Plies, false)),
    ];
    for (params, query) in &cases {
        let query = HeaderQuery { skip_deleted: true, ..query.clone() };
        let found = search_headers(&local, &query, None, &NoProgress).unwrap();
        for (sort_params, spec) in sorts {
            build_sort(&local, spec, &NoProgress).unwrap();
            let expected = if query == (HeaderQuery { skip_deleted: true, ..HeaderQuery::default() }) {
                page_ids(&local.lock().unwrap(), spec, 0, 500).unwrap()
            } else {
                sorted_subset(&local.lock().unwrap(), spec, &found).unwrap()
            };
            let page: GamePage =
                alice.get(&format!("/api/databases/{db}/games?limit=500&{params}{sort_params}")).ok().parse();
            let got: Vec<u32> = page.rows.iter().map(|r| r.id).collect();
            assert_eq!(got, expected, "{params}{sort_params}");
            assert_eq!(page.total as usize, expected.len());
            // Rows are the records chess-db reads.
            let rows = local.lock().unwrap().rows(&expected).unwrap();
            for (row, local) in page.rows.iter().zip(rows) {
                assert_eq!(row, &local.into());
            }
        }
    }
    // Paging: offset and limit cut the same list.
    let all: GamePage = alice.get(&format!("/api/databases/{db}/games?limit=500&sort=black")).ok().parse();
    let page: GamePage =
        alice.get(&format!("/api/databases/{db}/games?offset=10&limit=7&sort=black")).ok().parse();
    assert_eq!(page.rows, all.rows[10..17]);
    assert_eq!((page.total, page.offset), (all.total, 10));
    let past: GamePage = alice.get(&format!("/api/databases/{db}/games?offset=100000")).ok().parse();
    assert!(past.rows.is_empty());
    assert_eq!(
        alice.get(&format!("/api/databases/{db}/games?limit=100000")).ok().parse::<GamePage>().rows.len(),
        105
    );
    assert_eq!(alice.get(&format!("/api/databases/{db}/games?sort=nonsense")).status, 400);
}

/// After 1.e4 e5 2.Nf3, URL-encoded.
const AFTER_NF3: &str =
    "rnbqkbnr%2Fpppp1ppp%2F8%2F4p3%2F4P3%2F5N2%2FPPPP1PPP%2FRNBQKB1R%20b%20KQkq%20-%201%202";

#[test]
fn games_reaching_a_position_with_criteria_sort_and_paging() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let fixtures = ["crosstable.pgn", "reference.pgn", "variations.pgn"];
    let db = alice.create_db("Mixed", "private");
    for f in fixtures {
        alice.import_fixture(&db, f);
    }
    let tmp = tempfile::tempdir().unwrap();
    let local = local_db(tmp.path(), &fixtures);
    let mut pos = Chess::default();
    for uci in ["e2e4", "e7e5", "g1f3"] {
        let m = position::parse_uci(&pos, uci).unwrap();
        pos.play_unchecked(m);
    }
    let hits: Vec<u32> = find_position(&local, &pos).unwrap().entries.iter().map(|e| e.game).collect();
    assert!(hits.len() > 5, "{hits:?}");
    let set = chess_db::GameSet::from_ids(&hits);
    for (params, query) in [
        ("", HeaderQuery::default()),
        ("&text=pine", HeaderQuery { text: "pine".into(), ..HeaderQuery::default() }),
        (
            "&result=whiteWins",
            HeaderQuery { result: Some(ResultFilter::WhiteWins), ..HeaderQuery::default() },
        ),
    ] {
        let query = HeaderQuery { skip_deleted: true, ..query };
        let found = search_headers(&local, &query, Some(&set), &NoProgress).unwrap();
        for (sort_params, spec) in
            [("", SortSpec::NATURAL), ("&sort=white&desc=true", SortSpec::new(SortKey::White, true))]
        {
            build_sort(&local, spec, &NoProgress).unwrap();
            let expected = sorted_subset(&local.lock().unwrap(), spec, &found).unwrap();
            let r = alice
                .get(&format!("/api/databases/{db}/games?limit=500&fen={AFTER_NF3}{params}{sort_params}"));
            assert_eq!(r.ok().json()["kind"], "ready");
            let page: GamePage = alice
                .get(&format!("/api/databases/{db}/games?limit=500&fen={AFTER_NF3}{params}{sort_params}"))
                .parse();
            assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), expected, "{params}{sort_params}");
            let page: GamePage = alice
                .get(&format!(
                    "/api/databases/{db}/games?offset=2&limit=3&fen={AFTER_NF3}{params}{sort_params}"
                ))
                .ok()
                .parse();
            assert_eq!(page.total as usize, expected.len());
            let want: Vec<u32> = expected.iter().skip(2).take(3).copied().collect();
            assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), want);
        }
    }
    assert_eq!(alice.get(&format!("/api/databases/{db}/games?fen=nonsense")).status, 400);
}

#[test]
fn exports_stream_the_listed_games_as_pgn() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let fixtures = ["crosstable.pgn", "reference.pgn", "variations.pgn"];
    let db = alice.create_db("Réti's games", "private");
    for f in fixtures {
        alice.import_fixture(&db, f);
    }
    let tmp = tempfile::tempdir().unwrap();
    let local = local_db(tmp.path(), &fixtures);
    let pgn_of = |ids: &[u32]| -> String {
        let db = local.lock().unwrap();
        ids.iter().map(|&id| write_game(&db.read_game(id).unwrap())).collect()
    };
    let all: Vec<u32> = (0..local.lock().unwrap().count()).collect();
    let r = alice.get(&format!("/api/databases/{db}/export.pgn")).ok();
    assert_eq!(r.body, pgn_of(&all));
    assert_eq!(r.header("content-type"), Some("application/x-chess-pgn; charset=utf-8"));
    assert_eq!(
        r.header("content-disposition"),
        Some("attachment; filename=\"R_ti_s games.pgn\"; filename*=UTF-8''R%C3%A9ti%27s%20games.pgn")
    );
    assert!(r.header("content-length").is_none(), "streamed");
    // The same criteria and order as the list.
    let path = format!("fen={AFTER_NF3}&text=pine&sort=white&desc=true");
    let page: GamePage = alice.get(&format!("/api/databases/{db}/games?limit=500&{path}")).ok().parse();
    assert!(!page.rows.is_empty());
    let ids: Vec<u32> = page.rows.iter().map(|r| r.id).collect();
    assert_eq!(alice.get(&format!("/api/databases/{db}/export.pgn?{path}")).ok().body, pgn_of(&ids));
    // Deleted games stay out unless asked for.
    let open = s.state.pool.get(&db).unwrap();
    open.lock().set_deleted_many(&[0, 1], true).unwrap();
    drop(open);
    assert_eq!(alice.get(&format!("/api/databases/{db}/export.pgn")).ok().body, pgn_of(&all[2..]));
    assert_eq!(alice.get(&format!("/api/databases/{db}/export.pgn?deleted=only")).ok().body, pgn_of(&[0, 1]));
    assert_eq!(alice.get(&format!("/api/databases/{db}/export.pgn?white=nobody")).ok().body, "");
    // Only for those who may read it.
    assert_eq!(admin.get(&format!("/api/databases/{db}/export.pgn")).status, 404);
    // The admin's list says which they may export: their own and the server-wide ones.
    let wide = admin.create_db("Wide", "server");
    let listed = admin.get("/api/admin/databases").ok().json();
    let access =
        |id: &str| listed.as_array().unwrap().iter().find(|d| d["id"] == id).unwrap()["access"].clone();
    assert_eq!((access(&db), access(&wide)), (serde_json::Value::Null, json!("owner")));
}

#[test]
fn searching_several_databases_merges_their_lists() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let sets: [&[&str]; 3] =
        [&["crosstable.pgn"], &["reference.pgn", "setup.pgn"], &["variations.pgn", "lichess.pgn"]];
    let mut ids = Vec::new();
    let tmp = tempfile::tempdir().unwrap();
    let mut locals = Vec::new();
    for (i, fixtures) in sets.iter().enumerate() {
        let db = alice.create_db(&format!("Set {i}"), "private");
        for f in *fixtures {
            alice.import_fixture(&db, f);
        }
        ids.push(db);
        let dir = tmp.path().join(i.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        locals.push(local_db(&dir, fixtures));
    }
    let dbs = ids.iter().map(|d| format!("db={d}")).collect::<Vec<_>>().join("&");
    let local_refs: Vec<&Mutex<Database>> = locals.iter().collect();
    for (params, query, spec) in [
        ("", HeaderQuery::default(), SortSpec::NATURAL),
        ("&sort=white", HeaderQuery::default(), SortSpec::new(SortKey::White, false)),
        ("&sort=date&desc=true", HeaderQuery::default(), SortSpec::new(SortKey::Date, true)),
        (
            "&text=a&sort=black",
            HeaderQuery { text: "a".into(), ..HeaderQuery::default() },
            SortSpec::new(SortKey::Black, false),
        ),
        (
            "&sort=whiteElo&desc=true&eloMin=1",
            HeaderQuery { elo_min: Some(1), ..HeaderQuery::default() },
            SortSpec::new(SortKey::WhiteElo, true),
        ),
    ] {
        let query = HeaderQuery { skip_deleted: true, ..query };
        let want = chess_db::search_many(&local_refs, &query, None, spec, 0, 500, &NoProgress).unwrap();
        let got = alice.get(&format!("/api/search?{dbs}&limit=500{params}")).ok().json();
        assert_eq!(got["kind"], "ready");
        assert_eq!(got["total"].as_u64(), Some(want.total), "{params}");
        let rows: Vec<(String, u32)> = got["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["database"].as_str().unwrap().to_owned(), r["row"]["id"].as_u64().unwrap() as u32))
            .collect();
        let expected: Vec<(String, u32)> = want.rows.iter().map(|(i, r)| (ids[*i].clone(), r.id)).collect();
        assert_eq!(rows, expected, "{params}");
        assert_eq!(got["rows"][0]["databaseName"].as_str().unwrap(), format!("Set {}", want.rows[0].0));
        assert_eq!(got["databases"].as_array().unwrap().len(), 3);
        // Pages cut the same list.
        let page = alice.get(&format!("/api/search?{dbs}&offset=37&limit=11{params}")).ok().json();
        let got: Vec<(String, u32)> = page["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["database"].as_str().unwrap().to_owned(), r["row"]["id"].as_u64().unwrap() as u32))
            .collect();
        assert_eq!(got, expected.iter().skip(37).take(11).cloned().collect::<Vec<_>>(), "{params}");
    }
    // With a position too.
    let page = alice.get(&format!("/api/search?{dbs}&fen={AFTER_NF3}&limit=500")).ok().json();
    let mut pos = Chess::default();
    for uci in ["e2e4", "e7e5", "g1f3"] {
        let m = position::parse_uci(&pos, uci).unwrap();
        pos.play_unchecked(m);
    }
    let hits: u64 = locals.iter().map(|l| find_position(l, &pos).unwrap().entries.len() as u64).sum();
    assert_eq!(page["total"].as_u64(), Some(hits));
    // all=true is every database one may read; nothing names none.
    let all = alice.get("/api/search?all=true&limit=1").ok().json();
    assert_eq!(all["databases"].as_array().unwrap().len(), 3);
    assert_eq!(alice.get("/api/search").status, 400);
    assert_eq!(alice.get(&format!("/api/search?all=true&db={}", ids[0])).status, 400);
    assert_eq!(alice.get(&format!("/api/search?db={}&sort=nonsense", ids[0])).status, 400);
}

#[test]
fn a_position_search_waits_for_an_index_far_behind() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Behind", "private");
    // More games than a search replays (20,000), added without indexing.
    let open = s.state.pool.get(&db).unwrap();
    let games = chess_core::pgn::read_pgn_str(
        "[White \"A\"][Black \"B\"]\n1. e4 e5 2. Nf3 *\n\n[White \"C\"]\n1. d4 *",
    );
    let prepared: Vec<_> = games.iter().map(chess_db::PreparedGame::new).collect();
    for _ in 0..10_001 {
        open.lock().append_prepared(&prepared).unwrap();
    }
    drop(open);
    let r = admin.get(&format!("/api/databases/{db}/games?fen={AFTER_NF3}")).ok().json();
    assert_eq!(r["kind"], "job", "{r}");
    let job = admin.wait_job(r["job"].as_u64().unwrap());
    assert_eq!((job["state"].as_str(), job["kind"].as_str()), (Some("done"), Some("index")), "{job}");
    let page: GamePage =
        admin.get(&format!("/api/databases/{db}/games?fen={AFTER_NF3}&limit=3")).ok().parse();
    assert_eq!((page.total, page.rows.iter().map(|r| r.id).collect::<Vec<_>>()), (10_001, vec![0, 2, 4]));
}

/// Holds `db`'s writer slot with a job of `kind` (of no database for a
/// backup) until the sender is dropped, as an import or a backup does.
fn hold_writer(s: &TestServer, db: &str, kind: &str) -> std::sync::mpsc::Sender<()> {
    let (release, hold) = std::sync::mpsc::channel::<()>();
    let user = s.state.store.user_by_name("admin").unwrap().unwrap().0;
    let of = if kind == "backup" { ("", "") } else { (db, "Held") };
    let job = s.state.jobs.spawn(kind, "Holding the database".into(), of, &user, move |_| {
        let _ = hold.recv();
        Ok(String::new())
    });
    *s.state.pool.get(db).unwrap().writer.lock().unwrap() = Some(job);
    release
}

/// (database, game id) of each row of a search's answer.
fn search_rows(answer: &serde_json::Value) -> Vec<(String, u32)> {
    answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["database"].as_str().unwrap().to_owned(), r["row"]["id"].as_u64().unwrap() as u32))
        .collect()
}

/// While a write job adds games, a database's own sorted list keeps them
/// (after the sorted ones, without sorting every game again at each
/// batch), and a search across databases still merges every list in true
/// sort order.
#[test]
fn lists_keep_the_games_a_write_job_adds() {
    let s = TestServer::start();
    let admin = s.admin();
    let a = admin.create_db("Growing", "private");
    let b = admin.create_db("Other", "private");
    admin.import_fixture(&a, "lichess.pgn");
    admin.import_fixture(&b, "crosstable.pgn");
    let total = |q: &str| {
        admin.get(&format!("/api/databases/{a}/games?{q}&limit=500")).ok().json()["total"].as_u64().unwrap()
    };
    // Sort orders built (and kept) before the job.
    assert_eq!(total("sort=white"), 1);
    assert_eq!(total("sort=black&text=a"), total("text=a"));
    let both = format!("db={a}&db={b}");
    admin.get(&format!("/api/search?{both}&sort=white")).ok();

    // A write job holds the database and appends games, as an import's batches do.
    let release = hold_writer(&s, &a, "import");
    let added = chess_core::pgn::read_pgn_str(&String::from_utf8(fixture_bytes("reference.pgn")).unwrap());
    let open = s.state.pool.get(&a).unwrap();
    for g in &added {
        open.lock().append(g).unwrap();
    }
    let count = u64::from(open.lock().count());
    assert_eq!(count, 1 + added.len() as u64);

    // Its own list: every game, the ones added after the sorted one, in id order.
    assert_eq!(total("sort=white"), count);
    let page: GamePage = admin.get(&format!("/api/databases/{a}/games?sort=white&limit=500")).ok().parse();
    assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), (0..count as u32).collect::<Vec<_>>());
    let page: GamePage =
        admin.get(&format!("/api/databases/{a}/games?sort=white&offset=50&limit=3")).ok().parse();
    assert_eq!(page.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [50, 51, 52]);
    assert_eq!(total("sort=black&text=a"), total("text=a"));

    // Across databases: the same games, merged in true order, as chess-db
    // merges copies of the databases.
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["a", "b"] {
        std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    let local_a = local_db(&tmp.path().join("a"), &[]);
    import_pgn(&local_a, &fixture("lichess.pgn"), &NoProgress).unwrap();
    for g in &added {
        local_a.lock().unwrap().append(g).unwrap();
    }
    let local_b = local_db(&tmp.path().join("b"), &["crosstable.pgn"]);
    let ids = [a.clone(), b.clone()];
    for (params, query, spec) in [
        ("&sort=white", HeaderQuery::default(), SortSpec::new(SortKey::White, false)),
        ("&sort=date&desc=true", HeaderQuery::default(), SortSpec::new(SortKey::Date, true)),
        (
            "&text=a&sort=black",
            HeaderQuery { text: "a".into(), ..HeaderQuery::default() },
            SortSpec::new(SortKey::Black, false),
        ),
    ] {
        let query = HeaderQuery { skip_deleted: true, ..query };
        let want =
            chess_db::search_many(&[&local_a, &local_b], &query, None, spec, 0, 500, &NoProgress).unwrap();
        let expected: Vec<(String, u32)> = want.rows.iter().map(|(i, r)| (ids[*i].clone(), r.id)).collect();
        let got = admin.get(&format!("/api/search?{both}&limit=500{params}")).ok().json();
        assert_eq!(got["total"].as_u64(), Some(want.total), "{params}");
        assert_eq!(search_rows(&got), expected, "{params}");
        // Pages cut the same list: no row twice, none left out.
        let mut paged = Vec::new();
        for offset in (0..want.total).step_by(7) {
            let page = admin.get(&format!("/api/search?{both}&offset={offset}&limit=7{params}")).ok().json();
            paged.extend(search_rows(&page));
        }
        assert_eq!(paged, expected, "{params}");
    }
    drop(release);
}

/// The position index far behind while a backup holds the database's
/// writer slot: "busy", not the backup's job (which the caller can't see).
#[test]
fn a_position_search_is_busy_while_a_backup_holds_the_database() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Behind", "private");
    let open = s.state.pool.get(&db).unwrap();
    let games = chess_core::pgn::read_pgn_str("[White \"A\"]\n1. e4 e5 2. Nf3 *\n\n[White \"C\"]\n1. d4 *");
    let prepared: Vec<_> = games.iter().map(chess_db::PreparedGame::new).collect();
    for _ in 0..10_001 {
        open.lock().append_prepared(&prepared).unwrap();
    }
    drop(open);
    let release = hold_writer(&s, &db, "backup");
    for path in
        [format!("/api/databases/{db}/games?fen={AFTER_NF3}"), format!("/api/search?db={db}&fen={AFTER_NF3}")]
    {
        let r = alice.get(&path);
        assert_eq!((r.status, r.code().as_str()), (503, "busy"), "{}", r.body);
        assert!(r.header("retry-after").is_some());
    }
    drop(release);
    // Once it's done: the index job, which they can follow.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let r = loop {
        let r = alice.get(&format!("/api/databases/{db}/games?fen={AFTER_NF3}"));
        if r.status != 503 {
            break r.ok().json();
        }
        assert!(std::time::Instant::now() < deadline, "still busy");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(r["kind"], "job", "{r}");
    let job = alice.wait_job(r["job"].as_u64().unwrap());
    assert_eq!(job["kind"], "index");
}

#[test]
fn trees_and_lines_give_what_chess_db_gives() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Reference", "server");
    admin.import_fixture(&db, "reference.pgn");
    let alice = s.user(&admin, "alice");
    let tmp = tempfile::tempdir().unwrap();
    let local = local_db(tmp.path(), &["reference.pgn"]);

    let mut positions = vec![Chess::default()];
    for uci in ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5"] {
        let mut pos = positions.last().unwrap().clone();
        let m = position::parse_uci(&pos, uci).unwrap();
        pos.play_unchecked(m);
        positions.push(pos);
    }
    for pos in positions {
        let fen = position::to_fen(&pos);
        let expected =
            serde_json::to_value(reference_dto(&pos, reference_overview(&local, &pos).unwrap(), true))
                .unwrap();
        let got = alice.get(&format!("/api/databases/{db}/tree?fen={}", enc(&fen))).ok().json();
        assert_same(&got, &expected, &format!("tree at {fen}"));
        assert!(got["indexed"].as_bool().unwrap() && !got["partial"].as_bool().unwrap());

        let hits = find_position(&local, &pos).unwrap();
        let expected = lines_dto(&pos, &reference_lines(&local, &pos, &hits, &NoProgress).unwrap());
        let got: ReferenceLinesDto =
            alice.get(&format!("/api/databases/{db}/lines?fen={}", enc(&fen))).ok().parse();
        let ReferenceLinesDto::Ready { lines } = got else { panic!("a job for a few games") };
        let (got, expected) =
            (serde_json::to_value(&*lines).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_same(&got, &expected, &format!("lines at {fen}"));
    }
    // The start when no FEN is given; a bad FEN is refused.
    let start = alice.get(&format!("/api/databases/{db}/tree")).ok().json();
    assert_eq!(start["total"]["games"], 56);
    assert_eq!(alice.get(&format!("/api/databases/{db}/tree?fen=not%20a%20fen")).status, 400);
}

#[test]
fn games_come_back_as_stored() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Games", "private");
    for f in ["variations.pgn", "setup.pgn", "lichess.pgn"] {
        admin.import_fixture(&db, f);
    }
    let tmp = tempfile::tempdir().unwrap();
    let local = local_db(tmp.path(), &["variations.pgn", "setup.pgn", "lichess.pgn"]);
    let count = local.lock().unwrap().count();
    for id in 0..count {
        let got: GameDto = admin.get(&format!("/api/databases/{db}/games/{id}")).ok().parse();
        let g = local.lock().unwrap().read_game(id).unwrap();
        assert_eq!(got.pgn, write_game(&g));
        assert_eq!(got.headers.white, g.headers.white);
        assert_eq!(got.start_fen, g.setup.as_ref().map(position::to_fen));
        assert_eq!(got.notation.moves.len(), g.mainline_len());
        assert_eq!(got.row, local.lock().unwrap().row(id).unwrap().into());
    }
    let with_variations: GameDto = admin.get(&format!("/api/databases/{db}/games/0")).ok().parse();
    assert!(with_variations.notation.moves.iter().any(|m| !m.variations.is_empty()));
    let report = admin.get("/api/jobs/1").ok().json();
    assert_eq!(report["report"]["file"], "variations.pgn");
}

#[test]
fn uploads_are_limited_and_cleaned_up() {
    let s = TestServer::with(|c| c.max_upload = 4096);
    let admin = s.admin();
    let db = admin.create_db("Small", "private");
    let path = format!("/api/databases/{db}/import");
    let big = fixture_bytes("crosstable.pgn");
    assert!(big.len() > 4096);
    let r = admin.upload(&path, &big);
    assert_eq!((r.status, r.code().as_str()), (413, "tooLarge"), "{}", r.body);
    let r = admin.upload_chunked(&path, &big);
    assert_eq!(r.status, 413, "without a Content-Length too: {}", r.body);
    assert_eq!(admin.upload(&path, b"").status, 400);
    // At the limit is fine.
    let small = fixture_bytes("variations.pgn");
    assert!(small.len() <= 4096);
    let r = admin.upload_chunked(&format!("{path}?filename=..%2F..%2Fvariations.pgn"), &small);
    assert_eq!(r.status, 202, "{}", r.body);
    let job = admin.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(job["report"]["file"], "variations.pgn");
    // A bad PGN imports what it can; nothing is left in the uploads folder.
    admin.import_fixture(&db, "malformed.pgn");
    let left: Vec<_> = std::fs::read_dir(s.data_dir().join("tmp")).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(admin.get(&format!("/api/databases/{db}")).ok().json()["games"], 6);
}

#[test]
fn quotas_stop_imports() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let db = alice.create_db("Growing", "private");
    let size = alice.get(&format!("/api/databases/{db}")).ok().json()["sizeBytes"].as_u64().unwrap();
    admin.patch(&format!("/api/admin/databases/{db}"), json!({ "quotaBytes": size + 2000 })).ok();
    let r = alice.upload(&format!("/api/databases/{db}/import"), &fixture_bytes("crosstable.pgn"));
    assert_eq!(r.status, 202, "{}", r.body);
    let job = alice.wait_job(r.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["error"].as_str().unwrap().contains("quota"), "{job}");
    // Nor is their position index built past it, even by a tree.
    let info = |db: &str| alice.get(&format!("/api/databases/{db}")).ok().json();
    assert_eq!(info(&db)["index"]["upToDate"], false);
    alice.get(&format!("/api/databases/{db}/tree")).ok();
    let index_jobs = || {
        let jobs = admin.get("/api/admin/jobs").ok().json();
        jobs.as_array()
            .unwrap()
            .iter()
            .filter(|j| j["kind"] == "index")
            .map(|j| j["id"].clone())
            .collect::<Vec<_>>()
    };
    assert!(index_jobs().is_empty());
    let r = alice.upload(&format!("/api/databases/{db}/import"), &fixture_bytes("setup.pgn"));
    assert_eq!((r.status, r.code().as_str()), (507, "quotaExceeded"));
    admin.patch(&format!("/api/admin/databases/{db}"), json!({ "quotaBytes": null })).ok();
    // With room, the next tree builds it.
    alice.get(&format!("/api/databases/{db}/tree")).ok();
    let job = alice.wait_job(index_jobs()[0].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(info(&db)["index"]["upToDate"], true);
    // A user's quota covers all their databases.
    let id = admin
        .get("/api/admin/users")
        .ok()
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "alice")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "quotaBytes": 1 })).ok();
    let other = alice.create_db("Other", "private");
    let r = alice.upload(&format!("/api/databases/{other}/import"), &fixture_bytes("setup.pgn"));
    assert_eq!(r.status, 507);
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "quotaBytes": null })).ok();
    alice.import_fixture(&other, "setup.pgn");
}

#[test]
fn imports_at_once_share_their_owners_quota() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let info = |db: &str| alice.get(&format!("/api/databases/{db}")).ok().json();
    let size = |db: &str| info(db)["sizeBytes"].as_u64().unwrap();
    let pgn = Arc::new(common::many_games(20_000));
    // What one import of it takes: games, then their index.
    let probe = alice.create_db("Probe", "private");
    let empty = size(&probe);
    let started = std::time::Instant::now();
    let r = alice.upload(&format!("/api/databases/{probe}/import"), &pgn);
    assert_eq!(alice.wait_job(r.json()["job"].as_u64().unwrap())["state"], "done");
    let grown = size(&probe) - empty;
    let games = grown - info(&probe)["index"]["sizeBytes"].as_u64().unwrap();
    eprintln!("one import: {grown} bytes ({games} without the index) in {:?}", started.elapsed());
    alice.delete(&format!("/api/databases/{probe}?confirm=Probe")).ok();

    // Room for the games of one and a half imports.
    let (a, b) = (alice.create_db("A", "private"), alice.create_db("B", "private"));
    let used = size(&a) + size(&b);
    let users = admin.get("/api/admin/users").ok().json();
    let id = users.as_array().unwrap().iter().find(|u| u["username"] == "alice").unwrap()["id"].clone();
    admin.patch(&format!("/api/admin/users/{id}"), json!({ "quotaBytes": used + games * 3 / 2 })).ok();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let uploads: Vec<_> = [&a, &b]
        .into_iter()
        .map(|db| {
            let (c, path, pgn, barrier) =
                (alice.same(), format!("/api/databases/{db}/import"), pgn.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                let r = c.upload(&path, &pgn);
                assert_eq!(r.status, 202, "{}", r.body);
                r.json()["job"].as_u64().unwrap()
            })
        })
        .collect();
    let jobs: Vec<u64> = uploads.into_iter().map(|u| u.join().unwrap()).collect();
    let ends: Vec<_> = jobs.into_iter().map(|j| alice.wait_job(j)).collect();
    let stopped =
        |j: &serde_json::Value| j["state"] == "failed" && j["error"].as_str().unwrap().contains("quota");
    assert!(ends.iter().any(stopped), "one of them stops at the quota: {ends:?}");
    assert!(size(&a) + size(&b) < used + 2 * grown, "not both imported and indexed");
}

#[test]
fn the_admin_imports_from_the_server_folder_into_any_visibility() {
    let s = TestServer::start();
    let admin = s.admin();
    let alice = s.user(&admin, "alice");
    let folder = s.data_dir().join("import").join("twic");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::copy(fixture("reference.pgn"), folder.join("ref.pgn")).unwrap();
    std::fs::write(folder.join("notes.txt"), "not a game").unwrap();
    std::fs::write(s.data_dir().join("server-secret.pgn"), "x").unwrap();
    let files = admin.get("/api/admin/import-files").ok().json();
    assert_eq!(
        files,
        json!([{ "path": "twic/ref.pgn", "sizeBytes": std::fs::metadata(folder.join("ref.pgn")).unwrap().len(), "kind": "pgn" }])
    );

    // Private by default: a licensed database stays the admin's.
    let r = admin.post("/api/databases", json!({ "name": "Licensed" })).ok();
    let private = r.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(r.json()["visibility"], "private");
    let job = admin.post(&format!("/api/databases/{private}/import-file"), json!({ "path": "twic/ref.pgn" }));
    assert_eq!(job.status, 202, "{}", job.body);
    let job = admin.wait_job(job.json()["job"].as_u64().unwrap());
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(alice.get(&format!("/api/databases/{private}")).status, 404);
    // Server-wide for free reference data.
    let wide = admin.create_db("TWIC", "server");
    let job =
        admin.post(&format!("/api/databases/{wide}/import-file"), json!({ "path": "twic/ref.pgn" })).ok();
    admin.wait_job(job.json()["job"].as_u64().unwrap());
    assert_eq!(alice.get(&format!("/api/databases/{wide}")).ok().json()["games"], 56);
    assert_eq!(alice.get(&format!("/api/databases/{wide}/tree")).ok().json()["total"]["games"], 56);

    for bad in [
        "../server-secret.pgn",
        "/etc/passwd",
        "twic/../../server.sqlite",
        "twic/notes.txt",
        "twic",
        "",
        "nope.pgn",
    ] {
        let r = admin.post(&format!("/api/databases/{wide}/import-file"), json!({ "path": bad }));
        assert_eq!(r.status, 400, "{bad}: {}", r.body);
    }
    // Only the admin imports from the folder, even into their own database.
    let own = alice.create_db("Mine", "private");
    assert_eq!(
        alice.post(&format!("/api/databases/{own}/import-file"), json!({ "path": "twic/ref.pgn" })).status,
        403
    );
}

#[test]
fn bad_ids_are_refused_before_any_path_is_built() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Real", "private");
    admin.import_fixture(&db, "setup.pgn");
    let fake = "0".repeat(32);
    for id in [
        "..",
        "..%2F..%2Fserver.sqlite",
        "%2e%2e",
        "..%2Fdbs",
        &db.to_uppercase(),
        &db[1..],
        &format!("{db}0"),
        &format!("{db}.chessdb"),
        &fake,
        "%00",
    ] {
        for path in [
            format!("/api/databases/{id}"),
            format!("/api/databases/{id}/games"),
            format!("/api/databases/{id}/tree"),
        ] {
            let r = admin.get(&path);
            assert_eq!(r.status, 404, "{path}: {}", r.body);
        }
        assert_eq!(admin.upload(&format!("/api/databases/{id}/import"), b"1. e4 *").status, 404, "{id}");
        assert_eq!(admin.delete(&format!("/api/admin/databases/{id}?confirm=Real")).status, 404, "{id}");
    }
    for game in ["-1", "abc", "3", "4294967296", "1.0"] {
        assert_eq!(admin.get(&format!("/api/databases/{db}/games/{game}")).status, 404, "{game}");
    }
    for job in ["-1", "x", "999"] {
        assert_eq!(admin.get(&format!("/api/jobs/{job}")).status, 404);
    }
    assert_eq!(admin.get("/api/nothing/here").status, 404);
    // The real one is still there, and nothing outside dbs/ was touched.
    assert_eq!(admin.get(&format!("/api/databases/{db}")).ok().json()["games"], 3);
    assert!(s.data_dir().join("server.sqlite").is_file());
}

#[test]
fn the_page_and_health_are_served_with_safe_headers() {
    let s = TestServer::start();
    let c = s.client();
    assert_eq!(c.get("/api/health").ok().json(), json!({ "status": "ok" }));
    let page = c.get("/").ok();
    assert!(page.header("content-type").unwrap().starts_with("text/html"));
    assert!(page.header("content-security-policy").unwrap().contains("default-src 'self'"));
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    let api = c.get("/api/setup").ok();
    assert_eq!(api.header("cache-control"), Some("no-store"));
    assert_eq!(c.get("/no/such/file.js").status, 404);
}

#[test]
fn everything_survives_a_restart() {
    let s = TestServer::start();
    let admin = s.admin();
    let db = admin.create_db("Kept", "server");
    admin.import_fixture(&db, "reference.pgn");
    let token =
        admin.post("/api/tokens", json!({ "name": "t" })).ok().json()["token"].as_str().unwrap().to_owned();
    let s = s.restart();
    assert!(s.state.setup_token().is_none(), "no setup once there are users");
    let api = s.client().with_bearer(&token);
    assert_eq!(api.get(&format!("/api/databases/{db}/tree")).ok().json()["total"]["games"], 56);
    let mut again = s.client();
    again.login("admin", PASSWORD).ok();
    assert_eq!(again.get("/api/databases").ok().json()[0]["games"], 56);
}
