# The server (NET-03)

A self-hosted server that keeps several users' databases and runs searches on
them, like ChessBase's online database but on your own machine or VPS. This
is **phase 1**: accounts, permissions, reading and searching games over a
JSON API, imports, and a web UI for the admin. The code is in `server/`
(binary crate `chess-server`); conventions for working on it are in
[`server/AGENTS.md`](../server/AGENTS.md).

Besides reading and imports it has: the games reaching a position (SRCH-03),
PGN export, searches across databases (SRCH-13), deleting games, duplicates
(DB-08) and compaction, deleting users, ownership transfers, invites and
reset links, jobs that survive a restart, online backups, rate limits, an
audit log, metrics and TWIC updates (NET-06).

Not in phase 1 (see [What's next](#whats-next)): editing games, sync with the
desktop app, the desktop client, an offline copy of a database, parallel
queries on one database, a web UI for players (NET-09), encrypted
("sealed") databases.

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
  each. They're opened on first use, each behind one `Mutex<Database>`
  (`server/src/pool.rs`), as the app holds them, and closed after a while
  unused.
- **Long work** runs as `chess-jobs` jobs with progress and cancel: imports
  (then the position index), index builds, the lines of common positions,
  duplicates, compaction, backups and TWIC checks. A job records the
  database and the user who started it, and is kept in the store.
- **Metadata store**: SQLite (`server.sqlite`) for users, sessions, API
  tokens, databases, shares, transfers, invites, reset links, jobs, the
  audit log and settings. Games are never in it.
- **Housekeeping** (`server/src/housekeeping.rs`), once a minute: the audit
  log's retention, idle databases, scheduled backups and TWIC checks.
- **The admin UI** (Svelte 5, `server/admin/`) is built into `dist/` and
  embedded in the binary (`server/build.rs`); it fetches nothing from the
  network.

## The data directory

`CHESS_SERVER_DATA` (`/data` in the container, a volume):

| Path | What |
|---|---|
| `server.sqlite` (+ `-wal`, `-shm`) | Users (Argon2id password hashes, e-mail addresses), sessions, API tokens, invites and reset links (their SHA-256 only), databases (name, owner, visibility, quota, id epoch), shares and transfers, jobs, the audit log, settings and the TWIC issues taken. Schema version 2 (from 1, migrated at start) |
| `dbs/<id>.chessdb/` | The databases. `<id>` is 32 random hex digits; the API never takes a path, only an id, checked to be exactly that before a path is built |
| `import/` | Files the admin imports from: PGN, or ChessBase databases (the `.cbh` with its `.cbg`, `.cbp`, `.cbt`, `.cbc`) |
| `tmp/` | Uploads being received or imported; one stays until its import ends, also across a restart (the import goes on), and the others are deleted at start |

**It must be a local disk**, not NFS or SMB: databases are locked with
`File::try_lock` and rely on `fsync` for crash safety, which network file
systems don't reliably give. One server process per data directory (each
open database is locked).

**Backups**: see [Backups](#backups): the server makes them while it runs,
into a folder of its own (another volume, ideally).

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
| `CHESS_SERVER_OPEN_SIGNUP` | off | `1`: anyone may make an account without an invite |
| `CHESS_SERVER_SIGNUP_QUOTA_MB` | 1024 | The quota of an account made without an invite (`0`: none) |
| `CHESS_SERVER_SMTP_HOST` | none | An SMTP server for "Forgot password" e-mails (needs `CHESS_SERVER_PUBLIC_URL` for the link too) |
| `CHESS_SERVER_SMTP_PORT` | 587 (465 with `tls`) | |
| `CHESS_SERVER_SMTP_TLS` | `starttls` | `starttls`, `tls` (from the start), or `none` (only for a relay on the same machine or network) |
| `CHESS_SERVER_SMTP_USER`, `CHESS_SERVER_SMTP_PASSWORD` | none | Its login, if it needs one |
| `CHESS_SERVER_SMTP_FROM` | (needed with a host) | The sender, e.g. `Chess server <chess@example.com>` |
| `CHESS_SERVER_AUDIT_DAYS` | 365 | How long the audit log keeps entries |
| `CHESS_SERVER_TWIC_URL` | `https://theweekinchess.com` | Where TWIC issues come from (a stand-in, for tests) |
| `CHESS_SERVER_BACKUP_DIR` | none | Where backups go ([Backups](#backups)); another volume than the data, ideally |
| `CHESS_SERVER_BACKUP_EVERY_HOURS` | none | Back up on a schedule too |
| `CHESS_SERVER_BACKUP_KEEP` | 7 | Backups kept |
| `CHESS_SERVER_METRICS_TOKEN` | none | Turns `/metrics` on, for requests bearing it (16 characters or more) |
| `CHESS_SERVER_IDLE_CLOSE_MINUTES` | 30 | Databases unused this long are closed (`0`: never) |
| `CHESS_SERVER_RATE_LIMIT_PER_MINUTE` | 1200 | Requests a user may make a minute (a quarter of it at once); `0` turns it off |
| `CHESS_SERVER_RATE_LIMIT_ANON_PER_MINUTE` | 300 | The same per client address, for requests without a login; `0` turns it off |
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
  always an enabled admin.
- **Deleting** a user (the admin, confirmed with the username) deletes the
  databases they own with them: the admin UI lists them by name and size
  first. Their sessions, API tokens, shares, offers and reset links go,
  and their jobs are cancelled. Their databases can't go to the admin
  instead, which would let the admin read what was private to them. The
  admin can't delete themselves (so an enabled admin always remains).
- A new password set by the admin ends the user's sessions (not their API
  tokens); changing one's own ends one's other sessions.
- **Reset links**: the admin can make a one-time link for a user to set a
  password the admin never sees (`POST /api/admin/users/{id}/reset-link`,
  24 hours; a newer one replaces it). Stored as a hash, like tokens.
  Using it ends the user's sessions.
- **E-mail and "Forgot password"** (optional): users can give an address
  (with their password; it's only for reset links). With SMTP set up (the
  `CHESS_SERVER_SMTP_*` settings) and a public URL, "Forgot your
  password?" e-mails a link that works for an hour. The request always
  gets the same answer at once, the e-mail being sent afterwards, so it
  never tells whether an account or an address exists; requests are
  throttled per client address and per name asked, as logins are (each
  one counts).
- **Invites**: the admin makes single-use links (7 days by default, 30 at
  most) for one account each, with the role and the quota they choose;
  stored as hashes. **Open sign-up** is off unless
  `CHESS_SERVER_OPEN_SIGNUP=1`, and an account made that way gets
  `CHESS_SERVER_SIGNUP_QUOTA_MB` (1024 by default). Sign-ups are
  throttled per client address (open ones all count; with an invite, the
  wrong ones). An account made either way is logged in.
- **Users who aren't admins** get their Account page in the web UI
  (password, e-mail address, API tokens) and the pages links lead to
  (invites, reset links, "Forgot password"). No game browsing: the web UI
  is the admin's, a players' UI is NET-09.

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
  a quota (other fields are refused).
- **Transferring ownership**: only the owner offers a database, to a named
  enabled user, who accepts or declines it through the API (so nobody can
  fill another's quota). Accepting is checked against the recipient's
  quota (507 when it doesn't fit). The old owner keeps no access: they get
  no share, and a private database becomes the new owner's alone; the
  visibility and the other shares stay as they were (the recipient sees
  the visibility in the offer). The admin can transfer only databases
  they own.
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
| `GET /api/me/account` | any | `{username, role, email, quotaBytes, usedBytes, emailResets}` |
| `POST /api/me/password` | any | `{currentPassword, newPassword}` |
| `PUT /api/me/email` | any | `{currentPassword, email}` (`null` removes it) |
| `GET /api/invites/{token}` | none | What an unused invite makes (`{role}`), else 400 `badInvite` |
| `POST /api/signup` | none | `{invite?, username, password, email?}`: an account, logged in (session cookie); without `invite` only with open sign-up |
| `POST /api/password-reset` | none | `{token, password}`: a new password with a reset link; the account's sessions end; 400 `badReset` for a wrong, used or expired link |
| `POST /api/password-reset/request` | none | `{login}` (username or address): e-mails a reset link if the account has an address; always `202` |
| `GET /api/transfers` | any | `{incoming, outgoing}`: databases offered to one and by one |
| `POST /api/transfers/{id}/accept`, `…/decline` | the recipient | Takes the database (checked against one's quota), or turns it down; anyone else gets 404 |
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
| `PUT /api/databases/{id}/transfer` | owner | `{username}`: offers the database to that user (replacing an earlier offer) |
| `DELETE /api/databases/{id}/transfer` | owner | Withdraws the offer |
| `GET /api/databases/{id}/games` | read | A page of the game list (below) |
| `GET /api/databases/{id}/games/{n}` | read | One game: `row`, `headers`, `pgn`, `startFen`, `notation` (the move tree with comments, NAGs, clocks and evals), `opening` |
| `GET /api/databases/{id}/export.pgn` | read | The games the game list shows for the same criteria (all but the deleted ones by default; `fen=`, `sort=`… as below), in its order, as one PGN file (`Content-Disposition: attachment`). Streamed as it's written, never held in memory; `202 {"kind":"job"}` like the list while the position index is far behind |
| `GET /api/databases/{id}/tree?fen=` | read | The position tree with Hot, best players and top games (`chess_db::reference_overview`); the start when no FEN |
| `GET /api/databases/{id}/lines?fen=` | read | The main line and lines (`reference_lines`): `{"kind":"ready","lines":…}`, or from 50,000 games `{"kind":"job","job":N,"games":…}`: ask again when the job is done |
| `POST /api/databases/{id}/import?filename=` | write | The request body is a PGN file (at most `CHESS_SERVER_MAX_UPLOAD_MB`) → `202 {"job": N}` |
| `POST /api/databases/{id}/import-file` | write, admin | `{path}`: a `.pgn` or `.cbh` under `import/` → `202 {"job": N}` |
| `POST /api/databases/{id}/deleted` | write | `{games: [ids], deleted: true\|false, idEpoch?}`: marks games as deleted or restores them (DB-06), at most 100,000 at once → `{changed}` |
| `POST /api/databases/{id}/duplicates` | read | `{fuzzy?}`: looks for duplicate games (DB-08) in a job; its `duplicates` report (for who started it and the owner) counts the groups and copies |
| `GET /api/databases/{id}/duplicates/{job}` | read, the report's | `?offset=&limit=`: the groups found, `{keep, remove: [ids], exact}` each |
| `POST /api/databases/{id}/duplicates/{job}/delete` | write, the report's | Marks the copies as deleted, after giving each kept game what it lacked of them (as the app's Find duplicates does) → `{marked, filled, left}`. Refused (409) once the database changed since the search |
| `POST /api/databases/{id}/compact` | owner | Compacts the database in a job (DB-06): games marked as deleted go for good and **the games are renumbered**; then the position index is built again. Refused (409) while another write job runs on it |
| `GET /api/search?db=A&db=B…` | read on each | SRCH-13: the games of several databases in one list, with the game list's criteria (`fen=` too), sort and paging; `all=true` instead of `db=` searches every database one may read (at most 64). Naming a database one may not read answers 404 for the whole search, as every route does for it. `{"kind":"ready", total, offset, databases: [{id, name, total}], rows: [{database, databaseName, row}]}`, or `{"kind":"job", job, database}` while a database's position index is far behind |
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
`annotatedOnly`; `fen` for the games reaching a position (SRCH-03, through
the position index, so within its first 50 plies; with any of the other
criteria); and `deleted` (`hide`, the default, `show`, or `only` the games
marked as deleted). The answer is `{"kind": "ready", total, offset, rows}`.
With `fen` while the position index is far behind (over 20,000 games not
indexed, as during a big import), it's `{"kind": "job", "job": N}`: the job
bringing the index up to date, as `/lines` answers; ask again when it's
done.

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
| `DELETE /api/admin/users/{id}?confirm=USERNAME` | Deletes the user and the databases they own |
| `GET /api/admin/users/{id}/databases` | What deleting them deletes: `[{id, name, games, sizeBytes}]` |
| `POST /api/admin/users/{id}/password` | `{password}` |
| `POST /api/admin/users/{id}/reset-link` | A one-time reset link: `{token, url, expiresAt}`, shown once |
| `GET /api/admin/invites`, `POST /api/admin/invites` | Invites (never their tokens); `{role?, quotaBytes?, days?}` makes one: `{id, token, url, expiresAt}`, shown once |
| `DELETE /api/admin/invites/{id}` | Withdraws an invite |
| `GET /api/admin/databases` | Every database, as metadata |
| `PATCH /api/admin/databases/{id}` | `{quotaBytes}` only |
| `DELETE /api/admin/databases/{id}?confirm=NAME` | |
| `GET /api/admin/jobs`, `POST /api/admin/jobs/{n}/cancel` | |
| `GET /api/admin/disk` | Free space, and what the databases, the store, uploads and the import folder take |
| `GET /api/admin/import-files` | The `.pgn` and `.cbh` files in `import/` |
| `GET /api/admin/twic`, `PUT /api/admin/twic` | The TWIC updater (NET-06): `{database, databaseName, everyDays, lastCheck, lastIssue, issues}`; `PUT` takes `{database, everyDays}` (`database`: one the admin owns, or `null` for off) |
| `POST /api/admin/twic/check` | Checks for new TWIC issues now, as a job |
| `GET /api/admin/backups`, `POST /api/admin/backups` | The backups (names, dates, sizes; never their content), and making one now as a job |
| `GET /api/admin/audit` | The audit log, newest first: `?action=` (an action, or a group such as `login`), `who=` (a username, as who did it or to whom), `from=`, `to=` (Unix seconds), `offset=`, `limit=` (100, at most 500) |

## Backups

With `CHESS_SERVER_BACKUP_DIR` set, the admin's Backups page (or `POST
/api/admin/backups`) makes a backup while the server runs, as a job, and
`CHESS_SERVER_BACKUP_EVERY_HOURS` makes one on a schedule; the newest
`CHESS_SERVER_BACKUP_KEEP` (7) are kept. **Put the folder on another volume
than the data directory** (another disk, a mounted backup share), so a lost
disk doesn't take both; the compose example mounts one.

A backup is a folder `chess-server-<date>-<time>/` (written as `.partial`
and renamed once complete) holding:

- `server.sqlite`: the metadata store (users, password hashes, API tokens'
  hashes, databases, shares, invites, jobs, the audit log), copied with
  SQLite's online backup API, without the sessions;
- `dbs/<id>.chessdb/`: every database. Each is copied **holding its
  writer slot**, so no import, index build or compaction writes to it
  meanwhile and no batch is committing (a backup waits for one running to
  end); `games.idx`, with the committed count, and the name tables are
  copied first and the append-only `games.blob` and position index after,
  so a copy never refers to data it lacks (`docs/file-format.md`, Crash
  safety). Requests keep reading it during the copy.

Not in it: `import/` (the files to import from: keep them elsewhere) and
uploads in progress.

**Backups are never downloadable through the API**: they hold every user's
databases, private ones too, which the admin can't read on the server. The
API lists them (name, date, size, databases) and nothing more; they're
reached on the server's disk, by whoever runs it, like the data directory.

**Restoring** one (tested: `server/tests/running.rs` restores into a fresh
data directory and finds the same users, databases, games and tokens):

```sh
# 1. Stop the server (docker compose -f compose.example.yaml stop chess-server).
# 2. Set the data directory's store and databases aside:
cd /path/to/data && mkdir ../before-restore && mv server.sqlite* dbs ../before-restore/
# 3. Copy the backup in:
cp /path/to/backups/chess-server-20261111-030000/server.sqlite .
cp -r /path/to/backups/chess-server-20261111-030000/dbs .
# 4. Start the server.
```

Users, password hashes, API tokens, databases, shares and the audit log
are as they were at the backup; everyone logs in again (sessions aren't
kept), and jobs that ran then are marked interrupted.

## TWIC updates (NET-06)

The admin's TWIC page (or `PUT /api/admin/twic`) picks a database the
admin owns for [The Week in Chess](https://theweekinchess.com/twic)'s
weekly issues, and how often to look (every 7 days by default; 0: only when
asked). A check is a job (`twic`, or `POST /api/admin/twic/check` now): it
reads TWIC's page for the newest issue (`chess_net::twic`, one request at a
time, with a user agent saying what it is) and imports each issue out
since the last one taken, at most 10 a check. The first check takes the
newest issue only, not the years before it. Each issue is noted as taken
before its games go in, so **none is imported twice**, even across a
crash (one cut off then is shown as unfinished, not taken again); its zip
is unpacked into `tmp/` and imported by an ordinary import job, which goes
on after a restart like any other. Issues are free to use: a server-wide
database suits them. If the database stops being the admin's (deleted,
transferred), checks stop until another is chosen.

## The audit log

Security-relevant events go to the metadata store, each with who did it
(none for a failed login), what it was done to, when and the client's
address: logins and failed logins; users made (by the admin, an invite,
open sign-up or the setup token), their role changed, disabled, enabled,
given a quota, deleted; passwords changed, set by the admin, reset links
made, used or e-mailed; invites made and withdrawn; API tokens made and
deleted; databases made, renamed, deleted, made private, shared or
server-wide, given a quota; shares; transfers offered, withdrawn,
accepted, declined; imports started, exports, games deleted or restored,
duplicates deleted, compactions, backups, TWIC updates; jobs cancelled.
The admin's Audit log page shows them, with filters. Entries are kept
`CHESS_SERVER_AUDIT_DAYS` (365 by default).

**It never holds game content, search criteria, positions or file names**
(of uploads or imports): an entry says an import of an upload started into
a database, not from what file; an export, not of which games. Database
names and usernames are in it, as in the admin's other pages.

## Metrics

With `CHESS_SERVER_METRICS_TOKEN` set (16 characters or more), `GET
/metrics` answers in Prometheus' text format to a request with
`Authorization: Bearer <the token>` (401 otherwise; 404 when no token is
set). It's outside `/api`, not rate-limited, and Caddy can keep it from
the internet (or a scraper can reach the server's port directly):

| Metric | |
|---|---|
| `chess_server_http_requests_total{route, method, status}` | API requests, by route template (`/api/databases/{db}/games`, never a path with an id) |
| `chess_server_http_request_duration_seconds{route, method}` | Their latency (a histogram, 5 ms to 30 s) |
| `chess_server_open_databases` | Databases open now |
| `chess_server_jobs_running{kind}` | Jobs running, by kind |
| `chess_server_metadata_store_bytes`, `chess_server_databases_bytes` | Sizes on disk |
| `chess_server_games`, `chess_server_databases`, `chess_server_users` | How many there are |

No label holds a database name or id, or a username. Written by hand: the
format is a few lines of text, not worth a crate.

## Keeping a database in order

- **Deleting games** marks them as deleted (DB-06): they stay in the
  database, restorable, until it's compacted. The game list, the export
  and searches leave them out unless asked (`deleted=show`, or `only`).
- **Duplicates** (DB-08) are found as in the app (`docs/stack.md`,
  Duplicate games): exact copies, or fuzzy ones too. The groups found are
  kept in memory with the job (a restart forgets them: find them again),
  and deleting the copies is refused once the database changed since.
- **Compaction changes game ids**, as in the app: games after a deleted
  one move down. Each database has an `idEpoch` (in its info) that every
  compaction bumps; requests that take game ids (`deleted`) can send the
  epoch they read the ids at, and are refused (409, `staleIds`) after a
  compaction. Nothing syncs with the server yet, so nothing else holds
  ids; permanent game ids are a later feature. While a compaction runs,
  the database answers `503 busy` with `Retry-After` (it holds the
  database for the whole run); its search, sort and lines caches go with
  the generation, and the position index is built again after it.

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
- **Jobs are kept in the metadata store** (the newest 1,000 finished
  ones, with their reports), so they survive a restart; their ids go on
  from the highest kept. A stop (a shutdown, a crash, `kill -9`) leaves the
  jobs it interrupted marked so at the next start, except imports: **an
  import goes on by itself** from what it committed. Before each batch it
  notes where the file and the database will be once the batch is in; at
  start the database's game count says which of the last two notes made it,
  so no game is imported twice and none is left out (tested by killing the
  server mid-import, `server/tests/restart.rs`). Its upload stays in `tmp/`
  until the import ends or is discarded (an import that can't go on: its
  database or file gone); other files there are deleted at start. A
  resumed import is the same job, which who started it, the database's
  owner or the admin (jobs view) can cancel. A search for duplicates' groups
  are kept in memory only: find them again after a restart.

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
  (8 bytes a game) once a tree was asked for. **Idle databases close**: one
  no request or job used for `CHESS_SERVER_IDLE_CLOSE_MINUTES` (30) is
  closed (its files, its lock and its memory go), never while a request or
  a job has it, and the next request opens it again (half a second per
  million games).
- One process; no horizontal scaling. Data on a local disk.
- **Rate limits**: a token bucket in memory per user (any session or API
  token of theirs) and per client address for requests without a login (and
  for credentials that don't work). A bucket holds a quarter of a minute's
  requests (at least 10) and refills at the per-minute rate; an empty one
  answers `429` with `Retry-After`. Generous by default (1,200 a minute per
  user, 300 per address), to stop a runaway script or a flood rather than a
  person; `/api/health` isn't limited. Behind a proxy, set
  `CHESS_SERVER_TRUST_PROXY` so the addresses are the clients', not the
  proxy's.
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

The same machine and cap, Nov 11 2026, for what came after phase 1's
first cut. "Cold" is the first request after a start, which opens the
database too; "both" is `/api/search` over the two databases (SRCH-13).

| | Lichess 1M | Mega 2019 | Both |
|---|---|---|---|
| Folder import (then the index) | 61.0 s | 473.6 s | |
| First page by White, cold | 0.65 s | 0.89 s | 1.27 s (1.4× the slower) |
| First page by date, cold | 0.50 s | 0.63 s | 1.14 s (1.8×) |
| `text=magnus` by date, cold | 0.78 s | 0.73 s | 1.29 s (1.7×), 9,574 games |
| Any later page (offset 50, 4,000,000, the last of 8,622,531) | | | 1 to 14 ms |
| Games reaching the Ruy Lopez after 3...a6 (`fen=`), by date | | 0.15 s, 208,594 games | 0.63 s by White |
| The same with `white=carlsen` | | 0.14 s, 118 games | |
| PGN export, all games | 91 s, 2,185 MB; server memory 607 → at most 615 MB | 604 s, 5,172 MB; 615 → at most 683 MB | |
| Backup (store and both databases, 8.1 GB) | | | 1.1 s: btrfs clones the files (`copy_file_range`); elsewhere it copies them |
| Compaction with 1,000 games deleted, then the index | 11.2 s | | |
| Server memory | | | 402 MB after both imports (peak 548 MB); 484 MB after searching both; 893 MB at the end (peak 1.0 GB) |

A search across databases makes each database's list on a thread of its
own, so its first page takes about the slowest database's time plus the
other's sort; later pages are found by a selection over the cached lists,
in milliseconds at any depth.

## Licence

The server is part of the workspace and under its GPL-3.0-or-later. Whether
it should be AGPL-3.0 (so that people who run a modified server for others
must offer them its source) is an open question for the project.

## What's next

- **Phase 2**: the desktop app as a client (open a server database like a
  local one, through an API token), with its DTOs and the server's shared
  in one crate; sync of a user's own databases; an offline copy per
  database.
- Editing games on the server (write access beyond imports, deleting and
  compaction).
- Permanent game ids, so ids a client keeps survive a compaction (an
  `idEpoch` only says they changed).
- Parallel queries on one database (see Limits): several readers per
  database instead of one mutex.
- A web UI for players (NET-09); the web UI is the admin's, plus each
  user's Account page.
- An encrypted "sealed" visibility, so a database is unreadable on the
  server's disk.
- Material and pattern search (SRCH-05/06), on the server as in the app.
- Whether the server should be AGPL-3.0 (see Licence).
