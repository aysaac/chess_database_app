# Handoff

State of the project as of **2026-09-28**, for the next agent.
Conventions, commands and utilities are in [AGENTS.md](AGENTS.md); read
that first.

## TL;DR

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
| **Tabs** (new) | Opening a game reuses the current tab if it has no unsaved changes, else opens a new tab (no discard prompt). Next/previous game replaces the tab (with the prompt). Analysis follows the active tab | `app/src/lib/stores/game.svelte.ts` |
| **Reference database** (DB-13) | A path in settings (`referenceDatabase`). The backend opens it on its own handle, shared with the list when it's the same folder (`DbUse` flags in `AppState`: a database closes when neither the list nor the reference uses it). The tree picks its source (open or reference); "Find games" on the reference opens it in the list | `app/src-tauri/src/state.rs` |
| **Compaction** (new) | Rewrites into `*.new` files, commits with a `compact.commit` marker, renames; open finishes or rolls back. Game ids change; open tabs are remapped; the position index is rebuilt | `crates/chess-db/src/database.rs` |

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
  tree visibility, analysis lines), 02, 03, 05.
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
  letters aren't accepted on import.

**Engines**
- One analysis at a time (ENG-04 is P2). The blunder check uses the default
  engine and fixed thresholds (Lichess's). No engine I/O log view (logged at
  debug level only).
- Engine options: buttons (e.g. "Clear Hash") can't be pressed from the UI.

**UI**
- UI-01 has five board colour themes and display options (coordinates,
  legal-move dots, last move) under "Board appearance" in the nav bar, but
  only the cburnett piece set: more sets need vendored SVGs with checked
  licences (lichess's lila repo lists them in COPYING.md).
- The toolbar wraps to two rows in windows narrower than about 1,400 px, and the nav row
  (now with next/previous game) is wider than very small boards. The layout
  driver passes at all five sizes.
- `CommentDialog.svelte` and now `FenDialog.svelte` are unused (the setup
  editor replaced the FEN dialog). The user hasn't approved deleting them.
- Prettier isn't applied (the repo never was; `NotationLine.svelte` relies
  on exact whitespace), so don't run `npm run format` on everything.

**Not verified yet**
- Real pointer and keyboard input (dragging pieces, the setup editor's
  palette and square clicks, right-click menus, shortcuts, native file
  dialogs). chessground ignores synthetic events, so the smoke drivers can't
  click the board.
- Real input could now be tested on the Mac (`cliclick`), once the
  terminal has Accessibility permission.
- Bundles (`tauri build --config src-tauri/tauri.stockfish.json`) were built
  on macOS (`.app` and DMG) and Windows (MSI and NSIS installers), but
  neither was installed and clicked through; on Windows the release exe
  starts. Linux bundles were never built.
- Light mode was never screenshotted (the Mac can, once screenshots work).
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
5. Then catalog Phase 3 (P2): ENG-04/06/07/08/14, ANL-02/03/06, OPEN-02/03/05,
   REF-01/02, NET-01/07, SRCH-05/06/08/12…

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
  on the way is #6 (all platforms). Part B is done except B2/B3 (need a
  person at the keyboard), dark mode and other scales in B6, and B8. An
  MSI installed after an NSIS one lands in the NSIS folder (B1). B4 found two bugs for every platform,
  now A5 and A6 in the to-do list.
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
  Enter did, typed letters didn't).
