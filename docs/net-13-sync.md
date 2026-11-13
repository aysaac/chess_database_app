# NET-13: what a linked game holds on each side

NET-13 keeps games in a database in step with a Lichess study's chapters
(`docs/feature-catalog.md`, NET-13). This note covers the part built so
far: the link and its base snapshot (`chess-db`), and how a linked game is
split between what Lichess holds and what only the database holds
(`chess-net`). Pulling, merging and pushing come later.

**The user's decision (2026-11-12):** Lichess doesn't keep everything. A
linked game keeps in the program everything Lichess can't hold, attached to
the same moves, and only what Lichess can hold goes to Lichess and comes
from it. Three modes: a static download (no link, as today), follow only,
and two-way sync. Each linked game has a base snapshot, which is
**Lichess's own export of the chapter** (fetched again after a push), so
that what Lichess rewrites never looks like an edit made there. The
three-way merge on the move tree comes later.

## Where it lives

| What | Where |
|---|---|
| The link: study and chapter ids, mode, the study's `Last-Modified`, the base snapshot, the local-only layer | `games.links` in the database, keyed by permanent id (`crates/chess-db/src/links.rs`; `docs/file-format.md` › `games.links`) |
| The split: `lichess_projection`, `local_only_layer`, `apply_layer`, `normalize_for_compare`, `is_lichess_addition` | `chess_net::study::layer` (`crates/chess-net/src/study/layer.rs`) |

The split is in `chess-net`, not `chess-core`, because it is knowledge of
Lichess (which NAGs and tags it keeps, the result comment it adds), next to
the round-trip check that found it out (`study::roundtrip`), and because
the layer is stored in the database's own binary form (`chess_db::bytes`,
moves as `chess_db::blob` names them), which `chess-core` can't use. It
makes no requests.

## What Lichess holds

From the round-trip check on lichess.org (2026-11-12,
`docs/feature-catalog.md` › NET-13 round-trip check › On lichess.org; the
games are `testdata/pgn/study-roundtrip.pgn`). `lichess_projection` is the
game cut down to this, as our PGN writer sends it and our reader reads it
back:

| | Lichess holds | The projection | Held here only (the layer) |
|---|---|---|---|
| Moves, variations, their order, the start position | All, as sent | As they are | — |
| Comment before a move (a variation's first move) | Dropped | None | All of them |
| Other comments (the game's first one too) | Kept, text as sent; it keeps our line breaks | `}` as `)` and every run of spaces or line breaks one space (what our writer and reader make of it) | The text as written here, where that differs |
| NAGs | `$1 $2 $4 $5 $6 $7 $13 $14 $18 $22 $36 $40 $44 $132 $140 $146` kept, `$142` dropped | Those on `LICHESS_NAGS`, once each, in their order; none at the start | Every NAG of the move, in order, where one isn't held |
| Arrows, highlights (`%cal`, `%csl`) | All, four colours | All, highlights first (as our writer puts them) | Their order, where it was another |
| Clocks (`%clk`) | Rounded to the second (`0:04:57.5` → `0:04:58`) | To the nearest second, half up, as `h:mm:ss`; a clock Lichess can't read (not `h:mm:ss[.f]`) left out | Every command of the move, in order, where a clock is written otherwise |
| Evals, `%emt`, other `[%…]` | Dropped (an `%emt` becomes a clock, see below) | None | Every command of the move, in order |
| Event, Site, Date, Round, players, ratings, Result, WhiteTitle, BlackTitle, TimeControl, Termination, Orientation | Kept as sent | Kept, the tags beside the roster in `LICHESS_TAGS`'s order | Every tag's value and order |
| ECO, Opening | Lichess's own, from its book (`?` from a position) | None | Ours (Lichess's ignored) |
| Annotator | Kept, or `https://lichess.org/@/{owner}` where none | None | Ours, or none |
| EventDate, any other tag | EventDate dropped; others not tried | None | Ours |
| StudyName, ChapterName, ChapterURL | The study's | None | Ours, if the game has them |
| A game without Orientation | `white` added | None | That it had none |
| Result comment | `{ 1-0 White wins. }`, `{ 1/2-1/2 The game is a draw. }` added on the main line's last move of a finished game with no comment there | — | Whether the game's own last comment is one |

Not tried on lichess.org, so treated as **not held** until a run shows
otherwise (`LICHESS_NAGS` and `LICHESS_TAGS` are one constant each): every
other NAG (`$3`, `$10`, `$15`, `$16`, `$17`, `$19`, `$32`, `$138` among
them), every other tag (`UTCDate`, `PlyCount`, `WhiteFideId`…), two NAGs
of one kind on a move, clocks with fewer than three parts. `0-1 Black
wins.` is assumed from the other two.

## The local-only layer

`local_only_layer(game)` is what the projection took away, keyed by
**move path**: the moves from the start position to the node, so it
survives variations reordered and lines added on Lichess. A node has an
entry only if the projection changed it, and the entry keeps the node's
field in full where it changed (all its NAGs, all its commands, in order),
so that putting it back is exact. It also keeps the start position's FEN
(paths start there), ECO and Annotator, every other tag with its value in
its order, and whether the game's last main-line comment is one like
Lichess's result comment.

`apply_layer(pulled, layer)` puts it back on a game pulled from Lichess:

- **Each entry on the node its path leads to.** Field by field: where what
  Lichess holds of it is as here (the same comment once spacing and `}`
  are set aside; the same NAGs it holds, in any order; the same shapes;
  each clock within a second), the node gets this database's exactly.
  Where it changed on Lichess, Lichess's stays and only what Lichess can't
  hold is added (NAGs not on its list, commands other than clocks, the
  comment before the move).
- **Orphans, never dropped:** an entry whose move is gone (deleted on
  Lichess), every entry if the game starts from another position now, and
  a comment before a move where the pulled game has one already, come back
  in `Applied::orphaned` with what Lichess can't hold of them and the line
  in SAN. The caller decides.
- **Lichess's own additions are left out:** its result comment on the
  last move (unless the game's own last comment was one), its ECO, Opening
  and Annotator (ours put back, or none), its study tags (ours, if any),
  and the `white` it gives a game without an orientation. A tag it holds
  that changed there comes over; one removed there goes.

**Exact, with no allowance:** `apply_layer(&lichess_projection(&g),
&local_only_layer(&g)) == g` for any game, clocks to the tenth and
comments' spacing included, because the layer keeps them as written. The
tests check that for random trees (comments before variations' first
moves, NAGs held or not, shapes in any order, clocks in any form, evals,
other commands, tags in any order, set-up positions), with variations
reordered and a line added on the Lichess side, and through an emulation of
what lichess.org did, written and read with our PGN writer and reader both
ways (`study::layer` tests). Allowances only come where Lichess changed a
field: then its spacing and its rounding are what there is.

`normalize_for_compare(game)` is what the later merge compares: the
projection, without Lichess's result comment on the last move, with
`Orientation` lower case and `white` where there's none, NAGs and shapes
sorted and each once. A game sent as its projection compares equal to
Lichess's export of it, and two exports of an unchanged chapter compare
equal. `is_lichess_addition(comment)` says whether a comment is Lichess's
result comment, whatever its spacing.

**Send the projection, never the whole game.** Lichess makes a clock from
an `%emt` where a move has none (`[%clk 0:04:54]`: the player's clock
before, less the time, plus the increment). The round-trip check sent games
whole and got that clock; a projection has no `%emt`, so Lichess makes
none. A game linked after being sent whole (NET-12 as it is) gets that
clock on its first pull (`apply_layer` keeps a clock Lichess has where we
had none, next to our `%emt`), and `normalize_for_compare` can't tell it
from an edit.

### Stored

`Layer::to_bytes` writes, in the database's own binary form (little
endian, `varint` LEB128, `str` its length then UTF-8; `bytes` a `varint`
length then the bytes), into the link's `layer` (`games.links`):

```text
layer   := version:u8(=1) flags:u8 [setup:str]      -- flags & 1: setup FEN follows; & 2: own result comment
           eco:str annotator:str n_tags:varint (name:str value:str)*
           n_nodes:varint node*
node    := path:bytes fields:u8 [before:str] [after:str] [nags:bytes] [shapes] [commands]
           -- fields & 1 before, 2 after, 4 nags, 8 shapes, 16 commands
path    := one u8 a move: its index in the position's legal moves sorted by
           (from, to, promotion), as a game blob names moves
shapes  := n:varint (color:u8 from:u8 to:u8)*          -- as a game blob: G/R/Y/B, to 0xFF a highlight
commands:= n:varint (name:str value:str)*
```

Nodes are in the game's order, so each path shares its first moves with
the one before and is played from there. A path that isn't legal from the
start, a path twice, unknown flags or fields, or bytes after the end are
`Corrupt`; another version is refused as such.

## Pushing (not built): an open question and a proposal

**The question.** The push endpoint (`POST
/api/study/{id}/{chapter}/moves`) replaces the chapter's whole move tree.
Our PGN reader makes any run of spaces or line breaks one space and joins a
move's comments with a space; our writer wraps long comments at 80
columns, and Lichess keeps those breaks as part of the comment. So pushing
a game can change comments we didn't change: a line break typed on Lichess
becomes a space, two comments of a move become one, and our wrapping's
breaks appear in the text. Whether Lichess keeps several comments on one
move, or line breaks typed there, wasn't tried.

**Proposal** (not built; for the push step):

1. Push only what changed here: when the merge finds a difference between
   `normalize_for_compare` of the local game now and of the local game at
   the last sync (the base snapshot with the stored layer applied). Never
   push to tidy up.
2. Send `write_game(&lichess_projection(&game))`, never the whole game.
3. Keep Lichess's own comment texts where we didn't change them. From the
   base snapshot (raw PGN, as Lichess sent it), read each node's raw
   comments by path with a reader that doesn't flatten them; when writing
   the push, a node whose comment is unchanged since the base (equal once
   normalized) gets the base's raw comments written as they were, not
   ours. That needs a writer option to write a comment as one token (no
   wrapping inside it) and to take raw comments by path.
4. After the push, fetch the chapter again and store that export as the
   new base, with the layer of the game as pushed.
5. Tags go through `POST …/{chapter}/tags` with only those that changed;
   never ECO, Opening or Annotator.

Until then, a push may reflow comments on Lichess. Follow only never
pushes.

## Open questions

1. **NAGs and tags not tried** are held here only: if a collaborator puts
   `!!` (`$3`) on a move on Lichess, a pull brings it, and a push would send
   the game without it (it isn't on `LICHESS_NAGS`), so it would go on
   Lichess. lila's glyph list probably has `$3 $10 $15 $16 $17 $19 $32
   $138` too (from memory, not checked). One more round trip with them, or
   taking lila's list, settles it.
2. **A chapter renamed on Lichess** doesn't rename the game's ChapterName
   here (study tags are ours). The link has the ids, so nothing breaks;
   say if a rename should come over.
3. **A linked game deleted here and compacted** loses its link with it, so
   the next pull of its study would bring the chapter back as a new game.
   Keep a mark of deleted chapters, push the deletion (two-way), or accept
   it?
4. **DB-08's duplicate deletion** may keep an unlinked copy and delete the
   linked one; it should prefer keeping linked games.
5. **Orientation**: a missing one is taken as Lichess's `white`; only
   `white` was seen added, to three games from the usual start. Whether
   Lichess picks `black` for some games (from a position with Black to
   move?) wasn't tried.
6. **Clock rounding** is half up from one sample (`.5` → up); `.1`–`.4`
   weren't tried. `apply_layer` takes a clock within a second as the same,
   so either way ours come back.
