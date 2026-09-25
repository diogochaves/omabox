#!/usr/bin/env bash
# shellcheck disable=SC2016 # single-quoted $VARS are expanded inside the box
# omabox's regression suite: what NOTES' findings verified, as checks that run in real boxes.
#
#   test/run.sh              every test (~4 min; boxes are named t<pid>-*, all torn down)
#   test/run.sh unit         only the fast ones (no box)
#   test/run.sh PATTERN...   tests whose name matches any PATTERN (e.g. isolated systemd); a PATTERN
#                            that matches no test exits 2
#   --strict (or OMABOX_TEST_STRICT=1): a skipped check (a tool this machine lacks) fails
#
# Never touches the real desktop: every box is headless. The host's Hyprland is only read: its event
# socket is listened to for the whole run, and a window, focus change or virtual keyboard of the
# suite's showing up there fails the test that was running (t_leak_control first proves that on a box
# standing in for the host); the end checks that the host's focused workspace and window are what
# they were, or changed by events that were not the suite's (you working meanwhile). Pointer motion
# has no event: it is only seen when it moves focus. Needs a Hyprland session, python3, and the host
# ports 8093/8094 free (a throwaway HTTP server for the network tests).
# A test that runs no check fails, and so does a full run with fewer checks than MIN_CHECKS.
# Each run keeps a folder (the last 5 runs are kept) in ~/.local/state/omabox/test/: its provenance
# and, for a test's first failure, what its boxes showed then (screen, windows, focus, pointer,
# devices, logs) and every failure's full output.
set -uo pipefail
# The guard tests point HOME at a temp dir; these would still lead them to the real settings.
unset CLAUDE_CONFIG_DIR CODEX_HOME

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CLI=$ROOT/bin/omabox
P=t$$                      # box name prefix
export OMABOX_SUITE=$P     # marks what the suite starts on the host, for the leak detector
TMP=$(mktemp -d)
pass=0 fail=0 failed=() skips=()
STRICT=${OMABOX_TEST_STRICT:-0}
# A full run's floor (360 checks, 175 of them unit, on 2026-09-25): a test that silently stops
# checking shows up here even when everything that did run passed.
MIN_CHECKS=340 MIN_UNIT=170
EVID=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/test/$(date +%Y%m%d-%H%M%S)-$P

cleanup() {
  local b
  for b in $("$CLI" ls --json 2>/dev/null | jq -r '.[].name' | grep "^$P-"); do "$CLI" down "$b" >/dev/null 2>&1; done
  [ -n "${HTTP_PID:-}" ] && kill "$HTTP_PID" 2>/dev/null
  [ -n "${WATCH_PID:-}" ] && kill "$WATCH_PID" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

ob() { "$CLI" "$@"; }
# Notes from inside checks (a slow wait), whose output is captured: shown before the next result.
note() { printf '       (%s)\n' "$*" >> "$TMP/notes"; }
notes() { [ ! -s "$TMP/notes" ] || { cat "$TMP/notes"; : > "$TMP/notes"; }; }
ok() { notes; pass=$((pass + 1)); printf '  \e[32mok\e[0m   %s\n' "$1"; }
no() {
  notes; fail=$((fail + 1)); failed+=("$CUR: $1"); printf '  \e[31mFAIL\e[0m %s\n' "$1"
  [ -n "${2:-}" ] && printf '       %s\n' "${2:0:300}"
  evidence "$1" "${2:-}"
}
# What a failed test left, for reading after the run: every failure's whole output, and at its first
# failure, while its boxes are still up, each live $P-* box's screen, windows, layers, focus, pointer,
# devices and log tails, and the last wait that timed out.
evidence() {
  local e=$EVID/${CUR:-none} b d q f bd
  mkdir -p "$e" 2>/dev/null || return 0
  printf '%s\n%s\n\n' "$1" "$2" >> "$e/failures.txt"
  [ ! -e "$e/boxes" ] || return 0
  mkdir -p "$e/boxes"
  [ ! -s "$TMP/until.last" ] || cp "$TMP/until.last" "$e/last-wait.txt"
  slice "$EVID/host-events.log" "$CUR" > "$e/host-events.log" 2>/dev/null
  for b in $("$CLI" ls --json 2>/dev/null | jq -r --arg p "$P-" '.[] | select((.name | startswith($p)) and .state == "up") | .name'); do
    d=$e/boxes/$b; mkdir -p "$d"; bd=$("$CLI" path "$b")
    timeout 10 "$CLI" shot -b "$b" -o "$d/screen.png" >/dev/null 2>&1
    for q in clients layers activewindow activeworkspace devices; do timeout 5 "$CLI" hyprctl -b "$b" -j "$q" > "$d/$q.json" 2>&1; done
    timeout 5 "$CLI" hyprctl -b "$b" cursorpos > "$d/cursorpos" 2>&1
    for f in "$bd/box.log" "$bd"/home/*.log "$bd"/run/hypr/*/hyprland.log "$bd/run/events.log"; do [ -f "$f" ] && tail -n 100 "$f" > "$d/${f##*/}"; done
  done
  printf '       evidence: %s\n' "$e"
}
# check NAME CMD...: the command must succeed. check_eq NAME WANT GOT. check_fails NAME CMD...
check() { local n=$1; shift; local out; if out=$("$@" 2>&1); then ok "$n"; else no "$n" "$out"; fi; }
check_eq() { if [ "$2" = "$3" ]; then ok "$1"; else no "$1" "want [$2] got [$3]"; fi; }
check_fails() { local n=$1; shift; local out; if out=$("$@" 2>&1); then no "$n" "succeeded: $out"; else ok "$n"; fi; }
check_match() { if [[ $3 =~ $2 ]]; then ok "$1"; else no "$1" "want /$2/ got [$3]"; fi; }
# skip NAME WHY: a check this machine cannot run (a missing tool or file), counted and listed at the
# end instead of passing unseen; under --strict it fails.
skip() {
  skips+=("$CUR: $1 ($2)"); printf '  \e[33mskip\e[0m %s (%s)\n' "$1" "$2"
  [ "$STRICT" = 0 ] || no "$1" "skipped under --strict: $2"
}
now_ms() { local u=${EPOCHREALTIME//[!0-9]/}; echo $((u / 1000)); }
to_ms() { local s=${1%.*} f=0; [[ $1 != *.* ]] || f=${1#*.}000; echo $((10#$s * 1000 + 10#${f:0:3})); }
# until_ok T CMD...: poll the command until it succeeds, T seconds at most. Timing out, it says so with
# the command's last output (kept for the evidence too); a wait that needed over half of T is noted:
# a slower machine may need more.
until_ok() {
  local t=$1; shift; local t0 now out lim; t0=$(now_ms) lim=$(to_ms "$t")
  until out=$("$@" 2>&1); do
    now=$(now_ms)
    if [ $((now - t0)) -ge "$lim" ]; then
      printf 'until_ok %s %s\nlast output: %s\n' "$t" "$*" "$out" > "$TMP/until.last"
      echo "timed out after ${t}s: $*; last output: ${out:0:200}"
      return 1
    fi
    sleep 0.2
  done
  now=$((($(now_ms) - t0) / 100))
  [ $((now * 200)) -le "$lim" ] || note "slow wait: $((now / 10)).$((now % 10))s of ${t}s for: $*"
}
# holds T CMD...: the command succeeds now and keeps succeeding for T seconds (a "stays so" check: one
# look after a sleep misses a state that changed and came back).
holds() {
  local t=$1; shift; local t0 lim; t0=$(now_ms) lim=$(to_ms "$t")
  while "$@" >/dev/null 2>&1; do
    [ $(($(now_ms) - t0)) -lt "$lim" ] || return 0
    sleep 0.2
  done
  echo "held for $(($(now_ms) - t0)) ms of ${t}s: $*"; return 1
}
# For until_ok: nothing of box NAME runs any more (its dir in no command line); its reaper is gone.
none_running() { ! pgrep -f "$XDG_RUNTIME_DIR/omabox/$1/" >/dev/null; }
reaper_gone() { ! pgrep -f "omabox _reap $1 " >/dev/null; }
# A fresh git repo named $P-SUFFIX: a throwaway `run` from it gets a name no box of the user's has
# (from $ROOT it would be "omabox", which `omabox up` in the checkout also takes).
tmp_repo() { local r=$TMP/$P-$1; mkdir -p "$r" && git -C "$r" init -q && echo "$r"; }

# The CLI's functions without its dispatch at the end, for the pure ones (a file: it finds its ROOT
# from its own path).
mkdir -p "$TMP/lib/bin"
sed '/^main "\$@"; exit$/d' "$CLI" > "$TMP/lib/bin/omabox"
lib() { bash -c 'source "$1"; shift; "$@"' lib "$TMP/lib/bin/omabox" "$@"; }

# --- the leak detector (finding 80) --------------------------------------------------------------

# Listens to a Hyprland's event socket (.socket2.sock: read-only, what a bar listens to; nothing is
# ever written to it) and logs, timestamped, the events a box's input or windows would cause there.
# On a focus change it asks that compositor which window has focus (`hyprctl -j activewindow`, as the
# end-of-run checks do) and reads that client's /proc environ for OMABOX_SUITE / OMABOX_NAME: the
# window is the suite's, or a box's, when they are set. Stops when its socket closes or PARENT goes.
# The same code watches the host and, in t_leak_control, a box standing in for it.
#   python3 -c "$WATCHER" SOCKET [PARENT]
WATCHER='
import json, os, re, select, socket, subprocess, sys, time
def say(s):
    sys.stdout.write("%.3f %s\n" % (time.time(), s)); sys.stdout.flush()
path, parent = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 0
try:
    os.chdir(os.path.dirname(path))   # a box dir path is past the 108 bytes of a socket address
    s = socket.socket(socket.AF_UNIX); s.connect(os.path.basename(path))
except OSError as e:
    say("== watcher failed: %s" % e); sys.exit(1)
KEEP = ("openwindow>>", "closewindow>>", "activewindowv2>>", "workspacev2>>", "activelayout>>")
def focused():
    try:
        w = json.loads(subprocess.run(["hyprctl", "-j", "activewindow"], capture_output=True, text=True, timeout=5).stdout)
    except Exception as e:
        return "? %s" % e
    pid, cls, tags = w.get("pid", -1), w.get("class"), []
    try:
        with open("/proc/%d/environ" % pid, "rb") as f:
            for kv in f.read().split(b"\0"):
                k, _, v = kv.decode(errors="replace").partition("=")
                if k in ("OMABOX_SUITE", "OMABOX_NAME"): tags.append("%s=%s" % (k, v))
    except Exception:
        tags.append("environ=unreadable")
    if cls == "aquamarine":   # an interactive box: its bwrap binds <box dir>/run; no title names it
        try:
            m = re.search(rb"/omabox/([A-Za-z0-9_.-]+)/run\0", open("/proc/%d/cmdline" % pid, "rb").read())
            if m: tags.append("box=%s" % m.group(1).decode())
        except Exception:
            pass
    if cls == "omabox-peek": cls += " title=%s" % w.get("title")   # "omabox peek: NAME"
    return " ".join(["%s pid=%s" % (w.get("address"), pid)] + tags + ["class=%s" % cls])
say("== watching")
buf = b""
while True:
    ready = select.select([s], [], [], 2)[0]
    if parent and not os.path.exists("/proc/%d" % parent): break
    if not ready: continue
    d = s.recv(65536)
    if not d:
        say("== the event socket closed"); break
    *lines, buf = (buf + d).split(b"\n")
    for l in lines:
        l = l.decode(errors="replace")
        if not l.startswith(KEEP): continue
        say(l)
        if l.startswith("activewindowv2>>") and l != "activewindowv2>>": say("~ " + focused())
'

# leak_scan PREFIX < LOG: what in a watcher's log is the suite's (a box's name starts with PREFIX, the
# suite's own processes carry OMABOX_SUITE=${PREFIX%-}) or omabox's, one `leak: ...` line each, and one
# `note: ...` line for the rest (the user's own windows and workspace switches while the suite runs).
# Windows of omabox's own: an interactive box (class aquamarine; its title names no box, so any counts,
# unless it is your own box opened meanwhile) and peek (class omabox-peek, "omabox peek: NAME"; the
# tool started by hand is "omabox peek"). A virtual keyboard's layout event is `omabox keys` reaching
# that compositor; OMABOX_TEST_HOST_KEYBOARDS (a regex) names your own (wayvnc, an input method).
leak_scan() {
  local pre=$1 line data cls title kb notes=()
  while IFS= read -r line; do
    line=${line#* }
    case $line in
      openwindow\>\>*)
        IFS=, read -r _ _ cls title <<<"${line#*>>}"
        if [ "$cls" = aquamarine ] && data=$(their_new_box "$pre"); then notes+=("window of your box $data")
        elif data=$(omabox_window "$pre" "$cls" "$title"); then echo "leak: a window opened: $data"
        else notes+=("window $cls"); fi ;;
      '~ '*)
        cls=${line#* class=} title=""
        [[ $cls != *' title='* ]] || { title=${cls#* title=}; cls=${cls%% title=*}; }
        if [[ " $line " == *" OMABOX_SUITE=${pre%-} "* || $line == *" OMABOX_NAME=$pre"* || $line == *" box=$pre"* ]]; then
          echo "leak: focus went to a window of this run's: ${line#\~ }"
        elif [[ $line == *" OMABOX_NAME="* ]]; then
          echo "leak: focus went to a box's process (another run's, or yours): ${line#\~ }"
        elif [[ $line == *" box="* ]]; then data=${line#* box=}; notes+=("focus on your box ${data%% *}")
        elif data=$(omabox_window "$pre" "$cls" "$title"); then echo "leak: focus went to $data"
        else notes+=("focus $cls"); fi ;;
      activelayout\>\>*virtual-keyboard*)
        kb=${line#*>>}; kb=${kb%%,*}
        if [ -n "${OMABOX_TEST_HOST_KEYBOARDS:-}" ] && [[ $kb =~ $OMABOX_TEST_HOST_KEYBOARDS ]]; then notes+=("keyboard $kb")
        else echo "leak: keys from a virtual keyboard ($kb)"; fi ;;
      workspacev2\>\>*) data=${line#*>>}; notes+=("workspace ${data#*,}") ;;
    esac
  done
  [ ${#notes[@]} = 0 ] || echo "note: not the suite's: $(printf '%s\n' "${notes[@]}" | awk '!seen[$0]++ && n++ < 8' | paste -sd, - | sed 's/,/, /g')"
}
# omabox_window PREFIX CLASS TITLE: says what the window is when it is one of omabox's that is not the
# user's (TITLE empty: unknown).
omabox_window() {
  case $2 in
    aquamarine) echo "an interactive box's window ($2${3:+ \"$3\"})" ;;
    omabox-peek)
      case $3 in
        "omabox peek: $1"*|"omabox peek"|"") echo "a peek window${3:+ \"$3\"}" ;;
        *) return 1 ;;
      esac ;;
    *) return 1 ;;
  esac
}
# An interactive box of the user's (not PREFIX*) started during this run: whose window a new
# aquamarine window most likely is (that event names no box).
their_new_box() {
  find "$XDG_RUNTIME_DIR/omabox" -mindepth 2 -maxdepth 2 -name box.json -newer "$EVID/provenance" \
    -exec jq -r --arg p "$1" 'select(.mode == "interactive" and (.name | startswith($p) | not)) | .name' {} + 2>/dev/null | sed -n 1p | grep .
}
# slice FILE FROM [TO]: a watcher log's lines after the marker "== FROM" (up to "== TO", or the end).
slice() { awk -v a="== $2" -v b="== ${3:-}" '{m = substr($0, index($0, " ") + 1)} f && m == b {exit} f; m == a {f = 1}' "$1"; }
# After each test: what of it reached the host's desktop fails it; what is not the suite's is shown.
host_scan() {
  local out; out=$(slice "$EVID/host-events.log" "$1" | leak_scan "$P-")
  [[ $out != *leak:* ]] || no "nothing of it reached the host desktop" "$(grep '^leak:' <<<"$out")"
  [[ $out != *note:* ]] || printf '       (host, %s)\n' "$(grep '^note:' <<<"$out" | cut -c7-)"
}
# host_same NAME WANT GOT workspace|window: the host's focus at the end is what it was, or it changed
# through events that were not the suite's (you working meanwhile): the watcher saw the change, and
# nothing of the suite's reached the host all run. (A workspace switch has no owner in its event: one
# made by a leak and seen alone would pass here; the per-test scans still see what came with it.)
host_same() {
  local seen="" log=$EVID/host-events.log last
  if [ "$2" = "$3" ]; then ok "$1"; return; fi
  if [ "$4" = workspace ]; then grep -q "^[0-9.]* workspacev2>>$3," "$log" && seen=1
  else
    last=$(grep -E '^[0-9.]* (~ |activewindowv2>>$)' "$log" | tail -n 1 | cut -d' ' -f2-)
    { [ -n "$3" ] && [[ $last == "~ $3 "* ]]; } || { [ -z "$3" ] && [ "$last" = "activewindowv2>>" ]; } && seen=1
  fi
  if [ -n "$seen" ] && ! leak_scan "$P-" < "$log" | grep '^leak:' >/dev/null; then
    ok "$1 by the suite (it is $3 now: focus events that were not the suite's)"
  elif [ -n "$seen" ]; then no "$1" "want [$2] got [$3]"
  else no "$1" "want [$2] got [$3]; no event to it in the host's log"; fi
}

# --- tests ---------------------------------------------------------------------------------------

t_unit_parse_mode() {
  check_eq "WxH is @60" "1920x1080@60" "$(lib parse_mode 1920x1080)"
  check_eq "WxH@HZ kept" "3440x1440@144" "$(lib parse_mode 3440x1440@144)"
  check_eq "fractional HZ" "2560x1440@59.95" "$(lib parse_mode 2560x1440@59.95)"
  check_eq "Hz normalised (@60.0 is @60)" "1280x720@60" "$(lib parse_mode 1280x720@60.0)"
  check_fails "junk refused" lib parse_mode bogus
  check_fails "0 Hz refused" lib parse_mode 1x1@0
  check_fails "zero width refused" lib parse_mode 0x1080
  check_match "host = the focused monitor" '^[0-9]+x[0-9]+@[0-9.]+$' "$(lib parse_mode host)"
}

t_unit_duration() {
  check_eq "90s" 90 "$(lib duration 90s)"
  check_eq "30m" 1800 "$(lib duration 30m)"
  check_eq "2h" 7200 "$(lib duration 2h)"
  check_eq "bare minutes" 2700 "$(lib duration 45)"
  check_eq "0 = never" 0 "$(lib duration 0)"
  check_fails "junk refused" lib duration 2d
  check_eq "leading zero is decimal (010 = 10 min)" 600 "$(lib duration 010)"
  check_eq "90s reads 90s" 90s "$(lib human_duration 90)"
  check_eq "7200 reads 2h" 2h "$(lib human_duration 7200)"
}

# finding 63: what may go into a box, and where.
t_unit_mount_rules() {
  check "HOME refused" lib refuse_src "$HOME"
  check "HOME/.ssh refused" lib refuse_src "$HOME/.ssh"
  check "HOME/.config refused (holds omarchy's api keys)" lib refuse_src "$HOME/.config"
  check "the runtime dir refused" lib refuse_src "$XDG_RUNTIME_DIR"
  check "/run/user refused (contains it)" lib refuse_src /run/user
  check "/tmp refused" lib refuse_src /tmp
  check_fails "a repo inside /tmp is fine" lib refuse_src "$TMP"
  check_fails "a plugin inside ~/.config/omarchy/plugins is fine" lib refuse_src "$HOME/.config/omarchy/plugins/x"
  check "HOME/.config/omarchy itself refused (api keys)" lib refuse_src "$HOME/.config/omarchy"
  check "/ refused" lib refuse_src /
  check_fails "a repo is fine" lib refuse_src "$ROOT"
  check_fails "mise's installs are fine" lib refuse_src "${MISE_DATA_DIR:-$HOME/.local/share/mise}/installs"
  check "dest /home refused (hides /home/sbx)" lib refuse_dest /home
  check "dest /opt refused (hides /opt/omabox)" lib refuse_dest /opt
  check "dest /usr/x refused" lib refuse_dest /usr/x
  check_fails "dest /sbx is fine" lib refuse_dest /sbx
  check_fails "dest /mnt/x is fine" lib refuse_dest /mnt/x
  check_fails "dest inside /tmp is fine (a repo in /tmp)" lib refuse_dest /tmp/x/repo
  check "dest /tmp refused" lib refuse_dest /tmp
  check "dest /tmp/.X11-unix refused" lib refuse_dest /tmp/.X11-unix
  check_eq "//usr normalised to /usr" "/usr" "$(lib robind_spec "$ROOT://usr" | cut -f2)"
}

t_unit_refusals() {
  # Refused before anything is created: no box dir may appear.
  check_fails "--ro-bind HOME refused" ob up "$P-r1" --ro-bind "$HOME"
  check_fails "--ro-bind onto /usr refused" ob up "$P-r2" --ro-bind "$ROOT:/usr/x"
  check_fails "--ro-bind onto /run refused" ob up "$P-r3" --ro-bind "$ROOT:/run/x"
  check_fails "--ro-bind onto box HOME refused" ob up "$P-r4" --ro-bind "$ROOT:/home/sbx"
  check_fails "--allow needs --net isolated" ob up "$P-r5" --allow 8081
  check_fails "--net junk refused" ob up "$P-r6" --net internet
  check_fails "--allow junk refused" ob up "$P-r7" --net isolated --allow 80a
  check_fails "--env without = refused" ob up "$P-r8" --env FOO
  check_fails "--size junk refused" ob up "$P-r9" --size huge
  check_fails "--idle junk refused" ob up "$P-r10" --idle soon
  check_fails "--overlay HOME refused" ob up "$P-r11" --overlay "$HOME"
  check_fails "--ro-bind the runtime dir refused (host session bus)" ob up "$P-r12" --ro-bind "$XDG_RUNTIME_DIR:/mnt/rt"
  check_fails "--ro-bind /tmp refused" ob up "$P-r13" --ro-bind /tmp
  check_fails "--ro-bind onto /opt refused" ob up "$P-r14" --ro-bind "$ROOT:/opt"
  check_fails "--ro-bind onto //usr refused" ob up "$P-r15" --ro-bind "$ROOT://usr"
  check_fails "two names refused" ob up "$P-r16" "$P-r17"
  check_fails "--plugin HOME refused" bash -c "mkdir -p '$TMP/fakehome' && echo '{\"id\":\"x.y\"}' > '$TMP/fakehome/manifest.json' && HOME='$TMP/fakehome' '$CLI' up '$P-r18' --plugin '$TMP/fakehome'"
  local left; left=$(ob ls --json | jq -r '.[].name' | grep -c "^$P-r" || true)
  check_eq "refusals left no box behind" 0 "$left"
}

t_unit_run_named_dead() {
  check_fails "run -b on a box that is not up fails (no throwaway)" ob run -b "$P-nope" -- true
}

# finding 66: an edit to bin/omabox (linked from ~/.local/bin) while a command runs must not change
# what that command runs. A copy with a pause, rewritten in place mid-run, as an editor would.
t_unit_live_edit() {
  mkdir -p "$TMP/live/bin"
  sed 's/^main() {$/main() { sleep 1/' "$CLI" > "$TMP/live/bin/omabox"; chmod +x "$TMP/live/bin/omabox"
  "$TMP/live/bin/omabox" help > "$TMP/live/out" 2>&1 & local p=$!
  sleep 0.3
  python3 -c 'import sys; p = sys.argv[1]; n = len(open(p).read()); open(p, "w").write("#!/bin/bash\n" + "q\n" * n)' "$TMP/live/bin/omabox"
  wait $p; local rc=$?
  check_eq "an in-place edit mid-run does not break the running command" 0 "$rc"
  check_match "...which finishes as it started" "omabox up" "$(cat "$TMP/live/out")"
  check_eq "session.sh is one block (it runs for the box's life)" "}" "$(tail -n 1 "$ROOT/share/session.sh")"
}

# Settings (finding 70): `omabox config` in a HOME of the test's own; values that reach the host's Lua
# are refused unless they are one of the known shapes.
t_unit_config() {
  local h=$TMP/confhome; mkdir -p "$h"
  cfg() { HOME=$h "$CLI" config "$@" 2>&1; }
  check_eq "defaults" "workspace=9 confirm-close=off bar-icon=always" "$(cfg | tr '\n' ' ' | sed 's/ $//')"
  check_eq "set a workspace" workspace=4 "$(cfg workspace 4)"
  check_eq "special is the scratchpad" workspace=special:scratchpad "$(cfg workspace special)"
  check_eq "special:NAME" workspace=special:omabox "$(cfg workspace special:omabox)"
  check_eq "confirm-close yes reads on" confirm-close=on "$(cfg confirm-close yes)"
  check_eq "bar-icon auto" bar-icon=auto "$(cfg bar-icon auto)"
  check_fails "bar-icon takes auto or always" env HOME="$h" "$CLI" config bar-icon sometimes
  check_eq "--json for the widget" '{"workspace":"special:omabox","confirm-close":"on","bar-icon":"auto"}' "$(HOME=$h "$CLI" config --json | jq -c .)"
  local bad; for bad in 0 100 "3 silent" "special:a'b" "special:" "1;x" "special:$(printf 'x%.0s' {1..33})"; do
    check_fails "workspace '$bad' refused" env HOME="$h" "$CLI" config workspace "$bad"
  done
  check_eq "...and nothing changed" special:omabox "$(cfg workspace)"
  check_fails "unknown key refused" env HOME="$h" "$CLI" config nope 1
  check_eq "default removes it" workspace=9 "$(cfg workspace default)"
  check_eq "the rest of the file is kept" "confirm-close=on bar-icon=auto" "$(grep -v '^#' "$h/.config/omabox/config" | tr '\n' ' ' | sed 's/ $//')"
  echo "workspace=9'); os.execute('x" >> "$h/.config/omabox/config"
  check_match "a bad value in the file is ignored, said" "ignoring workspace" "$(cfg workspace)"
  check_eq "...and the default used" 9 "$(HOME=$h "$CLI" config workspace 2>/dev/null)"
  check_fails "up --workspace needs --interactive" "$CLI" up "$P-cfg" --workspace 3
  check_fails "up --workspace refuses a bad one" "$CLI" up "$P-cfg" --interactive --workspace "9 silent"
  check_fails "up --new picks the name: one given is refused" "$CLI" up "$P-cfg" --new
  # A config linked from a dotfiles repo stays a link (finding 74)
  mv "$h/.config/omabox/config" "$h/dotconfig"; ln -s "$h/dotconfig" "$h/.config/omabox/config"
  cfg workspace 3 >/dev/null
  check "a linked config stays a link" test -L "$h/.config/omabox/config"
  check "...and its target changes" grep -qx workspace=3 "$h/dotconfig"
  # The host writes into a box's runtime dir, which the box can write too: never through what is there
  # (finding 74). A link to a FIFO hung `omabox config`; a link to a host file would be written through.
  local d=$TMP/fakebox; mkdir -p "$d/run"; mkfifo "$d/fifo"; echo keep > "$d/host-file"
  ln -s "$d/fifo" "$d/run/omabox.confirm-close"; ln -s "$d/host-file" "$d/run/omabox.mode"
  check "a flag over a link to a FIFO is written, not blocked on" timeout 5 bash -c 'source "$1"; box_file "$2/run/omabox.confirm-close" on' _ "$TMP/lib/bin/omabox" "$d"
  lib box_file "$d/run/omabox.mode" 1280x720@60
  check_eq "...over a link to a host file: that file untouched" keep "$(cat "$d/host-file")"
  check "...the flag a plain file now" test -f "$d/run/omabox.mode" -a ! -L "$d/run/omabox.mode"
}

# seed_home's copies of the user's config dirs (finding 74): a link inside is kept as a link, never
# followed (a theme linked from its repo, a link to api-keys.env); no .git, no hidden files.
t_unit_seed_copy() {
  local s=$TMP/seed; mkdir -p "$s/repo/.git" "$s/term" "$s/out"
  echo secret > "$s/api-keys.env"; echo token > "$s/repo/.env"; echo url > "$s/repo/.git/config"
  echo colors > "$s/repo/colors.toml"; echo conf > "$s/term/foot.ini"
  ln -s "$s/api-keys.env" "$s/term/keys"; ln -s "$s/repo" "$s/theme"
  lib seed_copy "$s/theme" "$s/out/theme"
  lib seed_copy "$s/term" "$s/out/term"
  check_eq "a linked dir is copied" colors "$(cat "$s/out/theme/colors.toml")"
  check_fails "...without its hidden files" test -e "$s/out/theme/.env"
  check_fails "...or .git" test -e "$s/out/theme/.git"
  check "a link inside stays a link" test -L "$s/out/term/keys"
  check_fails "...with nothing of its target in the box HOME" grep -rqs secret "$s/out"
}

# up --new (finding 73): a free box-N name, printed; two at once never get the same one. The names
# are the user's namespace too (box-1 may be theirs): only the ones printed here are taken down.
t_new() {
  local a b; a=$(mktemp -p "$TMP") b=$(mktemp -p "$TMP")
  "$CLI" up --new --no-shell --idle 0 > "$a" 2>/dev/null & local pa=$!
  "$CLI" up --new --no-shell --idle 0 > "$b" 2>/dev/null & local pb=$!
  wait $pa $pb
  local na nb; na=$(cat "$a") nb=$(cat "$b")
  check_match "up --new prints a box-N name" '^box-[0-9]+$' "$na"
  check "...two at once, two different names ($na, $nb)" test -n "$nb" -a "$na" != "$nb"
  check "...both up" bash -c "'$CLI' ls --json | jq -e '[.[] | select(.name == \"$na\" or .name == \"$nb\") | select(.state == \"up\")] | length == 2'"
  [ -n "$na" ] && ob down "$na" >/dev/null 2>&1; [ -n "$nb" ] && ob down "$nb" >/dev/null 2>&1
}

# One version for the CLI and the widget (CHANGELOG.md).
t_unit_version() {
  local v; v=$(cat "$ROOT/VERSION")
  check_eq "omabox --version" "omabox $v" "$("$CLI" --version)"
  check_eq "the widget's manifest has it" "$v" "$(jq -r .version "$ROOT/plugin/manifest.json")"
  check_match "...and its Settings face" "pluginVersion: \"$v\"" "$(grep pluginVersion "$ROOT/plugin/Panel.qml")"
  check_match "...and the changelog" "^## $v " "$(grep "^## $v " "$ROOT/CHANGELOG.md")"
}

# The leak detector's reading of events (finding 80), on lines as the watcher logs them (the
# interactive window's openwindow is Hyprland 0.56's, seen in a stand-in box).
t_unit_leak_scan() {
  scan() { printf '1.000 %s\n' "$@" | leak_scan t1-; }
  leaks() { scan "$@" | grep '^leak:'; }
  check_match "a box of this run's taking focus" "^leak: focus went to a window of this run's: .*OMABOX_NAME=t1-main" "$(scan '~ 0xa pid=5 OMABOX_NAME=t1-main class=foot')"
  check_match "...a process the suite started" "^leak: focus went to a window of this run's" "$(scan '~ 0xa pid=5 OMABOX_SUITE=t1 class=foot')"
  check_match "...another run's box: a leak too, said so" "^leak: .*another run's" "$(scan '~ 0xa pid=5 OMABOX_NAME=t12-main class=foot')"
  check_eq "...another suite's process is not this one's" "" "$(leaks '~ 0xa pid=5 OMABOX_SUITE=t12 class=foot')"
  check_match "an interactive box's window opening" "^leak: a window opened: an interactive box's window" "$(scan 'openwindow>>5fc37ecc6c80,9,aquamarine,aquamarine - WAYLAND-1')"
  check_match "focus on an interactive box of this run's" "^leak: focus went to a window of this run's" "$(scan '~ 0xa pid=5 environ=unreadable box=t1-x class=aquamarine')"
  check_match "...on one of yours: a note" "^note: .*focus on your box mine$" "$(scan '~ 0xa pid=5 box=mine class=aquamarine')"
  check_match "a peek of this run's box" "^leak: a window opened: a peek window" "$(scan 'openwindow>>a,9,omabox-peek,omabox peek: t1-b')"
  check_match "...the peek tool started on its own" "^leak: focus went to a peek window" "$(scan '~ 0xa pid=5 class=omabox-peek title=omabox peek')"
  check_eq "...a peek of yours is not a leak" "" "$(leaks 'openwindow>>a,9,omabox-peek,omabox peek: mine' '~ 0xa pid=5 class=omabox-peek title=omabox peek: mine')"
  check_match "a virtual keyboard's keys" "^leak: keys from a virtual keyboard \(hl-virtual-keyboard-unknown\)" "$(scan 'activelayout>>hl-virtual-keyboard-unknown,English (US)')"
  check_eq "...not one named as yours (OMABOX_TEST_HOST_KEYBOARDS)" "" "$(OMABOX_TEST_HOST_KEYBOARDS='^hl-virtual-keyboard-unknown$' leaks 'activelayout>>hl-virtual-keyboard-unknown,English (US)')"
  check_eq "your own windows, focus and workspaces: one note, no leak" "note: not the suite's: window firefox, focus firefox, workspace 3" \
    "$(scan 'openwindow>>b,3,firefox,a, title' 'activewindowv2>>b' '~ 0xb pid=9 class=firefox' 'workspacev2>>3,3' '~ 0xb pid=9 class=firefox')"
  check_eq "a log slice starts after its marker and ends before the next" "b" \
    "$(printf '1 a\n2 == x\n3 b\n4 == y\n5 c\n' > "$TMP/slice"; slice "$TMP/slice" x y | cut -d' ' -f2)"
}

# The leak detector, proven (finding 80): the watcher the host gets, on a box standing in for the
# host. Quiet, it reports nothing; then a box's window taking focus, a workspace switch and back, and a
# key from `omabox keys` are leaked into the stand-in on purpose, and each must be reported. A clean
# host log means something only then: when this test fails, so does the host's verdict.
t_leak_control() {
  local S=$P-ctl f0=$fail log
  ob up "$S" --no-shell --net isolated >/dev/null 2>&1 || { no "up the stand-in" "failed"; return; }
  log=$(ob path -b "$S")/run/events.log
  ob run -b "$S" -d -- sh -c 'exec python3 -c "$1" "$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock" >> "$XDG_RUNTIME_DIR/events.log" 2>&1' sh "$WATCHER" >/dev/null 2>&1
  check "the watcher listens to the stand-in's events" until_ok 10 grep -q ' == watching$' "$log"
  mark() { ob run -b "$S" -- sh -c 'printf "%s == %s\n" "$(date +%s.%3N)" "$1" >> "$XDG_RUNTIME_DIR/events.log"' sh "$1"; }
  # shellcheck disable=SC2329 # called through until_ok
  reported() { slice "$log" leaks | leak_scan "$P-" | grep -- "$1" >/dev/null; }  # (not -q: its early exit would fail the pipe)
  mark quiet
  ob shot -b "$S" -o "$TMP/ctl.png" >/dev/null 2>&1; ob hyprctl -b "$S" -j clients >/dev/null
  mark leaks
  ob run -b "$S" -d -- foot sleep 60 >/dev/null 2>&1
  until_ok 10 bash -c "'$CLI' hyprctl -b '$S' -j activewindow | jq -e '.class == \"foot\"'"
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '3' })" >/dev/null
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  ob keys -b "$S" a >/dev/null
  check "a box's window taking focus is reported, with the box" until_ok 5 reported "^leak: focus went to a window of this run's: .*OMABOX_NAME=$S class=foot"
  check "a key from omabox keys is reported" until_ok 5 reported '^leak: keys from a virtual keyboard'
  check "a workspace switch and back is noted" until_ok 5 reported '^note: .*workspace 3, workspace 1'
  check_eq "...and while quiet, nothing" "" "$(slice "$log" quiet leaks | leak_scan "$P-")"
  ob down "$S" >/dev/null 2>&1
  [ "$fail" != "$f0" ] || LEAK_PROVEN=1
}

# Every test runs: a t_* function left out of UNIT and BOX would never run, and nobody would notice.
t_unit_registry() {
  check_eq "every t_* function is in UNIT or BOX, once" "$(declare -F | awk '$3 ~ /^t_/ {print $3}' | sort)" \
    "$(printf '%s\n' "${UNIT[@]}" "${BOX[@]}" | sort)"
  check_eq "UNIT has only t_unit_*, BOX none" "" "$(printf '%s\n' "${UNIT[@]}" | grep -v '^t_unit_'; printf '%s\n' "${BOX[@]}" | grep '^t_unit_')"
}

t_unit_cli() {
  check_eq "inside a box OMABOX=1 is not a box name" "$(basename "$ROOT")" "$(cd "$ROOT" && OMABOX=1 OMABOX_NAME=x lib default_name)"
  check_eq "on the host OMABOX names the box" mine "$(OMABOX=mine lib default_name)"
  check_match "run: unknown option named, no box started" "unknown option --interactive" "$(ob run --interactive -- true 2>&1)"
  check_match "run: a throwaway's up error is shown" "--net is host" "$(cd "$(tmp_repo ne)" && env -u OMABOX "$CLI" run --net bogus -- true 2>&1)"
  check_match "run --help is the usage" "omabox up" "$(ob run --help 2>&1)"
  check_eq "path NAME names the box" "$XDG_RUNTIME_DIR/omabox/$P-x" "$(ob path "$P-x")"
  check_fails "path: two names refused" ob path a b
  check_match "unknown command named" "unknown command: shoot" "$(ob shoot 2>&1)"
  check_fails "down --all with a name refused" ob down "$P-x" --all
  check_fails "peek --fps junk refused" ob peek -b "$P-x" --fps "10'"
}

# The uwsm stand-in's logout kills every process it can see: never outside a box. Checked in a bare
# pid namespace without /opt/omabox (where a broken guard could only kill that namespace).
t_unit_uwsm_guard() {
  local out rc=0
  out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --tmpfs /opt --die-with-parent \
        "$ROOT/share/bin/uwsm" stop 2>&1) || rc=$?
  check_eq "uwsm stop refuses outside a box" 1 "$rc"
  check_match "and says why" "not in a box" "$out"
}

# One box for most checks: host network, default size.
t_main() {
  local B=$P-main s0=$SECONDS
  check "up" ob up "$B" --env OMABOX_TEST=yes
  local D; D=$(ob path -b "$B")
  check_eq "box.json mode headless" headless "$(jq -r .mode "$D/box.json")"
  check_eq "box.json size WxH@HZ" "1920x1080@60" "$(jq -r .size "$D/box.json")"
  check_eq "idle default 2h" 7200 "$(jq -r .idle "$D/box.json")"
  check "ls --json lists it up" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\" and .state == \"up\")'"
  # finding 62: up waits for the shell (bar layer + notification server) before returning
  check "bar is up when up returns" bash -c "'$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-bar"
  check "notify-send works right after up" ob run -b "$B" -- notify-send omabox-test
  check "up on a running box is a no-op" ob up "$B"
  # run: exit codes, environment (findings 47, 49, 57, 58), --env
  # shellcheck disable=SC2016 # expanded inside the box
  check_eq "run passes exit codes" 7 "$(ob run -b "$B" -- sh -c 'exit 7'; echo $?)"
  check_eq "USER" "$(id -un)" "$(ob run -b "$B" -- sh -c 'echo $USER')"
  check_eq "HOME is the fake one" /home/sbx "$(ob run -b "$B" -- sh -c 'echo $HOME')"
  check_eq "--env reaches run" yes "$(ob run -b "$B" -- sh -c 'echo $OMABOX_TEST')"
  # --pass (finding 66): the caller's value, as it is, and never on a command line.
  check_eq "run does not pass the caller's variables" unset "$(OMABOX_T66=x ob run -b "$B" -- sh -c 'echo ${OMABOX_T66-unset}')"
  local secret=$'omabox-t66 = with\nnewline'
  check_eq "--pass hands one over, as it is" "$(printf %q "$secret")" \
    "$(OMABOX_T66=$secret ob run -b "$B" --pass OMABOX_T66 -- bash -c 'printf %q "$OMABOX_T66"')"
  (OMABOX_T66=$secret ob run -b "$B" --pass OMABOX_T66 -- sleep 2.066 >/dev/null 2>&1 &)
  until_ok 5 pgrep -fx 'sleep 2.066'   # the run is under way
  check_fails "...never in any command line" grep -qs omabox-t66 /proc/[0-9]*/cmdline
  check_fails "--pass of an unset variable fails (no silent skip)" env -u OMABOX_T66 "$CLI" run -b "$B" --pass OMABOX_T66 -- true
  check_fails "--pass takes a name only" ob run -b "$B" --pass 'A=1' -- true
  check_eq "--pass ROOT is the caller's, not omabox's own (finding 74)" "/x y" "$(ROOT="/x y" ob run -b "$B" --pass ROOT -- sh -c 'echo "$ROOT"')"
  check_fails "--pass of a name only omabox has fails" env -u GUARD "$CLI" run -b "$B" --pass GUARD -- true
  # finding 67: crashes in a box skip systemd-coredump (a core limit of exactly 1 byte), so they never
  # become crash notifications on the real desktop.
  check_eq "run: core limit 1 byte" 1 "$(ob run -b "$B" -- sh -c 'prlimit --pid $$ --core -o SOFT --noheadings | tr -d " "')"
  check_eq "the session too (Hyprland)" 1 "$(ob run -b "$B" -- sh -c 'prlimit --pid "$(pgrep -x Hyprland)" --core -o SOFT --noheadings | tr -d " "')"
  # The session itself (what the bar and apps launched from binds get), through a Hyprland exec.
  ob hyprctl -b "$B" dispatch "hl.dsp.exec_cmd('sh -c \"{ echo \$SHELL; echo \$OMABOX_TEST; echo \$LC_TIME; echo \$PATH; } > /tmp/sess\"')" >/dev/null
  until_ok 5 ob run -b "$B" -- test -s /tmp/sess
  local sess; sess=$(ob run -b "$B" -- cat /tmp/sess)
  check_eq "session SHELL is the login shell" "$(getent passwd "$(id -un)" | cut -d: -f7)" "$(sed -n 1p <<<"$sess")"
  check_eq "session sees --env" yes "$(sed -n 2p <<<"$sess")"
  check_eq "session LC_TIME is the host's" "${LC_TIME:-}" "$(sed -n 3p <<<"$sess")"
  check_match "session PATH has the box's ~/.local/bin early" '^([^:]*:){0,3}/home/sbx/\.local/bin:' "$(sed -n 4p <<<"$sess")"
  # repo read-only at the same path; nothing of HOME
  check "repo visible at its path" ob run -b "$B" -- test -f "$ROOT/bin/omabox"
  check_fails "repo read-only" ob run -b "$B" -- touch "$ROOT/.omabox-test-write"
  check_fails "real HOME invisible" ob run -b "$B" -- test -e "$HOME/.config"
  check_fails "no /dev/input" ob run -b "$B" -- test -e /dev/input
  check_fails "no DRM card node" ob run -b "$B" -- sh -c 'ls /dev/dri/card* >/dev/null 2>&1'
  local screen_name; screen_name=$(ob mode -b "$B" | cut -d' ' -f1)
  if [ "$(jq -r .wayland_screen "$D/box.json")" = true ]; then
    check_eq "NVIDIA uses the private Wayland screen" WAYLAND-1 "$screen_name"
    check "NVIDIA control node is present" ob run -b "$B" -- test -c /dev/nvidiactl
    check_eq "parent output matches the box" "1920 1080" \
      "$(ob run -b "$B" -- env WAYLAND_DISPLAY=wayland-0 wlr-randr --json | jq -r '.[0].modes[] | select(.current) | "\(.width) \(.height)"')"
  else
    check_eq "headless screen" HEADLESS-2 "$screen_name"
    skip "NVIDIA's private Wayland screen, nodes and parent output" \
      "the render node's driver is $(lib render_driver "$(lib render_node)"), not nvidia"
  fi
  # shot
  local png=$TMP/main.png
  check "shot" ob shot -b "$B" -o "$png"
  check_match "shot is 1920x1080" '1920 x 1080' "$(file "$png")"
  # keys into a terminal: Unicode through spare keycodes (finding 55)
  ob run -b "$B" -d -- foot sh -c 'cat > /tmp/typed' >/dev/null
  until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j activewindow | jq -e '.class == \"foot\"'"
  ob keys -b "$B" -t 'Hé ü € 😀' Return >/dev/null
  ob keys -b "$B" ctrl+d >/dev/null
  until_ok 5 ob run -b "$B" -- test -s /tmp/typed
  check_eq "keys types Unicode" 'Hé ü € 😀' "$(ob run -b "$B" -- cat /tmp/typed)"
  # click lands where asked
  ob click -b "$B" 700 400 >/dev/null
  check_eq "click moves the pointer there" "700, 400" "$(ob hyprctl -b "$B" cursorpos)"
  # --wait with the shell (finding 82): the menu's key settles, and its layer is there, then gone
  check "keys --wait super+space settles (the menu)" ob keys -b "$B" --wait super+space
  check "...and wait layer omarchy-menu" ob wait -b "$B" layer omarchy-menu
  ob keys -b "$B" --wait Escape >/dev/null
  check "wait layer omarchy-menu --gone" ob wait -b "$B" layer omarchy-menu --gone
  # mode and gpu (finding 56)
  check_eq "mode changes live" "$screen_name 1280x720@120" "$(ob mode -b "$B" 1280x720@120)"
  ob hyprctl -b "$B" reload >/dev/null; sleep 1
  check_eq "mode survives a reload" "$screen_name 1280x720@120" "$(ob mode -b "$B")"
  check_eq "box.json follows mode" "1280x720@120" "$(jq -r .size "$D/box.json")"
  local gpu; gpu=$(ob gpu -b "$B" 1 --json)
  check_eq "gpu --json names the box" "$B" "$(jq -r .box <<<"$gpu")"
  if [ "$(jq -r .wayland_screen "$D/box.json")" != true ]; then
    check "gpu --json lists Hyprland" jq -e '.percent | has("Hyprland")' <<<"$gpu"
  else
    # NVIDIA's driver may expose no drm-engine-* counters in /proc/*/fdinfo.
    check "gpu --json handles missing driver counters" jq -e '.percent | type == "object"' <<<"$gpu"
  fi
  check_match "gpu first line names the mode" "1280x720@120" "$(ob gpu -b "$B" 1 | head -1)"
  check_eq "mode @60.0 is accepted and kept as @60" "$screen_name 1280x720@60" "$(ob mode -b "$B" 1280x720@60.0)"
  check_eq "box.json keeps the normalised mode" "1280x720@60" "$(jq -r .size "$D/box.json")"
  check_match "shot into a missing dir says so" "no such directory" "$(ob shot -b "$B" -o "$TMP/nope/x.png" 2>&1)"
  check_match "keys takes -b after the tokens" "" "$(ob keys Escape -b "$B" 2>&1)"
  check_match "ls shows idle against the limit" "$B .* [0-9]+m/2h" "$(ob ls)"
  check "ls --json has idle, allow, bar, systemd" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\") | has(\"idle\") and has(\"allow\") and has(\"bar\") and has(\"systemd\")'"
  # a stub CLI in the box HOME's ~/.local/bin wins in run too (as in the session, finding 57)
  printf '#!/bin/sh\necho stub\n' > "$D/home/.local/bin/gh"; chmod +x "$D/home/.local/bin/gh"
  check_eq "run finds the ~/.local/bin stub first" /home/sbx/.local/bin/gh "$(ob run -b "$B" -- sh -c 'command -v gh')"
  rm -f "$D/home/.local/bin/gh"
  # ...but never in place of the box's own programs: a stub quickshell does not replace the bar
  printf '#!/bin/sh\nexec sleep 300\n' > "$D/home/.local/bin/quickshell"; chmod +x "$D/home/.local/bin/quickshell"
  # restart-shell (and it leaves a project's own quickshell alone)
  ob run -b "$B" -- sh -c 'cp /usr/bin/sleep /tmp/quickshell' >/dev/null
  ob run -b "$B" -d -- /tmp/quickshell 300 >/dev/null
  check "restart-shell" ob restart-shell -b "$B"
  rm -f "$D/home/.local/bin/quickshell"
  check "the bar is the Omarchy shell after restart (stub ignored)" bash -c "'$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-bar"
  check "a project's own quickshell survives restart-shell" ob run -b "$B" -- pgrep -f '^/tmp/quickshell 300'
  # run from a directory that exists in the box but is not the caller's: the box HOME instead
  check_eq "run from ~ runs in the box HOME" /home/sbx "$(cd "$HOME" && "$CLI" run -b "$B" -- pwd)"
  check_eq "run from the repo runs in the repo" "$ROOT" "$(cd "$ROOT" && "$CLI" run -b "$B" -- pwd)"
  # Omarchy's terminal setup in the box HOME (finding 69): an app id reaches the terminal, so window
  # rules (About's float/center) match
  if [ -f /usr/share/omarchy/applications/foot.desktop ]; then
    check "foot's desktop entry passes --app-id" grep -q '^X-TerminalArgAppId=' "$D/home/.local/share/applications/foot.desktop"
  else skip "foot's desktop entry passes --app-id" "no Omarchy foot.desktop on this machine"; fi
  if [ -f "$HOME/.config/foot/foot.ini" ]; then check "the user's foot config (theme colours)" test -f "$D/home/.config/foot/foot.ini"
  else skip "the user's foot config (theme colours)" "no ~/.config/foot/foot.ini"; fi
  # ...on top of the stock HOME (/etc/skel), never the user's secrets, and Omarchy's toggles apply
  if [ -d /etc/skel/.config/omarchy ]; then check "the box HOME starts from /etc/skel" test -f "$D/home/.local/state/omarchy/toggles/hypr/flags.lua"
  else skip "the box HOME starts from /etc/skel" "no /etc/skel/.config/omarchy"; fi
  check_fails "no api-keys.env in the box HOME" test -e "$D/home/.config/omarchy/api-keys.env"
  ob run -b "$B" -- omarchy-hyprland-window-gaps-toggle >/dev/null 2>&1
  check "an Omarchy toggle applies (gaps off)" until_ok 5 bash -c "'$CLI' hyprctl -b '$B' -j getoption general:gaps_out | jq -e '.css == \"0 0 0 0\"'"
  ob run -b "$B" -- omarchy-hyprland-window-gaps-toggle >/dev/null 2>&1
  # down cleans everything (finding 50, 59)
  check "down" ob down "$B"
  check_fails "box dir gone" test -e "$D"
  check_fails "box HOME gone" test -e "${XDG_CACHE_HOME:-$HOME/.cache}/omabox/$B"
  check_fails "no reaper left" pgrep -f "omabox _reap $B "
  echo "       (main: $((SECONDS - s0))s)"
}

t_throwaway() {
  # run with no box up: a throwaway box, the repo as a discarded overlay.
  local repo out; repo=$(tmp_repo tw)
  out=$(cd "$repo" && env -u OMABOX "$CLI" run -- sh -c "echo x > '$repo/.omabox-overlay-test' && cat '$repo/.omabox-overlay-test'" 2>&1)
  check_eq "throwaway: writes into the overlay work" x "$out"
  check_fails "throwaway: host checkout unchanged" test -e "$repo/.omabox-overlay-test"
  check_eq "throwaway: no box left" 0 "$(ob ls --json | jq '[.[] | select(.name | test("-run[0-9]+$"))] | length')"
}

t_isolated() {
  local B=$P-iso
  mkdir -p "$TMP/www" && echo hello > "$TMP/www/index.html"
  python3 -m http.server 8093 --bind 127.0.0.1 --directory "$TMP/www" >/dev/null 2>&1 & HTTP_PID=$!
  python3 -m http.server 8094 --bind 127.0.0.1 --directory "$TMP/www" >/dev/null 2>&1 & local other=$!
  until_ok 5 curl -sf --max-time 1 -o /dev/null http://127.0.0.1:8093/ && until_ok 5 curl -sf --max-time 1 -o /dev/null http://127.0.0.1:8094/
  check "up --net isolated --allow 8093" ob up "$B" --net isolated --allow 8093
  check_eq "allowed host port reachable" hello "$(ob run -b "$B" -- curl -s --max-time 3 http://127.0.0.1:8093/)"
  check_fails "other host port unreachable" ob run -b "$B" -- curl -s --max-time 3 http://127.0.0.1:8094/
  check_fails "no internet" ob run -b "$B" -- curl -s --max-time 3 -o /dev/null https://archlinux.org
  check_eq "hostname is the host's (finding 54)" "$(uname -n)" "$(ob run -b "$B" -- uname -n)"
  check "down" ob down "$B"
  kill "$other" 2>/dev/null
}

t_idle() {
  local B=$P-idle
  check "up --idle 10s" ob up "$B" --idle 10s --no-shell
  until_ok 30 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$B\")'"
  check_fails "box went down by itself" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\")'"
  check_match "next command says why" "went down after 10s idle" "$(ob shot -b "$B" 2>&1)"
  ob down "$B" >/dev/null 2>&1
  check_fails "down clears the note" test -e "$XDG_RUNTIME_DIR/omabox/.expired-$B"
}

# The reaper (finding 74): it decides on the box it watches, then waits for the lock in `down`; a box
# started under the name meanwhile is not the one it decided on. And the "closed by its user" file is
# for interactive boxes: a headless one that writes it and dies stays dead, logs kept (finding 71).
t_reap_race() {
  local B=$P-rr lock
  ob up "$B" --idle 10s --no-shell >/dev/null 2>&1 || { no "up" "failed"; return; }
  exec {lock}>"$XDG_RUNTIME_DIR/omabox/.lock-$B"; flock "$lock"   # what an `up` of the name holds
  check "the idle reaper decides and waits for the lock" until_ok 30 pgrep -f "^flock -w 60 [0-9]+$"
  local j=$XDG_RUNTIME_DIR/omabox/$B/box.json
  jq '.created = "a new box"' "$j" > "$j.t" && mv "$j.t" "$j"   # the new box, as `up` writes it
  exec {lock}>&-
  until_ok 10 reaper_gone "$B"   # it had decided: it goes once it had the lock
  check_eq "...and leaves the new box alone" up "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  ob down "$B" >/dev/null 2>&1
  B=$P-rc
  ob up "$B" --idle 20s --no-shell >/dev/null 2>&1 || { no "up" "failed"; return; }
  ob run -b "$B" -- sh -c 'echo 1 > "$XDG_RUNTIME_DIR/omabox.closed"; hyprctl dispatch "hl.dsp.exit()"' >/dev/null 2>&1
  until_ok 20 reaper_gone "$B"   # its first poll after the box died decides, then it goes
  check_eq "a headless box that wrote the closed file and died stays dead" dead "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  check "...its logs kept" test -e "$XDG_RUNTIME_DIR/omabox/$B/box.log"
  ob down "$B" >/dev/null 2>&1
}

t_stock_bar() {
  local B=$P-stock
  check "up --stock-bar" ob up "$B" --stock-bar --no-shell
  check_eq "default layout seeded" "omarchy.workspaces" \
    "$(jq -r '.bar.layout.left[1].id' "$(ob path -b "$B")/home/.config/omarchy/shell.json")"
  check "down" ob down "$B"
}

t_systemd() {
  local B=$P-sd
  check "up --systemd --net isolated" ob up "$B" --systemd --net isolated
  check_match "user manager up" '^(running|degraded)$' "$(ob run -b "$B" -- systemctl --user is-system-running 2>&1)"
  check_eq "only the document portal may fail" "" \
    "$(ob run -b "$B" -- systemctl --user --failed --no-legend --plain | awk '{print $1}' | grep -v '^xdg-document-portal.service$')"
  check "manager has the session environment" bash -c "'$CLI' run -b '$B' -- systemctl --user show-environment | grep -q '^WAYLAND_DISPLAY='"
  ob run -b "$B" -- systemd-run --user --on-active=1 --timer-property=AccuracySec=100ms --unit t1 touch /tmp/fired >/dev/null 2>&1
  check "a timer fires" until_ok 10 ob run -b "$B" -- test -e /tmp/fired
  check "notify-send works" ob run -b "$B" -- notify-send omabox-test
  local scope; scope=$(systemctl --user list-units --no-legend "omabox-$B-*" | awk '{print $1}')
  check_match "host scope exists" "^omabox-$B-" "$scope"
  check "down" ob down "$B"
  check "host scope gone" until_ok 5 bash -c "[ -z \"\$(systemctl --user list-units --no-legend 'omabox-$B-*')\" ]"
}

# A box must not be able to aim the host-side tools elsewhere (finding 63): its runtime dir is its
# own to write, so a socket swapped for a symlink to another compositor's must not be followed.
t_hostile() {
  local A=$P-hostA B=$P-hostB
  check "up A 1024x768" ob up "$A" --size 1024x768 --no-shell
  check "up B 800x600" ob up "$B" --size 800x600 --no-shell
  local DB; DB=$(ob path -b "$B")
  ob run -b "$A" -- sh -c "mv \$XDG_RUNTIME_DIR/wayland-1 \$XDG_RUNTIME_DIR/wayland-orig && ln -s '$DB/run/wayland-1' \$XDG_RUNTIME_DIR/wayland-1" >/dev/null 2>&1
  local out; out=$(ob shot -b "$A" -o "$TMP/hostile.png" 2>&1) || true
  if [ -s "$TMP/hostile.png" ]; then
    check_fails "shot does not follow the box's symlink to another screen" grep -q '800 x 600' <<<"$(file "$TMP/hostile.png")"
  else ok "shot does not follow the box's symlink to another screen (it failed instead)"; fi
  # a tampered omabox.env: a WAYLAND_DISPLAY with a path in it is refused
  ob run -b "$A" -- sh -c 'tr "\0" "\n" < $XDG_RUNTIME_DIR/omabox.env | sed "s|^WAYLAND_DISPLAY=.*|WAYLAND_DISPLAY=../../../run/user/1000/wayland-1|" | tr "\n" "\0" > $XDG_RUNTIME_DIR/e && mv $XDG_RUNTIME_DIR/e $XDG_RUNTIME_DIR/omabox.env'
  check_match "a path in WAYLAND_DISPLAY is refused" "no WAYLAND_DISPLAY" "$(ob hyprctl -b "$A" monitors 2>&1)"
  check "down both" ob down "$A" "$B"
}

# Two `up`s of one name at once: one box, and nothing left behind by `down` (finding 63).
t_race() {
  local B=$P-race
  ob up "$B" --no-shell >/dev/null 2>&1 & local a=$!
  ob up "$B" --no-shell >/dev/null 2>&1 & local b=$!
  wait $a; wait $b
  # bwrap is two processes per box (its monitor, and the box's PID 1)
  check_eq "one box" 2 "$(pgrep -fc "bwrap .*--bind $XDG_RUNTIME_DIR/omabox/$B/run " || true)"
  check "down" ob down "$B"
  check "nothing of it left running" until_ok 5 none_running "$B"
}

# A failed `up` does not leave a box running unseen (finding 63): the box is killed, its dir kept for
# the logs, `ls` shows it dead, `down` clears it.
t_failed_up() {
  local B=$P-fail
  local out
  # Disable the shell inside the box while up still expects it. A one-second timeout alone can pass
  # on a fast machine, so it does not reliably exercise failed-start cleanup.
  if out=$(env OMABOX_READY_TIMEOUT=2 "$CLI" up "$B" --env OMABOX_SHELL=0 2>&1); then
    no "up fails when the expected shell never starts" "$out"
  else
    ok "up fails when the expected shell never starts"
  fi
  # The namespace and bwrap parent can take a moment to exit after the failed command returns.
  # shellcheck disable=SC2329 # called through until_ok
  failed_box_dead() { [ "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')" = dead ]; }
  # shellcheck disable=SC2329 # called through until_ok
  failed_box_processes_gone() { [ "$(pgrep -fc "bwrap .*--bind $XDG_RUNTIME_DIR/omabox/$B/run " || true)" = 0 ]; }
  check "the box becomes dead, not running" until_ok 10 failed_box_dead
  check "nothing of it remains running" until_ok 10 failed_box_processes_gone
  check "down clears it" ob down "$B"
}

# A box whose Hyprland died reads dead, not up (finding 63).
t_hyprland_dies() {
  local B=$P-hdie
  check "up" ob up "$B" --no-shell
  ob run -b "$B" -- pkill -KILL -x Hyprland >/dev/null 2>&1
  check "the box ends with its Hyprland" until_ok 10 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\" and .state == \"dead\")'"
  ob down "$B" >/dev/null 2>&1
}

# --no-shell is a bare compositor.
t_no_shell() {
  local B=$P-bare
  check "up --no-shell" ob up "$B" --no-shell
  check "no quickshell starts" holds 2 bash -c "! '$CLI' run -b '$B' -- pgrep -x quickshell"
  check "down" ob down "$B"
}

# A stale pid (the box died, the pid now belongs to someone else) is never killed or entered.
t_stale_pid() {
  local B=$P-stale D=$XDG_RUNTIME_DIR/omabox/$P-stale
  sleep 300 & local victim=$!
  mkdir -p "$D"
  echo "{\"child-pid\": $victim}" > "$D/info.json"
  echo '{"name": "x", "mode": "headless", "net": "host", "pidns": "pid:[1]"}' > "$D/box.json"
  check_eq "reads dead" dead "$(ob ls --json | jq -r ".[] | select(.name == \"$B\") | .state")"
  check_fails "run refuses to enter it" ob run -b "$B" -- true
  ob down "$B" >/dev/null 2>&1
  check "down did not kill the process that has its pid now" kill -0 "$victim"
  kill "$victim" 2>/dev/null
}

# An isolated box whose host pid file is missing is still taken down (finding 63).
t_isolated_no_pidfile() {
  local B=$P-isopid
  check "up --net isolated" ob up "$B" --net isolated --no-shell
  rm -f "$XDG_RUNTIME_DIR/omabox/$B/pid"
  check "down" ob down "$B"
  check "nothing of it left running" until_ok 5 none_running "$B"
}

# `run` into a box counts as use; after it expires, `run` says so instead of starting a throwaway.
t_run_idle() {
  local repo=$TMP/$P-exp
  mkdir -p "$repo" && git -C "$repo" init -q
  (cd "$repo" && "$CLI" up --idle 10s --no-shell --net isolated >/dev/null 2>&1)
  for _ in 1 2 3 4 5; do (cd "$repo" && "$CLI" run -- true); sleep 3; done
  check "a box used only through run stays up" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$P-exp\" and .state == \"up\")'"
  until_ok 40 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$P-exp\")'"
  check_match "after expiry run reports it (no throwaway)" "went down after 10s idle" "$(cd "$repo" && "$CLI" run -- true 2>&1)"
  ob down "$P-exp" >/dev/null 2>&1
}

# A throwaway run from ~ (no repo) does not put HOME in the box (finding 63).
t_throwaway_home() {
  # From ~ (no repo) the throwaway is named after "default": a box of the user's with that name
  # would take the run instead.
  if [ -d "$XDG_RUNTIME_DIR/omabox/default" ]; then no "a box named 'default' is up: run it again after omabox down default"; return; fi
  local out; out=$(cd "$HOME" && env -u OMABOX "$CLI" run -- sh -c "test -e '$HOME/.config' && echo LEAK || echo ok; pwd" 2>&1)
  check_match "HOME not visible in a throwaway run from ~" '^ok' "$out"
  check_match "it runs in the box HOME" '/home/sbx$' "$out"
}

# A throwaway run killed outright (SIGKILL: no EXIT trap) does not leave its box behind.
t_throwaway_killed() {
  local repo; repo=$(tmp_repo tk)
  (cd "$repo" && exec env -u OMABOX "$CLI" run -- sleep 300) & local r=$!
  until_ok 30 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name | startswith(\"$P-tk-run\"))'"
  kill -KILL "$r"
  check "its box goes within 15 s" until_ok 15 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name | startswith(\"$P-tk-run\"))'"
}

# The keyboard and pointer tools (finding 64): keys named as binds name them, input checked before any
# of it is sent, spare keycodes X11 apps can see, and a lost box is a failure.
t_keys() {
  local B=$P-keys
  ob up "$B" --no-shell --net isolated --xwayland >/dev/null 2>&1 || { no "up" "failed"; return; }
  ob hyprctl -b "$B" eval "hl.bind('SUPER + CTRL + ALT + J', hl.dsp.exec_cmd('touch /tmp/lower'))
    hl.bind('SUPER + CTRL + ALT + SHIFT + J', hl.dsp.exec_cmd('touch /tmp/upper'))
    hl.bind('SUPER + CTRL + ALT + slash', hl.dsp.exec_cmd('touch /tmp/slash'))" >/dev/null
  ob keys -b "$B" SUPER+CTRL+ALT+J super+ctrl+alt+/ >/dev/null
  check "SUPER+CTRL+ALT+J fires the J bind" until_ok 3 ob run -b "$B" -- test -e /tmp/lower
  check_fails "...not the SHIFT+J one" ob run -b "$B" -- test -e /tmp/upper
  check "a single-character key (super+ctrl+alt+/)" ob run -b "$B" -- test -e /tmp/slash
  ob run -b "$B" -d -- foot sh -c 'cat > /tmp/typed' >/dev/null
  until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j activewindow | jq -e '.class == \"foot\"'"
  check_fails "a bad token fails" ob keys -b "$B" x no_such_key
  check_eq "junk -s refused (was 49 days)" 2 "$(timeout 5 "$CLI" keys -b "$B" -s -1 >/dev/null 2>&1; echo $?)"
  check_eq "junk -s refused (was 0)" 2 "$(ob keys -b "$B" -s abc x >/dev/null 2>&1; echo $?)"
  check_eq "junk --delay refused" 2 "$(ob keys -b "$B" --delay -1 x >/dev/null 2>&1; echo $?)"
  check_match "bad UTF-8 refused" "bad UTF-8" "$(ob keys -b "$B" -t $'\xe9x' 2>&1)"
  ob keys -b "$B" + - / shift+a Return ctrl+d >/dev/null
  until_ok 5 ob run -b "$B" -- test -s /tmp/typed
  check_eq "+ - / typed, nothing from the refused runs" '+-/A' "$(ob run -b "$B" -- cat /tmp/typed)"
  if command -v zenity >/dev/null; then
    ob run -b "$B" -d -- sh -c 'GDK_BACKEND=x11 zenity --entry --text t > /tmp/x11' >/dev/null
    until_ok 15 bash -c "'$CLI' hyprctl -b '$B' -j activewindow | jq -e '.class == \"zenity\" and .xwayland'"
    sleep 1   # mapped is not yet taking keys
    ob keys -b "$B" -t 'aÜbα😀' -s 300 Return >/dev/null
    until_ok 5 ob run -b "$B" -- test -s /tmp/x11
    check_eq "an X11 app gets characters outside the layout" 'aÜbα😀' "$(ob run -b "$B" -- cat /tmp/x11)"
  else skip "an X11 app gets characters outside the layout" "no zenity"; fi
  check "pointer: click, then move" ob pointer -b "$B" -- click move 10 10
  check_eq "pointer: sleep -1 refused" 2 "$(timeout 5 "$CLI" pointer -b "$B" -- sleep -1 >/dev/null 2>&1; echo $?)"
  ob keys -b "$B" -s 3000 a >/dev/null 2>&1 & local k=$!
  ob pointer -b "$B" -- sleep 3000 move 10 10 >/dev/null 2>&1 & local p=$!
  sleep 1.5; ob down "$B" >/dev/null
  wait $k; check_eq "keys exits 1 when the box goes mid-run" 1 $?
  wait $p; check_eq "pointer exits 1 when the box goes mid-run" 1 $?
}

# peek, run inside a box on that box's own screen (never on the host): it draws, and hidden on another
# workspace it stops capturing (finding 64; the old one scaled 30 frames a second nobody saw).
t_peek() {
  local B=$P-peek
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  local wd; wd=$(ob run -b "$B" -- sh -c 'echo $WAYLAND_DISPLAY')
  ob run -b "$B" -d -- "$ROOT/tools/peek/omabox-peek" --box "/run/user/$UID/$wd" --fps 30 >/dev/null 2>&1
  check "peek opens a window" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\")'"
  local pid; pid=$(ob hyprctl -b "$B" -j clients | jq '.[] | select(.class == "omabox-peek") | .pid')
  ticks() { ob run -b "$B" -- sh -c "a=\$(cut -d' ' -f14,15 /proc/$pid/stat | tr ' ' +); sleep 2; b=\$(cut -d' ' -f14,15 /proc/$pid/stat | tr ' ' +); echo \$(( (b) - (a) ))"; }
  local shown; shown=$(ticks)
  check "peek draws while shown (CPU ticks: $shown)" test "$shown" -gt 5
  ob hyprctl -b "$B" dispatch "hl.dsp.window.move({ workspace = '5', follow = false })" >/dev/null; sleep 0.5
  local hidden; hidden=$(ticks)
  check "peek idles while hidden (CPU ticks: $hidden)" test "$hidden" -le 2
  check_fails "peek refuses junk --fps" "$ROOT/tools/peek/omabox-peek" --box /nonexistent --fps abc
  ob down "$B" >/dev/null
}

# A throwaway whose run is gone and whose box died before the reaper saw it (a teardown that gave up
# under load left one): the reaper clears it rather than leave a dead box in ls.
t_throwaway_dead() {
  local repo; repo=$(tmp_repo td)
  (cd "$repo" && exec env -u OMABOX "$CLI" run -- sleep 300) & local r=$!
  until_ok 30 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name | startswith(\"$P-td-run\")) | select(.state == \"up\")'"
  local n; n=$(ob ls --json | jq -r ".[] | select(.name | startswith(\"$P-td-run\")) | .name")
  until_ok 30 pgrep -f "omabox _reap $n "   # up has finished
  local pid; pid=$(jq -r '."child-pid"' "$XDG_RUNTIME_DIR/omabox/$n/info.json")
  kill -KILL "$r" "$pid"
  check "the dead box goes within 15 s" until_ok 15 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$n\")'"
}

# install.sh with a temporary HOME and stubs first on PATH (sudo refuses, so it never installs a
# package): failures stop it with a message (finding 64), and it links, never nests.
t_unit_install() {
  local h=$TMP/ih stub=$TMP/ih-stub out
  mkdir -p "$h" "$stub"
  printf '#!/bin/sh\nexit 1\n' > "$stub/sudo"
  printf '#!/bin/sh\ncase "$*" in *tools/keyboard*) exit 1 ;; esac\nexec /usr/bin/make "$@"\n' > "$stub/make"
  chmod +x "$stub/sudo" "$stub/make"
  out=$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1); local rc=$?
  check_match "a failed tool build stops install.sh" "building tools/keyboard failed" "$out"
  check_eq "...with a failure" 1 "$rc"
  rm "$stub/make"
  mkdir -p "$h/.claude/skills/omabox"
  check_match "a real dir where a link goes is refused" "is not a link" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
  rmdir "$h/.claude/skills/omabox"
  check "install.sh in a clean HOME" env HOME="$h" PATH="$stub:$PATH" "$ROOT/install.sh"
  check_eq "the skill is a link" "$ROOT/skill" "$(readlink "$h/.claude/skills/omabox")"
  check_fails "no dir for an agent that is not installed" test -e "$h/.codex"
  check_fails "the agent guard is never turned on without asking" test -e "$h/.claude/settings.json"
  printf '#!/bin/sh\necho "  -g <geometry>   Set the region to capture."\n' > "$stub/grim"; chmod +x "$stub/grim"
  check_match "a grim with no -T (window capture, finding 81) stops it" "no -T" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
  rm "$stub/grim"
  printf '#!/bin/sh\necho Hyprland dev build\n' > "$stub/Hyprland"; chmod +x "$stub/Hyprland"
  check_match "an unreadable Hyprland version says so (was silent)" "too old" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
}

# The session's Omarchy defaults, and the uwsm-app stand-in's terminal options and desktop files
# (finding 64).
t_uwsm_app() {
  local B=$P-uw
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  check_eq "session TERMINAL is Omarchy's" xdg-terminal-exec "$(ob run -b "$B" -- sh -c 'echo $TERMINAL')"
  check_eq "session EDITOR is Omarchy's" "omarchy-launch-editor --inline" "$(ob run -b "$B" -- sh -c 'echo $EDITOR')"
  # Whether the terminal applies it is xdg-terminal-exec's business (foot's entry has no
  # TerminalArgAppId, so neither it nor real uwsm passes it on); the stand-in must hand it over.
  ob run -b "$B" -- uwsm-app -T --app-id=omabox.term -- sleep 60 >/dev/null
  check_match "uwsm-app -T hands --app-id to xdg-terminal-exec" "uwsm-app: xdg-terminal-exec --app-id=omabox.term sleep 60" "$(ob run -b "$B" -- cat /home/sbx/apps.log)"
  ob run -b "$B" -- sh -c 'printf "[Desktop Entry]\nType=Application\nName=t\nExec=foot --app-id=omabox.desk sleep 60\n" > /tmp/t.desktop'
  ob run -b "$B" -- uwsm-app -- /tmp/t.desktop >/dev/null
  check "uwsm-app launches a desktop file by path" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox.desk\")'"
  check_fails "uwsm-app fails on a missing command" ob run -b "$B" -- uwsm-app -- no-such-command-omabox
  check_eq "the theme's browser policy step is a quiet no-op (finding 75: pkexec)" "" "$(ob run -b "$B" -- omarchy-theme-set-browser-policy 1a1b26 2>&1)"
  ob down "$B" >/dev/null
}

# The bar widget (plugin/) in a box's own bar, against a stand-in omabox (a list from a file, actions
# logged) and an xdg-open that blocks like an image viewer left open (finding 64).
t_widget() {
  local B=$P-wg
  ob up "$B" --net isolated --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up" "failed"; return; }
  local H; H=$(ob path "$B")/home
  printf '#!/bin/sh\ncase "$1" in\n  ls) echo ls >> "$HOME/polls"; cat "$HOME/list.json" ;;\n  shot) echo "$*" >> "$HOME/actions"; echo "$HOME/x.png" ;;\n  config) echo "$*" >> "$HOME/actions"; cat "$HOME/config.json" 2>/dev/null || echo "{}" ;;\n  up) echo "$*" >> "$HOME/actions"; echo box-9 ;;\n  *) echo "$*" >> "$HOME/actions" ;;\nesac\n' > "$H/.local/bin/omabox"
  printf '#!/bin/sh\necho "xdg-open $*" >> "$HOME/actions"; exec sleep 300\n' > "$H/.local/bin/xdg-open"
  printf '#!/bin/sh\necho "notify-send $*" >> "$HOME/actions"\n' > "$H/.local/bin/notify-send"
  chmod +x "$H/.local/bin/omabox" "$H/.local/bin/xdg-open" "$H/.local/bin/notify-send"
  local row='{"mode":"headless","size":"1920x1080@60","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  jq -n "[$row + {name: \"a\"}, $row + {name: \"b\"}]" > "$H/list.json"; : > "$H/actions"
  # shellcheck disable=SC2329 # called through until_ok and check_fails
  panel() { ob hyprctl -b "$B" -j layers | grep -q omarchy-keyboard-panel; }
  # shellcheck disable=SC2329 # called through until_ok
  lines_over() { [ "$(grep -c -- "$3" "$1" 2>/dev/null)" -gt "$2" ]; }
  # A list poll that started after this call: the stub logs each `ls` before it reads the list, so
  # that poll read the list as it is now (then a moment for the widget to take it in).
  polled() { local n; n=$(grep -c . "$H/polls" 2>/dev/null); until_ok 12 lines_over "$H/polls" "${n:-0}" .; sleep 0.3; }
  polled
  ob run -b "$B" -- omarchy-shell chaves.omabox open
  check "the panel opens" until_ok 3 panel
  check "...and reads the settings from the CLI (finding 70)" until_ok 3 grep -qx "config --json" "$H/actions"
  ob keys -b "$B" Down Down >/dev/null
  jq "[$row + {name: \"0new\"}] + ." "$H/list.json" > "$H/l" && mv "$H/l" "$H/list.json"
  polled   # (every 2 s while open) 0new is above b now
  ob keys -b "$B" s >/dev/null
  check "the selection follows its box (shot b, not a)" until_ok 3 grep -qx "shot -b b" "$H/actions"
  check "the viewer is started" until_ok 3 grep -q "^xdg-open $H" <(sed "s|/home/sbx|$H|" "$H/actions")
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down p >/dev/null
  check "an action runs while the viewer is open" until_ok 3 grep -q "^peek -b " "$H/actions"
  # n: a new interactive box under the name the CLI picks, then brought forward (finding 73)
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" n >/dev/null
  check "one n starts nothing (a stray key; finding 74)" holds 1 bash -c "! grep -qx 'up --interactive --new' '$H/actions'"
  ob keys -b "$B" n >/dev/null
  check "n n starts a new interactive box (up --interactive --new)" until_ok 3 grep -qx "up --interactive --new" "$H/actions"
  check "...and shows the box it printed" until_ok 3 grep -qx "peek -b box-9 --focus" "$H/actions"
  # bar-icon (finding 72), read again whenever the settings file changes. always (the default):
  # the panel opens with no boxes, for its settings; auto: opened with none, it stays shut, and
  # does not pop up later when a box comes
  echo '[]' > "$H/list.json"; polled
  ob run -b "$B" -- omarchy-shell chaves.omabox open
  check "bar-icon always (default): the panel opens with no boxes" until_ok 3 panel
  ob keys -b "$B" Escape >/dev/null
  local reads; reads=$(grep -c "^config --json$" "$H/actions")
  echo '{"bar-icon":"auto"}' > "$H/config.json"; echo "bar-icon=auto" > "$H/.config/omabox/config"
  until_ok 5 lines_over "$H/actions" "$reads" "^config --json$"; sleep 0.3   # read again, taken in
  ob run -b "$B" -- omarchy-shell chaves.omabox open
  check "bar-icon auto: opened with no boxes, it stays shut" holds 1 bash -c "! '$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-keyboard-panel"
  jq -n "[$row + {name: \"a\"}]" > "$H/list.json"; polled
  check "...and does not pop up when one comes" holds 1 bash -c "! '$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-keyboard-panel"
  mv "$H/.local/bin/omabox" "$H/.local/bin/omabox.off"
  check "a list command that cannot run is notified" until_ok 20 grep -q "notify-send .*cannot run omabox" "$H/actions"
  ob down "$B" >/dev/null
}

# The agent guard (finding 65): the fake display agents' shells get, and omabox still finding the
# user's session from such a shell. Sessions are faked in a runtime dir of our own where it matters.
GUARDED=(env WAYLAND_DISPLAY=omabox-guard HYPRLAND_INSTANCE_SIGNATURE=omabox-guard DISPLAY= QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1)
t_unit_host_session() {
  check_fails "hyprctl under the guard fails" "${GUARDED[@]}" hyprctl -j version
  local found; found=$("${GUARDED[@]}" bash -c 'source "$1"; host_session; echo "$HOST_SIG"' _ "$TMP/lib/bin/omabox")
  check "under the guard omabox finds a live session ($found)" env HYPRLAND_INSTANCE_SIGNATURE="$found" hyprctl -j version
  check "and reaches it (read-only hyprctl)" "${GUARDED[@]}" bash -c 'source "$1"; host_hyprctl -j version' _ "$TMP/lib/bin/omabox"
  check_eq "--size host under the guard is the same monitor" "$(lib parse_mode host)" "$("${GUARDED[@]}" bash -c 'source "$1"; parse_mode host' _ "$TMP/lib/bin/omabox")"
  local rt=$TMP/rt n
  fake() { mkdir -p "$rt/hypr/$1"; printf '%s\n%s\n' $$ "$2" > "$rt/hypr/$1/hyprland.lock"; }
  sock() { python3 -c 'import socket,sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$1"; }
  hs() { XDG_RUNTIME_DIR=$rt "$@" bash -c 'source "$1"; host_session; echo "$HOST_SIG $HOST_WL"' _ "$TMP/lib/bin/omabox" 2>&1; }
  mkdir -p "$rt"
  check_match "no session: says so" "no Hyprland session" "$(hs "${GUARDED[@]}")"
  fake a_1_1 wayland-8
  check_match "a session without its Wayland socket: says so" "no Wayland socket" "$(hs "${GUARDED[@]}")"
  sock "$rt/wayland-8"
  check_eq "the only session and its socket" "a_1_1 $rt/wayland-8" "$(hs "${GUARDED[@]}")"
  fake b_2_2 wayland-9; sock "$rt/wayland-9"
  check_match "several sessions, none named: refused" "several Hyprland sessions" "$(hs "${GUARDED[@]}")"
  sock "$rt/hypr/b_2_2/.socket.sock"
  check_eq "several, the environment names a live one: that one" "b_2_2 $rt/wayland-9" "$(hs env HYPRLAND_INSTANCE_SIGNATURE=b_2_2)"
  n=$(hs env HYPRLAND_INSTANCE_SIGNATURE=gone_3_3)
  check_match "several, the environment names a dead one: refused" "several Hyprland sessions" "$n"
  mkdir -p "$rt/hypr/stale_4_4"; sock "$rt/hypr/stale_4_4/.socket.sock"
  check_match "...a crashed one whose socket file stayed: not taken (finding 74)" "several Hyprland sessions" "$(hs env HYPRLAND_INSTANCE_SIGNATURE=stale_4_4)"
  check_eq "...nor a HOST_SIG from the caller" "b_2_2 $rt/wayland-9" "$(hs env HYPRLAND_INSTANCE_SIGNATURE=b_2_2 HOST_SIG=x)"
}

# `omabox guard` edits Claude Code's settings.json: merged, never overwritten, reversible, a backup
# per change, a link stays a link, a file that is not JSON is left alone.
t_unit_guard_settings() {
  local h=$TMP/gh s out
  mkdir -p "$h/.claude"; s=$h/.claude/settings.json
  g() { HOME=$h "$CLI" guard "$@" 2>&1; }
  check_match "no file: off, and shows what on does" "off.*omabox guard on" "$(g | tr '\n' ' ')"
  g on >/dev/null
  check_eq "on creates it with our hook" 1 "$(jq '[.hooks.SessionStart[].hooks[] | select(.command | contains("omabox-guard"))] | length' "$s")"
  check_match "the state reads on" ": on$" "$(g | head -1)"
  jq -n '{model: "x", hooks: {SessionStart: [{matcher: "*", hooks: [{type: "command", command: "mine.sh"}]}], Stop: [{hooks: [{type: "command", command: "stop.sh"}]}]}, theme: "dark"}' > "$s"
  chmod 600 "$s"; local orig; orig=$(cat "$s")
  rm -f "$s".bak-*
  g on >/dev/null
  check_eq "on keeps everything else" "$(jq -S 'del(.hooks.SessionStart[1])' <<<"$orig")" "$(jq -S 'del(.hooks.SessionStart[1])' "$s")"
  check_eq "...and backs up the old file" "$orig" "$(cat "$s".bak-*)"
  check_eq "...keeping its mode" 600 "$(stat -c %a "$s")"
  check_match "on again: nothing to do" "already on" "$(g on)"
  check_eq "...and no second backup" 1 "$(compgen -G "$s.bak-*" | wc -l)"
  g off >/dev/null
  check_eq "off gives back the same file" "$orig" "$(cat "$s")"
  jq '.hooks.SessionStart += [{hooks: [{type: "command", command: "echo old omabox-guard"}]}]' <<<"$orig" > "$s"
  check_match "an older hook of ours reads outdated" "outdated" "$(g | head -1)"
  g on >/dev/null
  check_eq "on replaces it" 1 "$(jq '[.hooks.SessionStart[].hooks[] | select(.command | contains("omabox-guard"))] | length' "$s")"
  mv "$s" "$h/real.json"; ln -s "$h/real.json" "$s"
  g off >/dev/null
  check "a linked settings file stays a link" test -L "$s"
  check_eq "...and its target changes" "$orig" "$(cat "$h/real.json")"
  rm "$s"; echo '{"broken":' > "$s"; rm -f "$s".bak-*
  out=$(g on); local rc=$?
  check_match "a file that is not JSON is refused" "not a JSON object" "$out"
  check_eq "...with a failure" 1 "$rc"
  check_eq "...and left alone" '{"broken":' "$(cat "$s")"
  check_fails "...with no backup" compgen -G "$s.bak-*"
  mkdir -p "$h/alt"
  CLAUDE_CONFIG_DIR=$h/alt HOME=$h "$CLI" guard on >/dev/null 2>&1
  check "CLAUDE_CONFIG_DIR is honoured" jq -e '.hooks.SessionStart' "$h/alt/settings.json"
  local hook; hook=$(lib eval 'printf %s "$GUARD_HOOK"')
  check_match "the hook says so when it cannot apply the guard (finding 74)" "NOT applied" "$(env -u CLAUDE_ENV_FILE sh -c "$hook")"
  check_eq "...and applies it when it can" 1 "$(f=$TMP/envfile; CLAUDE_ENV_FILE=$f sh -c "$hook" >/dev/null; grep -c 'WAYLAND_DISPLAY=omabox-guard' "$f")"
  check_fails "guard junk refused" g maybe
  check_fails "guard on for an unknown agent refused" g on vim
  # Codex (finding 67): a marked block in config.toml, checked as TOML; only when Codex is installed.
  local c=$h/.codex/config.toml
  check_fails "no Codex dir: Codex not listed" grep -q Codex <<<"$(g)"
  mkdir -p "$h/.codex"; printf 'model = "x"\n\n[features]\nhooks = true\n' > "$c"; orig=$(cat "$c"; echo .)
  g on codex >/dev/null
  check_eq "Codex: on sets the variables for its commands" omabox-guard \
    "$(python3 -c 'import tomllib, sys; print(tomllib.load(open(sys.argv[1], "rb"))["shell_environment_policy"]["set"]["WAYLAND_DISPLAY"])' "$c")"
  check_match "...and reads on" "Codex .*: on$" "$(g | grep '^Codex')"
  check_match "a broken Claude Code file is reported, not fatal for the rest" "claude: .*not a JSON object" "$(g | grep claude)"
  rm "$s"
  check_match "on codex leaves Claude Code alone" "Claude Code .*: off$" "$(g | grep '^Claude')"
  g off codex >/dev/null
  check_eq "Codex: off gives back the same file (trailing newline too)" "$orig" "$(cat "$c"; echo .)"
  # Codex keeps its file's last comment, our end marker, last: a table it adds lands inside the block
  # (finding 74: off deleted MCP servers).
  g on codex >/dev/null
  sed -i 's/^# <<< omabox guard$/\n[mcp_servers.x]\ncommand = "true"\n# <<< omabox guard/' "$c"
  check_match "Codex: a table added inside the block still reads on" "Codex .*: on$" "$(g | grep '^Codex')"
  check_match "...on again leaves it" "already on" "$(g on codex)"
  g off codex >/dev/null
  check_eq "...and off keeps it" true "$(python3 -c 'import tomllib, sys; print(tomllib.load(open(sys.argv[1], "rb"))["mcp_servers"]["x"]["command"])' "$c")"
  check_fails "...with nothing of the guard left" grep -q omabox "$c"
  printf '%s\n' "$(cat "$c")" "" "# >>> omabox guard" "[shell_environment_policy.set]" 'MINE = "1"' "# <<< omabox guard" > "$c"
  check_match "Codex: a key of someone else's inside the block: refused" "added inside" "$(g off codex)"
  printf 'model = "x"\n\n[features]\nhooks = true\n' > "$c"
  printf '[shell_environment_policy.set]\nFOO = "1"\n' > "$c"; orig=$(cat "$c"; echo .)
  out=$(g on codex)
  check_match "Codex: its own [shell_environment_policy.set] is refused, with the block to add" "clashes.*WAYLAND_DISPLAY" "$(tr '\n' ' ' <<<"$out")"
  check_eq "...untouched" "$orig" "$(cat "$c"; echo .)"
  printf '[shell_environment_policy]\ninherit = "core"\n' > "$c"
  g on codex >/dev/null
  check_eq "Codex: its own [shell_environment_policy] is kept, the guard added" "core omabox-guard" \
    "$(python3 -c 'import tomllib, sys; p = tomllib.load(open(sys.argv[1], "rb"))["shell_environment_policy"]; print(p["inherit"], p["set"]["HYPRLAND_INSTANCE_SIGNATURE"])' "$c")"
  printf 'x = \n' > "$c"
  check_match "Codex: a file that is not TOML is refused" "not valid TOML" "$(g on codex)"
  check_eq "...untouched" 'x = ' "$(cat "$c")"
}

# guard exec (any agent, whole) and host (one command on the real desktop, from a guarded shell).
t_unit_guard_exec_host() {
  check_eq "guard exec: the guard's display" omabox-guard "$("$CLI" guard exec -- sh -c 'echo $WAYLAND_DISPLAY')"
  check_eq "guard exec: a core limit of 1 byte (no crash notification)" 1 "$("$CLI" guard exec -- sh -c 'prlimit --pid $$ --core -o SOFT --noheadings | tr -d " "')"
  # The session omabox finds, not $HYPRLAND_INSTANCE_SIGNATURE: under the guard (an agent running the
  # suite) that is the guard's.
  local sig want; sig=$(bash -c 'source "$1"; host_session; echo "$HOST_SIG"' _ "$TMP/lib/bin/omabox")
  want=$(hyprctl -j instances | jq -r --arg s "$sig" '.[] | select(.instance == $s) | .wl_socket')
  check_eq "host from a guarded shell: the real Wayland display" "$want" "$("${GUARDED[@]}" "$CLI" host -- sh -c 'echo $WAYLAND_DISPLAY' 2>/dev/null)"
  check "host: and hyprctl reaches it (read-only)" "${GUARDED[@]}" "$CLI" host -- hyprctl -j version
  check_eq "host: Qt logging as usual" unset "$("${GUARDED[@]}" "$CLI" host -- sh -c 'echo ${QT_FORCE_STDERR_LOGGING-unset}' 2>/dev/null)"
  check_match "host says what it runs" "on your real desktop: true" "$("${GUARDED[@]}" "$CLI" host -- true 2>&1)"
  check_fails "host with nothing to run refused" "${GUARDED[@]}" "$CLI" host
}

# Under the guard, with a box standing in for the host (finding 26): box commands work, an app run
# straight fails instead of opening a window, and omabox's own host windows (interactive, peek) and
# --size host still find the session. Never on the real desktop.
t_guard() {
  local B=$P-gd
  "${GUARDED[@]}" "$CLI" up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up under the guard" "failed"; return; }
  check_match "run under the guard gets the box's display" '^wayland-' "$("${GUARDED[@]}" "$CLI" run -b "$B" -- sh -c 'echo $WAYLAND_DISPLAY')"
  check "hyprctl under the guard" "${GUARDED[@]}" "$CLI" hyprctl -b "$B" -j version
  check "shot under the guard" "${GUARDED[@]}" "$CLI" shot -b "$B" -o "$TMP/guard.png"
  # Inside the stand-in host: the guard as an agent's shell there would have it.
  local in=("$CLI" run -b "$B" -- "${GUARDED[@]}")
  # A Wayland client says so too, and exits cleanly.
  check_match "a Wayland client under the guard fails with no display" "Failed to connect to a Wayland server" \
    "$("${in[@]}" wl-paste 2>&1)"
  check_fails "hyprctl under the guard fails there too" "${in[@]}" hyprctl -j version
  # Qt offscreen (what ctest sets) under the guard: runs, because the guard also blanks Omarchy's
  # QT_QPA_PLATFORMTHEME=gtk3, which starts GTK and needs a display. Run under a name
  # omarchy-crash-watch never announces, with no core file, in case it ever aborts.
  local qt='cp /usr/lib/qt6/bin/qml /tmp/omarchy-crash-omabox-qt
    printf "import QtQuick\nWindow { visible: true; Component.onCompleted: Qt.quit() }\n" > /tmp/q.qml
    exec timeout 10 /tmp/omarchy-crash-omabox-qt /tmp/q.qml'
  check "offscreen Qt runs under the guard" "${in[@]}" QT_QPA_PLATFORM=offscreen bash -c "$qt"
  check_match "...which needs the theme blanked (gtk3: GTK wants a display)" "cannot open display" \
    "$("${in[@]}" QT_QPA_PLATFORM=offscreen QT_QPA_PLATFORMTHEME=gtk3 bash -c "$qt" 2>&1)"
  # A Qt program with no display aborts: the reason reaches the caller (QT_FORCE_STDERR_LOGGING), and
  # the box's core limit keeps the abort out of the journal, so no crash notification (finding 67).
  local since; since=$(date '+%Y-%m-%d %H:%M:%S')
  check_match "a Qt app under the guard says why it fails" "could not connect to display" \
    "$("${in[@]}" QT_QPA_PLATFORM='wayland;xcb' bash -c "${qt//exec timeout/timeout}" 2>&1)"
  sleep 1
  check_eq "...and its abort is not a journaled crash" 0 "$(journalctl --since "$since" MESSAGE_ID=fc2e22bc6ee647b6b90729ab34a250b1 -o json --no-pager 2>/dev/null | grep -c omarchy-crash-omabox)"
  check "up --interactive under the guard" "${in[@]}" "$CLI" up inner --interactive --no-shell
  check_eq "...its window is on workspace 9" 9 "$(ob hyprctl -b "$B" -j clients | jq -r '.[] | select(.class == "aquamarine") | .workspace.name')"
  check_eq "...without focus" null "$(ob hyprctl -b "$B" -j activewindow | jq -r '.class')"
  # Its window hidden, nothing is rendered: wait cannot tell, and says so (finding 82)
  local out rc=0; out=$("${in[@]}" "$CLI" wait -b inner still 2>&1) || rc=$?
  check_eq "wait still on a hidden interactive box: unknown (1), never 0" 1 "$rc"
  check_match "...not rendered" "^unknown: box 'inner' not rendered" "$out"
  "${in[@]}" "$CLI" down inner >/dev/null 2>&1
  # The workspace setting (finding 70): a number, the scratchpad; neither takes focus
  check "up --interactive --workspace 3" "${in[@]}" "$CLI" up ws3 --interactive --no-shell --workspace 3
  check "up --interactive on the scratchpad (config)" "${in[@]}" bash -c "'$CLI' config workspace special >/dev/null && '$CLI' up wsp --interactive --no-shell; '$CLI' config workspace default >/dev/null"
  check_eq "...on workspace 3 and the scratchpad" "3 special:scratchpad" "$(ob hyprctl -b "$B" -j clients | jq -r '[.[] | select(.class == "aquamarine") | .workspace.name] | sort | join(" ")')"
  check_eq "...without focus" null "$(ob hyprctl -b "$B" -j activewindow | jq -r '.class')"
  "${in[@]}" "$CLI" down ws3 >/dev/null 2>&1; "${in[@]}" "$CLI" down wsp >/dev/null 2>&1
  # confirm-close: the first close opens a new window and asks, a second one ends the box; off, one
  # close ends it; `omabox config` changes a running box that took it from the config
  local cl=(ob hyprctl -b "$B" dispatch "hl.dsp.window.close({ window = 'class:aquamarine' })")
  state() { "${in[@]}" "$CLI" ls --json | jq -r --arg n "$1" '[.[] | select(.name == $n) | .state][0] // "gone"'; }
  # shellcheck disable=SC2329 # called through until_ok and holds
  gone() { [ "$(state "$1")" = gone ]; }
  # shellcheck disable=SC2329
  is() { [ "$(state "$1")" = "$2" ]; }
  "${in[@]}" "$CLI" up cc --interactive --no-shell --confirm-close >/dev/null 2>&1
  "${cl[@]}" >/dev/null
  check "confirm-close: the box stays after a close" holds 2 is cc up
  check_eq "...with a new window" 1 "$(ob hyprctl -b "$B" -j clients | jq '[.[] | select(.class == "aquamarine")] | length')"
  "${cl[@]}" >/dev/null
  check "...and a second close ends it, cleared: no dead box left (finding 71)" until_ok 8 gone cc
  "${in[@]}" "$CLI" up lv --interactive --no-shell >/dev/null 2>&1
  "${in[@]}" "$CLI" config confirm-close on >/dev/null
  "${cl[@]}" >/dev/null
  check "config confirm-close on reaches a running box" holds 2 is lv up
  "${in[@]}" "$CLI" config confirm-close off >/dev/null
  "${cl[@]}" >/dev/null
  check "...and off again: one close ends it, cleared" until_ok 8 gone lv
  # A box that dies without being closed stays dead, logs kept, until `down`
  "${in[@]}" "$CLI" up cr --interactive --no-shell >/dev/null 2>&1
  "${in[@]}" "$CLI" run -b cr -- pkill -KILL -x Hyprland >/dev/null 2>&1
  until_ok 10 is cr dead
  # its reaper's first poll after that decides, then it goes
  until_ok 10 bash -c "! '$CLI' run -b '$B' -- pgrep -f 'omabox _reap cr '"
  check_eq "an interactive box that crashed stays dead" dead "$(state cr)"
  "${in[@]}" "$CLI" down cr >/dev/null 2>&1
  check "up --size host under the guard" "${in[@]}" "$CLI" up inner2 --size host --no-shell --idle 0
  check_eq "...the stand-in's monitor" "$(ob mode -b "$B")" "$("${in[@]}" "$CLI" mode -b inner2)"
  check "peek under the guard" "${in[@]}" "$CLI" peek inner2
  check "...its window appears" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\") | select(.workspace.name == \"9\")'"
  "${in[@]}" "$CLI" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# Window selectors and what covers what (finding 81), on hyprctl JSON made up for it: floats above
# tiled windows whatever their order, later above earlier otherwise, fullscreen and a shown special
# workspace above the rest; off-screen workspaces, inactive group tabs and unmapped windows.
t_unit_window_select() {
  local c m m_sp a='{"address":"0xb"}'
  c=$(jq -nc '
    def w($a; $cl; $t; $ws; $x; $y; $w; $h; $fl): {address: $a, stableId: "1", class: $cl, title: $t,
      initialClass: $cl, initialTitle: $t, pid: 10, workspace: {id: $ws, name: ($ws | tostring)}, at: [$x, $y],
      size: [$w, $h], floating: $fl, fullscreen: 0, xwayland: false, mapped: true, hidden: false};
    [w("0xf"; "zenity"; "Question"; 1; 100; 100; 300; 200; true),
     w("0xa"; "foot"; "A"; 1; 0; 0; 1000; 1000; false) + {pid: 42, initialTitle: "first"},
     w("0xb"; "foot"; "B"; 1; 1000; 0; 900; 1000; false),
     w("0xg"; "foot"; "B tab"; 1; 1000; 0; 900; 1000; false) + {hidden: true},
     w("0x2"; "zenity"; "Other"; 1; 450; 350; 100; 100; true),
     w("0xc"; "Chromium"; "Docs - Chromium"; 5; 0; 0; 1900; 1000; false),
     w("0xd"; "foot"; "scratch"; -98; 500; 500; 200; 200; true),
     w("0xe"; "foot"; "gone"; 1; 0; 0; 10; 10; false) + {mapped: false}]')
  m='[{"activeWorkspace":{"id":1},"specialWorkspace":{"id":0}}]'
  m_sp='[{"activeWorkspace":{"id":1},"specialWorkspace":{"id":-98}}]'
  local W Wsp; W=$(lib win_annotate "$c" "$m" "$a") Wsp=$(lib win_annotate "$c" "$m_sp" "$a")
  sel() { lib win_select "$W" "$@" 2>/dev/null | jq -r .address; }
  check_eq "title:RE" 0xa "$(sel 'title:^A$')"
  check_eq "a word: part of the title, any case" 0xc "$(sel docs)"
  check_eq "a word: the class, any case" 0xc "$(sel chromium)"
  check_eq "pid:N and a word, ANDed" 0xa "$(sel pid:42 foot)"
  check_eq "an address, any case" 0xa "$(sel 0xA)"
  check_eq "address:0x..." 0xa "$(sel address:0xa)"
  check_eq "active" 0xb "$(sel active)"
  check_eq "initialTitle:RE" 0xa "$(sel initialTitle:^first)"
  check_eq "class:RE ANDed with title:RE" 0x2 "$(sel class:zen 'title:^oth')"
  check_eq "several matches: exit 2" 2 "$(lib win_select "$W" foot >/dev/null 2>&1; echo $?)"
  check_match "...listing them" "matches 4 windows.*0xa.*0xb.*0xg.*0xd" "$(lib win_select "$W" foot 2>&1 | tr '\n' ' ')"
  check_eq "none: exit 2" 2 "$(lib win_select "$W" nope >/dev/null 2>&1; echo $?)"
  check_match "...listing the box's windows" "no window matches nope.*0xf" "$(lib win_select "$W" nope 2>&1 | tr '\n' ' ')"
  check_eq "an unmapped window is not a window" 2 "$(lib win_select "$W" 'title:^gone$' >/dev/null 2>&1; echo $?)"
  check_match "a bad regex: exit 2, said" "bad window selector" "$(lib win_select "$W" 'title:(' 2>&1; echo " rc=$?")"
  check_match "...rc 2" "rc=2" "$(lib win_select "$W" 'title:(' 2>&1; echo " rc=$?")"
  cov() { jq -r --arg a "$2" '.[] | select(.address == $a) | [.cover[].address] | join(" ")' <<<"$1"; }
  on() { jq -r --arg a "$2" '.[] | select(.address == $a) | .onscreen' <<<"$1"; }
  check_eq "floats cover the tiled window under them, though one is earlier in the list" "0xf 0x2" "$(cov "$W" 0xa)"
  check_eq "...a tiled window never covers a float" "" "$(cov "$W" 0xf)"
  check_eq "...a later float covers an earlier one" "0x2" "$(jq -r '.[] | select(.address == "0xf") | .cover[0].address // ""' <<<"$(lib win_annotate "$(jq -c 'map(if .address == "0x2" then .at = [150, 150] else . end)' <<<"$c")" "$m" "$a")")"
  check_eq "...and not the other way" "" "$(cov "$W" 0x2)"
  check_eq "another workspace is off screen" false "$(on "$W" 0xc)"
  check_eq "an inactive group tab is off screen" false "$(on "$W" 0xg)"
  check_eq "...and covers nothing" "" "$(cov "$W" 0xb)"
  check_eq "a hidden special workspace is off screen" false "$(on "$W" 0xd)"
  check_eq "a shown one is on screen" true "$(on "$Wsp" 0xd)"
  check_eq "...above the workspace under it" "0xf 0x2 0xd" "$(cov "$Wsp" 0xa)"
  local Wfs; Wfs=$(lib win_annotate "$(jq -c 'map(if .address == "0xa" then .fullscreen = 1 else . end)' <<<"$c")" "$m" "$a")
  check_eq "a fullscreen window: nothing covers it" "" "$(cov "$Wfs" 0xa)"
  check_eq "...it covers the rest" 0xa "$(cov "$Wfs" 0xf | tr ' ' '\n' | grep -x 0xa)"
  local A; A=$(jq -c '.[] | select(.address == "0xa")' <<<"$W")
  check_match "a point under the float is covered, named" "covered there by 0xf zenity" "$(lib win_blocked "$A" 150 150)"
  check_eq "a point beside it is not" "" "$(lib win_blocked "$A" 900 900)"
  check_match "off screen says so" "not on screen \(workspace 5\)" "$(lib win_blocked "$(jq -c '.[] | select(.address == "0xc")' <<<"$W")" 1 1)"
  # --in (finding 81): image pixel -> screen pixel under its centre
  check_eq "1:1 maps to itself" "0 1919" "$(lib img_px 0 1920 1920) $(lib img_px 1919 1920 1920)"
  check_eq "x1.92: 500 of 1000 is 960" 960 "$(lib img_px 500 1000 1920)"
  check_eq "x1.92: the last pixel stays inside" 1919 "$(lib img_px 999 1000 1920)"
  check_eq "x0.5 (an image larger than the screen)" 50 "$(lib img_px 101 200 100)"
}

# Window-targeted shot, click and keys, and --in (finding 81), with the box's own windows: two
# tiled terminals, a float over one, others on hidden workspaces.
t_window() {
  local B=$P-win
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  term() { ob run -b "$B" -d -- foot -T "$1" sh -c "$2" >/dev/null; until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.title == \"$1\")'"; }
  win() { ob windows -b "$B" --json | jq -c --arg t "$1" '.[] | select(.title == $t)'; }
  addr() { win "$1" | jq -r .address; }
  at() { win "$1" | jq -r '"\(.at[0] + ('"${2:-0}"')), \(.at[1] + ('"${3:-0}"'))"'; }
  size() { win "$1" | jq -r '"\(.size[0]) x \(.size[1])"'; }
  disp() { ob hyprctl -b "$B" dispatch "$1" >/dev/null; }
  pos() { ob hyprctl -b "$B" cursorpos; }
  term A 'sleep 600'; term B 'sleep 600'
  ob wait -b "$B" still >/dev/null   # A shrinks as B opens: its buffer is the window's size once that settles
  local o=$TMP/win
  mkdir -p "$o"
  check_eq "an ambiguous selector: exit 2" 2 "$(ob shot -b "$B" -w foot >/dev/null 2>&1; echo $?)"
  check_eq "0xdead: exit 2" 2 "$(ob shot -b "$B" -w 0xdead >/dev/null 2>&1; echo $?)"
  check_match "...no window matches, the windows listed" "no window matches 0xdead.*\"A\"" "$(ob shot -b "$B" -w 0xdead 2>&1 | tr '\n' ' ')"
  ob shot -b "$B" -w 'title:^A$' -o "$o/a.png" >/dev/null 2>&1
  check_match "shot --window: the window's size" "$(size A)," "$(file "$o/a.png")"
  check_match "windows lists them" "foot +\"B\"" "$(ob windows -b "$B")"
  # A float over A: A's own pixels still, and said to be covered
  term C 'sleep 600'
  local c; c=$(addr C)
  disp "hl.dsp.window.float({ action = 'enable', window = 'address:$c' })"
  disp "hl.dsp.window.resize({ x = 400, y = 300, window = 'address:$c' })"
  disp "hl.dsp.window.move({ x = 100, y = 100, window = 'address:$c' })"
  until_ok 5 bash -c "'$CLI' windows -b '$B' --json | jq -e '.[] | select(.title == \"A\") | .cover != []'"
  local err; err=$(ob shot -b "$B" -w 'title:^A$' -o "$o/a2.png" 2>&1 >/dev/null)
  check_match "a covered window: its full size" "$(size A)," "$(file "$o/a2.png")"
  check_match "...stderr says what covers it" "covered by $c foot \"C\"" "$err"
  ob click -b "$B" --window 'title:^A$' 10 10 >/dev/null 2>&1
  check_eq "click --window: window coordinates" "$(at A 10 10)" "$(pos)"
  check_match "a point under another window is refused, naming it" "covered there by $c" "$(ob click -b "$B" --window 'title:^A$' 150 150 2>&1)"
  check_fails "a point outside the window is refused" ob click -b "$B" --window B 5000 5
  # Another workspace: B moves to 5; a click raises it
  local b; b=$(addr B)
  disp "hl.dsp.window.move({ workspace = '5', follow = false, window = 'address:$b' })"
  until_ok 5 bash -c "'$CLI' windows -b '$B' --json | jq -e '.[] | select(.title == \"B\") | .onscreen == false'"
  check_fails "--no-raise refuses a window off screen" ob click -b "$B" --window B --no-raise 1 1
  check_eq "...and switches nothing" 1 "$(ob hyprctl -b "$B" -j activeworkspace | jq .id)"
  ob click -b "$B" --window B 20 30 >/dev/null 2>&1
  check_eq "click on a window on workspace 5 shows 5" 5 "$(ob hyprctl -b "$B" -j activeworkspace | jq .id)"
  check_eq "...and lands in it" "$(at B 20 30)" "$(pos)"
  err=$(ob shot -b "$B" --active -o "$o/act.png" 2>&1 >/dev/null)
  check_match "--active is the active window" "window $b " "$err"
  check_match "...its size" "$(size B)," "$(file "$o/act.png")"
  ob click -b "$B" --in "$o/act.png" 0 0 >/dev/null 2>&1
  check_eq "click --in a window shot: 0,0 is its corner" "$(at B)" "$(pos)"
  # Priming (finding 81): a window off screen that keeps redrawing stops getting frames; the first
  # capture after a while is from long ago. Its colour comes from a file.
  term P 'while :; do printf "\033[4%sm\033[2J" "$(cat /tmp/col 2>/dev/null || echo 1)"; sleep 0.05; done'
  disp "hl.dsp.window.move({ workspace = '6', follow = false, window = 'address:$(addr P)' })"
  sleep 0.5
  ob shot -b "$B" -w 'title:^P$' -o "$o/p1.png" >/dev/null 2>&1
  ob run -b "$B" -- sh -c 'echo 4 > /tmp/col'; sleep 1
  ob shot -b "$B" -w 'title:^P$' -o "$o/p2.png" >/dev/null 2>&1
  check_fails "a window off screen: the shot is fresh, not an old frame" cmp -s "$o/p1.png" "$o/p2.png"
  check_eq "...and shooting it switched nothing" 5 "$(ob hyprctl -b "$B" -j activeworkspace | jq .id)"
  # keys --window: focus, then type
  term K 'cat > /tmp/typed'
  disp "hl.dsp.window.move({ workspace = '7', follow = false, window = 'address:$(addr K)' })"
  ob keys -b "$B" --window 'title:^K$' -t hi Return ctrl+d >/dev/null 2>&1
  check "keys --window types into it" until_ok 5 ob run -b "$B" -- test -s /tmp/typed
  check_eq "...what was typed" hi "$(ob run -b "$B" -- cat /tmp/typed)"
  # --fit and --in on the screen (idea 4)
  err=$(ob shot -b "$B" -o "$o/full.png" 2>&1 >/dev/null)
  check_eq "a 1:1 shot says nothing extra" "" "$err"
  err=$(ob shot -b "$B" --fit 1000 -o "$o/fit.png" 2>&1 >/dev/null)
  check_match "--fit 1000: 1000 wide" "1000 x 562," "$(file "$o/fit.png")"
  check_match "...stderr names the factor and --in" "1000x562 image of the screen's 1920x1080 at 0,0 \(x1.92\); click with --in $o/fit.png" "$err"
  ob click -b "$B" --in "$o/fit.png" 500 270 >/dev/null 2>&1
  check_eq "click --in a scaled shot" "960, 519" "$(pos)"
  ob pointer -b "$B" --in "$o/fit.png" -- move 250 100 >/dev/null 2>&1
  check_eq "pointer --in" "480, 193" "$(pos)"
  err=$(ob shot -b "$B" -g "300,200 400x100" -o "$o/g.png" 2>&1 >/dev/null)
  check_match "-g says it is cropped" "400x100 image of the screen's 400x100 at 300,200" "$err"
  ob click -b "$B" --in "$o/g.png" 7 9 >/dev/null 2>&1
  check_eq "click --in a -g shot: from its origin" "307, 209" "$(pos)"
  check_match "--in a file that is not a shot: refused" "not a shot of box" "$(ob click -b "$B" --in /etc/hostname 1 1 2>&1)"
  check_match "--in outside the image: refused" "outside the image" "$(ob click -b "$B" --in "$o/g.png" 400 1 2>&1)"
  touch "$o/g.png"
  check_match "--in a file changed since: refused" "changed since" "$(ob click -b "$B" --in "$o/g.png" 1 1 2>&1)"
  # A window shot follows its window: C moved after the shot
  ob shot -b "$B" -w 'title:^C$' -o "$o/c.png" >/dev/null 2>&1
  disp "hl.dsp.window.move({ x = 700, y = 300, window = 'address:$c' })"
  until_ok 5 bash -c "'$CLI' windows -b '$B' --json | jq -e '.[] | select(.title == \"C\") | .at == [700, 300]'"
  ob click -b "$B" --in "$o/c.png" 5 6 >/dev/null 2>&1
  check_eq "click --in a window shot after the window moved: where it is now" "705, 306" "$(pos)"
  ob mode -b "$B" 1280x720 >/dev/null
  check_match "--in after a mode change: refused" "mode is 1280x720@60" "$(ob click -b "$B" --in "$o/full.png" 1 1 2>&1)"
  ob down "$B" >/dev/null
}

# omabox wait and --wait (finding 82): the pure parts, and the tool's refusal outside a box, checked
# in a bare namespace with no /opt/omabox and no display (where a broken check could reach nothing).
t_unit_wait() {
  check_eq "300ms" 300 "$(lib ms_duration 300ms)"
  check_eq "1.5s" 1500 "$(lib ms_duration 1.5s)"
  check_eq "2m" 120000 "$(lib ms_duration 2m)"
  check_eq "a bare number is seconds" 10000 "$(lib ms_duration 10)"
  check_fails "0 refused" lib ms_duration 0
  check_fails "junk refused" lib ms_duration 2h
  check_eq "the cursor's rectangle" "952,532,56,56" "$(lib cursor_rect "960 540")"
  check_match "wait --timeout over 10 min refused" "at most 10m" "$(ob wait -b "$P-x" --timeout 11m still 2>&1)"
  check_match "keys --timeout over 10 min refused" "at most 10m" "$(ob keys -b "$P-x" --wait --timeout 601s a 2>&1)"
  check_match "wait for nothing: says what it can wait for" "still, change, window" "$(ob wait -b "$P-x" 2>&1)"
  check_match "an unknown condition refused" "unknown condition" "$(ob wait -b "$P-x" soon 2>&1)"
  check_match "--gone is for window and layer" "go with window or layer" "$(ob wait -b "$P-x" still --gone 2>&1)"
  check_match "cmd needs a command" "nothing to run" "$(ob wait -b "$P-x" cmd -- 2>&1)"
  check_match "--quiet without --wait refused" "go with --wait" "$(ob keys -b "$P-x" --quiet 1s a 2>&1)"
  check_match "run --wait needs -d" "goes with -d" "$(ob run -b "$P-x" --wait -- true 2>&1)"
  local out rc=0
  out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --die-with-parent \
        env -i "$ROOT/tools/still/omabox-still" still --timeout 100 2>&1) || rc=$?
  check_eq "omabox-still refuses outside a box" 2 "$rc"
  check_match "...and says so" "only runs inside an omabox box" "$out"
}

# omabox wait and --wait (finding 82) in a box of their own: two terminals side by side, one of them
# repainting 20 times a second.
t_wait() {
  local B=$P-wait out rc
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  local D; D=$(ob path "$B")
  out=$(ob wait -b "$B" still); rc=$?
  check_eq "an idle screen is still: exit 0" 0 "$rc"
  check_match "...said on one line" "^satisfied: still after 0\.[0-9]+s" "$out"
  touch -d '-1 hour' "$D/used"
  ob wait -b "$B" still --quiet 100ms >/dev/null
  check "wait counts as use (idle expiry)" test "$(( $(date +%s) - $(stat -c %Y "$D/used") ))" -lt 60
  out=$(ob run -b "$B" -d --wait -- foot -T A sh -c 'cat > /tmp/typed' 2>/dev/null); rc=$?
  check_eq "run -d --wait: settled" 0 "$rc"
  check_match "...once the window was drawn" "^satisfied: settled after .*last change" "$out"
  check "wait window: it is there" ob wait -b "$B" window 'title:^A$' --focused
  out=$(ob keys -b "$B" --wait -t hello); rc=$?
  check_eq "keys --wait: typing settles" 0 "$rc"
  check_match "...the cursor hidden by the key press is ignored, said" "ignored .*: cursor" "$out"
  local err; out=$(ob keys -b "$B" --wait --start 1s shift 2>"$TMP/wait.err"); rc=$?; err=$(cat "$TMP/wait.err")
  check_eq "keys --wait of a key that changes nothing: 124" 124 "$rc"
  check_match "...nothing changed" "^unsatisfied: nothing changed in 1\.[0-9]+s" "$out"
  check_match "...and the keys were sent all the same" "keys were sent" "$err"
  out=$(ob keys -b "$B" --wait --json -t x)
  check_eq "--json" "satisfied settle" "$(jq -r '"\(.result) \(.condition)"' <<<"$out")"
  # A caret blinking (foot's beam) is not a change; --strict counts it
  ob run -b "$B" -d --wait -- foot -T C -o cursor.blink=yes -o cursor.style=beam sh -c 'sleep 600' >/dev/null 2>&1
  out=$(ob wait -b "$B" still --quiet 1500ms); rc=$?
  check_eq "a blinking caret is still" 0 "$rc"
  check_match "...said as a caret" "ignored 2x[0-9]+ at [0-9]+,[0-9]+: caret\?" "$out"
  check_eq "...--strict: it is a change (124)" 124 "$(ob wait -b "$B" still --quiet 1500ms --strict --timeout 2500ms >/dev/null; echo $?)"
  ob run -b "$B" -- pkill -f 'foot -T C' >/dev/null
  check "wait window --gone" ob wait -b "$B" window 'title:^C$' --gone
  ob wait -b "$B" still >/dev/null   # A takes the whole screen once C is gone
  out=$(ob click -b "$B" --wait --start 1s --window 'title:^A$' 40 40 2>"$TMP/wait.err"); rc=$?
  check_eq "click --wait on what does nothing: 124" 124 "$rc"
  check_match "...said, and that the click was sent" "^unsatisfied: nothing changed.*click was sent" "$out $(cat "$TMP/wait.err")"
  check_eq "...it was: the pointer is there" "$(ob windows -b "$B" --json | jq -r '.[] | select(.title == "A") | "\(.at[0] + 40), \(.at[1] + 40)"')" "$(ob hyprctl -b "$B" cursorpos)"
  # A repaint loop: never still (124, naming where); a window beside it is
  ob run -b "$B" -d -- foot -T R sh -c 'while :; do printf "\033[4%sm\033[2J" $((RANDOM % 7)); sleep 0.05; done' >/dev/null 2>&1
  ob wait -b "$B" window 'title:^R$' >/dev/null
  local r; r=$(ob windows -b "$B" --json | jq -r '.[] | select(.title == "R") | "\(.at[0]) \(.at[0] + .size[0])"')
  out=$(ob wait -b "$B" still --timeout 1500ms); rc=$?
  check_eq "a repaint loop: 124 at the deadline" 124 "$rc"
  check_match "...naming where it changes" "^unsatisfied: still changing after 1\.[0-9]+s \(last change .* at [0-9]+,[0-9]+ [0-9]+x[0-9]+\)$" "$out"
  local x; x=$(sed -n 's/.* at \([0-9]*\),.*/\1/p' <<<"$out")
  check "...inside that window ($x in ${r% *}..${r#* })" test "${x:-0}" -ge "${r% *}" -a "${x:-0}" -lt "${r#* }"
  check "...a window beside it is still" ob wait -b "$B" still --window 'title:^A$'
  check_eq "wait change: the loop changes" 0 "$(ob wait -b "$B" change --window 'title:^R$' >/dev/null; echo $?)"
  # Absence and timeouts
  local t0=$SECONDS
  check_eq "a window that never comes: 124" 124 "$(ob wait -b "$B" --timeout 1s window 'title:^nope$' >/dev/null; echo $?)"
  check "...at the deadline" test $((SECONDS - t0)) -le 4
  check_eq "several windows match: exit 2" 2 "$(ob wait -b "$B" window foot >/dev/null 2>&1; echo $?)"
  # cmd: a condition inside the box, here one that becomes true a second later
  ob run -b "$B" -d -- sh -c 'sleep 1; touch /tmp/late' >/dev/null 2>&1
  out=$(ob wait -b "$B" cmd -- test -e /tmp/late); rc=$?
  check_eq "wait cmd: 0 once it succeeds" 0 "$rc"
  check_match "...after it did" "^satisfied: cmd after (0\.[5-9]|[1-9])" "$out"
  check_eq "wait cmd that keeps failing: 124" 124 "$(ob wait -b "$B" --timeout 500ms cmd -- false >/dev/null; echo $?)"
  # The box going down mid-wait is unknown (1), never satisfied
  ob wait -b "$B" still --quiet 30s > "$TMP/wait.out" 2>&1 & local w=$!
  sleep 1.5; ob down "$B" >/dev/null 2>&1
  wait $w; rc=$?
  check_eq "the box went down during wait: exit 1" 1 "$rc"
  check_match "...said" "^unknown: box '$B' went down after" "$(cat "$TMP/wait.out")"
}

# --- runner --------------------------------------------------------------------------------------

UNIT=(t_unit_config t_unit_wait t_unit_window_select t_unit_guard_exec_host t_unit_live_edit t_unit_parse_mode t_unit_duration t_unit_mount_rules t_unit_refusals t_unit_run_named_dead t_unit_cli t_unit_uwsm_guard t_unit_install t_unit_host_session t_unit_guard_settings t_unit_seed_copy t_unit_version
  t_unit_registry t_unit_leak_scan)
BOX=(t_leak_control t_main t_window t_wait t_new t_keys t_peek t_guard t_uwsm_app t_widget t_throwaway t_throwaway_home t_throwaway_killed t_throwaway_dead t_isolated t_isolated_no_pidfile t_idle t_reap_race t_run_idle t_stock_bar
  t_systemd t_hostile t_race t_failed_up t_hyprland_dies t_no_shell t_stale_pid)

# The host's session through the CLI's own lookup, so the suite runs from a guarded shell too.
hostctl() { lib host_hyprctl "$@"; }

# What this run ran on, for comparing a failure with a run that passed: the checkout, the stack a box
# is made of (the Hyprland binary boxes start and the host's running one can differ after an upgrade),
# and the GPU.
provenance() {
  local sha dirty="" so aq rn drv
  sha=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null) || sha=unknown
  [ -z "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ] || dirty=+dirty
  so=$(ldd "$(command -v Hyprland)" 2>/dev/null | awk '/libaquamarine/ {print $1; exit}')
  aq=$(readlink -f "$ROOT/build/prefix/lib/$so" 2>/dev/null); aq=${aq##*.so.}
  rn=$(lib render_node 2>/dev/null); drv=$(readlink -f "/sys/class/drm/${rn##*/}/device/driver" 2>/dev/null)
  printf 'omabox %s %s%s, Hyprland %s (host session %s), aquamarine %s (system %s), quickshell %s, labwc %s, bwrap %s, %s (%s), kernel %s\n' \
    "$(cat "$ROOT/VERSION")" "$sha" "$dirty" "$(Hyprland --version 2>/dev/null | awk 'NR == 1 {print $2}')" \
    "$(hostctl -j version | jq -r '.tag // "?"')" "${aq:-?}" "$(pkg-config --modversion aquamarine 2>/dev/null || echo ?)" \
    "$(quickshell --version 2>/dev/null | awk 'NR == 1 {print $2}')" "$(labwc --version 2>/dev/null | awk 'NR == 1 {print $2}')" \
    "$(bwrap --version | awk '{print $2}')" "${rn:-no render node}" "${drv##*/}" "$(uname -r)"
}

# The runner as one function, read whole before it starts: an edit to this file during a run (another
# agent's, in the same checkout) cannot change what the rest of the run does (as finding 66 did for
# bin/omabox).
main() {
  # (Underscored: the tests run inside this function and see its locals.)
  local _a _pats=() _filtered=0 _t _p _sel=() _n _unit_n=0
  for _a; do case $_a in --strict) STRICT=1 ;; *) _pats+=("$_a") ;; esac; done
  tests=("${UNIT[@]}" "${BOX[@]}")
  if [ "${_pats[*]}" = unit ]; then tests=("${UNIT[@]}")
  elif [ ${#_pats[@]} -gt 0 ]; then
    _filtered=1
    for _t in "${tests[@]}"; do for _p in "${_pats[@]}"; do [[ $_t == *"$_p"* ]] && { _sel+=("$_t"); break; }; done; done
    # A mistyped pattern would run nothing and pass: each one must match a test.
    for _p in "${_pats[@]}"; do
      [[ " ${tests[*]} " == *"$_p"* ]] || { echo "no test matches '$_p' (tests: ${tests[*]#t_})"; exit 2; }
    done
    tests=("${_sel[@]}")
  fi

  hostctl -j version >/dev/null 2>&1 || { echo "run from the Hyprland session (read-only hyprctl)"; exit 2; }
  ws0=$(hostctl -j activeworkspace | jq .id) win0=$(hostctl -j activewindow | jq -r '.address // ""')
  # The run's folder, and the last 5 runs' (a run still going, another agent's, is never removed).
  (umask 077; mkdir -p "$EVID")
  local _d; for _d in $(find "${EVID%/*}" -mindepth 1 -maxdepth 1 -type d -name '*-t[0-9]*' | sort -r | tail -n +6); do
    kill -0 "${_d##*-t}" 2>/dev/null || rm -rf "$_d"
  done
  provenance > "$EVID/provenance"
  echo "provenance: $(cat "$EVID/provenance")"
  # The host's event socket, listened to for the whole run (finding 80).
  local _sig; _sig=$(bash -c 'source "$1"; host_session; echo "$HOST_SIG"' _ "$TMP/lib/bin/omabox")
  HYPRLAND_INSTANCE_SIGNATURE=$_sig python3 -c "$WATCHER" "$XDG_RUNTIME_DIR/hypr/$_sig/.socket2.sock" $$ >> "$EVID/host-events.log" 2>&1 &
  WATCH_PID=$!
  until_ok 5 grep -q ' == watching$' "$EVID/host-events.log" || { echo "cannot listen to the host's events: $(cat "$EVID/host-events.log")"; exit 2; }

  for CUR in "${tests[@]}"; do
    echo "${CUR#t_}"
    _n=$((pass + fail + ${#skips[@]}))
    rm -f "$TMP/until.last"
    printf '%s == %s\n' "$(date +%s.%3N)" "$CUR" >> "$EVID/host-events.log"
    "$CUR"; notes
    [ $((pass + fail + ${#skips[@]})) -gt "$_n" ] || no "the test ran checks" "none: it returned before its first"
    [[ $CUR != t_unit_* ]] || _unit_n=$((_unit_n + pass + fail + ${#skips[@]} - _n))
    host_scan "$CUR"
  done

  echo "host"
  CUR=host
  printf '%s == host\n' "$(date +%s.%3N)" >> "$EVID/host-events.log"
  sleep 0.5   # the last test's events, still on their way
  host_scan host
  check "the host's event watcher ran to the end" kill -0 "$WATCH_PID"
  if [[ " ${tests[*]} " == *" t_leak_control "* ]]; then
    if [ "${LEAK_PROVEN:-0}" = 1 ]; then ok "the leak detector is proven (t_leak_control)"
    else no "the leak detector is proven (t_leak_control)" "it failed: a clean host log proves nothing this run"; fi
  else echo "       (the leak detector is unproven this run: t_leak_control did not run)"; fi
  host_same "host workspace untouched" "$ws0" "$(hostctl -j activeworkspace | jq .id)" workspace
  host_same "host focused window untouched" "$win0" "$(hostctl -j activewindow | jq -r '.address // ""')" window
  if [ $_filtered = 0 ]; then
    [ ${#tests[@]} = ${#UNIT[@]} ] || [ $((pass + fail)) -ge $MIN_CHECKS ] ||
      no "a full run has at least $MIN_CHECKS checks" "$((pass + fail)) ran (a test stopped checking?)"
    [ $_unit_n -ge $MIN_UNIT ] || no "the unit tests have at least $MIN_UNIT checks" "$_unit_n ran"
  fi

  echo
  echo "$pass passed, $fail failed, ${#skips[@]} skipped"
  for f in "${skips[@]}"; do echo "  skip $f"; done
  for f in "${failed[@]}"; do echo "  FAIL $f"; done
  [ $fail = 0 ] || echo "evidence: $EVID"
  [ $fail = 0 ]
}

main "$@"
