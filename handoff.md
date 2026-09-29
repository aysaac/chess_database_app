# Handoff

State of the project as of **2026-09-29**, for the next agent.
Conventions, commands and utilities are in [AGENTS.md](AGENTS.md); read
that first.

## TL;DR

- **QOL-10 "go to position" (2026-09-29, first P2 item)**, one commit on
  branch `qol-10-paste-fen` (not merged, not pushed). Ctrl+V outside a
  text box, or **Go to position…** in the toolbar, takes a FEN, a PGN
  `[FEN "…"]` tag, an EPD line or a Lichess analysis link. If the game on
  screen reaches the position (main line first, move counters ignored),
  the cursor goes there; otherwise it opens as a new game (in the tab
  unless it has unsaved changes, as opening a game does). The opening tree
  opens on it. A game from a set-up position now always offers the full
  search when the index has no game for it, since its move number comes
  from a FEN (often a made-up "1"). Checks: fmt, clippy, 163 tests,
  svelte-check, build, and the smoke drivers `goto` (new), `tree`,
  `layout`, `setup`, `basic`, `search` (without the 1M-game parts; `engine`
  not run, no Stockfish in that worktree). The desktop stayed unchanged.
- **QOL-06 board images (2026-09-29, branch `qol-06-copy-board-image`,
  one commit, not pushed):** "Board image ▾" in the toolbar copies the
  board as PNG or SVG, or saves either. The image is drawn in Rust by a new
  crate, `crates/chess-diagram` (resvg), and put on the clipboard by
  `tauri-plugin-clipboard-manager`. See [Board images](#new-board-images-qol-06-2026-09-29).
  Checks: fmt, clippy, 175 Rust tests (+1 ignored), svelte-check, the
  build, and the smoke drivers `image` (new), `basic`, `layout`, `setup`,
  `search` and `tree` (not `engine`: no Stockfish in that worktree; nothing
  engine-side changed), with the desktop unchanged.
- **QOL-08 onboarding (2026-09-29, branch `qol-08-onboarding`, one commit,
  not pushed or merged):** a welcome guide opens on a first run: start
  (import your games / explore the sample games / open a database), your
  engine (the detected Stockfish, or how to add one), a few tips. The
  sample is 12 classic games with short notes, compiled into the app and
  built into `Sample games.chessdb` in the app data folder on first use.
  The import wizard makes a new database from one or more PGN files
  (suggested in Documents, named after the first file), with progress,
  Cancel, and a summary of warnings and skipped games. A "?" at the right
  end of the status bar reopens the guide; the empty game list offers the
  import and the sample. New driver `scripts/smoke/onboarding.js`; the
  harness now starts other drivers as a returning user (see AGENTS.md).
  Checks: fmt, clippy, 166 tests (+1 ignored), svelte-check, the build and
  all seven drivers, with Stockfish and (onboarding) without; the 1M-game
  parts didn't run (no `testdata/large/` in that worktree).
- **QOL-07 (2026-09-29, branch `qol-07-remember-db-view`):** the game list
  remembers its sort order, column widths and filter per database and puts
  them back when the database is opened again. Columns can now be resized:
  drag a header's right edge, Shift+←/→ on a focused header, double-click
  the edge or right-click the header for the default width. Filters are
  saved as the steps that made them (quick filter, header searches and
  searches within them, a player/event/site/annotator by name, a position
  as FEN, full or through the index) and run again as a job on open, so
  they survive compaction and include games added since. A filter that
  can't be made again (the player was renamed) is dropped with a notice.
  New smoke driver `scripts/smoke/view.js`; reopening the 1M-game database
  with a sort by White Elo and a quick filter restored took 968 ms in
  `tauri dev`. Also fixed: the Opening column had no header label. All
  checks and the smoke drivers except `engine.js` (no Stockfish in that
  worktree) passed on Linux.
- **QOL-04 (2026-09-29, P2)** on branch `qol-04-drag-games` (one commit,
  not pushed, not merged). Select several games in the list and drag them
  onto another database (copy; Shift moves), onto the board or onto the
  tab strip (they open). It also completes DB-06, which only had
  delete/restore/compact: copying and moving between databases, also from
  the list's menu. Details in [New: dragging games](#new-dragging-games-qol-04-2026-09-29).
  Checks: fmt, clippy, 170 Rust tests plus the TWIC round trip,
  svelte-check, the build; smoke drivers off-screen: the new `drag.js` and
  `basic.js` with their 1M-game parts, `layout`, `search`, `tree`, `setup`.
  `engine.js` wasn't run (no Stockfish in that worktree).
- **macOS (2026-09-27):** the project now also lives on the user's Mac
  (Apple Silicon, macOS 26.5, see [Environment notes](#macos)). Everything
  passes there: fmt, clippy, 156 tests plus the ignored TWIC round trip,
  svelte-check, the build, and all six smoke drivers including the
  1M-game parts. Changes that session (PR #1, merged 2026-09-28):
  - Clippy 1.98 flags `chunks_exact` with a constant size and one
    `sort_by`; fixed with `as_chunks` / `sort_by_key` (stable since 1.88,
    so fine on the Linux box's 1.94 and the MSRV).
  - `scripts/smoke/run.sh` runs on macOS: stock bash 3.2 (no `mapfile`),
    no `setsid` (job control instead), `perl` instead of GNU `sed`, and
    `screencapture` of the app window for shots. It also takes a relative
    out-dir now (it broke because the app starts from `app/`).
  - **First bundled build:** `tauri build --config
    src-tauri/tauri.stockfish.json` makes `Chess Database.app` with
    `stockfish` next to `chess-app` (where `engines.rs` looks) and an
    87 MB DMG. The DMG step needs `CI=true` here (see below). The app is
    only ad-hoc signed.
- **Every P1 item is in**, except more piece sets (UI-01). The second
  session of 2026-09-26 added, one commit each: GAME-03 "Insert", intro
  comment editing (plus a fix for "comment before move"), next-move arrows
  and a promotion picker that fits small boards, import warnings (IO-01),
  the reference database (DB-13), board colour themes and display options
  (UI-01), typed move entry (UI-02) and folding variations (UI-03).
- All checks pass: fmt, clippy (`-D warnings`), 156 Rust tests (+1 ignored
  large-data test), svelte-check, the frontend build, and all six smoke
  drivers (`basic` with its 1M-game part, `layout`, `tree`, `search`,
  `setup`, `engine`), with the user's desktop verified unchanged. The new
  screens (arrows, board themes, folded variations, move box, import
  warnings, reference tree, promotion picker on a 208 px board) were
  looked at in the screenshots.
- **sway 1.11 is installed system-wide** (2026-09-26), so
  `scripts/smoke/run.sh` works as is. `pacman -S sway` got a 404 from
  stable-mirror.omarchy.org (its package list was ahead of its files), so
  the user installed the same versions from archive.archlinux.org with
  `pacman -U` (checksums matched the sync database). If a later
  `pacman -S` gives a 404, that's the mirror again.
- **Commits:** the user asked for a commit per finished feature. `main` has
  the initial commit plus the feature commits. **Remote:** the private
  GitHub repo `aysaac/chess_database_app` (`origin`), created and pushed on
  2026-09-27 at the user's request. Ask before pushing again.
  `HYPRLAND-DISRUPTION-REPORT.md` is still untracked, for the user to keep
  or delete.
- **The smoke harness doesn't touch the user's desktop.** It runs the app on
  a private headless sway (`scripts/offscreen.sh`); see
  [Environment notes](#environment-notes).
- **Windows tested (2026-09-27)** on the user's Windows 10 PC (from WSL2,
  running the Windows toolchain through `cmd.exe`): fmt, clippy, 156 tests,
  svelte-check, the frontend build, all six smoke drivers (with Stockfish 19
  for Windows) and `tauri build --config src-tauri/tauri.stockfish.json`
  (MSI + NSIS installers; the release exe starts). Fixes it took:
  `update_position_index` synced `openings.idx.tmp` through a read-only
  handle ("Access is denied" on Windows); `fitBoard()` now snaps the board
  in device pixels like chessground (at 120% scaling it left a 6 px strip);
  a race in `basic.js`; Rust 1.98 clippy lints. New: `scripts/smoke/run.ps1`,
  and `.gitattributes` pinning LF (the Windows checkout had turned every file
  into CRLF). One `cargo test` run on Windows had a single failure that 16
  reruns didn't reproduce; its name wasn't captured.
  The user allows visible windows on that PC. Merged as PR #2
  (2026-09-28). **What's left for Windows** (code fixes and checks on the
  PC) is in [docs/windows-testing.md](docs/windows-testing.md).
- **Next up:** CI once the user agrees, then catalog Phase 3
  (P2). More piece sets are on hold (the user said not now).

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
| **List view per database** (QOL-07) | `databaseViews` in `settings.json`, keyed by the canonical folder path (50 most recent): sort, resized column widths, and the filter as its steps (`FilterStep`) plus description. Not inside the database folder: no format change, and read-only or shared databases work. Rust records the steps in `ActiveFilter` and saves on every change (`AppState::set_filter`/`clear_filter`; closing keeps it); the frontend saves sort and widths (`save_db_layout`) and on open calls `db_view`, then `restore_filter` (a job that won't replace a filter set meanwhile) | `app/src-tauri/src/commands/dbview.rs` |
| **Tabs** (new) | Opening a game reuses the current tab if it has no unsaved changes, else opens a new tab (no discard prompt). Next/previous game replaces the tab (with the prompt). Analysis follows the active tab | `app/src/lib/stores/game.svelte.ts` |
| **Reference database** (DB-13) | A path in settings (`referenceDatabase`). The backend opens it on its own handle, shared with the list when it's the same folder (`DbUse` flags in `AppState`: a database closes when neither the list nor the reference uses it). The tree picks its source (open or reference); "Find games" on the reference opens it in the list | `app/src-tauri/src/state.rs` |
| **Onboarding** (QOL-08) | The guide opens while `welcomeDone` is unset and no database was ever opened. Sample games are compiled in (`app/src-tauri/data/sample-games.pgn`) and built into the app data folder, not Documents. `CHESSDB_CONFIG_DIR` holds app data too | `docs/stack.md` |
| **Compaction** (new) | Rewrites into `*.new` files, commits with a `compact.commit` marker, renames; open finishes or rolls back. Game ids change; open tabs are remapped; the position index is rebuilt | `crates/chess-db/src/database.rs` |
| **Board images** (QOL-06) | New Tauri-free crate `crates/chess-diagram`: an SVG of only paths (cburnett pieces, labels outlined from an embedded IBM Plex Sans Condensed), rasterized by `resvg`; the clipboard is written from Rust (arboard via `tauri-plugin-clipboard-manager`), the frontend has no clipboard permission. Images use the theme's light-mode colours in any mode | `docs/stack.md` |

Still open (catalog §6): how important ChessBase `.cbh` import is (#2), the
target scale of 1M vs 10M+ games (#3), and whether play-vs-engine (the Fritz
side) belongs in v1 (#4). **#3 now has concrete numbers:** the position index
takes about as much disk as the games (740 MB for 1M Lichess games), which
would be ~7 GB at 10M. Cheaper options are in `docs/file-format.md`.

## What exists

### Features (by catalog ID)
- **Delivered:** INF-01…04 · CORE-01…04 · GAME-01…07, 16, 17 ·
  IO-01, 02, 08, 11, 12 · DB-01, 02, 03, 05, 06, 13 · SRCH-01…04, 14 · OPEN-01, 11 ·
  ENG-01, 02, 03, 05, 09 · ANL-01, 04 · UI-01, 02, 03, 05, 07, 09, 15, 16 ·
  QOL-01 (reopens the last database and remembers orientation, pane size,
  tree visibility, analysis lines), 02, 03, 04, 05, 06, 07, 08, 10.
- **Partial:**
  - DB-04: players, events, sites and annotators have lists with counts,
    "show games" and rename/merge. No sources or teams, and no per-player
    dossier (REF-01 is P2).
  - DB-07: recent databases (toolbar menu, empty-list shortcuts), close,
    size and deleted count in the status bar. No library window, folders or
    favourites.
  - GAME-08: clocks show as tooltips; no time graph.
  - ENG-01: engines can be added, removed, made default and configured
    (common options plus "show all"), but there are no per-engine profiles.
  - UI-01: five board colour themes, but one piece set (cburnett).

### New: board images (QOL-06, 2026-09-29)
- **"Board image ▾"** (toolbar, after Copy FEN): Copy as PNG, Copy as SVG,
  Save as file… (PNG or SVG by the extension; `.png` is added to a bare
  name), and a size: Small 320, Medium 480 (default) or Large 800 px,
  remembered (`boardImageSize`). The image is the position at the cursor as
  the board shows it: orientation, theme, coordinates and last-move tint
  (following "Board appearance"), check glow, and the game's arrows and
  circles. The picker's arrows and the eval bar aren't included. The save
  dialog suggests `White - Black after 23...Qe7.png` (`Board.png` at the
  start).
- **Colours:** the image always uses the theme's own colours, not the
  dimmed dark-mode ones (images mostly land on white pages).
  `board-theme.css` now sets `--board-light-base`/`--board-dark-base` and
  `-dimmed` per theme; `--board-light`/`--board-dark` pick one.
- **Rust:** `crates/chess-diagram` (`Diagram::to_svg`, `to_png`,
  `to_rgba`) and `app/src-tauri/src/commands/image.rs`
  (`copy_board_image`, `save_board_image`, temp file + rename). The SVG
  carries the FEN as its `<title>`. librsvg (`rsvg-convert`) draws it the
  same as resvg. A copy takes 50–100 ms from the click to the notice in
  `tauri dev`, 800 px included (the rasterizer is optimized in dev builds).
- **Tests:** 9 in the crate (colours, orientation, highlights, arrows,
  labels, PNG header), 4 for the commands; `scripts/smoke/image.js` checks
  the menu, both copies, saving, a failed save and the suggested names. The
  harness has a new `clip:<name>` line that saves the private display's
  clipboard (`wl-paste`) to `<out>/clips/`: the copies offer `image/png`
  and text, and the saved clips were looked at.
- `cargo run -p chess-diagram --example render -- out.png [FEN] [SIZE] [black]`
  draws a FEN without the app.

### New: dragging games (QOL-04, 2026-09-29)
- **Selecting several games** in the list: click, Ctrl+click (add or
  remove; it moves the anchor), Shift+click and Shift+arrows (from the
  anchor), Ctrl+A. The selection is kept as row ranges (`lib/selection.ts`),
  so selecting a million rows costs nothing; the backend turns ranges into
  game ids in the list's sort and filter (`ListSelectionDto`,
  `selected_ids` in `commands/db.rs`). Delete and the menu act on the
  selection; the cursor row decides between delete and restore.
- **Dragging** starts after 6 px with the button down. A shelf over the
  notation lists the drop targets: the reference database, recent
  databases that still exist (not the open one) and "New database…" (asks
  where, via the save dialog). A label under the pointer says what the drop
  will do; Shift switches copy to move, Escape cancels. The board opens the
  first game like a double-click and the rest (up to 10) in new tabs; the
  tab strip opens all in new tabs. `lib/stores/drag.svelte.ts`,
  `DropShelf.svelte`.
- **Pointer events, not HTML5 drag and drop** (decision in
  `docs/stack.md`): WebView2 breaks HTML5 DnD while Tauri's file drop is on,
  which IO-09 needs, and smoke drivers can dispatch pointer events.
- **Copy/move (DB-06):** `chess_db::copy_games` appends the stored blob
  bytes unchanged in batches of 2,000 and re-interns the names in the
  target; nothing is decoded, so copies are exact (unit tests, a proptest
  property, and `drag.js` compares PGN of every copy). Deleted games are
  skipped. Moving marks the originals deleted after the copies commit.
  Cancelling keeps what's copied. A target that isn't open is opened (or
  created) for the job alone (`AppState::db_at`), shared if the list opens
  it meanwhile. A new target is added to the recent list second, so the
  open database is still the one reopened at start-up.
- **Menu and keyboard path:** the list menu (right-click, the Menu key or
  Shift+F10; arrows move between items) has "Copy … to database…" and
  "Move … to database…", which open `CopyGamesDialog.svelte`.
- `Database::set_deleted_many` marks many games with one fsync (the old
  per-id fsync was listed as debt below).

### New in the second session (2026-09-26, late)
- **Insert** in the variation dialog: the new move replaces the recorded
  one and the moves after it are replayed (by piece and squares) as far as
  they stay legal; a notice says how many were removed.
- **Comments:** the intro comment (before the first move) is edited in
  place; double-clicking any comment edits it; "comment before move" on a
  main-line move now edits the comment where PGN keeps it (the previous
  move's), prefilled, instead of silently overwriting it.
- **Next-move picker** draws its moves as arrows on the board (the
  highlighted one in blue). The **promotion picker** scales with the board.
- **Import warnings:** after an import with problems the status bar shows
  "N import warnings"; the dialog lists game number, players and problem,
  with Open for imported games.
- **Reference database** (DB-13): the opening tree's source picker chooses
  the open database or the reference, or sets the reference ("Choose
  reference database…", "Use <open> as reference"). "Find games" on the
  reference opens it in the list.
- **Board appearance** (nav bar): five colour themes, coordinates,
  legal-move dots and last-move highlight.
- **Typing moves** (UI-02): type `e4`, `Nf3`, `nf3`, `exd5`, `e8=Q`, `0-0`
  or `g1f3` with the board or notation focused; a box on the board lists
  matches and Enter plays.
- **Folding variations** (UI-03): click a top-level variation's left rule
  or a nested one's "(" to fold it into a stub; the move menu has collapse
  this / all and expand all.

### Go to position (QOL-10, 2026-09-29)
- **Ctrl+V** anywhere outside a text box with a FEN on the clipboard, or
  **Go to position…** in the toolbar (a box to paste into). Accepted: a
  FEN (also only the placement, or placement plus side to move), a PGN
  FEN tag, an EPD line, a Lichess analysis link (`_` or `%20` for spaces).
- The board goes to the move where the game on screen reaches the
  position, else to a new game from it; the opening tree opens. The status
  bar says which ("This game reaches that position at 5…c6; went there").
  No FEN on the clipboard, or an illegal position, gets a plain message.
- How: `position::find_fen` and `Game::find_position` (chess-core), the
  `goto_position` command, `gotoPosition.ts`, `FenPaste.svelte` (the
  Ctrl+V path; see the WebKit gotcha in AGENTS.md) and `FenDialog.svelte`
  (reused for the dialog).

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
| Copy 1M games to a new database (DB-06, `drag.js`) | 13.2 s in the dev app, with other agents' smoke runs loading the machine (blobs copied as is, nothing decoded) |

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
- Sort permutations and filter results live in memory only; reopening a
  database rebuilds its saved sort and filter (QOL-07). A moved or renamed
  database folder loses its saved view, and renaming a name doesn't update
  a saved name filter (it's dropped on the next open, with a notice).
- Column widths are per database; a new database starts with the defaults.
  The list has no horizontal scrolling, so columns widened past the pane
  are cut off at the right (as in narrow windows before).
- Deleting or restoring a selection (`set_deleted_rows`) runs in the command,
  not as a job: fine for thousands of rows, a second or two for a million.
- A copy target that another program has open fails with "database is
  open in another process"; nothing waits for it.
- Compaction holds the database lock for its whole run; the list waits.

**PGN** (unchanged)
- Line breaks inside comments are exported as spaces; a "comment before" on
  a main-line move becomes the previous move's comment; localized piece
  letters aren't accepted on import.

**Engines**
- ENG-09's auto-download/update isn't built: without a bundled or installed
  Stockfish, the welcome guide says where to get one and offers "Add
  engine…".
- One analysis at a time (ENG-04 is P2). The blunder check uses the default
  engine and fixed thresholds (Lichess's). No engine I/O log view (logged at
  debug level only).
- Engine options: buttons (e.g. "Clear Hash") can't be pressed from the UI.

**UI**
- UI-01 has five board colour themes and display options (coordinates,
  legal-move dots, last move) under "Board appearance" in the nav bar, but
  only the cburnett piece set: more sets need vendored SVGs with checked
  licences (lichess's lila repo lists them in COPYING.md).
- The toolbar needs about 1,420 px for one row (measured 2026-09-29); the
  default window is 1,440, so one more toolbar button wraps it there. That's
  why the welcome guide's "?" is in the status bar.
- The toolbar wraps to two rows in windows narrower than about 1,400 px, and the nav row
  (now with next/previous game) is wider than very small boards. The layout
  driver passes at all five sizes.
- `CommentDialog.svelte` is unused. The user hasn't approved deleting it.
  (`FenDialog.svelte`, also unused before, is the "Go to position" dialog
  now.)
- Prettier isn't applied (the repo never was; `NotationLine.svelte` relies
  on exact whitespace), so don't run `npm run format` on everything.
- Board images (QOL-06): "Copy as SVG" puts the markup on the clipboard as
  text only (arboard has no `image/svg+xml`), so drawing programs that
  want an SVG image need the saved file. No keyboard shortcut (the menu is
  reachable by Tab). One piece set, like the board. The embedded font is
  the fontsource Latin subset of IBM Plex Sans Condensed SemiBold (OFL 1.1,
  like the fonts the frontend already bundles), unpacked from WOFF to TTF;
  only its outlines of a–h and 1–8 reach the images.

**Not verified yet**
- Real pointer and keyboard input (dragging pieces, the setup editor's
  palette and square clicks, right-click menus, shortcuts, native file
  dialogs). That includes a real Ctrl+V with a FEN (QOL-10): `goto.js`
  sends the keydown and a paste event itself. The paste target relies on
  the webview pasting into the element focused during keydown (WebKitGTK
  and WebView2 do; on macOS, Cmd+V goes through the Edit menu, so check
  it there). chessground ignores synthetic events, so the smoke drivers can't
  click the board.
- Real input could now be tested on the Mac (`cliclick`), once the
  terminal has Accessibility permission.
- Dragging games (QOL-04) with a real mouse: `drag.js` dispatches synthetic
  pointer events at the real positions, but WebKitGTK/WebView2/WKWebView
  delivery of real ones (implicit capture, `buttons` on moves, releasing
  outside the window) is untested. The "New database…" drop target opens a
  native save dialog, so drivers call `db.copyRows(…, create)` instead.
- Bundles (`tauri build --config src-tauri/tauri.stockfish.json`) were built
  on macOS (`.app` and DMG) and Windows (MSI and NSIS installers), but
  neither was installed and clicked through; on Windows the release exe
  starts. Linux bundles were never built.
- Light mode was never screenshotted (the Mac can, once screenshots work).
- Board images (QOL-06) pasted into real programs: on Linux only
  `wl-paste` on the private display read them. Windows and macOS clipboards
  are untested (B10 in `docs/windows-testing.md`); the native save dialog
  wasn't clicked through (the driver saves to a given path).
- QOL-08: the guide's native dialogs ("Choose PGN files…", "Change…", "Add
  engine…", "Open a database") weren't clicked; the driver sets what they
  return. The `smoke: first-run` seeding in `run.ps1` wasn't run on Windows.
- The cursor repair in `scripts/smoke/run.sh` (see below).

## Next steps

Recommended order:

1. **Ask the user** about open question #3 (1M vs 10M+ games), and
   whether to set up CI on the GitHub repo.
2. **P1 polish left (on hold, the user said not now):**
   - UI-01 piece sets: vendor a few SVG sets with GPL-compatible licences
     (check lichess-org/lila's COPYING.md per set) and add a picker to
     "Board appearance".
3. **Housekeeping:** CI on GitHub Actions (Linux/Windows/macOS: fmt,
   clippy, tests, svelte-check, `tauri build`). CI needs
   `scripts/fetch-stockfish.sh <triple>` before a bundled build.
4. **Windows:** the agent on the Windows PC works through
   [docs/windows-testing.md](docs/windows-testing.md) and sends PRs; they
   are reviewed here with the Linux smoke harness before merging.
5. Catalog Phase 3 (P2) has begun: QOL-04, 06, 07, 08 and 10 are in. Next:
   ENG-04/06/07/08/14, ANL-02/03/06, OPEN-02/03/05, REF-01/02, NET-01/07,
   SRCH-05/06/08/12… DB-14 (clip database) and PLAY-11 now have DB-06's copy
   to build on; IO-09 (drop PGN files on the window) can use Tauri's file
   drop, which the in-app drags leave alone.

## Environment notes

- The user's machine: Arch Linux with Omarchy, Hyprland 0.54.3 (Wayland),
  2560×1600 at 1.25 scale, dark theme. Rust 1.94 (Arch package), Node 25,
  npm 11. `grim` and `hyprctl` are available; Xvfb, xdotool and ydotool
  aren't; sway is installed for the smoke harness.
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
- **sway is installed** (1.11, with `wlroots0.19`); the harness needs it.
  Without it the harness stops with exit code 2 instead of opening a
  window on the user's screen.
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

### macOS

- Apple Silicon (arm64), macOS 26.5, terminal Ghostty. Rust 1.98 via
  rustup (`source ~/.cargo/env` in each shell; installed 2026-09-27),
  Node 25 from Homebrew, Xcode command line tools. Stockfish 19
  (`stockfish-aarch64-apple-darwin`, universal) and `testdata/large/` are
  fetched.
- **No private display on macOS:** the smoke window opens on the user's
  desktop and takes focus. The user allowed this ("you have permission to
  control my screen for smoke test").
- **Permissions Ghostty lacks** (the user grants them in System Settings >
  Privacy & Security, then restarts Ghostty): Screen Recording, so
  `screencapture` works (the harness says "no screenshots" and runs
  without them until then); Automation of Finder, which the DMG's
  AppleScript window layout needs; it times out with -1712 otherwise.
  `CI=true npm run tauri build …` skips that step.
- Measured in the smoke drivers (debug `tauri dev`, 1M Lichess games):
  1M import ~42 s, index ready 7 s after it, tree 103 ms at the start and
  101 ms after 1.e4, position search 16 ms, quick filter 217 ms, header
  search 18 ms, sort by Black Elo 58 ms.

### Windows

- Windows 10 PC; PR #2 ran the Windows toolchain from WSL2 through
  `cmd.exe`. The user allows visible windows there (no private display).
- Settings live in `%APPDATA%\dev.chessdb.desktop\settings.json`.
- The to-do list, how to work (fresh `main`, one PR per fix) and how to
  verify each item: [docs/windows-testing.md](docs/windows-testing.md).
  Windows notes for the next agent go under this heading, not in the
  TL;DR, so Mac and Windows PRs don't conflict.
- **2026-09-28:** A1–A3 are PRs #3–#5, and a duplicate-engine race found
  on the way is #6 (all platforms). Part B is done except other scales in
  B6 and B8. An MSI installed after an NSIS one lands in the NSIS folder
  (B1). B2/B3 ran with real input (`SendInput`) while the user was away;
  they found A4 (reload, print, find bar) and A7 (three small bugs).
  Fixes: A4 #10, A5 #9, A6 #8, A7 #11. #4 conflicts with #6 and #9 in a
  few lines; rebase whichever merges after #4. B4 found two bugs for every platform,
  now A5 and A6 in the to-do list.
- **2026-09-28 (Linux review):** #3 and #5–#11 were tested on Linux
  (checks, all smoke drivers, throwaway drivers reproducing each bug on
  `main`) and merged. #9 needed a second fix: the reopened database got a
  second index build writing the same temp files (A5). #4 was merged as
  #12, rebased onto #6 and #9.
- From WSL, Windows programs inherit a PATH without
  `C:\Users\isaac\.cargo\bin`; prepend it in each `powershell.exe`
  call. `gh` is in `C:\Program Files\GitHub CLI\` and logged in. Commit
  and push with Windows git (`cmd.exe /c git …`); WSL git has no identity.
- The display uses custom scaling (120%), so the Scale setting is greyed
  out; test other ratios through the webview zoom
  (`core:webview:allow-set-webview-zoom` in a temporary capability).
- A release build can be driven without the mouse: UI Automation finds
  the webview's buttons (Invoke or Toggle) and inputs (Value); file
  dialogs take `WM_SETTEXT` on the name box and `BM_CLICK` on OK; keys
  posted to `Chrome_RenderWidgetHostHWND` reach the page (arrows and
  Enter did, typed letters didn't). Real input needs `SendInput` and the
  user's OK to leave the PC alone; the webview's keyboard layout belongs
  to the focus window's thread (`GetGUIThreadInfo`), not the top window's.
