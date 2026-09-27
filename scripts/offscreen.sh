#!/usr/bin/env bash
# Runs a command on a private, invisible Wayland display.
#
# The display is a headless sway (wlroots' headless backend) with its own
# runtime directory. The user's compositor is never contacted: no outputs,
# workspaces, window rules or cursor moves on their desktop, and nothing to
# restore there afterwards. The command also gets:
# - no X11 fallback (DISPLAY unset, GDK_BACKEND=wayland), so GTK can't open
#   on the user's Xwayland if the display fails;
# - its own D-Bus session (dbus-run-session), so file portals,
#   notifications, tray icons and single-instance checks can't reach the
#   user's desktop session. The bus starts no services: activation would
#   run a second portal, gvfs or keyring against the user's shared state
#   (the portals try to mount /run/user/<uid>/doc, for one).
#
# Usage:
#   scripts/offscreen.sh COMMAND [ARGS...]
#     Runs COMMAND on the display and stops the display when it exits.
#     Inside COMMAND, `grim FILE.png` screenshots the display and `swaymsg`
#     talks to its sway (e.g. `swaymsg -t get_tree` for window geometry).
#   source scripts/offscreen.sh; offscreen_start [LOG]; ...; offscreen_stop
#     For scripts: `"${OFFSCREEN_RUN[@]}" COMMAND` runs a command on the
#     display and `offscreen_shot FILE.png` takes a screenshot.
#
# OFFSCREEN_SIZE (default 2560x1600) and OFFSCREEN_SCALE (default 1.25) set
# the display's mode; the defaults match the user's laptop screen, so
# screenshots show what they would see. Needs sway (Arch: `pacman -S sway`)
# and dbus-run-session; grim for screenshots.

OFFSCREEN_SIZE="${OFFSCREEN_SIZE:-2560x1600}"
OFFSCREEN_SCALE="${OFFSCREEN_SCALE:-1.25}"
OFFSCREEN_DIR=""
OFFSCREEN_PID=""
OFFSCREEN_WAYLAND_DISPLAY=""
OFFSCREEN_SWAYSOCK=""
OFFSCREEN_RUN=()

# Starts the display. LOG receives sway's output (default: inside the private
# directory, which offscreen_stop deletes). Returns 1 if it can't start.
offscreen_start() {
  local log="${1:-}" tool wd
  for tool in sway dbus-run-session; do
    if ! command -v "$tool" >/dev/null; then
      echo "offscreen: $tool isn't installed (Arch: pacman -S ${tool/dbus-run-session/dbus})." >&2
      return 1
    fi
  done
  # Private runtime directory: sway's sockets live here, never next to the
  # user's compositor socket, so no client can find this display by
  # accident. Kept short: socket paths are limited to 108 bytes.
  OFFSCREEN_DIR=$(mktemp -d -p "${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}" chessdb-offscreen.XXXXXX)
  [[ -n "$log" ]] || log="$OFFSCREEN_DIR/sway.log"
  # sway runs `exec` lines once the config is applied, with WAYLAND_DISPLAY
  # and SWAYSOCK set: the ready file says it's up and where.
  cat >"$OFFSCREEN_DIR/sway.conf" <<EOF
output HEADLESS-1 resolution $OFFSCREEN_SIZE scale $OFFSCREEN_SCALE position 0 0
default_border none
default_floating_border none
xwayland disable
exec sh -c 'printf "%s %s\\n" "\$WAYLAND_DISPLAY" "\$SWAYSOCK" > "\$XDG_RUNTIME_DIR/ready"'
EOF
  # WLR_BACKENDS=headless: no DRM, no libinput and no logind session, so it
  # can't take the seat, the screen or the input devices from the user's
  # compositor. It renders on the GPU's render node, which is shared.
  env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK -u HYPRLAND_INSTANCE_SIGNATURE -u HYPRLAND_CMD \
    XDG_RUNTIME_DIR="$OFFSCREEN_DIR" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$OFFSCREEN_DIR/sway.conf" </dev/null >"$log" 2>&1 &
  OFFSCREEN_PID=$!
  for _ in $(seq 100); do
    [[ -s "$OFFSCREEN_DIR/ready" ]] && break
    kill -0 "$OFFSCREEN_PID" 2>/dev/null || break
    sleep 0.1
  done
  if [[ ! -s "$OFFSCREEN_DIR/ready" ]]; then
    echo "offscreen: sway didn't start. Its log:" >&2
    tail -n 20 "$log" >&2
    offscreen_stop
    return 1
  fi
  read -r wd OFFSCREEN_SWAYSOCK <"$OFFSCREEN_DIR/ready"
  # An absolute WAYLAND_DISPLAY: the command keeps the user's
  # XDG_RUNTIME_DIR for everything else.
  OFFSCREEN_WAYLAND_DISPLAY="$OFFSCREEN_DIR/$wd"
  # A session bus without <standard_session_servicedirs/>: nothing can be
  # activated on it. No accessibility bus either (it's a service).
  cat >"$OFFSCREEN_DIR/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:dir=$OFFSCREEN_DIR</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
  OFFSCREEN_RUN=(env -u DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u HYPRLAND_CMD
    WAYLAND_DISPLAY="$OFFSCREEN_WAYLAND_DISPLAY" SWAYSOCK="$OFFSCREEN_SWAYSOCK" GDK_BACKEND=wayland
    NO_AT_BRIDGE=1 GTK_A11Y=none
    dbus-run-session --config-file="$OFFSCREEN_DIR/bus.conf" --)
}

# Screenshots the whole display into FILE (PNG).
offscreen_shot() {
  WAYLAND_DISPLAY="$OFFSCREEN_WAYLAND_DISPLAY" grim "$1"
}

# Stops the display and deletes its directory. Stop its clients first.
offscreen_stop() {
  if [[ -n "$OFFSCREEN_PID" ]]; then
    kill -TERM "$OFFSCREEN_PID" 2>/dev/null || true
    for _ in $(seq 20); do
      kill -0 "$OFFSCREEN_PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$OFFSCREEN_PID" 2>/dev/null || true
    wait "$OFFSCREEN_PID" 2>/dev/null || true
    OFFSCREEN_PID=""
  fi
  if [[ "$OFFSCREEN_DIR" == */chessdb-offscreen.* ]]; then
    rm -rf -- "$OFFSCREEN_DIR"
  fi
  OFFSCREEN_DIR=""
  OFFSCREEN_RUN=()
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  if (($# == 0)); then
    sed -n '2,/^$/s/^# \{0,1\}//p' "$0" >&2
    exit 2
  fi
  trap offscreen_stop EXIT
  offscreen_start || exit 2
  echo "offscreen: display $OFFSCREEN_WAYLAND_DISPLAY ($OFFSCREEN_SIZE at scale $OFFSCREEN_SCALE)" >&2
  status=0
  "${OFFSCREEN_RUN[@]}" "$@" || status=$?
  exit "$status"
fi
