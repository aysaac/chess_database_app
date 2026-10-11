# IO-03: Reading ChessBase `.cbh` (Mega Database)

Feature **IO-03** in `docs/feature-catalog.md` (P3): read ChessBase `.cbh`
databases so the app can import the Mega Database. This doc records where the
real files live, how to get them onto Linux, and the plan for the reader.

## 1. Where the data lives

The ChessBase Mega Database lives inside a **Windows 11 (tiny11) VM that runs
on this Linux machine** as a Docker container (`dockurr/windows`). It is not a
separate machine.

| What | Value |
|---|---|
| Docker container | `omarchy-windows` (image `dockurr/windows`) |
| Hypervisor | QEMU + KVM (spawned by the container) |
| Windows login | user `docker`, password `admin` |
| Database path (in Windows) | `C:\Users\docker\Documents\ChessBase\Bases` |
| Web manager | http://localhost:8006 |
| VNC | `localhost:5900` |
| RDP | `localhost:3389` |
| Windows disk image (host) | `/home/aysaac/.windows/data.img` (64 GB raw) |

### Known paths inside the VM (verified by reading the NTFS MFT)

| What | Path in Windows |
|---|---|
| **ChessBase 16 (the DB program)** | `C:\Program Files\ChessBase\CBase16\CBase16.exe` |
| Fritz 20 (playing/analysis GUI) | `C:\Program Files\ChessBase\ChessProgram20\ChessProgram20.exe` |
| **Mega Database (the big one)** | `C:\Users\docker\Documents\ChessBase\Bases\Mega Database 2019.cbh` |
| Small test databases | `C:\Users\docker\Documents\ChessBase\Testsets\Marathon.cbh`, `...\Testsets\nunn.cbh`, `...\Training\Openings.cbh`, `...\Training\Endgames.cbh` |
| Opening book | `C:\Users\docker\Documents\ChessBase\Books\Fritz 20.ctg` |

- The Mega Database is the **2019** edition, not the latest.
- `Testsets\Marathon.cbh` and `nunn.cbh` are small — ideal committed fixtures.
- Start-menu shortcut is `ChessBase 16 64Bit.lnk` (confirms CBase16.exe is 64-bit).

> ⚠️ Do **not** mount `/home/aysaac/.windows/data.img` on the host while the
> VM is running. It is the live NTFS image. Use the shared folder below.

## 2. Getting files onto Linux (shared folder — the safe path)

The container already runs a Samba server shared with the guest:

- Host `/home/aysaac/Windows` → container `/shared` → Windows share **`\\host.lan\Data`**
- Guest access, read/write. (`host.lan` resolves inside the VM to the Samba
  server at `172.30.0.1`; share name is `Data`, from the container's `smb.conf`.)
- This folder was already used to move `CBProductInstallerSetup.msi` into the
  VM, so it is known to work.

### Step A — in Windows (one command, human or via RDP)

RDP to `localhost:3389` (`docker` / `admin`), open PowerShell, and run:

```powershell
robocopy "C:\Users\docker\Documents\ChessBase\Bases" "\\host.lan\Data\bases" /E /COPY:DAT /R:1 /W:1
```

- robocopy exit codes **0–7 mean success** (1 = files copied). 8+ = error.
- For the Mega Database, copy the `.cbh` plus its companions and the `.ctg`
  book; media can wait.

If Windows refuses the share with *"your organization's security policies block
unauthenticated guest access"*, run as administrator and reboot the VM:

```powershell
reg add HKLM\SYSTEM\CurrentControlSet\Services\LanmanWorkstation\Parameters /v AllowInsecureGuestAuth /t REG_DWORD /d 1 /f
```

### Step B — on Linux

The files appear in `/home/aysaac/Windows/bases/`. Verify:

```sh
ls -lah /home/aysaac/Windows/bases/
file   /home/aysaac/Windows/bases/*.cbh
```

## 3. The `.cbh` format

This is the classic ChessBase 6-era format (format version byte 1), the one the
real Mega Database 2019 files at `/home/aysaac/Windows/bases/` use. It is *not*
the newer `.2cbh` family ChessBase 17+ writes (seen in that same folder as
`Database2025.2cbh` and friends) — that's a different, undocumented-here format;
revisit if a later feature needs it.

A database is a set of files sharing one base name. The mandatory ones are
`.cbh`, `.cbg`, `.cba`, `.cbp`, `.cbt`, `.cbc` and `.cbs`; the rest (`.cbj`,
`.cbtt`, `.cbe`, `.cbl`, `.flags`, `.cbm`, the `.cit`/`.cib` family, `.cbb`,
`.cbgi`) were added by later ChessBase versions, can be rebuilt from the others,
or both, and may be missing or stale.

| Extension | Contents |
|---|---|
| `.cbh` | game headers: one fixed-size record per game or guiding text |
| `.cbj` | extended game headers added later: teams, material, ratings, timestamps |
| `.cbg` | the moves, and the body of each guiding text |
| `.cba` | annotations: comments, symbols, arrows, evaluations |
| `.cbp` / `.cbt` / `.cbtt` / `.cbc` / `.cbs` / `.cbe` / `.cbl` | entities: players, tournaments (+ extra), annotators, sources, teams, game tags |
| `.flags` | which games are Top Games |
| `.cbm` | manifest of media a guiding text uses |
| `.cit`/`.cib`/`.cit2`/`.cib2` | derived: games referring to each entity/game-tag |
| `.cbb` | derived: events that happened in each game |
| `.cbgi` | derived: offset of each game's moves in `.cbg` |

`.ctg` is a separate proprietary opening book (IO-06) and `.cbv` a compressed
archive of a whole database (IO-05) — out of scope here.

Games and guiding texts (free-standing text documents filed among the games,
e.g. an "About this database" page) share one 1-based **game id** space, the
position of their record in `.cbh`/`.cbj`. Entities (players, tournaments, …)
have their own 0-based **entity id** per type, the position of their record in
their file.

**Byte order is not uniform** — it was the single easiest thing to get wrong
when cross-checking against the real files below, so get it from this table,
never assume:

| File(s) | Byte order |
|---|---|
| `.cbh` | big-endian |
| `.cbj` | header little-endian, records big-endian |
| `.cbg`, `.cba` | big-endian (a couple of fields inside guiding-text records are little-endian, noted below) |
| `.flags`, `.cbb` | big-endian |
| `.cbp` `.cbt` `.cbc` `.cbs` `.cbe` `.cbl` `.cit` `.cib` `.cit2` `.cib2` `.cbgi` `.cbm` | little-endian |
| `.cbtt` | little-endian except the coordinates and the tournament end date |

A fixed-size string field is ISO-8859-1 (Latin-1), zero-terminated when shorter
than the field; bytes after the terminator are leftovers with no meaning. Free
text (comments, guiding-text bodies) has no declared encoding — try UTF-8, fall
back to Windows-1252.

A packed **date** is `day (bits 0-4, 1-31) | month (bits 5-8, 1-12) | year (bits
9-20)`; a part that's 0 is unset, so 0 is a fully unknown date and a year alone
is valid. `.cbh` packs a date into 3 bytes; everywhere else it's a 4-byte int
with the upper bits zero.

### 3.1 `.cbh` — game headers

A 46-byte file header, then one 46-byte record per game/text in id order, no
gaps: record *n* (1-based) starts at byte `46·n`. Record count is
`(file size − 46) / 46`.

File header (all big-endian):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 1 | | unknown, always 0 |
| 0x01 | 2 | ushort | bytes of this header in use: 44 (36 in older databases) |
| 0x03 | 2 | ushort | record size, 46 |
| 0x05 | 1 | byte | format version, 1 |
| 0x06 | 4 | int | next game id |
| 0x0a | 2 | | unknown, always 0 |
| 0x0c | 4 | int | next embedded-sound id |
| 0x10 | 4 | int | next embedded-picture id |
| 0x14 | 4 | int | next embedded-video id |
| 0x18 | 16 | | unknown, always 0 |
| 0x28 | 4 | int | next game id again (0 if the byte at 0x01 is 36) |
| 0x2c | 2 | | unknown, always 0 |

Verified byte-for-byte against `Mega Database 2019.cbh`: bytes 0–0x2d decode to
header-in-use 44, record size 46, version 1, next game id `0x745a01` =
7,625,217, which is exactly `(350759982 − 46) / 46 + 1`; the embedded-picture
counter is 0x111 = 273 and the field at 0x28 repeats the same next-game-id.

The first byte of every record is a type/flags byte:

| Bit | Meaning |
|---|---|
| 0 | set in every real record; an all-zero record is past the last game |
| 1 | this record is a guiding text, not a game |
| 7 | the game is marked deleted (record and id are kept) |

Game record (when bit 1 is clear):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 1 | byte | type/flags, above |
| 0x01 | 4 | uint | offset of the moves in `.cbg` |
| 0x05 | 4 | uint | offset of the annotations in `.cba`, 0 if none |
| 0x09 | 3 | uint24 | white player (entity id) |
| 0x0c | 3 | uint24 | black player |
| 0x0f | 3 | uint24 | tournament |
| 0x12 | 3 | uint24 | annotator |
| 0x15 | 3 | uint24 | source |
| 0x18 | 3 | uint24 | played date, packed (§3) |
| 0x1b | 1 | byte | result: 0 `0-1`, 1 `½-½`, 2 `1-0`, 3 line (unfinished), 4 `0-1` forfeit, 5 `½-½` forfeit, 6 `1-0` forfeit, 7 `0-0` both lost |
| 0x1c | 1 | byte | NAG evaluation, only meaningful when result is 3 (line) |
| 0x1d | 1 | byte | round, 0 if unset |
| 0x1e | 1 | byte | subround, 0 if unset |
| 0x1f | 2 | ushort | white elo, 0 if none |
| 0x21 | 2 | ushort | black elo |
| 0x23 | 2 | ushort | ECO or Chess960 start position, below |
| 0x25 | 2 | ushort | medals bitmask |
| 0x27 | 4 | uint | flags bitmask, below |
| 0x2b | 2 | ushort | annotation-magnitude bitmask |
| 0x2d | 1 | byte | full moves (not plies) in the main line, 0 for none; a longer game holds 255 or, from some writers, the count modulo 256 (Mega Database 2019 has both: a 279-move game says 23) |

An entity reference is never −1: a field the user left blank points at a
placeholder entity whose text is empty, created on demand (never entity id 0
specifically).

ECO: `value / 128 − 1` is the ECO number (0-99 → A00-A99, 100-199 → B00-B99, …
to E99); `value % 128` is the sub-code; 0 means no ECO. `value ≥ 64576`
(`65536 − 960`) is instead a **Chess960 start position**, `value − 64576`
(0-959).

Flags bitmask (bits not listed are unknown/unused):

| Bit | Meaning | Bit | Meaning |
|---|---|---|---|
| 0x01 | starts from a set-up position | 0x80000 | game quotation |
| 0x02 | has variations | 0x100000 | pawn structure |
| 0x04 | has commentary | 0x200000 | piece path |
| 0x08 | has symbols | 0x400000 | white clock |
| 0x10 | coloured squares | 0x800000 | black clock |
| 0x20 | arrows | 0x1000000 | critical position |
| 0x80 | time spent | 0x4000000 | annotation type `1a` (not always set when present) |
| 0x100 | annotation type `08` | 0x8000000 | unorthodox (Chess960) |
| 0x200 | training | 0x10000000 | web link |
| 0x10000 | embedded audio | | |
| 0x20000 | embedded picture | | |
| 0x40000 | embedded video | | |

These flags are informational (set when the game is saved, from what it
contains) — the import pipeline shouldn't rely on them over actually parsing
`.cbg`/`.cba`.

Medals is a bitmask, one bit per entry from bit 0: best game, decided
tournament, model game, novelty, pawn structure, strategy, tactics, with
attack, defence, sacrifice, material, piece play, endgame, tactical blunder,
strategical blunder, user-defined.

Guiding-text record (bit 1 set):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 1 | byte | type/flags, bit 1 set |
| 0x01 | 4 | uint | offset of the text in `.cbg` |
| 0x05 | 2 | | unknown, always 0 |
| 0x07 | 3 | uint24 | tournament |
| 0x0a | 3 | uint24 | source |
| 0x0d | 3 | uint24 | annotator (the text's author) |
| 0x10 | 1 | byte | round, 0 if unset |
| 0x11 | 1 | byte | subround, 0 if unset |
| 0x12 | 4 | uint | flags (only the audio/picture/video bits apply) |
| 0x16 | 24 | | unknown, always 0 |

Verified: game id 1 and id 2 in the real Mega Database are both guiding texts
(type byte `0x03`); game 1's text offset is 26, exactly the `.cbg` header size,
i.e. the very first thing after that header; its flags are `0x20000`
(embedded picture) with the preceding and trailing unknown spans all zero, and
its tournament/source/annotator entity ids are all 0 — all exactly as this
table predicts.

### 3.2 `.cbg` — moves (and guiding-text bodies)

A 26-byte file header (10 in old databases), then one record per game/text in
id order, reached only through the offset in that game's `.cbh`/`.cbj`
record — there's no index inside `.cbg` itself. Records sit back to back; one
that shrinks on a re-save leaves a **hole** behind it (the header's unused-byte
count tracks this), and how a growing record is relocated is not documented
anywhere public.

File header:

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 2 | ushort | header size, 26 (10 in old databases) |
| 0x02 | 4 | uint | file size |
| 0x06 | 4 | uint | unused bytes across all holes (a bit below the true sum) |
| 0x0a | 8 | long | file size again (absent when header is 10 bytes; not always current) |
| 0x12 | 8 | long | unused bytes again (ditto) |

Verified: the real `.cbg` header size field is 26 and its offset-0x02 size
matches the game-1 guiding-text offset of 26 found via `.cbh` (the text
immediately follows the header). `.cba` shares this same header shape.

Game/text record:

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 1 | byte | flags, below |
| 0x01 | 3 | uint24 | size of this record in bytes, header included |
| 0x04 | 28 | | start position, only present if flag bit 6 is set |
| … | | | the moves (or guiding-text body), to the end of the record |

Record flags: bit 7 is always set for a guiding text (meaning in a game is not
documented); bit 6 means the game doesn't start from the normal position (the
28-byte start position follows); bits 0-5 are the **encoding mode**.

When bit 6 is set, the 28-byte start position:

| Offset | Size | Field |
|---|---|---|
| 0x00 | 1 | always 1 |
| 0x01 | 1 | bits 0-3 en passant file (0 none, 1-8 = a-h); bit 4 side to move (0 white, 1 black). The en passant square is behind the pawn that just moved: rank 3 with Black to move, rank 6 with White. A few games name a file no pawn can have just moved on; the position stands without it |
| 0x02 | 1 | castling rights: bit 0 white O-O-O, bit 1 white O-O, bit 2 black O-O-O, bit 3 black O-O. ChessBase often stores 0x0f whatever the board (all 210 games of `Marathon`), so a right only counts while its king and rook are on their home squares |
| 0x03 | 1 | move number (0 and 1 both mean move 1) |
| 0x04 | 24 | the board: 192 bits, MSB-first, squares in order a1,a2,…a8,b1,…h8. Empty = one 0 bit. Occupied = 1 bit + colour bit (0 white) + 3-bit piece code (king 001, queen 010, knight 011, bishop 100, rook 101, pawn 110), padded with 0s past the last (≤32) piece |

A Chess960 game (encoding mode 10 or 11, below) always has a start position,
with 8 extra bytes after the board: white-king square, black-king square,
white kingside-rook square, white queenside-rook square, black kingside-rook
square, black queenside-rook square, then a ushort Chess960 start-position
number (0-959).

**Encoding modes** (low 6 bits of the flags byte): mode 0 is the overwhelming
majority of games (ordinary chess, "compact" encoder, "pre" modifier, default
piece order); mode 10 is Chess960. The full table of 20 modes (compact/simple ×
pre/post × default/reverse piece order, for ordinary chess, losing chess and
Chess960) and modes 12-19/63 (undocumented variants) is in the upstream
reference below — not reproduced here since the reader only needs to special-
case mode 10/11 (Chess960) and treat anything else unexpected as "unsupported,
surface a warning" rather than guess.

Every move byte is first **translated** through a 256-entry permutation table
that's specific to the encoding mode, combined with a running count *n* of
moves decoded so far in the game (every move counts, variations included,
never reset at the end of a variation; a null move and a two-byte move both
count once; the 254/255 variation markers don't count):

- "pre" modifier: `value = table[(byte − n) mod 256]`
- "post" modifier: `value = (table[byte] − n) mod 256`

Every byte of the stream goes through this translation with the current
*n*: move codes, the 235 escape and both bytes after it, and the 236/254/255
markers too. Only the *counting* skips the markers. Both bytes of a two-byte
move use the same *n*.

**Where the table comes from (decided):** derive it from the user's own
licensed ChessBase install, never vendor a third party's copy (morphy ships
`keys/key_*.bin` extracted from `CBase13.exe`; don't copy those). Mode 0's
table is in `CBase16.exe` (ChessBase 16, 64-bit) at file offset
`0x1416480`. `examples/extract_cb_key.rs` finds it: it scans every
256-byte window of the install for permutations (1,515 of them) and keeps
the one that decodes every game of the four databases bundled with
ChessBase to the main-line length `.cbh` declares (all 394 games; no
other candidate comes close). The windows next to it are permutations
too, presumably the other modes' tables, which we don't need. It's built
in as `MODE0_TABLE` in `cb/moves.rs`. Facts from the data agree with it
independently: the last byte of almost every game in the Mega Database is
12 after its last move (`T[12] = 255`), and the four most common first
bytes, 255, 11, 254 and 218 (46%, 29%, 7% and 6%), are 1.e4, 1.d4, 1.Nf3
and 1.c4. A first scan (2026-10-03) seemed to find nothing only because
its check compared plies with `.cbh`'s count of full moves.

After translation, the **compact encoder** (mode 0 and most others) reads one
byte as a code naming a piece (by its rank among same-kind pieces of that
colour, numbered in board-scan order from the start position: a1,a2,…a8,
b1,…h8, or the mirror in "reverse" order) and a movement relative to its
current square as `(dx, dy)` taken **mod 8** (so e.g. `(+7,0)` from `c2`
wraps to `b2`):

| Code | Meaning |
|---|---|
| 0 | null move |
| 1-8 | king step, the 8 directions starting `(0,+1)` clockwise |
| 9 / 10 | king castles short / long |
| 11-38 | 1st queen: 7 up, 7 right, 7 up-right diag, 7 down-right diag |
| 39-52 / 53-66 | 1st / 2nd rook: 7 up, 7 right |
| 67-80 / 81-94 | 1st / 2nd bishop: 7 up-right, 7 down-right |
| 95-102 / 103-110 | 1st / 2nd knight: the 8 knight jumps |
| 111-142 | the 8 pawns, 4 codes each (one step, two steps, right capture, left capture — see below) |
| 143-170 / 171-198 | 2nd / 3rd queen |
| 199-212 / 213-226 / 227-234 | 3rd rook / bishop / knight |
| 235 | next 2 translated bytes are a full move, below |
| 236 | ignored |
| 237-253 | unused |
| 254 | remember this position (a branch point — more than one move follows) |
| 255 | pop back to the last remembered position, or end the game if none |

When a non-pawn piece is captured, every same-kind piece of that colour above
it in rank renumbers down by one (second bishop becomes first, etc.); a pawn
keeps its own number for the whole game regardless of file. A castling rook
keeps its number on its new square. A captured or promoted pawn's number is
left empty, never reassigned, and nothing answers to it afterwards: a
decoder that keeps the dead pawn's last square finds it there again when
another pawn later stands on that square, and loses track of the live one.
A promotion adds a piece with the next number of its kind. A 4th-or-later
piece of a kind (only reachable via promotion) can't be named this way and
must use the 2-byte form (code 235). En passant is written as an ordinary pawn
capture.

A pawn's 4 codes, in a-pawn-first order (forward = `(0,+1)` white /
`(0,−1)` black): one step, two steps, right capture (`(+1,+1)` white /
`(−1,−1)` black), left capture (`(−1,+1)` white / `(+1,−1)` black).

King castling (codes 9/10) implicitly moves the rook too, except in Chess960
where castling always uses the 2-byte form with from == to == the king's
landing square (`g1`/`g8` short, `c1`/`c8` long).

2-byte move (after code 235), both bytes translated and read as one big-endian
ushort: bits 0-5 from-square, bits 6-11 to-square, bits 12-13 promotion piece
(0 queen, 1 rook, 2 bishop, 3 knight).

Variations are written depth-first, main line first at each branch: a 254
precedes every sibling move but the last at a branch point, and a 255 closes
each line, so there's always exactly one more 255 than 254 and the game's last
byte is a 255. Our own `game/editor.rs` tree model maps onto this directly —
254 ≈ "push, about to replay a sibling", 255 ≈ "pop".

**What our reader does with a move it can't play** (`cb/moves.rs`,
`decode_mode0`): a null move (our game tree can't hold one) or a move that
isn't legal cuts its line there, as PGN import does. The rest of that line
and its sub-variations are skipped by walking their bytes (254/255 nesting,
235 taking two more bytes), still counting every move so the bytes after
them translate right; the next line is read as usual. The game is imported
with a warning such as "The main line stops at 23...-- (a null move), which
can't be imported yet." A stream that ends without its last 255 keeps what
was read. `.cba` annotations on skipped moves are dropped: the decoder
returns the node of each move in storage order (`DecodedGame::order`,
`None` for a skipped one), which is what `.cba` positions index. Only an
encoding mode other than 0 (Chess960 is 10/11) or a bad start position
skips a whole game.

The **simple** encoder (odd-numbered mode, mostly historical) instead writes
every move as one translated 2-byte big-endian value: bits 0-5 from, 6-11 to,
12-13 promotion (reverse order swaps from/to), bit 14 "a variation ends after
this move", bit 15 "a variation starts before this move"; `a1`→`a1` is a null
move, and there's no explicit end marker — the last move has bit 14 set.

A guiding text's record starts with the same flags(`0x80`)+uint24-size header
as a game, but everything from offset 0x04 on is **little-endian**, not
translated, and holds the formatted text rather than moves: a ushort text
format version, a ushort title count, that many `{language: ushort, length:
ushort, text}` titles (languages 0 English, 1 German, 2 French, 3 Spanish, 4
Italian, 5 Dutch, 6 Portuguese, 7 unspecified), then the content blocks in the
same per-language shape (version 3, what current ChessBase writes, uses HTML
content with a uint length followed by 4 trailing zero bytes; versions 1-2 use
plain text with ushort/uint lengths and some still-undocumented trailing
formatting data).

### 3.3 `.cba` — annotations

Same 26-byte header shape as `.cbg`. One record per game that *has*
annotations (a game with none has no record and a 0 offset in `.cbh`; guiding
texts never have one), reached via the offset in `.cbh`/`.cbj`.

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 3 | uint24 | game id |
| 0x03 | 4 | | constant `01 00 0e 0e` |
| 0x07 | 3 | uint24 | number of annotations + 1 |
| 0x0a | 4 | uint | size of this record in bytes, header included |
| 0x0e | … | | the annotations, back to back |

Each annotation:

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 3 | int24 | position: −1 is "before the first move / the game as a whole", otherwise the 0-based index of a move in `.cbg`'s depth-first storage order (main line first, then each variation after the move it branches from) |
| 0x03 | 1 | byte | annotation type |
| 0x04 | 2 | ushort | size of this annotation in bytes, this 6-byte header included |
| 0x06 | … | | type-specific data |

Annotations for one game are sorted ascending by position; ties keep a
meaningful relative order a reader should preserve. An annotation of an
unrecognized type can be skipped using its declared size — the reader doesn't
need every type to stay correct on ones it does understand.

Types actually needed for the Reference/Notation tabs (OPEN-14, UI-19) and
blunder check (ENG-02): `02`/`82` text after/before the move (byte: unknown,
usually 0; byte: language as a nation code, 0 = any; rest: the text, `0x9e`
marks a ChessBase diagram, shown as `{#}`). One place can hold the same
comment in several languages, one annotation each: every comment in the
`Openings` and `Endgames` databases comes in German (53), Spanish (43),
French (49) and Russian (121), and the Mega Database also has English
(42) and Dutch (103); 1.4% of its commented places have more than one.
Our reader keeps the text for any language plus one language of the rest,
English if it's there, else the first. Lines end in `\r\n`, with one at
the end, so it turns them into `\n` and trims; `03` symbols (1-3 NAG bytes: move
symbol, position evaluation, prefix — trailing zero bytes are omitted so the
annotation can be 1-3 bytes); `04` coloured squares / `05` arrows (lists of
`{colour, square}` / `{colour, from, to}` to the end of the annotation; squares
here are numbered from 1, i.e. a1=1, h8=64, unlike everywhere else in this
format; colours 2 green, 3 yellow, 4 red, others unknown); `21` computer
evaluation (3 little-endian shorts: centipawns-or-mate value, a flag 0
ordinary/1 mate, search depth); `26` evaluations (ushort count, then that many
`{byte kind: 0 ordinary/0xff none/1 mate, byte depth, short value}` entries, on
position −1, one per ply including the start position — this is what feeds an
eval graph without re-running the engine). Everything else (time spent, time
control, training, game quotations, video, web links, medals mirror, variation
colour) is real but lower priority; keep it round-tripping as opaque
`{type, size, bytes}` rather than parsing it on day one.

### 3.4 Entities — `.cbp` `.cbt` `.cbtt` `.cbc` `.cbs` `.cbe` `.cbl`

Each entity type (player, tournament, annotator, source, team, game tag) lives
in its own little-endian file: a header, then fixed-size records that double
as nodes of an AVL tree keeping the entities in sorted order (lookup by id is
direct indexing; lookup by name/title walks the tree). An entity's id is the
0-based position of its record.

File header (28 bytes, or 32 when the header has 4 extra bytes at the end):

| Offset | Size | Type | Field |
|---|---|---|---|
| 0x00 | 4 | int | record count, deleted included |
| 0x04 | 4 | int | tree root id (0 or −1 if empty) |
| 0x08 | 4 | int | constant 1,234,567,890 |
| 0x0c | 4 | int | size of one record's data (excludes the 9-byte tree-node prefix) |
| 0x10 | 4 | int | id of the first deleted record, −1 if none |
| 0x14 | 4 | int | entity count, deleted excluded |
| 0x18 | 4 | int | extra header bytes: 0 or 4 |
| 0x1c | 4 | | the extra bytes, if present; may hold leftovers |

Verified against the real `.cbp`: the magic at 0x08 reads exactly
1,234,567,890, the data size at 0x0c is 58 (matching the player record below),
and the header has the 4 extra bytes (so is 32 bytes total); record 0's tree
node and its last-name string decode as expected.

Record: a 9-byte tree node (`int` left child id, `int` right child id, `byte`
balance factor = right height − left height, each −1/0/1) followed by the
type's data, below. A deleted record isn't unlinked — its left child is set to
the sentinel **−999** and its right child instead chains to the next deleted
record (−1 ends the chain; the header's offset-0x10 names the first), and it's
excluded from both the tree and the entity count.

Every type's data ends with the same two `int`s: number of references (a game
using the same entity for both colours counts twice) and the id of the first
game/text that references it. Between the 9-byte node and those trailing two
ints:

| Type | File | Data size | Record size | Layout |
|---|---|---|---|---|
| player | `.cbp` | 58 | 67 | `string[30]` last name, `string[20]` first name |
| tournament | `.cbt` | 90 | 99 | `string[40]` title, `string[30]` place, `int` packed start date, `byte` type+time-control (below), `byte` bit0=team event, `byte` nation, 1 unknown byte, `byte` category, `byte` flags (bit0/1 complete, bit2 board points, bit3 three-points-win), `byte` rounds, 1 unknown byte |
| annotator | `.cbc` | 53 | 62 | `string[45]` name |
| source | `.cbs` | 59 | 68 | `string[25]` title, `string[16]` publisher, `int` publication date, `int` date, `byte` version, `byte` quality (0 unset/1 high/2 medium/3 low) |
| team | `.cbe` | 63 | 72 | `string[45]` title, `int` team number, `byte` bit0=season (year/year+1), `int` year, `byte` nation |
| game tag | `.cbl` | 1608 | 1617 | 8 × `string[200]` titles: English, German, French, Spanish, Italian, Dutch, then two unused slots |

Tournament type/time-control byte: low 5 bits are the kind (0 none, 1 single
game, 2 match, 3 round robin, 4 Swiss, 5 team, 6 knockout, 7 simultaneous, 8
Scheveningen); bit 5 blitz, bit 6 rapid, bit 7 correspondence, none of the
three set = normal, at most one set.

Sort keys (strings compared byte-by-byte with the zero terminator counted, so
a prefix sorts first; no collation, so case and accents sort "wrong" by
natural-language standards): player by last-then-first name (unsigned bytes);
tournament by year *descending*, then title/place (signed bytes), then
month/day descending; annotator by name (signed); source by title (signed,
unverified); team by title (unsigned), then number, season, year, nation.
Within a type no two entities share a key.

A blank field in a game header never points at entity id −1: it points at a
placeholder entity (created on first use) whose text is empty — treat "empty
text" as "not set", don't surface it as a literally-named entity.

`.cbtt` adds a few tournament fields that don't fit the `.cbt` record (little-
endian except as noted): a 32-byte header (`int` version, `int` record size —
61/65/223/405 for versions 2/3/4/5, `int` index of the last tournament with a
record, 20 unused bytes), then one record per tournament id (a tournament past
the end of the file has no extra info): `double` latitude **big-endian**,
`double` longitude **big-endian**, 34 unknown bytes, 10 one-byte tie-break
rule ids (unused slots hold 1), `byte` count of rules in use, `int` end date
**big-endian** (version 3+), then version-4/5-only trailing bytes (still
undocumented, some hold text).

`.cbj` extended game headers (little-endian 32-byte file header: `int`
version, `int` record size, `int` record count, 20 unused bytes; record size
follows from version — v1 adds team ids at size 8, v3 adds media/annotation
offsets at 20, v5 adds final material at 30, v6 adds the moves offset at 38,
v7 adds rating types at 74, v8 adds one more int at 78, v11 adds
version/timestamps/endgame-info/game-tag at 120) carry, per game, big-endian
records with: white/black team id (−1 if none, into `.cbe`), the `.cbm`
manifest offset for a guiding text's media, the `.cba` offset (wider than
`.cbh`'s, same meaning), two "final material" ints (packed rook/bishop/knight/
queen/pawn counts, the greater value first regardless of colour) and a
not-fully-reliable "total material" ushort, the `.cbg` offset (wider than
`.cbh`'s), two 16-byte rating-type records (byte kind: 1 international/2
national; byte time control; byte nation for national ratings; 11-byte name
string, e.g. `FIDE`/`ICCF`), a save-count version ushort, creation and
last-saved timestamps (two different epochs/units — not needed unless a future
feature surfaces them, see the upstream reference), 20 bytes of endgame-type
information (types are unlabeled numbers, still undocumented), and a game-tag
id (−1 if none, into `.cbl`).

We don't need `.cbj` for a first cut — every field the UI needs today (result,
elo, eco, round, dates, flags, move/annotation offsets) is in `.cbh`. Treat
`.cbj` as optional and fall back to defaults when the file is missing or too
short for a field.

### 3.5 Derived files (optional, rebuildable)

`.flags` (Top Games, all big-endian): 12-byte header (4-byte magic `0f 01 0b
09`, `int` capacity in 32-bit words, `int` bits per game — normally 2), then 2
bits per game packed from the body's LSB: the higher bit says "ChessBase has
evaluated whether this is a Top Game", the lower says "and it is one". Not
needed for import; useful later for a Top-Games filter.

`.cit`/`.cib` and `.cit2`/`.cib2` (little-endian): for each entity (or game
tag), the ids of games referencing it, as a `.cit` table of per-entity
`{first block, last block}` pointers into `.cib`'s 64-byte linked blocks. A
derived index our own `.cbb`-adjacent `positions.rs`-style position index could
stand in for, rather than parsing — see §4.

`.cbb` (big-endian) and `.cbgi` (little-endian): per-game derived data (events
in the game; the game's move offset, respectively) — pure optimizations we can
skip entirely and recompute ourselves, since `.cbh` already has the move
offset directly.

`.cbm` (little-endian, 32-byte header) is the manifest of media files a
guiding text embeds; relevant only once guiding-text rendering is in scope.

The `.accelerators/` and `.patterns/` folders (per-database, alongside the
files above): ~59 shard pairs each (`<name><N>.accelerator`/`.acceleratori`,
`N` 0-58), plus a `.config`. Not reverse-engineered in detail — the shard
count (one per half-move up to ~ply 58-59, i.e. the first ~30 full moves)
and the accelerator files' high-entropy contents (consistent with packed
position hashes) point to a precomputed "which games reach this position at
this ply" index: this is ChessBase's **Search Booster** (confirmed by the
`[SearchBooster]` section in the database's own `.ini` — "accelerator" is
just the on-disk file extension, same feature). That's exactly what our own position index
(SRCH-04, `crates/chess-db/src/positions.rs`) already provides, built from
the games themselves once imported — there's no reason to parse
ChessBase's own binary index format to get a capability we already build
ourselves. Skip these entirely; don't revisit without a concrete reason.

### Sources

The byte-level layout above was cross-checked directly against the real files
in `/home/aysaac/Windows/bases/Mega Database 2019.*` with `od` (several
examples are called out inline above) and against the independent write-up at
[Yarin78/morphy](https://github.com/Yarin78/morphy) (`format/v1/*.md`), a Java
ChessBase-database library. That repository carries no license (so its text
and code are not reused here — nothing is copied from it, and the prose,
tables and wording above are written independently from first principles and
verified against the real files), but the *facts* it documents about an
unencumbered, reverse-engineered binary format are exactly what we need, and
every field load-bearing for this feature was independently confirmed against
real bytes rather than taken on trust. **SCID / Scid vs PC** and **ChessX**
(both GPL) are known alternative open-source readers with their own
(partial/lossy, per their own docs) `.cbh` support, worth a look if a specific
field here turns out wrong once we're importing real games, but weren't
needed for this pass.

Crate or module: `.cbh`+`.cbg`+`.cba`+entities needs nothing `chess-db`
doesn't already have (positional I/O, `thiserror`, no Tauri deps), so it's a
module, `crates/chess-db/src/cb/`.

## 4. Status (2026-10-08)

Built, in `crates/chess-db/src/cb/` (a module of `chess-db`, not a crate):
`.cbh` headers, entities, `.cbg` mode-0 moves with the built-in table,
`.cba` comments/symbols/squares/arrows, and `import_cb(db, cbh_path,
progress)`, which batches like `import_pgn`. The app imports a `.cbh`
through Import games (ribbon, empty list): the `import_cb` command runs it
as an `import` job and keeps its warnings for the import report.

**On the real files.** Every game of the four databases bundled with
ChessBase 16 (394 games) decodes with every move legal to its declared
length, and imports with no warnings. Mega Database 2019, all 7,623,697
games (`cb_decode_stats`, 2 min 59 s on one core):

| Outcome | Games |
|---|---|
| Read in full, to the declared length | 7,622,095 |
| Kept with a line cut at a null move (1,012 in variations, 763 in the main line) | 1,775 |
| Kept, its moves end abruptly | 1 |
| Skipped: Chess960 (mode 10/11) | 161 |
| Skipped: encoding mode 4 or 5 | 5 |
| Header's length differs (stale, or a count modulo 256), moves all legal | 24 |

Import benchmark (`import_bench` on the `.cbh`, release build, one core
at low priority while the machine was in use): **7,623,531 games in 388 s**
(19.6k games/s), 166 skipped and 1,943 warnings as above, 670 MB peak
RSS. The result is 1,355 MB (178 bytes a game, against 754 MB of `.cbg`).
On it: open 139 ms, sort by White 656 ms, a random game 150 µs, header
search 150 ms, position index 93 s (364M positions, 5.8 GB), opening tree
at the start 459 ms and after 1.e4 92 ms, Reference lines at the start
9.8 s (a job).

**Dev tools:** `examples/extract_cb_key.rs` (finds the table in a ChessBase
install and validates it), `examples/cb_decode_stats.rs` (tallies a
database's decode outcomes by message; `show ID` prints one game).

**Tests:** unit tests next to the code (`cb::`), plus ignored integration
tests in `crates/chess-db/tests/cb_import.rs` on the real files, which
aren't committed: the sample databases (`CBH_SAMPLES`, default
`testdata/large/cbh-samples/`) and the Mega Database (`CBH_MEGA`, default
`testdata/large/Mega Database 2019.cbh`).

**Left:**
- A committed fixture. The bundled databases are ChessBase's (and
  `Openings.cba` holds about 30 KB of written annotations), so committing
  even `nunn` (10 games, 2 KB) is the user's call. Without one, the
  real-file tests only run where the files are.
- Null moves: the game tree can't hold them, so their lines are cut.
- Chess960 (modes 10/11, with the 8 extra start-position bytes) and the
  other modes.
- Not read: `.cbj`, sources (`.cbs`), teams, eval annotations 0x21/0x26,
  time and clock annotations, guiding texts, deleted games.
- The welcome guide's import wizard still picks PGN files only.
