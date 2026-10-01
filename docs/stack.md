# Tech Stack

Status: **decided 2026-09-25** (frontend, license and platform policy added the same day, before the first prototype). Covers open questions #1 and #5 in [feature-catalog.md](feature-catalog.md#6-open-questions-for-the-build-spec).

## Decisions

| Area | Choice | Why |
|---|---|---|
| Back end / core logic | **Rust** | Speed for PGN import, the position index, search over millions of games, and engine process management |
| Desktop shell / UI | **Tauri 2** | Uses the OS webview instead of bundling Chromium like Electron does: small binaries (~10 MB), low RAM, Rust back end built in |
| Frontend | **Svelte 5 + TypeScript** on plain Vite (no SvelteKit) | Lightest runtime; fine-grained reactivity suits streaming updates; a desktop app needs no router |
| License | **GPL-3.0-or-later** | Lets us build on the best chess libraries: `shakmaty`, `pgn-reader` and `@lichess-org/chessground` (all GPL-3) |
| Platforms | **Linux first**, following the [cross-platform rules](#cross-platform-rules) | One codebase. Windows/macOS CI gets added once the repo is on GitHub |
| Programmatic access (CLI, local API, remote control of the running app, MCP) | **Deferred** | Not needed for now; see [Deferred](#deferred-programmatic-access) |
| Board images (QOL-06) | **Drawn in Rust** (`crates/chess-diagram`): an SVG built by hand, rasterized with `resvg`; put on the clipboard by `tauri-plugin-clipboard-manager` (arboard) | The same image on every platform and in unit tests, without a webview. The SVG is only paths: the cburnett pieces from chessground and coordinate labels outlined (with `ttf-parser`) from an embedded IBM Plex Sans Condensed, so it needs no fonts wherever it's pasted. WebKitGTK's async clipboard isn't dependable for images; arboard writes `image/png` on Wayland (data-control), X11, Windows and macOS. The frontend has no clipboard permissions: only Rust writes to it |
| Clipboard text (IO-08, UI-18) | **Read and written in Rust** through the same plugin: `clipboard_text`, `copy_text` | A page only reads the clipboard in a paste event (none fires in WebKit where nothing is editable), and WebKit only lets it write during a user gesture, which ends at the first `await` (Copy game fetches the PGN first). So the ribbon's Paste game/Paste position read through Rust, and Copy game/Copy position write through it; Ctrl+V still uses the paste event (`FenPaste.svelte`) |
| Ribbon icons (UI-18) | **Lucide**, vendored: only the icons used, as inline SVG markup in `app/src/lib/icons/lucide.ts` (licence in `COPYING` there) | ISC (some icons MIT, from Feather), GPL-compatible, maintained (lucide-static 1.49, not deprecated). Strokes use `currentColor`, so they follow the theme. No npm dependency for 28 icons |

## Versions in use

| Piece | Version |
|---|---|
| Rust | 1.94 (edition 2024, MSRV 1.89 for `File::try_lock`) |
| Tauri | 2.11 (3.0 is alpha only) |
| shakmaty / pgn-reader | 0.30 / 0.29 |
| Svelte / Vite / TypeScript | 5.57 / 8.3 / 6.0 (svelte-check doesn't support TS 7 yet) |
| Board | `@lichess-org/chessground` 10.2 (the old `chessground` npm package is deprecated) |
| Rust → TypeScript types | `ts-rs` 12, generated into `app/src/lib/bindings/` by `npm run bindings` |
| Board images | `resvg` 0.48 (no default features: no text, fonts or raster images), `ttf-parser` 0.25, `tauri-plugin-clipboard-manager` 2.3 |
| Icons | Lucide (lucide-static 1.49.0), vendored, not a dependency |

## How Tauri splits the work

```
┌───────────────────────── Tauri app ──────────────────────────┐
│  Frontend (webview: HTML/CSS/TS)                             │
│   board · notation pane · game list · panels                 │
│        │  invoke("command", args)        ▲ events (stream)   │
│        ▼                                  │                  │
│  Rust side                                                   │
│   #[tauri::command] handlers  ──►  core crates               │
│                                    (rules, db, search,       │
│                                     engine, analysis)        │
└──────────────────────────────────────────────────────────────┘
```

- **Rust owns all state and logic**: databases, search, engines, analysis. The frontend only renders and sends user intents.
- **Commands** (`invoke`) cover request/response calls such as open DB, fetch game, run search, and fetch a page of the game list.
- **Events** cover streaming data: engine info lines, import/search progress, batch-analysis progress.
- **Large data stays in Rust.** The game list (DB-03, 10M+ rows) is virtualized, and the frontend asks only for the visible row window. Full result sets never go over IPC.

## Guiding rule: keep the core UI-agnostic

Programmatic access is deferred, but the core crates must not depend on Tauri. That keeps the door open for a CLI or local API later at almost no cost now, and makes the core testable headless (Phase 0 of the catalog).

Cargo workspace:

```
crates/
  chess-core/  rules glue, notation, game tree, editor, PGN I/O   (no Tauri)
  chess-jobs/  background jobs with progress and cancellation     (no Tauri)
  chess-db/    storage format, import/export, sorting, search,
               position index, compaction                         (no Tauri)
  chess-engine/ UCI host, analysis driver, blunder check           (no Tauri)
  chess-diagram/ board images: SVG and PNG of a position           (no Tauri)
app/
  src-tauri/   Tauri commands/events wrapping the crates above
  src/         Svelte frontend
```

## Known trade-offs of Tauri

- **Linux webview = WebKitGTK.** It's slower and quirkier than Chromium (rendering performance, some CSS/JS gaps). Test the heavy views (notation pane, game list) on Linux early.
- **Different engine per OS**: WebView2 (Chromium) on Windows, WKWebView on macOS, WebKitGTK on Linux. The UI must be tested on each target.
- **IPC cost**: commands serialize through JSON by default. Fine for UI traffic; keep bulk data paged, as described above.
- **Engines run as child processes** spawned by the Rust side (never from the webview). Stockfish ships as a separate binary: `scripts/fetch-stockfish.sh` downloads the official release for a target (checksum pinned) to `app/src-tauri/binaries/stockfish-<triple>`, and `npm run tauri build -- --config src-tauri/tauri.stockfish.json` bundles it as a sidecar next to the app binary, where the app looks for it first. The binary is never committed.

## Cross-platform rules

We develop and test on Linux; these rules keep Windows and macOS builds a
matter of running the build, not porting:

- Use `Path`/`PathBuf` only, never paths built from strings. Use Tauri's path API for app directories. No hardcoded `/tmp` or `~`.
- Write whole files as temp file + rename. Close (and later unmap) files before renaming or deleting them: Windows refuses otherwise.
- Lock files with `std::fs::File::try_lock`. Do positional I/O through `crates/chess-db/src/io.rs`, which picks `read_at`/`seek_read` per OS.
- File formats use explicit little-endian encoding. PGN input accepts CRLF; PGN output uses `\n`.
- Define shortcuts as Ctrl-or-Cmd. Use only standard CSS (no WebKit-only features).
- Drags inside the app (games out of the list, QOL-04) use pointer events and `elementFromPoint` hit testing on `data-drop` targets, not HTML5 drag and drop. WebView2 breaks HTML5 drag and drop while Tauri's OS file-drop handler (`dragDropEnabled`) is on, and that handler is what dropping PGN files on the window (IO-09) will need. Pointer events also behave the same in WebKitGTK, WKWebView and WebView2, and smoke drivers can dispatch them.
- No Linux-only crates in the `crates/` dependency tree.

## Storage

Decided while building DB-01: custom files, no SQLite (yet). See
[file-format.md](file-format.md): fixed 64-byte index records, append-only
game blobs, normalized name tables, crash-safe commits. There is no
memory-mapping in the prototype; positional reads were fast enough (see the
benchmarks there).

## App folders and onboarding (QOL-08)

- **Settings** live in the app's config folder (`settings.json`), **app
  data** in its data folder (Tauri's `app_config_dir` / `app_data_dir`).
  `CHESSDB_CONFIG_DIR` points both at one folder, so test runs touch
  neither of the user's.
- **Sample games** are 12 famous games (1851 to 1999, plus the Saavedra
  study) in `app/src-tauri/data/sample-games.pgn`, compiled into the
  binary with `include_str!`. Game scores are facts; the notes in them are
  our own, under the project's GPL. The first "Explore the sample games"
  builds `Sample games.chessdb` in the data folder (under a temporary name,
  renamed once complete) and later ones open it. It isn't put in
  Documents, so a first look leaves nothing there.
- **The welcome guide** opens when `welcomeDone` isn't set in the settings
  and no database was ever opened (so people who used the app before the
  guide existed don't get it). Closing or finishing it sets `welcomeDone`.
  Its import wizard suggests `<Documents>/<first PGN's name>.chessdb`
  (" 2", " 3"… when taken), like the file dialogs.
- **No engine download.** The guide shows the engine that detection found
  (the bundled Stockfish, ENG-09) or explains how to add one; fetching
  Stockfish at run time (ENG-09's "auto-download") is still open.

## Deferred: programmatic access

Discussed and parked. Recorded here so the design isn't lost:

1. **Library**: other programs link the core crates (optionally Python bindings via PyO3).
2. **Headless CLI** (`chessdb import/search/analyze …`) that runs without the GUI.
3. **Remote control of the running app** over a local socket (like VS Code's `code` command or `nvim --remote`). The CLI forwards commands to the GUI when it's running.
4. **Single writer per database** (lock file), so the GUI and the CLI never write the same DB at once.
5. **MCP server** exposing the same API to AI agents.

The only thing this requires today is the rule above: core crates stay free of Tauri dependencies.
