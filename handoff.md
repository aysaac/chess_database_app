# Handoff

State of the project as of **2026-10-08**, for the next agent.
Conventions, commands and utilities are in [AGENTS.md](AGENTS.md); read
that first.

## TL;DR

- **IO-03 ChessBase import (2026-10-08, merged into `main` 2026-10-10).**
  Import games (the ribbon's Database group, until now "Import PGN", and
  the empty list's button) takes a ChessBase `.cbh` as well as a PGN. The
  game comes with its variations, comments, symbols, coloured squares and
  arrows, and its players, event, site, annotator, date, result, Elos and
  ECO. The `.cbg` move-translation table was read from the user's own
  ChessBase 16 (`CBase16.exe` at 0x1416480, as decided) and is built in.
  **Mega Database 2019 imports 7,623,531 of its 7,623,697 games in 6.5
  minutes** (19.6k games/s, a 1.36 GB database). The 166 skipped are
  Chess960 and two rare encodings; 1,776 games lose a line at a null move
  (the game tree can't hold one), with a warning each. Details in [New:
  ChessBase import](#new-chessbase-import-io-03-2026-10-08) and
  docs/io-03-cbh.md §4. Checks, on a tree holding only the IO-03 changes
  (the working tree also had the unrelated, uncommitted DB-07 library
  work, under which the drivers can't find the ribbon): fmt, clippy, 264
  Rust tests (+3 ignored), svelte-check (0 errors, 0 warnings), the build,
  the ignored real-file tests on the four bundled databases, and the smoke
  drivers `cbh` (new), `ribbon`, `layout`, `basic` and `onboarding`
  off-screen (`SMOKE_BIG=0`); the monitor layout stayed the same
  (read-only `hyprctl`).

- **UI-04 docking (2026-10-01, branch `ui-04-docking`, step 4 of
  [docs/workspace-ui.md](docs/workspace-ui.md), built on `main` after the
  OPEN-14 merge, ad823a7; not merged yet).** Every panel is a tab that
  docks to any side of the window: Notation, Opening tree, Reference,
  Engine analysis and Game list. Drag a tab to a band along a window edge
  (a group of its own, next to the nearest group there) or onto another
  group's tab strip (it joins it, where the pointer is; along its own
  strip it reorders), with a label under the pointer saying what happens;
  Escape or a release elsewhere cancels. Or right-click the tab
  (Shift+F10, the context-menu key): Move to ▸ Left/Right/Top/Bottom, Move
  into ▸ another group. Empty groups and sides go and the board grows.
  Dividers between each side and the board and between groups resize them
  (drag, ←/→ or ↑/↓ by 16 px, double-click resets), stopping at 220/120 px
  sides and a 160 px board; they replace the list's divider (`listShare`
  carries over). View ▸ Reset layout. The whole layout is saved (the
  `layout` pref, now version 2) and restored. The board is the largest
  square in the middle. Details and the choices the spec left open in
  [New: docking](#new-docking-ui-04-2026-10-01) and docs/workspace-ui.md
  "As built". **This was the last step: docs/workspace-ui.md's build order
  (UI-19, UI-18, OPEN-14, UI-04) is all built;** what's left there are the
  tickets for later (T-1…T-5). Checks: fmt, clippy, 235 Rust tests (+1
  ignored), svelte-check (0 errors, 0 warnings), the build, and all 15
  smoke drivers off-screen with their 1M-game parts and Stockfish: the new
  `dock.js`, `layout` (now three layouts × six sizes), `panels`, `ribbon`,
  `reference`, `basic`, `tree`, `goto`, `search`, `setup`, `image`,
  `onboarding`, `view`, `drag`, `engine`; `dock`, `layout`, `panels`,
  `ribbon` and `reference` also in light mode. The desktop stayed
  unchanged (read-only `hyprctl` snapshots).

- **OPEN-14 Reference tab (2026-10-01, branch `open-14-reference-tab`,
  step 3 of [docs/workspace-ui.md](docs/workspace-ui.md), built on `main`
  after the UI-19/UI-18 merge; merged into `main` as ad823a7).** The third panel tab, next
  to Notation and Opening tree, with a Reference button in the ribbon's View
  group. It reads the reference database, or the open one with the note
  "Reading <name>: no reference database is set. Choose one in the ribbon."
  Four parts: the moves table with **Hot** (bar and ↑/↓) and **Best
  players**; the **main line** and up to 12 **lines**; the **top 100
  games** by average Elo; a view-only **mini board**. Details, and the
  choices the spec left open, in
  [New: the Reference tab](#new-the-reference-tab-open-14-2026-10-01).
  Checks: fmt, clippy, 235 Rust tests plus the ignored TWIC round trip,
  svelte-check (0 errors, 0 warnings), the build, and all 14 smoke drivers
  off-screen with their 1M-game parts and Stockfish: the new
  `reference.js`, `panels`, `ribbon`, `layout`, `basic`, `tree`, `goto`,
  `search`, `setup`, `image`, `onboarding`, `view`, `drag`, `engine`;
  `reference`, `panels`, `layout` and `ribbon` also in light mode
  (`GTK_THEME=Adwaita`). The desktop stayed unchanged (read-only `hyprctl`
  snapshots).
  Performance on 1M games in `tauri dev`: moves, Hot, Best players and top
  games 35 ms at the start position (92 ms the very first time, loading
  the per-game stats), 18 ms after 1.e4, 4–13 ms deeper (target 200 ms);
  lines 79 ms for a 36,333-game position and 10–42 ms for smaller ones
  (target 500 ms under 50,000 games); as jobs, 1.6 s at the start and
  0.9 s after 1.e4, then instant from the cache (more in
  [Measured performance](#measured-performance)).

- **UI-19 panel tabs (2026-09-30, branch `ui-19-panel-tabs`, step 1 of
  [docs/workspace-ui.md](docs/workspace-ui.md)).** The column beside the
  board is a tab group: **Notation** (the game header, the moves and the
  eval graph, ANL-04, at its bottom) and **Opening tree**. The engine lines
  stay under the tabs while the engine runs. The tree queries only while
  its tab shows. The toolbar's "Opening tree" now shows that tab, and "Go
  to position" does too. The active tab is saved in a versioned `layout`
  pref, the panel model docking (UI-04) will extend (`stores/panels.svelte.ts`). Details in
  [New: panel tabs](#new-panel-tabs-ui-19-2026-09-30). Checks: fmt, clippy,
  202 Rust tests (+1 ignored), svelte-check, the build, and 11 smoke
  drivers off-screen with their 1M-game parts: the new `panels.js`,
  `basic`, `layout`, `tree`, `goto`, `search`, `setup`, `image`,
  `onboarding`, `view`, `drag`. `panels` and `layout` also ran in light
  mode, the first light-mode screenshots. The desktop stayed unchanged.
  `engine.js` wasn't run: in the main checkout `app/src-tauri/binaries` is
  a symlink to itself (dated 2026-09-29 18:44), so there's no Stockfish.
  Run `scripts/fetch-stockfish.sh` again once that link is removed.
- **UI-18 ribbon (2026-09-30, branch `ui-18-ribbon`, step 2 of
  docs/workspace-ui.md, built from `main` without UI-19).** One band under
  the game tabs replaces the toolbar: labelled groups Clipboard, Game,
  Reference, Analysis, Database and View, with Lucide icons. It never
  wraps: groups from the right lose their small buttons' labels, then
  collapse into dropdowns. It finishes IO-08 (**Paste game** from a button
  or Ctrl+V, Copy game/Copy position, Paste position) and moves the
  reference database (DB-13, DB-07) into a dropdown of recent databases;
  the tree's select keeps only "open" and "reference". **Ctrl+N** is New
  game. Details and the notes for merging with `ui-19-panel-tabs` in
  [New: the ribbon](#new-the-ribbon-ui-18-2026-09-30). Also fixed: in
  windows narrower than ~1,000 px the game list widened the whole app past
  the window. Checks: fmt, clippy, 209 Rust tests (+1 ignored),
  svelte-check, the build, and 11 smoke drivers off-screen with their
  1M-game parts: the new `ribbon.js`, `basic`, `layout`, `goto`, `setup`,
  `image`, `search`, `tree`, `view`, `drag`, `onboarding`. `ribbon` and
  `layout` also ran in light mode (`GTK_THEME=Adwaita`). The desktop
  stayed unchanged. `engine.js` wasn't run: `app/src-tauri/binaries` in
  the main checkout is a symlink to itself (as the UI-19 agent found), so
  there's no Stockfish; nothing engine-side changed.
- **UI-19/UI-18 merged (2026-10-01, branch `ui-integration`).** Each was
  built independently from the same `main`, per
  [docs/workspace-ui.md](docs/workspace-ui.md)'s note that steps 2 and 3
  don't depend on step 1. Cherry-picked UI-19 first, then UI-18, with
  conflicts in `AGENTS.md`, `App.svelte` (imports, `.games` CSS),
  `tree.svelte.ts` (additive: `referenceTarget` plus the tab-aware
  `refresh()`), `main.ts` (the dev `window.__chessdb` export list),
  `docs/feature-catalog.md` and this file. `Toolbar.svelte` was a
  modify/delete (UI-19 touched it, UI-18 deleted it for `Ribbon.svelte`):
  took the deletion, then updated `Ribbon.svelte`'s View group to call
  `panels.show("notation"|"tree")` with `aria-pressed={panels.isShowing(…)}`
  instead of the old `tree.visible`/`tree.toggle()`, and added a Notation
  button there (the `history` icon; `file-text` wasn't vendored). Fixed the
  same leftover `tree.visible`/`tree.toggle()` calls in `scripts/smoke/ribbon.js`
  (three places, plus a selector in `panels.js` that looked for the old
  toolbar) to use `panels.show`/`panels.isShowing`, and a `ribbon.js`
  assertion that assumed the View group had one button (now Notation is
  first, Opening tree second). Also fixed the main checkout's
  `app/src-tauri/binaries` symlink, which both UI-19 and UI-18 found broken
  (a self-referential loop): removed it and re-ran
  `scripts/fetch-stockfish.sh`, so `engine.js` could finally run here.
  Checks on the merged tree: fmt, clippy with `-D warnings`, 209 Rust tests
  (+1 ignored), svelte-check (0 errors), the build, and all 11 smoke
  drivers off-screen with their 1M-game parts, Stockfish included this
  time (`panels`, `ribbon`, `basic`, `layout`, `tree`, `goto`, `search`,
  `setup`, `image`, `onboarding`, `view`, `drag`, `engine` — more than 11
  once `engine` is counted). `panels`, `ribbon` and `layout` were also
  screenshotted in light mode. Read-only `hyprctl` snapshots before and
  after differ only by the user's own desktop activity in the meantime
  (focus, window sizes, an idle screensaver) — nothing from the private
  offscreen display touched Hyprland. Not pushed; `main` is fast-forwarded
  to this branch next.
- **Five P2 quality-of-life features merged into `main` (2026-09-29):**
  QOL-04, 06, 07, 08 and 10, each built on its own branch from the same
  `main` and described in the bullets below. They were reviewed and
  cherry-picked one commit each in the order 10, 06, 08, 07, 04. The
  conflicts were in the docs, `dto.rs`, `settings.rs`, `GameList.svelte`
  (row selection, column resizing and the empty list's buttons now live
  together) and `db.svelte.ts`. QOL-07's tests now call QOL-08's
  `AppState::init_dirs`. One fix on top: a database created by copying
  games into it forgets any list view saved for its path, as `create_db`
  does. Checks on the merged result: fmt, clippy, 202 Rust tests plus the
  TWIC round trip, svelte-check, the build, and all 11 smoke drivers with
  their 1M-game parts and Stockfish (`goto`, `image`, `onboarding`, `view`,
  `drag`, `basic`, `layout`, `search`, `tree`, `setup`, `engine`), with the
  desktop unchanged. Not pushed yet.
- **QOL-10 "go to position" (2026-09-29, first P2 item, merged).** Ctrl+V
  outside a text box, or **Go to position…** in the toolbar, takes a FEN, a PGN
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
- **QOL-06 board images (2026-09-29, merged):** "Board image ▾" in the
  toolbar copies the board as PNG or SVG, or saves either. The image is drawn in Rust by a new
  crate, `crates/chess-diagram` (resvg), and put on the clipboard by
  `tauri-plugin-clipboard-manager`. See [Board images](#new-board-images-qol-06-2026-09-29).
  Checks: fmt, clippy, 175 Rust tests (+1 ignored), svelte-check, the
  build, and the smoke drivers `image` (new), `basic`, `layout`, `setup`,
  `search` and `tree` (not `engine`: no Stockfish in that worktree; nothing
  engine-side changed), with the desktop unchanged.
- **QOL-08 onboarding (2026-09-29, merged):** a welcome guide opens on a first run: start
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
- **QOL-07 (2026-09-29, merged):** the game list
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
- **QOL-04 (2026-09-29, P2, merged).** Select several games in the list and drag them
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
| **Panel layout** (UI-19, UI-04) | `Layout` (four sides → groups of panels → the active one; sides sized as shares of the window, groups as shares of their side) in `stores/panels.svelte.ts`, saved whole as the `layout` UI pref (version 2; version 1 and `listShare` are read once and replaced). `Dock.svelte` turns it into pixels (`sidePixels`: minimums, the middle's room for a 160 px board, proportional shrinking) on a CSS grid; the board fills the middle (`fitBoard`). Tabs are dragged with our own pointer handling, like QOL-04's games. Every panel stays mounted (hidden with `hidden`); a hidden panel does no work | `docs/workspace-ui.md` |
| **Reference tab** (OPEN-14) | One pass over the index hits and the in-memory `GameStat`s gives the moves, Hot, Best players and top games (records read only for what's shown). The lines read each game's next 10 plies, packed in `u128`s sorted into an implicit trie; from 50,000 games a `chess-jobs` job, cached in memory (32 positions) until the database changes. Choices the spec left open: docs/workspace-ui.md "As built" | `crates/chess-db/src/reference.rs` |
| **Ribbon and clipboard** (UI-18, IO-08) | The ribbon picks each group's form (full, compact, collapsed) by arithmetic on the widths of a hidden, inert copy of every group in every form, so the live buttons never move to be measured. Clipboard text is read (`clipboard_text`) and written (`copy_text`) in Rust; only Ctrl+V uses the page's paste event. `paste_game` opens the first game in place of a clean session, else in a new one. Icons are vendored Lucide (ISC). The ribbon's View group calls `panels.show(…)` (UI-19) rather than a tree-only toggle | `docs/stack.md`, `docs/workspace-ui.md` |

Still open (catalog §6): how important ChessBase `.cbh` import is (#2), the
target scale of 1M vs 10M+ games (#3), and whether play-vs-engine (the Fritz
side) belongs in v1 (#4). **#3 now has concrete numbers:** the position index
takes about as much disk as the games (740 MB for 1M Lichess games), which
would be ~7 GB at 10M. Cheaper options are in `docs/file-format.md`.

## What exists

### Features (by catalog ID)
- **Delivered:** INF-01…04 · CORE-01…04 · GAME-01…07, 16, 17 ·
  IO-01, 02, 03, 08, 11, 12 · DB-01, 02, 03, 05, 06, 13 · SRCH-01…04, 14 · OPEN-01, 11, 14 ·
  ENG-01, 02, 03, 05, 09 · ANL-01, 04 · UI-01, 02, 03, 05, 07, 09, 15, 16, 18, 19 ·
  QOL-01 (reopens the last database and remembers orientation, the panel
  layout with its sizes and active tabs, analysis lines), 02, 03, 04, 05,
  06, 07, 08, 10.
- **Partial:**
  - DB-04: players, events, sites and annotators have lists with counts,
    "show games" and rename/merge. No sources or teams, and no per-player
    dossier (REF-01 is P2).
  - DB-07: recent databases (the ribbon's Recent ▾ and reference dropdown,
    empty-list shortcuts), close, size and deleted count in the status bar.
    No library window, folders or favourites (T-3 in docs/workspace-ui.md).
  - GAME-08: clocks show as tooltips; no time graph.
  - ENG-01: engines can be added, removed, made default and configured
    (common options plus "show all"), but there are no per-engine profiles.
  - UI-01: five board colour themes, but one piece set (cburnett).
  - UI-04: panel tabs dock to the four sides, resize and are remembered
    (the first pass of docs/workspace-ui.md). Free docking, floating
    panels, several boards and named layouts are T-1 (with UI-14).

### New: ChessBase import (IO-03, 2026-10-08)
- **What the user sees:** Import games opens the file dialog on "PGN files
  and ChessBase databases" (or either alone). A `.cbh` imports as a job
  like a PGN, with the same progress, Cancel, notice and import report. A
  `.cbh` whose other files are missing fails with "Mega.cbg is missing. A
  ChessBase database needs its .cbh, .cbg, .cbp, .cbt and .cbc files in
  one folder: copy them there and import again." Warnings read like PGN
  import's: "The main line stops at 23...-- (a null move), which can't be
  imported yet.", "Chess960 games can't be imported yet."
- **Reading:** `crates/chess-db/src/cb/` (spec: docs/io-03-cbh.md, which
  has the byte layouts and what the real files showed). A move the game
  tree can't take (a null move, an illegal one) cuts its line there and the
  rest of the stream is still read, as in PGN import; `.cba` annotations on
  cut moves are dropped. A comment stored in several languages keeps the
  any-language text and one language, English first (ChessBase's own
  databases have German, Spanish, French and Russian). Set-up positions
  drop castling rights the board rules out (ChessBase stores 0x0f by
  default) and impossible en passant files.
- **Bugs found on the real files** (the decoder had only seen hand-made
  streams): the castled rook went to the d-file in the slot table; a
  captured pawn's slot kept its square and later caught another pawn's
  moves; the set-up en passant square was on the wrong rank. Each has a
  test now.
- **The table:** the first scan of the ChessBase install (2026-10-03)
  looked like a failure only because its check compared plies with
  `.cbh`'s count of full moves. With that fixed, `extract_cb_key` finds
  exactly one table that decodes all 394 games of the four bundled
  databases.
- **Code:** `cb/moves.rs` (`MODE0_TABLE`, `decode_mode0` →
  `DecodedGame { game, order, problems }`), `cb/annotations.rs`,
  `cb/import.rs` (`import_cb(db, cbh_path, progress)`), `cb/header.rs`
  (`GameRecord::main_line_matches`); the `import_cb` command and
  `spawn_import` (shared with `import_pgn`) in `commands/db.rs`;
  `api.importCb`; `db.importFrom` picks by extension.
- **Tests:** unit tests in `cb::` (29); ignored integration tests on the
  real files in `crates/chess-db/tests/cb_import.rs` (`CBH_SAMPLES`,
  `CBH_MEGA`, defaults under `testdata/large/`); the smoke driver
  `scripts/smoke/cbh.js`. The real databases aren't committed; on this
  machine `testdata/large/cbh-samples/` and `testdata/large/Mega Database
  2019.*` are symlinks to `/home/aysaac/Windows/`.
- **Not yet:** Chess960 and the other encoding modes, null moves, `.cbj`,
  sources, teams, eval and clock annotations, guiding texts, a choice of
  comment language, and `.cbh` in the welcome guide's import wizard (it
  picks PGN files only).

### New: docking (UI-04, 2026-10-01)
- **What the user sees:** the same layout as before at first: Notation,
  Opening tree and Reference as tabs on the right, Engine analysis under
  them, the game list at the bottom. Now every group has a tab strip, even
  with one tab ("Engine analysis", "Game list", whose own Games/Players…
  tabs stay under it). While the engine is off its tab says "The engine is
  off." with an Analyze button, and the group takes only that line; when it
  runs, it takes a quarter of the side, with a divider above it.
  - **Dragging a tab** (4 px): a band along each window edge (the top one
    over the ribbon's lower edge, the bottom one over the status bar) and
    every tab strip become drop targets. The label under the pointer says
    "Dock on the left" (an empty side, previewed at the size it will open
    with), "Dock on the right, below Engine analysis" (a line marks the
    place), "Move into Game list" or, along its own strip, "Move Reference
    here" (a caret between the tabs). Escape, or a release anywhere else,
    cancels.
  - **The tab's menu** (right-click, Shift+F10, the context-menu key):
    "Move to" Left, Right, Top, Bottom (on its own side, it splits off
    next to its group; off when it's alone there) and "Move into" each
    other group, named by its tabs ("Notation · Reference"). The focus
    follows the tab.
  - A group without tabs goes; a side without groups closes and the board
    grows. A side that opens takes its room from the side across the board
    first, as long as that one keeps as much (Notation to the left halves
    the right side instead of shrinking the board).
  - **Dividers** between each side and the board, and between groups: drag
    them, or focus one (Tab) and press ←/→ or ↑/↓ (16 px); a double-click
    resets. `role="separator"` with `aria-orientation`, `aria-valuenow`,
    min and max. Sides stop at 220 px (left, right) and 120 px (top,
    bottom), the middle at a 160 px board with its padding, eval bar and
    nav bar; groups at 80 px stacked, 160 px side by side.
  - **View ▸ Reset layout** in the ribbon (Lucide `layout-dashboard`).
  - **The board** is the largest square in the middle, centred. In a middle
    under 480 px the nav bar is compact (Flip board as an icon), and under
    about 330 px it wraps to two rows; `fitBoard()` measures its height.
  - **Panels at any size:** each scrolls inside its group; the page never
    scrolls sideways. The game list drops columns from the right of a
    priority order when narrow (White, Black, Result, Date, the Elos,
    Event, #, Opening, ECO, Moves, Site, Round, flags), its filter box,
    the name lists and the empty-list buttons shrink or wrap, and its own
    tabs scroll. Panel tabs shrink with an ellipsis (full name in the
    tooltip). The drop shelf for dragged games (QOL-04) covers the
    notation's group wherever it's docked.
- **Sizes and the pref:** sides are shares of the window's width or height
  (defaults: right 0.6, bottom 0.28, left 0.3, top 0.25), groups shares of
  their side. `layout` is now version 2 and holds everything (sides,
  groups, order, active tabs, sizes; no group ids). A version 1 layout
  (UI-19) gives the default with its active tabs; `listShare` becomes the
  bottom side's size; both older prefs are removed on the first save. A
  panel a saved layout lacks joins its default neighbours' group; anything
  unreadable gives the default.
- **Code:** `stores/panels.svelte.ts` (the model, `sidePixels`, `move`,
  `resizeSide`, `resizeGroups`, `reset`, `restoreLayout`, `parseLayout`),
  `stores/dock.svelte.ts` (the tab drag: threshold, hit testing of strips
  then bands, the label, Escape; and the menu's state), `Dock.svelte`
  (the grid of sides, dividers and the middle, in the order they're seen,
  so Tab goes top, left, board, right, bottom), `Divider.svelte`,
  `DockOverlay.svelte`, `TabMenu.svelte`, `PanelGroup.svelte` (a strip on
  every group, drag and menu, the shelf host), `Panel.svelte` (the engine
  off line), `App.svelte` (`fitBoard()` from the middle, `middleMin()`),
  `BoardNav.svelte` (compact, wrapping), `GameList.svelte` (columns by
  priority). `listShare`, the list resizer and `.game-column` are gone;
  drivers get `dock` in `window.__chessdb`.
- **Tests:** `scripts/smoke/dock.js` (see AGENTS.md); `layout.js` runs its
  six sizes in three layouts (default, the notation at the bottom,
  everything on the left) and checks the board inside the middle, clear of
  every side and divider, the sides' minimums, the strips and the game
  list's columns fitting, and no sideways scroll; `panels.js`,
  `reference.js` and `goto.js` find the tab strip and the list by panel
  ids instead of `.game-column`/`.games`; `panels.js` expects version 2;
  `ribbon.js` checks Reset layout. With the new View column the ribbon
  goes compact one group earlier (Database and View at 1,200 px).

### New: the Reference tab (OPEN-14, 2026-10-01)
- **What the user sees:** a **Reference** panel tab after Notation and
  Opening tree (also View ▸ Reference in the ribbon). Its header has the
  database, the opening and the number of games, and Find games. Below,
  three sections with dividers (drag them, or focus one and press ↑/↓;
  double-click resets; sizes saved as the `referenceSizes` pref):
  - **Moves** (3a): the tree's columns (Games, Score, the White/draw/Black
    bar, Elo, Year; the row cells are shared with the tree through
    `StatsHead`/`StatsCells`), then **Hot**: a bar with the move's share
    of the games from the last three years of the database, ↑/↓ when that
    share moved, and a tooltip "38% of games from 2018–2020 (70% over all
    years)"; and **Best players**: up to four surnames, full name and Elo
    as tooltip. A move plays on the main board.
  - **Main variations** (3b): "Main line: 1.e4 e5 2.Nf3 … (20 games)", then
    up to 12 lines grouped by first move (a rule between groups), with
    games, score, last year and average Elo. Clicking one shows it on the
    mini board. For positions reached by 50,000 games or more they're
    counted by a job: "Counting the lines of 585,292 games… 42%" with
    Cancel; moving on cancels it quietly.
  - **Top games and the mini board** (3c, 3d): the 100 strongest games
    (average Elo of both players). Click or ↑/↓ shows one on the mini
    board, where ⏮ ◀ ▶ ⏭ (and ←/→/Home/End on it, ←/→ in the list) step
    through it; ⟲ goes back to the board's position, as any move on the
    main board does. Enter or a double-click opens the game on the main
    board where it reaches the position (in this tab unless it has unsaved
    changes); its menu also opens it in a new tab. Side by side when the
    tab is wide, stacked under 560 px (a container query); under 480 px
    the moves table drops the bar and year.
  - While a new position loads, the last one stays dimmed, so the tab
    doesn't flash. Past the indexed plies it shows the tree's note and its
    "Search every move of every game".
- **Rust (chess-db, `reference.rs`):** `reference_overview` (tree stats,
  Hot, Best players and top games in one pass over the index hits and the
  in-memory `GameStat`s; names are read only for the strongest candidates,
  64 at a time until four players, and records only for the 100 games
  shown) and `reference_lines` (reads each game's next 10 plies after the
  position with the new `blob::mainline_window`, through `for_each_game`;
  packs them into `u128`s that sort like the lines, so the lines' trie is
  ranges of one sorted array; the main line filters by ply). Also
  `game_mainline` for the mini board, `ranked_moves` shared with
  `position_tree`, and `Database::blob_of` (one record read per game in
  `for_each_game` instead of two, which the index build uses too). 10 unit
  tests on `testdata/pgn/reference.pgn` (56 games made for known counts:
  generated, the groups described in the test) and generated databases:
  ties, missing Elos and years, deleted games, games ending inside the 6
  plies, the 10-ply and 10% rules, the 12-line cap, exact Hot thresholds.
- **Tauri (`commands/reference.rs`):** `reference_view(handle, session)`,
  `reference_lines(handle, session)` (the lines, or a job id below
  `ReferenceLinesDto`; ask again when the job is done: it put them in
  `AppState::lines`, an LRU of 32 keyed by handle, position hash, the
  database's change counter and indexed games) and `game_line(handle, game,
  session)` (positions as FEN/SAN/UCI/move number, and where the game
  reaches the board's position, by hash). `open_game` takes an optional
  `ply`. DTOs: `ReferenceDto`, `ReferenceMoveDto`, `HotDto`, `TrendDto`,
  `BestPlayerDto`, `TopGameDto`, `LinesDto`, `ReferenceLineDto`,
  `LinePositionDto`, `GameLineDto`, `ReferenceLinesDto`.
- **Frontend:** `stores/reference.svelte.ts` (the `#seq` guard as in the
  tree store; queries only while `panels.isShowing("reference")`; a hidden
  tab or a new position cancels the lines job it started, and if the board
  comes back before that job ends it starts again), `ReferencePanel`,
  `ReferenceMoves`, `ReferenceLines`, `TopGames`, `MiniBoard` (a second,
  view-only chessground with the `chessboard` class, sized in whole device
  pixels like the main board). The reference dropdown's tooltip now says
  "what the Reference tab and the opening tree read".
- **Choices the spec left open** are listed in docs/workspace-ui.md under
  "As built": ↓ needs 5 recent games expected at the move's old share; a
  first move always makes a line; tie-breaks; ←/→ in the top games step
  the mini board.
- **Tests:** `scripts/smoke/reference.js` (see AGENTS.md); `panels.js` now
  expects three tabs (End reaches Reference) and `ribbon.js` checks the
  View group's three buttons.

### New: panel tabs (UI-19, 2026-09-30)
- **What the user sees:** a tab strip over the column beside the board,
  with **Notation** (shown by default) and **Opening tree**. Notation holds
  the game header, the moves and the eval graph (ANL-04, unchanged, at the
  bottom; still only with 2+ evaluated moves). The tree lost its × and its
  "Opening tree" title (the tab says it); the toolbar's "Opening tree"
  shows its tab instead of toggling it. Engine lines stay under the tabs
  while the engine runs. Panel tabs look like the list's tabs (Games,
  Players…), not like the game tabs above the board.
- **Code:** `stores/panels.svelte.ts` (the model: `PanelId`, `Side`,
  `Group`, `Zone`, `Layout`, `defaultLayout`, `parseLayout`,
  `restoreLayout`, and the `panels` store with `isShowing`/`show`),
  `PanelGroup.svelte` (tab strip and panels), `Panel.svelte` (panel id →
  component), `NotationPanel.svelte`. `App.svelte` renders the right and
  bottom sides from the layout. Smoke drivers get `panels` in
  `window.__chessdb`.
- **Queries only while showing:** `tree.showing` is `panels.isShowing("tree")`;
  `OpeningTree`'s effect does nothing while it's false, so the tree queries
  when its tab shows and on each move after that. `tree.visible` and
  `tree.toggle()` are gone. The last tree stays loaded while hidden;
  `tree.current` shows it only for the same position *and* database, so
  going back to the tab at the same position is instant and never shows
  another database's numbers.
- **Move popovers:** a hidden tab (`hidden`, so `display: none`) hides
  everything in it, `position: fixed` popovers included, and a tree move
  can need the variation choice. So the variation popover and the
  next-move picker moved from `Notation.svelte` to `MovePopovers.svelte`
  (mounted once in `App.svelte`). They open under the current move as
  before, or under the tab strip while the Opening tree shows.
- **Keyboard:** WAI-ARIA tabs (`tablist`/`tab`/`tabpanel`, roving
  `tabindex`, automatic activation): ←/→ (wrapping), Home, End. These keys
  don't reach the game while the focus is in the strip; elsewhere the
  arrows step through the game as before. No new shortcut.
- **Saved:** the `layout` pref (`{version: 1, zones: {left, right, top,
  bottom}}`) is written when a tab is chosen. At start, the default layout
  takes each saved group's active tab. With no readable `layout`, the old
  `treeVisible` pref decides (so QOL-01's restore carries over); it's
  deleted when the layout is first written. Anything unreadable gives the
  default. The default's sizes and shares (right 0.6 with groups 0.75 and
  0.25, bottom 0.28) are placeholders: nothing applies them before UI-04,
  which should replace them with real ones and carry `listShare` over.
- **For later steps:** `PANEL_TITLES` already names `reference`; step 3
  adds it to `defaultLayout()`'s first right group and to `Panel.svelte`.
  `restoreLayout` keeps the default structure, so a saved step-1 layout
  needs no migration when a panel is added. Docking (UI-04) replaces
  `restoreLayout` with the full saved structure (`parseLayout` already
  checks it) and adds moves and sizes to the store. `fitsContent` (the
  analysis group takes its content's height) is a stand-in until groups
  get their shares. A group of one panel has no tab strip yet (analysis,
  game list); docking will need one on every group to drag from.
- **Tests:** `scripts/smoke/panels.js` checks the tabs' ARIA wiring, the
  graph inside Notation, query counts (none while hidden, one when shown,
  one per move), both popovers opening from the tree tab, the keys, the
  toolbar button, and the saved pref; then it reloads the page three times
  to check the restore, the `treeVisible` migration and an unreadable
  layout. `layout.js` now checks the tabs fit at every size; `tree.js` and
  `goto.js` show the tab through `panels`. In `tauri dev` on 1M games
  (unchanged): tree 101 ms at the start and after 1.e4, position search
  20 ms.

### New: the ribbon (UI-18, 2026-09-30)
- **What the user sees:** one band under the game tabs (where
  docs/workspace-ui.md puts it), in six labelled groups. Large buttons have
  the icon over the label; small ones icon and label on one line, up to
  three to a column; each group's caption is under it.
  - **Clipboard:** Paste game (large), Paste position, Copy game, Copy
    position, Board image ▾.
  - **Game:** New game ▾ (large split button: the top does New game, the
    arrow opens "New game (Ctrl+N)" and "Set up position…"), Save, Save
    as new, Undo, Redo.
  - **Reference:** the reference dropdown, Find games (the games reaching
    the position in the reference database, which opens in the list, or in
    the open database when none is set), Go to position….
  - **Analysis:** Analyze (pressed while the engine runs), Blunder check…,
    Engines…. **Database:** New database, Open database, Recent ▾, Import
    PGN, Export PGN. **View:** Opening tree (pressed while shown).
  - Titles keep the old shortcut hints; disabled states are as before.
- **Narrow windows:** from the right, one group at a time, small buttons
  drop their labels (icon only; the label stays the `aria-label` and the
  tooltip), then groups collapse into a button named after the group that
  opens it as a dropdown. In `ribbon.js`: everything labelled at 1,500 px;
  View compact at 1,200; Analysis to View at 1,000; all compact at 800 and
  720; Reference to View collapsed at 640; all collapsed at 480. About
  440 px is the floor: narrower, the row runs off the right edge. How:
  `Ribbon.svelte` renders each group in each form in a hidden, inert copy
  (`.ribbon .measure`), and a ResizeObserver on the bar and on that copy
  picks the roomiest level that fits, by arithmetic. The live buttons are
  never re-laid out to measure, so there's no flicker and no
  ResizeObserver loop; fonts loading or a label changing refit too.
- **Keyboard:** Tab reaches every button in screen order. Dropdowns
  (`RibbonDropdown.svelte`) follow the menu button pattern: Enter, Space
  or ↓ opens and focuses the first item, ↑/↓/Home/End move, Escape closes
  and returns the focus to the button (an inner menu first, then its
  collapsed group). A click on an item closes the dropdown, except items
  marked `data-keep-open` (the board image sizes, Recent's ×). Letters
  don't start a typed move while a dropdown has the focus. **Ctrl+N** (Cmd+N)
  is New game: in the tab unless it has unsaved changes, as before. It was
  free (App.svelte's handler and every component's were checked).
- **Clipboard (IO-08):**
  - **Paste game** reads the clipboard in Rust (`clipboard_text`; empty or
    an image is `null`) and calls `paste_game(session, text)`. The first
    game with a move or a tag with a known value opens as a new unsaved
    game, in place of the session's game unless it has unsaved changes (Rust
    checks), else in a new session, which the frontend shows in a new tab.
    Notices: "Pasted the game." or "Pasted the first of 5 games.", plus the
    first problem ("The main line stops at 2.Ke3, …"); no game: "The
    clipboard has no PGN game. Copy a game with its moves, or use Paste
    position for a FEN."; a game that can't be read (a variant, a bad FEN
    tag): "Can't open the pasted game. It's a Crazyhouse game, …".
  - **Paste position** does what Go to position… does with the clipboard's
    text. **Copy game** and **Copy position** are the old Copy PGN and Copy
    FEN, now written through Rust (`copy_text`): WebKit refuses
    `navigator.clipboard.writeText` outside a user gesture, and Copy PGN
    awaited the PGN first. Drivers fill the clipboard with `api.copyText`.
  - **Ctrl+V** outside a text box tries a position, then a game; nothing:
    "The clipboard has no game or position. Copy a PGN game, a FEN or a
    Lichess link and try again." One change to the spec's order: text with
    PGN tags is tried as a game first, so a game from a set-up position
    opens with its moves instead of as its `[FEN]` tag's position. A lone
    `[FEN "…"]` line still goes to the position (it's no game).
  - `lib/paste.ts` holds the three flows; `FenPaste.svelte` calls
    `pasteText`.
- **Reference dropdown (DB-13, DB-07):** a select-like box with the
  reference's name or "No reference database". Its menu: a "Recent
  databases" section (`db.recent`, the open one included; the reference
  too if it was never opened in the list), the current reference ticked,
  gone ones disabled and marked "Not found", then "Choose a database…",
  "Use the open database (<name>)" unless it's the reference already, and
  "Stop using a reference database" (disabled with none). Sections are a
  list, so favourites (T-3) can go first. Picking calls `db.setReference`;
  the tree follows when its source is the reference. The tree's select
  keeps two choices: the open database and the reference ("No reference
  database", disabled, when there's none). The tooltip says "what the
  opening tree reads" until the Reference tab (OPEN-14) exists; then use
  the spec's "what the Reference tab and the opening tree read".
- **Code:** `Ribbon.svelte` (groups as snippets, the fitting),
  `RibbonButton.svelte`, `RibbonDropdown.svelte`, `ReferenceMenu.svelte`,
  `Icon.svelte` with `lib/icons/lucide.ts` (28 icons, licence in
  `lib/icons/COPYING`), `lib/paste.ts`; `RecentMenu` and `BoardImageMenu`
  now use `RibbonDropdown`; `Toolbar.svelte` is gone. Rust:
  `commands/clipboard.rs` (`clipboard_text`, `copy_text`) and `paste_game`
  in `commands/game.rs` (`PastedGameDto`; it reads with chess-core's
  `PgnReader`, and warnings are labelled with `chess_db::game_label`, now
  public). `tree.referenceTarget` and `tree.findGames(full, target)` serve
  the ribbon's Find games. Drivers get `notices` in `window.__chessdb`.
- **Also fixed:** the game list's column minimums (~1,000 px) widened the
  whole app in narrower windows, so `layout.js`'s "tiled" (960) and
  "narrow" (720) sizes were really 1,000 px wide, with the toolbar and the
  notation cut off at the window's edge. `.games` now has `min-width: 0`
  and clips the list on the right; `layout.js` checks that nothing is wider
  than the window and that the ribbon is one whole row.
- **Merging with UI-19** (`ui-19-panel-tabs`, built in parallel):
  - `Toolbar.svelte` is deleted here and edited there (Opening tree and Go
    to position show the tab): keep the deletion and port those into the
    ribbon. The View group then gets **Notation** and **Opening tree**
    buttons calling `panels.show(…)`, with `aria-pressed` from
    `panels.isShowing(…)`, replacing this branch's `tree.visible` toggle
    (`tree.visible`/`tree.toggle()` are gone in UI-19). Reference (OPEN-14)
    and Reset layout (UI-04) join it later. The View group is a snippet in
    `Ribbon.svelte`; nothing else needs to change for more buttons.
  - `gotoPosition.ts`: keep UI-19's version (it shows the tree tab).
  - `App.svelte`, `AGENTS.md`, `basic.js`, `goto.js`, `tree.js`,
    `layout.js`, `main.ts` and the catalog are edited on both branches.
    The GTK_THEME line in AGENTS.md is word for word the same.

### New: board images (QOL-06, 2026-09-29)
- **"Board image ▾"** (the ribbon's Clipboard group): Copy as PNG, Copy as SVG,
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
  **Go to position…** in the ribbon (a box to paste into). Accepted: a
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
- **Analyze** (ribbon, Ctrl+Shift+A): Stockfish runs with 1–5 lines, depth,
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
| Reference tab (OPEN-14), release (`import_bench`) | Moves + Hot + Best players + top games: 19.5 ms at the start (1M games), 13 ms after 1.e4, 0.8–1.7 ms deeper. Lines: 1.4 s at the start, 0.75 s after 1.e4 (jobs), 58 ms for 1.d4 d5 2.c4 (36,333 games), 26 ms for 1.e4 c5 2.Nf3 d6 (15,921), 6 ms for the Najdorf (3,101) |
| Mega Database 2019 (IO-03, `import_bench` on the `.cbh`, 7,623,531 games, 754 MB `.cbg`) | Import 388 s (19.6k games/s), 670 MB peak RSS; DB 1,355 MB; open 139 ms; sort by White 656 ms; read a game ~150 µs; header search 150 ms; position index 93 s, 364M positions, 5.8 GB; tree at the start 459 ms, after 1.e4 92 ms; Reference lines at the start 9.8 s (a job) |
| Reference tab in `tauri dev` (`reference.js`, round trips through IPC) | Moves + Hot + Best players + top games: 92 ms the first time (loads the per-game stats), then 35 ms at the start, 18 ms after 1.e4, 4–13 ms deeper (target 200 ms). Lines: 79 ms for 36,333 games, 42 ms for 15,921, 10 ms for 3,101 (target 500 ms under 50,000 games); as jobs 1.64 s at the start, 0.86 s after 1.e4; 1–6 ms from the cache. The tab on screen at the start: 125–130 ms |

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
- The ribbon (UI-18) shows every label from 1,200–1,280 px up and fits any
  window from about 440 px (all groups collapsed); the old toolbar's
  wrapping below 1,400 px is gone (the toolbar itself is gone, replaced by
  the ribbon in the UI-19/UI-18 merge). It's 97 px tall against the
  toolbar's ~36, so in short windows (the 1600×520 layout size) the board
  is at its 160 px minimum. Hiding the captions in short windows would win
  back ~16 px if that matters.
- The nav row under the board (with next/previous game, ~444 px) no
  longer cuts anything off (UI-04): in a narrow middle it turns compact
  (Flip board as an icon) and then wraps to two rows.
- Switching databases right after an import into the first can log
  "database 1 not found" from the index loop of the database just closed
  (seen in `ribbon.js`, which creates two databases in a row); no notice
  stays up.
- Docking (UI-04): sides are shares of the window, so in a narrow window
  the default layout's right side (60%) leaves a smaller board than the
  old "notation gets 320 px, the board the rest" did (at 720 px wide: 236
  against ~350 px); drag the divider. A drag moves one tab, not a whole
  group. Dividers have no Home/End. While the engine is off there's no
  divider next to its group (it takes only its content's height). Panel
  tabs ellipsize in a narrow group (5 tabs in 220 px leave a few letters
  each). A tab moved to another group remounts its panel (the game list
  forgets its scroll position and Players/Events tab). Free docking,
  floating panels and named layouts are T-1.
- Reference tab (OPEN-14): the lines cache is in memory only (a restart
  counts the start position's lines again, a job of ~2 s on 1M games). The
  top games can't be re-sorted by column (the spec's first pass), and in a
  narrow tab they scroll sideways. The mini board shows no check and no
  arrows. Best players are by Elo in the game only (no FIDE-ID merging of
  name variants; DB-09). The moves and lines tables are reached by Tab
  (each move and line is a button), not by arrow keys like the top games.
  No Popularity or Endgames sub-tabs (T-2).
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
- Light mode: only `panels.js`, `ribbon.js`, `layout.js`, `reference.js`
  and `dock.js` were screenshotted in it (2026-09-30/10-01,
  `GTK_THEME=Adwaita`, see AGENTS.md); the other drivers' screens haven't
  been.
- Docking (UI-04) with real input: `dock.js` drags tabs and dividers with
  synthetic pointer events at the elements' real positions and sends the
  keys itself (Shift+F10, the context-menu key, arrows, Escape). A real
  right-click's menu, WebKitGTK/WebView2/WKWebView pointer delivery while
  dragging over the bands, and releasing outside the window (which cancels)
  are untested.
- The Reference tab (OPEN-14) with real input: the dividers were dragged
  with synthetic pointer events, and the keys sent by the driver. Real
  WebKitGTK/WebView2 pointer delivery while dragging past the divider is
  untested (the same window-listener pattern as the list's column edges).
- The ribbon (UI-18) with real input: `ribbon.js` clicks with `.click()`
  and sends keydowns itself, so a real Tab walk, a real Ctrl+N (WebView2
  debug builds keep the browser's shortcuts on; check that Ctrl+N doesn't
  also open a window there) and real Escape focus handling on macOS (where
  a click doesn't focus a button) are untested. Reading the clipboard
  (Paste game, Paste position) was only tried on the private sway display
  (arboard over Wayland data-control); Windows, macOS and GNOME (no
  data-control: arboard needs XWayland there) are untested, like B10 in
  `docs/windows-testing.md`.
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

1. **Workspace UI (agreed 2026-09-30):** [docs/workspace-ui.md](docs/workspace-ui.md)'s
   steps 1 (UI-19 panel tabs) and 2 (UI-18 ribbon, with IO-08 paste game and
   the reference dropdown) are built and merged (2026-10-01; see
   [New: panel tabs](#new-panel-tabs-ui-19-2026-09-30) and
   [New: the ribbon](#new-the-ribbon-ui-18-2026-09-30)). Step 3, OPEN-14
   (the Reference tab), is merged ([New: the Reference tab](#new-the-reference-tab-open-14-2026-10-01)).
   Step 4, UI-04 (docking), the last one, is built on branch
   `ui-04-docking`, to be reviewed and merged
   ([New: docking](#new-docking-ui-04-2026-10-01)); with it the spec's
   build order is done. Left in it: the tickets T-1…T-5 (free docking,
   Reference sub-tabs, favourite databases, more panel tabs) and T-4 (what
   the Opening tree is for, now that the Reference tab has its moves),
   which is for the user to decide.
2. **Ask the user** about open question #3 (1M vs 10M+ games), and
   whether to set up CI on the GitHub repo.
3. **P1 polish left (on hold, the user said not now):**
   - UI-01 piece sets: vendor a few SVG sets with GPL-compatible licences
     (check lichess-org/lila's COPYING.md per set) and add a picker to
     "Board appearance".
4. **Housekeeping:** CI on GitHub Actions (Linux/Windows/macOS: fmt,
   clippy, tests, svelte-check, `tauri build`). CI needs
   `scripts/fetch-stockfish.sh <triple>` before a bundled build.
5. **Windows:** the agent on the Windows PC works through
   [docs/windows-testing.md](docs/windows-testing.md) and sends PRs; they
   are reviewed here with the Linux smoke harness before merging.
6. Catalog Phase 3 (P2) has begun: QOL-04, 06, 07, 08 and 10 and UI-18 are in. Next:
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
