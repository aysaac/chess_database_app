# AGENTS.md

Guidance for AI agents (and humans) working in this repo. Read this first.

**Where the project stands** (what's built, partly built or planned, and
the known gaps) is tracked in the
[Feature Status doc](https://claude.ai/artifact/SfWucTSjakLh9oNSWSNQrV#93fdfc30-cba2).
Read it before picking up work, and update it when a feature's status
changes. It's a Claude Doc: open it with the Claude Docs tools (`read` the
link), not a web fetch. Without those tools, ask the user.

**The project:** a desktop chess database and analysis app, an open
alternative to ChessBase and Fritz. Rust core, Tauri 2 shell, Svelte 5 +
TypeScript UI. GPL-3.0-or-later. Linux first; the code is kept portable to
Windows and macOS.

This file holds what applies everywhere. The rest lives next to what it's
about; read it when your task gets there:

| Before you… | Read |
|---|---|
| Change anything under `app/` (Svelte UI, Tauri shell) | [`app/AGENTS.md`](app/AGENTS.md): its conventions and gotchas |
| Change `crates/chess-db`: the file format, import/export, `.cbh`, benchmarks | [`crates/chess-db/AGENTS.md`](crates/chess-db/AGENTS.md), then [`docs/file-format.md`](docs/file-format.md) or [`docs/io-03-cbh.md`](docs/io-03-cbh.md) |
| Run or write a smoke test, or open any window | [`scripts/smoke/README.md`](scripts/smoke/README.md) |
| Work on the ribbon, panel tabs, docking or the Reference tab | [`docs/workspace-ui.md`](docs/workspace-ui.md): the spec for UI-18/19, UI-04 and OPEN-14, with the build order |
| Work on the Windows PC | [`docs/windows-testing.md`](docs/windows-testing.md): code fixes and checks, with how to verify each |
| Pick or scope a feature | [`docs/feature-catalog.md`](docs/feature-catalog.md): every ChessBase/Fritz feature with IDs (`DB-01`, `ENG-02`…), tiers P0–P5 and dependencies. **Features are referred to by these IDs**. Their status is in the Feature Status doc (above) |
| Add a dependency or make a stack decision | [`docs/stack.md`](docs/stack.md) |

## Where things are

| Path | What |
|---|---|
| `crates/chess-core` | Chess domain: rules glue over `shakmaty`, notation, game tree (`game/mod.rs`), editing session with undo (`game/editor.rs`), PGN reader and writer (`pgn/`) |
| `crates/chess-jobs` | Background jobs: threads, throttled progress, cooperative cancellation |
| `crates/chess-db` | Native database: file format, crash-safe commits, import/export, sorted views, header search, position index (`positions.rs`), the Reference tab's queries (`reference.rs`), compaction, ChessBase `.cbh` import (`cb/`, IO-03) |
| `crates/chess-engine` | UCI engine host, analysis driver thread, blunder check (`review.rs`); `src/bin/fake_uci.rs` is a toy engine for tests |
| `crates/chess-diagram` | Board images (QOL-06): a position as SVG or PNG, drawn like the app's board (resvg); vendored pieces and font in `data/` (licences in `data/COPYING`) |
| `crates/chess-net` | Online games (NET-01): a player's games from Lichess and Chess.com into a database (`download_into`), skipping the ones it has (`keys.rs`); blocking HTTPS (ureq). `tests/download.rs` runs it against a local stand-in server |
| `app/src-tauri` | Tauri shell: commands (`src/commands/`), DTOs (`src/dto.rs`), view building (`src/view.rs`), state (`src/state.rs`), engines and analysis (`src/engines.rs`), settings (`src/settings.rs`) |
| `app/src` | Svelte UI: `lib/api.ts` (typed commands), `lib/stores/` (state), `lib/components/` (the ribbon is `Ribbon.svelte`, the docked panels around the board `Dock.svelte`), `lib/bindings/` (generated types), `lib/icons/` (vendored Lucide icons; licence in `COPYING`) |
| `testdata/pgn/` | Small edge-case PGN fixtures (committed) |
| `testdata/large/` | Big real-world PGNs (git-ignored; see `scripts/fetch-testdata.sh`) |
| `scripts/smoke/` | End-to-end smoke test harness for the running app |

## Commands

Run from the repo root unless noted. Benchmarks, the lossless round trip
and ChessBase imports are in [`crates/chess-db/AGENTS.md`](crates/chess-db/AGENTS.md).

```sh
# Run the app (hot reload for the UI; Rust changes trigger a rebuild)
cd app && npm install && npm run tauri dev

# All checks: run these before saying you're done
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # also regenerates app/src/lib/bindings/
cd app && npm run check           # svelte-check (TypeScript + Svelte)
cd app && npm run build           # production frontend build
cd app && npm test                # frontend unit tests (node --test; Node 22.18+)

# Regenerate the TypeScript types after changing a DTO in app/src-tauri/src/dto.rs
cd app && npm run bindings

# Stockfish 19 for this machine → app/src-tauri/binaries/ (git-ignored, ~100 MB)
scripts/fetch-stockfish.sh

# Real-world data: TWIC issue + first 1M Lichess games → testdata/large/ (~2.2 GB)
scripts/fetch-testdata.sh

# End-to-end smoke test of the real app, on a private headless display (needs sway)
scripts/smoke/run.sh [scripts/smoke/DRIVER.js]

# Any other window you need to open: run it off-screen, never on the user's desktop
scripts/offscreen.sh COMMAND [ARGS...]

# A FEN as a board image (QOL-06), with a sample arrow, to check diagrams without the app
cargo run -p chess-diagram --example render -- out.png [FEN] [SIZE] [black]   # or out.svg

# A player's online games into a database (NET-01), as the app does: real servers
cargo run -p chess-net --example download -- lichess|chess.com USERNAME out.chessdb [YYYY-MM-DD]
```

## The user's machine

The user works on this machine while you test.

- **Open every window on a private headless display:** a window that pops
  up, takes focus or grabs a tile makes the computer unusable. The app goes
  through `scripts/smoke/run.sh`, anything else through
  `scripts/offscreen.sh`. `SMOKE_VISIBLE=1` puts the app on the user's
  desktop; only use it when they ask to watch.
- **Their own database is `app/src-tauri/test_/My games.chessdb`**
  (git-ignored user data): never delete or move it.
- **They may be running `npm run tauri dev` themselves** (port 1420; the
  smoke harness uses 1430). Don't kill processes you didn't start.
- **Never change the user's Hyprland session to test something:** no
  `hyprctl output create`, `hyprctl keyword` or `hyprctl dispatch`.
  Read-only queries (`hyprctl -j clients`, `monitors`, `cursorpos`) are
  fine for checking. If the cursor is ever gone,
  `hyprctl keyword monitor ",preferred,auto,1.25"` restores it.

Why, how the display is kept apart and how to check it worked:
[`scripts/smoke/README.md`](scripts/smoke/README.md#the-private-display).

## Conventions

### General
- Refer to features by their catalog IDs (for example "SRCH-04 position
  index") in commits, comments and docs.
- Keep `docs/` current: new decisions go in `docs/stack.md`, format changes
  in `docs/file-format.md`, and progress in the Feature Status doc.
- Only add GPL-3-compatible dependencies. Check the license first, and also
  check whether the package is deprecated (the old `chessground` npm package
  was; use `@lichess-org/chessground`).
- **Git:** one commit per finished feature or fix, with its catalog ID in
  the subject.
  - **On a branch other than `main`:** commit and push to `origin` as you
    go (`git push -u origin <branch>`), without asking. When the work is
    done and the checks pass, open a PR against `main` with
    `gh pr create`: what changed, the checks you ran and what wasn't
    verified. The user merges it.
  - **On `main`:** commit or push only when the user asks.
  - Never force-push without asking.
- Area guidance goes in the area's file (`app/AGENTS.md`,
  `crates/chess-db/AGENTS.md`, `scripts/smoke/README.md`), and a trap in a
  single file goes in a comment there. Only what applies everywhere goes
  here.

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
- shakmaty 0.30 moved `zobrist_hash` onto the `Position` trait. `pgn-reader`
  0.29 uses a `Visitor` with `Tags`/`Movetext`/`Output` associated types
  and `ControlFlow`.

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

## Gotchas that cross areas

The ones for one area are in its file (see the table at the top).

- **Game ids change on compaction.** Anything that stores `(handle, game)`
  must be remapped (`AppState::remap_sources`) or dropped (filters, sorts,
  the position index).
- **`tauri dev` reloads on edits.** Editing files under `app/src`
  hot-reloads the UI, and **any write under `app/src-tauri/` or `crates/*`
  rebuilds and restarts the app**, which looks like a crash. A smoke run in
  progress then re-runs its driver. It happened when a database lived in
  `src-tauri/`, which is `tauri dev`'s working directory and where file
  dialogs used to start (they start in Documents now).
  `app/src-tauri/.taurignore` ignores `*.chessdb/`. The watcher filters
  events only through `.taurignore`, not `.gitignore`. It also only watches
  folders that existed when `tauri dev` started, and nothing under the
  git-ignored `target/`, so reproduce with a database created *before*
  launching and outside `target/`.
