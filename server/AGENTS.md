# server/: the self-hosted server (NET-03)

What's specific to `chess-server`; the root [AGENTS.md](../AGENTS.md) still
applies. [`docs/server.md`](../docs/server.md) is the reference:
architecture, data directory, API, roles and what "private" guarantees.

## Layout

| Path | What |
|---|---|
| `src/lib.rs` | `AppState`, the router, middleware (security headers, Origin check, client address and anonymous rate limit), `serve` |
| `src/main.rs` | The CLI: `serve`, `admin create/password`, `healthcheck` |
| `src/access.rs` | Who may do what with a database (`require`) |
| `src/auth.rs` | Sessions (cookie + CSRF), bearer tokens, login throttling, the per-user rate limit |
| `src/store.rs` | The SQLite metadata store (schema version 2) |
| `src/pool.rs` | Open databases, their caches, single-writer slot, idle closing |
| `src/listing.rs` | What a game list shows (criteria, position, deleted), cached: the games route, the export and search share it |
| `src/jobs.rs` | `chess-jobs` jobs with their database and user, kept in the store |
| `src/audit.rs`, `src/metrics.rs`, `src/ratelimit.rs` | The audit log, `/metrics`, the token buckets |
| `src/backup.rs`, `src/twic.rs`, `src/mail.rs` | Online backups, TWIC updates (NET-06), reset links by e-mail |
| `src/housekeeping.rs` | Once a minute: audit retention, idle databases, scheduled backups and TWIC checks |
| `src/routes/` | The API, by area; `admin.rs` is the admin's |
| `src/wire.rs` | The JSON types (mirrors of the app's DTOs where they exist) |
| `build.rs`, `src/ui.rs` | The admin UI embedded from `admin/dist` |
| `admin/` | The admin UI: Svelte 5 + TypeScript + Vite, like `app/` |
| `tests/` | Integration tests against a running server (`tests/common`) |
| `Dockerfile`, `deploy/` | The container, and a compose example with Caddy |

## Commands

```sh
cargo test -p chess-server                       # unit + integration tests
cd server/admin && npm ci && npm run check && npm run build && npm test
cargo run -p chess-server -- serve --data target/server-data   # 127.0.0.1:8080
cd server/admin && npm run dev                   # the UI on 5175, /api passed to 8080
docker build -f server/Dockerfile -t chess-server .   # from the repository root
```

Rebuild the UI before the server when the UI changed: `build.rs` embeds
whatever `admin/dist` holds (a page saying how to build it when there's
none).

## Rules

- **Every route that touches a database's games goes through
  `access::require`** with the access it needs. A database the caller may
  not read is "not found" (404), never "forbidden", and is checked before
  anything else in the handler (role checks, validation, reading a body),
  so its existence doesn't leak. A route with a body about a database
  takes it as `Bytes` and parses it after the check (`routes::json_body`):
  an extractor would read it first. Requests that read or change a
  database's games take it with `AppState::open_db` (503 while it's being
  compacted).
- **Never log game content, search criteria, positions or file names in
  the audit log** (`state.audit`); the admin's jobs view leaves out labels
  and messages for the same reason. No database name, id or username in a
  metrics label either.
- **Backups are never downloadable**, and no admin route reads games:
  they'd let the admin read private databases.
- **The admin gets no access to games from being admin.** Routes in
  `routes/admin.rs` return metadata only; never call a chess-db read of
  games, positions or searches there. Visibility, shares and owner are the
  owner's; `AdminUpdateDatabase` refuses unknown fields so they can't
  creep in.
- **No path from user input** except through `secret::is_db_id` (database
  folders) and `routes::imports::resolve_in` (the import folder). `Pool::path`
  asserts the id.
- **Core crates stay synchronous**: call chess-db and the store inside
  `blocking(…)`. Never add an async runtime to `crates/`.
- Wire types live in `wire.rs`. Where the app has a DTO for the same thing,
  keep its name, fields and casing (`app/src-tauri/src/dto.rs`), but don't
  edit or import the app's: the desktop client will share them later.
  `admin/src/lib/types.ts` is written by hand; keep it in step.
- Jobs that write a database (imports, index builds, compaction) take its writer slot
  (`OpenDb::writer`): one at a time. A backup holds it while it copies the
  database. Compaction renumbers games: bump the `idEpoch` (the store).
- Tests turn the rate limits off (`tests/common`); a test of them sets them.
- The admin UI runs under a strict Content-Security-Policy: no inline
  scripts or `style` attributes (classes only), nothing from other origins.
- Tests bind 127.0.0.1 only (`tests/common::TestServer`), with a temporary
  data directory. If you look at the UI, use headless Chromium; never open
  a window on the user's desktop.

## Gotchas

- Floats sent as JSON can come back a unit in the last place off
  (serde_json without `float_roundtrip`): compare tree results with a
  tolerance, as `tests/api.rs` does.
- Argon2id is slow unoptimized: the root `Cargo.toml` builds `argon2` and
  `blake2` at `opt-level = 3` in dev, or every login in the tests takes
  seconds.
- The upload route disables axum's body limit and enforces
  `Config::max_upload` itself (`http_body_util::Limited`), with or without a
  Content-Length.
- Jobs are kept in the metadata store (`jobs.rs`): insert a row before a
  job starts (`Jobs::spawn` does), and an import notes how it goes on
  (`set_resume`) before each batch. The setup token is new at each start
  until there is a user.
- `server/tests/restart.rs` runs the real binary (`CARGO_BIN_EXE_chess-server`)
  and SIGKILLs it mid-import; it needs no network but takes a few seconds.
