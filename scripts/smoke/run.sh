#!/usr/bin/env bash
# End-to-end smoke test of the desktop app.
#
# Launches `npm run tauri dev` (debug build) with CHESSDB_SMOKE pointing at a
# driver script. The driver runs inside the webview, drives the UI through
# `window.__chessdb` (dev builds only) and reports via `[smoke] …` log lines,
# ending with `[smoke] done` or `[smoke] FAILED: …`. Each
# `[smoke] shot:<name>` line is captured as <out-dir>/shots/<name>.png.
#
# The app runs on a private, invisible display (scripts/offscreen.sh: a
# headless sway with its own sockets and D-Bus). The user's desktop is never
# touched: no windows, focus changes, monitors or cursor moves. Needs sway,
# plus grim for screenshots.
#
# Usage: scripts/smoke/run.sh [driver.js] [out-dir]
# (out-dir defaults to target/smoke/<timestamp>, which git ignores)
#
# Placeholders replaced in the driver: __ROOT__ (repo root), __OUT__ (out-dir),
# __BIG__ ("1" to run the 1M-game part; default: if
# testdata/large/lichess_1m.pgn exists, override with SMOKE_BIG=0/1).
# SMOKE_TIMEOUT: seconds before giving up (default 900; the first run
# compiles the app).
# SMOKE_PORT: dev-server port (default 1430), separate from the normal
# `tauri dev` port 1420 so both can run at the same time.
# OFFSCREEN_SIZE / OFFSCREEN_SCALE: the display's mode (default 2560x1600 at
# 1.25, the user's laptop screen).
# SMOKE_VISIBLE=1: open the window on your own desktop instead, to watch it
# (it takes a tile and focus there; no screenshots).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DRIVER="${1:-$ROOT/scripts/smoke/basic.js}"
OUT="${2:-$ROOT/target/smoke/$(date +%Y%m%d-%H%M%S)}"
TIMEOUT="${SMOKE_TIMEOUT:-900}"
PORT="${SMOKE_PORT:-1430}"
BIG="${SMOKE_BIG:-}"
if [[ -z "$BIG" ]]; then
  BIG=0
  [[ -f "$ROOT/testdata/large/lichess_1m.pgn" ]] && BIG=1
fi

if (exec 3<>"/dev/tcp/localhost/$PORT") 2>/dev/null; then
  echo "smoke: port $PORT is already in use; set SMOKE_PORT to a free port" >&2
  exit 2
fi

mkdir -p "$OUT/shots"
# The harness window gets its own title, to tell it from a normal `tauri dev`
# window in logs and window lists.
TITLE="Chess Database smoke $PORT"
python3 - "$ROOT/app/src-tauri/tauri.conf.json" "$PORT" "$TITLE" >"$OUT/tauri.smoke.json" <<'PY'
import json, sys
conf, port, title = sys.argv[1:]
windows = json.load(open(conf))["app"]["windows"]
for w in windows:
    w.update(title=title)
print(json.dumps({
    "build": {"devUrl": f"http://localhost:{port}",
              "beforeDevCommand": f"npm run dev -- --port {port} --strictPort"},
    "app": {"windows": windows},
}))
PY
sed -e "s|__ROOT__|$ROOT|g" -e "s|__OUT__|$OUT|g" -e "s|__BIG__|$BIG|g" "$DRIVER" >"$OUT/driver.js"
LOG="$OUT/dev.log"
: >"$LOG"

# shellcheck source=scripts/offscreen.sh
source "$ROOT/scripts/offscreen.sh"
APP_PID=""
cleanup() {
  if [[ -n "$APP_PID" ]]; then
    kill -TERM -- "-$APP_PID" 2>/dev/null || true
    sleep 1
    kill -KILL -- "-$APP_PID" 2>/dev/null || true
  fi
  offscreen_stop
}
trap cleanup EXIT

# The app gets a private headless display. It never falls back to the user's
# desktop: without sway the run stops here and says so.
launch=()
can_shoot=0
if [[ -n "${SMOKE_VISIBLE:-}" ]]; then
  echo "smoke: SMOKE_VISIBLE=1: the app window opens on your desktop"
else
  if ! offscreen_start "$OUT/sway.log"; then
    echo "smoke: no private display, so no run (SMOKE_VISIBLE=1 runs it on your desktop instead)" >&2
    exit 2
  fi
  launch=("${OFFSCREEN_RUN[@]}")
  command -v grim >/dev/null && can_shoot=1
  echo "smoke: app on a private headless display ($OFFSCREEN_SIZE at scale $OFFSCREEN_SCALE); your desktop isn't touched"
fi

shoot() {
  local name
  name=$(printf '%s' "$1" | tr -cd 'a-zA-Z0-9_-')
  ((can_shoot)) || return 0
  # The whole display: sway tiles the only window over all of it, with no
  # borders or bar.
  if offscreen_shot "$OUT/shots/$name.png"; then
    echo "    screenshot: $OUT/shots/$name.png"
  fi
}

# Own session, so npm, vite, cargo, the app and its private bus can be
# stopped together. Settings go to the output folder, so the user's own
# settings stay untouched.
setsid "${launch[@]}" bash -c "cd '$ROOT/app' && CHESSDB_SMOKE='$OUT/driver.js' CHESSDB_CONFIG_DIR='$OUT/config' exec npm run tauri dev -- --config '$OUT/tauri.smoke.json'" >"$LOG" 2>&1 &
APP_PID=$!

echo "smoke: driver $DRIVER, output $OUT (1M-game part: $BIG)"
status=1
seen=0
start=$SECONDS
while ((SECONDS - start < TIMEOUT)); do
  mapfile -t lines < <(sed 's/\x1b\[[0-9;]*m//g' "$LOG" | grep -F '[smoke]' || true)
  for ((i = seen; i < ${#lines[@]}; i++)); do
    msg="${lines[$i]#*\[smoke\] }"
    echo "  $msg"
    case "$msg" in
      shot:*) shoot "${msg#shot:}" ;;
      done) status=0; break 2 ;;
      FAILED*) break 2 ;;
    esac
  done
  seen=${#lines[@]}
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "smoke: the app exited before the driver finished"
    break
  fi
  sleep 0.5
done
((SECONDS - start >= TIMEOUT)) && echo "smoke: timed out after ${TIMEOUT}s"

if ((status == 0)); then
  echo "smoke: passed (log: $LOG)"
else
  echo "smoke: FAILED (log: $LOG)"
  grep -E 'error|panicked|ERROR' "$LOG" | tail -n 20 || true
fi
exit "$status"
