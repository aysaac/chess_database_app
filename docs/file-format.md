# Database File Format (version 3)

Native storage for games (catalog item DB-01). It's designed for fast
append-only imports, instant access to any game by id, and crash safety. The
code lives in `crates/chess-db`; this document is the reference for it.

All integers are **little-endian**. `varint` is unsigned LEB128. `str` is
`varint length` followed by that many UTF-8 bytes.

## Layout

A database is a directory named `<Name>.chessdb/`:

| File | Contents |
|---|---|
| `db.meta` | Text: `chessdb 3` on line 1, `id <32 hex chars>` on line 2 |
| `games.idx` | 64-byte header, then one 64-byte record per game |
| `games.blob` | 16-byte header, then game blobs (append-only) |
| `games.ids` | 16-byte header, then each game's permanent id (see [below](#permanent-game-ids)) |
| `games.origins` | 16-byte header, then the origin of each game copied in from another database |
| `players.names` | Name table for White/Black |
| `events.names` | Name table for Event |
| `sites.names` | Name table for Site |
| `annotators.names` | Name table for Annotator |
| `.lock` | Empty; locked exclusively while the database is open |
| `positions-<gen>.idx` | Position index segments (derived, see [below](#position-index-srch-04)) |
| `positions.dirty` | Games replaced since they were indexed |
| `openings.idx` | Opening classification per game (derived) |

**Versions.** `db.meta` version 3 means every game has a permanent id in
`games.ids`. Version 2 meant the database keeps `positions.dirty` up to date
when a game is replaced; version 1 came before the position index. Both open
fine and are upgraded to 3 on open ([the upgrade](#the-upgrade-to-version-3)).
Once a newer app has opened a database, an app from before version 3
refuses it ("unsupported format version 3"), in its library too, and
changes nothing: it would add, copy or compact games without their ids, and
a compaction would leave each id on another game. The binary files keep
their own header versions (all 1).

A game's **id** in the code and the UI (`GameId`) is its **slot**: its
zero-based position in `games.idx`. Deleting a game only sets a flag, but a
compaction (DB-06) renumbers the slots. Its **permanent id** never changes
(see [below](#permanent-game-ids)).

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
the original. It gets its own permanent id there, and the original's is
recorded as its origin (`games.origins`). Copies go in batches of 2,000
games, each a normal crash-safe commit; a move marks the originals as
deleted only after their copies are committed, so a crash in between leaves
duplicates, never a lost game.

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

## Permanent game ids

Every game has a permanent id: 16 random bytes from the OS's random source,
laid out as a UUID version 4 (RFC 9562) and written as one in text
(`1b4e28ba-2fa1-4d3b-a3f5-ef19b5a7633b`). With 122 random bits it's unique
across databases, and it's never reused: a game keeps it through edits,
deletion, undeletion and compactions, and a game a compaction removes takes
its id with it. It's the groundwork for NET-13 (games kept in step with a
Lichess study's chapters) and for editing a server's games from the app.
Code: `crates/chess-db/src/uid.rs`.

Slots stay what everything else uses: the game list, sorts, searches, the
position index, the Reference tab, cross-tables and duplicates. Permanent ids
are for the edges (links, sync state, an API), which find a game's slot
through a map built on first use: it reads `games.ids` and keeps 8 bytes a
game in memory (an id's first 4 bytes and its slot, sorted; a lookup reads
the candidate's whole id from the file), and follows appends and compactions
until the database is closed.

PGN never carries ids: export and import don't need them, and the lossless
round trip is unchanged. Importing a game (PGN or `.cbh`), also one imported
before, gives it a new id.

### `games.ids`

A 16-byte header (magic `CHDBUID1`, version `1`, zero padding), then one
16-byte id per slot, in slot order: as many as the committed count says (see
[Crash safety](#crash-safety)).

### `games.origins`

A 16-byte header (magic `CHDBORG1`, version `1`, zero padding), then 32-byte
entries: a game's permanent id, then its origin's. Copying games into the
database (DB-06, QOL-04; `copy_games`, or a `PreparedGame` given an origin)
appends one per copy, naming the id of the game copied, in the database it
came from: a copy of a copy names the copy. Games imported, entered or
saved there have none. The last entry for an id wins (there is only ever one
so far). A compaction keeps the entries of the games it keeps.

It's the first table keyed by permanent id. NET-13's link from a game to a
study chapter and the base snapshot for merging can be kept the same way,
in their own table, without touching `games.idx`.

### The upgrade to version 3

Opening a version 1 or 2 database:

1. Writes `games.ids.tmp` with a new id for every committed game (deleted
   ones too), fsyncs it and renames it to `games.ids`.
2. Writes an empty `games.origins` the same way.
3. Writes `db.meta` with version 3 (temp file + rename).

No id is handed out before step 3. After a crash before it, the database is
still version 2 and the next open does it all again, replacing a
`games.ids` already there (an older app may have changed the games since).
`*.tmp` files left over are deleted on open. What it costs is under
[Measured](#measured-prototype-release-build-sep-2026).

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
3. Write the new games' permanent ids after the committed ones in
   `games.ids`, then fsync. For copies, append their origins to
   `games.origins`, then fsync.
4. Write the index records after the committed ones, then fsync.
5. Write the new committed count in the `games.idx` header, then fsync.

Opening a database:

- Index bytes and ids beyond the committed count are truncated.
- A torn entry at the end of a name table or of `games.origins` is
  truncated. A name can't be referenced, because records are written after
  names. A whole origin entry whose id no game has is never looked up, and
  the next compaction drops it.
- Blob bytes nobody references are harmless.
- `games.ids` holding fewer ids than there are games, or missing (no crash
  does this: ids are synced before the count), gives the games without one
  new ids, with a warning, and the database opens. A missing
  `games.origins` is made again, empty.

In-place updates (soft delete, repointing a replaced game) rewrite a single
64-byte record, then fsync. They keep the game's slot, so its permanent id.

Compacting (DB-06) writes the new files next to the old ones as `*.new`:
`games.idx`, `games.blob` and the name tables, `games.ids` (the kept games'
ids, in their new slots) and `games.origins` (the kept games' entries, when
there are any), and fsyncs them. Then `compact.commit` is written, the `.new`
files are renamed over the old ones and the marker is removed. Opening a
database finishes the renames if the marker exists, and deletes `.new` files
otherwise.

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

Tournament cross-tables (DB-16) read only the index records: one scan for
the event's games, then the standings in memory. Release build: all 55
TWIC 1600 events in 40 ms together. In the app (`tauri dev`, Oct 2026), from
the click to the table on screen: a 10-player round robin 50 ms, the
597-player Titled Tuesday Swiss 260–470 ms; on the 1M Lichess games, the
"Rated Blitz game" month (428,886 games, too big for a table) 253 ms and a
1,446-game arena 278 ms. What the table costs the webview grows with its
cells, 40–50 µs each on the smoke harness's headless display (two frames
after the click): a 600-player 11-round Swiss (19,200 cells) 0.9–1.1 s, a
2,000-player 7-round one (48,000) 2.1–2.8 s. So `MAX_CELLS` is 20,000 and
bigger tables get the standings only: 2,000 players 0.6–0.7 s, "Rated
UltraBullet game" (1,130 players without rounds, one with 138 games: 3.3 s
as a full table) 0.4 s.

Permanent ids (format version 3; release build, Nov 2026, page cache warm,
on a laptop in use): `import_bench` built from before them (`main` at
`118a278`) and after, run in turns on Mega Database 2019 (7,623,531 games)
and the first 1M Lichess games:

| | Mega Database 2019 | Lichess, 1M games |
|---|---|---|
| Size | 1,353.1 → 1,475.1 MB: +16 bytes a game and 32 bytes of headers | 697.3 → 713.3 MB |
| First open of a version 2 database (the upgrade) | 361 ms | — |
| Open, before → after | 94, 107, 113 → 81–88, 110, 111 ms | 483, 449 → 449, 450 ms |
| Import, before → after | 370.8 → 393.6 s (one run each) | 51.9, 49.9 → 50.7, 50.8 s |
| Map of ids: first lookup, memory | 131–149 ms, 61 MB | 17 ms, 8 MB |
| A lookup after | 1.3–1.6 µs | 0.7–0.9 µs |

The import's extra fsync per batch and the ids it writes are within the
noise on Lichess, where the runs alternated. Lists, sorts, searches, trees
and the Reference tab don't touch the ids and measure the same: on Mega,
sort by White 485 → 497 ms, header search 141 → 140 ms, tree at the start
261 → 231 ms, Reference lines at the start 9.2 → 8.9 s (second runs).
