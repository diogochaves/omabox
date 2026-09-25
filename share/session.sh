#!/usr/bin/env bash
# The box's first process (under bwrap's init). Starts a private session bus, the throwaway keyring,
# then the compositor stack for the box's mode.
set -uo pipefail
# One block, read whole before it runs: this runs for the box's whole life from the repo, so an edit
# to the file meanwhile would otherwise change what bash reads next (finding 66).
{

# Interactive boxes get one connected fd to the host compositor in WAYLAND_SOCKET (tools/wlfd). It is
# for the box's Hyprland alone: take it out of the environment and close it for everything else
# started here. (Under dbus-run-session the bus daemon held a copy of the connection, and every service
# it activated inherited a stale WAYLAND_SOCKET, which libwayland prefers over WAYLAND_DISPLAY: Files
# and the portals died with "Failed to open display". NOTES finding 33.)
host_fd=${WAYLAND_SOCKET:-}
unset WAYLAND_SOCKET
without_host_fd() { if [ -n "$host_fd" ]; then "$@" {host_fd}>&-; else "$@"; fi; }

# A real systemd user manager with `omabox up --systemd` (finding 61): units, timers, `systemd-run
# --user` work as on the host. It gets the box HOME, and it runs the session bus itself: a manager only
# joins the bus when its own dbus.service is up, so that unit is dbus-daemon (as in every box; the
# host's is dbus-broker), socket-activated at $XDG_RUNTIME_DIR/bus. The keyring daemon starts below as
# in every box, so its socket unit is masked; so are PipeWire's (no audio devices in a box).
if [ "${OMABOX_SYSTEMD:-0}" = 1 ]; then
  u=$HOME/.config/systemd/user
  mkdir -p "$u"
  # Not xdg-document-portal: it fails in every box (no /dev/fuse), so the manager reads `degraded`, but
  # masked, xdg-desktop-portal refuses to start at all.
  for m in gnome-keyring-daemon.socket pipewire.socket pipewire-pulse.socket; do ln -sfn /dev/null "$u/$m"; done
  printf '%s\n' '[Unit]' 'Description=D-Bus User Message Bus (omabox)' 'Requires=dbus.socket' '[Service]' \
    'ExecStart=/usr/bin/dbus-daemon --session --address=systemd: --nofork --nopidfile --systemd-activation' \
    'ExecReload=/usr/bin/busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus ReloadConfig' \
    > "$u/dbus.service"
  without_host_fd /usr/lib/systemd/systemd --user --log-target=console > "$HOME/systemd.log" 2>&1 &
  sd_ok=0
  for _ in $(seq 100); do
    case $(systemctl --user is-system-running 2>/dev/null) in running|degraded) sd_ok=1; break ;; esac
    sleep 0.05
  done
  # Not a box that half works with no bus: this lands in box.log and `up` says the box died.
  [ $sd_ok = 1 ] || { echo "session.sh: systemd --user did not come up; see ~/systemd.log" >&2; kill -KILL -1; exit 1; }
  DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
else
  # Private session bus: tray, notifications, portals, dconf, keyring stay inside. Started by hand,
  # not with dbus-run-session, so the host fd can be kept from it.
  DBUS_SESSION_BUS_ADDRESS=$(without_host_fd dbus-daemon --session --fork --print-address) || exit 1
fi
export DBUS_SESSION_BUS_ADDRESS
echo "$DBUS_SESSION_BUS_ADDRESS" > "$XDG_RUNTIME_DIR/dbus-address"

# Throwaway keyring on the private bus: a default "login" keyring with an empty password, which
# gnome-keyring keeps as an unencrypted file and never locks, so apps store and read secrets without a
# prompt. (`--unlock` with an empty password does not create one: the first store then waits on a
# gcr-prompter.) It lives in the box's HOME and dies with the box.
# shellcheck disable=SC2174 # only the keyrings dir itself needs 0700
mkdir -p -m 700 "$HOME/.local/share/keyrings"
if [ ! -e "$HOME/.local/share/keyrings/login.keyring" ]; then
  printf '[keyring]\ndisplay-name=login\nctime=0\nmtime=0\nlock-on-idle=false\nlock-after=false\n' > "$HOME/.local/share/keyrings/login.keyring"
  echo login > "$HOME/.local/share/keyrings/default"
fi
without_host_fd gnome-keyring-daemon --daemonize --components=secrets > "$HOME/keyring.log" 2>&1

# The session's PATH (finding 57), so the bar, plugins and apps launched from binds find what `omabox
# run` finds: omabox's stand-ins, the box HOME's ~/.local/bin (a test can drop a stub CLI there), then
# the PATH `omabox up` ran with in its order, keeping only absolute dirs the box can see (mise's
# installs: claude, gh, node), then the box's own. The box's own programs (Hyprland, labwc, quickshell,
# hyprctl) are called by absolute path, so no stub or project build on this PATH replaces them.
mkdir -p "$HOME/.local/bin"
p=/opt/omabox/share/bin:$HOME/.local/bin
IFS=: read -ra dirs <<< "${OMABOX_CALLER_PATH:-}:$PATH"
for d in "${dirs[@]}"; do
  case $d in /*) ;; *) continue ;; esac
  case ":$p:" in *":$d:"*) ;; *) [ -d "$d" ] && p=$p:$d ;; esac
done
export PATH=$p
unset OMABOX_CALLER_PATH

# Omarchy's session defaults (TERMINAL, EDITOR), as its uwsm env.d does on the host. Always the
# packaged Omarchy: a dev link (/etc/omarchy.conf, OMARCHY_PATH) is not followed into the box.
# shellcheck source=/dev/null
[ -r /usr/share/omarchy/default/uwsm/default ] && . /usr/share/omarchy/default/uwsm/default
export TERMINAL=${TERMINAL:-xdg-terminal-exec} EDITOR=${EDITOR:-omarchy-launch-editor --inline}

hypr=(env -u WLR_BACKENDS -u WLR_LIBINPUT_NO_DEVICES -u WLR_HEADLESS_OUTPUTS -u WLR_RENDER_DRM_DEVICE -u DISPLAY
  OMARCHY_PATH=/usr/share/omarchy LD_LIBRARY_PATH=/opt/omabox/lib HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1
  /usr/bin/Hyprland --config /opt/omabox/share/hyprland.lua)

if [ "${OMABOX_INTERACTIVE:-0}" = 1 ]; then
  # Nested straight into the host compositor through the one connection omabox-wlfd handed us
  # (WAYLAND_SOCKET). Aquamarine picks its Wayland backend when WAYLAND_DISPLAY is set; libwayland
  # prefers WAYLAND_SOCKET, so the name is never opened.
  WAYLAND_SOCKET=$host_fd WAYLAND_DISPLAY=omabox-host "${hypr[@]}" > "$HOME/hyprland.log" 2>&1 &
  hypr_pid=$!
  exec {host_fd}>&-   # Hyprland has it now
  wait "$hypr_pid"    # Hyprland only: with --systemd the user manager is a child too, and never exits
  # The window was closed (share/hyprland.lua exits on it) or Hyprland died. bwrap's PID 1 only exits
  # once it has no children left, and the shell's helpers would keep the box alive, screenless: end
  # everything else in the box's pid namespace.
  kill -KILL -1
  exit 0
fi

# Headless: labwc is the invisible parent compositor (finding 2: aquamarine needs a parent that offers
# wl_compositor v6 and xdg_wm_base v6). DISPLAY is dropped for Hyprland so nothing wakes labwc's lazy
# Xwayland. -S: labwc ends when Hyprland does, and then so does the box, as in the interactive branch;
# otherwise a box whose Hyprland died reads `up` while nothing in it works (finding 63).
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=$OMABOX_RENDER_NODE
/usr/bin/labwc -S "${hypr[*]}" > "$HOME/labwc.log" 2>&1
kill -KILL -1
exit 0
}
