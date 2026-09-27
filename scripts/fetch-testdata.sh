#!/usr/bin/env bash
# Downloads large test data into testdata/large/ (git-ignored):
#  - one TWIC issue (annotated-free master games, realistic headers)
#  - the first ~1M games of a Lichess monthly dump (volume test)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/testdata/large"
mkdir -p "$OUT"

TWIC_ISSUE="${TWIC_ISSUE:-1600}"
LICHESS_MONTH="${LICHESS_MONTH:-2024-01}"
LICHESS_GAMES="${LICHESS_GAMES:-1000000}"

if [[ ! -f "$OUT/twic$TWIC_ISSUE.pgn" ]]; then
  echo "Downloading TWIC $TWIC_ISSUE..."
  # TWIC rejects requests without browser-like headers (HTTP 406).
  curl -fL -o "$OUT/twic$TWIC_ISSUE.zip" \
    -H "User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0" \
    -H "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8" \
    -H "Referer: https://theweekinchess.com/twic" \
    "https://theweekinchess.com/zips/twic${TWIC_ISSUE}g.zip"
  unzip -o -q "$OUT/twic$TWIC_ISSUE.zip" -d "$OUT"
  rm "$OUT/twic$TWIC_ISSUE.zip"
fi

LICHESS_OUT="$OUT/lichess_1m.pgn"
if [[ ! -f "$LICHESS_OUT" ]]; then
  echo "Streaming the first $LICHESS_GAMES games of Lichess $LICHESS_MONTH..."
  # Stop after N games: count [Event lines and cut before the (N+1)th.
  curl -fsL "https://database.lichess.org/standard/lichess_db_standard_rated_${LICHESS_MONTH}.pgn.zst" \
    | zstdcat 2>/dev/null \
    | awk -v n="$LICHESS_GAMES" '/^\[Event / { if (++c > n) exit } { print }' \
    > "$LICHESS_OUT" || true
  echo "Wrote $(grep -c '^\[Event ' "$LICHESS_OUT") games to $LICHESS_OUT"
fi

ls -lh "$OUT"
