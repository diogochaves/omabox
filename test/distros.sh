#!/usr/bin/env bash
# test/distros.sh [IMAGE...]: install.sh and omabox doctor on other distributions (#247), each in a
# throwaway rootless podman container: the image's own packages, this checkout's tracked files (as
# they are in the working tree), install.sh run until it passes (each command it names run as the
# container's root, which stands in for the user), then `omabox doctor`. Starts no box. Default
# images: archlinux (Arch without Omarchy), debian:sid, opensuse/tumbleweed. Logs in
# ${XDG_STATE_HOME:-~/.local/state}/omabox/distros/<image>.log; exit 1 when one failed.
#
# The container gets what a box gets and no more (AGENTS.md's box invariant): the first render node
# and /dev/net/tun, never /dev/dri/card*, /dev/input, the runtime dir or the system bus. Seccomp and
# the masked /proc paths are off inside it only, so bwrap and pasta work there as on a host, and it
# may grant CAP_SYS_NICE (in its own user namespace), which Debian's Hyprland carries as a file
# capability: without it the container cannot exec Hyprland at all.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
command -v podman >/dev/null || { echo "distros.sh: needs podman" >&2; exit 2; }
node=$(compgen -G "/dev/dri/renderD*" | head -n 1) || node=
[ -n "$node" ] || { echo "distros.sh: no render node (/dev/dri/renderD*)" >&2; exit 2; }
[ $# -gt 0 ] || set -- docker.io/library/archlinux:latest docker.io/library/debian:sid docker.io/opensuse/tumbleweed
logs=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/distros
mkdir -p "$logs"

# Inside: refresh the package lists, then install.sh until it passes; each `sudo CMD` it stops with is
# run without sudo, made non-interactive.
# shellcheck disable=SC2016 # expanded in the container
inner='
set -uo pipefail
mkdir -p /work /tmp/rt && chmod 700 /tmp/rt && tar -xf - -C /work
export XDG_RUNTIME_DIR=/tmp/rt
if command -v pacman >/dev/null; then pacman -Syu --noconfirm >/dev/null
elif command -v apt-get >/dev/null; then apt-get update -qq >/dev/null
elif command -v zypper >/dev/null; then
  # (download.opensuse.org serves a half-synced snapshot at times; its CDN does not)
  sed -i "s|http://download.opensuse.org|https://cdn.opensuse.org|" /etc/zypp/repos.d/*.repo
  zypper -q --gpg-auto-import-keys refresh >/dev/null
fi
for i in 1 2 3 4; do
  echo "==> install.sh (run $i)"
  out=$(/work/install.sh 2>&1 </dev/null) && { echo "$out"; ok=1; break; }
  echo "$out"; ok=0
  cmd=$(printf "%s\n" "$out" | grep -o "sudo [a-z-]* .*$" | tail -n 1 | sed "s/^sudo //")
  [ -n "$cmd" ] || break
  case $cmd in
    pacman*) cmd="$cmd --noconfirm" ;;
    apt\ install*) cmd="env DEBIAN_FRONTEND=noninteractive apt-get install -y -q ${cmd#apt install }" ;;
    zypper\ install*) cmd="zypper -n -q install ${cmd#zypper install }" ;;
  esac
  echo "==> as root: $cmd"
  $cmd >/tmp/pkg.log 2>&1 || { tail -n 20 /tmp/pkg.log; break; }
done
# (setup on its own too: install.sh may have stopped before it, at the aquamarine step)
echo "==> omabox setup"
/work/bin/omabox setup </dev/null; echo "setup exit $?"
echo "==> omabox doctor"
/work/bin/omabox doctor; echo "doctor exit $?"
/work/bin/omabox doctor --json > /tmp/doctor.json; echo "doctor --json exit $?"
cat /tmp/doctor.json; echo
echo "==> versions"
Hyprland --version 2>&1 | head -n 1; labwc --version 2>&1 | head -n 1; /work/bin/omabox --version
[ "$ok" = 1 ]
'

rc=0
for img in "$@"; do
  log=$logs/$(basename "${img%%:*}")-${img##*:}.log
  [[ $img == *:* ]] || log=$logs/$(basename "$img").log
  echo "== $img (log: $log)"
  if git -C "$ROOT" ls-files -z | (cd "$ROOT" && tar --null -cf - -T - 2>/dev/null) |
    podman run --rm -i --security-opt seccomp=unconfined --security-opt label=disable \
      --security-opt unmask=ALL --cap-add SYS_NICE --device /dev/net/tun --device "$node" "$img" bash -c "$inner" > "$log" 2>&1; then
    echo "   install.sh passed; $(grep -E '^(all [0-9]+ checks|[0-9]+ (failed|warnings?))' "$log" | tail -n 1)"
  else
    rc=1
    echo "   FAILED: $(grep -E 'install.sh: |failed' "$log" | tail -n 1)"
  fi
done
exit $rc
