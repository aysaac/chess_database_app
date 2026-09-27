#!/usr/bin/env bash
# Downloads the official Stockfish release for this machine (ENG-09) and puts
# it where Tauri expects a sidecar binary:
#   app/src-tauri/binaries/stockfish-<rust target triple>[.exe]
# The app finds it there in dev builds; `tauri build` bundles it when run with
# `--config src-tauri/tauri.stockfish.json` (see docs/stack.md).
#
# Stockfish is GPL-3.0 and runs as a separate process.
#
# Usage: scripts/fetch-stockfish.sh [target-triple]
set -euo pipefail

VERSION="sf_19"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/app/src-tauri/binaries"
TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

# Release asset and its SHA-256 (from the GitHub release page) per target.
case "$TRIPLE" in
  x86_64-unknown-linux-gnu)
    ASSET=stockfish-linux-x86-64-universal.tar.gz
    SHA=9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f ;;
  aarch64-unknown-linux-gnu)
    ASSET=stockfish-linux-arm64-universal.tar.gz
    SHA=fe26cfd1d9db4c8af3d21e24d9ff34cacb31c1f940085a7583da11796f2bac01 ;;
  x86_64-apple-darwin | aarch64-apple-darwin)
    ASSET=stockfish-macos-universal.tar.gz
    SHA=a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77 ;;
  x86_64-pc-windows-msvc)
    ASSET=stockfish-windows-x86-64-universal.zip
    SHA=3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7 ;;
  aarch64-pc-windows-msvc)
    ASSET=stockfish-windows-arm64-universal.zip
    SHA=8372ad3f0d7276deb2c70f801f541ec7db463219fc6d9c7592864e542aa4f401 ;;
  *)
    echo "fetch-stockfish: no Stockfish release for $TRIPLE" >&2
    exit 1 ;;
esac

EXT=""
[[ "$TRIPLE" == *windows* ]] && EXT=".exe"
OUT="$DEST/stockfish-$TRIPLE$EXT"
if [[ -x "$OUT" ]]; then
  echo "fetch-stockfish: $OUT already exists"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/official-stockfish/Stockfish/releases/download/$VERSION/$ASSET"
echo "fetch-stockfish: downloading $URL"
curl -fL --retry 3 -o "$TMP/$ASSET" "$URL"

if command -v sha256sum >/dev/null; then
  GOT="$(sha256sum "$TMP/$ASSET" | cut -d' ' -f1)"
else
  GOT="$(shasum -a 256 "$TMP/$ASSET" | cut -d' ' -f1)"
fi
if [[ "$GOT" != "$SHA" ]]; then
  echo "fetch-stockfish: checksum mismatch for $ASSET (got $GOT)" >&2
  exit 1
fi

mkdir -p "$TMP/x"
case "$ASSET" in
  *.zip) unzip -q "$TMP/$ASSET" -d "$TMP/x" ;;
  *) tar -xf "$TMP/$ASSET" -C "$TMP/x" ;;
esac
# The archive holds sources and docs too; the engine is the stockfish-*
# executable at the top of the extracted folder.
BIN="$(find "$TMP/x" -maxdepth 2 -type f -name "stockfish*$EXT" ! -name '*.txt' ! -name '*.md' | head -n1)"
if [[ -z "$BIN" ]]; then
  echo "fetch-stockfish: no engine binary found in $ASSET" >&2
  exit 1
fi
mkdir -p "$DEST"
cp "$BIN" "$OUT"
chmod +x "$OUT"
echo "fetch-stockfish: installed $OUT"
"$OUT" <<<"quit" | head -n1 || true
