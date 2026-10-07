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
  # not with dbus-run-session, so the host fd can be kept from it. At $XDG_RUNTIME_DIR/bus, where a
  # session has it (and the systemd boxes above), not a socket in /tmp: a helper that checks its bus
  # wants it in the 0700 runtime dir (#153).
  DBUS_SESSION_BUS_ADDRESS=$(without_host_fd dbus-daemon --session --fork --print-address \
    --address="unix:path=$XDG_RUNTIME_DIR/bus") || exit 1
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
# `up --omarchy DIR` (finding 135): its bin next, as Omarchy's env-bootstrap puts a dev link's first.
[ -z "${OMABOX_OMARCHY:-}" ] || p=$p:$OMABOX_OMARCHY/bin
IFS=: read -ra dirs <<< "${OMABOX_CALLER_PATH:-}:$PATH"
for d in "${dirs[@]}"; do
  case $d in /*) ;; *) continue ;; esac
  case ":$p:" in *":$d:"*) ;; *) [ -d "$d" ] && p=$p:$d ;; esac
done
export PATH=$p
unset OMABOX_CALLER_PATH

# Omarchy's session defaults (TERMINAL, EDITOR), as its uwsm env.d does on the host: the packaged
# Omarchy's, or `up --omarchy`'s tree's. The host's dev link is not followed (finding 135).
o=${OMABOX_OMARCHY:-/usr/share/omarchy}
# shellcheck source=/dev/null
[ -r "$o/default/uwsm/default" ] && . "$o/default/uwsm/default"
export TERMINAL=${TERMINAL:-xdg-terminal-exec} EDITOR=${EDITOR:-omarchy-launch-editor --inline}

# `up --theme NAME` (finding 245): the box's theme is NAME, not the desktop's, set as Omarchy sets one
# (its themes overlaid with the user's, its templates, the first background) before Hyprland and the
# bar read it. Headless: no shell to tell, no terminals to restart, no hooks. seed_home left no
# current theme, so one that fails here ends the box (box.log says why) rather than run without one.
if [ -n "${OMABOX_THEME:-}" ]; then
  if ! without_host_fd env PATH="$o/bin:$PATH" OMARCHY_PATH="$o" OMARCHY_THEME_HEADLESS=1 \
      "$o/bin/omarchy-theme-set" "$OMABOX_THEME" > "$HOME/theme-set.log" 2>&1 ||
    [ ! -d "$HOME/.local/state/omarchy/current/theme" ]; then
    echo "session.sh: up --theme: omarchy-theme-set $OMABOX_THEME failed: $(grep . "$HOME/theme-set.log" | tail -n 1) (~/theme-set.log)" >&2
    kill -KILL -1; exit 1
  fi
fi

# The theme's light or dark mode in the box's dconf, as `omarchy-theme-set` leaves it on the host
# (finding 148): a fresh dconf says "no preference", so portal dialogs, GTK, libadwaita and Qt apps
# came out light under a dark theme. In the background: the session does not wait for dconf.
command -v omarchy-theme-set-gnome >/dev/null &&
  { without_host_fd omarchy-theme-set-gnome > "$HOME/theme-gnome.log" 2>&1 & }

# `up --hyprland PATH` (finding 116): which build runs, in box.log (this script's stderr), as that
# build says once it is up: the first line of `hyprctl version` (version, commit, dirty or clean).
if [ -n "${OMABOX_HYPRLAND:-}" ]; then
  # shellcheck disable=SC2016 # expanded by the inner bash
  without_host_fd bash -c '
    env=$XDG_RUNTIME_DIR/omabox.env
    for _ in $(seq 600); do [ -s "$env" ] && break; sleep 0.1; done
    sig=$(tr "\0" "\n" < "$env" 2>/dev/null | sed -n "s/^HYPRLAND_INSTANCE_SIGNATURE=//p" | head -n 1)
    v=$(HYPRLAND_INSTANCE_SIGNATURE=$sig /usr/bin/hyprctl version 2>&1 | head -n 1)
    echo "session.sh: Hyprland $OMABOX_HYPRLAND: ${v:-no answer to hyprctl version}" >&2' &
fi

if [ "${OMABOX_INTERACTIVE:-0}" = 1 ]; then
  # Nested straight into the host compositor through the one connection omabox-wlfd handed us
  # (WAYLAND_SOCKET). Aquamarine picks its Wayland backend when WAYLAND_DISPLAY is set; libwayland
  # prefers WAYLAND_SOCKET, so the name is never opened.
  WAYLAND_SOCKET=$host_fd WAYLAND_DISPLAY=omabox-host /opt/omabox/share/start-hyprland.sh > "$HOME/hyprland.log" 2>&1 &
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
# Started through a link named omabox-labwc (#128): a process's name (comm, what pkill -x matches) is
# the name of the path it was started by, so a test's own `pkill -x labwc` no longer ends the box.
# More monitors on NVIDIA (#165, finding 234), where every screen of the box is a window on labwc:
# - WLR_SCENE_DISABLE_VISIBILITY: a window wholly under another got no frame callbacks (wlroots sends
#   them to what is visible on an output), so a monitor's window under the main one never drew. With
#   it every window on labwc's one output gets them, wherever the others are.
# - labwc's config is omabox's own, in the runtime dir (-C; not the box HOME's ~/.config/labwc), as
#   `omabox monitor add` rewrites it (bin/omabox labwc_rc, which keeps these lines): aquamarine takes
#   every size labwc configures its window with as its mode (0x0 as 1280x720), and labwc configures it
#   again on a focus change, a reload or an output resize, with the size it last set itself. So the
#   main window is maximized (its size is labwc's output's: `omabox mode` resizes that), and each
#   monitor's window is given its mode's size when it maps (ResizeTo, by its title).
lw=$XDG_RUNTIME_DIR/labwc
mkdir -p "$lw"
printf '%s\n' '<?xml version="1.0"?>' '<labwc_config>' '  <windowRules>' \
  '    <windowRule identifier="aquamarine" serverDecoration="no"/>' \
  '    <windowRule title="aquamarine - WAYLAND-1"><action name="Maximize"/></windowRule>' \
  '  </windowRules>' '</labwc_config>' > "$lw/rc.xml"
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=$OMABOX_RENDER_NODE \
  WLR_SCENE_DISABLE_VISIBILITY=1
ln -sf /usr/bin/labwc "$XDG_RUNTIME_DIR/omabox-labwc"
"$XDG_RUNTIME_DIR/omabox-labwc" -C "$lw" -S /opt/omabox/share/start-hyprland.sh > "$HOME/labwc.log" 2>&1
kill -KILL -1
exit 0
}
