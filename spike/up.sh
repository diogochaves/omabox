#!/usr/bin/env bash
# spike/up.sh <dir> : start a sandbox whose /run/user/1000 is <dir>/run and HOME is <dir>/home
set -euo pipefail
D=$1; SPIKE=$(cd "$(dirname "$0")" && pwd); mkdir -p "$D/run" "$D/home"
exec 3>"$D/info.json"
exec env -i PATH=/usr/bin TERM=dumb LANG=${LANG:-C.UTF-8} bwrap --info-fd 3 --die-with-parent --unshare-pid --unshare-ipc --unshare-uts \
  --ro-bind /usr /usr --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib /lib64 --ro-bind /etc /etc \
  --proc /proc --dev /dev --dev-bind /dev/dri/renderD128 /dev/dri/renderD128 --ro-bind /sys /sys \
  --tmpfs /tmp --bind "$D/run" /run/user/1000 --bind "$D/home" /home/sbx --ro-bind "$HOME/code" "$HOME/code" --ro-bind "$SPIKE" /spike --ro-bind "$SPIKE/../build/prefix/lib" /opt/omabox/lib \
  --setenv XDG_RUNTIME_DIR /run/user/1000 --setenv HOME /home/sbx \
  dbus-run-session -- bash /spike/session.sh
