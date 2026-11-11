# The server (NET-03)

A self-hosted server that keeps several users' databases and runs searches on
them, like ChessBase's online database but on your own machine or VPS. This
is **phase 1**: accounts, permissions, reading and searching games over a
JSON API, imports, and a web UI for the admin. The code is in `server/`
(binary crate `chess-server`); conventions for working on it are in
[`server/AGENTS.md`](../server/AGENTS.md).

Not in phase 1 (see [What's next](#whats-next)): editing games, sync with the
desktop app, the desktop client, an offline copy of a database, a web UI for
players (NET-09), encrypted ("sealed") databases.

## Architecture

```
 browser (admin UI) ──┐                       ┌──────────── chess-server ────────────┐
 desktop app (later) ─┼── HTTPS ── Caddy ── HTTP ── axum on tokio                    │
 scripts (API token) ─┘                       │     │ auth, permissions (every route) │
                                              │     ▼ spawn_blocking                  │
                                              │   metadata store   open databases    │
                                              │   (SQLite)         Arc<Mutex<Database>>
                                              │                    chess-db, chess-core
                                              │   chess-jobs: imports, index, lines  │
                                              └──────────────────────────────────────┘
                                                        │ /data volume
```

- **One process**, axum on tokio. The core crates stay synchronous and
  UI-free (no async runtime in `crates/`): every call into them, and into
  the metadata store, runs on tokio's blocking pool (`spawn_blocking`).
- **Databases** are the app's own format (`docs/file-format.md`), one folder
  each. They're opened on first use and stay open, each behind one
  `Mutex<Database>` (`server/src/pool.rs`), as the app holds them.
- **Long work** runs as `chess-jobs` jobs with progress and cancel: imports
  (then the position index), index builds, and the lines of common
  positions. A job records the database and the user who started it.
- **Metadata store**: SQLite (`server.sqlite`) for users, sessions, API
  tokens, databases and shares. Games are never in it.
- **The admin UI** (Svelte 5, `server/admin/`) is built into `dist/` and
  embedded in the binary (`server/build.rs`); it fetches nothing from the
  network.

## The data directory

`CHESS_SERVER_DATA` (`/data` in the container, a volume):

| Path | What |
|---|---|
| `server.sqlite` (+ `-wal`, `-shm`) | Users (Argon2id password hashes), sessions and API tokens (their SHA-256 only), databases (name, owner, visibility, quota) and shares |
| `dbs/<id>.chessdb/` | The databases. `<id>` is 32 random hex digits; the API never takes a path, only an id, checked to be exactly that before a path is built |
| `import/` | Files the admin imports from: PGN, or ChessBase databases (the `.cbh` with its `.cbg`, `.cbp`, `.cbt`, `.cbc`) |
| `tmp/` | Uploads being received or imported; emptied at start |

**It must be a local disk**, not NFS or SMB: databases are locked with
`File::try_lock` and rely on `fsync` for crash safety, which network file
systems don't reliably give. One server process per data directory (each
open database is locked).

**Backups**: stop the server (or the container), then copy the directory.
A copy taken while an import commits can hold a database's `games.blob`
from before a commit and its `games.idx` from after it, which doesn't open.
With no import running the database files don't change; the SQLite store
can be copied live with `sqlite3 server.sqlite ".backup copy.sqlite"`.

## Running it

### On a machine, for development

```sh
cd server/admin && npm ci && npm run build && cd ../..   # the admin UI (else a page says how)
cargo run -p chess-server -- serve --data target/server-data   # http://127.0.0.1:8080
```

It listens on **127.0.0.1:8080** by default: only this machine can reach it.
At the first start it prints a **setup token**; open the page, enter it and
make the first admin. Or make one from the command line (this works while
the server runs):

```sh
chess-server admin create alice          # asks for the password on stdin
CHESS_SERVER_PASSWORD=… chess-server admin password alice   # a new password; their sessions end
```

`npm run dev` in `server/admin` serves the UI with hot reload on 5175 and
passes `/api` to a server on 8080.

### Settings

| Flag / variable | Default | |
|---|---|---|
| `--data`, `CHESS_SERVER_DATA` | `./data` (`/data` in the container) | The data directory |
| `--bind`, `CHESS_SERVER_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in the container) | Address to listen on |
| `CHESS_SERVER_MAX_UPLOAD_MB` | 256 | Largest PGN upload |
| `CHESS_SERVER_PUBLIC_URL` | none | e.g. `https://chess.example.com`: makes the session cookie `Secure` and is the only origin state-changing requests may come from |
| `CHESS_SERVER_TRUST_PROXY` | off | `1` behind a reverse proxy: the client's address from `X-Forwarded-For` (for login throttling), https from `X-Forwarded-Proto`. Leave it off when clients reach the server directly, or they could pick their address |
| `RUST_LOG` | `info` | `debug` logs every request |

### In a container, behind Caddy

`server/Dockerfile` builds the admin UI, then the server with the UI
embedded, and copies the binary into a small Debian image, where it runs as
a non-root user (uid 10001) with `/data` as a volume, a
`HEALTHCHECK` (`chess-server healthcheck`, which asks `/api/health`) and
port 8080. `server/deploy/` has an example compose file with
[Caddy](https://caddyserver.com) in front, which gets the TLS certificate
for your domain:

```sh
docker build -f server/Dockerfile -t chess-server .          # from the repository root
cd server/deploy   # set your domain in compose.example.yaml and Caddyfile.example
docker compose -f compose.example.yaml up -d --build
docker compose -f compose.example.yaml logs chess-server     # the setup token
docker compose -f compose.example.yaml exec chess-server chess-server admin create alice   # or this
```

The server itself speaks plain HTTP: don't expose port 8080 to the
internet, only Caddy (or another proxy doing TLS). To give the admin files
to import, copy them into the volume's `import/` folder
(`docker compose cp Mega.cbh chess-server:/data/import/`, with its sibling
files) or bind-mount a host folder there read-only (commented out in the
compose file).

## Accounts

- **The first admin** comes from the setup token printed at the first start
  (a new one each start until there is a user; it's only in memory), or
  from `chess-server admin create`.
- **Passwords** are hashed with Argon2id (19 MiB, 2 passes; OWASP's
  recommendation), at most 4 at once. At least 8 characters.
- **Browser sessions**: `POST /api/login` sets an `HttpOnly`,
  `SameSite=Strict` cookie (`Secure` when the public URL is https or a
  trusted proxy says https), valid 14 days. Every state-changing request on
  a session must send the session's CSRF token in `X-CSRF-Token` (the login
  and `GET /api/me` return it), and requests with an `Origin` header must
  come from the server's own origin.
- **API tokens** (`POST /api/tokens`) for scripts and, later, the desktop
  client: `Authorization: Bearer cdb_…`. Shown once; the store keeps a
  hash. They need no CSRF token (a browser doesn't send them on its own).
- **Failed logins** are throttled per account and per client address: after
  5 failures within 15 minutes, further attempts (even right ones) are
  refused until the window passes (`429` with `Retry-After`). An unknown
  username costs the same time as a wrong password. Someone can lock an
  account out for 15 minutes this way; that's the trade-off.
- **Disabling** a user stops their sessions and tokens at once (every
  request checks) and keeps their data; their server-wide databases stay
  readable. The admin can't disable or demote themselves, and there is
  always an enabled admin. Users aren't deleted in phase 1.
- A new password set by the admin ends the user's sessions (not their API
  tokens); changing one's own ends one's other sessions.

## Roles and visibility

Two roles: **user** and **admin**. Each database has an owner and a
visibility:

| Visibility | Who reads it | Who imports into it |
|---|---|---|
| **Private** (the default) | The owner. Not the admin either | The owner |
| **Shared** | The owner and the users it's shared with | The owner and users shared with read/write |
| **Server-wide** | Every user | The owner and users shared with read/write |

- The **owner** renames, deletes, and sets the visibility and the shares.
  Shares are kept while a database is private but give nothing then.
- Only an **admin** can make a database server-wide (so users can't push
  content to everyone).
- The **admin** manages users and the server. For any database they see its
  metadata (name, owner, visibility, game count, size, quota, when it was
  made), set its quota, and delete it (confirmed with its name). They can't
  read a database that isn't theirs or shared with them, and can't change
  any database's visibility, shares or owner but their own: those routes
  check ownership like any other, and the admin's database route only takes
  a quota (other fields are refused). Ownership isn't transferable in
  phase 1.
- A database someone may not read answers **404**, exactly like one that
  doesn't exist, on every route (info, games, one game, tree, lines,
  shares, import, its jobs). One they may read but not change answers 403.
- The admin's **jobs view** shows what runs on which database for whom and
  how far it is, not a job's label, message or import report (they name
  files and count games).

### What "private" guarantees, and what it doesn't

It guarantees that **no route of the server** returns a private database's
games, positions, trees, lines, search results or import reports to anyone
but its owner, the admin included, and that nobody but the owner can make
it readable to others.

It does **not** protect the files from whoever can read the disk: the
databases are stored unencrypted, so the server's operator (or anyone with a
copy of the volume or its backups) can read them with the desktop app. A
user who needs secrecy from the operator should keep the database on their
own machine until an encrypted ("sealed") mode exists, a later phase. The
server process itself can read everything, so its host must be trusted.

### Importing from the server folder, and licences

The admin imports PGN files and ChessBase databases from `import/` (the
admin UI's Import page, or `POST /api/databases/{id}/import-file`) into a
database they may write to. **They choose its visibility, and it's private
by default**:

- **Server-wide** suits free reference data every user may read: TWIC
  issues, the Lichess database dumps.
- **Private** is for licensed data: a ChessBase **Mega Database is sold per
  user**, so it goes in as the admin's own private database, which only the
  admin can read. Making it server-wide would give every user a copy the
  licence doesn't cover.

## The API

JSON over HTTP under `/api`. Errors are
`{"error": {"code": "notFound", "message": "Not found"}}` with the status.
Types are in `server/src/wire.rs`; where the desktop app has the same thing
(`GameRowDto`, `GamePage`, `HeadersDto`, `NotationDto`, `ReferenceDto`,
`LinesDto`, `JobDto`, `ImportReportDto`), the JSON is the same, so the
desktop client can share the types later.

### Accounts

| Route | Login | |
|---|---|---|
| `GET /api/health` | none | `{"status":"ok"}` |
| `GET /api/setup`, `POST /api/setup` | none | Whether the first admin is still to be made; make it with `{token, username, password}` |
| `POST /api/login` | none | `{username, password}` → the user and `csrfToken`, and the session cookie |
| `POST /api/logout` | any | Ends the session (nothing for an API token) |
| `GET /api/me` | any | The user (and the session's `csrfToken`) |
| `POST /api/me/password` | any | `{currentPassword, newPassword}` |
| `GET /api/tokens`, `POST /api/tokens`, `DELETE /api/tokens/{id}` | any | One's API tokens; `POST` takes `{name}` and returns the token once |

### Databases (permission-checked on every route)

| Route | Needs | |
|---|---|---|
| `GET /api/databases` | | The databases one can read, with one's `access` (`read`, `write`, `owner`) |
| `POST /api/databases` | | `{name, visibility?}`: a new empty database, private unless asked |
| `GET /api/databases/{id}` | read | Its info and position index status |
| `PATCH /api/databases/{id}` | owner | `{name?, visibility?}` |
| `DELETE /api/databases/{id}?confirm=NAME` | owner | Deletes it for good |
| `GET /api/databases/{id}/shares` | owner | `[{username, access}]` |
| `PUT /api/databases/{id}/shares/{username}` | owner | `{access: "read" \| "write"}` |
| `DELETE /api/databases/{id}/shares/{username}` | owner | |
| `GET /api/databases/{id}/games` | read | A page of the game list (below) |
| `GET /api/databases/{id}/games/{n}` | read | One game: `row`, `headers`, `pgn`, `startFen`, `notation` (the move tree with comments, NAGs, clocks and evals), `opening` |
| `GET /api/databases/{id}/tree?fen=` | read | The position tree with Hot, best players and top games (`chess_db::reference_overview`); the start when no FEN |
| `GET /api/databases/{id}/lines?fen=` | read | The main line and lines (`reference_lines`): `{"kind":"ready","lines":…}`, or from 50,000 games `{"kind":"job","job":N,"games":…}`: ask again when the job is done |
| `POST /api/databases/{id}/import?filename=` | write | The request body is a PGN file (at most `CHESS_SERVER_MAX_UPLOAD_MB`) → `202 {"job": N}` |
| `POST /api/databases/{id}/import-file` | write, admin | `{path}`: a `.pgn` or `.cbh` under `import/` → `202 {"job": N}` |
| `GET /api/jobs/{n}` | read on its database | Its state and progress; its import `report` for who started it and the owner |
| `POST /api/jobs/{n}/cancel` | who started it, or the owner | |

**A page of games**: `offset`, `limit` (50, at most 500), `sort` (`id`,
`white`, `black`, `whiteElo`, `blackElo`, `date`, `event`, `site`, `round`,
`result`, `eco`, `plies`), `desc=true`, and the header search
(SRCH-01) fields of the app's `HeaderQueryDto`: `white`, `black`,
`ignoreColors`, `text` (any player, event, site or annotator), `event`,
`site`, `annotator`, `yearFrom`, `yearTo`, `ecoFrom`, `ecoTo`, `result`
(`whiteWins`, `blackWins`, `draw`, `decisive`), `eloMin`, `eloMax`,
`eloMode` (`both`, `either`, `average`), `movesMin`, `movesMax`,
`annotatedOnly`. The answer is `{total, offset, rows}`.

```sh
curl -H "Authorization: Bearer $TOKEN" \
  "https://chess.example.com/api/databases/$DB/games?white=carlsen&sort=date&desc=true&limit=20"
curl -H "Authorization: Bearer $TOKEN" --data-binary @twic1600.pgn \
  "https://chess.example.com/api/databases/$DB/import?filename=twic1600.pgn"
```

### Admin

| Route | |
|---|---|
| `GET /api/admin/users`, `POST /api/admin/users` | Users with their database count and use; `{username, password, role?}` |
| `PATCH /api/admin/users/{id}` | `{role?, disabled?, quotaBytes?}` (`null` removes the quota) |
| `POST /api/admin/users/{id}/password` | `{password}` |
| `GET /api/admin/databases` | Every database, as metadata |
| `PATCH /api/admin/databases/{id}` | `{quotaBytes}` only |
| `DELETE /api/admin/databases/{id}?confirm=NAME` | |
| `GET /api/admin/jobs`, `POST /api/admin/jobs/{n}/cancel` | |
| `GET /api/admin/disk` | Free space, and what the databases, the store, uploads and the import folder take |
| `GET /api/admin/import-files` | The `.pgn` and `.cbh` files in `import/` |

## Imports, jobs and quotas

- An import is a job: it reads the PGN (or the ChessBase database),
  committing a batch every 300 ms as the app does, then brings the position
  index up to date, so the tree and lines work. Cancel keeps the games
  committed so far; the index catches up at the next tree request (any read
  of a tree starts an index job when the index is behind).
- One write job (an import or an index build) per database at a time;
  another answers 409.
- An upload is streamed to `tmp/` and refused past the size limit (413),
  checked before reading (Content-Length) and while reading (chunked).
- **Quotas**: per database and per user (all the databases they own
  together), set by the admin. An import is refused when there's no room
  (507) and stops when the database outgrows it (checked every half
  second): the games so far stay and the job fails with "reached its
  quota". The size counts the position index, which the import builds
  after, so a database can end up somewhat over.
- Jobs live in memory: a restart forgets finished jobs, and a running
  import stops between two batches (keeping what it committed).

## Limits, and what changes for many users

Phase 1 is sized for a club or a team: a handful of concurrent users, a few
million games.

- **Queries on one database run one at a time.** Each open database is one
  `Mutex<Database>`, as in the app. chess-db's long scans (header search,
  sorting, lines) take the lock a chunk at a time, so they interleave, but
  two users searching the same big database wait for each other. With many
  users this is the first thing to change: read-only queries that take
  `&self` on a shared handle (most of chess-db's reads already work on
  `&Database`; the per-game stats and the name tables' caches need interior
  mutability or loading at open), or several read handles per database
  (the format's lock would need a shared mode), plus caching tree results
  per position.
- Search results and their sort orders are cached per database (8 each),
  and lines (32), until the database changes. A header search reads every
  index record (64 bytes a game), never the games themselves.
- Each open database keeps its files open and its per-game stats in memory
  (8 bytes a game) once a tree was asked for; nothing closes idle
  databases yet.
- One process; no horizontal scaling. Data on a local disk.
- No rate limiting beyond logins; put limits in the proxy if needed.
- TLS comes from the proxy in front; the server speaks plain HTTP.
- Opening a database the first time takes about half a second per million
  games (the first request to it waits).

## Measured

Release build on the development laptop (16 threads), the server capped at
4 GB of memory, one client on 127.0.0.1, Nov 2026. Times are whole HTTP
requests.

| | Lichess, first 1M games (2.3 GB PGN) | Mega Database 2019 (`.cbh`, 7.6M games), as the admin's private database |
|---|---|---|
| Folder import, then the position index | 61 s (52 s + 9 s); 1.4 GB on disk, 740 MB of it the index | 472 s (384 s + 88 s), 7,623,531 games (166 skipped, as importing it directly); 7.2 GB, 5.8 GB of it the index |
| Tree at the start | 84 ms the first time (loads the per-game stats), then 21 ms | 590 ms, then 293 ms |
| Tree after 1.e4 / after 5...a6 (Najdorf) | 23 ms / 2 ms (3,101 games) | 147 ms / 20 ms (189,080 games) |
| Lines after 5...a6 | 269 ms | A job (over 50,000 games); then from the cache in under 1 ms |
| First page by White (sorts every game) / next pages | 262 ms / 8 ms | 1.6 s / 4 ms |
| `text=magnus` by date / its next page | 577 ms (1,501 games) / 5 ms | 1.8 s (8,073 games) / 5 ms |
| One game (PGN and move tree) | 5 ms | 7 ms |
| 8 trees after 1.e4 at once | 174 ms: they wait for the database's lock in turn | 1.2 s |
| Server memory after the import | 630 MB resident | 1.0 GB with both databases open |

A cold header search or sort reads every index record, so on millions of
games it takes a second or two; repeating it, or paging through it, comes
from the cache.

## Licence

The server is part of the workspace and under its GPL-3.0-or-later. Whether
it should be AGPL-3.0 (so that people who run a modified server for others
must offer them its source) is an open question for the project.

## What's next

- **Phase 2**: the desktop app as a client (open a server database like a
  local one, through an API token), with its DTOs and the server's shared
  in one crate; sync of a user's own databases; an offline copy per
  database.
- Editing games on the server, and write access beyond imports.
- A web UI for players (NET-09).
- An encrypted "sealed" visibility, so a database is unreadable on the
  server's disk.
- Concurrency for many users (above), closing idle databases.
