# Workspace UI: ribbon, panel tabs, docking and the Reference tab

Build spec for the ChessBase-style workspace the user asked for on
2026-09-30: a ribbon, tabbed panels that dock to the four sides of the
window, and a Reference tab. Read [AGENTS.md](../AGENTS.md) and
[handoff.md](../handoff.md) first; this file says what to build, in what
order, and how to check it.

The user's notes and the ChessBase 18 screenshots they're based on are
outside the repo: `~/Documents/notes/blog/Chessbase clon/Chessbase UI features.md`
(images in that folder and in `~/Documents/notes/blog/`). Look at them, but
don't copy them into the repo: they're ChessBase's screens, not ours to ship.

## At a glance

| Step | ID | Feature | Today | To build |
|---|---|---|---|---|
| 1 | UI-19 (new) | Panel tabs | Notation, tree, eval graph and engine lines are stacked in one column; the tree opens with a toolbar button | A tab group with Notation and Opening tree; the game header and eval graph live in the Notation tab |
| 1 | ANL-04 | Eval graph | Built and Lichess-style, under the notation | Moves into the Notation tab; no restyle |
| 2 | UI-18 (new) | Ribbon | One row of text buttons that wraps below about 1,400 px | Labelled groups: Clipboard, Game, Reference, Analysis, Database, View |
| 2 | IO-08 | Clipboard | Copy PGN, copy FEN, paste a FEN (Ctrl+V, Go to position…) | Paste game (PGN) from a button and from Ctrl+V; copy buttons renamed |
| 2 | DB-13, DB-07 | Reference dropdown | The reference is chosen in the opening tree's header | A dropdown in the ribbon listing recently opened databases |
| 3 | OPEN-14 (new) | Reference tab | Only the tree's moves table exists | Moves with Hot and Best players, main variations, top games by rating, a mini board |
| 4 | UI-04 | Docking | Only the divider between the list and the board moves | Drag any tab to the left, right, top or bottom; resize; remembered (built 2026-10-01, see [As built](#as-built-2026-10-01-1)) |

Work the user wants tracked for later is under [Tickets for later](#tickets-for-later).

## What the user decided (2026-09-30)

- **Docking:** panels dock against the four sides of the window for now.
  Free docking may come later (T-1).
- **Ribbon:** the Clipboard group, a New game menu and the reference
  dropdown go in a ribbon.
- **Eval graph:** any style is fine for the first pass; the user prefers
  Lichess's. The current graph already looks like Lichess's.
- **Reference dropdown:** lists recently opened databases for now. Later:
  favourites first, then recent (T-3).
- **Opening tree:** becomes a tab of its own next to Reference; the
  Reference tab doesn't replace it. How the two differ will be settled
  later (T-4).
- **Eval graph placement:** it belongs to the Notation tab only.
- **Mini board:** steps through games and lines on its own, starting from
  the main board's current position.
- **Sub-tabs:** no Popularity or Endgames sub-tabs in the Reference tab for
  now (T-2).
- **Moves:** clicking a move in the Reference tab plays it on the main
  board, as the tree does.

Taken here rather than by the user (ask if one looks wrong):

- The ribbon replaces the whole toolbar: one bar, not a toolbar plus a
  ribbon. It has one page, with no Home / Insert / Board page tabs.
- The Reference tab reads the reference database. With none set, it reads
  the open database and says so.
- The board stays in the middle and can't be docked.
- "Tab" in "move each tab on its own" means the panel tabs (Notation,
  Reference…), not the game tabs above the board.

## Build order

Each step is one feature, leaves the app working, and gets its own commit
when the user asks for commits.

1. **UI-19 panel tabs**, with the eval graph moved into Notation. Small,
   and it brings in the panel model that docking extends.
2. **UI-18 ribbon**, with IO-08's paste game and the reference dropdown.
3. **OPEN-14 Reference tab.** Adds the third tab; most of the Rust work.
4. **UI-04 docking.** It rearranges everything, so it comes last, when
   every panel exists.

Steps 2 and 3 don't depend on each other and can be built on parallel
branches. Step 4 needs step 1.

---

## 1. UI-19 Panel tabs (and ANL-04 in the Notation tab)

In ChessBase the pane beside the board is one window with tabs (Notation,
Reference, Replay training, Table…). We start with two, and step 3 adds
Reference.

```
┌ board ─────────────────┐ ┌ [Notation] [Opening tree] ─────────────┐
│                        │ │ Aronian – Nakamura  1-0                │
│                        │ │ 1.c4 e6 2.g3 d5 3.Bg2 Nf6 4.Nf3 Be7 …  │
│                        │ │                                        │
│                        │ │ ▁▂▃▅▆▇ eval graph (Notation tab only)  │
└────────────────────────┘ ├ Engine lines (while the engine runs) ──┤
                           └────────────────────────────────────────┘
```

**Behaviour**

- The game column becomes a tab group with **Notation** and **Opening
  tree**. Notation is active by default.
- **Notation tab:** the game header (`GameHeader`), the notation and, at its
  bottom, the eval graph. The graph still shows only when at least 2 moves
  have evals.
- **Opening tree tab:** today's `OpeningTree` content. Its × button goes
  away. The toolbar's "Opening tree" toggle becomes "show the Opening tree
  tab" (in the ribbon's View group after step 2).
- **Engine analysis** (`AnalysisPane`) stays a panel of its own under the
  tabs. It shows while the engine runs, as it does now.
- **Queries:** a tab runs its query only while it's showing. The tree
  queries when its tab becomes active, and again on each move while it
  stays active. This replaces `tree.visible`; the `treeVisible` pref maps to
  "Opening tree tab active", so session restore (QOL-01) keeps working.
- **Remembered:** the active tab is saved as part of the layout pref (below).
- **Keyboard:** follow the WAI-ARIA tabs pattern (`role="tablist"`, `tab`,
  `tabpanel`; ←/→ move between tabs, plus Home/End). Arrow keys still step
  through the game when focus isn't in the tab strip. No new global
  shortcut is needed. If you add one, note that Ctrl+Tab already switches
  game tabs.
- **Naming:** these are *panel tabs*. The tabs above the board
  (`GameTabs.svelte`, UI-05) are *game tabs*. Keep the two apart in code,
  for example `PanelGroup.svelte` and `stores/panels.svelte.ts`.

**The panel model (UI-04 extends it)**

`lib/stores/panels.svelte.ts` holds the layout, so docking can extend it
without a rewrite:

```ts
type PanelId = "notation" | "tree" | "reference" | "analysis" | "games";
type Side = "left" | "right" | "top" | "bottom";
type Group = { panels: PanelId[]; active: PanelId; share: number }; // share of its side
type Zone = { size: number; groups: Group[] };                     // size: share of the window
type Layout = { version: 1; zones: Record<Side, Zone> };
```

Step 1 only uses the default layout: right = [notation, tree] above
[analysis]; bottom = [games]. Save it as one UI pref (`layout`) with a
version number. Anything unreadable falls back to the default.

**Eval graph (ANL-04).** It moves into the Notation tab and nothing else
changes. It already has the Lichess look: a dark background, White's
winning chance as the light area, a middle line, and dots on ?!, ? and ??.

**Checks**

- A new smoke driver `panels.js`: switch tabs, confirm the tree queries only
  while it's showing, confirm the graph is inside Notation, and confirm the
  active tab is saved in prefs.
- Update `tree.js` and `basic.js` wherever they toggle the tree.
- `layout.js` still passes. Look at the screenshots.

## 2. UI-18 Ribbon, IO-08 clipboard, reference dropdown (DB-13, DB-07)

One band under the game tabs, split into labelled groups like ChessBase's
Home ribbon (the user's `Full UI.png`). Everything on today's toolbar goes
into it: go through `Toolbar.svelte` and give each item a place. Keep the
existing titles and shortcut hints.

| Group | Contents (**bold** = large button) |
|---|---|
| Clipboard | **Paste game**, Paste position, Copy game, Copy position, Board image ▾ |
| Game | **New game ▾** (split button; its menu: New game, Set up position…), Save, Save as new, Undo, Redo |
| Reference | The reference database dropdown (wide), Find games, Go to position… |
| Analysis | Engine on/off, Blunder check…, Engines… |
| Database | New database, Open database, Recent ▾, Import games, Export |
| View | Notation, Opening tree, Reference (each shows that tab); Reset layout (once step 4 is in) |

**Look**

- **Buttons:** a large button has its icon above a one- or two-line label.
  Small buttons have the icon and label on one line, three to a column.
  Each group's caption sits under it, in sentence case and `--ink-soft`.
- **Icons:** inline SVG from a set with a GPL-compatible licence (Lucide is
  ISC, Tabler is MIT). Vendor only the icons you use and record the
  licence, the way `crates/chess-diagram/data/COPYING` does. Check that the
  package isn't deprecated. Icons use `currentColor`, so they work in both
  themes.
- **Colours:** design tokens only; every colour needs a dark-mode value.
- **Disabled states:** as today (for example, Copy game is disabled when no
  game is open).

**Narrow windows.** This also fixes the known gap where the toolbar wraps
below about 1,400 px.

- The ribbon never wraps to a second row.
- As the window narrows, small buttons lose their labels first. They become
  icon-only, with `title` and `aria-label`.
- After that, groups collapse from the right into one button named after
  the group, which opens the group as a dropdown.
- At every `layout.js` size (tiled, full screen, narrow, short) the ribbon
  is one row with nothing cut off.

**Keyboard**

- Tab reaches every button, in the order they appear.
- Dropdowns follow the menu pattern: ↑/↓ and Enter; Escape closes the menu
  and returns focus to its button.
- Add Ctrl+N (Cmd+N) for New game. It looked free on 2026-09-30 (Ctrl+T
  opens a new game tab); check the key handlers before you take it.
- No single-letter shortcuts: letters start a typed move (UI-02).

**Clipboard (finishes IO-08)**

- **Copy game** is today's Copy PGN, and **Copy position** is Copy FEN.
  Only the labels change.
- **Paste position** reads the clipboard and does what Go to position…
  does with that text (QOL-10: a FEN, a `[FEN]` tag, an EPD line or a
  Lichess link).
- **Paste game** reads the clipboard. If it holds a PGN game, the game
  opens as a new, unsaved game: in the current game tab, unless that tab has
  unsaved changes. This is the same rule QOL-10 and opening from the list
  follow.
  - Several games: open the first and show "Pasted the first of 5 games."
  - No game: show "The clipboard has no PGN game. Copy a game with its
    moves, or use Paste position for a FEN."
- **Ctrl+V** outside a text box (`FenPaste.svelte`) tries a position first,
  as now, then a PGN game. If neither works: "The clipboard has no game or
  position. Copy a PGN game, a FEN or a Lichess link and try again."
- **Reading the clipboard from a button:** a page only gets the clipboard
  in a paste event (see the WebKit gotcha in AGENTS.md), so read it in Rust.
  Add a command `clipboard_text() -> AppResult<Option<String>>` that uses
  `tauri_plugin_clipboard_manager`. It's already a dependency, and
  `commands/image.rs` writes with it. The frontend keeps no clipboard
  permissions.
- **Parsing:** add a command `paste_game(session: Option<u32>, text: String)
  -> AppResult<PastedGameDto>`. It returns a `GameViewDto`, the number of
  games in the text, and any import warnings, and uses chess-core's PGN
  reader. Register it in `generate_handler!` and add the `api.ts` wrapper.
- **New game ▾:** the main part of the button does what New game does
  today. Its menu has "New game (Ctrl+N)" and "Set up position…" (today's
  dialog).

**Reference dropdown (DB-13, DB-07)**

- **The button:** a wide dropdown showing the reference database's name,
  or "No reference database". Title: "Reference database: what the
  Reference tab and the opening tree read".
- **The menu:**
  - The recently opened databases, most recent first, from
    `api.recentDatabases()` as `RecentMenu` uses. The open database is
    included.
  - The current reference is ticked. Databases that are gone are disabled
    and marked "Not found".
  - Then: "Choose a database…" (`db.chooseReference()`), "Use the open
    database (<name>)" when it isn't the reference already, and "Stop using
    a reference database".
- **Choosing:** picking a database calls `db.setReference(path)`. Whatever
  reads the reference follows: the Opening tree tab when its source is the
  reference, and the Reference tab.
- **The tree's own select:** the reference-setting entries move out of it.
  It keeps two choices: the open database and the reference database.
- **Favourites later (T-3):** build the menu so a "Favourites" section can
  go above the recent databases.

**Checks**

- A new smoke driver `ribbon.js`:
  - Copy game, then Paste game, using the private display's clipboard (log
    `clip:<name>` to save it).
  - Paste position with a FEN.
  - A clipboard with no game shows the error.
  - The New game menu.
  - The reference dropdown: choose from recent, use the open database, stop
    using one.
  - Ctrl+V with a PGN: send the keydown, then a paste event with stubbed
    `clipboardData`, as `goto.js` does.
- Drivers can fill the clipboard with Copy game or with
  `navigator.clipboard.writeText`.
- Screenshots of the ribbon at every `layout.js` size, in light and dark.
- Update drivers that find toolbar buttons by label or selector: grep
  `scripts/smoke/` for `Toolbar`, `nav[aria-label` and the button texts.

## 3. OPEN-14 Reference tab

ChessBase's Reference tab answers four questions: what is played here, how
often, by whom, and in which strong games (the user's
`Pasted image 20260930184005.png`). Ours has four parts. They read the
reference database, or the open one when none is set, with the line
"Reading <name>: no reference database is set. Choose one in the ribbon."

```
┌ [Notation] [Opening tree] [Reference] ────────────────────────────┐
│ Mega 2019 · E05 Catalan, open · 14,276 games          Find games  │
│ Move  Games  Score  W/D/B   Elo   Last  Hot      Best players     │
│ 6.b3  6,728  57%    ███▒░   2475  2018  ▮▮▮▮     So, Kramnik, …   │
│ 6.d4  6,413  62%    ███▒░   2490  2018  ▮▮▮▮▮ ↑  So, Kramnik, …   │
├ Main line: 6.d4 dxc4 7.Qc2 (1,720 games) ─────────────────────────┤
│ 6.b3 c5 7.Bb2 Nc6 8.e3 b6         712   59%  2018  2333           │
│ 6.d4 dxc4 7.Qc2 a6 8.Qxc4 b5      892   57%  2018  2364           │
├ Top games ──────────────────────────────────┬ Mini board ─────────┤
│ White     Elo   Black       Elo   Res  Year │  ┌──────────┐       │
│ Kramnik   2792  Mamedyarov  2808  1-0  2018 │  │          │       │
│ So        2778  Kramnik     2792  ½-½  2018 │  └──────────┘       │
│ …                                           │  ⏮ ◀ ▶ ⏭  ⟲        │
└─────────────────────────────────────────────┴─────────────────────┘
```

**The whole tab**

- **Sections:** draggable dividers split the three sections, and their
  sizes are remembered.
- **Narrow or wide:** use container queries. When the tab is wide, top
  games and the mini board sit side by side; when it's narrow (docked on a
  thin side), they stack.
- **Header:** the database name, the opening name, the total game count,
  and Find games (`tree.findGames`, which lists every game in the game list).
- **Queries:** like the tree, the tab queries only while it's showing, and
  again on every move. It drops replies for positions the board has
  already left; copy the `#seq` pattern in `stores/tree.svelte.ts`.
- **Past the index:** beyond the indexed plies (the first 50), it shows the
  tree's note and its "Search every move of every game" button.

### 3a. Moves table

- **Columns:** Move · Games · Score (for the side to move) · White/draw/Black
  bar · Elo (average of the side that played the move) · Last played (year)
  · Hot · Best players. The tree's `TreeStats` already covers the first six.
  Share the row code with `OpeningTree.svelte` instead of copying it.
- **Playing a move:** clicking a move, or Enter on a focused row, plays it
  on the main board (`tree.play`) with the usual variation choice.
- **Hot:** how much the move is played lately.
  - *Recent* means games from the last 3 years with a known year, counted
    back from the newest year in the database.
  - The bar's length is the move's share of the recent games in this
    position.
  - Show ↑ when that share is at least 1.25 times its share over all years
    and the move has at least 5 recent games; show ↓ at 0.8 times or less.
  - Tooltip: "12% of games from 2016–2018 (7% over all years)".
  - `GameStat.year` is already in memory, so this is cheap.
- **Best players:** up to four different players with the highest Elo
  among the games where they played this move. That's the side to move, at
  their Elo in that game.
  - Show surnames; the tooltip has the full name and Elo.
  - To stay fast at a position with a million games, pick candidates by Elo
    from `GameStat` (in memory). Read player names only for the top
    candidates, until you have four different players.

### 3b. Main variations

- **Main line:** from the position, follow the most-played move for as long
  as it keeps at least 10% of the position's games and at least 20 games,
  up to 10 plies. Shown as "Main line: 6.d4 dxc4 7.Qc2 (1,720 games)".
- **Lines:**
  - Up to 12 lines, each 6 plies long. A line is shorter if its games end
    or its count drops below 5.
  - Each line shows its number of games, the score for the side to move at
    the starting position, the year it was last played, and the average Elo
    of both players.
  - Group lines by first move, in the moves table's order. Show at most 3
    per first move, most games first within each group.
- **Clicking a line** shows it on the mini board (3d).
- **Data:**
  - The index stores only the next move for each (game, ply). Counting
    lines therefore needs each game's moves after the position: read the
    games' blobs with `for_each_game` in `positions.rs` (the parallel reader
    `scan_position` uses) and count the next 6 plies into a trie.
  - For very common positions in the 1M-game database this can take
    seconds. Show the moves table and top games first, fill in the lines
    when they're ready, and cancel when the position changes.
  - When more than about 50,000 games reach the position, run it as a
    `chess-jobs` job with progress and Cancel.
  - Cache results by (database, position key) until the database changes.

### 3c. Top games

- **Which games:** the games reaching the position, deleted ones left out.
  - Highest average Elo of the two players first. A game with only one Elo
    uses that one; games with neither go last.
  - Ties: newest first.
  - Show the first 100. No re-sorting by column in the first pass.
- **Columns:** White · Elo · Black · Elo · Result · Year · Event.
- **Selecting** a game (click, ↑/↓) shows it on the mini board (3d).
- **Opening:** Enter or a double-click opens the game on the main board, at
  the ply where it reaches the position. It opens the way opening from the
  list does: in the current tab, unless that tab has unsaved changes.
  Context menu: Open, Open in a new tab.
- **Data:** sort `PositionTree.games` in memory by the `GameStat` Elos, and
  read records only for the 100 shown.

### 3d. Mini board

- **Look:** a small square board, about 200–280 px and sized by its
  section. It uses the app's board theme (the `chessboard` class and
  `board-theme.css`), with no coordinates, and is view-only (no dragging
  pieces).
- **Starting position:** it starts at the main board's position, and goes
  back to it whenever the main board moves.
- **Stepping:** selecting a game (3c) or a line (3b) loads it at the current
  position.
  - ⏮ ◀ ▶ ⏭ step through it. A game goes back to its first move and on to
    its end; a line goes from the position to the line's end.
  - ← → work when the mini board area has focus.
  - ⟲ goes back to the main board's position.
- **Caption:** names the move and the game, for example
  "24…Nf6 · Kramnik – Mamedyarov, 2018".
- **Independence:** it never changes the main board.
- **Data:** a command returns one game's main line as positions (FEN, SAN,
  the last move as UCI, ply), plus the ply where the game reaches the
  current position. The frontend only draws them; Rust owns the chess
  logic. Lines from 3b come with their positions in the same shape.

### Rust side

- **chess-db (no Tauri):** new functions next to `position_tree` in
  `positions.rs`, or in a new `reference.rs`: Hot stats, best players, top
  games, the main line and the lines.
  - Unit tests on small fixtures in `testdata/pgn/`. Cover ties, missing
    Elos and years, and games that end within the 6 plies.
- **Commands,** in `commands/tree.rs` or a new `commands/reference.rs`:
  - `reference_view(handle, session)`: moves with Hot and Best players, plus
    top games. This one is fast.
  - `reference_lines(handle, session)`: may be slow, so it becomes a job
    when the position is common.
  - `game_line(handle, game, session)`: for the mini board.
- **DTOs:** `#[derive(Serialize, TS)]`, `camelCase`, `#[ts(type = "number")]`
  on `u64`. Regenerate the bindings.
- **Performance targets,** on the 1M-game database in `tauri dev`:
  - Under 200 ms for the moves table with Hot and Best players plus top
    games, at any position, the starting position included.
  - Under 500 ms for the lines, for positions fewer than 50,000 games reach.
  - Measure in the smoke driver's `__BIG__` part and record the numbers in
    handoff.md.

### Checks

A new smoke driver `reference.js`:

- Fixtures with known counts: moves, Hot arrows, best players, lines, the
  main line, and the order of top games.
- Clicking a move moves the main board.
- Selecting a game lets the mini board step without moving the main board.
- A move on the main board resets the mini board.
- With no reference set, the tab reads the open database and shows the note.
- With `__BIG__`, time the starting position and a deep one.

### As built (2026-10-01)

Built as specified, on branch `open-14-reference-tab`. Where the spec left
a choice, this is what was taken (ask the user if one looks wrong):

- **Hot ↓** also needs the move to have had at least 5 recent games at its
  share over all years (that share × the position's recent games). Without
  it, every rare move with no recent game got ↓. ↑ is as specified. Both
  compare the shares exactly, in integers.
- **Lines:** a first move always starts a line, however few its games;
  past the first move a line goes on only while 5 games follow. A line
  ends where none of its continuations has 5 games, or where its games
  end. Ties (most games first) go to the newer line (last year played),
  then the moves table's order.
- **Main line:** at each ply the most played move, ties to the newer one,
  then the moves table's order; it needs 20 games and a tenth of the
  position's games at every ply. The main line is clickable too (it shows
  on the mini board).
- **Best players:** ties at the same Elo go to the newer game.
- **Top games:** ties at the same average go to the newer year, then the
  game added later. Games without a year come after those with one.
- **Mini board:** follows the main board's orientation. In the top games,
  ←/→ step the mini board (not the main one), since the selected game is
  what they'd expect to step; ↑/↓, Home/End and PageUp/PageDown move the
  selection, as the spec says for ↑/↓.
- **Sizes:** the dividers save `referenceSizes` (shares of the tab's
  height for the moves and the lines; default 30% and 25%).
- **Narrow:** below 560 px the top games and the mini board stack; below
  480 px the moves table drops the White/draw/Black bar and the year, and
  the lines their year and Elo.
- **While a position loads,** the last view of the same database stays,
  dimmed and inert, so the tab doesn't flash on every move.
- **Lines cache:** in memory, by database handle, position and database
  state (its change counter and indexed games), 32 positions. A hidden tab
  cancels a running lines job; a move cancels the one for the position it
  left.
- **Opening a top game** at its ply uses a new optional `ply` argument of
  `open_game`.

## 4. UI-04 Docking to the four sides

**The layout**

- The board stays in the middle.
- **Sides:** top and bottom run the full width of the window, under the
  ribbon and game tabs and above the status bar. Left and right fit between
  them.
- **Groups:** each side holds zero or more tab groups, stacked top to bottom
  on the left and right sides and placed side by side on the top and
  bottom. An empty side takes no space.
- **Panels:** Notation (with the game header and eval graph), Opening tree,
  Reference, Engine analysis and Game list. Each panel is a tab and moves on
  its own.
- **Default layout** (today's): right = [Notation, Opening tree, Reference]
  above [Engine analysis]; bottom = [Game list].
- **The user's screenshots:**
  - Drag the right side's divider to the left for a big notation and a
    small board (`Pasted image 20260930185704.png`), or to the right for a
    big board (`…210456.png`).
  - Drag the Notation tab to the bottom to put the notation under the board
    (`…211004.png`).

**Moving a tab**

- **Dragging:** after 4 px of movement, drop targets appear:
  - A strip along each window edge, to dock there as a new group. On a side
    that already has groups, the new group goes next to the group nearest
    the pointer.
  - Every group's tab strip, to join that group where the pointer is.
- A label under the pointer says what will happen ("Dock on the left",
  "Move into Notation"). Escape, or releasing anywhere else, cancels.
- **Reordering:** dragging a tab along its own strip reorders it.
- **Keyboard and menu:** right-click a tab, or press the context-menu key or
  Shift+F10 on a focused tab, for "Move to ▸ Left, Right, Top, Bottom" and
  "Move into ▸ <each other group>".
- **Empty groups and sides:** when a group's last tab leaves, the group
  goes. When a side's last group goes, the side collapses and the board
  grows.
- **Reset:** View ▸ Reset layout in the ribbon restores the default.

**Sizes**

- **Dividers:** there's one between each side and the board, and one
  between groups on a side.
  - Drag a divider, or focus it and use the arrow keys (16 px steps);
    double-click resets it.
  - They use `role="separator"` with `aria-orientation` and `aria-valuenow`.
  - They replace the list/board divider (`listShare`). Carry its saved
    value over to the bottom side.
- **Window resizes:** sides are stored as shares of the window, so they
  scale when it's resized or tiled, as the list does today.
- **Minimums:** 220 px for a left or right side, 120 px for top or bottom.
  The middle always keeps room for a 160 px board plus its chrome, and
  dividers stop there. When the window gets too small for all of that, the
  sides shrink in proportion; the board never goes below its minimum.
- **The board:** it's the largest square that fits the middle. That's
  `fitBoard()` in `App.svelte`, now measured from the middle area instead of
  the workspace minus `NOTATION_MIN`. Keep its constants in step with the
  CSS (an AGENTS.md gotcha), and keep the 8-device-pixel snapping.
- **Panels at any size:** every panel works at any side's size. It scrolls
  inside, the page never scrolls sideways, and the game list's columns stay
  usable when it's docked on a side.

**Remembered.** The whole layout (groups, order, active tabs, sizes) is
saved in the `layout` pref from step 1 and restored at start (QOL-01).

**Drag code.** Use the app's own pointer handling, like the game drags in
QOL-04 (`stores/drag.svelte.ts`). The events go to our elements, not to
chessground, so smoke drivers can send synthetic `PointerEvent`s (with
`buttons: 1` on moves).

**Checks**

- A new smoke driver `dock.js`:
  - Drag each panel to each side, and into another group, with synthetic
    pointer events.
  - The menu path and reordering.
  - Moving a divider with the keys.
  - Reset layout.
  - The layout pref is written and read back.
- Extend `layout.js` to run its window sizes in three layouts: the default,
  the notation at the bottom, and everything on the left. In each, the
  board is square, inside the middle and clear of every panel.
- Screenshots of each layout, in light and dark.

### As built (2026-10-01)

Built as specified, on branch `ui-04-docking`. Where the spec left a
choice, this is what was taken (ask the user if one looks wrong):

- **Every group has a tab strip**, even with one tab (Engine analysis,
  Game list), since tabs are dragged from it and dropped on it. The game
  list's own tabs (Games, Players…) stay under its panel tab.
- **Engine analysis while the engine is off:** its tab stays, with "The
  engine is off. Analyze". Alone in a group stacked with others, that group
  takes only its content's height (as before docking, the notation keeps the
  room), and there's no divider next to it until the engine runs; then it
  takes its share (a quarter of the right side by default).
- **Sizes:** sides are shares of the window's width (left, right) or height
  (top, bottom), like `listShare` was. Defaults: right 60% (the board fills
  the middle's height in a full-screen 16:10 window), bottom 28% (the old
  list default), left 30%, top 25%. An empty side keeps its size for when a
  panel docks there again.
- **Opening a side** takes its room from the side across the board first, as
  long as that one keeps as much as the new one, so the board keeps its size
  where it can (Notation docked on the left halves the right side instead of
  squeezing the board to 160 px).
- **The middle's room:** a 160 px board plus 46 px across (padding and the
  eval bar) and 18 px plus the nav bar down. In a middle narrower than 480 px
  the nav bar turns compact (Flip board becomes an icon), and below about
  330 px it wraps onto two rows. When the window can't hold everything,
  sides above their minimum shrink first, in proportion to what they have
  above it, then all of them.
- **The board** is centred in the middle, both ways, when it has room to
  spare.
- **Drop targets:** the bands are 34 px; the top one sits over the ribbon's
  lower edge and the bottom one over the status bar, so a side full of
  groups still takes a new one; the left and right ones run between them.
  Tab strips are drawn above the bands. A drop on an empty side previews the
  side as it will open; on a side with groups, a line shows where the new
  group goes.
- **Labels:** "Dock on the left" / "on the right" / "at the top" / "at the
  bottom"; with groups there, ", above (below, left of, right of) <group>".
  A group is named by the tab it shows ("Move into Notation"); in its own
  strip, "Move <tab> here". A tab alone in its group has no reorder target.
- **The tab menu** is one list with two headings, "Move to" (Left, Right,
  Top, Bottom) and "Move into" (each other group, named by all its tabs:
  "Notation · Reference"), not submenus. Move to its own side splits the tab
  off next to its group; it's off when the tab is alone there. Another side
  gets the new group at its end. The focus follows the tab to its new place.
- **New group sizes:** a new group gets a share as if every group of the side
  weighed the same (the engine's alone a third), and the others give way in
  proportion. A double-click on a divider between groups splits their room
  by those weights; on a side's divider, it goes back to the default size.
- **Dividers** have ←/→ or ↑/↓ (16 px), no Home/End. A divider between
  groups stops at 80 px for stacked groups, 160 px side by side.
- **Tab order** follows the screen: top, left, the board, right, bottom.
- **The game list on a narrow side** drops columns, last first in this order:
  White, Black, Result, Date, the two Elos, Event, #, Opening, ECO, Moves,
  Site, Round, flags. The flexible columns' minimums went down (names 80 px)
  so more fit before any goes.
- **The `layout` pref is version 2.** Version 1 (UI-19) meant only the
  active tabs; it's read for those, on the default layout. `listShare`
  becomes the bottom side's size, and both older prefs (`treeVisible`,
  `listShare`) are removed when the layout is first saved. A panel missing
  from a saved layout (one added later) joins the group of its default
  neighbours. Group ids aren't saved.

## Tickets for later

| Ticket | What | Catalog |
|---|---|---|
| T-1 | Free docking: split any group anywhere, float a panel in its own window, named layouts | UI-04 (the rest), UI-14 |
| T-2 | Reference tab sub-tabs Popularity and Endgames, as in ChessBase. Popularity likely shows how often lines were played over the years; Endgames, which endgames games from here reach. Ask the user what each should show before building them | OPEN-15 (new, P3) |
| T-3 | Favourite databases: star a database; the reference dropdown lists favourites first, then recent | DB-07 |
| T-4 | Opening tree vs Reference tab: decide what each is for (merge them, or give the tree another job). The user will clarify | Catalog §6, question 6 |
| T-5 | More panel tabs from ChessBase: Replay training (TRN-03), Table (UI-13), Plan explorer, Score sheet, LiveBook (OPEN-02), Openings book (IO-06, OPEN-03), My moves (OPEN-05) | Several |

## Checks for every step

- **Commands:** the full list in AGENTS.md: fmt, clippy with
  `-D warnings`, tests (which regenerate the bindings), svelte-check, build.
- **Smoke drivers:** run all of them, not just the new one. Include the
  1M-game parts (`testdata/large/`) and `engine.js` with Stockfish.
- **Screenshots:** look at them, in light mode too; only dark mode has been
  screenshotted so far.
- **The user's desktop:** it stays unchanged (the `hyprctl` snapshots
  described in AGENTS.md).
- **Docs:** update the status notes on the IDs in `docs/feature-catalog.md`,
  update `handoff.md`, and update this file if a decision changes.

## Code map

| Where | What |
|---|---|
| `app/src/App.svelte` | Grid, `fitBoard()`, list divider (`listShare`); becomes the dock layout |
| `app/src/lib/components/Toolbar.svelte` | Today's toolbar; `RecentMenu.svelte` and `BoardImageMenu.svelte` move into the ribbon |
| `app/src/lib/components/OpeningTree.svelte`, `lib/stores/tree.svelte.ts` | Tree table, source select, `#seq` stale-reply guard, `findGames`, `play` |
| `app/src/lib/stores/db.svelte.ts` | `reference`, `recent`, `chooseReference`, `setReference`, `openAt` |
| `EvalGraph.svelte`, `GameHeader.svelte`, `Notation.svelte`, `AnalysisPane.svelte`, `ListPane.svelte` | The panels' contents |
| `DropShelf.svelte` | Drop targets shown over the notation while games are dragged (QOL-04); keep it working wherever Notation is docked |
| `FenPaste.svelte`, `lib/stores/game.svelte.ts` | Ctrl+V; `copyPgn`, `copyFen`, `newGame`, go to position |
| `lib/stores/drag.svelte.ts`, `lib/stores/prefs.svelte.ts` | Pointer-drag pattern; UI prefs |
| `app/src-tauri/src/commands/tree.rs`, `game.rs`, `image.rs` | `opening_tree`, `search_position`; `goto_position`, `new_game`; clipboard through the plugin |
| `crates/chess-db/src/positions.rs`, `database.rs` | `find_position`, `position_tree`, `TreeStats`, `for_each_game`, `scan_position`; `GameStat`, `game_stats()` |
