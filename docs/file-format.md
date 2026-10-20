# Database File Format (version 2)

Native storage for games (catalog item DB-01). It's designed for fast
append-only imports, instant access to any game by id, and crash safety. The
code lives in `crates/chess-db`; this document is the reference for it.

All integers are **little-endian**. `varint` is unsigned LEB128. `str` is
`varint length` followed by that many UTF-8 bytes.

## Layout

A database is a directory named `<Name>.chessdb/`:

| File | Contents |
|---|---|
| `db.meta` | Text: `chessdb 2` on line 1, `id <32 hex chars>` on line 2 |
| `games.idx` | 64-byte header, then one 64-byte record per game |
| `games.blob` | 16-byte header, then game blobs (append-only) |
| `players.names` | Name table for White/Black |
| `events.names` | Name table for Event |
| `sites.names` | Name table for Site |
| `annotators.names` | Name table for Annotator |
| `.lock` | Empty; locked exclusively while the database is open |
| `positions-<gen>.idx` | Position index segments (derived, see [below](#position-index-srch-04)) |
| `positions.dirty` | Games replaced since they were indexed |
| `openings.idx` | Opening classification per game (derived) |

**Versions.** `db.meta` version 2 means the database keeps `positions.dirty`
up to date when a game is replaced. Version 1 databases (from before the
position index) open fine and are upgraded to 2 on open; nothing else
changes. The binary files keep their own header versions (all 1).

A game's **id** is its zero-based position in `games.idx`. Ids never change;
deleting a game only sets a flag.

## `games.idx`

Header (64 bytes):

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | Magic `CHDBIDX1` |
| 8 | 4 | Format version (`1`) |
| 12 | 4 | Record size (`64`) |
| 16 | 8 | **Committed game count** |
| 24 | 40 | Reserved (zero) |

Record (64 bytes, one per game):

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | Blob offset in `games.blob` |
| 8 | 4 | Blob length |
| 12 | 4 | Flags: `1` deleted, `2` has variations, `4` has comments, `8` starts from a set-up position, `16` has symbols or arrows |
| 16 | 4 | White (player name id) |
| 20 | 4 | Black (player name id) |
| 24 | 4 | Event (event name id) |
| 28 | 4 | Site (site name id) |
| 32 | 4 | Annotator (annotator name id) |
| 36 | 4 | Date: `year << 9 \| month << 5 \| day`; 0 means that part is unknown |
| 40 | 2 | Round (0 = unknown) |
| 42 | 2 | Sub-round, e.g. board number in `3.1` (0 = none) |
| 44 | 2 | White Elo (0 = unknown) |
| 46 | 2 | Black Elo (0 = unknown) |
| 48 | 1 | Result: 0 unknown, 1 white wins, 2 black wins, 3 draw |
| 49 | 1 | Reserved |
| 50 | 2 | ECO: `(letter − 'A' + 1) × 100 + number` (0 = unknown) |
| 52 | 2 | Main-line length in plies (saturating) |
| 54 | 10 | Reserved (zero) |

Records hold everything the game list shows, so listing, sorting and paging
never read `games.blob`.

## Name tables (`*.names`)

A 16-byte header (magic `CHDBNAME`, version `1`, zero padding), then entries
`str` back to back. An entry's id is its ordinal. **Id 0 is always the empty
string**, meaning "unknown". Names are deduplicated exactly, case-sensitively;
spelling cleanup (DB-09) will build on these tables.

## `games.blob`

A 16-byte header (magic `CHDBBLOB`, version `1`, zero padding), then blobs at
the offsets given in the records. Replacing a game appends a new blob and
repoints its record. Old blobs are unreferenced garbage until the database is
compacted (DB-06).

A blob holds no name-table ids, so it means the same in any database:
copying or moving games to another database (DB-06, QOL-04) appends the
blob bytes unchanged and writes a new record whose names are looked up (or
added) in the target's tables. Nothing is decoded, and the copy is exactly
the original. Copies go in batches of 2,000 games, each a normal crash-safe
commit; a move marks the originals as deleted only after their copies are
committed, so a crash in between leaves duplicates, never a lost game.

Deleting duplicate games (DB-08) gives the same guarantee. Each kept game
first takes the fields of its index record that it lacks from the copies
about to be deleted (ratings, round and board, ECO, date parts, event, site,
result): a new blob with its new Date, Round and ECO strings and everything
else byte for byte the same, appended like a replaced game's, then its new
record. Both are fsynced before any copy is marked as deleted (see
[Crash safety](#crash-safety)). The format can't commit several records at
once (only an append is atomic across games), so after a crash the database
holds one of: nothing done; the kept games filled in and their copies still
there (nothing lost: deleting them again fills in nothing more); or the kept
games filled in and some copies deleted. Never a deleted copy whose fields
its kept game lacks. Restore marks the copies as not deleted, then points
each kept game back at its earlier record (whose blob stays until a
compaction) if the game is unchanged since. That undo lives in memory, like
the list of deleted copies: once the database is closed, Restore in the game
list still brings the copies back, but the kept games keep what they took.

### Game blob

```text
blob       := version:u8(=1) flags:u8 [setup_fen:str]
              date:str round:str eco:str
              n_extra:varint (tag_name:str tag_value:str)*
              moves END_GAME(0xFD)
              n_annotations:varint annotation*
moves      := (move variation*)*
variation  := BEGIN_VAR(0xFF) move moves END_VAR(0xFE)
move       := u8
```

- `flags & 1` means a `setup_fen` follows.
- The exact PGN strings of Date, Round and ECO are kept here; the record holds
  only the packed, sortable forms. Other PGN tags are kept in order as
  `extra`. White, Black, Event, Site, Annotator, Elo and Result live only in
  the record.
- **Move encoding:** a move byte is the move's index in the position's legal
  moves sorted by `(from square, to square, promotion piece)`, with castling
  as king square to rook square. The sort order is defined by this format,
  not by the move generator, so it stays stable across library versions. A
  position has at most 218 legal moves, so indices never collide with the
  marker bytes.
- **Tree order:** after each main-line move come its alternatives, each
  wrapped in `BEGIN_VAR … END_VAR`, and then the main line continues. This is
  PGN order, and decoding is iterative along main lines.

### Annotations

Nodes are numbered in the order they appear in the byte stream: the root
(start position) is 0, then each move in turn. Each annotation is
`node:varint kind:u8 payload`:

| Kind | Payload | Meaning |
|---|---|---|
| 1 | `str` | Comment after the move (for node 0: comment before the first move) |
| 2 | `str` | Comment before the move (first move of a variation) |
| 3 | `varint n`, `n × u8` | NAGs (`$1` = `!`, `$14` = `⩲`, …) |
| 4 | `varint n`, `n × (color:u8 from:u8 to:u8)` | Arrows and squares; color is ASCII `G`/`R`/`Y`/`B`, `to = 0xFF` marks a square |
| 5 | `name:str value:str` | Any other `[%name value]` comment command, kept verbatim |
| 6 | `which:u8 deciseconds:varint` | `[%clk]` (`which = 0`) or `[%emt]` (`1`) |
| 7 | `kind:u8 value:zigzag-varint` | `[%eval]`: centipawns (`kind = 0`) or mate in N (`1`) |

Kinds 6 and 7 are compact forms for the per-move clock and eval data in
online games. They are used only when re-rendering reproduces the original
text exactly (`0:03:00`, `0.17`, `#-9`). Anything else falls back to kind 5,
so export stays lossless.

## Position index (SRCH-04)

Derived data: every file below can be deleted at any time and is rebuilt by
the next index update (the app runs one after opening a database and after
each import). Code: `crates/chess-db/src/positions.rs`.

### Segments: `positions-<gen>.idx`

For every game and every main-line position from ply 0 (the start) up to
`max_ply` (50 by default), one 16-byte entry, sorted by `(key, game, ply)`:

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | Polyglot Zobrist hash of the position (`chess_core::position::hash`) |
| 8 | 4 | Game id |
| 12 | 2 | Ply of the position in the game |
| 14 | 1 | Move played next, as a move byte (see the blob format); `0xFF`: the game ends here |
| 15 | 1 | Result, as in the index record |

Header (64 bytes): magic `CHDBPOS1`, version `1`, entry size `16`, `max_ply`
(u32), first game id (u32), end game id (u32, exclusive), entry count (u64),
zero padding. After the entries comes the fence table: the key of every
512th entry (u64 each), which is loaded into memory. A lookup binary-searches
the fences and reads only the blocks that can hold the key.

Each segment covers a range of game ids; together they cover `0..covered`
without gaps. An update indexes the games added since (`covered..count`) as a
new segment. With 6 segments, more than 4096 dirty games, or a changed
opening table, the next update rebuilds everything into one segment instead.
`<gen>` increases with every segment written.

Building: games are read in batches of 4096 and decoded on up to 8 threads
(main line only, see `blob::mainline`); entries are sorted in runs of 4M
(64 MB) spilled to `positions.run<N>.tmp`, then merged into
`positions-<gen>.idx.tmp`, fsynced and renamed into place. On open, `*.tmp`
leftovers are deleted, a segment overlapping a newer one is deleted (a crash
between writing a rebuilt segment and deleting the old ones), and segments
after a gap in coverage are dropped.

### `positions.dirty`

Game ids (u32 each) replaced since their segment was built. `replace()`
appends the id and fsyncs *before* rewriting the game's index record, so the
index never trusts entries of a changed game. A kept duplicate taking fields
from its copies (DB-08) keeps its moves, so it is added only if its result
changes (entries hold the result), and so is one whose fill Restore undoes. Entries of dirty games are
ignored; dirty games and games not covered yet are replayed at query time
(up to 20,000 of them; beyond that results are marked partial until the next
update). A rebuild clears the list, keeping ids added while it ran.

### `openings.idx`

Magic `CHDBOPN1`, the opening table's data version (u32, a hash of
`chess-core/data/openings.tsv`), 4 zero bytes, then one u16 per game: the
1-based id of its opening in that table, 0 for none. Written by the same
pass as the segments; ignored (and rebuilt) when the data version differs.

## Crash safety

Appending a batch of games (an import commits one batch every ~300 ms):

1. Write the blobs at the end of `games.blob`, then fsync.
2. Append new names to the name tables, then fsync.
3. Write the index records after the committed ones, then fsync.
4. Write the new committed count in the `games.idx` header, then fsync.

Opening a database:

- Index bytes beyond the committed count are truncated.
- A torn entry at the end of a name table is truncated. It can't be
  referenced, because records are written after names.
- Blob bytes nobody references are harmless.

In-place updates (soft delete, repointing a replaced game) rewrite a single
64-byte record, then fsync.

Deleting duplicates (DB-08), a batch of groups at a time (the app takes up to
16,384 copies a batch):

1. Append the filled-in kept games' new blobs to `games.blob`, then fsync.
2. Append to `positions.dirty` the kept games whose result changes, then
   fsync.
3. Rewrite the kept games' records, then fsync.
4. Mark the copies as deleted (their records' flag), then fsync.

Each record is written whole, so a crash leaves each kept game with its old
record or its filled-in one, and each copy deleted or not; a copy is only
marked once its kept game's record is on disk. Restore runs it backwards:
the copies first, then the kept games' earlier records.

Only one process may have a database open at a time: `.lock` is held with
`File::try_lock` (Rust ≥ 1.89, works on Linux, macOS and Windows).

## Measured (prototype, release build, Sep 2026)

| Data | Import | Size | Open | Sort 1M by White | Read one game |
|---|---|---|---|---|---|
| TWIC 1600 (8,895 games, 7.9 MB PGN) | 0.42 s | 2.7 MB | < 1 ms | 1 ms | ~70 µs |
| Lichess Jan 2024, first 1M games (2.3 GB PGN with clocks and evals) | 48.9 s | 697 MB | 478 ms | 117 ms | ~65 µs |

Peak memory during the 1M-game import: 310 MB.

Position index on the same 1M games (release build, 8 threads, Sep 2026):

| Build | Entries | Size | Tree at the start (1M games) | Tree after 1.e4 | Tree after 5...a6 (Najdorf) |
|---|---|---|---|---|---|
| 12.3 s | 46.2M | 740 MB | 34 ms (first use, loads the per-game stats) | 6.7 ms | 0.1 ms |

The index is about as large as the games themselves for online games (46
positions per game on average). Ways to shrink it if 10M-game databases
matter: fewer plies (`max_ply` is stored per segment), or storing positions
reached by a single game in a compact side table.
