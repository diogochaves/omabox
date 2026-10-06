#!/usr/bin/env bash
# install.sh: set up omabox on an Omarchy machine. Idempotent: re-run after pulling.
#
#   ./install.sh           packages (sudo only if some are missing), aquamarine's fix, tools, then
#                          omabox setup: links, settings dir, the agent guard (asked)
#   ./install.sh --check   the same, then start a box, screenshot it and tear it down
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
PKGS=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret   # run a box
  quickshell gtk3 xdg-terminal-exec dbus                                 # in a box (Omarchy has them)
  passt                                                                  # every box's network (pasta)
  python                                                                 # the agent guard's Codex check
  wayland libxkbcommon base-devel pkgconf                                               # tools/
  git cmake ninja hyprwayland-scanner hyprutils seatd libdisplay-info hwdata libinput   # aquamarine
  libdrm mesa pixman
)

step() { printf '\n==> %s\n' "$*"; }
die() { echo "install.sh: $*" >&2; exit 1; }

check=0
case ${1:-} in --check) check=1 ;; "") ;; *) die "usage: ./install.sh [--check]" ;; esac
command -v pacman >/dev/null || die "this expects Arch/Omarchy (pacman)"
command -v Hyprland >/dev/null || die "Hyprland is not installed"
compgen -G "/dev/dri/renderD*" >/dev/null || die "no GPU render node (/dev/dri/renderD*): a box renders on the GPU"
hv=$(Hyprland --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
[ "$(vercmp "${hv:-0}" 0.56.0)" -ge 0 ] || die "Hyprland ${hv:-?} is too old: omabox needs 0.56+ (Lua config)"

step "Packages"
mapfile -t missing < <(pacman -T "${PKGS[@]}" || true)
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}"
  sudo pacman -S --needed "${missing[@]}"
else
  echo "all present"
fi
# `omabox shot --window` captures a window by its toplevel id (grim -T, 1.5+; NOTES finding 81).
grim -h 2>&1 | grep -q -- '^ *-T ' || die "this grim cannot capture a window (no -T): omabox needs grim 1.5 or later"

# A checkout builds it into build/prefix, for every box this checkout starts (the suite needs it for
# NVIDIA and confirm-close): nothing to do while the system's aquamarine has the fix (finding 125).
step "aquamarine's fix for nested Wayland outputs (hyprwm/aquamarine#415)"
"$ROOT/bin/omabox" setup --aquamarine || die "omabox setup --aquamarine failed"

step "Tools"
# (Not `make && echo`: set -e ignores a failure on the left of &&, and install.sh would carry on.)
for t in pointer keyboard wlfd peek still relay events; do
  make -s -C "$ROOT/tools/$t" || die "building tools/$t failed"
  echo "tools/$t"
done

# What each user needs (links: omabox, the agent skill, the bar widget; the settings dir; the agent
# guard, asked for): `omabox setup`, which a package's users run themselves (finding 126).
step "Links"
"$ROOT/bin/omabox" setup || die "omabox setup failed"

if [ $check = 1 ]; then
  step "Check: a box up, a screenshot, down"
  name=install-check-$$
  trap '"$ROOT/bin/omabox" down "$name" >/dev/null 2>&1 || true' EXIT
  "$ROOT/bin/omabox" up "$name"
  shot=$("$ROOT/bin/omabox" shot -b "$name")
  echo "screenshot: $shot"
fi
step "Done. Try: omabox up && omabox shot"
