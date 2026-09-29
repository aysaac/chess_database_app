#!/usr/bin/env bash
# End-to-end smoke test of the desktop app.
#
# Launches `npm run tauri dev` (debug build) with CHESSDB_SMOKE pointing at a
# driver script. The driver runs inside the webview, drives the UI through
# `window.__chessdb` (dev builds only) and reports via `[smoke] …` log lines,
# ending with `[smoke] done` or `[smoke] FAILED: …`. Each
# `[smoke] shot:<name>` line is captured as <out-dir>/shots/<name>.png, and
# each `[smoke] clip:<name>` line saves the private display's clipboard as
# <out-dir>/clips/<name>.png (an image) or .txt (text), with wl-paste.
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
# Runs start as a returning user (the welcome guide stays closed), unless the
# driver contains the text "smoke: first-run".
#
# macOS has no private display: the window opens on the desktop (it takes
# focus), and screenshots are `screencapture` of that window alone, which
# needs Screen Recording permission for the terminal. Runs on the stock
# bash 3.2.
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
# Absolute: the app is started from app/.
OUT="$(cd "$OUT" && pwd)"
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
# Every run starts with fresh settings, which is a first run: the welcome
# guide (QOL-08) would open over the app. Drivers start as a returning user
# unless they say "smoke: first-run" (onboarding.js).
if ! grep -q 'smoke: first-run' "$DRIVER"; then
  mkdir -p "$OUT/config"
  [[ -f "$OUT/config/settings.json" ]] || printf '{"welcomeDone": true}\n' >"$OUT/config/settings.json"
fi
LOG="$OUT/dev.log"
: >"$LOG"

MACOS=0
[[ "$(uname -s)" == Darwin ]] && MACOS=1
if ((!MACOS)); then
  # shellcheck source=scripts/offscreen.sh
  source "$ROOT/scripts/offscreen.sh"
fi
APP_PID=""
cleanup() {
  if [[ -n "$APP_PID" ]]; then
    kill -TERM -- "-$APP_PID" 2>/dev/null || true
    sleep 1
    kill -KILL -- "-$APP_PID" 2>/dev/null || true
  fi
  ((MACOS)) || offscreen_stop
}
trap cleanup EXIT

# The app gets a private headless display. It never falls back to the user's
# desktop: without sway the run stops here and says so.
launch=(env)
can_shoot=0
if ((MACOS)); then
  if screencapture -x "$OUT/shots/.probe.png" 2>/dev/null; then
    can_shoot=1
  else
    echo "smoke: no screenshots: give the terminal Screen Recording permission (System Settings > Privacy & Security)"
  fi
  rm -f "$OUT/shots/.probe.png"
  echo "smoke: macOS: the app window opens on your desktop"
elif [[ -n "${SMOKE_VISIBLE:-}" ]]; then
  echo "smoke: SMOKE_VISIBLE=1: the app window opens on your desktop"
else
  if ! offscreen_start "$OUT/sway.log"; then
    echo "smoke: no private display, so no run (SMOKE_VISIBLE=1 runs it on your desktop instead)" >&2
    exit 2
  fi
  launch=(env "${OFFSCREEN_RUN[@]}")
  command -v grim >/dev/null && can_shoot=1
  echo "smoke: app on a private headless display ($OFFSCREEN_SIZE at scale $OFFSCREEN_SCALE); your desktop isn't touched"
fi

# macOS: the CoreGraphics id of the harness window, found by its title.
mac_window_id() {
  osascript -l JavaScript - "$TITLE" <<'JXA'
ObjC.import("CoreGraphics");
function run(argv) {
  const list = ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0));
  for (let i = 0; i < list.count; i++) {
    const w = list.objectAtIndex(i);
    if (ObjC.unwrap(w.objectForKey("kCGWindowName")) === argv[0]) {
      return String(ObjC.unwrap(w.objectForKey("kCGWindowNumber")));
    }
  }
  return "";
}
JXA
}

shoot() {
  local name id
  name=$(printf '%s' "$1" | tr -cd 'a-zA-Z0-9_-')
  ((can_shoot)) || return 0
  if ((MACOS)); then
    id=$(mac_window_id)
    # -o: no window shadow.
    if [[ -n "$id" ]] && screencapture -x -o -l "$id" "$OUT/shots/$name.png"; then
      echo "    screenshot: $OUT/shots/$name.png"
    fi
    return 0
  fi
  # The whole display: sway tiles the only window over all of it, with no
  # borders or bar.
  if offscreen_shot "$OUT/shots/$name.png"; then
    echo "    screenshot: $OUT/shots/$name.png"
  fi
}

# The private display's clipboard (only there: never the user's, so not
# with SMOKE_VISIBLE or on macOS). Prints the types on offer and saves the
# PNG image, or else the text.
clip() {
  local name types
  name=$(printf '%s' "$1" | tr -cd 'a-zA-Z0-9_-')
  if ((MACOS)) || [[ -n "${SMOKE_VISIBLE:-}" ]] || ! command -v wl-paste >/dev/null; then
    echo "    clipboard: not read (needs the private display and wl-paste)"
    return 0
  fi
  mkdir -p "$OUT/clips"
  local paste=(env -u DISPLAY WAYLAND_DISPLAY="$OFFSCREEN_WAYLAND_DISPLAY" wl-paste)
  types=$("${paste[@]}" --list-types 2>/dev/null | tr '\n' ' ' || true)
  echo "    clipboard types: ${types:-none}"
  if [[ " $types " == *" image/png "* ]]; then
    "${paste[@]}" --type image/png >"$OUT/clips/$name.png" && echo "    clipboard: $OUT/clips/$name.png"
  elif [[ -n "$types" ]]; then
    "${paste[@]}" --no-newline >"$OUT/clips/$name.txt" && echo "    clipboard: $OUT/clips/$name.txt"
  fi
  return 0
}

# Own session, so npm, vite, cargo, the app and its private bus can be
# stopped together. Settings and app data (the sample database) go to the
# output folder, so the user's own stay untouched.
# macOS has no setsid; job control gives the background job its own process
# group instead.
if command -v setsid >/dev/null; then
  launch=(setsid "${launch[@]}")
else
  set -m
fi
"${launch[@]}" bash -c "cd '$ROOT/app' && CHESSDB_SMOKE='$OUT/driver.js' CHESSDB_CONFIG_DIR='$OUT/config' exec npm run tauri dev -- --config '$OUT/tauri.smoke.json'" >"$LOG" 2>&1 &
APP_PID=$!
# Without job control's "Terminated" line when cleanup stops it.
((MACOS)) && disown

echo "smoke: driver $DRIVER, output $OUT (1M-game part: $BIG)"
status=1
seen=0
start=$SECONDS
while ((SECONDS - start < TIMEOUT)); do
  # No mapfile: macOS ships bash 3.2.
  count=0
  while IFS= read -r line; do
    count=$((count + 1))
    ((count > seen)) || continue
    msg="${line#*\[smoke\] }"
    echo "  $msg"
    case "$msg" in
      shot:*) shoot "${msg#shot:}" ;;
      clip:*) clip "${msg#clip:}" ;;
      done) status=0; break 2 ;;
      FAILED*) break 2 ;;
    esac
  done < <(perl -pe 's/\e\[[0-9;]*m//g' "$LOG" | grep -F '[smoke]' || true)
  seen=$count
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
