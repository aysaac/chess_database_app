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
| Threat display (ENG-07) | **A second, small process of the analysis engine**: one thread, 16 MB hash, one line, the user's other options. It searches the position after a null move (`chess_engine::threat`) up to depth 20 or 4 s, then idles until the board moves, and quits when "Show threat" or the analysis goes off, or the main engine fails (`ThreatSearch` in `app/src-tauri/src/engines.rs`). It starts in the background once the main analysis searches, and nothing waits for it (a slow engine, Lc0 loading its network, holds up neither the analysis nor the page); one that finishes starting after "Show threat" was asked again (a double click) quits, as the analysis counts the requests. Its events are numbered and a page keeps the newest, so an update on its way when the board moved can't replace what came after it | Sharing the main engine would mean stopping its infinite search for every threat search and restarting it, so its depth would fall back each time and its lines flicker. A second process leaves the main analysis alone; capped at one thread and a few seconds, it takes little CPU. It isn't a second analysis (ENG-04): it follows the main one's engine and session |
| Frontend preferences across windows (DB-07) | **A copy per window, kept in step by an event**: `set_ui_pref` numbers each change (a revision, not saved) and emits it as `ui-pref` to every window; each window loads the prefs before it mounts (`main.ts`, two seconds at most, then it reads again until a read succeeds) and keeps the newest revision per key (`app/src/lib/prefsLedger.ts`). It sends one save per key at a time, and saves what's still in its debounce when it closes (`windowClose.ts`). Per-window keys (the dock layout, the Reference tab's dividers, the board orientation, the tree's source) are saved but not pushed to other open windows (`PER_WINDOW` in `prefs.svelte.ts`) | The library and every editor window share one Rust process but each has its own page. Without this a window that hadn't loaded the prefs, or held an old copy, saved its stale value over a newer one (a star lost). The event and the reply to a save can arrive in either order, hence the revisions; two saves of one key on their way at once could also be saved in either order. A Tauri window's page gets no event in time to send anything when the window closes. Rearranging one editor shouldn't rearrange the others; the next window opened starts from the last arrangement saved |
| Duplicate games (DB-08) | **Buckets first, then compare**, in `crates/chess-db/src/duplicates.rs`: the index records alone sort the games into buckets (exact: the same header fields; fuzzy: both players' surnames), only games that share one have their moves read (as std `DefaultHasher` hashes, about 80 bytes a game, with a hash of the annotations and commands), and within a fuzzy bucket the main lines are sorted, so the games a line is the start of follow it. Games are taken longest first; fuzzy matches don't chain (every copy matches the kept game itself), and a game that matches two games that aren't copies of each other stays. Delete and Restore are chunked jobs; each search has a number the actions send back, and a report holds only while `Database::generation()` is unchanged. **Fill before delete:** the kept game first takes the index-record fields it lacks from its copies (a new blob with only the Date, Round and ECO strings changed, appended like an edit, and its record), committed before any copy is marked as deleted; Restore points it back at its earlier record, kept in memory. A group whose kept game is open in an editor tab is left, checked under the database's lock, which opening a game holds until its session exists; sessions belong to their window and go when it closes or reloads. No format change, no new dependency | No pairwise comparison of a million games: 1M Lichess games take 0.2 s (each online game has its own Site URL, which rules a bucket out before any moves are read), a 1M file imported twice 5 s, 100,000 engine games of one pair that share their opening 0.5 s (the first version compared every pair in a bucket: 24 s). Deleting a game that wasn't a copy loses it, so the rules lean to leaving games alone (PR #16's review on Mega Database 2019). Removal is the existing soft delete, so it is undone the same way and compaction stays a separate step. Leaving both copies when the kept one lacked a rating or board (review round 3) defeated the point (an annotated copy never replaced TWIC's plain one); deleting it lost what searches and the Reference tab use. The format can't commit several records atomically, so the fill and the delete are two commits, ordered so that a crash leaves duplicates, never a lost field (as DB-06's move does). A tab holds the game as it was read, and saving it rewrites the record: filling in a game open in a tab lost the fill at the next save (review round 5), and reloading every tab under its user's edits isn't safe either |
| Online games (NET-01) | **`crates/chess-net`: blocking HTTPS with `ureq` 3** (rustls with ring, gzip, and the OS's certificates through `rustls-platform-verifier`). Lichess: `/api/games/user/{name}` as NDJSON, each line a game with its PGN (`pgnInJson`, oldest first, clock times, evals and the opening's name; `perfType` lists the speeds, which leaves out the variants), asked again from the last game when the stream breaks off. Chess.com: the player's monthly archives as JSON, kept to standard chess (`rules`) of the chosen `time_class`, rated or not, from a date (`end_time`); usernames in lowercase. A thread of its own fetches the games and passes them over a bounded channel to a `Read` that feeds `chess_db::import_reader`, so they're committed in batches as they come. That reader waits with a timeout, so Cancel, or a server that sends no byte for 5 minutes, ends the import at once, between two games (a fetcher thread stuck in a read stays until its connection drops: ureq has no timeout between reads). Games are told apart by their page (`chess_net::keys`: the Lichess game id, the Chess.com live or daily number, from `Site`, `Link` or `GameId`). The database's own are read from the blobs' tags first (`chess_db::scan_tags`), so a download skips them, and each download reports how far it got (`covered`: when the newest game the server sent started or ended, by the server's clock, so a game Chess.com's archive doesn't have yet comes next time, and a Lichess game still being played does too if it started after the newest one sent). On a cancel the importer isn't cancelled itself: the feed ends between two games and the reader takes in the games it read ahead, so every game sent is in the database (review round 2). The app keeps that per database with its list view (`settings::DownloadMark`: site, player, time controls, rated only, reached), and "new ones only" starts at the furthest mark whose games take in the ones asked for; with none, all of them (those there skipped). One download at a time in the app, one request at a time; a 429 waits (a full minute on Lichess) and asks again, three times at most, and a lookup the user waits for gives up at once and after 30 seconds. A start date is the day's midnight where the user is. The Tauri side keeps one client, so connections are reused. No format change | No async runtime in a core crate, and Tauri's own reqwest is built without TLS. The platform verifier trusts what the OS trusts (a company proxy's certificate, say). NDJSON gives each game's id and start time without parsing the PGN. Skipping games by their page works when a database is copied, merged or filled from two accounts, and finishes a download that stopped partway. "New ones only" first started at the newest game in the database that the player played, but that took in games where they were the opponent of a player downloaded before, and games of other time controls, and left out theirs (review round 1); a mark per set of games doesn't. Keeping the marks in the settings rather than the database folder needs no format change; a database moved or made again starts over. Lichess sends about 20 games a second to a client that isn't logged in, so 10,000 games take about 8 minutes; a token (60 a second for one's own games) isn't asked for yet. Licences: MIT/Apache-2.0 (ureq, rustls-platform-verifier), Apache-2.0/ISC/MIT (rustls), Apache-2.0 AND ISC (ring), CDLA-Permissive-2.0 (webpki-roots, compiled in by ureq's rustls feature but unused), all GPL-3 compatible |
| Frontend unit tests | **`node --test`** on `*.test.ts` next to pure modules (`cd app && npm test`), Node stripping the types itself | No test framework to add or keep up for a few pure helpers; the UI itself is covered by the smoke drivers. Modules under test import nothing from Svelte or Tauri |
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
| Online games | `ureq` 3.4 (rustls 0.23 with ring, rustls-platform-verifier 0.7) |

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
  chess-net/   a player's games from Lichess and Chess.com         (no Tauri)
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
