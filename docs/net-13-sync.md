# NET-13: a database kept in step with a Lichess study

NET-13 keeps games in a database in step with a Lichess study's chapters
(`docs/feature-catalog.md`, NET-13). This note covers the link and its base
snapshot (`chess-db`), how a linked game is split between what Lichess
holds and what only the database holds (`chess-net`), and the sync that
the Sync button runs: the three-way merge, pulling, pushing, new chapters
both ways, conflicts left for the user ([The sync](#the-sync), built
2026-11-16).

**The user's decision (2026-11-12):** Lichess doesn't keep everything. A
linked game keeps in the program everything Lichess can't hold, attached to
the same moves, and only what Lichess can hold goes to Lichess and comes
from it. Three modes: a static download (no link, as today), follow only,
and two-way sync. Each linked game has a base snapshot, which is
**Lichess's own export of the chapter** (fetched again after a push), so
that what Lichess rewrites never looks like an edit made there. The
three-way merge on the move tree is `chess_net::study::merge` (below).

## Where it lives

| What | Where |
|---|---|
| The link: study and chapter ids, mode, the study's `Last-Modified`, the base snapshot, the local-only layer | `games.links` in the database, keyed by permanent id (`crates/chess-db/src/links.rs`; `docs/file-format.md` › `games.links`) |
| The split: `lichess_projection`, `local_only_layer`, `apply_layer`, `normalize_for_compare`, `is_lichess_addition` | `chess_net::study::layer` (`crates/chess-net/src/study/layer.rs`) |
| The three-way merge: `merge`, `changed_here`, `same_on_lichess`, `tag_changes` | `chess_net::study::merge` (`crates/chess-net/src/study/merge.rs`) |
| The sync: `plan` (the preview), `apply`, `resolve`, `linked_studies`, `waiting`, `set_mode`, `unlink`; a merge waiting for the user (`Pending`) | `chess_net::study::sync` (`crates/chess-net/src/study/sync.rs`) |
| The Sync button: its commands (`study_sync_*`), the dialog | `app/src-tauri/src/commands/study_sync.rs`, `app/src/lib/components/StudySyncDialog.svelte`, `app/src/lib/stores/studySync.svelte.ts` |
| From the command line | `cargo run -p chess-net --example study_sync -- plan\|apply DB STUDY_LINK` (or `new "Name"`), `status DB`, `resolve DB UID mine\|theirs\|both` |
| Tests | `crates/chess-net/tests/sync.rs` (against the stand-in Lichess), the merge's proptests, `scripts/smoke/study-sync.js` (the dialog, against the stand-in) |

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
| A game without Orientation | `white` added; `black` seen too (2026-11-16: a game with no players whose last position has Black to move) | None | That it had none |
| A game without Event (or `?`), Date | Event "{study}: Chapter n" or "{study}: White - Black", the day as Date (2026-11-16; `????.??.??` as sent is kept) | As here | — (the sync takes them as Lichess's, not changes) |
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

`normalize_for_compare(game)` is a game as Lichess holds it, compared: the
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
           n_nodes:varint node* [pending:bytes]       -- flags & 4: a merge waits for the user (the sync's)
pending := version:u8(=1) kind:u8 found:varint theirs:str
           -- kind 1 changed on both sides, 2 removed on Lichess and changed here,
           -- 3 the chapter by its ChapterURL but different, never synced;
           -- found: ms since the epoch; theirs: the chapter's PGN as Lichess exported it then
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
`Corrupt`; another version is refused as such. `Layer::to_bytes_with` and
`from_bytes_with` write and read the pending section; `from_bytes` reads a
layer with one and leaves it out. A pending merge in the link's bytes means
it lasts exactly as long as the link (a compaction keeps or drops both),
and no database format changed for it.

## The sync

The Sync button (ribbon › Database, next to Import study… and Send to
study…) runs `chess_net::study::sync`. **Linked**, the dialog looks at both
sides as soon as it opens (a job) and shows what a sync would do, "3 new
games here to send, 2 changed here, 1 changed on Lichess", game by game;
one confirm does it (a job, one at a time with NET-01's downloads; closing
the dialog leaves it running; Stop takes effect after the request under
way). **Not linked**, the button is Link to a study…: a new study for the
database's games (private by default, named after the database) or one of
the user's (the one a static copy's ChapterURLs name is offered), both ways
(the default) or follow only; the same preview, then Link and sync. The
dialog can change the mode or unlink (the games stay, a static copy).

**Change detection.** Per linked game, three games: ours (the database's
now), theirs (the chapter in the study's export now) and the base (the
link's snapshot: Lichess's own export at the last sync). All three are
compared as what Lichess holds of them (`merge::view`: the projection,
without Lichess's result comment, orientation in lower case; NAGs and
shapes as sets): **changed here** is ours ≠ base (`changed_here`), **changed
on Lichess** is theirs ≠ base (`same_on_lichess`). What only the database
holds (evals, `%emt`, comments before a variation's first move, NAGs
Lichess doesn't keep, ECO…) never counts as a change. **Tags Lichess fills
in** don't either (seen on lichess.org, 2026-11-16): a chapter with no Event
(or `?`) gets "{study}: Chapter n" (or "{study}: White - Black"), one with
no Date gets the day's, one with no Orientation the side Lichess picks
(`black` for a game with no players whose last position has Black to move).
A game here without them is the same as the base with them; a pull keeps
the game's own (none) unless Lichess changed them since the base. The
study's export is fetched once per plan and once per apply (with the login
when there is one); `If-Modified-Since` isn't used for it, see the open
questions.

| Here | On Lichess | Two-way | Follow only |
|---|---|---|---|
| unchanged | unchanged | nothing (the base is brought up to date if Lichess only rewrote its export) | the same |
| unchanged | changed | **pull**: the chapter, with the layer of the game now put back (`apply_layer` on the merge) | the same |
| changed | unchanged | **push**: the projection to `…/{chapter}/moves`, the tags that differ to `…/tags` | kept here, nothing sent |
| changed | changed | **merge** (`merge::merge`): clean and not large, here and pushed; else **left for the user** | clean: here only; else left for the user |
| linked game deleted here | there | its chapter stays, unless the preview's "Also remove the chapters of games deleted here" is ticked (`DELETE …/{chapter}`) | stays |
| unchanged | chapter removed | the game is marked as deleted here (Undelete until Compact) and unlinked | the same |
| changed | chapter removed | left for the user | the same |
| a game here not linked | — | **a new chapter** (`import-pgn`, in batches of ≤ 400 kB, ids from Lichess's answer in order), linked | not sent |
| — | a chapter with no game here | a game here that is that chapter (its ChapterURL names it, or the same as it as Lichess holds it) is **linked** to it; one by its ChapterURL that differs is left for the user (no base says which side changed); otherwise **a new game** here, linked | the same |

- **The merge** (`merge::merge`) matches nodes by their moves from the
  start. Per field, a change on one side goes in, the same change on both
  is one, different changes are a conflict: comments and clocks one value
  each; a move's assessment NAG (`!`, `?`…) and a position's (`+=`…) one
  value each, the other NAGs a set; arrows and highlights a set (never a
  conflict). A line added on either side is in; a line removed on one side
  goes if the other didn't change it, else it's a conflict. The order of a
  move's lines comes from the side that changed it; both making another
  move the main line is a conflict. The Lichess tags one value each (the
  fill-ins above aside). Start positions changed on both sides: a conflict.
  **Large**: a clean merge that would take more than 10 of this game's moves
  away (lines removed on Lichess), or any of its main line, waits for the
  user too. Proptests: an unchanged side gives way to the other, the same
  change on both is one, edits to different nodes and lines added on both
  sides all go in, conflicts don't depend on which side is ours.
- **Left for the user** = a pending merge on the link (`Pending`, kept in
  its layer bytes, see Stored): Lichess's version as found, and why. The
  base stays the last one both sides had; nothing is overwritten on either
  side, and later syncs leave the game until it's resolved. The ribbon
  button says Sync (n); the dialog lists the games with Resolve…, which
  shows what's in conflict (where, this database's value, Lichess's) and
  each side's main line, with:
  - **Keep this database's**: two-way, it goes to Lichess, but only if the
    chapter is still what was found (changed again: the game keeps waiting
    with the newer version, and says so); following only, it stays here
    and Lichess's becomes the base. For a removed chapter: sent again as a
    new chapter (two-way), or kept here unlinked.
  - **Keep Lichess's**: it comes here, what only the database holds put
    back where its moves still are. For a removed chapter: the game is
    marked as deleted here.
  - **Keep both**: Lichess's here, and this database's kept as a new game
    marked with a `LichessSyncCopy` tag (the chapter's link). A sync never
    sends a marked copy; without the tag it's a new game like any.
- **Pushes send the projection only**, never evals, `%emt`, comments before
  a variation's first move, NAGs off `LICHESS_NAGS`, ECO, Opening,
  Annotator, EventDate or study tags; **with the movetext on one line**
  (`pgn_to_send`): Lichess's `…/moves` keeps the line breaks our writer's
  wrapping at 80 columns puts inside comments (lichess.org, 2026-11-16:
  "{ The⏎Italian. }"), which `import-pgn` doesn't. After the pushes, the
  study's export is fetched again for the new bases: a chapter that doesn't
  show what was sent yet (Lichess caches a public study's export for about
  10 seconds) is asked for again 3 times, 4 seconds apart, then the base is
  what was sent (the next sync then takes Lichess's export, as the two are
  the same as Lichess holds them). Before each push the chapter is fetched
  and must still be what the decision saw; one changed meanwhile is left
  ("sync again").
- **Safe to run again, and nothing half-linked.** Every step commits on its
  own (a pulled game, then its link; links in one commit), and a step cut
  short heals on the next sync: a push whose new base wasn't recorded shows
  as the same change on both sides (merged to itself); new chapters whose
  links weren't recorded are linked by their moves, never sent twice. The
  apply looks at Lichess and the database again, so what changed since the
  preview is taken into account (never overwritten). A study made for the
  sync is said so even when what follows fails.
- **Games open in a tab** (any window) aren't changed: left until the tab
  is closed, and the preview says so. Each change here re-reads the game
  under the database's lock and leaves it if it changed meanwhile.
- **The login** comes from the app's keyring (NET-11). Two-way needs it,
  and its scopes are checked first (`/api/token/test`: study:write), so a
  token that can't write is refused before anything is sent or made; a
  token Lichess turned down logs out, as elsewhere. Following a public
  study needs none.
- **Rate limits:** writes that get a 429 wait for its `Retry-After` (a
  minute when it says none, at most 10 minutes) and are sent again, up to
  3 times (Lichess did nothing with them); then the sync stops there, says
  why, and keeps what was done.
- **Requests:** a sync's preview is 2 (the token's scopes, the export); its
  apply 2 more, then per changed game 2 or 3 (the chapter, its moves, its
  tags if they changed), one per batch of new chapters, and the export once
  more at the end (more while Lichess's cache lags).
- **Cloud and server databases** (NET-03) aren't synced with studies yet.

### On lichess.org, 2026-11-16

Run with `examples/study_sync.rs` on the user's account, on one new private
study ("Sync check (throwaway) 2026-11-16"), from a database of the six
round-trip games (`testdata/pgn/study-roundtrip.pgn`: evals, `%emt`,
comments before variations, `$142`, EventDate, set-up positions, non-ASCII):

- Linked to a new study: 6 chapters, then a second plan found **nothing
  changed on either side** (no phantom changes from what Lichess rewrites).
  Two games added here (one without any tags): 2 new chapters, then nothing
  again. Lichess filled in "Sync check …: Fresh - Game" and "Sync check …:
  Chapter 8" as Events and `black` as an Orientation; none counted.
- A comment changed here: pushed (the first time with our wrapping's line
  breaks in its comments, hence the one-line movetext), then nothing again.
- A collaborator's edit (a comment and a new line, sent with `curl` to
  `…/moves`): pulled; the game kept its eval and its own Event (none).
- The same first comment changed on both sides, on two games: both left for
  the user, neither side overwritten; Keep this database's sent one to
  Lichess, Keep both kept the other's copy (not sent).
- A tag changed on Lichess (`…/tags`) and a comment here: merged, both on
  both sides. A chapter removed on Lichess: its game marked as deleted here.
- A static copy of the study (IO-10's import into another database), linked
  following only: its 7 games linked by their ChapterURLs; a note added
  there stayed there, a rename on Lichess came.

Not run on lichess.org: a 429 (never met), a public study's cache (the
study was private; authenticated exports came back with
`cf-cache-status: BYPASS`), chapters removed on Lichess by the sync
(`DELETE …/{chapter}` was tried by hand: 204), more than 64 chapters.
Lichess's API has no way to delete a study: the throwaway study's chapters
were removed and the study is left to delete on the site.

## Open questions

1. **NAGs and tags not tried** are held here only. A `!!` (`$3`) a
   collaborator puts on a move on Lichess isn't what Lichess holds as far as
   the sync knows (it isn't on `LICHESS_NAGS`), so it's no change there and
   never comes here; a push of that game drops it on Lichess. lila's glyph
   list probably has `$3 $10 $15 $16 $17 $19 $32 $138` too (from memory, not
   checked). One more round trip with them, or taking lila's list, settles
   it.
2. **A chapter renamed on Lichess** doesn't rename the game's ChapterName
   here (study tags are ours). The link has the ids, so nothing breaks;
   say if a rename should come over.
3. **A linked game deleted here** keeps its chapter unless the preview's
   "Also remove the chapters of games deleted here" is ticked. Once the
   database is compacted its link goes with it, and the next sync brings
   the chapter back as a new game. Keep a mark of chapters deleted here,
   remove them by default, or accept it?
4. **DB-08's duplicate deletion** may keep an unlinked copy and delete the
   linked one; it should prefer keeping linked games.
5. **Clock rounding** is half up from one sample (`.5` → up); `.1`–`.4`
   weren't tried. `apply_layer` takes a clock within a second as the same,
   so either way ours come back.
6. **Comments' spacing on a push.** A pushed game's comments are its
   projection's: every run of spaces and line breaks one space (what our
   reader makes of them). A line break a collaborator typed in a comment on
   Lichess becomes a space when that game is next pushed (only a game that
   changed here is). Keeping Lichess's own text where a comment didn't
   change here (raw comments from the base, by path) isn't built.
7. **`If-Modified-Since`** (2026-11-16): Lichess answered 200 to a study
   export asked with its own `Last-Modified`, and with times a little after
   it; only a time well after (ahead of its clock) got a 304. The sync
   doesn't rely on it (it compares every chapter); IO-10's and NET-14's
   refreshes do, so they download a study that didn't change again.
8. **More than one study.** New games here go to the study only when the
   database is linked to that one alone, and one study holds 64 chapters:
   the rest stay here, and the preview says so. Making "Name (2/2)" as
   Send to study… does isn't built.
9. **Copies kept from a conflict** ("Keep both") are never sent. Offer them
   as new chapters instead?
10. **No study can be deleted through Lichess's API** (`DELETE
    /api/study/{id}` is a 404): a study made by Link to a study… stays on
    the account until deleted on the site.
11. **Cloud and server databases** (NET-03) can't be linked to a study yet.
