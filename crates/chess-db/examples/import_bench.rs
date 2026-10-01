//! Dev benchmark (not a product CLI): imports a PGN into a fresh database
//! and times the operations the UI depends on. Given an existing
//! `.chessdb` folder instead, it skips the import and leaves it in place.
//!
//! Usage: cargo run --release -p chess-db --example import_bench -- FILE.pgn|DB.chessdb [OUT_DIR]

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use chess_core::position::parse_uci;
use chess_core::shakmaty::{Chess, Position};
use chess_db::{
    Database, HeaderQuery, NoProgress, ResultFilter, SortKey, SortSpec, build_sort, find_position,
    import_pgn, page_ids, position_tree, reference_lines, reference_overview, search_headers,
    update_position_index,
};

fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(args.next().ok_or("usage: import_bench FILE.pgn|DB.chessdb [OUT_DIR]")?);
    let existing = input.is_dir();
    let db_path = if existing {
        input.clone()
    } else {
        let out_dir = args.next().map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        out_dir.join(format!("bench-{}.chessdb", std::process::id()))
    };
    if !existing {
        import(&input, &db_path)?;
    }
    run(&db_path)?;
    if !existing {
        std::fs::remove_dir_all(&db_path)?;
    }
    Ok(())
}

fn import(pgn: &std::path::Path, db_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let pgn_size = std::fs::metadata(pgn)?.len();
    let db = Mutex::new(Database::create(db_path)?);
    let t = Instant::now();
    let summary = import_pgn(&db, pgn, &NoProgress)?;
    let secs = t.elapsed().as_secs_f64();
    let info = db.lock().unwrap().info();
    println!(
        "import      {} games in {secs:.2}s = {:.0} games/s ({:.1} MB/s)",
        summary.imported,
        summary.imported as f64 / secs,
        pgn_size as f64 / 1e6 / secs
    );
    println!("            skipped {}, warnings {}", summary.skipped, summary.warning_count);
    println!(
        "size        PGN {:.1} MB -> DB {:.1} MB ({:.1} bytes/game)",
        pgn_size as f64 / 1e6,
        info.size_bytes as f64 / 1e6,
        info.size_bytes as f64 / summary.imported.max(1) as f64
    );
    Ok(())
}

fn run(db_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let t = Instant::now();
    let db = Mutex::new(Database::open(db_path)?);
    println!("open        {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    {
        let db = db.lock().unwrap();
        let ids = page_ids(&db, SortSpec::NATURAL, 0, 200)?;
        db.rows(&ids)?;
    }
    println!("first page  {:.2} ms", t.elapsed().as_secs_f64() * 1e3);

    let spec = SortSpec::new(SortKey::White, false);
    let t = Instant::now();
    build_sort(&db, spec, &NoProgress)?;
    println!("sort White  {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    {
        let db = db.lock().unwrap();
        let count = db.count();
        let ids = page_ids(&db, spec, count.saturating_sub(200), 200)?;
        db.rows(&ids)?;
    }
    println!("sorted page {:.2} ms (last 200 rows)", t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    let n = {
        let db = db.lock().unwrap();
        let count = db.count().max(1);
        let n = 1000.min(count);
        for i in 0..n {
            db.read_game((u64::from(i) * 7919 % u64::from(count)) as u32)?;
        }
        n
    };
    println!("read game   {:.1} µs avg ({n} random games)", t.elapsed().as_secs_f64() * 1e6 / f64::from(n));

    for (label, query) in [
        ("quick filter", HeaderQuery { text: "magnus".into(), ..HeaderQuery::default() }),
        ("  again", HeaderQuery { text: "carl".into(), ..HeaderQuery::default() }),
        (
            "header search",
            HeaderQuery { elo_min: Some(2500), result: Some(ResultFilter::Draw), ..HeaderQuery::default() },
        ),
    ] {
        let t = Instant::now();
        let found = search_headers(&db, &query, None, &NoProgress)?.len();
        println!("{label:<14}{:.0} ms ({found} games)", t.elapsed().as_secs_f64() * 1e3);
    }

    let t = Instant::now();
    let summary = update_position_index(&db, &NoProgress)?;
    let size = db.lock().unwrap().positions().size_bytes();
    println!(
        "pos index   {:?}: {} games, {} positions in {:.1}s, {:.0} MB",
        summary.plan,
        summary.games,
        summary.positions,
        t.elapsed().as_secs_f64(),
        size as f64 / 1e6
    );
    for (label, line) in [
        ("tree start", &[][..]),
        ("tree 1.e4", &["e2e4"][..]),
        (
            "tree Najdorf",
            &["e2e4", "c7c5", "g1f3", "d7d6", "d2d4", "c5d4", "f3d4", "g8f6", "b1c3", "a7a6"][..],
        ),
    ] {
        let mut pos = Chess::default();
        for u in line {
            let m = parse_uci(&pos, u).ok_or("bad move")?;
            pos.play_unchecked(m);
        }
        let t = Instant::now();
        let tree = position_tree(&db, &pos)?;
        println!(
            "{label:<12}{:.1} ms ({} games, {} moves)",
            t.elapsed().as_secs_f64() * 1e3,
            tree.total.games,
            tree.moves.len()
        );
    }

    // The Reference tab (OPEN-14): the fast part, then the lines, which
    // read every game reaching the position.
    for (label, line) in [
        ("start", &[][..]),
        ("1.e4", &["e2e4"][..]),
        ("1.d4 d5 2.c4", &["d2d4", "d7d5", "c2c4"][..]),
        ("Sicilian 2.Nf3 d6", &["e2e4", "c7c5", "g1f3", "d7d6"][..]),
        ("Najdorf", &["e2e4", "c7c5", "g1f3", "d7d6", "d2d4", "c5d4", "f3d4", "g8f6", "b1c3", "a7a6"][..]),
    ] {
        let mut pos = Chess::default();
        for u in line {
            let m = parse_uci(&pos, u).ok_or("bad move")?;
            pos.play_unchecked(m);
        }
        let t = Instant::now();
        let overview = reference_overview(&db, &pos)?;
        let fast = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let hits = find_position(&db, &pos)?;
        let lines = reference_lines(&db, &pos, &hits, &NoProgress)?;
        println!(
            "reference {label:<18} overview {fast:.1} ms, lines {:.0} ms ({} games, {} lines, main line {} plies)",
            t.elapsed().as_secs_f64() * 1e3,
            overview.total.games,
            lines.lines.len(),
            lines.main_line.map_or(0, |l| l.moves.len())
        );
    }

    if let Some(mb) = peak_rss_mb() {
        println!("peak RSS    {mb:.0} MB");
    }
    Ok(())
}
