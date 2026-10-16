# Smoke test harness

`scripts/smoke/run.sh [driver.js] [out-dir]` launches the app in dev mode
on a private headless display and runs a driver script inside the webview.
It exits 0 only if the driver logs `done`. It doesn't touch the user's
desktop, so it's safe to run while they work.

```sh
scripts/smoke/run.sh                            # basic.js
scripts/smoke/run.sh scripts/smoke/layout.js    # any other driver
# The same on Windows (the window opens on the desktop; PowerShell 5.1+)
powershell -ExecutionPolicy Bypass -File scripts\smoke\run.ps1 [scripts\smoke\layout.js]
```

## How it works

Debug builds only. `app/src-tauri/src/lib.rs` evaluates the file in
`CHESSDB_SMOKE` after page load. `app/src/main.ts` exposes
`window.__chessdb = { db, game, jobs, analysis, prefs, tree, panels, dock, reference, onboarding, notices, library, appearance, api, invoke }`
once the window's shell is mounted.
The driver calls the stores exactly as the UI does and reports through the
`dev_log` command as `[smoke] …` lines.

## Drivers

Each driver's header comment says what it covers in full.

| Driver | What it checks |
|---|---|
| `basic.js` (default) | Imports the fixtures, sorts, opens a game, picks a branch in the next-move picker (with its arrows), changes the board appearance, adds a variation through the popover, types comments inline, promotes the variation, undoes, saves, reopens, folds variations, types moves, inserts a move, edits the intro comment, promotes a pawn and lists import warnings, asserting each step. With `testdata/large/lichess_1m.pgn` present it also imports 1M games and checks scrolling and sorting (`SMOKE_BIG=0` skips that). |
| `layout.js` | Resizes `#app` to several window sizes (tiled, full screen, narrow, short) in three panel layouts (UI-04: the default, the notation docked at the bottom, every panel on the left) and checks the board stays square, inside the dock's middle and clear of every side and divider, sides keep their minimums, with the promotion picker inside the board, nothing wider than the window (no sideways page scroll), every group's panel tabs fitting their strip, the game list's columns fitting it, and the ribbon (UI-18) on one row with every group inside it. **Run it after layout changes.** |
| `engine.js` | Analysis, lines, blunder check. Needs `scripts/fetch-stockfish.sh`. |
| `search.js` | Filters, header search, tabs, name lists, compaction, recent databases. |
| `tree.js` | Position index, opening tree, position search, opening names, reference database. |
| `setup.js` | Position editor. |
| `goto.js` | QOL-10: Ctrl+V with a FEN, the "Go to position" dialog. |
| `image.js` | Board images: the menu, copy as PNG and SVG, save, a failed save. |
| `onboarding.js` | Welcome guide, sample games, engine page, import wizard (QOL-08). |
| `view.js` | QOL-07: resizing columns, and the sort, widths and each kind of filter coming back after reopening. |
| `crosstable.js` | DB-16 on the fixture `testdata/pgn/crosstable.pgn` with known numbers: the Events list's Cross-table button, the round-robin grid (points, score, performance, Sonneborn-Berger, a forfeit, an unfinished game, tooltips), the rounds view, opening a game from a result, the game header's button with that game highlighted, a Swiss with byes and a shared place, two editions (a double round robin), Show games, light mode. With `testdata/large/twic1600.pgn`, real TWIC events (a round robin, a 597-player Swiss, a knock-out); with `__BIG__`, an event too big for a table and an online arena, timed. |
| `drag.js` | QOL-04/DB-06: selecting several games, dragging them onto another database, the board and the tab strip, Shift to move, Escape, the list menu's copy dialog, copying into a new and into the reference database. |
| `panels.js` | UI-19: the Notation, Opening tree and Reference tabs, the eval graph in Notation, the tree querying only while its tab shows, the move popovers from the tree tab, the keyboard, and the active tab restored after a reload, also from the old `treeVisible` pref. |
| `ribbon.js` | UI-18 and IO-08: the groups, Copy game then Paste game through the real clipboard, Paste position, the messages for a clipboard without a game, Ctrl+V with a PGN, the New game menu and Ctrl+N, the reference dropdown, and narrower widths: labels drop, then groups collapse into dropdowns. |
| `reference.js` | OPEN-14, on the fixture `testdata/pgn/reference.pgn` with known counts: the moves with Hot and Best players, the main line and lines, the top games' order, playing a move, the mini board stepping on its own and resetting when the board moves, opening a top game where it reaches the position, the dividers, the stacked narrow layout, the note without a reference database. |
| `dock.js` | UI-04: every panel tab dragged to each side and into other groups with synthetic pointer events, next to the nearest group, reordering a strip, Escape and a release elsewhere cancelling, the tab menu from a right-click, Shift+F10 and the context-menu key, empty groups and sides going, the dividers by keys, pointer and double-click and their limits, View ▸ Reset layout, and the `layout` pref read back after reloads, also from a version 1 layout with `listShare`, an incomplete one and an unreadable one. |
| `library.js` | DB-07: the library window (cards with counts, the preview, its mini board, resizing it, the bottom layout), Open spawning an editor window, favourites and the preview layout and size surviving a reload, the theme applying to the library itself, and preferences changed in one window reaching the other both ways (the orientation, a per-window one, staying put), and a change made just before a window closes being saved (the editor's, then the library's, checked from another window). Runs in every window, which take turns through a `smokeStep` pref. |
| `collapse.js` | UI-04: minimizing the Game list group to the status bar and restoring it, after a reload too. Opens an editor window from the library and runs there. |
| `cbh.js` | IO-03: Import games in the ribbon, a `.cbh` whose files are missing, and, with the databases that ship with ChessBase linked into `testdata/large/cbh-samples/`, Openings' four-language comments shown in one language and Marathon's set-up positions. |

`search.js`, `tree.js`, `view.js` and `reference.js` time the 1M-game
database when `__BIG__`; `drag.js` times copying it.

## Writing a driver

- **A new flow is a new driver.** Copy `basic.js`; the placeholders
  `__ROOT__`, `__OUT__` and `__BIG__` are filled in by `run.sh`. Log
  `shot:<name>` to request a screenshot and `FAILED: …` to fail.
- **The main window is the library (DB-07), not an editor.** A driver
  that needs the board or the panels opens an editor window from it
  (`api.openEditorWindow`) and does its work there: the harness runs the
  driver in every window that loads a page (`collapse.js`, `library.js`).
  An editor window doesn't reopen its database after a reload. Drivers
  written before DB-07 still expect an editor in the main window.
- **Never open native dialogs from a driver:** nobody can click them on the
  private display, so the driver hangs until it times out. Every driver
  starts with `game.confirmDiscard = async () => true`.
- **Reloading:** the harness runs the driver again after every page load,
  so a driver can test what a restart restores with `location.reload()`.
  Keep the phase it's in in `sessionStorage`, and wait for debounced pref
  writes to reach `api.uiPrefs()` first (`panels.js` does both; it's
  `{ values, revision }`).
- **Clipboard:** a driver logs `clip:<name>` to have the harness save the
  private display's clipboard (`wl-paste`) as `<out-dir>/clips/<name>.png`
  or `.txt`, and print the types on offer. Linux only; it never reads the
  user's clipboard (not with `SMOKE_VISIBLE`). Look at the images. Drivers
  put text on the clipboard with `api.copyText`, as Copy game does: WebKit
  refuses `navigator.clipboard.writeText` without a real user gesture.
- **First run:** every run starts with fresh settings, so the harness writes
  `{"welcomeDone": true}` first and the welcome guide stays closed. A driver
  that contains the text `smoke: first-run` (only `onboarding.js`) starts
  with no settings, as on a first run.
- **The app's DOM has traps for drivers:** the ribbon's hidden copy of
  itself, the dock's edge bands during a tab drag, and Ctrl+V. They're
  under Gotchas in [`app/AGENTS.md`](../../app/AGENTS.md).

## What it can't do

It can't drag pieces or press real keys (no input tools are installed), and
chessground ignores synthetic mouse events. It calls the same functions
those gestures call. The app's own pointer handling (list selection,
dragging games) does take synthetic `PointerEvent`s: `drag.js` dispatches
them at elements' real positions, with `buttons: 1` on moves (a move
without it ends the drag, as a release outside the window does).

## Settings, screenshots and output

- **Settings:** the harness sets `CHESSDB_CONFIG_DIR` to `<out-dir>/config`,
  so runs never touch the user's settings. The app keeps its data there
  too (the sample database), not in the user's data folder.
- **Screenshots:** taken with `grim` of the whole private display (the app
  window fills it), into `<out-dir>/shots/`. Look at them: a passing driver
  doesn't prove the layout is right. The display is 2560×1600 at scale 1.25
  like the user's screen (`OFFSCREEN_SIZE`, `OFFSCREEN_SCALE`), and the
  window has focus, so focus rings render. The webview follows the user's
  GTK theme, which is dark; `GTK_THEME=Adwaita scripts/smoke/run.sh …`
  shoots light mode.
- **Ports and output:** it uses dev port 1430 (`SMOKE_PORT`), so it can run
  next to a normal `tauri dev` on 1420. Output goes to
  `target/smoke/<timestamp>/`, including the display's log (`sway.log`).
- **Off-screen:** the app runs on the private display (next section) and
  never shows on, or changes, the user's desktop. Without sway the harness
  stops with exit code 2; it never falls back to the user's screen.
  `SMOKE_VISIBLE=1` opens the window on the user's desktop instead (it
  takes a tile and focus; no screenshots); only use it when the user asks
  to watch.

## Windows and macOS

- **Windows:** `scripts/smoke/run.ps1` takes the same drivers, placeholders
  and variables. Windows has no private display, so the window opens on the
  desktop; screenshots use `PrintWindow`, so covered windows still shoot.
  Paths reach the drivers with forward slashes (`C:/…`).
- **macOS:** there is no private display, so the window opens on the
  desktop and takes focus (only with the user's OK). Shots are
  `screencapture` of the app window and need Screen Recording permission
  for the terminal; without it the run goes on without shots. The script
  keeps to macOS's bash 3.2.

## The private display

The user works on the same machine while tests run, and a window that pops
up, takes focus or grabs a tile makes the computer unusable. So any window
you open for testing goes to a **private headless display**, through
`scripts/offscreen.sh`:

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
`run.sh` does. Needs `sway` (Arch: `pacman -S sway`); only its headless
mode is used, and it's never the user's session.

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

**Why the user's Hyprland is off limits:** the previous harness added a
headless output to it; that moved their auto-placed monitor to x=22048, and
Hyprland 0.54.3 left the cursor off screen when the output was removed.
Waybar and windows vanished too (`HYPRLAND-DISRUPTION-REPORT.md`). The rule
itself is in the root [`AGENTS.md`](../../AGENTS.md).

Check it worked: read-only snapshots of `hyprctl -j monitors all`,
`hyprctl cursorpos`, `hyprctl -j activewindow` and `hyprctl -j clients`
stay identical before, during and after the run.
