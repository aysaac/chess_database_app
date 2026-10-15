# crates/chess-db: the native database

What's specific to the database crate; the root [AGENTS.md](../../AGENTS.md)
still applies. The on-disk format is specified in
[`docs/file-format.md`](../../docs/file-format.md), ChessBase import (IO-03)
in [`docs/io-03-cbh.md`](../../docs/io-03-cbh.md).

## Storage format rules

- Any change to the on-disk layout must bump the format version, update
  `docs/file-format.md`, and keep crash safety: write data, fsync, then
  publish (the committed count is written last).
- A move byte is the move's index in the legal moves **sorted by
  (from, to, promotion)** as defined in `blob.rs`, never shakmaty's
  generation order.
- Import must stay lossless: `cargo test --release -p chess-db -- --ignored`
  re-imports an export and compares blob bytes.

## Commands

Run from the repo root. The big PGNs come from `scripts/fetch-testdata.sh`
(into `testdata/large/`, git-ignored).

```sh
# Performance: catch regressions with import_bench
cargo run --release -p chess-db --example import_bench -- testdata/large/lichess_1m.pgn
cargo run --release -p chess-db --example import_bench -- path/to/existing.chessdb   # skip the import

# Lossless import → export → import, on TWIC or any PGN
cargo test --release -p chess-db -- --ignored
LARGE_PGN=path/to.pgn cargo test --release -p chess-db -- --ignored

# ChessBase databases (IO-03; not committed: link or copy them into testdata/large/)
cargo run --release -p chess-db --example import_bench -- "testdata/large/Mega Database 2019.cbh"
cargo test --release -p chess-db --test cb_import -- --ignored   # CBH_SAMPLES=dir, CBH_MEGA=file.cbh
cargo run --release -p chess-db --example cb_decode_stats -- "path/to/Base"  [STEP | show ID]
```

## Helpers (`examples/`)

- `import_bench.rs`: imports a PGN or a `.cbh` and times open, paging,
  sorting and random game reads. Use it to catch performance regressions.
- `cb_decode_stats.rs`: decodes every game (or every STEP-th) of a
  ChessBase database and counts the outcomes by message; `show ID` prints
  one game as PGN. The first stop when a `.cbh` imports badly.
- `extract_cb_key.rs`: finds the `.cbg` translation table in a ChessBase
  install and checks it against real databases (how the built-in table was
  made; `docs/io-03-cbh.md` §3.2).

## Gotchas

- **`.cbh` counts a game's main line in full moves, not plies** (and
  modulo 256 past 255). Checking decoded games against plies made the right
  translation table look wrong for a whole session; use
  `GameRecord::main_line_matches`.
- TWIC rejects `curl` without browser-like headers (HTTP 406). The fetch
  script sends them.
