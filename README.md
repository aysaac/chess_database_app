# Chess Database

A desktop chess database and analysis app, built as an open alternative to
ChessBase and Fritz. Rust core, Tauri 2 shell, Svelte 5 UI.

This is an early prototype: native database storage, fast PGN import and
export, a game list that scrolls through millions of games, a board with
notation for replaying and editing games, Stockfish analysis and blunder
check, an opening tree and position search, header search and filters.
What's planned beyond it is in [docs/feature-catalog.md](docs/feature-catalog.md);
the status of each feature is in the [Feature Status doc](https://claude.ai/artifact/SfWucTSjakLh9oNSWSNQrV#93fdfc30-cba2).

## What you can do

- Create or open a database (a `.chessdb` folder) and import PGN files. Imports
  run in the background with progress and a Cancel button, and the list grows
  while they run.
- Sort the game list by any column (click the header; click again to reverse).
  Scroll with the mouse or the arrow, Page Up/Down and Home/End keys. Enter
  opens the selected game; Delete marks it as deleted (or restores it).
- Replay a game with ←/→, Home/End or by clicking moves in the notation.
- Edit games:
  - Play moves on the board. If another move is already recorded there, the
    app asks whether to add a variation, make it the main line, or overwrite.
  - Right-click a move to add comments or move symbols (`!`, `?`, `±`, …), or
    to promote or delete a variation.
  - Right-click the board to draw arrows and circles. Shift, Alt and Alt+Shift
    change the colour.
  - Edit the players, event and other tags with **Edit details**.
  - Undo and redo with Ctrl+Z and Ctrl+Shift+Z.
- Save changes with Ctrl+S, or save a copy with Ctrl+Shift+S. Export the
  database (or the filtered games) to PGN, or copy the current game as PGN
  or the position as FEN.
- Analyze with Stockfish (Ctrl+Shift+A; run `scripts/fetch-stockfish.sh`
  once), run a blunder check, browse the opening tree, filter and search the
  list, open several games in tabs, and set up positions.

## Development

Requirements: Rust ≥ 1.89, Node ≥ 20, and the
[Tauri Linux prerequisites](https://tauri.app/start/prerequisites/)
(`webkit2gtk-4.1`, `libsoup3`, …).

```sh
cd app
npm install
npm run tauri dev        # run the app with hot reload
npm run tauri build      # release bundle
```

Checks:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace   # also regenerates app/src/lib/bindings/
cd app && npm run check  # svelte-check
```

Benchmarks and real-world data (downloads TWIC and 1M Lichess games into the
git-ignored `testdata/large/`):

```sh
scripts/fetch-testdata.sh
cargo run --release -p chess-db --example import_bench -- testdata/large/lichess_1m.pgn
cargo test --release -p chess-db -- --ignored   # lossless import → export → import
```

End-to-end smoke test of the real app (drives the UI through a script and
takes screenshots, on a private headless display that leaves your desktop
alone; needs `sway` and `grim`):

```sh
scripts/smoke/run.sh
```

Working on the code? Read [AGENTS.md](AGENTS.md) for conventions and the
[Feature Status doc](https://claude.ai/artifact/SfWucTSjakLh9oNSWSNQrV#93fdfc30-cba2) for where the project stands.

## Layout

```
crates/chess-core   rules glue, notation, game tree + editor, PGN reader/writer
crates/chess-jobs   background jobs (progress, cancellation)
crates/chess-db     storage format, import/export, sorted views, search, position index
crates/chess-engine UCI engine host, analysis, blunder check
app/src-tauri       Tauri commands and events over the crates
app/src             Svelte frontend
docs/               feature catalog, stack decisions, file format
scripts/            test-data and Stockfish download, smoke test harness
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
