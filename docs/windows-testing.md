# Windows: what's left to test and fix

For the agent working on the user's Windows PC. Read [AGENTS.md](../AGENTS.md)
and [handoff.md](../handoff.md) first; this file is the Windows to-do list.

**Status (2026-09-28).** PR #2 is merged: on Windows 10 fmt, clippy, the
156 tests, svelte-check, the frontend build, all six smoke drivers (through
`scripts/smoke/run.ps1`) and `tauri build` with the Stockfish sidecar pass,
and the release exe starts. It fixed the position index flush
(`openings.idx.tmp` was synced through a read-only handle), the strip beside
the board at 120% scaling, a race in `basic.js`, and pinned LF line endings
with `.gitattributes`.

What's left is below: **part A** is code to change, **part B** is checks
that only a real Windows PC can do. Work through A1–A3 first, then B; A4
depends on what B3 finds.

## How to work

- **Start every item from a fresh `main`:**
  `git fetch origin && git switch -c windows/<topic> origin/main`.
  PRs #1 and #2 both started from an old `main`, fixed the same lints twice
  and conflicted.
- **One PR per code item** (A1, A2…). The checks in part B can share one
  PR that updates this file and the handoff.
- **Before opening a PR:** rebase onto `origin/main` and run the
  [baseline checks](#baseline-checks). The PR description gets what changed,
  a results table and what wasn't verified, like PRs #1 and #2.
- **Record progress here:** tick the boxes of the items the PR does, with a
  one-line result. Windows notes for the next agent go in `handoff.md` under
  `### Windows` in "Environment notes", not in the TL;DR list, so Mac and
  Windows PRs don't edit the same lines.
- **Keep Linux and macOS unchanged.** Every PR is reviewed on Linux with the
  smoke harness before it's merged. Use `#[cfg(windows)]` for code that
  only makes sense there.
- **Ask the user first** before anything destructive or outside the repo.
  Defender, OneDrive and display settings belong to the user: ask before
  changing them and put them back afterwards. Don't push to `main`.
- The user allows visible windows on this PC (the smoke window opens on
  the desktop; there's no private display on Windows).

## Baseline checks

Every PR runs these, from the repo root in PowerShell (from WSL, run each
through `cmd.exe /c`, as PR #2 did):

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd app; npm run check; npm run build; cd ..
# Each of: basic, layout, search, tree, setup, engine
powershell -ExecutionPolicy Bypass -File scripts\smoke\run.ps1 scripts\smoke\basic.js
```

- The 1M-game parts need `testdata\large\` (`scripts/fetch-testdata.sh`).
- `engine.js` needs Stockfish:
  `scripts/fetch-stockfish.sh x86_64-pc-windows-msvc` (a bash script;
  run it from WSL or Git Bash).
- Look at the screenshots in `target\smoke\<timestamp>\shots\`: a passing
  driver doesn't prove the layout is right.

There's no CI yet (the user hasn't decided), so these runs are the only
Windows check a change gets.

## Part A: code changes

### A1. `run.ps1`: don't leak the smoke settings; exit 2 on a busy port

- [ ] Done

**Problems** in `scripts/smoke/run.ps1`:
1. It sets `$env:CHESSDB_SMOKE` and `$env:CHESSDB_CONFIG_DIR` and never
   clears them. Run as `.\scripts\smoke\run.ps1` from an open PowerShell
   (instead of `powershell -File …`, which is a new process), they stay
   set: the next `npm run tauri dev` in that window runs the smoke driver
   and uses the smoke settings folder.
2. On a busy port, `Write-Error` under `$ErrorActionPreference = 'Stop'`
   throws before `exit 2`, so the script exits with 1. `run.sh` exits 2.

**Fix:** save both variables before setting them and restore (or remove)
them in the `finally` block. For the port, print the message without
throwing (`[Console]::Error.WriteLine(...)` or `Write-Host`) and then
`exit 2`.

**Verify:**
- From an open PowerShell: `.\scripts\smoke\run.ps1 scripts\smoke\layout.js`,
  then `Test-Path env:CHESSDB_SMOKE` and `Test-Path env:CHESSDB_CONFIG_DIR`
  both print `False`.
- Occupy port 1430 (for example `cd app; npm run dev -- --port 1430` in a
  second window), run the script, and check that `$LASTEXITCODE` is `2`.
- A normal run still passes.

### A2. Paths show as `\\?\C:\…`

- [x] Done (2026-09-28): `dunce` 1.0.5 (CC0/MIT-0/Apache-2.0, already in
  the lockfile through Tauri). New and old settings show `C:\…` only; an
  old file's duplicate Recent entry merges; a lower-case path reopens the
  same handle; the reference shares it. Checked through the API from a
  driver, not by clicking the dialogs.

**Problem.** `canonicalize()` returns extended-length paths on Windows
(`\\?\C:\Users\…`). The app canonicalizes database and engine paths in
five places:

- `app/src-tauri/src/commands/db.rs` (`open_for`, `set_reference_db`)
- `app/src-tauri/src/state.rs` (`add_db`)
- `app/src-tauri/src/engines.rs` (`detect`)
- `app/src-tauri/src/commands/engine.rs` (adding an engine)

So `settings.json`, the tooltips of recent databases and the Engines dialog
show `\\?\C:\…`. Opening still works. Names are fine (they come from
`file_stem()`).

**Fix:**
1. Use the `dunce` crate: `dunce::canonicalize` returns `C:\…` whenever
   that's a valid path, and is plain `canonicalize` elsewhere, so Linux
   and macOS don't change. Before adding it, check its licence (should be
   MIT or Apache-2.0, which is GPL-compatible) and that it's maintained, as
   AGENTS.md asks. It's only needed in `app/src-tauri/Cargo.toml`.
2. Replace the five calls. Keep the paths canonical: `find_db`
   (`state.rs`) compares them to make sure a folder is only opened once.
3. Settings written before the fix keep `\\?\` paths. In `Settings::load`
   (`settings.rs`), pass `recent_databases`, `reference_database` and each
   engine's `path` through `dunce::simplified`, then drop duplicate recent
   entries (keep the first). Otherwise Recent lists the same database
   twice.
4. Add a `#[cfg(windows)]` unit test in `settings.rs`: settings holding
   both `\\?\C:\x\a.chessdb` and `C:\x\a.chessdb` load as one entry
   `C:\x\a.chessdb`.

**Verify:**
- Open and create databases, set a reference database, add an engine by
  file. `%APPDATA%\dev.chessdb.desktop\settings.json`, the Recent menu
  tooltips, the "Recently opened" list and the Engines dialog show
  `C:\…`, never `\\?\`.
- Open a database from Recent, then open the same folder through Open
  database typed in another letter case (`c:\users\…`): it's one database
  (no second handle, no "already open" error). "Use <open> as reference"
  in the opening tree still shares the open database.
- Put an old `settings.json` with `\\?\` entries back in place: it loads,
  Recent has no duplicates, the reference and engines still work.

### A3. Refit the board when the display scale changes

- [ ] Done

**Problem.** `fitBoard()` in `app/src/App.svelte` snaps the board to
multiples of 8 device pixels (PR #2), but it only runs when the workspace
changes size (`ResizeObserver`). When the scale changes and the window
keeps its size in CSS pixels, nothing refits and the strip beside the
board comes back until the next resize. That happens when:
- the window moves to a monitor with a different scale;
- the user changes Settings > System > Display > Scale while the app runs.

**Fix:** also refit when `devicePixelRatio` changes. Listen to
``matchMedia(`(resolution: ${devicePixelRatio}dppx)`)``, call `fitBoard()`
on `change`, then listen again for the new ratio. Remove the listener in
the cleanup that `onMount` returns.

Leave Linux alone: its webview reports `devicePixelRatio` 2 at the user's
1.25 scale, so there the board is 0.25 CSS px wider than chessground's
board. That was checked in zoomed screenshots and isn't visible.

**Verify:**
- With `npm run tauri dev` running, change the Windows scale
  100% → 125% → 150% → 175% and back. If there are two monitors with
  different scales, drag the window between them. After each change, run
  this in the webview inspector's console (right-click → Inspect Element):
  ```js
  const b = document.querySelector(".board-square").getBoundingClientRect();
  const c = document.querySelector(".board-square cg-container").getBoundingClientRect();
  [devicePixelRatio, b.width, c.width, b.width - c.width];
  ```
  The last number must be below 1, and no strip shows beside the board.
- The baseline `layout.js` run passes at each scale.

### A4. WebView2 browser shortcuts (only if B3 finds a problem)

- [ ] Not needed / done

**Background.** WebView2 has its own browser shortcuts (find, reload,
print…) that Linux and macOS don't have. Tauri 2.11 has no setting for
them (only `zoomHotkeysEnabled`, which is off by default). The app's
shortcut handler (`onKey` in `App.svelte`) calls `preventDefault` for its
own keys, but ignores keys while a text field has focus.

**Fix, preferred:** turn off WebView2's `AreBrowserAcceleratorKeysEnabled`
from Rust. Reach the controller through the window's `with_webview`
(Windows: `controller()`), get `CoreWebView2().Settings()`, cast to
`ICoreWebView2Settings3` and call `SetAreBrowserAcceleratorKeysEnabled(false)`.
That needs `webview2-com` as a `[target.'cfg(windows)'.dependencies]`
entry, at the version Tauri already uses (see `Cargo.lock`), after a
licence check. Check whether it also removes the inspector's shortcut in
debug builds; if so, apply it in release builds only.

**Fallback:** `preventDefault` for F5, Ctrl+R, Ctrl+Shift+R and Ctrl+P in
`onKey`. It doesn't cover keys pressed while a text field has focus.

**Verify:** B3 again, and the app's own shortcuts from B2 still work.

## Part B: checks on the real PC

Record every result, including "works". Something that fails and isn't
obvious to fix becomes a new item in part A: describe it and ask the user
before a big change.

### B1. The installed app

- [ ] NSIS installer
- [ ] MSI installer

Build from `app/`:
`npm run tauri build -- --config src-tauri/tauri.stockfish.json`. The
installers land in `target\release\bundle\nsis\` and
`target\release\bundle\msi\`. They're unsigned, so SmartScreen warns
("More info" → "Run anyway"); note the wording.

For each installer:
- [ ] It installs; note where (NSIS installs per user, MSI per machine and
  needs admin). `stockfish.exe` is next to the app's exe.
- [ ] The Start menu entry and the icon are right, and the window title is
  "Chess Database".
- [ ] Engines… lists the bundled Stockfish from the install folder;
  Analyze shows lines and the eval bar; Blunder check… finishes.
- [ ] Create a database in Documents, import `testdata\pgn\*.pgn`, sort,
  open a game, edit, save. Close and restart the app: it reopens the last
  database (QOL-01).
- [ ] Uninstall. If the uninstaller offers to delete app data, leave that
  unticked. The databases and `%APPDATA%\dev.chessdb.desktop\` stay;
  nothing of the app is left in the install folder.

### B2. Real mouse and keyboard

- [ ] Done

The smoke drivers can't do any of this (chessground ignores synthetic
events). In the installed app or `npm run tauri dev`:
- [ ] Dragging pieces: a legal move, an illegal one (the piece goes back),
  a promotion by drag (the picker opens), dropping off the board.
- [ ] Clicking pieces: click-click moves, the next-move picker and its
  arrows.
- [ ] Right-click menus in the notation and the game list.
- [ ] Typing moves with the board focused: `e4`, `Nf3`, `exd5`, `0-0`,
  `e8=Q`, `g1f3`, and Enter to play.
- [ ] Shortcuts, each doing its job: Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z,
  Ctrl+S, Ctrl+Shift+S, Ctrl+F (flip), Ctrl+T, Ctrl+W, Ctrl+Tab,
  Ctrl+Shift+Tab, Ctrl+Up/Down (previous/next game), Ctrl+Shift+A
  (analysis), arrows, Home, End; F2 renames in the Players/Events lists;
  middle-click closes a tab; Ctrl+double-click opens a game in a new tab.
- [ ] Comments with characters typed through AltGr on the user's keyboard
  layout (for example `@`, `{`, `[`, `€`): they type normally and trigger
  no shortcut.
- [ ] Native dialogs: Open database, New database, Import PGN (the PGN
  filter), Export PGN, Add engine by file. They start in Documents and
  return to the app.
- [ ] The setup editor (Set up position…): palette, move and remove tools,
  square clicks, dragging pieces off the board.

### B3. WebView2 browser shortcuts

- [ ] Done

With the board focused (not a text field), then again with a comment being
edited, press each key and note what happens:

| Key | Expected |
|---|---|
| Ctrl+F | The board flips; no find bar, in a text field either |
| F5, Ctrl+R, Ctrl+Shift+R | Nothing; the page doesn't reload |
| Ctrl+P | No print dialog |
| Ctrl+plus, Ctrl+minus, Ctrl+0, Ctrl+wheel | No zoom |
| Alt+Left, Alt+Right | No navigation |
| Ctrl+Shift+I, F12 (installed release build) | No inspector |

Anything else goes to A4.

### B4. Unusual paths

- [ ] Done

For each case, create a database, import TWIC (`testdata\large\twic1600.pgn`),
sort, open a game, save an edit, run Compact…, close it and reopen it from
Recent:
- [ ] a folder with spaces and accents, for example
  `Documents\Partidas de ajedrez ñ`;
- [ ] a path longer than 260 characters (note whether the dialog or the app
  refuses it, and the message);
- [ ] another drive, and a network share, if the PC has them.

### B5. OneDrive

- [ ] Done / Documents isn't on OneDrive

File dialogs start in Documents, which OneDrive often syncs. Check with
`Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders' -Name Personal`.
If it's under OneDrive:
- [ ] Create a database there, import TWIC, save several edits, run
  Compact…, then close and reopen it.
- [ ] Watch for "Access is denied" or sharing violations, and for OneDrive
  conflict copies (file names with the PC's name) inside the `.chessdb`
  folder.
- [ ] With the app closed, wait for the sync to finish and reopen: the
  database opens with the same game count.

If renames fail here, note which operation. A retry on
`ERROR_ACCESS_DENIED` and `ERROR_SHARING_VIOLATION` is the usual fix, but
discuss it with the user before coding it, because it touches crash-safe
commits (see `docs/file-format.md`).

### B6. Display scaling and themes

- [ ] Done

- [ ] At 100%, 125%, 150% and 175%: `layout.js` passes, the screenshots
  show no strip beside the board, and text is sharp.
- [ ] Two monitors with different scales (if available): after moving the
  window, the board refits (after A3).
- [ ] Light and dark app mode (Settings > Personalization > Colors): the
  app follows the mode, and the title bar matches it. Light mode was never
  looked at on any platform, so take screenshots of both.

### B7. Performance

- [ ] Done

Release build, from the repo root:

```powershell
cargo run --release -p chess-db --example import_bench -- testdata/large/lichess_1m.pgn
cargo run --release -p chess-db --example import_bench -- testdata/large/twic1600.pgn
```

Compare with Linux (16-core laptop, release build; from `handoff.md`):

| | Linux |
|---|---|
| Import 1M Lichess games | 48.9 s (20k games/s), 310 MB peak RSS |
| Open | 478 ms warm |
| Sort by White | 117–430 ms |
| Read a game | ~65 µs warm |
| Header search (Elo ≥ 2500, draws) | 42–58 ms |
| Position index build | 12.3 s |

The 1M-game smoke parts print timings too. Linux dev build, 2026-09-28:
index ready 12 s after the import, opening tree 106 ms, position search
21 ms, quick filter 744 ms, header search 138 ms.

Note the CPU, core count and disk. If Windows is much slower, suspect
Defender scanning the files as they're written. Ask the user before
trying a folder exclusion, and remove it afterwards.

### B8. Windows 11

- [ ] Done / not available

If a Windows 11 PC is available: the baseline checks, B1 and B3.

### B9. The one-off test failure

- [ ] Done

In PR #2, one `cargo test` run on Windows had a single failure that
16 reruns didn't reproduce, and its name wasn't captured. Run the suite
20 times and keep the logs:

```powershell
for ($i = 1; $i -le 20; $i++) {
  cargo test --workspace *> "target\test-run-$i.log"
  if ($LASTEXITCODE -ne 0) { "run $i failed" }
}
Select-String -Path target\test-run-*.log -Pattern 'FAILED|panicked'
```

The first suspect is `analyzer_reports_engine_failure`
(`crates/chess-engine/tests/engine.rs`): it gives the analyzer 1 s to stop
after the fake engine crashes, and Windows tears processes down more slowly.
If it fails, give it more time rather than removing the check.

## Not on this list (decisions for the user)

- **Signing** the installers, which removes the SmartScreen warning.
- **WebView2 install mode.** The default installer downloads WebView2 when
  it's missing; an offline installer is about 127 MB bigger.
- **Which installer to ship** (NSIS per user, MSI per machine), and
  whether the app opens `.pgn` files from Explorer.
- **CI** on GitHub Actions for Linux, Windows and macOS.
