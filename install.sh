#!/usr/bin/env bash
# install.sh: set up omabox on an Omarchy machine, or on another Linux with Hyprland 0.56+ (#247).
# Idempotent: re-run after pulling.
#
#   ./install.sh           packages (on Arch, sudo pacman only if some are missing, and Arch's rust
#                          when there is no cargo; on another distribution, the command that installs
#                          them, then it stops), the tools and omabox itself (cargo, into build/omabox),
#                          omabox's aquamarine build (for the soname the system's Hyprland links; when
#                          it cannot be built, boxes run on the system's, said), then omabox setup:
#                          links, settings dir, the agent guard (asked)
#   ./install.sh --check   the same, then start a box, screenshot it and tear it down
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
# What install.sh installs, by the distribution's family (doctor's: crates/omabox/src/cmd/doctor.rs,
# whose unit test holds these lists to its own names). Arch's are the ones Omarchy machines run;
# Debian sid's and openSUSE Tumbleweed's were installed in containers (NOTES finding 646); Fedora 45's
# in a VM (finding 665; no Hyprland there but COPR's); NixOS's are named, not installed.
PKGS_arch=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret   # run a box
  imagemagick wl-clipboard wf-recorder ffmpeg                            # shot's options, check, clip, record (Omarchy has them)
  quickshell gtk3 xdg-terminal-exec dbus                                 # in a box (Omarchy has them)
  passt                                                                  # every box's network (pasta)
  python                                                                 # the agent guard's Codex check, a jailed box's covers
  wayland libxkbcommon base-devel pkgconf libpng at-spi2-core glib2                    # tools/ (pixman below)
  git cmake ninja hyprwayland-scanner hyprutils seatd libdisplay-info hwdata libinput   # aquamarine
  libdrm mesa pixman
)
PKGS_debian=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret-tools imagemagick wl-clipboard wf-recorder ffmpeg
  quickshell libgtk-3-0t64 xdg-terminal-exec dbus dbus-daemon dbus-bin dbus-user-session
  passt
  python3
  build-essential pkg-config libwayland-dev libxkbcommon-dev libpng-dev libpixman-1-dev libatspi2.0-dev libglib2.0-dev
  git cmake ninja-build hyprwayland-scanner libhyprutils-dev libseat-dev libdisplay-info-dev hwdata libinput-dev
  libdrm-dev libgbm-dev libegl-dev libgles-dev libgl-dev libopengl-dev libudev-dev wayland-protocols
)
PKGS_fedora=(
  labwc wlr-randr bubblewrap util-linux iproute jq grim gnome-keyring libsecret ImageMagick wl-clipboard wf-recorder ffmpeg-free
  gtk3 xdg-terminal-exec dbus-daemon dbus-tools                          # (no quickshell but COPR's)
  passt
  python3
  gcc make pkgconf-pkg-config wayland-devel libxkbcommon-devel libpng-devel pixman-devel at-spi2-core-devel glib2-devel
  git cmake ninja-build hyprwayland-scanner-devel gcc-c++ hyprutils-devel libseat-devel libdisplay-info-devel hwdata-devel libinput-devel
  libdrm-devel mesa-libgbm-devel mesa-libEGL-devel mesa-libGL-devel libglvnd-devel systemd-devel wayland-protocols-devel
)
PKGS_suse=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring secret-tool ImageMagick wl-clipboard wf-recorder ffmpeg
  libgtk-3-0 xdg-terminal-exec dbus-1 dbus-1-daemon dbus-1-tools         # (no quickshell)
  passt
  python3
  gcc make pkgconf wayland-devel libxkbcommon-devel libpng16-compat-devel libpixman-1-0-devel at-spi2-core-devel glib2-devel
  git cmake ninja hyprwayland-scanner gcc-c++ hyprutils-devel seatd-devel libdisplay-info-devel hwdata libinput-devel
  libdrm-devel libgbm-devel Mesa-libEGL-devel Mesa-libGL-devel Mesa-libGLESv2-devel Mesa-libGLESv3-devel systemd-devel wayland-protocols-devel
)
PKGS_nix=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret imagemagick wl-clipboard wf-recorder ffmpeg
  quickshell gtk3 xdg-terminal-exec dbus
  passt
  python3
  gcc gnumake pkg-config wayland wayland-scanner libxkbcommon libpng pixman at-spi2-core glib
  git cmake ninja hyprwayland-scanner
)

step() { printf '\n==> %s\n' "$*"; }
die() { echo "install.sh: $*" >&2; exit 1; }

# The distribution's family, as doctor tells it: os-release's ID, else the first of ID_LIKE's words
# that is one (Omarchy: ID=omarchy, ID_LIKE=arch); with neither, the package manager there.
# (OMABOX_TEST_OS_RELEASE: the suite's stand-in for /etc/os-release.)
os_family() {
  local f=${OMABOX_TEST_OS_RELEASE:-/etc/os-release} id="" like="" w
  [ -r "$f" ] || f=/usr/lib/os-release
  if [ -r "$f" ]; then
    id=$(sed -n 's/^ID=//p' "$f" | tr -d "\"'") || id=
    like=$(sed -n 's/^ID_LIKE=//p' "$f" | tr -d "\"'") || like=
  fi
  for w in $id $like; do
    case $w in
      arch) echo arch; return ;;
      debian | ubuntu) echo debian; return ;;
      fedora | rhel | centos) echo fedora; return ;;
      suse | sles | opensuse*) echo suse; return ;;
      nixos) echo nix; return ;;
    esac
  done
  for w in pacman:arch apt-get:debian dnf:fedora zypper:suse nixos-rebuild:nix; do
    if command -v "${w%%:*}" >/dev/null; then echo "${w#*:}"; return; fi
  done
}

check=0
case ${1:-} in --check) check=1 ;; "") ;; *) die "usage: ./install.sh [--check]" ;; esac
family=$(os_family)
case $family in
  arch) PKGS=("${PKGS_arch[@]}") install="sudo pacman -S --needed" rust=rust ;;
  debian) PKGS=("${PKGS_debian[@]}") install="sudo apt install" rust=cargo ;;
  fedora) PKGS=("${PKGS_fedora[@]}") install="sudo dnf install" rust=cargo ;;
  suse) PKGS=("${PKGS_suse[@]}") install="sudo zypper install" rust=cargo ;;
  nix) PKGS=("${PKGS_nix[@]}") install="" rust=cargo ;;
  *) die "this distribution is not one omabox knows the packages of (Arch, Debian, Fedora, openSUSE, NixOS):" \
    "install Hyprland 0.56+ and what omabox doctor names, then build by hand (docs/install.md)" ;;
esac
[ "$family" != arch ] || command -v pacman >/dev/null || die "this expects Arch/Omarchy (pacman)"
# NixOS has no /usr/bin, where a box's start looks for every program it runs (#247).
[ "$family" != nix ] || die "NixOS is not supported yet: a box's start looks for its programs in /usr/bin." \
  "What it would need, in environment.systemPackages: hyprland ${PKGS[*]} cargo"
command -v Hyprland >/dev/null || die "Hyprland is not installed${install:+: $install hyprland}"
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
# (sort -V, not pacman's vercmp: every distribution's coreutils has it; Hyprland says X.Y.Z only)
# A Hyprland that does not run says no version: what it says instead (Debian's, with its file
# capability, in a container that may not grant it: "Operation not permitted").
[ -n "$hv" ] || die "Hyprland --version says no version: $(Hyprland --version 2>&1 | head -n 1 || true)"
[ "$(printf '%s\n' 0.56.0 "$hv" | sort -V | head -n 1)" = 0.56.0 ] || die "Hyprland $hv is too old: omabox needs 0.56+ (Lua config)"

step "Packages"
case $family in
  arch) mapfile -t missing < <(pacman -T "${PKGS[@]}" || true) ;;
  debian) mapfile -t missing < <(for p in "${PKGS[@]}"; do
    [ "$(dpkg-query -W -f='${db:Status-Abbrev}' "$p" 2>/dev/null)" = "ii " ] || echo "$p"
  done) ;;
  # (--whatprovides: a name another package provides, as python3 on openSUSE, counts)
  fedora | suse) mapfile -t missing < <(for p in "${PKGS[@]}"; do
    rpm -q --whatprovides "$p" >/dev/null 2>&1 || echo "$p"
  done) ;;
esac
# omabox is built from crates/ (#324): the distribution's rust, unless a rustup or another cargo is
# already on PATH.
command -v cargo >/dev/null || missing+=("$rust")
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}"
  # Elsewhere than Arch, no sudo of install.sh's: the command, to run and come back (#247).
  [ "$family" = arch ] || die "run this, then $0 again: $install ${missing[*]}"
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

# A checkout builds it into build/prefix, for every box this checkout starts: #415 where the system's
# lacks it (headless NVIDIA boxes, confirm-close; finding 125) and omabox's own patches (interactive
# boxes; finding 247), from the source for the soname Hyprland links (0.15.1's .so.14, 0.16's .so.15;
# finding 655). Nothing to do while boxes use this omabox's build; a soname omabox has no source for
# is said and skipped by setup itself. A build that fails is no reason to stop (boxes can run on the
# system's aquamarine, finding 655): said, with what boxes use and lose.
step "omabox's aquamarine build (hyprwm/aquamarine#415 and omabox's own fixes)"
if ! "$ROOT/bin/omabox" setup --aquamarine; then
  uses=$("$ROOT/bin/omabox" --version 2>&1 | sed -n 's/^aquamarine: //p') || uses=
  echo "install.sh: omabox's aquamarine build failed (above); going on without it. Boxes use ${uses:-no aquamarine omabox could find (omabox doctor says why)}. Interactive boxes go without omabox's own fixes (keys held when focus leaves, the pointer and cursor in a monitor's window, a class for each monitor's window). Try again with: omabox setup --aquamarine" >&2
fi

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
