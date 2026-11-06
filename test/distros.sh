#!/usr/bin/env bash
# test/distros.sh [--boxes] [IMAGE...]: install.sh and omabox doctor on other distributions (#247),
# each in a throwaway rootless podman container: the image's own packages, this checkout's tracked
# files (as they are in the working tree), install.sh run as a user of the container until it passes
# (each command it names run as the container's root, which stands in for that user's sudo), then
# `omabox setup` and `omabox doctor`. Default images: archlinux (Arch without Omarchy), debian:sid,
# opensuse/tumbleweed. Logs in ${XDG_STATE_HOME:-~/.local/state}/omabox/distros/<image>.log; exit 1
# when one failed.
#
# --boxes then starts real boxes there, as that user (test/distros-boxes.sh, the acceptance run of
# #247): a terminal and a font installed first (foot and DejaVu, which install.sh does not name), and
# the distribution's wf-recorder and ffmpeg (#442), then a plain box driven and recorded, and a plain
# box with a Quickshell bar as its shell. Its shots, and the logs of a
# box whose step failed, are copied out to <image>/ next to the log.
#
# The container gets what a box gets and no more (AGENTS.md's box invariant): the first render node
# and /dev/net/tun, never /dev/dri/card*, /dev/input, the runtime dir, the system bus, a Wayland or X11
# socket; no --privileged, no host network. Seccomp and the masked /proc paths are off inside it only,
# so bwrap and pasta work there as on a host, and it may grant CAP_SYS_NICE (in its own user
# namespace), which Debian's Hyprland carries as a file capability: without it the container cannot
# exec Hyprland at all. --init gives it a PID 1 that reaps, as systemd does on a host: a box's pasta
# and reaper outlive the command that started them.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
boxes=0
[ "${1:-}" != --boxes ] || { boxes=1; shift; }
command -v podman >/dev/null || { echo "distros.sh: needs podman" >&2; exit 2; }
node=$(compgen -G "/dev/dri/renderD*" | head -n 1) || node=
[ -n "$node" ] || { echo "distros.sh: no render node (/dev/dri/renderD*)" >&2; exit 2; }
[ $# -gt 0 ] || set -- docker.io/library/archlinux:latest docker.io/library/debian:sid docker.io/opensuse/tumbleweed
logs=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/distros
mkdir -p "$logs"

# Inside, as root: refresh the package lists, make the user (uid 1000, its runtime dir), then
# install.sh as the user until it passes; each `sudo CMD` it stops with is run as root, made
# non-interactive.
# shellcheck disable=SC2016 # expanded in the container
inner='
set -uo pipefail
mkdir -p /work && tar -xf - -C /work
useradd -m -u 1000 user && chown -R user:user /work
mkdir -p /run/user/1000 && chown user:user /run/user/1000 && chmod 700 /run/user/1000
as_user() { runuser -u user -- env HOME=/home/user XDG_RUNTIME_DIR=/run/user/1000 \
  PATH=/home/user/.local/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin "$@"; }
if command -v pacman >/dev/null; then pacman -Syu --noconfirm >/dev/null
elif command -v apt-get >/dev/null; then apt-get update -qq >/dev/null
elif command -v zypper >/dev/null; then
  # (download.opensuse.org serves a half-synced snapshot at times; its CDN does not)
  sed -i "s|http://download.opensuse.org|https://cdn.opensuse.org|" /etc/zypp/repos.d/*.repo
  zypper -q --gpg-auto-import-keys refresh >/dev/null
fi
pkg() {
  case $1 in
    pacman) set -- "$@" --noconfirm ;;
    apt) shift 2; set -- env DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$@" ;;
    zypper) shift 2; set -- zypper -n -q install "$@" ;;
  esac
  echo "==> as root: $*"
  "$@" >/tmp/pkg.log 2>&1 || { tail -n 20 /tmp/pkg.log; return 1; }
}
for i in 1 2 3 4; do
  echo "==> install.sh (run $i)"
  out=$(as_user /work/install.sh 2>&1 </dev/null) && { echo "$out"; ok=1; break; }
  echo "$out"; ok=0
  cmd=$(printf "%s\n" "$out" | grep -o "sudo [a-z-]* .*$" | tail -n 1 | sed "s/^sudo //")
  [ -n "$cmd" ] || break
  pkg $cmd || break
done
# (setup on its own too: install.sh may have stopped before it, at the aquamarine step)
echo "==> omabox setup"
as_user omabox setup </dev/null; echo "setup exit $?"
echo "==> omabox doctor"
as_user omabox doctor; echo "doctor exit $?"
as_user omabox doctor --json > /tmp/doctor.json; echo "doctor --json exit $?"
cat /tmp/doctor.json; echo
echo "==> versions"
Hyprland --version 2>&1 | head -n 1; labwc --version 2>&1 | head -n 1
qs --version 2>&1 | head -n 1; as_user omabox --version
[ "$ok" = 1 ] || exit 1
[ "$BOXES" = 1 ] || exit 0
echo "==> boxes: a terminal and a font (install.sh names neither), wf-recorder and ffmpeg"
if command -v pacman >/dev/null; then pkg pacman -S --needed foot ttf-dejavu wf-recorder ffmpeg
elif command -v apt-get >/dev/null; then pkg apt install foot fonts-dejavu-core wf-recorder ffmpeg
else pkg zypper install foot dejavu-fonts wf-recorder ffmpeg
fi || exit 1
as_user bash /work/test/distros-boxes.sh /home/user/out; rc=$?
cp -r /home/user/out/. /out/ 2>/dev/null
exit $rc
'

rc=0
for img in "$@"; do
  base=$(basename "${img%%:*}")-${img##*:}
  [[ $img == *:* ]] || base=$(basename "$img")
  log=$logs/$base.log out=$logs/$base
  rm -rf "$out" && mkdir -p "$out"
  echo "== $img (log: $log)"
  if git -C "$ROOT" ls-files -z | (cd "$ROOT" && tar --null -cf - -T - 2>/dev/null) |
    podman run --rm -i --init --security-opt seccomp=unconfined --security-opt label=disable \
      --security-opt unmask=ALL --cap-add SYS_NICE --device /dev/net/tun --device "$node" \
      -e BOXES=$boxes -v "$out:/out" "$img" bash -c "$inner" > "$log" 2>&1; then
    echo "   install.sh passed; $(grep -E '^(all [0-9]+ checks|[0-9]+ (failed|warnings?))' "$log" | tail -n 1)"
    [ $boxes = 0 ] || echo "   $(grep -E '^boxes: ' "$log" | tail -n 1); shots in $out"
  else
    rc=1
    echo "   FAILED: $(grep -E 'install.sh: |failed|^FAIL|^boxes: ' "$log" | tail -n 1)"
  fi
  rmdir "$out" 2>/dev/null || true
done
exit $rc
