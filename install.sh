#!/usr/bin/env bash
# install.sh: set up omabox on an Omarchy machine. Idempotent: re-run after pulling.
#
#   ./install.sh           packages (sudo only if some are missing), patched aquamarine, tools, links
#   ./install.sh --check   the same, then start a box, screenshot it and tear it down
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
# "wayland: fix configure not applying sometimes (#415)": needed until a release has it (NOTES finding 4).
AQ_COMMIT=7bb8bdf4
AQ_SRC=$ROOT/build/aquamarine
AQ_PREFIX=$ROOT/build/prefix
PKGS=(
  labwc wlr-randr bubblewrap util-linux iproute2 jq grim gnome-keyring libsecret   # run a box
  quickshell gtk3 xdg-terminal-exec dbus                                 # in a box (Omarchy has them)
  passt                                                                  # up --net isolated (pasta)
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

step "Patched aquamarine ($AQ_COMMIT) in build/prefix"
if [ "$(cat "$AQ_PREFIX/.omabox-commit" 2>/dev/null)" = "$AQ_COMMIT" ]; then
  echo "already built"
else
  [ -d "$AQ_SRC/.git" ] || git clone -q https://github.com/hyprwm/aquamarine "$AQ_SRC"
  git -C "$AQ_SRC" cat-file -e "$AQ_COMMIT^{commit}" 2>/dev/null || git -C "$AQ_SRC" fetch -q origin
  git -C "$AQ_SRC" checkout -q "$AQ_COMMIT"
  cmake -S "$AQ_SRC" -B "$AQ_SRC/out" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$AQ_PREFIX" >/dev/null
  cmake --build "$AQ_SRC/out"
  cmake --install "$AQ_SRC/out" >/dev/null
  echo "$AQ_COMMIT" > "$AQ_PREFIX/.omabox-commit"
fi
# The nested Hyprland picks the private build up through LD_LIBRARY_PATH, which only works while it
# has the soname Hyprland links against. After a Hyprland/aquamarine upgrade this is what breaks.
want=$(ldd "$(command -v Hyprland)" 2>/dev/null | awk '/libaquamarine/ {print $1; exit}' || true)
[ -n "$want" ] || die "cannot tell which libaquamarine Hyprland links (ldd $(command -v Hyprland))"
[ -e "$AQ_PREFIX/lib/$want" ] || die "Hyprland links $want but build/prefix has $(cd "$AQ_PREFIX/lib" && ls libaquamarine.so.*): bump AQ_COMMIT"
echo "Hyprland links $want: ok"

step "Tools"
# (Not `make && echo`: set -e ignores a failure on the left of &&, and install.sh would carry on.)
for t in pointer keyboard wlfd peek; do
  make -s -C "$ROOT/tools/$t" || die "building tools/$t failed"
  echo "tools/$t"
done

step "Links"
# ln -sfn onto a real directory would put the link inside it and leave the old copy in use.
link() {
  [ -L "$2" ] || [ ! -e "$2" ] || die "$2 exists and is not a link: move it away, then re-run"
  mkdir -p "$(dirname "$2")"
  ln -sfn "$1" "$2"
}
link "$ROOT/bin/omabox" "$HOME/.local/bin/omabox"
echo "$HOME/.local/bin/omabox -> bin/omabox"
# The skill goes everywhere Omarchy puts its own agent skills (omarchy-provision-user): the shared
# ~/.agents/skills (OpenCode and other Agent Skills readers) and Claude Code always; Codex, pi and
# Hermes when they are installed (their dir exists).
linked=()
for dir in .agents .claude .codex .pi/agent .hermes; do
  case $dir in .agents|.claude) ;; *) [ -d "$HOME/$dir" ] || continue ;; esac
  link "$ROOT/skill" "$HOME/$dir/skills/omabox"
  # shellcheck disable=SC2088 # display text, not a path
  linked+=("~/$dir")
done
echo "agent skill 'omabox' -> ${linked[*]} (skills/)"
# The bar widget: linked so a pull updates it; it stays off until `omarchy plugin enable chaves.omabox`.
link "$ROOT/plugin" "$HOME/.config/omarchy/plugins/chaves.omabox"
echo "bar widget chaves.omabox -> ~/.config/omarchy/plugins (enable: omarchy plugin enable chaves.omabox)"
# Where `omabox config` keeps the settings. The widget watches the file, and a watch only sees the
# file appear if its directory already exists (finding 72).
mkdir -p "$HOME/.config/omabox"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "note: add ~/.local/bin to PATH" ;; esac

# The agent guard (findings 65-67) changes the agents' own settings (Claude Code, Codex), so it is
# only ever asked for, in a terminal, yes by default; `omabox guard` shows it, `omabox guard off`
# takes it out again. A "no" is remembered: a re-run after a pull does not ask again, where an Enter
# out of habit would turn it on (finding 74).
if [ -d "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" ] || [ -d "${CODEX_HOME:-$HOME/.codex}" ]; then
  step "Agent guard"
  state=$("$ROOT/bin/omabox" guard) || true
  states=$(grep -E '^(Claude Code|Codex|claude|codex)' <<<"$state" || true)
  echo "$states"
  if grep -qv ': on$' <<<"$states"; then
    echo "It gives your agents' shell commands a display that does not exist, so a window, hyprctl or"
    echo "grim outside a box fails instead of reaching your desktop. omabox keeps working, and"
    echo "\`omabox host -- CMD\` runs what you ask for on the real desktop. See: omabox guard"
    declined=$HOME/.config/omabox/guard-declined
    if [ -e "$declined" ]; then
      echo "not asked (you said no before; rm $declined to be asked again): omabox guard on"
    elif [ -t 0 ] && [ -t 1 ]; then
      read -r -p "Turn it on? [Y/n] " yn
      if [[ $yn = [nN]* ]]; then
        mkdir -p "$(dirname "$declined")" && date -Is > "$declined"
      else
        "$ROOT/bin/omabox" guard on || echo "the guard is not (fully) on, see above; the rest of the install is done"
      fi
    else
      echo "not asked (no terminal): omabox guard on"
    fi
  fi
fi

if [ $check = 1 ]; then
  step "Check: a box up, a screenshot, down"
  name=install-check-$$
  trap '"$ROOT/bin/omabox" down "$name" >/dev/null 2>&1 || true' EXIT
  "$ROOT/bin/omabox" up "$name"
  shot=$("$ROOT/bin/omabox" shot -b "$name")
  echo "screenshot: $shot"
fi
step "Done. Try: omabox up && omabox shot"
