# ChessBase + Fritz — Consolidated Feature Catalog

Starting point for the build spec. It lists every ChessBase (database / preparation) and Fritz
(play / engine / training) feature I could find, merged into one program. Each feature has a
priority tier and its dependencies so the catalog can be cut into a build order.

Reference versions: **ChessBase 18** (the current release; no CB19 exists as of Sep 2026) and **Fritz 20**
(2025), plus long-standing features from earlier versions (CB 9–17, Fritz 11–19). Where a feature
belongs to a paid ChessBase online service, §5 gives an open replacement.

---

## 0. How to read this

**Tiers** (tier = importance *in relation to the rest of the system*, not "how cool it is"):

| Tier | Meaning | Rule of thumb |
|---|---|---|
| **P0** | Foundation | Other features can't be built without it. No UI value by itself. |
| **P1** | MVP core | Needed before the app is usable as a daily chess database. |
| **P2** | v1 parity | Features serious users expect from any modern tool (SCID, En Croissant, Lichess set this bar). |
| **P3** | Power / differentiator | Features people pay ChessBase for. Deep search, prep, and reports. |
| **P4** | Later / nice-to-have | Polish, niche workflows, heavy R&D for a small payoff. |
| **P5** | Out of scope (initially) | Depends on ChessBase's commercial ecosystem or needs servers or community. |

**Source**: `CB` = ChessBase, `FR` = Fritz, `CB+FR` = both.
**Depends on**: IDs that must exist first. An ID used by many others is a **linchpin** (see §2).

---

## 1. Architecture layers (how the groups relate)

```
 ┌───────────────────────────────────────────────────────────────────────────┐
 │  L5  Experience     PLAY · TRN · AI · PUB · HW · NET(cloud/social)        │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  L4  Insight        ANL (auto-analysis, reports) · REF (players, prep)    │
 │                     OPEN (tree, repertoire, books)                        │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  L3  Query          SRCH (header, position, pattern, material, themes)    │
 │                     ENG (UCI engines, tablebases)                         │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  L2  Storage        DB (native store, indexes, entities) · IO (PGN, CBH…) │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  L1  Domain model   CORE (rules, FEN, SAN, hashing) · GAME (tree, notes)  │
 ├───────────────────────────────────────────────────────────────────────────┤
 │  L0  Infra          INF (jobs, settings, crash safety)                    │
 └───────────────────────────────────────────────────────────────────────────┘
      UI and QOL cut across every layer. They are the views onto each one.
```

Two sides of one program:
- **ChessBase side** = L1–L4 plus PUB. Data-heavy: storage, indexing, search, and preparation.
- **Fritz side** = ENG, PLAY, TRN, and AI. Engine-heavy: playing, coaching, and engine matches.
- Both sides share CORE, GAME, ENG, and the board UI. That shared core is why one program covering both is practical.

---

## 2. Linchpin features (highest fan-out: build these first and build them well)

| ID | Feature | Why it matters |
|---|---|---|
| CORE-01 | Legal move generation | Everything |
| CORE-04 | Zobrist position hashing | Position index, transpositions, repetition, tree, repertoire, novelties, prep |
| GAME-01 | Game tree with nested variations | Notation, annotation, analysis output, repertoire, PGN I/O |
| IO-01/02 | PGN read/write | Only universal interchange format; also the test fixture for everything |
| DB-01 | Native storage format | Performance ceiling for the whole product (target 10M+ games like Mega DB) |
| SRCH-04 | Position index ("Search Booster") | Unlocks position search, opening tree, repertoire scan, novelty finder, player prep |
| DB-04 | Normalized entities (players/events/…) | Player dossier, spelling cleanup, fast header search, cross-tables |
| ENG-01 | UCI engine host | All analysis, all Fritz play modes, blunder check, training |
| INF-01 | Background job system | Import, search, batch analysis, and engine matches all run in the background |
| ANL-01 | Per-move engine eval pass | Blunder check, accuracy, error report, puzzles from own games, style report |
| AI-04 | Theme/pattern classifier | Strategic theme search, AI commentary, style report, puzzle themes |

---

## 3. Feature catalog

### INF — Infrastructure (cross-cutting)

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| INF-01 | Background job system: progress, cancel, queue, notifications | CB+FR | P0 | — | Imports of millions of games; batch analysis for hours |
| INF-02 | Settings/preferences persistence (per-user, per-DB) | CB+FR | P1 | — | |
| INF-03 | Crash-safe writes (journaling / atomic appends) | CB | P1 | DB-01 | ChessBase DB corruption is a known pain point. Do better here |
| INF-04 | Logging & diagnostics (engine I/O log, error reports) | CB+FR | P1 | — | |

### CORE — Chess rules & notation primitives

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| CORE-01 | Board representation, legal move gen, check/mate/stalemate, castling, e.p., promotion | CB+FR | P0 | — | Use a proven lib (shakmaty / python-chess / chess.js) |
| CORE-02 | Move formats: SAN, LAN, UCI; figurine output; localized piece letters (K/D/T/L/S etc.) | CB+FR | P0 | CORE-01 | Localized letters matter for PGN from German sources |
| CORE-03 | FEN parse/write (incl. X-FEN / Shredder-FEN for 960) | CB+FR | P0 | CORE-01 | |
| CORE-04 | Zobrist hashing; draw rules (threefold, 50-move, insufficient material) | CB+FR | P0 | CORE-01 | Same hash as Polyglot, so Polyglot books work for free |
| CORE-05 | Chess960 support throughout (rules, FEN, UI castling) | CB+FR | P3 | CORE-01..03 | Cheap if designed in from day one, expensive to retrofit |

### GAME — Game model, notation & annotation

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| GAME-01 | Game tree: main line + arbitrarily nested variations | CB+FR | P0 | CORE-01 | |
| GAME-02 | Headers: Seven Tag Roster + extended (Elo, ECO, EventDate, Annotator, TimeControl, FIDE IDs, Round/Board, custom tags) | CB | P0 | — | |
| GAME-03 | Move entry with **variation dialog** (new variation / new main line / overwrite / insert) | CB+FR | P1 | GAME-01, UI-02 | Signature ChessBase UX. Offer a "don't ask" default too |
| GAME-04 | Variation editing: promote/demote, delete variation, delete remaining/previous moves, cut/copy/paste lines | CB | P1 | GAME-01 | |
| GAME-05 | Text comments before/after moves; multi-language comments | CB | P1 | GAME-01 | Multi-language = P4 |
| GAME-06 | Symbol annotations (NAGs): move quality, position eval (±, +−, =, ∞), prefix symbols (with idea, novelty N, better is…) | CB | P1 | GAME-01 | |
| GAME-07 | Graphical annotations: colored arrows & squares (`[%cal]`/`[%csl]`) | CB | P1 | GAME-01, UI-01 | |
| GAME-08 | Clock/time annotations (`[%clk]`, `[%emt]`), time-usage graph | CB+FR | P2 | GAME-01 | Online games all carry clocks |
| GAME-09 | Critical-position markers (opening / middlegame / endgame) | CB | P3 | GAME-01 | Used by training & reports |
| GAME-10 | Training annotations: quiz questions, multiple choice, hidden solutions, points | CB | P3 | GAME-01 | Feeds TRN-04 |
| GAME-11 | Medals per game (best game, novelty, tactics, strategy, sacrifice, endgame, blunder, user…) | CB | P2 | DB-01 | Searchable (SRCH-10) |
| GAME-12 | Stored evaluation profile (eval per move) | CB+FR | P2 | GAME-01 | Output of ANL-01, drives ANL-04 graph |
| GAME-13 | Multimedia annotations (images, audio, video links) | CB | P4 | GAME-01 | |
| GAME-14 | Text documents stored in a DB (intros, tournament reports, HTML) | CB | P3 | DB-01 | ChessBase "text" entries, used for courses and magazines |
| GAME-15 | Merge games (combine many games into one tree as variations) | CB | P3 | GAME-01 | Useful for building opening surveys |
| GAME-16 | Undo/redo for all edits | CB+FR | P1 | GAME-01 | |
| GAME-17 | Games from set-up positions / fragments | CB | P1 | CORE-03 | |
| GAME-18 | Correspondence chess metadata (ICCF headers, sent/received dates) | CB | P4 | GAME-02 | |

### IO — File formats & import/export

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| IO-01 | PGN read: tolerant, streaming (multi-GB files), recovery from malformed games | CB+FR | P0 | GAME-01/02, CORE-02 | Import speed is a headline metric |
| IO-02 | PGN write incl. comments, NAGs, variations, `%cal/%csl/%clk/%eval` | CB+FR | P0 | GAME-01 | Round-trip test suite |
| IO-03 | ChessBase **.cbh** read (+ .cbg/.cba/.cbp/.cbt/.cbs/.cbj… companions) | CB | P3 | DB-01 | Done (2026-10-08): Import games takes a `.cbh`. Moves with variations (encoding mode 0, the translation table read from the user's own ChessBase 16), comments, symbols, coloured squares and arrows, players, events, sites, annotators, dates, results, Elos, ECO. Mega Database 2019: 7,623,531 of 7,623,697 games; Chess960 and two rare modes are skipped, lines with a null move are cut. Not yet: Chess960, null moves, `.cbj`, sources, teams, guiding texts. Spec: [io-03-cbh.md](io-03-cbh.md) |
| IO-04 | .cbh write | CB | P5 | IO-03 | Low value, high risk |
| IO-05 | .cbv (compressed ChessBase archive) read | CB | P4 | IO-03 | |
| IO-06 | Opening books: Polyglot .bin read/write; ChessBase .ctg read | CB+FR | P2 (bin) / P4 (ctg) | CORE-04 | CTG is proprietary, Polyglot is open |
| IO-07 | EPD read/write with opcodes (test suites, engine matches) | CB+FR | P3 | CORE-03 | |
| IO-08 | Clipboard: copy/paste FEN, PGN, whole game, position image | CB+FR | P1 | IO-01/02 | Done (2026-09-30, with UI-18): Copy game/position, Board image (QOL-06), Paste position (QOL-10) and Paste game in the ribbon; Ctrl+V takes a position or a PGN game. The clipboard is read and written in Rust |
| IO-09 | Drag-and-drop PGN/files onto the app to open/import | CB | P2 | IO-01 | |
| IO-10 | Import from URL (Lichess/Chess.com game or study link) | — | P2 | NET-01 | Not in CB. Cheap and much liked |
| IO-11 | Export filtered/selected games | CB | P1 | IO-02, SRCH-14 | |
| IO-12 | Text encoding handling (UTF-8, Latin-1/CP1252 legacy PGN) | CB | P1 | IO-01 | |
| IO-13 | SCID .si4/.si5 import | — | P4 | DB-01 | Migration from SCID users |

### DB — Database storage & management

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| DB-01 | Native storage format: compact move encoding, append-fast, indexed headers, soft-delete | CB | P0 | GAME-01/02 | CB18 advertises a new "accelerated data format". Speed is the product |
| DB-02 | Create/open/close/rename/delete DBs; many open simultaneously | CB | P0 | DB-01 | |
| DB-03 | Game list: virtualized (10M+ rows), custom columns, multi-column sort, flags, medals | CB | P0 | DB-01 | |
| DB-04 | Normalized entity indexes: **players, tournaments, annotators, sources, teams**, each with its own list and games-of-entity view | CB | P1 | DB-01 | Core ChessBase idea: rename a player once and every game updates |
| DB-05 | Edit headers; bulk edit / find-and-replace across selection | CB | P1 | DB-04 | |
| DB-06 | Copy/move/delete games between DBs; soft delete, undelete, pack/compact | CB | P1 | DB-01 | |
| DB-07 | Database library window: folders, favorites, icons per DB type (openings, training, corr, repertoire…), recent, properties (count/size) | CB | P1 | DB-02 | Partial: recent databases (Recent and the reference dropdown in the ribbon, the empty list), size and count in the status bar. No library window or folders; favourites are T-3 in [workspace-ui.md](workspace-ui.md) |
| DB-08 | Duplicate detection & removal (exact + fuzzy: players/date/move prefix; keep best-annotated) | CB | P2 | DB-01, CORE-04 | CB's "Double game killer". Done (2026-10-16, review fixes 2026-10-17 and 2026-10-19): Find duplicates… in the ribbon's Database group. When in doubt, two games aren't copies. Exact copies: same main line, White, Black, their ratings, Event, Site, Date, Round, Result and game ids (`Link`, `GameId`, `UTCTime`, `Board`…). Fuzzy ones optionally: players however written (the surname's last word and the first initial, one surname's words ending the other's, brackets dropped, so "Cervantes Landeiro, T." is "Thalia Cervantes Landeiro" but "Garcia Martinez" isn't "Perez Martinez"; names with digits, such as engines, need the same words), dates, results, rounds with their board ("5.1" ≠ "5.3"), ratings, Site URLs and game ids that agree where both have them, and the same main line or one the start of the other (from 10 moves on, with the year in both); below 20 moves, or with more than the last move missing, they also share the event, the full date, or the round in the same month. Games of one move or none are exact copies only between named players (not NN/BYE) with the round or an online game's page, and fuzzy ones only as an unannotated record of a game next to the one game with moves that has the same players, event, full date, round with board and result, and starts with its move. A game that could be a copy of two different games (or of more than 64 that start alike) stays. Each group keeps the longest, then the best annotated, then the one with clock times and evals, then the most complete headers; a copy with annotations, commands, or index-record fields (ratings, round and board, ECO, date, event, site) the kept game lacks is never deleted. The others are marked as deleted in a job (Restore until compacting, also after Find again), or the groups are shown in the list (a filter that isn't saved). A report goes stale when the database changes and is refused once another search replaced it. On Mega Database 2019 that leaves 13 exact groups (each one record entered twice) and, in fuzzy mode, 98 records of a game without its moves and 8 fuzzy copies, from 131 and 345; TWIC imported again as another source would have it is found at 99% without a wrong group. A job (`chess_db::find_duplicates`): 1M Lichess games in 0.2 s, the same file imported twice (2M) in 5 s, one players' bucket of 100,000 engine games in 0.5 s. Not yet: merging headers into the kept copy, picking the kept copy by hand, skipping copies while importing (NET-06) |
| DB-09 | Name normalization: spelling database, merge player/tournament entities, typo detection | CB | P2 | DB-04 | Matters a lot when merging TWIC + Lichess |
| DB-10 | Classification keys: user-editable hierarchies for openings, themes, tactics, strategy, endgames; auto-classify | CB | P3 | DB-01, SRCH-05 | ChessBase "Keys" |
| DB-11 | Game flags / user colors / marks | CB | P2 | DB-01 | |
| DB-12 | Integrity check & repair | CB | P2 | DB-01 | |
| DB-13 | **Reference database** concept: one designated big DB that powers tree, prep, novelties | CB | P1 | DB-02 | Conceptual glue for L4 features. Chosen in the ribbon's reference dropdown since UI-18 (2026-09-30); the tree picks the open or the reference database; the Reference tab (OPEN-14) reads it |
| DB-14 | Clip database (scratch DB for collecting games quickly) | CB | P2 | DB-06 | |
| DB-15 | Physical sort / renumber DB | CB | P4 | DB-01 | |
| DB-16 | Tournament cross-tables (round-robin & Swiss), standings, performance ratings | CB | P2 | DB-04 | Built (2026-10-15): Cross-table in the Events list and in the game header opens a dialog: points, games, score, average opponent Elo, FIDE performance, Sonneborn-Berger and wins (round robins) or Buchholz cut 1 and Buchholz (Swiss); knock-outs rank by the round of the first match lost, then the third-place match (a match level on points that the games don't settle puts its players between that round's winners and losers, with a note). Detects round robin (with cycles), Swiss, knock-out (by its shape: losers play no later round), match and events without rounds; a grid by opponent or a row of rounds; clicking a result opens the game; shared places show on every row ("5–6"). Byes, forfeits (decisive, no moves), unfinished games, missing Elos and rounds are handled, and a game with a board or game number that is in the database twice counts once; an event name used for several editions is split by dates, and Show games lists the edition shown (for the session only when the event has several: that filter isn't saved). A table over 20,000 cells (rows × columns, plus results) shows the standings only, so it paints in about a second. `chess-db` `crosstable.rs` |
| DB-17 | DB statistics: results by color, Elo averages, years, ECO distribution | CB | P2 | DB-01 | |
| DB-18 | Team competitions (team index, match tables) | CB | P4 | DB-04 | |
| DB-19 | Concurrent read while importing/writing | CB | P2 | DB-01, INF-01 | |

### SRCH — Search

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| SRCH-01 | Header search: player (either/specific color), Elo ranges, date ranges, event/site, result, ECO range, move count, annotator, source | CB | P1 | DB-04 | |
| SRCH-02 | Quick filter box over any list | CB | P1 | DB-03 | |
| SRCH-03 | Exact position search (incl. transpositions) | CB | P1 | SRCH-04 | |
| SRCH-04 | **Position index** (ChessBase "Search Booster"): hash → game ids | CB | P1 | CORE-04, DB-01 | Linchpin. Index size vs speed is a key design decision |
| SRCH-05 | Partial position / pattern search: piece-on-square masks, "any/none/not" squares, side to move | CB | P2 | CORE-01 | Scans games. Needs bitboard prefilter to be fast |
| SRCH-06 | Material search: exact balance, piece counts, ranges, "reached for ≥ N moves" | CB | P2 | CORE-01 | Endgame study use case |
| SRCH-07 | Maneuver search: piece routes (N d2–f1–g3), captures, move sequences with gaps | CB | P3 | CORE-01 | |
| SRCH-08 | Pawn-structure / similar-structure search | CB | P2 | SRCH-05 | Middlegame plan study |
| SRCH-09 | Similar-endgame search | CB | P3 | SRCH-06 | |
| SRCH-10 | Annotation search: comment text, NAGs, medals, arrows, training Qs, critical positions, eval ranges | CB | P3 | GAME-05..12 | |
| SRCH-11 | Strategic theme search (CB18: ~30 themes such as king attack, space advantage, strong knight), ranked by relevance | CB | P4 | AI-04 | R&D-heavy |
| SRCH-12 | Combined search masks (AND across header/position/material/…); saved searches | CB | P2 | SRCH-01..06 | |
| SRCH-13 | Search across multiple DBs at once | CB | P3 | SRCH-12 | |
| SRCH-14 | Results as filter; search within results; filter persists while browsing games | CB | P1 | SRCH-01 | |

### OPEN — Openings: tree, books, repertoire

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| OPEN-01 | Opening explorer/tree for current position from any DB: moves, #games, score %, draw %, avg Elo, performance, last played | CB+FR | P1 | SRCH-04, DB-13 | Most-used panel in practice |
| OPEN-02 | Online trees (Lichess masters + Lichess players DB; CB "Live Book" / 6B Lichess games) | CB | P2 | NET-01 | Free replacement for Mega + Live Book |
| OPEN-03 | Opening-book authoring: create from DB, edit weights, mark moves main/good/bad/forbidden | CB+FR | P2 | IO-06, OPEN-01 | Needed for engine play & matches |
| OPEN-04 | Book learning & tournament-book settings (variety, optimize) | FR | P4 | OPEN-03 | |
| OPEN-05 | **Repertoire DB**: mark lines as mine (per color); highlight in tree/notation | CB | P2 | OPEN-01 | CB18 "My Moves" (cloud) |
| OPEN-06 | Repertoire scanner: scan new games against repertoire, file relevant ones by line | CB | P3 | OPEN-05, SRCH-04 | |
| OPEN-07 | Repertoire report / print / gap detection ("positions with no prepared answer") | CB | P3 | OPEN-05 | Gap detection not in CB. Big value |
| OPEN-08 | Variation trainer / repertoire drills (spaced repetition) | CB | P2 | OPEN-05 | Chessable-style. CB18 has a basic version |
| OPEN-09 | Opening report for a position: main lines, stats, key players, plans, notable games, novelties | CB | P3 | OPEN-01, OPEN-10 | |
| OPEN-10 | Novelty detection vs reference DB (mark N, show predecessor games) | CB | P2 | SRCH-04, DB-13 | |
| OPEN-11 | ECO classification & opening names (auto-assign on import) | CB+FR | P1 | CORE-04 | Open data: lichess-org/chess-openings (CC0) |
| OPEN-12 | Engine evals shown in tree (cloud/Let's Check) | CB | P3 | OPEN-01, NET-08 | |
| OPEN-13 | Tree statistics inline in notation (game counts next to moves) | CB | P3 | OPEN-01 | |
| OPEN-14 | Reference tab: moves with "hot" and best players, main variations, top games by rating, mini board | CB | P2 | OPEN-01, DB-13, UI-19 | Done (2026-10-01): the third panel tab, reading the reference database (or the open one, with a note). Moves with Hot (↑/↓) and Best players, the main line and up to 12 lines (a job from ~50,000 games, cached), the top 100 games by average Elo, a view-only mini board. Resizable sections, stacked when narrow. Spec: [workspace-ui.md](workspace-ui.md). Overlaps OPEN-09, which keeps plans and novelties |
| OPEN-15 | Reference tab sub-tabs: Popularity, Endgames | CB | P3 | OPEN-14 | Ticket T-2 in [workspace-ui.md](workspace-ui.md); ask the user what each shows |

### ENG — Engines & tablebases

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| ENG-01 | UCI engine host: install/detect engines, options (Hash, Threads, MultiPV, SyzygyPath), per-engine profiles | CB+FR | P1 | INF-01 | Older ChessBase engine protocol not needed |
| ENG-02 | Infinite analysis pane: MultiPV, depth/seldepth, nps, eval, PVs; click PV to play it | CB+FR | P1 | ENG-01 | |
| ENG-03 | Eval bar + win/draw/loss probability (WDL) | CB+FR | P1 | ENG-02 | |
| ENG-04 | Multiple simultaneous engines (kibitzers) | CB+FR | P2 | ENG-01 | |
| ENG-05 | Paste engine line into notation as variation (with eval comment) | CB+FR | P1 | ENG-02, GAME-04 | |
| ENG-06 | Hover preview of PV positions (mini board) | CB | P2 | ENG-02 | |
| ENG-07 | Threat display (null-move "what does opponent threaten") | CB+FR | P2 | ENG-01 | Done: "Show threat" in the engine pane (remembered): the opponent's best line after a pass, with its eval, and a dashed arrow on the board; a note instead in check, after mate, or when the opponent would have no move. A threat engine that fails or can't start says so in the pane until "Show threat" is turned off and on. A second engine process, capped (see stack.md) |
| ENG-08 | Tablebases: local Syzygy probing, online lookup, tablebase panel with DTZ/WDL per move | CB+FR | P2 | CORE-01, NET-07 | |
| ENG-09 | Bundled engine (Stockfish), auto-download/update | FR | P1 | ENG-01 | Fritz ships its own engine. We ship Stockfish (GPL, run as separate process) |
| ENG-10 | Assisted analysis: color target squares by eval while dragging a piece | CB | P3 | ENG-02 | Distinctive CB feature |
| ENG-11 | Neural engines (Lc0): backend/GPU config, WDL display | CB+FR | P3 | ENG-01 | Mostly falls out of UCI |
| ENG-12 | Engine vs engine matches & tournaments: pairings, time controls, openings from book/EPD, crosstable, Elo estimate | FR | P3 | ENG-01, IO-07, OPEN-03 | Fritz feature, cutechess-like |
| ENG-13 | Remote engines (engine over network/SSH; CB "Engine Cloud") | CB | P3 | ENG-01 | |
| ENG-14 | Explain all moves (eval of every legal move) | FR | P2 | ENG-01 | Cheap with MultiPV=all |
| ENG-15 | Monte-Carlo / shootout analysis (play position out many times) | FR | P4 | ENG-01 | |
| ENG-16 | Hardware benchmark ("Fritzmark") | FR | P5 | ENG-01 | |

### ANL — Automated analysis & reports

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| ANL-01 | **Blunder check**: per-move eval pass, mark inaccuracy/mistake/blunder with better line | CB+FR | P1 | ENG-01, GAME-06/12 | Users now expect this (Lichess sets the bar) |
| ANL-02 | Full / tactical analysis: annotate with variations, auto text commentary, eval profile | CB+FR | P2 | ANL-01 | |
| ANL-03 | Batch analysis of a DB or filter (queued, resumable) | CB+FR | P2 | ANL-01, INF-01 | |
| ANL-04 | Clickable evaluation graph (+ time graph from GAME-08) | CB+FR | P1 | GAME-12 | Lichess-style; at the bottom of the Notation panel tab since UI-19 (2026-09-30). No time graph yet |
| ANL-05 | Deep position analysis: engine builds an annotated tree of best lines | CB | P3 | ANL-01 | |
| ANL-06 | Accuracy / avg centipawn loss / move-classification counts per player | — | P2 | ANL-01 | Lichess-style, expected now |
| ANL-07 | Error report (CB18 "Blunder report"): error types, game phase, "calculation Elo" | CB | P3 | ANL-03, GAME-09 | |
| ANL-08 | Style report (CB18): style profile vs GM averages (risk, sacrifices, endgame skill…) with sample games | CB | P4 | ANL-03, AI-04 | |
| ANL-09 | Compare engines on same position/game | CB+FR | P3 | ENG-04 | |

### REF — Players, tournaments & preparation

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| REF-01 | Player dossier: results, openings per color (tree), score by year, top opponents, best games | CB | P2 | DB-04, SRCH-04 | |
| REF-02 | **Opponent preparation**: collect games (local + online), per-color repertoire tree, weak lines, recent changes | CB | P2 | REF-01, NET-01 | Main reason club players buy CB |
| REF-03 | Rating-list import (FIDE) & Elo history graphs | CB | P3 | DB-04 | |
| REF-04 | Player encyclopedia: photos, bios, titles, FIDE IDs | CB | P5 | REF-03 | Content/licensing problem, not code |
| REF-05 | Online-account identification: match Lichess/Chess.com accounts to a real player by repertoire similarity | CB | P4 | REF-02, NET-01 | New in CB18 |

### PLAY — Playing vs the computer (Fritz)

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| PLAY-01 | Play vs engine with clocks: increment, delay, multi-stage classical, hourglass | FR | P2 | ENG-01, UI-01 | |
| PLAY-02 | Strength limiting by Elo; human-like weakening | FR | P2 | PLAY-01 | UCI_Elo + Maia-style nets |
| PLAY-03 | Handicap modes: sparring (engine leaves tactics for you), friend mode (auto-adjusts), material odds | FR | P3 | PLAY-02 | |
| PLAY-04 | Coach: hints, threat warnings, "coach is watching" blunder alerts, takeback | FR | P2 | PLAY-01, ENG-07 | |
| PLAY-05 | Personalities/styles (Fritz 20: Tal, Fischer, Karpov, Hypermodern, Romantic; configurable via style sliders) | FR | P4 | PLAY-02, ANL-08 | |
| PLAY-06 | Personal rating per time control, history | FR | P3 | PLAY-01 | |
| PLAY-07 | Chess960 games | FR | P3 | CORE-05, PLAY-01 | |
| PLAY-08 | Bullet training mode (Fritz 20) | FR | P4 | PLAY-01 | |
| PLAY-09 | Play from any position / take over from a DB game / play out from opening | FR | P2 | PLAY-01 | |
| PLAY-10 | Blindfold / hidden-pieces modes | FR | P4 | PLAY-01 | |
| PLAY-11 | Auto-save played games to a "My games" DB | FR | P2 | PLAY-01, DB-06 | |
| PLAY-12 | Human-like engine time management | FR | P4 | PLAY-01 | |

### TRN — Training

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| TRN-01 | Tactics trainer: rated puzzles by theme | CB+FR | P2 | UI-01 | Lichess puzzle DB (CC0, ~5M) |
| TRN-02 | Puzzles from your own mistakes (auto-extracted) | — | P3 | ANL-03 | Strong differentiator |
| TRN-03 | Guess-the-move / replay training vs master games, scored | CB+FR | P3 | GAME-01 | |
| TRN-04 | Training-annotation quiz mode (plays GAME-10 content) | CB | P3 | GAME-10 | |
| TRN-05 | Endgame trainer: play out positions vs tablebase | FR | P3 | ENG-08 | |
| TRN-06 | Opening drills (= OPEN-08) | CB | P2 | OPEN-08 | |
| TRN-07 | Video courses synced to board (ChessBase media format) | CB+FR | P5 | GAME-13 | Content ecosystem |
| TRN-08 | Calculation/visualization drills | FR | P4 | UI-01 | |
| TRN-09 | Unified spaced-repetition scheduler for all training types | — | P3 | TRN-01, OPEN-08 | |

### UI — Board & windows

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| UI-01 | 2D board: themes, piece sets, coordinates, flip, last-move/check highlight, legal-move hints | CB+FR | P1 | CORE-01 | |
| UI-02 | Move input: drag, click-click, keyboard SAN typing, promotion picker | CB+FR | P1 | UI-01 | |
| UI-03 | Notation pane: nested/indented variations, collapse, inline comments, click-to-jump | CB+FR | P1 | GAME-01 | The hardest UI component. Prototype early |
| UI-04 | Dockable/resizable panels; saved layouts | CB+FR | P2 | — | First pass done (2026-10-01): every panel tab (Notation, Opening tree, Reference, Engine analysis, Game list) docks to the left, right, top or bottom side, as a group of its own or into another group, by dragging or from the tab's menu; dividers resize the sides and groups (keys too); View ▸ Reset layout; the whole layout is saved in the `layout` pref (version 2). Spec: [workspace-ui.md](workspace-ui.md). Free docking, floating panels and named layouts later (T-1) |
| UI-05 | Tabs for multiple games/DBs | CB | P1 | — | |
| UI-06 | Game-list board preview on hover/selection | CB | P2 | DB-03 | |
| UI-07 | Position setup editor (castling, e.p., side to move, clear/start, from FEN) | CB+FR | P1 | CORE-03 | |
| UI-08 | 3D animated boards | CB+FR | P5 | UI-01 | |
| UI-09 | Light/dark themes, font scaling, HiDPI | CB+FR | P1 | — | |
| UI-10 | Customizable shortcuts; command palette | CB | P2 | — | Palette not in CB. A modern upgrade |
| UI-11 | Localization (UI + piece letters) | CB+FR | P3 | CORE-02 | |
| UI-12 | Sounds, move animation speed | FR | P2 | UI-01 | |
| UI-13 | Table notation view (variations in parallel columns) | CB | P4 | UI-03 | |
| UI-14 | Multiple boards/windows, multi-monitor | CB | P3 | UI-04 | |
| UI-15 | Status bar with background-task progress | CB | P2 | INF-01 | |
| UI-16 | Context menus everywhere | CB+FR | P1 | — | |
| UI-17 | Touch-friendly mode | CB | P4 | UI-01 | |
| UI-18 | Ribbon toolbar: labelled groups (Clipboard, Game, Reference, Analysis, Database, View) that collapse on narrow windows | CB | P2 | — | Done (2026-09-30): one row at any width; labels drop, then groups collapse into dropdowns from the right. View shows the Notation, Opening tree and Reference tabs (UI-19, OPEN-14) and has Reset layout (UI-04). Spec: [workspace-ui.md](workspace-ui.md) |
| UI-19 | Panel tabs beside the board (Notation, Opening tree, Reference) | CB | P2 | UI-03 | Done: Notation and Opening tree (2026-09-30), Reference (2026-10-01, OPEN-14). A tab queries only while it shows; the active tab is saved in the `layout` pref. Since UI-04 every panel, Engine analysis and Game list included, is a tab that can be docked anywhere. Spec: [workspace-ui.md](workspace-ui.md) |

### QOL — Quality of life

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| QOL-01 | Session restore (reopen DBs, games, positions, layout) | CB | P2 | INF-02 | |
| QOL-02 | Unsaved-change prompts; replace-game vs save-as-new; recovery after crash | CB+FR | P1 | INF-03 | |
| QOL-03 | Next/previous game in current list/filter without leaving board | CB | P1 | SRCH-14 | |
| QOL-04 | Drag games between DBs / onto board | CB | P2 | DB-06 | Done 2026-09-29 (pointer drags; Shift moves) |
| QOL-05 | Cancellable long operations with ETA | CB | P1 | INF-01 | |
| QOL-06 | Copy board as image (PNG/SVG) | CB+FR | P2 | UI-01 | **Done** (2026-09-29): "Board image ▾" in the ribbon copies PNG or SVG, or saves either; theme colours, coordinates, last move, check and drawings; three sizes |
| QOL-07 | Remember column widths/sort/filters per DB | CB | P2 | INF-02 | |
| QOL-08 | Onboarding: sample DB, bundled engine, first-run import wizard | FR | P2 | ENG-09 | Done: welcome guide on first run, 12 classic sample games, engine check, PGN → new database wizard |
| QOL-09 | Share game via link / QR code (CB18) | CB | P4 | NET-03 | |
| QOL-10 | Global "go to position": paste FEN anywhere to jump & tree-search it | — | P2 | SRCH-03 | Done: Ctrl+V outside a text box or "Go to position…"; FEN, FEN tag or Lichess link |

### NET — Online & cloud

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| NET-01 | Download games by username from Lichess / Chess.com APIs, incremental sync | CB | P2 | IO-01 | CB18 has a built-in Chess.com API |
| NET-02 | Search huge online DBs (Lichess explorer; CB online DB of ~10M games) | CB | P3 | NET-01 | |
| NET-03 | Cloud DBs / sync across devices (CB "My Games", "My Moves") | CB | P4 | DB-01 | Needs a server |
| NET-04 | Live broadcasts viewer (Lichess broadcast API; CB Playchess/live DB) | CB+FR | P4 | NET-01 | |
| NET-05 | Playing server (Playchess) | CB+FR | P5 | — | Out of scope. Link to Lichess |
| NET-06 | Auto-updater for TWIC weekly issues into a reference DB | — | P3 | IO-01, DB-08 | Replaces Mega DB weekly updates |
| NET-07 | Online tablebase lookup (lichess tablebase API) | CB+FR | P2 | — | |
| NET-08 | Cloud eval lookup (Lichess cloud-eval; CB "Let's Check") | CB+FR | P3 | — | |
| NET-09 | Web/mobile companion app | CB | P5 | NET-03 | |
| NET-10 | Shared/collaborative DBs | CB | P5 | NET-03 | |

### PUB — Printing, publishing & export

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| PUB-01 | Print games with diagrams, layout options | CB+FR | P3 | UI-01, IO-02 | PDF generation |
| PUB-02 | Export diagram PNG/SVG; animated GIF of game | CB | P2 | UI-01 | |
| PUB-03 | Export HTML (static web replayer) | CB | P3 | IO-02 | |
| PUB-04 | Export DOCX/RTF/LaTeX (xskak) | CB | P4 | IO-02 | |
| PUB-05 | Print crosstables, repertoire, reports | CB | P4 | DB-16, OPEN-07 | |
| PUB-06 | QR codes for games/positions | CB | P4 | QOL-09 | |
| PUB-07 | Scoresheet printing | CB | P4 | — | |

### HW — Hardware & accessibility

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| HW-01 | DGT / electronic board support | CB+FR | P4 | UI-02 | |
| HW-02 | Voice move input (CB18) | CB | P4 | CORE-02 | |
| HW-03 | Move speech output / screen-reader accessibility | FR | P4 | CORE-02 | |

### AI — AI features

| ID | Feature | Src | Tier | Depends on | Notes |
|---|---|---|---|---|---|
| AI-01 | LLM chat about the position, grounded on engine output (Fritz 20 AI chat) | FR | P4 | ENG-02 | Ground it on engine output to avoid hallucinated lines |
| AI-02 | Natural-language game commentary (Fritz 20: ~170 themes, voice output) | FR | P4 | AI-04, ANL-02 | |
| AI-03 | LLM opening tips / player bios (CB18 ChatGPT integration) | CB | P5 | — | |
| AI-04 | Theme/pattern classifier (pins, forks, outposts, IQP, space, king attack…) | CB+FR | P3 | CORE-01, ENG-01 | Shared base for SRCH-11, ANL-08, AI-02, TRN-01 themes |

---

## 4. Suggested build phases (derived from tiers + dependencies)

> **Status (2026-09-26, late):** every P0 item, and of P1: UI-01/02/03/05/07/09/16,
> GAME-03…07/16/17, DB-02…06 (DB-04 and DB-07 in part), DB-13, SRCH-01…04/14, OPEN-01/11,
> ENG-01/02/03/05/09, ANL-01/04, IO-08/11/12, INF-02/03/04, QOL-01/02/03/05.
> Open P1 work: more piece sets (UI-01). Status: the [Feature Status doc](https://claude.ai/artifact/SfWucTSjakLh9oNSWSNQrV#93fdfc30-cba2).
>
> **Phase 3 (P2), started 2026-09-29:** QOL-04 (drag games onto another database, the
> board or the tab strip, which also completes DB-06's copy/move between databases),
> QOL-06 (board as a PNG/SVG image), QOL-07 (list sort, column widths and filter per
> database), QOL-08 (welcome guide, sample games, import wizard), QOL-10 (go to a pasted
> position), UI-19 (panel tabs: Notation, with the eval graph, and Opening tree beside the
> board; 2026-09-30), UI-18 (the ribbon, with IO-08's paste game and the reference
> dropdown; 2026-09-30), OPEN-14 (the Reference tab, the third panel tab; 2026-10-01),
> UI-04 (first pass: panel tabs dock to the four sides of the window; 2026-10-01).


| Phase | Goal | Contents |
|---|---|---|
| **0 — Foundation** | Headless engine you can test | CORE-01..04, GAME-01/02, IO-01/02, DB-01/02, INF-01..04. Benchmarks: import Lichess monthly PGN, round-trip PGN |
| **1 — Database MVP** | Replace a PGN viewer + SCID for browsing | UI-01/02/03/05/07/09/16, DB-03..07, DB-13, SRCH-01/02/14, GAME-03..07/16/17, IO-08/11/12, QOL-02/03/05 |
| **2 — Analysis** | Replace Lichess analysis board, offline | ENG-01/02/03/05/09, ANL-01/04, SRCH-03/04, OPEN-01/11 |
| **3 — Preparation** | The "ChessBase" value | OPEN-02/03/05/08/10, REF-01/02, NET-01/07, SRCH-05/06/08/12, DB-08/09/11/16/17, ENG-04/06/07/08/14, ANL-02/03/06, GAME-08/11/12, IO-06/09/10, PUB-02, QOL-01/04/06/07/08/10, UI-04/06/10/12/15 |
| **4 — Fritz side** | Play & train | PLAY-01/02/04/09/11, TRN-01/06, then PLAY-03/06/07, TRN-02/03/05/09, ENG-12 |
| **5 — Power** | Differentiators | IO-03 (CBH import), SRCH-07/09/10/13, OPEN-06/07/09/12/13, ANL-05/07/09, DB-10, AI-04, ENG-10/11/13, NET-02/06/08, PUB-01/03 |
| **6 — Later** | Everything P4 | Style report, personalities, AI commentary, DGT, voice, cloud sync… |

---

## 5. ChessBase ecosystem features → open replacements

| ChessBase / Fritz service | What it gives | Open replacement |
|---|---|---|
| Mega Database / Big Database (~10M games, weekly updates) | Reference DB | TWIC (weekly PGN), Lichess Elite DB, Lichess masters explorer |
| Online Database / Live Book | Huge online tree | Lichess opening explorer API (masters + all Lichess games) |
| Let's Check | Shared deep evals | Lichess cloud-eval API + local eval cache |
| Engine Cloud | Rented remote engines | SSH/remote UCI to own machine |
| Fritz engine / Fritz neural net | Strong engine | Stockfish (GPL, separate process), Lc0 |
| Fritz Powerbook / CTG books | Opening book | Polyglot books generated from our own DBs |
| Playchess | Play online | Link out to Lichess (and read its broadcasts) |
| Player Encyclopedia | Photos, bios | FIDE rating list download (IDs, titles, ratings) |
| Tactics training DB | Puzzles | Lichess puzzle DB (CC0) |
| ECO/opening keys | Classification | lichess-org/chess-openings (CC0) |
| Mobile app / cloud DB | Sync | Deferred (needs a server) |

---

## 6. Open questions for the build spec

1. ~~**Platform/stack**~~ **Decided: Rust + Tauri 2.** See [stack.md](stack.md).
2. **CBH import priority**: how important is migrating existing ChessBase users? That decides whether IO-03 is P2 or P5.
3. **Target scale**: 10M games (Mega-scale) sets the SRCH-04 index design. 1M games allows a much simpler design.
4. **Fritz side in v1?** One app with both sides (recommended: shared core) or DB-first and play later?
5. ~~**License**~~ **Decided: GPL-3.0-or-later.** See [stack.md](stack.md).
6. **Opening tree vs Reference tab** (OPEN-01 vs OPEN-14): both are tabs for now; the user will say whether they merge or the tree gets another job (T-4 in [workspace-ui.md](workspace-ui.md)).

---

## Sources

- [A quick and complete review and overview of ChessBase 18](https://en.chessbase.com/post/a-quick-and-complete-review-and-overview-of-chessbase-18)
- [ChessBase 18: New dimensions in study and preparation](https://en.chessbase.com/post/chessbase-18-new-dimensions-in-study-and-preparation)
- [Review: Fritz 20 – A revolutionary leap forward in chess software](https://en.chessbase.com/post/review-fritz-20-a-revolutionary-leap-forward-in-chess-software)
- [Brand new release: Fritz 20](https://en.chessbase.com/post/brand-new-release-fritz-20)
- [Fritz 20 (ChessBase shop)](https://shop.chessbase.com/en/products/fritz_20)
- [Fritz (chess) — Wikipedia](https://en.wikipedia.org/wiki/Fritz_(chess))
- Long-standing features (keys, medals, search masks, repertoire, Search Booster, Fritz play modes) come from ChessBase/Fritz documentation across versions 9–19. Version attribution for older features is approximate.
