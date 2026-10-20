# app/: the Tauri shell and the Svelte UI

What's specific to `app/`; the root [AGENTS.md](../AGENTS.md) still
applies. Paths below are from the repo root. To try a change in the
running app, see [`scripts/smoke/README.md`](../scripts/smoke/README.md).

## Conventions

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
- `svelte-check` doesn't support TypeScript 7 yet. Stay on TS 6.

## Debugging

`window.__chessdb` in `npm run tauri dev` holds the stores, `api` and
`invoke` (`app/src/main.ts`); handy from the webview inspector (right-click
→ Inspect Element in dev builds).

## Gotchas (each one cost time)

### Board and layout
- **chessground puts `cg-wrap` and `orientation-*` on the element you pass
  it**, not on a child. The board theme lives in `app/src/board-theme.css`
  (shared by the main board and the setup editor) as
  `.chessboard.cg-wrap.orientation-white …`; give a new board the
  `chessboard` class.
- **Board colours come in pairs:** each theme in `board-theme.css` sets
  `--board-light-base`/`--board-dark-base` (light mode) and `-dimmed`
  (dark mode), and `--board-light`/`--board-dark` pick one. Board images
  (QOL-06) read the `-base` ones in any mode and parse them in Rust, so
  keep them `#rrggbb`.
- **An `auto` grid column can't measure a board sized by `height: %` +
  `aspect-ratio`.** The column only fit the nav bar, and the board spilled
  over the notation at full screen. `fitBoard()` in `App.svelte` sizes the
  board in pixels from the dock's middle (UI-04) less its padding, eval bar
  and the nav bar's measured height; keep its constants (and `middleMin`,
  the room dividers stop at) in step with the CSS of the board column and
  `BoardNav.svelte`.
- **`container-type` makes an element the containing block of its
  `position: fixed` children** (layout containment). The board column holds
  the board settings dialog, so the nav bar's compact form is chosen in
  `fitBoard()` from the middle's width, not by a container query.
- **Browsers cap element height** (~17–33M px). `GameList.svelte` scales the
  scrollbar above 8M px and handles wheel scrolling itself. Test big lists
  with the 1M-game database.

### Dock, panels and ribbon
- **Flex grow factors that add up to less than 1 leave the rest empty.**
  A dock side's groups grow by their shares, and the engine analysis takes
  only its content's height while the engine is off, so the other groups'
  shares no longer added up to 1 and a quarter of the side stayed blank.
  `Dock.svelte` (`grow`) divides each share by the shares of the groups
  that take one.
- **During a tab drag (UI-04) the edge bands overlap tab strips.** The
  strips are raised above the bands (`body.dragging-tab`), so a drop on a
  strip joins the group. A driver aiming at a left or right band near its
  top must stay below the strips there (`dock.js`'s `onBand`). The top band
  sits over the ribbon's lower edge and the bottom one over the status bar,
  so a side full of groups can still get a new one.
- **A hidden panel tab hides everything in it** (UI-19 hides inactive
  panels with `hidden`, so `display: none`), `position: fixed` popovers
  included. Popovers that must open while their panel is hidden live
  outside it, like the move popovers in `MovePopovers.svelte`. Panels stay
  mounted while hidden; anything that queries must check
  `panels.isShowing(id)`, as the opening tree does.
- **The ribbon has a hidden copy of itself.** `Ribbon.svelte` renders every
  group in each form (full, compact, collapsed) in an inert, invisible
  `.ribbon .measure` to measure them, so it never has to try forms on the
  live buttons. Drivers look for ribbon buttons under `.ribbon > .groups`
  (by `aria-label`, which every ribbon button has); a plain
  `querySelectorAll("button")` also finds the copies, after the live ones.
- **The Reference tab's lines job is quiet.** Its kind (`reference`) gets
  no notice when done or cancelled (`jobs.svelte.ts`): the tab cancels it
  whenever the board leaves the position, and shows its own progress.

### Svelte and CSS
- **Svelte keeps whitespace between template nodes.** `NotationLine.svelte`
  deliberately has no whitespace between nodes and uses explicit `{" "}`,
  so parentheses hug their moves. Keep it that way when editing.
- **Svelte 5 props are live getters.** A dialog shown by `{#if thing}` with
  `prop={thing.x}` throws once its `onClose` sets `thing = null` and it then
  reads `prop`. Copy props into locals before calling `onClose`
  (`CopyGamesDialog.svelte`).
- **Table cells from a child component miss the parent's scoped CSS.**
  The moves tables of the opening tree and the Reference tab share their
  cells (`StatsHead.svelte`, `StatsCells.svelte`), so their styles are
  global, under `.stats-table` in `app.css`; a table that changes them
  uses `:global(…)` (`ReferenceMoves.svelte`). For a column that takes
  the rest of a WebKit table, give it `width: 100%; max-width: 0`: a
  fixed width on another column doesn't stop that one taking it.

### App windows (DB-07)
- **A closing window's page sends nothing from `blur`, `pagehide`,
  `visibilitychange`, `beforeunload` or `unload`:** the window is gone
  before the call reaches Rust. Work that must finish before a window
  closes goes in `windowClose.ts` (one close handler per window, set up in
  `main.ts`; a shell adds its checks with `beforeClose`). Tauri waits for
  that handler, so keep what it awaits short and bounded.
- **A session (a tab's game) belongs to its window.** A command that adds
  one takes the `Webview` and passes its label (`Session::in_window`);
  `lib.rs` drops a window's sessions when it closes or loads its page
  again, since the page closes none as it goes. A session left over counts
  as a game open in a tab, and Delete in Find duplicates (DB-08) leaves
  that game's group.

### WebKit and the clipboard
- **WebKit only writes the clipboard during a user gesture**, and the
  gesture ends at the first `await`. It also **fires no `paste` event where
  nothing is editable**, and `navigator.clipboard.readText()` needs
  clipboard access Tauri doesn't enable. So Copy game and Copy position
  write through Rust (`copy_text`), the paste buttons read through it
  (`clipboard_text`), and only Ctrl+V uses the page's paste event: Ctrl+V
  with a FEN (QOL-10) first moves the focus to a hidden textarea
  (`FenPaste.svelte`); the paste lands there and the focus goes back.
  Drivers can't press Ctrl+V: `goto.js` sends the keydown, then a paste
  event with a stubbed `clipboardData`.
