# AGENTS.md

Guidance for AI agents (and humans) working in this repo. Read this first,
then [handoff.md](handoff.md) for where the project stands and what's next.

**The project:** a desktop chess database and analysis app, an open
alternative to ChessBase and Fritz. Rust core, Tauri 2 shell, Svelte 5 +
TypeScript UI. GPL-3.0-or-later. Linux first; the code is kept portable to
Windows and macOS.

## Where things are

| Path | What |
|---|---|
| `crates/chess-core` | Chess domain: rules glue over `shakmaty`, notation, game tree (`game/mod.rs`), editing session with undo (`game/editor.rs`), PGN reader and writer (`pgn/`) |
| `crates/chess-jobs` | Background jobs: threads, throttled progress, cooperative cancellation |
| `crates/chess-db` | Native database: file format, crash-safe commits, import/export, sorted views, header search, position index (`positions.rs`), compaction |
| `crates/chess-engine` | UCI engine host, analysis driver thread, blunder check (`review.rs`); `src/bin/fake_uci.rs` is a toy engine for tests |
| `app/src-tauri` | Tauri shell: commands (`src/commands/`), DTOs (`src/dto.rs`), view building (`src/view.rs`), state (`src/state.rs`), engines and analysis (`src/engines.rs`), settings (`src/settings.rs`) |
| `app/src` | Svelte UI: `lib/api.ts` (typed commands), `lib/stores/` (state), `lib/components/`, `lib/bindings/` (generated types) |
| `docs/feature-catalog.md` | Every ChessBase/Fritz feature, with IDs (`DB-01`, `ENG-02`…), priority tiers P0–P5 and dependencies. **Features are referred to by these IDs** |
| `docs/stack.md` | Stack decisions and the cross-platform rules |
| `docs/file-format.md` | On-disk database format spec and benchmark numbers |
| `docs/windows-testing.md` | Windows to-do list: code fixes and checks for the agent on the Windows PC, with how to verify each |
| `testdata/pgn/` | Small edge-case PGN fixtures (committed) |
| `testdata/large/` | Big real-world PGNs (git-ignored; see `scripts/fetch-testdata.sh`) |
| `scripts/smoke/` | End-to-end smoke test harness for the running app |

## Commands

Run from the repo root unless noted.

```sh
# Run the app (hot reload for the UI; Rust changes trigger a rebuild)
cd app && npm install && npm run tauri dev

# All checks: run these before saying you're done
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # also regenerates app/src/lib/bindings/
cd app && npm run check           # svelte-check (TypeScript + Svelte)
cd app && npm run build           # production frontend build

# Regenerate the TypeScript types after changing a DTO in app/src-tauri/src/dto.rs
cd app && npm run bindings

# Stockfish 19 for this machine → app/src-tauri/binaries/ (git-ignored, ~100 MB)
scripts/fetch-stockfish.sh

# Real-world data and performance
scripts/fetch-testdata.sh         # TWIC issue + first 1M Lichess games → testdata/large/ (~2.2 GB)
cargo run --release -p chess-db --example import_bench -- testdata/large/lichess_1m.pgn
cargo run --release -p chess-db --example import_bench -- path/to/existing.chessdb   # skip the import
cargo test --release -p chess-db -- --ignored        # lossless import → export → import on TWIC
LARGE_PGN=path/to.pgn cargo test --release -p chess-db -- --ignored

# End-to-end smoke test of the real app, on a private headless display (see below; needs sway)
scripts/smoke/run.sh
# The same on Windows (the window opens on the desktop; PowerShell 5.1+)
powershell -ExecutionPolicy Bypass -File scripts\smoke\run.ps1 [scripts\smoke\layout.js]

# Any other window you need to open: run it off-screen, never on the user's desktop
scripts/offscreen.sh COMMAND [ARGS...]
```

## Utilities

### Smoke test harness (`scripts/smoke/`)

`scripts/smoke/run.sh [driver.js] [out-dir]` launches the app in dev mode
on a private headless display and runs a driver script inside the webview.
It exits 0 only if the driver logs `done`. It doesn't touch the user's
desktop, so it's safe to run while they work.

- **How it works:** debug builds only. `app/src-tauri/src/lib.rs` evaluates the file in
  `CHESSDB_SMOKE` after page load. `app/src/main.ts` exposes
  `window.__chessdb = { db, game, jobs, api, invoke }`. The driver calls the
  stores exactly as the UI does and reports through the `dev_log` command
  as `[smoke] …` lines.
- **Default driver:** `scripts/smoke/basic.js`. It imports the fixtures, sorts, opens a
  game, picks a branch in the next-move picker (with its arrows), changes
  the board appearance, adds a variation through the popover, types
  comments inline, promotes the variation, undoes, saves, reopens, folds
  variations, types moves, inserts a move, edits the intro comment,
  promotes a pawn and lists import warnings, asserting each step. With `testdata/large/lichess_1m.pgn`
  present it also imports 1M games and checks scrolling and sorting
  (`SMOKE_BIG=0` skips that).
- **Layout driver:** `scripts/smoke/layout.js` resizes `#app` to several window
  sizes (tiled, full screen, narrow, short) and checks the board stays square,
  inside its column and clear of the notation, with the promotion picker
  inside the board. Run it after layout changes.
- **Feature drivers:** `engine.js` (analysis, lines, blunder check; needs
  `scripts/fetch-stockfish.sh`), `search.js` (filters, header search, tabs,
  name lists, compaction, recent databases), `tree.js` (position index,
  opening tree, position search, opening names, reference database),
  `setup.js` (position editor). `search.js` and `tree.js` time the 1M-game database when `__BIG__`.
- **Never open native dialogs from a driver:** nobody can click them on the
  private display, so the driver hangs until it times out. Every driver
  starts with `game.confirmDiscard = async () => true`.
- **Settings:** the harness sets `CHESSDB_CONFIG_DIR` to `<out-dir>/config`,
  so runs never touch the user's settings.
- **Writing a driver:** a new flow is a new driver. Copy `basic.js`; the placeholders
  `__ROOT__`, `__OUT__` and `__BIG__` are filled in by `run.sh`. Log
  `shot:<name>` to request a screenshot and `FAILED: …` to fail.
- **Screenshots:** taken with `grim` of the whole private display (the app
  window fills it), into `<out-dir>/shots/`. Look at them: a passing driver
  doesn't prove the layout is right. The display is 2560×1600 at scale 1.25
  like the user's screen (`OFFSCREEN_SIZE`, `OFFSCREEN_SCALE`), and the
  window has focus, so focus rings render.
- **Ports and output:** it uses dev port 1430 (`SMOKE_PORT`), so it can run next to a
  normal `tauri dev` on 1420. Output goes to `target/smoke/<timestamp>/`,
  including the display's log (`sway.log`).
- **Windows:** `scripts/smoke/run.ps1` takes the same drivers, placeholders
  and variables. Windows has no private display, so the window opens on the
  desktop; screenshots use `PrintWindow`, so covered windows still shoot.
  Paths reach the drivers with forward slashes (`C:/…`).
- **Off-screen:** the app runs on a private headless display
  (`scripts/offscreen.sh`, next section) and never shows on, or changes,
  the user's desktop. Without sway the harness stops with exit code 2; it
  never falls back to the user's screen. `SMOKE_VISIBLE=1` opens the window
  on the user's desktop instead (it takes a tile and focus; no
  screenshots); only use it when the user asks to watch.
- **macOS:** there is no private display, so the window opens on the
  desktop and takes focus (only with the user's OK). Shots are
  `screencapture` of the app window and need Screen Recording permission
  for the terminal; without it the run goes on without shots. The script
  keeps to macOS's bash 3.2.
- **What it can't do:** it can't drag pieces or press real keys (no input tools are
  installed), and chessground ignores synthetic mouse events. It calls the
  same functions those gestures call.

### Running windows off-screen (`scripts/offscreen.sh`)

The user works on the same machine while tests run, and a window that pops
up, takes focus or grabs a tile makes the computer unusable. So any window
you open for testing goes to a **private headless display**:

```sh
scripts/offscreen.sh COMMAND [ARGS...]
# e.g. an app plus a screenshot of it:
scripts/offscreen.sh sh -c 'some-app & sleep 5; grim shot.png; kill %1'
```

It starts a headless sway (`WLR_BACKENDS=headless`) with its own runtime
directory, runs the command on it and stops it afterwards. Inside the
command, `grim` screenshots the display and `swaymsg -t get_tree` gives
window geometry. Scripts can `source` it instead (`offscreen_start`,
`"${OFFSCREEN_RUN[@]}" cmd`, `offscreen_shot`, `offscreen_stop`), as
`scripts/smoke/run.sh` does. Needs `sway` (Arch: `pacman -S sway`); only
its headless mode is used, and it's never the user's session.

What keeps it isolated:
- **A separate compositor.** Sway on the headless backend opens no DRM
  device, no input devices and no logind session, so it can't take the
  screen, seat or keyboard from Hyprland. Its sockets live in a private
  directory under `$XDG_RUNTIME_DIR`, so no client finds it by accident.
- **No way back to the user's desktop.** The command runs with
  `WAYLAND_DISPLAY` set to the private socket (absolute path), `DISPLAY`
  and `HYPRLAND_INSTANCE_SIGNATURE` unset and `GDK_BACKEND=wayland`: GTK
  fails instead of falling back to the user's Xwayland, and `hyprctl`
  inside it can't reach Hyprland.
- **Its own D-Bus session**, which starts no services. On the user's bus,
  GTK single-instance apps (ghostty, for one) open their window through
  the user's running instance, and portals show dialogs on the user's
  screen. With service activation, the private bus would start a second
  xdg-desktop-portal, gvfs or keyring against the user's shared state
  (they try to mount `/run/user/<uid>/doc` and `/run/user/<uid>/gvfs`).

**Never change the user's Hyprland session to test something:** no
`hyprctl output create`, `hyprctl keyword` or `hyprctl dispatch`. Read-only
queries (`hyprctl -j clients`, `monitors`, `cursorpos`) are fine for checking.
The previous harness added a headless output to the user's Hyprland; that
moved their auto-placed monitor to x=22048, and Hyprland 0.54.3 left the
cursor off screen when the output was removed. Waybar and windows vanished
too (`HYPRLAND-DISRUPTION-REPORT.md`). If the cursor is ever gone again,
`hyprctl keyword monitor ",preferred,auto,1.25"` restores it.

Check it worked: read-only snapshots of `hyprctl -j monitors all`,
`hyprctl cursorpos`, `hyprctl -j activewindow` and `hyprctl -j clients`
stay identical before, during and after the run.

### Other helpers

- `crates/chess-db/examples/import_bench.rs`: imports a PGN and times open,
  paging, sorting and random game reads. Use it to catch performance
  regressions.
- `window.__chessdb` in `npm run tauri dev`: handy from the webview
  inspector (right-click → Inspect Element in dev builds).

## Conventions

### General
- Refer to features by their catalog IDs (for example "SRCH-04 position
  index") in commits, comments and docs.
- Keep `docs/` current: new decisions go in `docs/stack.md`, format changes
  in `docs/file-format.md`, and progress in `handoff.md`.
- Only add GPL-3-compatible dependencies. Check the license first, and also
  check whether the package is deprecated (the old `chessground` npm package
  was; use `@lichess-org/chessground`).
- Don't commit or push unless the user asks.

### Rust
- Edition 2024, MSRV 1.89. `rustfmt.toml` sets `max_width = 110` and
  `use_small_heuristics = "Max"`. Clippy must pass with `-D warnings`.
- **Crates in `crates/` must never depend on Tauri** or any UI code. This
  keeps the core headless-testable and leaves room for a future CLI or API
  (see "Deferred" in `docs/stack.md`).
- Errors: one `thiserror` enum per crate (`chess_core::Error`,
  `chess_db::DbError`, `chess_jobs::JobError`), converted to `AppError` at
  the Tauri boundary. No `unwrap()` outside tests; use `expect("why")` only
  for real invariants (lock poisoning, "non-root node has a move").
- Tests live next to the code (`#[cfg(test)]`) plus integration tests in
  `crates/*/tests/`. Round trips (PGN, blob codec) get `proptest`
  properties. New PGN edge cases become fixtures in `testdata/pgn/`.
- Long work (import, export, sort, later engine analysis) runs as a
  `chess-jobs` job and takes a `&dyn chess_db::Progress`, so it can
  report progress and be cancelled.
- Hot paths are optimized even in dev builds (`[profile.dev.package.*]` in
  the root `Cargo.toml`). Keep it that way, or `tauri dev` imports get ~5×
  slower.

### Storage format (`crates/chess-db`)
- Any change to the on-disk layout must bump the format version, update
  `docs/file-format.md`, and keep crash safety: write data, fsync, then
  publish (the committed count is written last).
- A move byte is the move's index in the legal moves **sorted by
  (from, to, promotion)** as defined in `blob.rs`, never shakmaty's
  generation order.
- Import must stay lossless: `cargo test --release -p chess-db -- --ignored`
  re-imports an export and compares blob bytes.

### Tauri boundary (`app/src-tauri`)
- Commands are `async fn … -> AppResult<T>`. Sync commands run on the
  main thread and freeze the UI.
- Rust owns all state; the frontend only renders views and sends intents.
  Large data never crosses IPC in bulk (the game list is paged).
- DTOs: `#[derive(Serialize, TS)] #[serde(rename_all = "camelCase")]
  #[ts(export)]`, with `#[ts(type = "number")]` on `u64`. Regenerate and
  commit `app/src/lib/bindings/` after changing them.
- Register every new command in `generate_handler!` in `src/lib.rs` and add
  a typed wrapper in `app/src/lib/api.ts`. Components never call `invoke`
  directly.
- Background job updates are emitted as the `job` event (`JobDto`).

### Frontend (`app/src`)
- Svelte 5 runes only (`$state`, `$derived`, `$effect`, `$props`). Shared
  state lives in classes in `lib/stores/*.svelte.ts`.
- Use the design tokens in `app.css` (`--paper`, `--ink`, `--felt`,
  `--brass`, `--board-*`, `--font-book`, `--font-ui`). Don't hardcode
  colors. Every color needs a dark-mode value.
- UI copy: sentence case, plain verbs that say what happens ("Save
  changes", "Import PGN"), no ALL-CAPS labels, errors that say what went
  wrong and what to do.
- Accessibility floor: visible `:focus-visible`, aria labels on icon
  buttons, keyboard paths for list and board navigation.
- Keyboard shortcuts use Ctrl **or** Cmd (`e.ctrlKey || e.metaKey`).

### Cross-platform rules (from `docs/stack.md`)
- Use `Path`/`PathBuf`, never paths built from strings. No hardcoded
  `/tmp` or `~`.
- Write whole files as temp file + rename. Close files before renaming or
  deleting them (Windows).
- Positional I/O goes through `crates/chess-db/src/io.rs`. Locking uses
  `File::try_lock`.
- Formats are explicitly little-endian. PGN in accepts CRLF; PGN out writes
  `\n`.
- Text files are LF everywhere: `.gitattributes` overrides Git for Windows'
  `core.autocrlf=true`, so WSL and Windows git see the same files and diffs
  don't turn every line over. Fixtures in `testdata/` are kept byte for byte.

## Gotchas (each one cost time)

- **chessground puts `cg-wrap` and `orientation-*` on the element you pass
  it**, not on a child. The board theme lives in `app/src/board-theme.css`
  (shared by the main board and the setup editor) as
  `.chessboard.cg-wrap.orientation-white …`; give a new board the
  `chessboard` class.
- **Game ids change on compaction.** Anything that stores `(handle, game)`
  must be remapped (`AppState::remap_sources`) or dropped (filters, sorts,
  the position index).
- **Svelte keeps whitespace between template nodes.** `NotationLine.svelte`
  deliberately has no whitespace between nodes and uses explicit `{" "}`,
  so parentheses hug their moves. Keep it that way when editing.
- **An `auto` grid column can't measure a board sized by `height: %` +
  `aspect-ratio`.** The column only fit the nav bar, and the board spilled
  over the notation at full screen. `fitBoard()` in `App.svelte` now sizes the
  board in pixels from the workspace; keep its constants in step with the CSS.
- **Browsers cap element height** (~17–33M px). `GameList.svelte` scales the
  scrollbar above 8M px and handles wheel scrolling itself. Test big lists
  with the 1M-game database.
- shakmaty 0.30 moved `zobrist_hash` onto the `Position` trait. `pgn-reader`
  0.29 uses a `Visitor` with `Tags`/`Movetext`/`Output` associated types
  and `ControlFlow`.
- `svelte-check` doesn't support TypeScript 7 yet. Stay on TS 6.
- TWIC rejects `curl` without browser-like headers (HTTP 406). The fetch
  script sends them.
- Editing files under `app/src` or the crates while `tauri dev` runs
  triggers a hot reload or rebuild. If a smoke run is in progress, that
  reload re-runs the driver.
- **Any write under `app/src-tauri/` or `crates/*` restarts the app under
  `tauri dev`**, and it looks like a crash. It happened when a database
  lived in `src-tauri/`, which is `tauri dev`'s working directory and where
  file dialogs used to start (they start in Documents now).
  `app/src-tauri/.taurignore` ignores `*.chessdb/`. The watcher filters
  events only through `.taurignore`, not `.gitignore`. It also only watches
  folders that existed when `tauri dev` started, and nothing under the
  git-ignored `target/`, so reproduce with a database created *before*
  launching and outside `target/`.
