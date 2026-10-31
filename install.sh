#!/usr/bin/env bash
# install.sh: set up omabox on an Omarchy machine. Idempotent: re-run after pulling.
#
#   ./install.sh           packages (sudo only if some are missing; Arch's rust when there is no
#                          cargo), the tools and omabox itself (cargo, into build/omabox), aquamarine's
#                          fix, then omabox setup: links, settings dir, the agent guard (asked)
#   ./install.sh --check   the same, then start a box, screenshot it and tear it down
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
PKGS=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret   # run a box
  quickshell gtk3 xdg-terminal-exec dbus                                 # in a box (Omarchy has them)
  passt                                                                  # every box's network (pasta)
  python                                                                 # the agent guard's Codex check, a jailed box's covers
  wayland libxkbcommon base-devel pkgconf libpng at-spi2-core glib2                    # tools/ (pixman below)
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
# A node that renders in software only passes that, and a box then dies "while starting" (#395):
# virtio-gpu (a VM's) with neither virgl nor context types (venus), as its virtio device's features
# say (one 0/1 per bit: 0 virgl, 4 context init). That virtio device is the node's device's child
# virtioN on PCI (device/ is virtio-pci's), or device/ itself (virtio-mmio). Told without starting
# anything; only when every node is one (a box on such a node beside a GPU is up's to say).
SYSDRM=${OMABOX_TEST_SYSDRM:-/sys/class/drm}
soft=() hw=0
for n in /dev/dri/renderD*; do
  sw=0
  for d in "$SYSDRM/${n##*/}/device" "$SYSDRM/${n##*/}/device"/virtio*; do
    drv=$(readlink -f "$d/driver") || drv=
    f=$(cat "$d/features" 2>/dev/null) || f=
    if [ "${drv##*/}" = virtio_gpu ] && [ "${f:0:1}" = 0 ] && [ "${f:4:1}" = 0 ]; then sw=1; fi
  done
  if [ $sw = 1 ]; then soft+=("${n##*/}"); else hw=1; fi
done
[ $hw = 1 ] || die "every render node renders in software only (${soft[*]}: virtio-gpu without virgl):" \
  "a box needs GPU rendering; in a VM, virtio-gpu with virgl (QEMU's virtio-vga-gl, libvirt's accel3d with an OpenGL display)"
hv=$(Hyprland --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
[ "$(vercmp "${hv:-0}" 0.56.0)" -ge 0 ] || die "Hyprland ${hv:-?} is too old: omabox needs 0.56+ (Lua config)"

step "Packages"
mapfile -t missing < <(pacman -T "${PKGS[@]}" || true)
# omabox is built from crates/ (#324): Arch's rust, unless a rustup or another cargo is already on PATH.
command -v cargo >/dev/null || missing+=(rust)
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}"
  # With no terminal (an agent's run, `./install.sh </dev/null`) sudo can only fail, in words of its
  # own that vary: the command to run instead (#395). Stdin, where pacman reads its [Y/n].
  [ -t 0 ] || die "no terminal for sudo's password: run this, then $0 again: sudo pacman -S --needed ${missing[*]}"
  sudo pacman -S --needed "${missing[@]}"
else
  echo "all present"
fi
# `omabox shot --window` captures a window by its toplevel id (grim -T, 1.5+; NOTES finding 81).
grim -h 2>&1 | grep -q -- '^ *-T ' || die "this grim cannot capture a window (no -T): omabox needs grim 1.5 or later"

# A cargo that is there but cannot build (a rustup with no default toolchain) says so in its own words.
cargo --version >/dev/null 2>&1 ||
  die "cargo is there but cannot build: $(cargo --version 2>&1 | head -n 1) (with rustup: rustup default stable)"

# Built before anything runs omabox (#324 wave 6-flip, proposal D2).
step "Tools"
# (Not `make && echo`: set -e ignores a failure on the left of &&, and install.sh would carry on.)
for t in pointer keyboard wlfd peek still burst tree relay events polkit fixture; do
  make -s -C "$ROOT/tools/$t" || die "building tools/$t failed"
  echo "tools/$t"
done
# omabox itself (crates/omabox): into build/omabox by a rename, so no caller ever runs a half-written
# file, and a command running meanwhile keeps the binary it started with (finding 66). Into ROOT's own
# target/, whatever target dir the user's CARGO_TARGET_DIR or cargo config names (finding 525).
CARGO_TARGET_DIR="$ROOT/target" cargo build --release --locked -q --manifest-path "$ROOT/Cargo.toml" ||
  die "building crates/omabox failed"
install -Dm755 "$ROOT/target/release/omabox" "$ROOT/build/omabox.new" || die "installing build/omabox failed"
mv -f "$ROOT/build/omabox.new" "$ROOT/build/omabox" || die "installing build/omabox failed"
echo "build/omabox"

# A checkout builds it into build/prefix, for every box this checkout starts (the suite needs it for
# NVIDIA and confirm-close): nothing to do while the system's aquamarine has the fix (finding 125).
step "aquamarine's fix for nested Wayland outputs (hyprwm/aquamarine#415)"
"$ROOT/bin/omabox" setup --aquamarine || die "omabox setup --aquamarine failed"

# What each user needs (links: omabox, the agent skill, the bar widget; the settings dir; the agent
# guard, asked for): `omabox setup`, which a package's users run themselves (finding 126).
step "Links"
"$ROOT/bin/omabox" setup || die "omabox setup failed"

if [ $check = 1 ]; then
  step "Check: a box up, a screenshot, down"
  name=install-check-$$
  # A failed check's logs are copied out of the box's folder, which the down on exit removes and up's
  # own failure names (#395). A check that passes removes an earlier one's.
  kept=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/install-check
  rm -rf "$kept"
  trap '"$ROOT/bin/omabox" down "$name" >/dev/null 2>&1 || true' EXIT
  if "$ROOT/bin/omabox" up "$name" && shot=$("$ROOT/bin/omabox" shot -b "$name"); then
    echo "screenshot: $shot"
  else
    box=${XDG_RUNTIME_DIR:-/nonexistent}/omabox/$name
    mkdir -p "$kept/home" || die "the check box failed, and its logs could not be kept in $kept"
    cp -P "$box/box.log" "$kept/" 2>/dev/null || true
    for f in "$box"/home/*.log; do [ ! -f "$f" ] || cp -P "$f" "$kept/home/" || true; done
    die "the check box failed; its logs are kept in $kept (box.log, home/*.log), the box taken down"
  fi
fi
step "Done. Try: omabox up && omabox shot"
