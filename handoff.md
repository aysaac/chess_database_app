# Handoff

State of the project as of **2026-09-26 (evening)**, for the next agent.
Conventions, commands and utilities are in [AGENTS.md](AGENTS.md); read that
first.

## TL;DR

- **Most P1 items are now in.** On top of the first prototype, this session
  added engines and analysis (ENG-01/02/03/05/09, ANL-01/04), the position
  index with opening tree, position search and opening names
  (SRCH-03/04, OPEN-01/11), header search, quick filter and filtered lists
  (SRCH-01/02/14, IO-11, QOL-03), settings and recent databases
  (INF-02, DB-07, QOL-01), player/event lists with rename and merge (DB-04),
  a position setup editor (UI-07), tabs (UI-05) and compaction (DB-06).
- All checks pass: fmt, clippy (`-D warnings`), 149 Rust tests (+1 ignored
  large-data test), svelte-check, the frontend build, and six smoke drivers
  (`basic`, `engine`, `search`, `tree`, `setup`, `layout`), with the 1M-game
  parts of `search` and `tree` too.
- **Everything so far is in the first commit on `main`** (2026-09-26, at
  the user's request). There's no remote yet. Ask before committing again
  or pushing. `HYPRLAND-DISRUPTION-REPORT.md` was left out, untracked, for
  the user to keep or delete.
- **The smoke harness no longer touches the user's desktop.** It used to
  add a headless output to the user's Hyprland, which moved their monitor
  and lost the cursor (`HYPRLAND-DISRUPTION-REPORT.md`). Now the app runs
  on a private headless sway (`scripts/offscreen.sh`), and `basic` and
  `layout` pass there with the desktop verified unchanged. It needs
  `sway` installed (see [Environment notes](#environment-notes)).
- **Next up:** the remaining P1 polish (see [Next steps](#next-steps)), then
  GitHub and CI once the user agrees.

## Decisions so far

| Topic | Decision | Where |
|---|---|---|
| Scope | ChessBase + Fritz in one program; build order by catalog tiers | `docs/feature-catalog.md` |
| Stack | Rust core, Tauri 2.11, Svelte 5 + TS on Vite | `docs/stack.md` |
| License | GPL-3.0-or-later (unlocks shakmaty, pgn-reader, chessground) | `docs/stack.md` |
| Platforms | Linux first, portable code; CI for Windows/macOS once on GitHub | `docs/stack.md` |
| Programmatic access (CLI/API/MCP) | Deferred; the core crates stay Tauri-free so it's cheap later | `docs/stack.md` |
| Storage | Custom files (no SQLite): 64-byte index records, append-only blobs, name tables, crash-safe commits | `docs/file-format.md` |
| UI design | "Tournament hall" palette, Source Serif 4 for notation, IBM Plex Sans Condensed for chrome, system light/dark | `app/src/app.css` |
| **Engines** (new) | New Tauri-free crate `crates/chess-engine`: UCI host, analysis driver thread, blunder check. Tests run against a tiny fake engine (`src/bin/fake_uci.rs`) | `crates/chess-engine` |
| **Bundled engine** (new) | Stockfish 19 official "universal" build, fetched by `scripts/fetch-stockfish.sh` (SHA-256 pinned) into git-ignored `app/src-tauri/binaries/`. Found there in dev builds; `tauri build --config src-tauri/tauri.stockfish.json` bundles it as a sidecar. Not committed (~100 MB) | `scripts/fetch-stockfish.sh` |
| **Position index** (new) | Sorted 16-byte entries `(hash, game, ply, next move, result)` for main-line plies 0–50, in segment files; imports add segments, edits go to a dirty list and are replayed; rebuilt after 6 segments or 4,096 dirty games. Derived data, deletable. ~740 bytes per online game (see the open question below) | `docs/file-format.md` |
| **Format version 2** (new) | `db.meta` says `chessdb 2`: replaced games are recorded in `positions.dirty`. Version 1 databases upgrade on open (only the meta file changes). Binary headers stay at version 1 | `docs/file-format.md` |
| **Opening names** (new) | lichess-org/chess-openings (CC0) vendored as `crates/chess-core/data/openings.tsv` (commit c67912b), keyed by position hash; classified in the index build and stored in `openings.idx` | `crates/chess-core/src/openings.rs` |
| **Settings** (new) | `settings.json` in the app config dir (engines, default engine, recent databases, frontend prefs). `CHESSDB_CONFIG_DIR` overrides the folder; the smoke harness uses its own | `app/src-tauri/src/settings.rs` |
| **Tabs** (new) | Opening a game reuses the current tab if it has no unsaved changes, else opens a new tab (no discard prompt). Next/previous game replaces the tab (with the prompt). Analysis follows the active tab | `app/src/lib/stores/game.svelte.ts` |
| **Compaction** (new) | Rewrites into `*.new` files, commits with a `compact.commit` marker, renames; open finishes or rolls back. Game ids change; open tabs are remapped; the position index is rebuilt | `crates/chess-db/src/database.rs` |

Still open (catalog §6): how important ChessBase `.cbh` import is (#2), the
target scale of 1M vs 10M+ games (#3), and whether play-vs-engine (the Fritz
side) belongs in v1 (#4). **#3 now has concrete numbers:** the position index
takes about as much disk as the games (740 MB for 1M Lichess games), which
would be ~7 GB at 10M. Cheaper options are in `docs/file-format.md`.

## What exists

### Features (by catalog ID)
- **Delivered:** INF-01…04 · CORE-01…04 · GAME-01…07, 16, 17 ·
  IO-01, 02, 08, 11, 12 · DB-01, 02, 03, 05, 06 · SRCH-01…04, 14 · OPEN-01, 11 ·
  ENG-01, 02, 03, 05, 09 · ANL-01, 04 · UI-01, 02, 03, 05, 07, 09, 15, 16 ·
  QOL-01 (reopens the last database and remembers orientation, pane size,
  tree visibility, analysis lines), 02, 03, 05.
- **Partial:**
  - DB-04: players, events, sites and annotators have lists with counts,
    "show games" and rename/merge. No sources or teams, and no per-player
    dossier (REF-01 is P2).
  - DB-07: recent databases (toolbar menu, empty-list shortcuts), close,
    size and deleted count in the status bar. No library window, folders or
    favourites.
  - DB-13 (reference database): the tree and position search use the open
    database; there's no separate "reference" database yet.
  - GAME-03: the variation dialog still has no "Insert" choice.
  - GAME-08: clocks show as tooltips; no time graph.
  - ENG-01: engines can be added, removed, made default and configured
    (common options plus "show all"), but there are no per-engine profiles.

### What the user can do now (new this session)
- **Analyze** (toolbar, Ctrl+Shift+A): Stockfish runs with 1–5 lines, depth,
  speed, WDL, and an eval bar beside the board. Click a move in a line to play
  up to it; **Add line** pastes it as a variation with `+0.35/22`.
- **Blunder check…**: the engine checks every main-line move (0.3/1/3 s per
  move) and adds `?!`/`?`/`??`, the better line, `[%eval]` per move and the
  annotator. One undo step removes it all. The **eval graph** under the
  notation shows `[%eval]` data (Lichess games have it) with the mistakes
  marked; click to jump.
- **Engines…**: find installed engines, add one by file, set the default,
  edit options.
- **Opening tree**: moves from the current position with games, score, a
  White/draw/Black bar, average Elo, performance and last year; click to
  play; **Find games** filters the list to games reaching the position
  (by any move order). Positions deeper than 25 moves offer a full search.
- **Filters:** a quick filter box over the list (any player, event, site or
  annotator; accents and case ignored), **Search…** for header search
  (players, colours, event, site, annotator, years, ECO, result, Elo range
  for both/either/average, moves, annotated only, skip deleted, search
  within results). Sorting and export work on the filtered list.
  Ctrl+Up/Down or the nav buttons open the previous/next game of the list.
- **Lists tabs** under the board: Games, Players, Events, Sites,
  Annotators; Enter or double-click shows the games; F2 renames (merging
  with an existing name).
- **Opening names** in the game header and an Opening column in the list
  (ECO from the positions when the game has no ECO tag, shown dimmed).
- **Set up position…**: board editor with palettes, move/remove tools,
  side to move, castling (only where possible), en passant, move number and
  a FEN box, with plain-language errors ("each side needs a king").
- **Tabs** for open games (Ctrl+T new, Ctrl+W close, Ctrl+Tab cycle,
  middle-click closes, Ctrl+double-click or the context menu opens in a new
  tab).
- **Compact…** in the status bar removes deleted games for good.

### Measured performance
Release build, first 1M games of Lichess Jan 2024 (2.3 GB of PGN with
clocks and evals), 16-core laptop:

| | |
|---|---|
| Import | 48.9 s (20k games/s), 310 MB peak RSS |
| DB size | 697 MB |
| Open | 478 ms warm (0.9–1.2 s right after writing) |
| Sort by White | 117–430 ms |
| Read a game | ~65 µs warm |
| Quick filter (text) | 380 ms first time (folds all names once), then ~45–75 ms |
| Header search (Elo ≥ 2500, draws) | 42–58 ms |
| Position index build | 12.3 s (8 threads), 46.2M positions, 740 MB |
| Opening tree: start / 1.e4 / Najdorf | 34 ms (first use, loads per-game stats) / 6.7 ms / 0.1 ms |
| In the dev app (`tauri dev`) | index ready 14 s after a 1M import; tree 250 ms first, ~100 ms after 1.e4; position search (585k games) 45 ms |

### How the pieces talk (additions)
- **Analysis:** `start_analysis` starts an `Analyzer` (`chess-engine`) on
  its own thread. `AppState::with_session` calls `Analysis::follow` after
  every session command, so when the cursor of the followed session moves,
  the new position is searched (bursts are coalesced). Updates reach the
  frontend as `analysis` events (`AnalysisDto`, scores from White's point of
  view), throttled to ~10/s. `add_session` and `activate_session` re-attach
  the analysis to the game on screen.
- **Blunder check:** a job with its own engine instance runs
  `review_game` on a snapshot, then applies `apply_review` through
  `GameEditor::edit_with` (one undo step). The frontend reloads the view.
- **Filters:** `search_games` (job) / `search_position` / `show_name_games`
  install an `ActiveFilter` per database handle; `list_games` pages through
  it in the current sort (`sorted_subset`, cached per sort).
- **Index:** the frontend calls `update_index` after opening and after each
  import (`db.ensureIndex`, looping until up to date). `opening_tree` reads
  the index and the per-game stats column (`Database::game_stats`).

## Known gaps and tech debt

**Storage**
- The position index is big (see above) and covers the main line only,
  plies 0–50. Deeper positions need the full scan (seconds per million
  games).
- Name tables: rename rewrites the whole file; merges rewrite the affected
  records (no DB-09 fuzzy matching yet). Merged-away names stay in the table
  until compaction.
- Sort permutations and filters live in memory only.
- `set_deleted` fsyncs once per id; batch it before bulk deletes exist.
- Compaction holds the database lock for its whole run; the list waits.

**PGN** (unchanged)
- Line breaks inside comments are exported as spaces; a "comment before" on
  a main-line move becomes the previous move's comment; localized piece
  letters aren't accepted on import; import warnings aren't listed in the UI.

**Engines**
- One analysis at a time (ENG-04 is P2). The blunder check uses the default
  engine and fixed thresholds (Lichess's). No engine I/O log view (logged at
  debug level only).
- Engine options: buttons (e.g. "Clear Hash") can't be pressed from the UI.

**UI**
- UI-01 has one board theme and piece set; UI-02 has no keyboard SAN
  entry; UI-03 can't collapse variations.
- The toolbar wraps to two rows in windows narrower than about 1,400 px, and the nav row
  (now with next/previous game) is wider than very small boards. The layout
  driver passes at all five sizes.
- The promotion picker is still wider than boards under ~400 px.
- The root comment can't be edited; no "Insert" in the variation dialog; no
  arrow preview in the next-move picker (user review follow-ups).
- `CommentDialog.svelte` and now `FenDialog.svelte` are unused (the setup
  editor replaced the FEN dialog). The user hasn't approved deleting them.
- Prettier isn't applied (the repo never was; `NotationLine.svelte` relies
  on exact whitespace), so don't run `npm run format` on everything.

**Not verified yet**
- Real pointer and keyboard input (dragging pieces, the setup editor's
  palette and square clicks, right-click menus, shortcuts, native file
  dialogs). chessground ignores synthetic events, so the smoke drivers can't
  click the board.
- `tauri build` with `tauri.stockfish.json` (bundling the sidecar) was never
  run. Windows and macOS were never built.
- Light mode was never screenshotted.
- The cursor repair in `scripts/smoke/run.sh` (see below).

## Next steps

Recommended order:

1. **Ask the user** about installing sway for the smoke harness (see
   Environment notes) and open question #3.
2. **P1 polish left:**
   - GAME-03 "Insert" in the variation dialog (keep following moves where
     they stay legal).
   - Editing the root comment; promotion picker size on small boards;
     arrow preview in the next-move picker.
   - Show import warnings in the UI (they're counted in the job message and
     the first 20 are logged).
   - DB-13 reference database (use one designated database for the tree).
   - UI-01 board/piece themes, UI-02 keyboard SAN entry, UI-03 collapsing
     variations.
3. **Housekeeping:** GitHub, CI (Linux/Windows/macOS: fmt,
   clippy, tests, svelte-check, `tauri build`). CI needs
   `scripts/fetch-stockfish.sh <triple>` before a bundled build.
4. Then catalog Phase 3 (P2): ENG-04/06/07/08/14, ANL-02/03/06, OPEN-02/03/05,
   REF-01/02, NET-01/07, SRCH-05/06/08/12…

## Environment notes

- The user's machine: Arch Linux with Omarchy, Hyprland 0.54.3 (Wayland),
  2560×1600 at 1.25 scale, dark theme. Rust 1.94 (Arch package), Node 25,
  npm 11. `grim` and `hyprctl` are available; Xvfb, xdotool and ydotool
  aren't, and sway has to be installed for the smoke harness.
  **Stockfish 19** is now in `app/src-tauri/binaries/` (fetched,
  git-ignored), not installed system-wide.
- **Windows for testing go on a private headless display**
  (`scripts/offscreen.sh`, used by `scripts/smoke/run.sh`): a headless sway
  with its own sockets and D-Bus. Never change the user's Hyprland session
  (`hyprctl output create`, `keyword`, `dispatch`). The old harness did,
  and the user's auto-placed monitor (`monitor=,preferred,auto,1.25`) moved
  to x=22048 while it ran; Hyprland 0.54.3 then left the cursor off screen
  and waybar and windows seemed to vanish. If the cursor is ever gone,
  `hyprctl keyword monitor ",preferred,auto,1.25"` brings it back.
- **sway must be installed** (`sudo pacman -S sway`; it pulls in
  `wlroots0.19`). Without it the harness stops with exit code 2 instead of
  opening a window on the user's screen. It was verified on 2026-09-26 with
  sway 1.11 unpacked into a temporary folder; it wasn't installed
  system-wide yet at that point.
- Smoke drivers must never open native dialogs: nobody can click them on
  the private display, so the driver would hang. Every driver stubs
  `game.confirmDiscard = async () => true`.
- `HYPRLAND-DISRUPTION-REPORT.md` at the repo root wasn't written by the
  agent of this session; it explains the problem above. Leave it for the
  user to keep or delete.
- **The user keeps working while you test.** Never open a visible window.
- The user's own database is `app/src-tauri/test_/My games.chessdb`
  (git-ignored user data; don't delete or move it). The first time the app
  opens it, it's upgraded to format 2 and indexed (new files appear in it).
- `testdata/large/` holds `twic1600.pgn` and `lichess_1m.pgn` (git-ignored).
- The user's global instructions are in `~/AGENTS.md` (turn format:
  things changed, blocked on me, things found). Ask before anything
  destructive or outside this repository.
- The user may be running `npm run tauri dev` themselves (port 1420). The
  smoke harness uses port 1430. Don't kill processes you didn't start.
