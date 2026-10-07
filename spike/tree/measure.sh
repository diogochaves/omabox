#!/usr/bin/env bash
# Spike #144: the numbers in NOTES finding 244, from one fresh headless box.
#   spike/tree/measure.sh [OUTDIR]
# For each app: its AT-SPI tree (atspi-tree.py, run in the box), a `shot --window` of it and that
# shot's OCR (ocr.sh, on the host: it reads a PNG). One line per app on stdout; the trees, shots
# and OCR text are left in OUTDIR. Needs at-spi2-core, python-gobject, qt6-base (qmake6, a C++
# compiler), qt6-declarative (qml6), quickshell, chromium, nautilus, foot, tesseract, ImageMagick.
# VS Code is measured when `code` is installed.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HERE=$ROOT/spike/tree
OUT=${1:-$(mktemp -d)}
mkdir -p "$OUT"
ob() { "$ROOT/bin/omabox" -b "$B" "$@"; }
B=$("$ROOT/bin/omabox" up --new --env ATSPI_DBUS_IMPLEMENTATION=dbus-daemon | tail -1)
trap 'ob down >/dev/null 2>&1 || true' EXIT
echo "box $B; out $OUT" >&2

# Qt reads org.a11y.Status.IsEnabled at start (and follows it after): what a screen reader sets.
ob run -- busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true

qmake6 -o "$OUT/qtw/Makefile" "$HERE/qt-widgets-demo/qt-widgets-demo.pro" >/dev/null 2>&1
make -s -C "$OUT/qtw" >/dev/null
cp "$OUT/qtw/qt-widgets-demo" "$(ob path)/home/"

tree() { ob run -- /usr/bin/python3 "$HERE/atspi-tree.py" "$@"; }

# measure NAME SEL APP_RE [--float WxH] -- CMD...
measure() {
  local name=$1 sel=$2 app=$3 size=""
  shift 3
  if [ "${1:-}" = --float ]; then size=$2; shift 2; fi
  shift # --
  ob run -d --wait -- "$@" >/dev/null 2>&1 || true
  ob wait --timeout 20s window "$sel" >/dev/null 2>&1 || true
  if [ -n "$size" ]; then
    local addr
    addr=$(ob windows | awk -v s="$sel" 'tolower($0) ~ tolower(s) { print $1; exit }')
    ob hyprctl dispatch "hl.dsp.window.float({ window = 'address:$addr' })" >/dev/null
    ob hyprctl dispatch "hl.dsp.window.resize({ window = 'address:$addr', x = ${size%x*}, y = ${size#*x} })" >/dev/null
  fi
  ob wait --timeout 15s still >/dev/null 2>&1 || true
  tree "$app" --stats >"$OUT/$name.tree" 2>"$OUT/$name.tree-stats" || true
  ob shot --window "$sel" -o "$OUT/$name.png" >/dev/null 2>&1 || true
  local ocr="(no shot)"
  [ -s "$OUT/$name.png" ] && ocr=$("$HERE/ocr.sh" "$OUT/$name.png" 2>&1 >"$OUT/$name.ocr" | tail -1)
  printf '%-14s tree: %s\n%-14s ocr:  %s\n' "$name" "$(tail -1 "$OUT/$name.tree-stats")" "" "$ocr"
  ob run -- pkill -f -- "$(basename "$1")" >/dev/null 2>&1 || true
  ob wait --timeout 10s window "$sel" --gone >/dev/null 2>&1 || true
}

measure gtk4-demo TreeDemo python3 --float 480x420 -- /usr/bin/python3 "$HERE/gtk4-demo.py"
measure gtk4-tiled TreeDemo python3 -- /usr/bin/python3 "$HERE/gtk4-demo.py"
measure qt-widgets qt-widgets-demo qt-widgets-demo --float 480x420 -- /home/sbx/qt-widgets-demo
measure qml org.qt-project.qml 'Qml Runtime' --float 480x420 -- qml6 "$HERE/qml-demo.qml"
measure nautilus nautilus nautilus -- nautilus /usr/share/omarchy/shell/plugins
measure designer 'title:^Qt Widgets Designer$' Designer -- /usr/lib/qt6/bin/designer
measure chromium-off chromium Chromium -- chromium --user-data-dir=/home/sbx/chr --no-first-run \
  --no-default-browser-check "file://$HERE/web-demo.html"
measure chromium chromium Chromium -- chromium --force-renderer-accessibility \
  --user-data-dir=/home/sbx/chr --no-first-run --no-default-browser-check "file://$HERE/web-demo.html"
measure chromium-text chromium Chromium -- chromium --force-renderer-accessibility \
  --user-data-dir=/home/sbx/chr --no-first-run --no-default-browser-check file:///usr/share/doc/bash/bash.html
if command -v code >/dev/null; then
  measure vscode-off code '^code$' -- code --user-data-dir /home/sbx/vsc --disable-workspace-trust "$HERE"
  measure vscode code '^code$' -- code --force-renderer-accessibility --user-data-dir /home/sbx/vsc \
    --disable-workspace-trust "$HERE"
fi
measure foot foot foot -- foot

# Quickshell: Omarchy's own shell (started with the box, IsEnabled set after: restarted so it reads
# it), its menu open, and a config of our own with a FloatingWindow and a PanelWindow.
ob restart-shell >/dev/null
ob run -- omarchy-shell shell summon omarchy.menu >/dev/null
ob wait --timeout 10s still >/dev/null 2>&1 || true
tree quickshell --raw --stats >"$OUT/quickshell.tree" 2>"$OUT/quickshell.tree-stats" || true
printf '%-14s tree: %s\n' omarchy-shell "$(tail -1 "$OUT/quickshell.tree-stats")"
ob run -- omarchy-shell shell hide omarchy.menu >/dev/null || true
ob run -d -- env QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 quickshell -n -p "$HERE/qs-demo" >/dev/null
ob wait --timeout 10s window 'Tree demo Quickshell' >/dev/null 2>&1 || true
tree --raw >"$OUT/apps.txt" || true
printf '%-14s %s\n' qs-demo "$(grep -c "quickshell pid=.* windows=\[\]" "$OUT/apps.txt") quickshell app objects on the bus, none with a window"
ob run -- ps -eo rss,args | awk '/at-spi|accessibility.conf/ && !/awk/ { kb += $1 } END { printf "a11y processes: %d MB RSS\n", kb / 1024 }'
