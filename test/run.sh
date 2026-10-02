#!/usr/bin/env bash
# shellcheck disable=SC2016 # single-quoted $VARS are expanded inside the box
# shellcheck disable=SC2329 # tests are called by name; see the exit at the end
# omabox's regression suite: what NOTES' findings verified, as checks that run in real boxes.
#
#   test/run.sh              every test, box tests in parallel (~1.5 min on 16 CPUs; boxes are named
#                            t<pid>-*, all torn down)
#   -j N (or OMABOX_TEST_JOBS=N): N box tests at a time (default: half the CPUs, at most 8); -j 1 runs
#                            them one at a time (~8.5 min), so a leak is pinned to one test
#   test/run.sh unit         only the fast ones (no box)
#   test/run.sh PATTERN...   tests whose name matches any PATTERN (e.g. isolated systemd); a PATTERN
#                            that matches no test exits 2
#   --strict (or OMABOX_TEST_STRICT=1): a skipped check (a tool this machine lacks) fails
#   test/run.sh --installed [ARGS...]   the same against omabox installed as a package would be:
#                            read-only at /usr/lib/omabox, /usr/bin/omabox (a throwaway namespace)
#
# Never touches the real desktop: every box is headless. The host's Hyprland is only read: its event
# socket is listened to for the whole run, and a window, focus change or virtual keyboard of the
# suite's showing up there fails the test that was running (t_leak_control first proves that on a box
# standing in for the host); the end checks that the host's focused workspace and window are what
# they were, or changed by events that were not the suite's (you working meanwhile). omabox's
# workspace coming up there fails it too, unless it brings focus to a window that is not the suite's
# (your box, your peek, your own app: you went there) or you were on it when the run started. A peek
# you open at the run's t<pid>-* boxes (bar widget, omabox peek) is noted, not a leak: `omabox peek`
# marks its window as yours (finding 121), and a check it holds up (a box you watch is not reaped) is
# skipped, saying so. A peek from an omabox older than this suite has no mark and fails the run, as
# does one any command of the suite's opens. Pointer motion
# has no event: it is only seen when it moves focus. Needs a Hyprland session and python3. The
# network tests run throwaway HTTP servers on free ports they find, and t_connected makes one
# connection from a box to its gateway, the router.
# A test that runs no check fails, and so does a full run with fewer checks than MIN_CHECKS.
# Each run keeps a folder (the last 5 runs are kept) in ~/.local/state/omabox/test/: its provenance
# and, for a test's first failure, what its boxes showed then (screen, windows, focus, pointer,
# devices, logs) and every failure's full output.
set -uo pipefail
# The guard tests point HOME at a temp dir; these would still lead them to the real settings.
unset CLAUDE_CONFIG_DIR CODEX_HOME
# An agent running the suite would otherwise give every default-named box its session's suffix.
unset OMABOX_SESSION CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID CLAUDE_PID OMABOX_AGENT_PID

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# --installed (issue #50, finding 128): the suite against omabox as a package installs it. This
# checkout's tracked files and built tools, read-only at /usr/lib/omabox with /usr/bin/omabox linking
# to it, in a throwaway mount namespace: overlays on /usr/lib and /usr/bin made as root of a user
# namespace, then back to your own uid in one below it (unshare sets no no_new_privs, which boxes
# behind pasta need: finding 89). The suite run there is the install's own copy, its ROOT
# /usr/lib/omabox; nothing on the host changes. (The files stay yours, not root's: read-only, not
# root-owned.)
if [ "${1:-}" = --installed ]; then
  shift
  [ ! -e /usr/lib/omabox ] || { echo "--installed: /usr/lib/omabox exists here already (a real install?): run its own test/run.sh"; exit 2; }
  inst=$(mktemp -d) || exit 2
  trap 'chmod -R u+w "$inst" 2>/dev/null; rm -rf "$inst"' EXIT
  mkdir -p "$inst/tree" "$inst/up/lib" "$inst/up/bin" "$inst/work/lib" "$inst/work/bin"
  git -C "$ROOT" ls-files -z | (cd "$ROOT" && tar --null -cf - -T -) | tar -xf - -C "$inst/tree" || exit 2
  for t in "$ROOT"/tools/*/omabox-*; do
    [ -x "$t" ] && [ -f "$t" ] || continue
    cp "$t" "$inst/tree/${t#"$ROOT"/}"
  done
  chmod -R a-w "$inst/tree"
  # shellcheck disable=SC2016 # expanded by the namespace's shell
  unshare -r -m bash -c '
    set -e; i=$1; shift
    mount -t overlay overlay -o "lowerdir=/usr/lib,upperdir=$i/up/lib,workdir=$i/work/lib" /usr/lib
    mkdir /usr/lib/omabox; mount --bind "$i/tree" /usr/lib/omabox; mount -o remount,bind,ro /usr/lib/omabox
    mount -t overlay overlay -o "lowerdir=/usr/bin,upperdir=$i/up/bin,workdir=$i/work/bin" /usr/bin
    ln -s ../lib/omabox/bin/omabox /usr/bin/omabox
    exec unshare -U --map-user="$1" --map-group="$2" env OMABOX_TEST_INSTALLED=1 /usr/lib/omabox/test/run.sh "${@:3}"
  ' _ "$inst" "$(id -u)" "$(id -g)" "$@"
  exit $?
fi
CLI=$ROOT/bin/omabox
# An installed omabox: a package's /usr/lib/omabox, or --installed's stand-in. Both are read-only, a
# checkout never is, so the suite tells them apart by itself (it once failed 13 checks run from a real
# install, which sets no OMABOX_TEST_INSTALLED: that marks --installed's user namespace only).
INSTALLED=0; [ -w "$ROOT" ] || INSTALLED=1
P=t$$                      # box name prefix
export OMABOX_SUITE=$P     # marks what the suite starts on the host, for the leak detector
TMP=$(mktemp -d)
pass=0 fail=0 failed=() skips=()
STRICT=${OMABOX_TEST_STRICT:-0}
# A full run's floor: a test that silently stops checking shows up here even when everything that
# did run passed. About 90% of the fewest seen (2026-10-01: 1217 checks in --installed on a VM, 1350
# from a checkout; 651 unit), so skips on another machine still clear it. Raise it as tests grow.
MIN_CHECKS=1100 MIN_UNIT=580
EVID=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/test/$(date +%Y%m%d-%H%M%S)-$P
SERVERS=()                 # host-side test servers, stopped on exit

cleanup() {
  local b p
  # A run stopped midway (Ctrl-C): whatever it still runs goes first (parallel tests' subshells and
  # what they started, which ignore SIGINT: bash leaves background jobs so), then every box of its.
  descendants $$ > "$TMP/descendants" 2>/dev/null
  while read -r p; do [ "$p" = "$BASHPID" ] || kill "$p" 2>/dev/null; done < "$TMP/descendants"
  for b in $("$CLI" ls --json 2>/dev/null | jq -r '.[].name' | grep "^$P-"); do "$CLI" down "$b" >/dev/null 2>&1; done
  cat "$TMP"/new-[ab] 2>/dev/null | while read -r b; do "$CLI" down "$b" >/dev/null 2>&1; done   # t_new's box-N boxes
  [ ${#SERVERS[@]} = 0 ] || kill "${SERVERS[@]}" 2>/dev/null
  [ -n "${WATCH_PID:-}" ] && kill "$WATCH_PID" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# descendants PID: every process under it, deepest first.
descendants() { local c; for c in $(pgrep -P "$1"); do descendants "$c"; echo "$c"; done; }

ob() { "$CLI" "$@"; }
# Notes from inside checks (a slow wait), whose output is captured: shown before the next result.
# (NOTES and UNTIL are per test when tests run in parallel.)
NOTES=$TMP/notes UNTIL=$TMP/until.last
note() { printf '       (%s)\n' "$*" >> "$NOTES"; }
notes() { [ ! -s "$NOTES" ] || { cat "$NOTES"; : > "$NOTES"; }; }
ok() { notes; pass=$((pass + 1)); printf '  \e[32mok\e[0m   %s\n' "$1"; }
no() {
  if [ -n "${HELD:-}" ] && your_peek "$HELD"; then
    local h=$HELD; HELD=""; skip "$1" "held by your peek at $h: a box you watch is in use (not reaped, it gets marks)"; HELD=$h; return
  fi
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
  [ ! -s "$UNTIL" ] || cp "$UNTIL" "$e/last-wait.txt"
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
# Finding 125: whether the boxes an omabox starts lack aquamarine's fix for nested Wayland outputs
# (the system's 0.15.1 or older, no private build or OMABOX_AQUAMARINE=system). ARGS: that omabox.
aq_unfixed() { "$@" --version 2>/dev/null | grep -q '^aquamarine: .*without the fix'; }
# Issue #45 (finding 121): a peek window you opened on one of the run's boxes (bar widget, omabox
# peek; its process marked OMABOX_PEEK_BY=you) makes that box in use, by design: it is not reaped for
# idling or for its agent's exit, and click/keys write marks for it. `held BOX CHECK...` runs a check
# that such a peek would fail; failing while one is open on BOX, it is skipped, saying so (`no`).
your_peek() {
  local p
  for p in $(pgrep -f "omabox-peek --box $XDG_RUNTIME_DIR/omabox/$1/run/"); do
    tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep -x OMABOX_PEEK_BY=you >/dev/null && return 0
  done
  return 1
}
held() { HELD=$1; shift; "$@"; HELD=""; }
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
      printf 'until_ok %s %s\nlast output: %s\n' "$t" "$*" "$out" > "$UNTIL"
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
KEEP = ("openwindow>>", "closewindow>>", "activewindowv2>>", "workspacev2>>", "activespecial>>", "activelayout>>")
def hypr(what):
    return json.loads(subprocess.run(["hyprctl", "-j", what], capture_output=True, text=True, timeout=5).stdout)
def focused():
    try:
        return describe(hypr("activewindow"))
    except Exception as e:
        return "? %s" % e
def opened(addr):   # a peek window just opened: whose (its process environment says who asked)
    try:
        return describe(next(w for w in hypr("clients") if w.get("address") == "0x" + addr))
    except Exception as e:
        return "0x%s ? %s" % (addr, e)
def describe(w):
    pid, cls, tags = w.get("pid", -1), w.get("class"), []
    try:
        with open("/proc/%d/environ" % pid, "rb") as f:
            for kv in f.read().split(b"\0"):
                k, _, v = kv.decode(errors="replace").partition("=")
                if k in ("OMABOX_SUITE", "OMABOX_NAME", "OMABOX_PEEK_BY"): tags.append("%s=%s" % (k, v))
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
        if l.startswith("openwindow>>") and l[12:].split(",", 3)[2:3] == ["omabox-peek"]: say("+ " + opened(l[12:].split(",")[0]))
'

# leak_scan PREFIX < LOG: what in a watcher's log is the suite's (a box's name starts with PREFIX, the
# suite's own processes carry OMABOX_SUITE=${PREFIX%-}) or omabox's, one `leak: ...` line each, and one
# `note: ...` line for the rest (the user's own windows and workspace switches while the suite runs).
# Windows of omabox's own: an interactive box (class aquamarine; its title names no box, so any counts,
# unless it is your own box opened meanwhile) and peek (class omabox-peek, "omabox peek: NAME"; the
# tool started by hand is "omabox peek"). A peek window's process says who asked for it (finding
# 121), on the `+` line the watcher logs after it opens and on its focus lines: OMABOX_SUITE is this
# run's commands, a leak; OMABOX_PEEK_BY=you is `omabox peek` run by you (the bar widget), noted as
# watched by you; neither is a peek something else opened, a leak. A virtual keyboard's layout event
# is `omabox keys` reaching that compositor (ours is anonymous there: hl-virtual-keyboard-unknown);
# Omarchy's input method, fcitx5, is one of yours, and OMABOX_TEST_HOST_KEYBOARDS (a regex) names
# others (wayvnc). WS, when given, is omabox's workspace (1-99 or special:NAME, which shows with
# activespecial, not workspacev2): it coming up is judged by the focus it brings, which Hyprland sends
# just before it (a peek's --focus also once after; issue #56). Focus on a window that is not the
# suite's (your box, your peek, your own app) is a note: you went there. Focus on the suite's, on
# something omabox's that is not marked yours, or on nothing (the suite's boxes open nothing there)
# is a leak.
leak_scan() {
  local pre=$1 ws=${2:-} line data cls title kb notes=() peek="" wsup="" prev="" who
  local wsleak="leak: omabox's workspace $ws came up (if you went there yourself, run the suite again)"
  while IFS= read -r line; do
    line=${line#* }
    # What the next line decides: a peek window's `+` line; the focus omabox's workspace brought.
    if [ -n "$peek" ] && [[ $line != '+ '* ]]; then echo "leak: a window opened: $peek"; peek=""; fi
    if [ -n "$wsup" ] && [[ $line != '~ '* && $line != activewindowv2\>\>?* ]]; then echo "$wsleak"; wsup=""; fi
    # What the focus line just before says, for a workspace line (whose, or `leak`); gone after any other.
    [[ $line == '~ '* || $line == workspacev2\>\>* || $line == activespecial\>\>* ]] || prev=""
    case $line in
      openwindow\>\>*)
        IFS=, read -r _ _ cls title <<<"${line#*>>}"
        if [ "$cls" = aquamarine ] && data=$(their_new_box "$pre"); then notes+=("window of your box $data")
        elif data=$(omabox_window "$pre" "$cls" "$title"); then
          if [ "$cls" = omabox-peek ]; then peek=$data; else echo "leak: a window opened: $data"; fi
        else notes+=("window $cls"); fi ;;
      '+ '*)
        [ -n "$peek" ] || continue
        title=${line#* title=}
        if [[ " $line " == *" OMABOX_SUITE=${pre%-} "* ]]; then echo "leak: a window opened: a peek window, by this run's commands: ${line#+ }"
        elif [[ " $line " == *" OMABOX_PEEK_BY=you "* ]]; then notes+=("a peek at ${title#omabox peek: } watched by you")
        else echo "leak: a window opened: $peek"; fi
        peek="" ;;
      '~ '*)
        cls=${line#* class=} title=""
        [[ $cls != *' title='* ]] || { title=${cls#* title=}; cls=${cls%% title=*}; }
        data=""; [[ $cls != omabox-peek || " $line " != *" OMABOX_PEEK_BY=you "* ]] || data=yours
        who=""   # whose the focus is, when not the suite's
        if [[ " $line " == *" OMABOX_SUITE=${pre%-} "* || $line == *" OMABOX_NAME=$pre"* || $line == *" box=$pre"* ]]; then
          echo "leak: focus went to a window of this run's: ${line#\~ }"
        elif [[ $line == *" OMABOX_NAME="* ]]; then
          echo "leak: focus went to a box's process (another run's, or yours): ${line#\~ }"
        elif [[ $line == *" box="* ]]; then data=${line#* box=}; who="your box ${data%% *}"; notes+=("focus on $who")
        elif [ -n "$data" ]; then who="your peek"; notes+=("focus on your peek at ${title#omabox peek: }")
        elif data=$(omabox_window "$pre" "$cls" "$title"); then echo "leak: focus went to $data"
        elif [[ $line == '~ ? '* ]]; then notes+=("focus unknown (${line#\~ ? })")   # the watcher could not ask
        else who=$cls; notes+=("focus $cls"); fi
        if [ -n "$wsup" ]; then
          if [ -n "$who" ]; then notes+=("workspace $ws for $who"); else echo "$wsleak"; fi
          wsup=""
        fi
        prev=${who:-leak} ;;
      activelayout\>\>*virtual-keyboard*)
        kb=${line#*>>}; kb=${kb%%,*}
        if [ "$kb" = hl-virtual-keyboard-fcitx5 ] ||
           { [ -n "${OMABOX_TEST_HOST_KEYBOARDS:-}" ] && [[ $kb =~ $OMABOX_TEST_HOST_KEYBOARDS ]]; }; then notes+=("keyboard $kb")
        else echo "leak: keys from a virtual keyboard ($kb)"; fi ;;
      workspacev2\>\>*|activespecial\>\>*)
        # workspacev2>>ID,NAME; activespecial>>NAME,MONITOR (NAME empty: the special one closed).
        data=${line#*>>}
        if [[ $line == workspacev2* ]]; then data=${data#*,}; else data=${data%,*}; fi
        if [ -z "$data" ]; then :
        elif [ -z "$ws" ] || [ "$data" != "$ws" ]; then notes+=("workspace $data")
        elif [ "$prev" = leak ]; then echo "$wsleak"
        elif [ -n "$prev" ]; then notes+=("workspace $ws for $prev")
        else wsup=1; fi ;;
    esac
  done
  [ -z "$peek" ] || echo "leak: a window opened: $peek"
  [ -z "$wsup" ] || echo "$wsleak"
  [ ${#notes[@]} = 0 ] || echo "note: not the suite's: $(printf '%s\n' "${notes[@]}" | awk '!seen[$0]++ && n++ < 8' | paste -sd, - | sed 's/,/, /g')"
}
# omabox_window PREFIX CLASS TITLE: says what the window is when it is one of omabox's that is not the
# user's (TITLE empty: unknown). A peek window here is one not marked as yours (leak_scan).
omabox_window() {
  case $2 in
    aquamarine) echo "an interactive box's window ($2${3:+ \"$3\"})" ;;
    omabox-peek)
      case $3 in
        "omabox peek: $1"*|"omabox peek"|"") echo "a peek window${3:+ \"$3\"} of this run's box, not marked as yours: a command other than \`omabox peek\` opened it, or you did with an omabox older than this suite (bar widget, omabox peek): if you opened it, that is the cause; run again without peeking" ;;
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
# slice FILE FROM [TO]: a watcher log's lines after the marker "== FROM" (up to "== TO", or the end),
# without other markers (tests running in parallel mark their starts and ends in between: a marker
# between an event and the line that decides it would read as a leak).
slice() { awk -v a="== $2" -v b="== ${3:-}" '{m = substr($0, index($0, " ") + 1)} f && m == b {exit} f && m !~ /^== / {print} m == a {f = 1}' "$1"; }
# After each test: what of it reached the host's desktop fails it; what is not the suite's is shown.
# host_scan TEST [END]: from its marker to END's (a test run in parallel: "/TEST"), or to now. A test
# run in parallel shares its window with the tests beside it: a leak there fails each, naming them.
host_scan() {
  local out beside=""; out=$(slice "$EVID/host-events.log" "$1" "${2:-}" | leak_scan "$P-" "$HWS")
  [ -z "${2:-}" ] || beside=$(beside "$1")
  [[ $out != *leak:* ]] || no "nothing of it${beside:+, or of the tests beside it,} reached the host desktop" "$(grep '^leak:' <<<"$out")${beside:+ (beside it: $beside)}"
  [[ $out != *note:* ]] || printf '       (host, %s)\n' "$(grep '^note:' <<<"$out" | cut -c7-)"
}
# beside TEST: the tests that ran in parallel with TEST at any time (from the log's markers).
beside() {
  awk -v t="$1" '$2 == "==" && $3 ~ /^\/?t_/ { n = $3; e = sub(/^\//, "", n)
      if (n == t) { if (e) exit; mine = 1; for (k in run) seen[k] = 1; next }
      if (!e) { run[n] = 1; if (mine) seen[n] = 1 } else delete run[n] }
    END { for (n in seen) printf "%s ", n }' "$EVID/host-events.log" | sed 's/ $//'
}
# gap_events: the events of a parallel phase while none of its tests ran (between one's end and the
# next one's start), which no test's window holds.
gap_events() {
  awk '$2 == "==" && $3 == "parallel" {on = 1; next} $2 == "==" && $3 == "/parallel" {exit} !on {next}
    $2 == "==" && $3 ~ /^\/t_/ {r--; next} $2 == "==" && $3 ~ /^t_/ {r++; next} $2 == "==" {next} r == 0' "$EVID/host-events.log"
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
  if [ -n "$seen" ] && ! leak_scan "$P-" "$HWS" < "$log" | grep '^leak:' >/dev/null; then
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
  # (/dev/net/tun hidden in a mount namespace of its own: pasta would fail inside the box, 10 s later)
  check_match "a connected box without /dev/net/tun is refused up front" "needs /dev/net/tun" \
    "$(unshare -Urm bash -c 'mount -t tmpfs none /dev/net && exec "$0" up "$1"' "$CLI" "$P-r19" 2>&1)"
  # (bwrap would fail at its uid map behind pasta, and only box.log would say so, 10 s later; a
  # connected box falls back to none there instead, t_no_new_privs)
  check_match "an isolated box is refused up front from a no_new_privs process" \
    "cannot start from a no_new_privs process" "$(setpriv --no-new-privs "$CLI" up "$P-r20" --net isolated 2>&1)"
  check_fails "...before its box dir is made" test -e "$XDG_RUNTIME_DIR/omabox/$P-r20"
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
  wait "$p"; local rc=$?
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
  check_eq "confirm-close yes reads on" confirm-close=on "$(cfg confirm-close yes | tail -1)"
  check_eq "bar-icon auto" bar-icon=auto "$(cfg bar-icon auto)"
  check_fails "bar-icon takes auto or always" env HOME="$h" "$CLI" config bar-icon sometimes
  check_eq "--json for the widget" '{"workspace":"special:omabox","confirm-close":"on","bar-icon":"auto"}' \
    "$(HOME=$h "$CLI" config --json | jq -c 'del(.["confirm-close-available"], .version)')"
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

# The box's shell.json (finding 21) and its workspace numbers (issue #21, finding 115): a bar left
# with no omarchy.workspaces gets one where a left-out plugin's workspace widget was, else after the
# menu; never a second one, nor next to a mounted plugin that shows workspaces.
t_unit_bar_filter() {
  # shellcheck disable=SC2329 # called below
  bar() { lib shell_json_filter "$1" true "$2" <<<"$3" | jq -c "${4:-.bar.layout}"; }
  local user='{"bar":{"centerAnchor":"x.solari","layout":{"left":[{"id":"omarchy.menu"},{"id":"omarchy.system-update"},{"id":"x.gauge"}],
    "center":[{"id":"x.indicators"},{"id":"x.solari"},{"id":"njpatel.omapager"}],"right":[{"id":"omarchy.tray"},{"id":"x.clock"}]}}}'
  check_eq "no workspace widget at all: after the menu, the rest filtered" \
    '{"left":[{"id":"omarchy.menu"},{"id":"omarchy.workspaces"},{"id":"omarchy.system-update"}],"center":[],"right":[{"id":"omarchy.tray"}]}' \
    "$(bar '[]' '[]' "$user")"
  check_eq "...the dropped centre anchor gone" null "$(bar '[]' '[]' "$user" .bar.centerAnchor)"
  local pager='{"bar":{"layout":{"left":[{"id":"omarchy.menu"}],"center":[{"id":"omarchy.clock"},{"id":"x.pager","n":1},"y.pager",{"id":"x.other"}]}}}'
  check_eq "a left-out plugin's workspace widget: omarchy.workspaces in its place, once" \
    '{"left":[{"id":"omarchy.menu"}],"center":[{"id":"omarchy.clock"},{"id":"omarchy.workspaces"}]}' \
    "$(bar '[]' '["x.pager","y.pager"]' "$pager")"
  check_eq "...one given as a bare id too" '["omarchy.clock","omarchy.workspaces"]' \
    "$(bar '[]' '["y.pager"]' "$pager" '[.bar.layout.center[] | if type == "object" then .id else . end]')"
  check_eq "a mounted one stays, and no omarchy.workspaces" '{"left":[{"id":"omarchy.menu"}],"center":[{"id":"omarchy.clock"},{"id":"x.pager","n":1}]}' \
    "$(bar '["x.pager"]' '["x.pager"]' "$pager")"
  check_eq "...nor for one mounted that the user's bar does not have (it is added where its manifest says)" \
    '{"left":[{"id":"omarchy.menu"},{"id":"omarchy.system-update"}],"center":[],"right":[{"id":"omarchy.tray"}]}' \
    "$(bar '["z.pager"]' '["z.pager"]' "$user")"
  check_eq "the user's own omarchy.workspaces: left as it is" '{"right":[{"id":"omarchy.workspaces"}],"center":[]}' \
    "$(bar '[]' '["x.pager"]' '{"bar":{"layout":{"right":[{"id":"omarchy.workspaces"}],"center":[{"id":"x.pager"}]}}}')"
  check_eq "no menu: at the start of left" '{"left":[{"id":"omarchy.workspaces"},{"id":"omarchy.clock"}]}' \
    "$(bar '[]' '[]' '{"bar":{"layout":{"left":[{"id":"omarchy.clock"}]}}}')"
  check_eq "no left: one made" '{"right":[{"id":"omarchy.tray"}],"left":[{"id":"omarchy.workspaces"}]}' \
    "$(bar '[]' '[]' '{"bar":{"layout":{"right":[{"id":"omarchy.tray"}]}}}')"
  check_eq "no layout of the user's (the shell's default has workspaces): none made" null \
    "$(bar '[]' '[]' '{"bar":{"position":"top"}}')"
  # Which plugins show workspaces: their manifests, the user's and the mounted ones'.
  local h=$TMP/pagers; mkdir -p "$h/.config/omarchy/plugins/"{ws,notif,bad,svc} "$h/mnt"
  echo '{"id":"a.ws","barWidget":{"displayName":"Pills","description":"Workspace pills per monitor"}}' > "$h/.config/omarchy/plugins/ws/manifest.json"
  echo '{"id":"njpatel.omapager","barWidget":{"displayName":"Notification state","aliases":["notifications","pager"]}}' > "$h/.config/omarchy/plugins/notif/manifest.json"
  echo '{"id":"x.svc","description":"moves workspaces around","barWidget":null}' > "$h/.config/omarchy/plugins/svc/manifest.json"
  echo '{not json' > "$h/.config/omarchy/plugins/bad/manifest.json"
  echo '{"id":"b.mine","barWidget":{"displayName":"Mine","aliases":["workspaces"]}}' > "$h/mnt/manifest.json"
  check_eq "workspace widgets: by name, description or alias; not a notifications pager, a service or a broken manifest" \
    '["a.ws","b.mine"]' "$(printf 'b.mine\t%s\n' "$h/mnt" | HOME=$h lib workspace_widgets | jq -c .)"
  check_eq "...none: an empty list" '[]' "$(HOME=$TMP/nohome lib workspace_widgets < /dev/null | jq -c .)"
}

# up --new (finding 73): a free box-N name, printed; two at once never get the same one. The names
# are the user's namespace too (box-1 may be theirs): only the ones printed here are taken down.
t_new() {
  local a=$TMP/new-a b=$TMP/new-b   # fixed names: cleanup takes these boxes down if the suite stops here
  "$CLI" up --new --no-shell --idle 0 > "$a" 2>/dev/null & local pa=$!
  "$CLI" up --new --no-shell --idle 0 > "$b" 2>/dev/null & local pb=$!
  wait "$pa" "$pb"
  local na nb; na=$(cat "$a") nb=$(cat "$b")
  check_match "up --new prints a box-N name" '^box-[0-9]+$' "$na"
  check "...two at once, two different names ($na, $nb)" test -n "$nb" -a "$na" != "$nb"
  check "...both up" bash -c "'$CLI' ls --json | jq -e '[.[] | select(.name == \"$na\" or .name == \"$nb\") | select(.state == \"up\")] | length == 2'"
  [ -n "$na" ] && ob down "$na" >/dev/null 2>&1; [ -n "$nb" ] && ob down "$nb" >/dev/null 2>&1
  rm -f "$a" "$b"   # down: a later box-N of someone else's may get these names
}

# One version for the CLI and the widget (CHANGELOG.md).
# rich_text FILE...: each QML element that shows text (Text, Label, TextEdit, TextArea, TextField)
# with no `textFormat: Text.PlainText` of its own, as FILE:LINE TYPE. Braces in strings and //
# comments are ignored; a component declared from Text counts, its uses (`SettingText { }`) do not.
RICH_TEXT='
import re, sys
shows = re.compile(r"(?<![\w.])(Text|Label|TextEdit|TextArea|TextField)\s*$")
plain = re.compile(r"textFormat:\s*Text\.PlainText")
for path in sys.argv[1:]:
    stack = []   # one [line, type or None, plain] per open brace
    for n, line in enumerate(open(path), 1):
        line = re.sub(r"\"(\\.|[^\"\\])*\"", "\"\"", line).split("//")[0]
        at = 0
        for m in re.finditer(r"[{}]", line):
            if stack and plain.search(line, at, m.start()): stack[-1][2] = True
            at = m.end()
            if m.group() == "{":
                t = shows.search(line[:m.start()])
                stack.append([n, t.group(1) if t else None, False])
            elif stack:
                ln, typ, ok = stack.pop()
                if typ and not ok: print("%s:%d %s" % (path, ln, typ))
        if stack and plain.search(line, at): stack[-1][2] = True
'
rich_text() { python3 -c "$RICH_TEXT" "$@"; }

t_unit_version() {
  local v; v=$(cat "$ROOT/VERSION")
  check_eq "omabox --version" "omabox $v" "$("$CLI" --version | head -1)"
  check_match "...then the aquamarine boxes use (finding 125)" "^aquamarine: (a private build, |the system's, )" "$("$CLI" --version | sed -n 2p)"
  check_eq "the widget's manifest has it" "$v" "$(jq -r .version "$ROOT/plugin/manifest.json")"
  check_match "...and its Settings face" "pluginVersion: \"$v\"" "$(grep pluginVersion "$ROOT/plugin/Panel.qml")"
  # The widget shows box names and errors agents wrote, in the user's real bar: never as rich text.
  check_eq "every text in the widget is plain text" "" "$(rich_text "$ROOT"/plugin/*.qml)"
  printf '%s\n' 'Item {' '  Text { text: "a {"; color: "red" }' '  component T: Text {' '    textFormat: Text.PlainText' '  }' \
    '  T { text: "b" }' '  Label {' '    Text { textFormat: Text.PlainText }' '  }' '}' > "$TMP/rich.qml"
  check_eq "...a check that finds one (and not the plain ones)" "$TMP/rich.qml:2 Text $TMP/rich.qml:7 Label" "$(rich_text "$TMP/rich.qml" | tr '\n' ' ' | sed 's/ $//')"
  check_match "...and the changelog" "^## $v " "$(grep "^## $v " "$ROOT/CHANGELOG.md")"
  check_eq "config --json names it, for a widget left from before an upgrade (finding 133)" "$v" "$("$CLI" config --json | jq -r .version)"
  # Agent Skills hosts cap a skill's description at 1024 characters (it grows with each trigger).
  check "the skill's description is at most 1024 characters" \
    test "$(sed -n 's/^description: //p' "$ROOT/skill/SKILL.md" | head -n 1 | tr -d '\n' | wc -c)" -le 1024
  if command -v omarchy-plugin-validate >/dev/null; then
    check "the widget passes omarchy-plugin-validate" omarchy-plugin-validate "$ROOT/plugin"
  else
    skip "the widget passes omarchy-plugin-validate" "no omarchy-plugin-validate here"
  fi
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
  # Issue #45 (finding 121): a peek at this run's box, as the bar widget opens it (`omabox peek
  # --focus`: the window, its `+` line, focus, workspace 9, focus again; seen in a stand-in), with each
  # marker its process can have.
  widget() {   # MARKER: the lines, the tags on the peek's lines
    local t="pid=7${1:+ $1} class=omabox-peek title=omabox peek: t1-b"
    printf '1.000 %s\n' 'openwindow>>a,9,omabox-peek,omabox peek: t1-b' "+ 0xa $t" 'activewindowv2>>a' "~ 0xa $t" \
      'workspacev2>>9,9' 'activewindowv2>>a' "~ 0xa $t" | leak_scan t1- 9
  }
  check_eq "a peek you opened (OMABOX_PEEK_BY=you): no leak, workspace 9 and focus too" \
    "note: not the suite's: a peek at t1-b watched by you, focus on your peek at t1-b, workspace 9 for your peek" "$(widget OMABOX_PEEK_BY=you)"
  local out; out=$(widget OMABOX_SUITE=t1)
  check_match "one this run's commands opened (OMABOX_SUITE): a leak, said so" "^leak: a window opened: a peek window, by this run's commands: 0xa pid=7 OMABOX_SUITE=t1" "$out"
  check_match "...its focus a leak" "leak: focus went to a window of this run's: 0xa pid=7 OMABOX_SUITE=t1" "$out"
  check_match "...and workspace 9 coming up for it" "leak: omabox's workspace 9 came up" "$out"
  out=$(widget)
  check_match "one with no marker (a command other than peek opened it): a leak" "^leak: a window opened: a peek window \"omabox peek: t1-b\" of this run's box, not marked as yours: a command other than \`omabox peek\` opened it" "$out"
  check_match "...naming your old omabox as the other cause" "if you opened it, that is the cause; run again without peeking" "$out"
  check_eq "...its two focus changes and workspace 9 leaks too" "4" "$(grep -c '^leak:' <<<"$out")"
  check_match "...another run's marker is not yours" "^leak: a window opened: a peek window .*not marked as yours" "$(widget OMABOX_SUITE=t12)"
  check_match "a peek whose process the watcher could not find: a leak" "^leak: a window opened: a peek window" \
    "$(scan 'openwindow>>a,9,omabox-peek,omabox peek: t1-b' '+ 0xa ? StopIteration()' '~ 0xb pid=9 OMABOX_PEEK_BY=you class=omabox-peek title=omabox peek: t1-c')"
  check_match "...nor one whose + line is missing" "^leak: a window opened: a peek window" \
    "$(scan 'openwindow>>a,9,omabox-peek,omabox peek: t1-b' 'closewindow>>a' '+ 0xa pid=7 OMABOX_PEEK_BY=you class=omabox-peek title=omabox peek: t1-b')"
  # Issue #56: workspace 9 is judged by the focus it brings, which Hyprland 0.56 sends just before it
  # (a dispatch and SUPER+9 alike, seen in a box): not the suite's, a note; the suite's or none, a leak.
  ws9() { local w=$1; shift; printf '1.000 %s\n' "$@" | leak_scan t1- "$w"; }
  check_eq "workspace 9 up with focus on your own box (the lines from #56): a note" \
    "note: not the suite's: focus on your box box-1, workspace 9 for your box box-1, keyboard hl-virtual-keyboard-fcitx5" \
    "$(ws9 9 'activewindowv2>>5f2b' '~ 0x5f2b pid=9 box=box-1 class=aquamarine' 'workspacev2>>9,9' 'activelayout>>hl-virtual-keyboard-fcitx5,English (US)')"
  check_eq "...on an app of yours: a note" "note: not the suite's: focus firefox, workspace 9 for firefox" \
    "$(ws9 9 'activewindowv2>>b' '~ 0xb pid=9 class=firefox' 'workspacev2>>9,9')"
  check_eq "...on one of yours right after it (a peek's --focus): a note too" "note: not the suite's: focus firefox, workspace 9 for firefox" \
    "$(ws9 9 'workspacev2>>9,9' 'activewindowv2>>b' '~ 0xb pid=9 class=firefox')"
  check_match "...on a box of this run's: a leak" "leak: omabox's workspace 9" \
    "$(ws9 9 'activewindowv2>>a' '~ 0xa pid=5 OMABOX_NAME=t1-main class=foot' 'workspacev2>>9,9')"
  check_match "...on a process the suite started" "leak: omabox's workspace 9" \
    "$(ws9 9 'activewindowv2>>a' '~ 0xa pid=5 OMABOX_SUITE=t1 class=foot' 'workspacev2>>9,9')"
  check_match "...on an interactive box not yours" "leak: omabox's workspace 9" \
    "$(ws9 9 'activewindowv2>>a' '~ 0xa pid=5 class=aquamarine' 'workspacev2>>9,9')"
  check_match "...on a window the watcher could not ask about" "^leak: omabox's workspace 9" \
    "$(ws9 9 'activewindowv2>>a' '~ ? TimeoutExpired()' 'workspacev2>>9,9')"
  check_match "...on nothing" "^leak: omabox's workspace 9" "$(ws9 9 'activewindowv2>>' 'workspacev2>>9,9' 'activelayout>>hl-virtual-keyboard-fcitx5,English (US)')"
  check_match "...on nothing, then another workspace" "^leak: omabox's workspace 9" "$(ws9 9 'workspacev2>>9,9' 'activewindowv2>>' 'workspacev2>>1,1')"
  check_match "...your focus long before it does not count" "^leak: omabox's workspace 9" \
    "$(ws9 9 '~ 0xb pid=9 class=firefox' 'openwindow>>c,1,foot,foot' 'workspacev2>>9,9')"
  # A special workspace (the config's special:NAME) shows with activespecial>>NAME,MONITOR (seen in a box).
  check_match "a special one: coming up with nothing in it, a leak" "^leak: omabox's workspace special:omabox came up" \
    "$(ws9 special:omabox 'activespecial>>special:omabox,DP-1' 'activespecial>>,DP-1')"
  check_eq "...with your own window in it: a note" "note: not the suite's: focus foot, workspace special:omabox for foot" \
    "$(ws9 special:omabox 'activewindowv2>>a' '~ 0xa pid=9 class=foot' 'activespecial>>special:omabox,DP-1' 'activespecial>>,DP-1')"
  check_eq "...another special one: a note" "note: not the suite's: workspace special:magic" "$(ws9 special:omabox 'activespecial>>special:magic,DP-1')"
  check_eq "...workspace 9 is nothing special then" "note: not the suite's: workspace 9" "$(ws9 special:omabox 'workspacev2>>9,9')"
  check_eq "...on your peek at a box of yours: not a leak either" "" \
    "$(printf '1.000 %s\n' 'workspacev2>>9,9' '~ 0xb pid=9 OMABOX_PEEK_BY=you class=omabox-peek title=omabox peek: mine' | leak_scan t1- 9 | grep '^leak:')"
  # The end's focus checks: focus left on a peek of yours (and workspace 9 with it) is not the suite's.
  mkdir -p "$TMP/hs"
  printf '1.000 %s\n' 'openwindow>>a,9,omabox-peek,omabox peek: t1-b' '+ 0xa pid=7 OMABOX_PEEK_BY=you class=omabox-peek title=omabox peek: t1-b' \
    'workspacev2>>9,9' 'activewindowv2>>a' '~ 0xa pid=7 OMABOX_PEEK_BY=you class=omabox-peek title=omabox peek: t1-b' > "$TMP/hs/host-events.log"
  check_match "host focus on your peek at the end: not the suite's" "ok.*by the suite" "$(EVID=$TMP/hs HWS=9 P=t1 host_same f 0xz 0xa window)"
  check_match "...nor workspace 9" "ok.*by the suite" "$(EVID=$TMP/hs HWS=9 P=t1 host_same w 1 9 workspace)"
  sed -i 's/ OMABOX_PEEK_BY=you//' "$TMP/hs/host-events.log"
  check_match "...on an unmarked one: a failure" "FAIL.* f" "$(EVID=$TMP/hs HWS=9 P=t1 CUR=hs host_same f 0xz 0xa window)"
  # A check a peek of yours holds up is skipped (a stand-in process, named and marked as that peek).
  local fake="omabox-peek --box $XDG_RUNTIME_DIR/omabox/$P-held/run/wayland-1"
  env OMABOX_PEEK_BY=you bash -c 'exec -a "$0" sleep 30' "$fake" & local fp=$!
  env OMABOX_PEEK_BY=x bash -c 'exec -a "$0" sleep 30' "${fake/held/other}" & local fo=$!
  until_ok 5 pgrep -f "$fake"; until_ok 5 pgrep -f "${fake/held/other}"
  check "your_peek: a peek marked yours at that box" your_peek "$P-held"
  check_fails "...not one marked otherwise" your_peek "$P-other"
  check_match "a held check that fails while your peek is open: skipped, saying why" "skip.* x \(held by your peek at $P-held" "$(EVID=$TMP/hs held "$P-held" no x y)"
  check_match "...another box's: a failure" "FAIL.* x" "$(EVID=$TMP/hs held "$P-other" no x y 2>&1 | head -1)"
  kill "$fp" "$fo" 2>/dev/null; wait "$fp" "$fo" 2>/dev/null
  check_match "...once it is closed: a failure" "FAIL.* x" "$(EVID=$TMP/hs held "$P-held" no x y 2>&1 | head -1)"
  # Who asked, as `omabox peek` hands it to the peek process.
  check_eq "peek_marker: a command the suite ran" "OMABOX_SUITE=t9" "$(OMABOX_SUITE=t9 MAIN_CMD=peek lib peek_marker)"
  check_eq "...omabox peek run by you (no OMABOX_SUITE)" "OMABOX_PEEK_BY=you" "$(env -u OMABOX_SUITE MAIN_CMD=peek bash -c 'source "$1"; peek_marker' _ "$TMP/lib/bin/omabox")"
  check_eq "...any other command: none (the suite fails on its peek)" "" "$(env -u OMABOX_SUITE MAIN_CMD=up bash -c 'source "$1"; peek_marker' _ "$TMP/lib/bin/omabox")"
  check_eq "...an OMABOX_SUITE that is not a tame word: none" "" "$(OMABOX_SUITE="t1' x" MAIN_CMD=peek lib peek_marker)"
  check_match "a virtual keyboard's keys" "^leak: keys from a virtual keyboard \(hl-virtual-keyboard-unknown\)" "$(scan 'activelayout>>hl-virtual-keyboard-unknown,English (US)')"
  check_eq "...not one named as yours (OMABOX_TEST_HOST_KEYBOARDS)" "" "$(OMABOX_TEST_HOST_KEYBOARDS='^hl-virtual-keyboard-unknown$' leaks 'activelayout>>hl-virtual-keyboard-unknown,English (US)')"
  check_eq "...nor Omarchy's input method (fcitx5, on every focus change of yours)" "" "$(leaks 'activelayout>>hl-virtual-keyboard-fcitx5,English (US)')"
  check_eq "your own windows, focus and workspaces: one note, no leak" "note: not the suite's: window firefox, focus firefox, workspace 3" \
    "$(scan 'openwindow>>b,3,firefox,a, title' 'activewindowv2>>b' '~ 0xb pid=9 class=firefox' 'workspacev2>>3,3' '~ 0xb pid=9 class=firefox')"
  check_match "omabox's workspace coming up (from #13's watch)" "^leak: omabox's workspace 9 came up" "$(printf '1.000 %s\n' 'workspacev2>>9,9' | leak_scan t1- 9)"
  check_eq "...a note when the run does not watch it (you were on it)" "note: not the suite's: workspace 9" "$(scan 'workspacev2>>9,9')"
  check_eq "a log slice starts after its marker and ends before the next" "b" \
    "$(printf '1 a\n2 == x\n3 b\n4 == y\n5 c\n' > "$TMP/slice"; slice "$TMP/slice" x y | cut -d' ' -f2)"
}

# par_fake DIR scan|run: the runner's own functions against DIR, with counts of its own (locals: the
# run's are untouched): host_scan of t_b, or par_test/par_done on made-up tests. Prints what failed.
par_fake() {
  local EVID=$1 TMP=$1 HWS=9 pass=0 fail=0 failed=() skips=() LEAK_PROVEN="" t
  if [ "$2" = scan ]; then host_scan t_b /t_b >/dev/null 2>&1; printf '%s\n' "${failed[@]}"; return; fi
  # shellcheck disable=SC2329 # run by par_test
  t_fake_ok() { check_eq "made up" 1 1; LEAK_PROVEN=1; }
  # shellcheck disable=SC2329
  t_fake_dies() { check_eq "made up too" 1 1; exit 3; }
  # shellcheck disable=SC2329
  t_fake_none() { :; }
  mkdir -p "$1/par"
  for t in t_fake_ok t_fake_dies t_fake_none; do
    printf '%s == %s\n' 20 "$t" >> "$1/host-events.log"; (par_test "$t") > "$1/par/$t.out" 2>&1
    printf '%s == /%s\n' 21 "$t" >> "$1/host-events.log"; par_done "$t" >/dev/null 2>&1
  done
  echo "pass=$pass fail=$fail proven=$LEAK_PROVEN"; printf '%s\n' "${failed[@]}"
}
# The parallel runner (issue #60), on a made-up log and made-up tests, in a subshell of its own (its
# counts and EVID are not the run's): windows, the tests beside, the gaps, and what a test hands back.
t_unit_parallel() {
  local d=$TMP/par-unit out; mkdir -p "$d"
  printf '%s\n' '1 == watching' '2 == parallel' '3 == t_a' '4 == t_b' '5 activelayout>>hl-virtual-keyboard-unknown,x' \
    '5.5 openwindow>>c,1,foot,foot' '6 == /t_a' '7 == t_c' '8 == /t_b' '9 == /t_c' '9.5 activelayout>>hl-virtual-keyboard-unknown,x' \
    '10 == t_d' '11 == /t_d' '12 == /parallel' > "$d/host-events.log"
  ev() { EVID=$d "$@"; }
  check_eq "beside: the tests running during a test's time" "t_b" "$(ev beside t_a)"
  check_eq "...every one it met" "t_a t_c" "$(ev beside t_b | tr ' ' '\n' | sort | paste -sd' ')"
  check_eq "...none" "" "$(ev beside t_d)"
  check_eq "a test's window, markers of the others left out" "5 activelayout>>hl-virtual-keyboard-unknown,x|5.5 openwindow>>c,1,foot,foot" \
    "$(slice "$d/host-events.log" t_a /t_a | paste -sd'|')"
  check_eq "gap_events: only what no test's time holds" "9.5 activelayout>>hl-virtual-keyboard-unknown,x" "$(ev gap_events)"
  out=$(par_fake "$d" scan)
  check_match "a leak in a test's window fails it, naming the tests beside it" "nothing of it, or of the tests beside it, reached.*" "$out"
  # par_test (in its subshell, as the runner starts it) and par_done on made-up tests: one that passes
  # and proves the leak detector, one that dies midway, one that checks nothing.
  out=$(par_fake "$d" run)
  check_match "par_done adds a test's counts and LEAK_PROVEN (one that stopped midway's too)" "^pass=2 fail=2 proven=1" "$out"
  check_match "...a test that stops midway fails, said so" "t_fake_dies: the test ran to its end" "$out"
  check_match "...one that ran no check too" "t_fake_none: the test ran checks" "$out"
}

# The leak detector, proven (finding 80): the watcher the host gets, on a box standing in for the
# host. Quiet, it reports nothing; then a box's window taking focus, a workspace switch and back,
# omabox's workspace coming up, and a key from `omabox keys` are leaked into the stand-in on purpose,
# and each must be reported. A clean
# host log means something only then: when this test fails, so does the host's verdict.
t_leak_control() {
  local S=$P-ctl f0=$fail log
  ob up "$S" --no-shell --net isolated >/dev/null 2>&1 || { no "up the stand-in" "failed"; return; }
  log=$(ob path -b "$S")/run/events.log
  ob run -b "$S" -d -- sh -c 'exec python3 -c "$1" "$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock" >> "$XDG_RUNTIME_DIR/events.log" 2>&1' sh "$WATCHER" >/dev/null 2>&1
  check "the watcher listens to the stand-in's events" until_ok 10 grep -q ' == watching$' "$log"
  mark() { ob run -b "$S" -- sh -c 'printf "%s == %s\n" "$(date +%s.%3N)" "$1" >> "$XDG_RUNTIME_DIR/events.log"' sh "$1"; }
  # shellcheck disable=SC2329 # called through until_ok
  reported() { slice "$log" leaks | leak_scan "$P-" 9 | grep -- "$1" >/dev/null; }  # (not -q: its early exit would fail the pipe)
  mark quiet
  ob shot -b "$S" -o "$TMP/ctl.png" >/dev/null 2>&1; ob hyprctl -b "$S" -j clients >/dev/null
  mark leaks
  ob run -b "$S" -d -- foot sleep 60 >/dev/null 2>&1
  until_ok 10 bash -c "'$CLI' hyprctl -b '$S' -j activewindow | jq -e '.class == \"foot\"'"
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '3' })" >/dev/null
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '9' })" >/dev/null
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  ob keys -b "$S" a >/dev/null
  check "a box's window taking focus is reported, with the box" until_ok 5 reported "^leak: focus went to a window of this run's: .*OMABOX_NAME=$S class=foot"
  check "a key from omabox keys is reported" until_ok 5 reported '^leak: keys from a virtual keyboard'
  check "a workspace switch and back is noted" until_ok 5 reported '^note: .*workspace 3, workspace 1'
  check "omabox's workspace coming up is reported" until_ok 5 reported "^leak: omabox's workspace 9 came up"
  check_eq "...and while quiet, nothing" "" "$(slice "$log" quiet leaks | leak_scan "$P-" 9)"
  # Peeks at a box of this run's (finding 121), one in the stand-in: `omabox peek --focus` run there as
  # the bar widget runs it (the box's environment has no OMABOX_SUITE) is yours, a note; the same with
  # the suite's OMABOX_SUITE is a leak; and a peek the stand-in's Hyprland starts with no marker (what
  # a command other than `omabox peek` opening one would look like) is a leak.
  local in=("$CLI" run -b "$S" --) C=$P-cin wd
  ob run -b "$S" -- pkill -x foot   # (focus coming back to it would be a leak of its own)
  "${in[@]}" "$CLI" up "$C" --no-shell --idle 0 >/dev/null 2>&1 || no "up a box in the stand-in" "failed"
  # shellcheck disable=SC2329 # called through until_ok
  seen() { slice "$log" "$1" "$2" | grep -- "$3" >/dev/null; }
  # shellcheck disable=SC2329
  scanned() { slice "$log" "$1" "$2" | leak_scan "$P-" "${4:-9}" | grep -- "$3" >/dev/null; }   # FROM TO RE [WS]
  unpeek() { ob run -b "$S" -- pkill -x omabox-peek; ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null; }
  mark yours
  "${in[@]}" "$CLI" peek "$C" --focus >/dev/null 2>&1
  until_ok 5 seen yours "" "workspacev2>>9,9"; until_ok 5 seen yours "" "~ .*class=omabox-peek"
  unpeek; mark suites
  "${in[@]}" env OMABOX_SUITE="$P" "$CLI" peek "$C" --focus >/dev/null 2>&1
  until_ok 5 seen suites "" "workspacev2>>9,9"; until_ok 5 seen suites "" "~ .*class=omabox-peek"
  unpeek; mark raw
  wd=$("${in[@]}" "$CLI" run -b "$C" -- sh -c 'echo $WAYLAND_DISPLAY')
  ob hyprctl -b "$S" eval "hl.exec_cmd('$ROOT/tools/peek/omabox-peek --box $("${in[@]}" "$CLI" path "$C")/run/$wd --title \"omabox peek: $C\"', { workspace = '9 silent', no_initial_focus = true })" >/dev/null
  until_ok 5 seen raw "" "^[0-9.]* + "
  unpeek; mark end
  check_eq "a peek you opened (omabox peek --focus): no leak" "" "$(slice "$log" yours suites | leak_scan "$P-" 9 | grep '^leak:')"
  check "...noted as watched by you, with its workspace and focus" scanned yours suites "^note: .*a peek at $C watched by you, .*workspace 9 for your peek"
  check "one opened by the suite's command is reported" scanned suites raw "^leak: a window opened: a peek window, by this run's commands: .*OMABOX_SUITE=$P "
  check "...its workspace too" scanned suites raw "^leak: omabox's workspace 9 came up"
  check "one started with no marker is reported" scanned raw end "^leak: a window opened: a peek window \"omabox peek: $C\" of this run's box, not marked as yours"
  # Issue #56: workspace 9 brought up by you (focus on a window that is not the suite's: here one with
  # no omabox marks, as your own app's) is a note; a special workspace of omabox's (special:NAME in
  # the config) coming up empty is a leak.
  ob run -b "$S" -d -- env -u OMABOX_NAME foot sleep 60 >/dev/null 2>&1
  until_ok 10 bash -c "'$CLI' hyprctl -b '$S' -j activewindow | jq -e '.class == \"foot\"'"
  ob hyprctl -b "$S" dispatch "hl.dsp.window.move({ workspace = '9', window = 'class:foot' })" >/dev/null
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  mark mine
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '9' })" >/dev/null
  until_ok 5 seen mine "" "workspacev2>>9,9"
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  mark special
  ob hyprctl -b "$S" dispatch 'hl.dsp.workspace.toggle_special("omabox")' >/dev/null
  ob hyprctl -b "$S" dispatch 'hl.dsp.workspace.toggle_special("omabox")' >/dev/null
  until_ok 5 seen special "" "activespecial>>,"
  mark end2
  check_eq "omabox's workspace brought up by you (focus on your app there): no leak" "" "$(slice "$log" mine special | leak_scan "$P-" 9 | grep '^leak:')"
  check "...noted, with the focus it brought" scanned mine special "^note: .*workspace 9 for foot"
  check "a special workspace of omabox's coming up empty is reported" scanned special end2 "^leak: omabox's workspace special:omabox came up" special:omabox
  ob down "$S" >/dev/null 2>&1
  [ "$fail" != "$f0" ] || LEAK_PROVEN=1
}

# The broker for ai-jail (finding 99): a jail's policy read from its bwrap's command line, and what a
# jailed caller is refused, without a jail.
t_unit_jail_policy() {
  local d=$TMP/jp; mkdir -p "$d/proj/in" "$d/other" "$d/under/x"
  pol() { printf '%s\0' /usr/bin/bwrap "$@" | lib jail_policy; }
  local p
  p=$(pol --tmpfs /tmp --bind "$d/proj" "$d/proj" --unshare-net --setenv A b -- sh)
  check_eq "no network: --unshare-net" false "$(jq -r .net <<<"$p")"
  check_eq "a folder at its own path is one of the jail's" "$d/proj false" "$(jq -r '.roots[] | "\(.path) \(.masked)"' <<<"$p")"
  check_eq "network without --unshare-net" true "$(pol --bind "$d/proj" "$d/proj" -- sh | jq -r .net)"
  check_eq "--unshare-all is no network" false "$(pol --unshare-all -- sh | jq -r .net)"
  check_eq "...unless --share-net" true "$(pol --unshare-all --share-net -- sh | jq -r .net)"
  check_eq "a mount inside a folder masks it" true "$(pol --bind "$d/proj" "$d/proj" --ro-bind /etc/hostname "$d/proj/in/h" -- sh | jq -r '.roots[0].masked')"
  check_eq "...one before it does not (the folder covers it)" false "$(pol --tmpfs "$d/proj/in" --bind "$d/proj" "$d/proj" -- sh | jq -r '.roots[0].masked')"
  check_eq "a later mount at or above a folder covers it" 0 "$(pol --bind "$d/proj" "$d/proj" --tmpfs "$d" -- sh | jq '.roots | length')"
  check_eq "a folder mounted elsewhere is not one" 0 "$(pol --bind "$d/proj" /work -- sh | jq '.roots | length')"
  check_match "an unknown option fails the read" "unknown bwrap option --bogus" "$(pol --bogus -- sh 2>&1; echo " rc=$?")"
  # ai-jail 2.6.2 (its #147): the options in a memfd of ai-jail's, named on the command line by --args.
  # A process holds one here as ai-jail does; the policy reads it through that process's fd dir.
  local mf=$TMP/jp-memfd pid fd
  python3 -c 'import os, sys, time
def hold(name, args):
    fd = os.memfd_create(name, 0); os.write(fd, b"".join(a.encode() + b"\0" for a in args)); return fd
good = hold("ai-jail-bwrap-args", ["--tmpfs", "/tmp", "--bind", sys.argv[1], sys.argv[1], "--unshare-net"])
other = hold("something-else", ["--share-net"])
print(good, other, flush=True); time.sleep(60)' "$d/proj" > "$mf" &
  pid=$!
  until_ok 5 test -s "$mf"
  read -r fd ofd < "$mf"
  polfd() { printf '%s\0' /usr/bin/bwrap "$@" | lib jail_policy "/proc/$pid/fd"; }
  p=$(polfd --args "$fd" -- sh)
  check_eq "--args: the options read from ai-jail's memfd (no network)" false "$(jq -r .net <<<"$p")"
  check_eq "...and its folder" "$d/proj false" "$(jq -r '.roots[] | "\(.path) \(.masked)"' <<<"$p")"
  check_eq "...options after it still count" true "$(polfd --args "$fd" --share-net -- sh | jq -r .net)"
  check_match "--args: a fd that is not ai-jail's options memfd fails" "not ai-jail's options memfd" "$(polfd --args "$ofd" -- sh 2>&1; echo " rc=$?")"
  check_match "--args: one that isn't open fails" "not ai-jail's options memfd" "$(polfd --args 999 -- sh 2>&1)"
  check_match "--args: only once" "more than once" "$(polfd --args "$fd" --args "$fd" -- sh 2>&1)"
  check_match "--args: not without ai-jail's fds" "no ai-jail" "$(pol --args "$fd" -- sh 2>&1; echo " rc=$?")"
  kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  check_match "...rc 1" "rc=1" "$(pol --bogus -- sh 2>&1; echo " rc=$?")"
  check_match "an option cut short fails" "cut short" "$(printf '%s\0' bwrap --bind a | lib jail_policy)"
  check_match "no -- fails" "no --" "$(pol --tmpfs /x)"
  # jail_root/jail_sees/repo_top with a policy like that one.
  git -C "$d/proj" init -q
  local J; J=$(jq -nc --arg p "$d/proj" --arg u "$d/under" --arg c "$d/proj/in" \
    '{id: "1 2", net: false, cwd: $c, roots: [{path: $p, masked: false}, {path: $u, masked: true}]}')
  check_eq "jail_root: the folder itself" "$d/proj" "$(OMABOX_JAIL=$J lib jail_root "$d/proj")"
  check_fails "jail_root: not a folder below it (a link could replace it)" env OMABOX_JAIL="$J" bash -c 'source "$1"; jail_root "$2"' _ "$TMP/lib/bin/omabox" "$d/proj/in"
  check_fails "jail_root: not a masked one" env OMABOX_JAIL="$J" bash -c 'source "$1"; jail_root "$2"' _ "$TMP/lib/bin/omabox" "$d/under"
  check_fails "jail_root: not one the jail lacks" env OMABOX_JAIL="$J" bash -c 'source "$1"; jail_root "$2"' _ "$TMP/lib/bin/omabox" "$d/other"
  check_eq "jail_sees: inside a folder" "$d/proj/in" "$(OMABOX_JAIL=$J lib jail_sees "$d/proj/in")"
  ln -sfn "$d/other" "$d/proj/link"
  check_fails "jail_sees: not through a link out of it" env OMABOX_JAIL="$J" bash -c 'source "$1"; jail_sees "$2"' _ "$TMP/lib/bin/omabox" "$d/proj/link"
  # A repo's config is the jail's to write: repo_top must not run git on it.
  git -C "$d/proj" config core.fsmonitor "touch $d/fsmonitor-ran"
  check_eq "repo_top: the jail's folder with a .git, without git" "$d/proj" "$(OMABOX_JAIL=$J lib repo_top)"
  check_fails "...its core.fsmonitor never ran" test -e "$d/fsmonitor-ran"
  check_eq "the default box is the one the caller's omabox sent" mine "$(OMABOX_JAIL=$J OMABOX_RELAY_DEFAULT=mine lib default_name)"
  check_eq "--pass reads the caller's value as sent" s3 "$(OMABOX_JAIL=$J OMABOX_RELAY_PASS_X=s3 X=host lib caller_env X)"
  local c
  for c in host peek keys-to-box guard broker _reap "config workspace 3" "save s" saves "saves rm s"; do
    # shellcheck disable=SC2086 # the command and its arguments
    check_match "refused to a jailed agent: $c" "not for an agent inside ai-jail|does not change" "$(lib broker_check $c 2>&1)"
  done
  # A save that does not exist: were --from let through, up would stop at "no save", no box started.
  check_match "up --from refused to a jailed agent (finding 100)" "saves are the user's" "$(OMABOX_JAIL=$J "$CLI" up "$P-jf" --from nosuch 2>&1)"
  check "allowed: shot, keys, up, down, config --json" bash -c 'source "$1"; for c in shot keys up down; do broker_check $c; done; broker_check config --json' _ "$TMP/lib/bin/omabox"
}

# omabox broker on/off writes a socket and a service for the user manager (systemctl stubbed: the real
# one is never touched) and prints the lines for ~/.ai-jail; it never edits that file.
t_unit_broker_units() {
  local h=$TMP/bh stub=$TMP/bstub out; mkdir -p "$h" "$stub"
  printf '#!/bin/sh\necho "$*" >> %q\n' "$TMP/systemctl.log" > "$stub/systemctl"; chmod +x "$stub/systemctl"
  echo 'ro_maps = ["/x"]' > "$h/.ai-jail"
  out=$(HOME=$h PATH=$stub:$PATH "$CLI" broker on 2>&1)
  local u=$h/.config/systemd/user
  check_match "on: the socket" "ListenStream=%t/omabox/.broker/sock" "$(cat "$u/omabox-broker.socket" 2>&1)"
  check_match "...private to the user" "SocketMode=0600" "$(cat "$u/omabox-broker.socket" 2>&1)"
  check_match "on: the service runs the relay on this checkout's omabox" "ExecStart=$ROOT/tools/relay/omabox-relay listen -- $ROOT/bin/omabox" "$(cat "$u/omabox-broker.service" 2>&1)"
  check_match "...and boxes outlive it" "KillMode=process" "$(cat "$u/omabox-broker.service" 2>&1)"
  check_match "on: the socket enabled" "enable --now omabox-broker.socket" "$(cat "$TMP/systemctl.log")"
  check_match "on: prints the ~/.ai-jail lines" "ro_maps = \[\"$ROOT/bin/omabox:.*/omabox-relay\", \"$ROOT/skill\", \"$XDG_RUNTIME_DIR/omabox/.broker\"\]" "$out"
  check_eq "...and leaves ~/.ai-jail alone" 'ro_maps = ["/x"]' "$(cat "$h/.ai-jail")"
  command -v systemd-analyze >/dev/null && check "the units are valid" systemd-analyze --user verify "$u/omabox-broker.socket" "$u/omabox-broker.service"
  HOME=$h PATH=$stub:$PATH "$CLI" broker off >/dev/null 2>&1
  check_fails "off: the units gone" test -e "$u/omabox-broker.service"
}

# The relay itself, host to host: what reaches the command, and what does not.
t_unit_relay() {
  local R=$ROOT/tools/relay/omabox-relay d=$TMP/relay
  [ -x "$R" ] || { skip "the relay (tools/relay)" "not built: run install.sh"; return; }
  mkdir -p "$d"
  "$R" listen --socket "$d/sock" -- /usr/bin/bash -c '
    printf "args=%s|" "$*"; env | grep -E "^(OMABOX_|PATH=|BASH_ENV)" | sort | tr "\n" "|"
    [ -n "${OMABOX_BROKER_PIDFD:-}" ] && grep -q "^Pid:[[:space:]]*$OMABOX_BROKER_PEER$" "/proc/self/fdinfo/$OMABOX_BROKER_PIDFD" && printf "pidfd=ok|"
    [ ! -e /dev/fd/3 ] || [ "$OMABOX_BROKER_PIDFD" = 3 ] || { printf "fd3=%s|" "$(cat /dev/fd/3)"; }
    [ "$1" != sleep ] || { echo $$ > '"$d/cmd"'; exec sleep 30; }
    exit 7' bash >/dev/null 2>&1 & SERVERS+=($!)
  until_ok 5 test -S "$d/sock" >/dev/null
  local out rc=0
  out=$(cd "$d" && "$R" call "$d/sock" --env FOO=bar --env PATH=/evil -- one "two words" 2>&1) || rc=$?
  check_eq "the command's exit status comes back" 7 "$rc"
  check_match "arguments arrive as sent" "args=one two words\|" "$out"
  check_match "an --env entry arrives renamed, PATH included" "OMABOX_RELAY_FOO=bar\|OMABOX_RELAY_PATH=/evil\|" "$out"
  check_match "the command's own PATH is the broker's" "\|PATH=/usr/local/bin:/usr/bin\|" "$out"
  check_match "the caller and its cwd are named" "OMABOX_BROKER_CWD=$d\|OMABOX_BROKER_PEER=[0-9]+\|" "$out"
  check_match "a pidfd for the caller" "pidfd=ok" "$out"
  # With a socket for stdin, bash would source ~/.bashrc (SHLVL below 2): not in the broker.
  out=$(cd "$d" && python3 -c 'import socket, subprocess, sys
a, b = socket.socketpair()
sys.stdout.write(subprocess.run(sys.argv[1:], stdin=a, capture_output=True, text=True).stdout)' "$R" call "$d/sock" -- x)
  check_match "...its PATH stays fixed with a socket for stdin (no ~/.bashrc)" "\|PATH=/usr/local/bin:/usr/bin\|" "$out"
  check_match "a variable name that is not one is refused" "bad --env" "$("$R" call "$d/sock" --env 'BASH_FUNC_x%%=1' -- x 2>&1)"
  echo passed > "$d/f"
  check_match "an --fd file reaches the command as fd 3" "fd3=passed" "$("$R" call "$d/sock" --fd 5 -- x 5<"$d/f" 2>&1)"
  timeout 1 "$R" call "$d/sock" -- sleep >/dev/null 2>&1
  check "the command goes when its caller does" until_ok 5 bash -c "! kill -0 \$(cat '$d/cmd') 2>/dev/null"
}

# A jailed agent's omabox, through the broker, in a real ai-jail (finding 99): it drives a box of its
# own, which has no more than the jail (no network, only the jail's project), and nothing else.
t_jail() {
  command -v ai-jail >/dev/null || { skip "an agent inside ai-jail drives its box through the broker" "ai-jail is not installed"; return; }
  # ai-jail runs only a root-owned bwrap, and --installed's user namespace shows root's files as
  # nobody's (the broker's units and lines from /usr/lib/omabox: t_unit_broker_units).
  [ "${OMABOX_TEST_INSTALLED:-0}" = 0 ] ||
    { skip "an agent inside ai-jail drives its box through the broker" "--installed: ai-jail trusts no bwrap in a user namespace"; return; }
  local R=$ROOT/tools/relay/omabox-relay br=$TMP/br repo out
  [ -x "$R" ] || { skip "an agent inside ai-jail drives its box through the broker" "tools/relay not built"; return; }
  mkdir -p "$br" "$TMP/lacks"; repo=$(tmp_repo jail); echo hi > "$repo/README"
  # The broker as systemd starts it: a few variables, the socket handed over.
  env -i HOME="$HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" LANG="${LANG:-C.UTF-8}" USER="$USER" \
    systemd-socket-activate -E HOME -E XDG_RUNTIME_DIR -E LANG -E USER -l "$br/sock" "$R" listen -- "$CLI" >"$br/log" 2>&1 & SERVERS+=($!)
  until_ok 5 test -S "$br/sock" >/dev/null
  ob up "$P-host" >/dev/null 2>&1   # the user's box: not the jail's to drive
  cat > "$repo/t.sh" <<EOF
O=$CLI
r() { echo "== \$1"; shift; "\$@" 2>&1; echo "rc=\$?"; }
r up \$O up
r ls \$O ls
r pwd \$O run -- sh -c 'pwd; cat README'
r shot \$O shot
r shotfile sh -c 'f=\$(ls /tmp/omabox-*.png); head -c 8 "\$f" | od -An -c | tr -d " \n"'
r shot-o \$O shot -o ./out.png
r travel-mod \$O click --steps 3 --mod ctrl 960 600
r pointer-mod \$O pointer --steps 2 --mod shift -- move 900 500 move 960 540 --steps 3
r relay-o $R call $br/sock -- shot -b $P-jail -o $br/pwned.png
r host \$O host -- touch $br/pwned
r ro-bind \$O up $P-other --ro-bind $TMP/lacks
r net \$O up $P-other --net connected
r other \$O shot -b $P-host
r pre-b \$O -b $P-jail windows
r pre-b-other \$O -b $P-host windows
r net-in-box \$O run -- sh -c 'curl -s --max-time 3 -o /dev/null https://archlinux.org || echo no-internet'
r run-d \$O run -d -q --print-log -- sleep 604
r replace \$O run -d -q --replace -- sleep 604
r sleeps \$O run -- pgrep -cxf 'sleep 604'
r down \$O down
EOF
  out=$(cd "$repo" && timeout 180 ai-jail --no-save-config --map "$ROOT" --map "$br" --env OMABOX_BROKER_SOCK="$br/sock" bash t.sh </dev/null 2>&1)
  printf '%s\n' "$out" > "$TMP/jail.out"
  sect() { awk -v s="== $1" '$0 == s {on = 1; next} /^== / {on = 0} on' "$TMP/jail.out" | tr '\n' ' '; }
  check_match "up from the jail" "box '$P-jail' up .*rc=0" "$(sect up)"
  check_match "its box has no network (the jail has none)" "$P-jail .* isolated " "$(sect ls)"
  check_match "...and only its own boxes are listed" "^NAME [^$]*$P-jail[^$]*rc=0 $" "$(sect ls | sed "s/$P-host//")"
  check_eq "run starts in the jail's project, mounted" "$repo hi rc=0 " "$(sect pwd)"
  check_match "a shot is written into the jail" "211PNG" "$(sect shotfile)"
  check_match "shot -o into the jail's project" "out.png rc=0" "$(sect shot-o)"
  check "...there, on the host too" test -s "$repo/out.png"
  check_match "click --steps --mod through the broker (#38, #25)" "^rc=0 $" "$(sect travel-mod)"
  check_match "...pointer --steps --mod, and move's own --steps" "^rc=0 $" "$(sect pointer-mod)"
  check_match "a path of the broker's never written" "never at a path here rc=1" "$(sect relay-o)"
  check_match "host refused" "not for an agent inside ai-jail rc=1" "$(sect host)"
  check_match "a folder the jail lacks does not go in" "not a folder this jail was given whole.* rc=1" "$(sect ro-bind)"
  check_match "a network the jail lacks is refused" "has no network.* rc=1" "$(sect net)"
  check_match "the user's own box is not the jail's" "not this jail's.* rc=1" "$(sect other)"
  check_match "-b NAME before the command, through the broker (#36)" "rc=0 $" "$(sect pre-b)"
  check_match "...the user's box still not the jail's" "not this jail's.* rc=1" "$(sect pre-b-other)"
  check_match "no internet in its box" "no-internet rc=0" "$(sect net-in-box)"
  check_match "run -d -q --print-log through the broker: only the log path" "^/[^ ]*/run-[0-9]+\.log rc=0 $" "$(sect run-d)"
  check_eq "run -d --replace through the broker: one job left" "1 rc=0 " "$(sect sleeps)"
  check_match "down" "box '$P-jail' down rc=0" "$(sect down)"
  check_fails "nothing written where the jail could not" bash -c "ls '$br'/pwned*"
  check "the user's box still up" ob shot -b "$P-host" -o "$TMP/host.png"
  ob down "$P-host" >/dev/null 2>&1
}

# Every test runs: a t_* function left out of UNIT and BOX would never run, and nobody would notice.
t_unit_registry() {
  check_eq "every t_* function is in UNIT or BOX, once" "$(declare -F | awk '$3 ~ /^t_/ {print $3}' | sort)" \
    "$(printf '%s\n' "${UNIT[@]}" "${BOX[@]}" | sort)"
  check_eq "UNIT has only t_unit_*, BOX none" "" "$(printf '%s\n' "${UNIT[@]}" | grep -v '^t_unit_'; printf '%s\n' "${BOX[@]}" | grep '^t_unit_')"
}

t_unit_cli() {
  check_eq "inside a box OMABOX=1 is not a box name" "$(cd "$ROOT" && lib default_name)" "$(cd "$ROOT" && OMABOX=1 OMABOX_NAME=x lib default_name)"
  check_eq "on the host OMABOX names the box" mine "$(OMABOX=mine lib default_name)"
  check_eq "under run, OMABOX=NAME names the box (#19)" inner "$(OMABOX=inner OMABOX_NAME=outer lib default_name)"
  check_match "run: unknown option named, no box started" "unknown option --interactive" "$(ob run --interactive -- true 2>&1)"
  check_match "run: a throwaway's up error is shown" "--net is connected" "$(cd "$(tmp_repo ne)" && env -u OMABOX "$CLI" run --net bogus -- true 2>&1)"
  check_match "run --help is the usage" "omabox up" "$(ob run --help 2>&1)"
  # One command's help (`help CMD`, `CMD --help`): its lines and paragraphs, not the whole text.
  local full cmds c bad=""
  full=$(ob help)
  cmds=$(awk 'NR > 2 && $0 == "" {exit} NR > 2 && $1 == "omabox" {print $2}' <<<"$full" | sort -u)
  for c in $cmds; do [[ $(ob help "$c" | head -1) == "  omabox $c "* ]] || bad+=" $c"; done
  check_eq "help CMD starts with CMD's line, for each command" "" "$bad"
  bad=""
  for c in $(lib usage_text | awk '/^@/ {for (i = 2; i <= NF; i++) print $i}' | sort -u); do
    grep -qx -- "$c" <<<"$cmds" || bad+=" $c"
  done
  check_eq "...every paragraph's @ names a command" "" "$bad"
  check_match "...the first paragraph has one (so every line has one)" "^@ " "$(lib usage_text | awk 'NR > 2 && p {print; exit} NR > 2 && $0 == "" {p = 1}')"
  check_fails "...the @ lines never in the full help" grep -q '^@' <<<"$full"
  check_eq "shot --help is help shot" "$(ob help shot)" "$(ob shot --help)"
  check_eq "...after -b NAME too" "$(ob help shot)" "$(ob -b "$P-x" shot --help)"
  check_eq "...an alias's (screenshot)" "$(ob help shot)" "$(ob screenshot -h)"
  check_match "...a fraction of the full help" "^yes$" "$( (( $(ob help shot | wc -c) * 5 < ${#full} )) && echo yes)"
  check_match "...run's has up's options" "omabox up .*--net isolated" "$(ob help run | tr '\n' ' ')"
  check_eq "...an unknown one: one line, exit 2" "1 2" "$(ob help shoot 2>&1 | wc -l) $(ob help shoot >/dev/null 2>&1; echo $?)"
  check_match "...hyprctl's --help is hyprctl's" "no box '$P-x' is up" "$(ob -b "$P-x" hyprctl --help 2>&1)"
  check_eq "path NAME names the box" "$XDG_RUNTIME_DIR/omabox/$P-x" "$(ob path "$P-x")"
  check_fails "path: two names refused" ob path a b
  check_match "unknown command named" "unknown command: shoot" "$(ob shoot 2>&1)"
  check_eq "...in one line, not the help (#36)" "1 2" "$(ob shoot 2>&1 | wc -l) $(ob shoot >/dev/null 2>&1; echo $?)"
  # -b NAME before the command (#36): the same as after it.
  check_eq "-b NAME before the command" "$XDG_RUNTIME_DIR/omabox/$P-x" "$(ob -b "$P-x" path)"
  check_eq "...--box NAME too" "$XDG_RUNTIME_DIR/omabox/$P-x" "$(ob --box "$P-x" path)"
  check_match "...reaches the command's own -b (windows)" "no box '$P-x' is up" "$(ob -b "$P-x" windows 2>&1)"
  check_match "...hyprctl, which takes -b only first" "no box '$P-x' is up" "$(ob -b "$P-x" hyprctl clients 2>&1)"
  local c out=""
  for c in lua log events clip; do out+="$c: $(ob -b "$P-x" "$c" </dev/null 2>&1 | head -1)"$'\n'; done
  check_fails "...lua, log, events and clip too (none unknown)" grep -qE 'unknown command|takes no -b' <<<"$out"
  check_match "...up NAME as well is two names" "up: one box name, got $P-x and $P-y" "$(ob -b "$P-x" up "$P-y" 2>&1)"
  check_eq "...a command that takes no -b: one line" "omabox: ls takes no -b: it is not about one box" "$(ob -b "$P-x" ls 2>&1)"
  check_eq "...exit 2" 2 "$(ob -b "$P-x" ls >/dev/null 2>&1; echo $?)"
  check_eq "...an unknown one: one line" 1 "$(ob -b "$P-x" shoot 2>&1 | wc -l)"
  check_match "...no command: said" "goes with a command" "$(ob -b "$P-x" 2>&1)"
  check_match "...help still helps" "omabox up" "$(ob -b "$P-x" help 2>&1)"
  check_match "...no value: said" "-b needs a value" "$(ob -b 2>&1)"
  # In an empty runtime dir: if the refusal broke, --all would take down every box on the machine.
  mkdir -p "$TMP/rt"
  check_match "down --all with a name refused" "--all or names, not both" "$(XDG_RUNTIME_DIR=$TMP/rt "$CLI" down "$P-x" --all 2>&1)"
  check_fails "peek --fps junk refused" ob peek -b "$P-x" --fps "10'"
  # ls while a box goes down (seen with the suite's boxes in parallel, issue #60): its box.json gone,
  # the rest of the list still shown. Three boxes in a runtime dir of its own; the middle one half gone.
  local lb; for lb in a b c; do mkdir -p "$TMP/rt-ls/omabox/$P-ls$lb"; echo '{}' > "$TMP/rt-ls/omabox/$P-ls$lb/info.json"; done
  for lb in a c; do echo '{"mode":"headless","size":"1x1@60","net":"none","idle":0}' > "$TMP/rt-ls/omabox/$P-ls$lb/box.json"; done
  check_eq "ls: a box going down meanwhile is left out, not the end of the list" "$P-lsa $P-lsc 0" \
    "$(XDG_RUNTIME_DIR=$TMP/rt-ls "$CLI" ls 2>&1 | awk 'NR > 1 {printf "%s ", $1}'; echo "${PIPESTATUS[0]}")"
  # run -d's -q, --print-log and --replace (issues #42, #29; findings 109, 110)
  check_match "run -q without -d refused" "go with -d" "$(ob run -b "$P-x" -q -- true 2>&1)"
  check_match "run --replace without -d refused" "go with -d" "$(ob run -b "$P-x" --replace -- true 2>&1)"
  check_match "run --quiet without a duration: -q named" "-q is the flag" "$(ob run -b "$P-x" -d --quiet -- true 2>&1)"
  # job_procs: a job's processes are its session's; its pid leading a session that started at another
  # time (or at all, when the leader had exited before the record) is a reused pid, not the job.
  local ps=$'100 7 7 500\n101 8 7 510\n102 9 9 520'
  check_eq "job_procs: the session's processes" $'100 7\n101 8' "$(lib job_procs 7 500 <<<"$ps")"
  check_eq "job_procs: the leader restarted under the pid: none" "" "$(lib job_procs 7 499 <<<"$ps")"
  check_eq "job_procs: the leader gone, its session left" "101 8" "$(lib job_procs 7 - <<<$'101 8 7 510')"
  check_eq "job_procs: - with a leader there: none" "" "$(lib job_procs 7 - <<<"$ps")"
}

# `omabox clip` (issue #23, finding 119): never for an agent, and what it hands over. Only refusals and
# pure functions here: whoever runs the suite may be an agent, which clip refuses whatever else holds
# (t_clip runs the rest in a box standing in for the host, where no agent is).
t_unit_clip() {
  # A clean environment but for the mark tested (the caller's own PATH may be the guard's). Should a
  # refusal ever break, clip must still reach no clipboard: an empty runtime dir (no Hyprland session
  # to find) and a box that does not exist.
  mkdir -p "$TMP/rt-clip"
  cl() { env -i PATH=/usr/bin:/bin HOME="$HOME" XDG_RUNTIME_DIR="$TMP/rt-clip" "$@" 2>&1; }
  local m
  for m in CLAUDECODE=1 CLAUDE_CODE_SESSION_ID=70178a3a CLAUDE_PID=1 CODEX_THREAD_ID=01a0314c OPENCODE=1 AI_AGENT=x \
      OMABOX_AGENT_PID=1 OMABOX_SESSION=abcd1234 WAYLAND_DISPLAY=omabox-guard HYPRLAND_INSTANCE_SIGNATURE=omabox-guard; do
    check_match "clip refused: $m" "clip is not for agents \(${m%%=*} set for " "$(cl "$m" "$CLI" clip -b "$P-x")"
  done
  check_match "clip refused: the guard's PATH" "clip is not for agents \(PATH set" "$(cl PATH="$ROOT/share/guard:/usr/bin:/bin" "$CLI" clip -b "$P-x")"
  check_eq "...in one line" 1 "$(cl CLAUDECODE=1 "$CLI" clip -b "$P-x" | wc -l)"
  check_match "...before anything else (an argument it does not know)" "not for agents" "$(cl CLAUDECODE=1 "$CLI" clip --bogus)"
  # Dropped from the environment, still in a process it runs under (`; true`: bash would exec its last
  # command otherwise, and the process with the variable would be gone).
  check_match "clip refused: a variable dropped with env -u, still in its shell's" "not for agents \(CLAUDECODE set for bash" \
    "$(cl CLAUDECODE=1 bash -c 'env -u CLAUDECODE "$0" clip -b "$1"; true' "$CLI" "$P-x")"
  cp /usr/bin/bash "$TMP/claude"; cp /usr/bin/bash "$TMP/node"
  check_match "clip refused: under a program named claude, with no variable at all (exec env -i)" "not for agents \(claude, pid" \
    "$(cl "$TMP/claude" -c '"$0" clip -b "$1"; true' "$CLI" "$P-x")"
  check_match "clip refused: under node running Claude Code" "not for agents \(an agent in node" \
    "$(cl "$TMP/node" -c '"$1" clip -b "$2"; true' /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js "$CLI" "$P-x")"
  check_match "clip refused: guard exec" "not for agents" "$(cl "$CLI" guard exec -- "$CLI" clip -b "$P-x")"
  check_eq "clip refused: inside ai-jail (the broker's command)" "inside ai-jail" "$(OMABOX_JAIL='{}' lib agent_caller)"
  check_match "...and the broker never takes it" "clip is not for agents" "$(lib broker_check clip 2>&1)"
  check_match "clip is in the usage" "omabox clip \[-b NAME\] \[--from-box\]" "$(ob help)"
  # The type handed over: text first, then an image, PNG first; nothing else.
  check_eq "clip_pick: text over an image" "text/plain;charset=utf-8" "$(lib clip_pick $'image/png\ntext/html\ntext/plain\ntext/plain;charset=utf-8')"
  check_eq "clip_pick: UTF8_STRING over text/plain" UTF8_STRING "$(lib clip_pick $'STRING\ntext/plain\nUTF8_STRING')"
  check_eq "clip_pick: PNG over another image" image/png "$(lib clip_pick $'image/jpeg\nimage/png\ntext/html')"
  check_eq "clip_pick: an image by its own type" image/webp "$(lib clip_pick $'text/html\nimage/webp')"
  check_fails "clip_pick: neither text nor an image" lib clip_pick $'text/html\ntext/uri-list\nx-special/gnome-copied-files'
  check_fails "clip_pick: no types (an empty clipboard)" lib clip_pick ""
  check_fails "clip_pick: an image type that is not a tame name" lib clip_pick $'image/png;x=\e[31m\nimage/../x'
  check_eq "clip_types: a box's type list, tame and short" "image/png;x=31m text/html " "$(lib clip_types $'image/png;x=\e[31m\ntext/html')"
  check_eq "clip_size" "1 byte|12 bytes|1.5 KiB|64.0 MiB" "$(lib clip_size 1)|$(lib clip_size 12)|$(lib clip_size 1536)|$(lib clip_size $((64 << 20)))"
}

# keys-to-box (issue #22, finding 117): its arguments, its state in ls, and the host's Lua.
t_unit_keys_to_box() {
  check_match "keys-to-box: a box that is not up" "no box '$P-x' is up" "$(ob keys-to-box -b "$P-x" 2>&1)"
  check_match "...-b before the command reaches it" "no box '$P-x' is up" "$(ob -b "$P-x" keys-to-box on 2>&1)"
  check_match "...anything but on or off refused" "on, off, or nothing" "$(ob keys-to-box -b "$P-x" yes 2>&1)"
  check_match "...on and off at once refused" "on or off, once" "$(ob keys-to-box -b "$P-x" on off 2>&1)"
  # ls shows it from the box dir's file: a dead box in a runtime dir of our own.
  local rt=$TMP/rt-keys d; d=$rt/omabox/$P-k; mkdir -p "$d"
  echo '{"mode":"interactive","size":"window","created":"2026-09-30T12:00:00Z","plugins":[]}' > "$d/box.json"
  echo '{}' > "$d/info.json"
  check_eq "ls --json: keys_to_box off by default" false "$(XDG_RUNTIME_DIR=$rt "$CLI" ls --json | jq -r '.[0].keys_to_box')"
  check_fails "...and ls says nothing" grep -q keys-to-box <<<"$(XDG_RUNTIME_DIR=$rt "$CLI" ls)"
  echo on > "$d/keys-to-box"
  check_eq "...on while its file is there" true "$(XDG_RUNTIME_DIR=$rt "$CLI" ls --json | jq -r '.[0].keys_to_box')"
  check_match "...and ls says so under the box" "^  keys-to-box: on" "$(XDG_RUNTIME_DIR=$rt "$CLI" ls | sed -n 3p)"
  if command -v luac >/dev/null; then check "passthrough.lua compiles" luac -p "$ROOT/share/passthrough.lua"
  else skip "passthrough.lua compiles" "no luac"; fi
  check_eq "PASS_VERSION is passthrough.lua's VERSION" "$(sed -n 's/^local VERSION = \([0-9]*\)$/\1/p' "$ROOT/share/passthrough.lua")" \
    "$(sed -n 's/^PASS_VERSION=\([0-9]*\) .*/\1/p' "$CLI")"
}

# finding 88: an agent session's default box is its own.
t_unit_agent_session() {
  local repo; repo=$(tmp_repo as)
  dn() { (cd "$repo" && env -u OMABOX -u OMABOX_SESSION -u CLAUDE_CODE_SESSION_ID -u CODEX_THREAD_ID "$@" bash -c 'source "$1"; default_name' _ "$TMP/lib/bin/omabox"); }
  check_eq "no agent: the repo's name" "$P-as" "$(dn)"
  check_eq "Claude Code: the session id's tail" "$P-as-5cc72cdc" "$(dn CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc)"
  check_eq "Codex: the random tail of its UUIDv7, not the timestamp" "$P-as-c8d64254" "$(dn CODEX_THREAD_ID=01a0314c-9a07-72f1-8183-d09fc8d64254)"
  check_eq "Codex sessions started together get different boxes" "$P-as-6ad3e01f" "$(dn CODEX_THREAD_ID=01a0314c-9a07-72f1-8183-d0a16ad3e01f)"
  check_eq "OMABOX_SESSION wins" "$P-as-abcdef12" "$(dn OMABOX_SESSION=abcdef12 CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc)"
  check_eq "OMABOX_SESSION= opts out" "$P-as" "$(dn OMABOX_SESSION= CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc)"
  check_eq "OMABOX still names the box" shared "$(dn OMABOX=shared CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc)"
  check_eq "a short id is no session" "$P-as" "$(dn CLAUDE_CODE_SESSION_ID=abc)"
  local long; long=$TMP/$P-a-repo-with-a-name-far-longer-than-forty; mkdir -p "$long" && git -C "$long" init -q
  local got; got=$(cd "$long" && CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc bash -c 'source "$1"; select_box ""; echo "$NAME"' _ "$TMP/lib/bin/omabox")
  check_match "a long repo name keeps the session suffix" '^.{31}-5cc72cdc$' "$got"
  check_match "guard exec: a session of its own" '^g[0-9a-f]{12}$' "$(env -u OMABOX_SESSION "$CLI" guard exec -- sh -c 'echo $OMABOX_SESSION')"
  check_eq "guard exec: an OMABOX_SESSION set is kept" mine12345 "$(OMABOX_SESSION=mine12345 "$CLI" guard exec -- sh -c 'echo $OMABOX_SESSION')"
  # finding 93: the agent is Claude Code's CLAUDE_PID, the process `guard exec` became, or (Codex) the
  # nearest ancestor without its session variable; always one this command descends from.
  # (`; true`: bash would otherwise exec its last command, and the "agent" would be gone)
  local out lib=$TMP/lib/bin/omabox
  out=$(bash -c 'echo "me=$$"; CLAUDE_CODE_SESSION_ID=$0 CLAUDE_PID=$$ bash -c '\''source "$1"; agent_proc'\'' _ "$1"; true' "t-$P-session" "$lib")
  check_eq "Claude Code: the agent is CLAUDE_PID" "${out%%$'\n'*}" "me=$(sed -n 2p <<<"$out" | cut -d' ' -f1)"
  sleep 30 & local other=$!
  check_fails "...only when this command descends from it" env CLAUDE_CODE_SESSION_ID="t-$P-session" CLAUDE_PID="$other" bash -c 'source "$1"; agent_proc' _ "$lib"
  out=$(bash -c 'echo "me=$$"; CODEX_THREAD_ID=$0 bash -c '\''source "$1"; agent_proc'\'' _ "$1"; true' "t-$P-thread-0001" "$lib")
  check_eq "Codex: the nearest process without its session variable" "${out%%$'\n'*}" "me=$(sed -n 2p <<<"$out" | cut -d' ' -f1)"
  # ...with 1.5 MB of environment after its variable too (a `tr | grep -q` pipe lost the match there).
  local big=() pad i; printf -v pad '%01000d' 0
  for i in $(seq 1500); do big+=("V$i=$pad"); done
  out=$(bash -c 'echo "me=$$"; env -i PATH="$PATH" HOME="$HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" CODEX_THREAD_ID="$0" "${@:2}" \
    bash -c '\''source "$1"; agent_proc'\'' _ "$1"; true' "t-$P-thread-0001" "$lib" "${big[@]}")
  check_eq "...with a large environment too" "${out%%$'\n'*}" "me=$(sed -n 2p <<<"$out" | cut -d' ' -f1)"
  out=$("$CLI" guard exec -- bash -c 'echo "me=$$"; bash -c '\''source "$1"; agent_proc'\'' _ "$0"' "$lib")
  check_eq "guard exec: the agent is what it ran" "${out%%$'\n'*}" "me=$(sed -n 2p <<<"$out" | cut -d' ' -f1)"
  # A guard exec inside a guarded agent keeps its session, so its agent too (the box is the outer
  # one's), unless it does not run under that agent; a session of its own starts with its own agent.
  out=$("$CLI" guard exec -- bash -c 'echo "me=$$"; "$0" guard exec -- sh -c '\''echo "me=$OMABOX_AGENT_PID"'\''; true' "$CLI")
  check_eq "guard exec in a guarded agent: the agent is still the outer one" "${out%%$'\n'*}" "$(sed -n 2p <<<"$out")"
  out=$(OMABOX_SESSION=abcd1234 OMABOX_AGENT_PID=$other "$CLI" guard exec -- sh -c 'echo "me=$$"; echo "me=$OMABOX_AGENT_PID"')
  check_eq "...not an OMABOX_AGENT_PID it does not run under" "${out%%$'\n'*}" "$(sed -n 2p <<<"$out")"
  out=$(bash -c 'OMABOX_AGENT_PID=$$ "$0" guard exec -- sh -c '\''echo "me=$$"; echo "me=$OMABOX_AGENT_PID"'\''; true' "$CLI")
  check_eq "...and not one from outside a session it starts" "${out%%$'\n'*}" "$(sed -n 2p <<<"$out")"
  kill "$other" 2>/dev/null
  check_fails "OMABOX_SESSION set for one command names no agent (it would go down a minute later)" \
    env OMABOX_SESSION=abcd1234 bash -c 'source "$1"; agent_proc' _ "$lib"
  # An agent that exited is gone even while it is a zombie: its parent (an exec'd sleep) never waits.
  local zd=$TMP/zombie z; mkdir -p "$zd"
  bash -c 'sleep 300 & echo $! > "$0"; exec sleep 300' "$zd/pid" & local zparent=$!
  until_ok 5 test -s "$zd/pid"; z=$(cat "$zd/pid")
  jq -n --argjson a "$z" --arg s "$(bash -c 'source "$1"; proc_start "$2"' _ "$lib" "$z")" '{agent: $a, agent_start: $s}' > "$zd/box.json"
  check "an agent that runs is alive" env D="$zd" bash -c 'source "$1"; agent_alive' _ "$lib"
  kill "$z"; until_ok 5 grep -q '^[^)]*) Z' "/proc/$z/stat"
  check_fails "...one that exited is not, though its parent has not reaped it" env D="$zd" bash -c 'source "$1"; agent_alive' _ "$lib"
  kill "$zparent" 2>/dev/null
  # None recorded, or no start time, is none alive. An empty pid read /proc//stat (the system-wide
  # /proc/stat: no state Z) and an empty start matched the empty one read for it, or for a pid gone.
  # In a condition, as its callers call it (set -e would end it at a failed read otherwise).
  # shellcheck disable=SC2329 # called through check_fails
  alive() { env D="$zd" bash -c 'source "$1"; agent_alive || exit 1' _ "$lib"; }
  local nopid=$(($(cat /proc/sys/kernel/pid_max) + 1))
  echo '{"agent": null}' > "$zd/box.json"
  check_fails "a box that records no agent has none alive" alive
  rm -f "$zd/box.json"
  check_fails "...nor one with no box.json" alive
  jq -n --argjson a "$nopid" '{agent: $a, agent_start: ""}' > "$zd/box.json"
  check_fails "...nor one that records a pid gone, with no start time" alive
  check_fails "proc_stat takes no empty pid" bash -c 'source "$1"; proc_stat "" 0' _ "$lib"
  check_fails "agent_proc fails when its agent's start time cannot be read (it exited meanwhile)" \
    env CLAUDE_CODE_SESSION_ID="t-$P-session" bash -c 'source "$1"; proc_start() { :; }; CLAUDE_PID=$$; agent_proc' _ "$lib"
}

# findings 88 and 93: two agent sessions in one repo get a box each, and one's `down` leaves the
# other's up. A session's box goes when its agent exits, but not while it is in use, and a dead one
# keeps its logs. The "agent" here is a shell that exports its own pid as CLAUDE_PID, as Claude Code does.
t_agent_session() {
  local repo; repo=$(tmp_repo ag)
  local s1=11111111-2222-4333-8444-5555aaaa0001 s2=11111111-2222-4333-8444-5555aaaa0002
  local s3=11111111-2222-4333-8444-5555aaaa0003 s4=11111111-2222-4333-8444-5555aaaa0004
  local s5=11111111-2222-4333-8444-5555aaaa0005 s6=11111111-2222-4333-8444-5555aaaa0006
  as() { local s=$1; shift; (cd "$repo" && env -u OMABOX -u OMABOX_IDLE CLAUDE_CODE_SESSION_ID="$s" "$CLI" "$@"); }
  # Run in the background: exec makes that process the agent, so $! is its pid. Its first command is
  # `up --no-shell` with the options given, or the one after -- (agent S -- run -- true); then it
  # touches $TMP/ag-PID (its output in $TMP/ag-PID.out) and waits.
  agent() {
    local s=$1; shift
    if [ "${1:-}" = -- ]; then shift; else set -- up --no-shell "$@"; fi
    cd "$repo" || exit 1
    exec env -u OMABOX -u OMABOX_IDLE CLAUDE_CODE_SESSION_ID="$s" bash -c 'export CLAUDE_PID=$$
      "${@:2}" >"$1-$$.out" 2>&1; touch "$1-$$"; exec sleep 300' _ "$TMP/ag" "$CLI" "$@" >/dev/null 2>&1
  }
  idle_of() { jq -r .idle "$XDG_RUNTIME_DIR/omabox/$1/box.json"; }
  agent_of() { jq -r .agent "$XDG_RUNTIME_DIR/omabox/$1/box.json" 2>/dev/null; }
  state_of() { "$CLI" ls --json | jq -r --arg n "$1" '[.[] | select(.name == $n) | .state][0] // "gone"'; }
  # shellcheck disable=SC2329 # called through until_ok
  gone() { [ "$(state_of "$1")" = gone ]; }
  local b1=$P-ag-aaaa0001 b2=$P-ag-aaaa0002 b3=$P-ag-aaaa0003 b4=$P-ag-aaaa0004 named=$P-ag-named
  local b5=$P-ag-aaaa0005 b6=$P-ag-aaaa0006 s7=11111111-2222-4333-8444-5555aaaa0007 b7=$P-ag-aaaa0007
  local s8=11111111-2222-4333-8444-5555aaaa0008 b8=$P-ag-aaaa0008 s9=11111111-2222-4333-8444-5555aaaa0009
  local s10=11111111-2222-4333-8444-5555aaaa0010 b10=$P-ag-aaaa0010
  # shellcheck disable=SC2329 # called through until_ok
  polls_every() { pgrep -fx "sleep $2" -P "$(pgrep -f "omabox _reap $1 " | head -1)" >/dev/null; }
  agent "$s1" & local a1=$!
  agent "$s2" --idle 45m & local a2=$!
  agent "$s7" --idle 0 & local a7=$!
  check "the sessions' boxes start" until_ok 40 test -e "$TMP/ag-$a1" -a -e "$TMP/ag-$a2" -a -e "$TMP/ag-$a7"
  check_eq "session 1 has its box" up "$(state_of "$b1")"
  check_eq "session 2 has its own" up "$(state_of "$b2")"
  check_eq "the box knows its agent" "$a1" "$(agent_of "$b1")"
  check_eq "a session's box keeps the 2 h idle limit" 7200 "$(idle_of "$b1")"
  check_eq "--idle still sets it" 2700 "$(idle_of "$b2")"
  check "--idle 0: a reaper still watches the agent, once a minute (not every 5 s)" until_ok 5 polls_every "$b7" 60
  check_eq "session 1's commands reach its box" "$b1" "$(as "$s1" run -- sh -c 'echo $OMABOX_NAME')"
  as "$s2" down >/dev/null 2>&1
  check_eq "session 2's down leaves session 1's box up" up "$(state_of "$b1")"
  as "$s1" up "$named" --no-shell --idle 30s >/dev/null 2>&1
  check_eq "a box named with -b is not tied to the agent" null "$(jq .agent "$XDG_RUNTIME_DIR/omabox/$named/box.json")"
  (cd "$repo" && env -u OMABOX OMABOX_SESSION=abcd1234 "$CLI" up --no-shell --idle 30s >/dev/null 2>&1)
  check_eq "OMABOX_SESSION set for one command: no agent" null "$(jq .agent "$XDG_RUNTIME_DIR/omabox/$P-ag-abcd1234/box.json")"
  (cd "$repo" && env -u OMABOX OMABOX_SESSION=abcd1234 "$CLI" guard exec -- "$CLI" run -- true >/dev/null 2>&1)
  check_eq "...nor does an agent of that session take it over" null "$(jq .agent "$XDG_RUNTIME_DIR/omabox/$P-ag-abcd1234/box.json")"
  ob down "$named" "$P-ag-abcd1234" "$b1" "$b7" >/dev/null 2>&1; kill "$a1" "$a2" "$a7" 2>/dev/null
  # Short idle limits, so the reaper polls every few seconds.
  agent "$s3" --idle 30s & local a3=$!
  agent "$s4" --idle 30s & local a4=$!
  until_ok 40 test -e "$TMP/ag-$a3" -a -e "$TMP/ag-$a4"
  pid_of() { bash -c 'source "$1"; select_box "$2"; box_pid' _ "$TMP/lib/bin/omabox" "$1"; }
  ob run -b "$b3" -- sleep 15 & local busy=$!
  # The run is use once its nsenter is up; a check before that would find the box unused.
  until_ok 10 pgrep -f "^nsenter -t $(pid_of "$b3") "; kill "$a3" 2>/dev/null
  local pid4; pid4=$(pid_of "$b4")
  kill -KILL "$pid4" 2>/dev/null; kill "$a4" 2>/dev/null
  sleep 10
  check_eq "its agent gone, a box in use stays" up "$(state_of "$b3")"
  check_eq "a box that died stays dead, logs and all, when its agent goes" dead "$(state_of "$b4")"
  wait "$busy" 2>/dev/null
  held "$b3" check "...and once not in use, the box goes (before its idle limit)" until_ok 15 gone "$b3"
  ob down "$b3" "$b4" >/dev/null 2>&1
  # A session resumed in a new process (`claude --continue`: the same id, another CLAUDE_PID) takes
  # its box over once the agent it records is gone, whether its first command is `run`, `up`, one
  # through need_box (`hyprctl`) or `path`; another session's command that names the box does not.
  # The reapers are stopped meanwhile, so that no check falls between the old agent's exit and that
  # command.
  agent "$s5" --idle 20s & local a5=$!
  agent "$s6" --idle 20s & local a6=$!
  agent "$s8" --idle 20s & local a8=$!
  until_ok 40 test -e "$TMP/ag-$a5" -a -e "$TMP/ag-$a6" -a -e "$TMP/ag-$a8"
  agent "$s5" -- run -- true & local c5=$!
  until_ok 20 test -e "$TMP/ag-$c5"
  check_eq "a second agent of a session leaves its box to the first while that runs" "$a5" "$(agent_of "$b5")"
  kill "$c5" 2>/dev/null
  local reapers; mapfile -t reapers < <(pgrep -f "omabox _reap ($b5|$b6|$b8) ")
  kill -STOP "${reapers[@]}"
  kill "$a5" "$a6" "$a8" 2>/dev/null; wait "$a5" "$a6" "$a8" 2>/dev/null
  agent "$s5" -- run -- true & local r5=$!
  agent "$s6" & local r6=$!
  agent "$s9" -- hyprctl -b "$b8" -j version & local o8=$!
  until_ok 20 test -e "$TMP/ag-$o8"
  check_eq "another session's command naming the box (-b) does not take it over" "$a8" "$(agent_of "$b8")"
  kill "$o8" 2>/dev/null; wait "$o8" 2>/dev/null   # (had it, the next checks still see their own part)
  agent "$s8" -- hyprctl -j version & local r8=$!
  until_ok 20 test -e "$TMP/ag-$r8"
  check_eq "a resumed session's first hyprctl takes its box over (need_box)" "$r8" "$(agent_of "$b8")"
  kill "$r8" 2>/dev/null; wait "$r8" 2>/dev/null
  agent "$s8" -- path & local q8=$!
  until_ok 20 test -e "$TMP/ag-$q8" -a -e "$TMP/ag-$r5" -a -e "$TMP/ag-$r6"
  check_eq "...and so does its first path" "$q8" "$(agent_of "$b8")"
  kill -CONT "${reapers[@]}"
  sleep 11   # two checks
  check_eq "a resumed session's first run takes its box over" "up $r5" "$(state_of "$b5") $(agent_of "$b5")"
  check_eq "...and so does its first up" "up $r6" "$(state_of "$b6") $(agent_of "$b6")"
  ob path "$b5" >/dev/null; ob path "$b6" >/dev/null   # idle clocks back to 0: what takes them down now is the agent
  kill "$r5" "$r6" 2>/dev/null
  held "$b5" check "...and the box goes with the new agent" until_ok 10 gone "$b5"
  held "$b6" check "...(the one it took over with up too)" until_ok 10 gone "$b6"
  ob down "$b5" "$b6" "$b8" >/dev/null 2>&1
  # The reaper decided to take a box down for an agent that exited, and its `down` waits for the
  # box's lock, which the session's new agent took first to take the box over: the down checks the
  # agent again under the lock and leaves the box, and the reaper then watches the new agent. The
  # order is fixed by hand: the lock is held while both queue, and the reaper's flock is stopped
  # until the new agent's command is done.
  local lk=$XDG_RUNTIME_DIR/omabox/.lock-$b10 reaper held rf
  # shellcheck disable=SC2329 # called through until_ok
  flock_of() { pgrep -x flock -P "$(pgrep -d, -P "$1")"; }   # the flock an agent's command waits in
  agent "$s10" --idle 20s & local a10=$!
  until_ok 40 test -e "$TMP/ag-$a10"
  reaper=$(pgrep -f "omabox _reap $b10 " | head -1)
  flock "$lk" sh -c 'touch "$0"; until [ -e "$1" ] || [ ! -d "${1%/*}" ]; do sleep 0.1; done' "$TMP/held" "$TMP/release" & held=$!
  until_ok 5 test -e "$TMP/held"
  kill "$a10" 2>/dev/null; wait "$a10" 2>/dev/null
  agent "$s10" -- run -- true & local r10=$!
  until_ok 10 flock_of "$r10"                      # its take_over waits for the lock
  until_ok 15 pgrep -x flock -P "$reaper"          # the reaper said "taking it down"; its down waits
  rf=$(pgrep -x flock -P "$reaper")
  [ -z "$rf" ] || kill -STOP "$rf"
  touch "$TMP/release"; wait "$held" 2>/dev/null
  until_ok 20 test -e "$TMP/ag-$r10"
  [ -z "$rf" ] || kill -CONT "$rf"
  held "$b10" check "a takeover made while the reaper's down waited for the lock keeps the box" \
    until_ok 10 grep -q ": kept$" "$XDG_RUNTIME_DIR/omabox/$b10/reap.log"
  check_eq "...for the new agent" "up $r10" "$(state_of "$b10") $(agent_of "$b10")"
  # The same with the reaper first (the new command's flock stopped): its down takes the box, and the
  # command that waited says no box is up (`run -d` wrote its log into the box's dir, gone by then).
  rm -f "$TMP/held" "$TMP/release"
  flock "$lk" sh -c 'touch "$0"; until [ -e "$1" ] || [ ! -d "${1%/*}" ]; do sleep 0.1; done' "$TMP/held" "$TMP/release" & held=$!
  until_ok 5 test -e "$TMP/held"
  kill "$r10" 2>/dev/null; wait "$r10" 2>/dev/null
  agent "$s10" -- run -d -- true & local d10=$!
  until_ok 10 flock_of "$d10"
  until_ok 15 pgrep -x flock -P "$reaper"          # the same reaper, now for r10's exit
  local df; df=$(flock_of "$d10")
  [ -z "$df" ] || kill -STOP "$df"
  touch "$TMP/release"; wait "$held" 2>/dev/null
  held "$b10" check "...and the reaper goes on watching it: the box goes when it exits" until_ok 15 gone "$b10"
  [ -z "$df" ] || kill -CONT "$df"
  until_ok 20 test -e "$TMP/ag-$d10"
  held "$b10" check_match "a first command that waited for the lock while the box went says no box is up" \
    "no box '$b10' is up" "$(cat "$TMP/ag-$d10.out" 2>/dev/null)"
  ob down "$b10" >/dev/null 2>&1
  # And with an `up` of the name that finds no agent (no CLAUDE_PID) queued too, let in after the
  # reaper's down and before the new agent's command: that command must not take over the new box,
  # which records no agent. The up's flock and the command's are stopped until their turn.
  local s11=11111111-2222-4333-8444-5555aaaa0011 b11=$P-ag-aaaa0011 uf tf
  agent "$s11" --idle 20s & local a11=$!
  until_ok 40 test -e "$TMP/ag-$a11"
  reaper=$(pgrep -f "omabox _reap $b11 " | head -1)
  rm -f "$TMP/held" "$TMP/release"
  lk=$XDG_RUNTIME_DIR/omabox/.lock-$b11
  flock "$lk" sh -c 'touch "$0"; until [ -e "$1" ] || [ ! -d "${1%/*}" ]; do sleep 0.1; done' "$TMP/held" "$TMP/release" & held=$!
  until_ok 5 test -e "$TMP/held"
  kill "$a11" 2>/dev/null; wait "$a11" 2>/dev/null
  agent "$s11" -- run -- true & local r11=$!
  (cd "$repo" && exec env -u OMABOX -u OMABOX_IDLE CLAUDE_CODE_SESSION_ID="$s11" "$CLI" up --no-shell \
    --idle 20s) >/dev/null 2>&1 & local u11=$!
  until_ok 10 flock_of "$r11"
  until_ok 10 pgrep -x flock -P "$u11"
  until_ok 15 pgrep -x flock -P "$reaper"
  tf=$(flock_of "$r11"); uf=$(pgrep -x flock -P "$u11")
  [ -z "$tf" ] || kill -STOP "$tf"
  [ -z "$uf" ] || kill -STOP "$uf"
  touch "$TMP/release"; wait "$held" 2>/dev/null
  until_ok 15 gone "$b11"                          # the reaper's down
  [ -z "$uf" ] || kill -CONT "$uf"
  wait "$u11" 2>/dev/null
  [ -z "$tf" ] || kill -CONT "$tf"
  until_ok 20 test -e "$TMP/ag-$r11"
  held "$b11" check_eq "...nor does it take over the box an up of the name started meanwhile, with no agent" \
    "up null" "$(state_of "$b11") $(agent_of "$b11")"
  ob down "$b11" >/dev/null 2>&1
  kill "$a1" "$a2" "$a3" "$a4" "$a5" "$a6" "$a7" "$c5" "$r5" "$r6" "$a8" "$o8" "$r8" "$q8" "$a10" "$r10" "$d10" "$a11" "$r11" 2>/dev/null
}

# `mode` writes box.json's size under the box's lock, as a takeover writes its agent (finding 93): a
# mode change made meanwhile would otherwise put back the agent that exited. The lock is held here.
t_mode_lock() {
  local B=$P-ml m rc=0 held lk=$XDG_RUNTIME_DIR/omabox/.lock-$P-ml
  check "a box for mode" ob up "$B" --no-shell --idle 5m
  local before; before=$(jq -r .size "$XDG_RUNTIME_DIR/omabox/$B/box.json")
  flock "$lk" sh -c 'touch "$0"; until [ -e "$1" ] || [ ! -d "${1%/*}" ]; do sleep 0.1; done' "$TMP/ml-held" "$TMP/ml-release" & held=$!
  until_ok 5 test -e "$TMP/ml-held"
  "$CLI" mode -b "$B" 1280x720 > "$TMP/ml-out" 2>&1 & m=$!
  check "mode waits for the box's lock to record the new size" until_ok 10 pgrep -x flock -P "$m"
  check_eq "...box.json keeps the old one meanwhile" "$before" "$(jq -r .size "$XDG_RUNTIME_DIR/omabox/$B/box.json")"
  touch "$TMP/ml-release"; wait "$held" 2>/dev/null
  wait "$m" || rc=$?
  check_eq "...and records it once it has the lock" "0 1280x720@60" "$rc $(jq -r .size "$XDG_RUNTIME_DIR/omabox/$B/box.json")"
  ob down "$B" >/dev/null 2>&1
}

# finding 90: when an interactive box gives no frame, `shot` never says to show its window but to ask
# the user, and for a box started before drawn_hidden, or whose window confirm-close replaced, that it
# needs a restart (which ends their session in it).
t_unit_shot_hidden() {
  local d=$TMP/boxes/oldbox out
  mkdir -p "$d"
  # (The box's Hyprland answers for its screen size, finding 81; only grim gets no frame.)
  shot_msg() { bash -c 'source "$1"; BOXES=$2; need_box() { :; }
    on_box() { [ "$*" = "hyprctl -j monitors" ] || return 1; echo "[{\"x\": 0, \"y\": 0, \"width\": 1920, \"height\": 1080, \"scale\": 1}]"; }
    cmd_shot -b oldbox -o "$2/x.png"' _ "$TMP/lib/bin/omabox" "$TMP/boxes" 2>&1; }
  echo '{"mode": "interactive", "workspace": "9"}' > "$d/box.json"
  out=$(shot_msg)
  check_match "an old interactive box: it needs a restart, ask the user" "needs a restart.*ask the user" "$out"
  check_fails "...not told to restart it" grep -q "omabox down" <<<"$out"
  echo '{"mode": "interactive", "workspace": "9", "drawn_hidden": true}' > "$d/box.json"
  out=$(shot_msg)
  check_match "a new one: never switch the user's workspace for a frame" "never switch the user's workspace or focus" "$out"
  check_match "...ask the user" "ask the user" "$out"
  check_fails "...and no restart" grep -q restart <<<"$out"
  check_fails "...and no partial PNG left" test -e "$TMP/boxes/x.png.part"
  check_match "a box whose Hyprland does not answer: said, not a silent exit" "cannot read the screen size of box 'oldbox'" \
    "$(bash -c 'source "$1"; BOXES=$2; need_box() { :; }; on_box() { return 1; }; cmd_shot -b oldbox -o "$2/x.png"' _ "$TMP/lib/bin/omabox" "$TMP/boxes" 2>&1)"
  mkdir -p "$d/run" && echo 1 > "$d/run/omabox.reopened"
  out=$(shot_msg)
  check_match "a window confirm-close reopened: not drawn while hidden, ask the user" "confirm-close.*not drawn while hidden.*ask the user" "$out"
  check_match "...never switch the user's workspace for a frame" "Never switch the user's workspace or focus" "$out"
  # Issue #63: an -o that cannot be written is said so before anything is captured, in either mode
  # (an interactive box said "no frame ... ask the user", a headless one "grim failed").
  local ro=$TMP/ro m; mkdir -p "$ro"; chmod 555 "$ro"
  for m in interactive headless; do
    echo "{\"mode\": \"$m\", \"workspace\": \"9\", \"drawn_hidden\": true}" > "$d/box.json"
    out=$(bash -c 'source "$1"; BOXES=$2; need_box() { :; }; on_box() { echo "on_box $*"; return 1; }; cmd_shot -b oldbox -o "$3/x.png"' \
      _ "$TMP/lib/bin/omabox" "$TMP/boxes" "$ro" 2>&1)
    check_match "-o in a dir that cannot be written ($m box): said so" "^omabox: shot: cannot write $ro/x.png: Permission denied$" "$out"
    check_fails "...before the box is asked for anything" grep -q on_box <<<"$out"
  done
  check_fails "...and no partial PNG left" test -e "$ro/x.png.part"
  check_match "-o where no directory can be made" "cannot make the directory /proc/self/omabox-x" "$("$CLI" shot -o /proc/self/omabox-x/y.png 2>&1)"
  chmod 755 "$ro"
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

# One box for most checks: connected network, default size.
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
  # A token of this run's own, so only a process that got the value fails the check below, not one
  # that merely mentions the test (an agent grepping the suite, another run of it).
  local tag secret
  tag=omabox-t66-$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
  secret="$tag = with"$'\nnewline'
  check_eq "--pass hands one over, as it is" "$(printf %q "$secret")" \
    "$(OMABOX_T66=$secret ob run -b "$B" --pass OMABOX_T66 -- bash -c 'printf %q "$OMABOX_T66"')"
  (OMABOX_T66=$secret ob run -b "$B" --pass OMABOX_T66 -- sleep 2.066 >/dev/null 2>&1 &)
  until_ok 5 pgrep -fx 'sleep 2.066'   # the run is under way
  check_fails "...never in any command line" grep -qsF -- "$tag" /proc/[0-9]*/cmdline
  check_fails "--pass of an unset variable fails (no silent skip)" env -u OMABOX_T66 "$CLI" run -b "$B" --pass OMABOX_T66 -- true
  check_fails "--pass takes a name only" ob run -b "$B" --pass 'A=1' -- true
  # --env-file (#30): data, not a script; --pass wins; a bad line is named, its text never shown.
  printf '%s\n' '# a comment' '' 'T30A=plain value' 'export T30B="in # quotes"' "T30C='\$HOME'" 'T30D=file' > "$TMP/t30.env"
  # shellcheck disable=SC2016 # expanded inside the box
  check_eq "--env-file: plain, export, quotes, no expansion; --pass wins" "plain value|in # quotes|\$HOME|pass" \
    "$(T30D=pass ob run -b "$B" --env-file "$TMP/t30.env" --pass T30D -- sh -c 'echo "$T30A|$T30B|$T30C|$T30D"')"
  printf 'T30A=1\nno equals sign hunter2\n' > "$TMP/t30bad.env"
  local e30; e30=$(ob run -b "$B" --env-file "$TMP/t30bad.env" -- true 2>&1)
  check_match "--env-file: a bad line refused by its number" "line 2 is not KEY=VALUE" "$e30"
  check_fails "...its text never shown" grep -q hunter2 <<<"$e30"
  check_eq "--pass ROOT is the caller's, not omabox's own (finding 74)" "/x y" "$(ROOT="/x y" ob run -b "$B" --pass ROOT -- sh -c 'echo "$ROOT"')"
  check_fails "--pass of a name only omabox has fails" env -u GUARD "$CLI" run -b "$B" --pass GUARD -- true
  # finding 67: crashes in a box skip systemd-coredump (a core limit of exactly 1 byte), so they never
  # become crash notifications on the real desktop.
  check_eq "run: core limit 1 byte" 1 "$(ob run -b "$B" -- sh -c 'prlimit --pid $$ --core -o SOFT --noheadings | tr -d " "')"
  check_eq "the session too (Hyprland)" 1 "$(ob run -b "$B" -- sh -c 'prlimit --pid "$(pgrep -x Hyprland)" --core -o SOFT --noheadings | tr -d " "')"
  # finding 98: a Qt app with no terminal logs to the journal, which a box has none of; the box
  # sends it to stderr, so a `run -d` log has what the app printed.
  printf '%s\n' 'import QtQml' 'QtObject { Component.onCompleted: { console.warn("omabox-t98"); Qt.quit() } }' > "$D/home/t98.qml"
  local qlog; qlog=$(ob run -b "$B" -d -- qml6 -platform offscreen /home/sbx/t98.qml 2>&1 >/dev/null | sed -n 's/.*(log: \(.*\))$/\1/p')
  check "run -d: a Qt app's warning reaches its log" until_ok 10 grep -qs omabox-t98 "$qlog"
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
  elif [ "$(lib render_driver "$(lib render_node)")" = nvidia ]; then
    # (Before, this said "the render node's driver is nvidia, not nvidia": up had failed on NVIDIA.)
    no "NVIDIA uses the private Wayland screen" "the render node ($(lib render_node)) is NVIDIA's, but the box did not get the private Wayland screen (box.json wayland_screen: $(jq -r .wayland_screen "$D/box.json" 2>&1)); did up fail?"
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
    # gpu reads DRM fdinfo, which some drivers do not keep at all (virtio_gpu, a VM's): there a
    # client of the render node has no drm-driver line, and nothing is there to list.
    local rn fd fdinfo=0; rn=$(lib render_node)
    exec {fd}<"$rn" && { ! grep -q '^drm-driver:' "/proc/$BASHPID/fdinfo/$fd" || fdinfo=1; exec {fd}<&-; }
    if [ "$fdinfo" = 1 ]; then
      check "gpu --json lists Hyprland" jq -e '.percent | has("Hyprland")' <<<"$gpu"
    else
      skip "gpu --json lists Hyprland" "the render node's driver keeps no DRM fdinfo ($rn)"
    fi
  else
    # NVIDIA's driver may expose no drm-engine-* counters in /proc/*/fdinfo.
    check "gpu --json handles missing driver counters" jq -e '.percent | type == "object"' <<<"$gpu"
  fi
  check_match "gpu first line names the mode" "1280x720@120" "$(ob gpu -b "$B" 1 | head -1)"
  check_eq "gpu --json names the box's aquamarine (#47)" "$(jq -c .aquamarine "$D/box.json")" "$(jq -c .aquamarine <<<"$gpu")"
  check "...and so does its text" grep -qF "aquamarine: " <<<"$(ob gpu -b "$B" 1 | sed -n 2p | grep -F -- "$(jq -r .aquamarine.version "$D/box.json")")"
  check_eq "mode @60.0 is accepted and kept as @60" "$screen_name 1280x720@60" "$(ob mode -b "$B" 1280x720@60.0)"
  check_eq "box.json keeps the normalised mode" "1280x720@60" "$(jq -r .size "$D/box.json")"
  check_eq "shot into a missing dir makes it (#27)" "$TMP/nope/x.png" "$(ob shot -b "$B" -o "$TMP/nope/x.png" 2>/dev/null)"
  check_match "...unless it cannot" "cannot make the directory" "$(ob shot -b "$B" -o "/proc/nope/x.png" 2>&1)"
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
  # Its own box only, as the other throwaway tests look for theirs: another agent's `omabox run` may
  # have a throwaway box up on the same machine. The name first, or the count below tests nothing.
  check_match "throwaway: named $P-tw-runN" "^$P-tw-run[0-9]+\$" "$(cd "$repo" && env -u OMABOX "$CLI" run -- sh -c 'echo "$OMABOX_NAME"' 2>&1)"
  check_eq "throwaway: no box left" 0 "$(ob ls --json | jq --arg p "$P-tw-run" '[.[] | select(.name | startswith($p))] | length')"
}

# Two throwaway host servers on free ports, serving a token of this run's: a server someone else runs
# on a fixed port would answer (or fail) for them.
t_isolated() {
  local B=$P-iso tok=iso-$P-$RANDOM allowed other
  mkdir -p "$TMP/www" && echo "$tok" > "$TMP/www/index.html"
  allowed=$(free_port); until other=$(free_port); [ "$other" != "$allowed" ]; do :; done
  python3 -m http.server "$allowed" --bind 127.0.0.1 --directory "$TMP/www" >/dev/null 2>&1 & SERVERS+=($!)
  python3 -m http.server "$other" --bind 127.0.0.1 --directory "$TMP/www" >/dev/null 2>&1 & SERVERS+=($!)
  check "the host reaches both servers" until_ok 5 bash -c \
    "curl -fsS --max-time 2 http://127.0.0.1:$allowed/ | grep -qx '$tok' && curl -fsS --max-time 2 http://127.0.0.1:$other/ | grep -qx '$tok'"
  check "up --net isolated --allow $allowed" ob up "$B" --net isolated --allow "$allowed"
  check_eq "allowed host port reachable" "$tok" "$(ob run -b "$B" -- curl -s --max-time 3 "http://127.0.0.1:$allowed/")"
  check_fails "other host port unreachable" ob run -b "$B" -- curl -s --max-time 3 "http://127.0.0.1:$other/"
  check_fails "no internet" ob run -b "$B" -- curl -s --max-time 3 -o /dev/null https://archlinux.org
  check_eq "hostname is the host's (finding 54)" "$(uname -n)" "$(ob run -b "$B" -- uname -n)"
  check "down" ob down "$B"
}

# A port below the ephemeral range that nothing on the host listens on, TCP or UDP (pasta forwards
# both), so the server that answers is the test's own.
free_port() {
  local p
  while p=$((20000 + RANDOM % 12000)); ss -Htuln "sport = :$p" | grep -q .; do :; done
  echo "$p"
}

# An HTTP server on a port the kernel picks (bind port 0: the ephemeral range, which pasta's auto
# alone does not forward), which writes that port to FILE once it listens. python3 -c "$PORT0" DIR FILE
PORT0='import functools, http.server, os, sys
d, f = sys.argv[1:3]
s = http.server.HTTPServer(("127.0.0.1", 0), functools.partial(http.server.SimpleHTTPRequestHandler, directory=d))
open(f + ".tmp", "w").write(str(s.server_port)); os.rename(f + ".tmp", f)
s.serve_forever()'

# The host's abstract X11 sockets that are new since BEFORE and not known to belong to a process
# outside pid namespace NS: a box another checkout starts meanwhile holds its own, but a socket
# whose owner cannot be read counts. new_x11 BEFORE NS
new_x11() {
  local n pid ns
  for n in $(grep -o '@/tmp/\.X11-unix/X[0-9]*' /proc/net/unix | LC_ALL=C sort -u | LC_ALL=C comm -13 <(echo "$1") -); do
    pid=$(ss -xlpH | awk -v n="$n" '$5 == n' | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2)
    ns=$(readlink "/proc/${pid:-0}/ns/pid" 2>/dev/null) && [ "$ns" != "$2" ] && continue
    echo "$n"
  done
}

# finding 89: a box's labwc binds the abstract @/tmp/.X11-unix/X0 (its lazy Xwayland) when it starts.
# In the host's network namespace that was the host's :0 (or the next free one), and host X11 apps
# opened in the box. A connected box still reaches the internet and host loopback, and the host its
# servers, on any port. Read-only on the host: no X client, and ports nobody listens on; from the box,
# one connection to its gateway (the router).
t_connected() {
  local B=$P-conn D=$XDG_RUNTIME_DIR/omabox/$P-conn before out rc ns port tok hport htok gw got
  before=$(grep -o '@/tmp/\.X11-unix/X[0-9]*' /proc/net/unix | LC_ALL=C sort -u)
  # (No box, nothing to check; and no stray dir from writing into its HOME.)
  if out=$(ob up "$B" --no-shell --net host 2>&1); then ok "up (--net host: connected's old name)"
  else no "up (--net host: connected's old name)" "$out"; return; fi
  check_match "ls shows it connected" "$B +headless .* up +connected " "$(ob ls)"
  # A no_new_privs process (a sandboxed agent's) cannot start a box behind pasta, but can use this one.
  rc=0; out=$(setpriv --no-new-privs "$CLI" up "$B" 2>&1) || rc=$?
  check_match "up from a no_new_privs process finds it already up" "box '$B' is already up" "$out"
  check_eq "...and succeeds" 0 "$rc"
  # -D none: with --no-map-gw pasta cannot hand the box a loopback nameserver (the host's 127.0.0.53),
  # and would say so on every start.
  check_eq "pasta printed no warning (box.log is empty)" "" "$(cat "$D/box.log" 2>&1)"
  check_eq "no abstract X11 socket of the box's on the host" "" "$(new_x11 "$before" "$(jq -r .pidns "$D/box.json")")"
  # Not the fix itself (on main the box's /proc/net/unix is the host's, and this passes too): labwc
  # did bind one, so the check above had something to find.
  check "labwc's abstract X11 socket is there, seen from the box" ob run -b "$B" -- grep -qE '@/tmp/\.X11-unix/X0$' /proc/net/unix
  ns=$(ob run -b "$B" -- readlink /proc/self/ns/net)
  if [[ $ns == net:* ]] && [ "$ns" != "$(readlink /proc/self/ns/net)" ]; then ok "the box has a network namespace of its own"
  else no "the box has a network namespace of its own" "box [$ns], host [$(readlink /proc/self/ns/net)]"; fi
  if getent ahosts archlinux.org >/dev/null 2>&1; then
    check "DNS resolves in the box" ob run -b "$B" -- getent ahosts archlinux.org
  else echo "       (the host resolves no names: DNS not checked)"; fi
  # host -> box as localhost: the host tries ::1 first, which must be refused, not reset (pasta
  # binds the host side on 127.0.0.1 only); forwards take up to about a second to appear.
  port=$(free_port) tok=box-$P-$RANDOM
  mkdir -p "$D/home/www" && echo "$tok" > "$D/home/www/index.html"
  ob run -b "$B" -d -- python3 -m http.server "$port" --bind 127.0.0.1 --directory /home/sbx/www >/dev/null
  check "the host reaches a box server as localhost" until_ok 10 bash -c "curl -fsS --max-time 2 http://localhost:$port/ | grep -qx '$tok'"
  check_eq "...which is bound on the host's 127.0.0.1 only" "127.0.0.1:$port" "$(ss -Htuln "sport = :$port" | awk '{print $5}' | sort -u)"
  # ...and on a port the kernel chose (1-65535,auto: auto alone skips the ephemeral range)
  tok=box0-$P-$RANDOM
  mkdir -p "$D/home/www0" && echo "$tok" > "$D/home/www0/index.html"
  ob run -b "$B" -d -- python3 -c "$PORT0" /home/sbx/www0 /home/sbx/port0 >/dev/null
  if until_ok 10 test -s "$D/home/port0"; then port=$(cat "$D/home/port0")
    check "the host reaches a box server on a port the kernel chose" until_ok 10 bash -c "curl -fsS --max-time 2 http://127.0.0.1:$port/ | grep -qx '$tok'"
  else no "the host reaches a box server on a port the kernel chose" "the box server wrote no port"; fi
  # box -> host as 127.0.0.1 (localhost from the box resets on an IPv4-only host server: NOTES 89)
  hport=$(free_port) htok=host-$P-$RANDOM
  mkdir -p "$TMP/conn" && echo "$htok" > "$TMP/conn/index.html"
  python3 -m http.server "$hport" --bind 127.0.0.1 --directory "$TMP/conn" >/dev/null 2>&1 & SERVERS+=($!)
  check_eq "the box reaches a host server on 127.0.0.1" "$htok" \
    "$(ob run -b "$B" -- curl -fs --retry 10 --retry-connrefused --retry-delay 1 --max-time 2 "http://127.0.0.1:$hport/")"
  tok=host0-$P-$RANDOM
  mkdir -p "$TMP/conn0" && echo "$tok" > "$TMP/conn0/index.html"
  python3 -c "$PORT0" "$TMP/conn0" "$TMP/conn0.port" >/dev/null 2>&1 & SERVERS+=($!)
  if until_ok 10 test -s "$TMP/conn0.port"; then port=$(cat "$TMP/conn0.port")
    check_eq "...and on a port the kernel chose" "$tok" \
      "$(ob run -b "$B" -- curl -fs --retry 10 --retry-connrefused --retry-delay 1 --max-time 2 "http://127.0.0.1:$port/")"
  else no "...and on a port the kernel chose" "the host server wrote no port"; fi
  # --no-map-gw: the box's gateway is the router, not the host's loopback. One connection to it, on
  # the host server's port: refused or timed out, but not that server.
  gw=$(ob run -b "$B" -- ip -4 route show default | awk '$2 == "via" {print $3; exit}')
  if [ -n "$gw" ]; then
    got=$(ob run -b "$B" -- curl -s --max-time 2 "http://$gw:$hport/")
    if [[ $got != *"$htok"* ]]; then ok "the box's gateway is not the host's loopback"
    else no "the box's gateway is not the host's loopback" "$gw:$hport answered as the host server"; fi
  else echo "       (the box has no default route: gateway not checked)"; fi
  # Stopped now, and dropped from SERVERS (cleanup's, for a suite stopped midway): by the end of the
  # suite their pids may be another process's.
  kill "${SERVERS[@]}" 2>/dev/null; SERVERS=()
  check "down" ob down "$B"
}

t_idle() {
  local B=$P-idle
  check "up --idle 10s" ob up "$B" --idle 10s --no-shell
  until_ok 30 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$B\")'"
  held "$B" check_fails "box went down by itself" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\")'"
  held "$B" check_match "next command says why" "went down after 10s idle" "$(ob shot -b "$B" 2>&1)"
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
  # This box's reaper's flock (cmd_down runs in the reaper's own process), not any box's up or down.
  held "$B" check "the idle reaper decides and waits for the lock" until_ok 30 bash -c 'r=$(pgrep -f "omabox _reap $1 ") && pgrep -P "$r" -f "^flock -w 60 [0-9]+$"' _ "$B"
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

# finding 138: why a mounted plugin is not in the box's bar, said at up and restart-shell (Omarchy's
# validator, then the shell's list and log), and in ls --json; and `omarchy plugin add` from a checkout
# acting on the box only.
t_plugin_check() {
  local B=$P-pc F=$TMP/plugins out
  pfix() {   # DIR ID JQ: a bar widget that draws its dir's name
    mkdir -p "$F/$1"
    jq -n --arg id "$2" '{schemaVersion: 1, id: $id, name: $id, version: "0.1.0", kinds: ["bar-widget"],
      entryPoints: {barWidget: "W.qml"}, barWidget: {displayName: $id, defaultSection: "right"}}' | jq "$3" > "$F/$1/manifest.json"
    printf 'import QtQuick\nimport qs.Ui\nBarWidget {\n  id: root\n  implicitWidth: b.implicitWidth\n  implicitHeight: b.implicitHeight\n  WidgetButton { id: b; bar: root.bar; text: "%s" }\n}\n' "$1" > "$F/$1/W.qml"
  }
  pfix good "$P.good" .; pfix schema "$P.schema" '.schemaVersion = "1"'
  pfix entry "$P.entry" '.entryPoints.barWidget = "Missing.qml"'; pfix qml "$P.qml" .
  cp "$F/qml/W.qml" "$F/W.qml.fixed"; echo 'BarWidget {' >> "$F/qml/W.qml"
  git -C "$F/good" init -q && git -C "$F/good" add . && git -C "$F/good" -c user.name=t -c user.email=t@t commit -qm t && touch "$F/good/new"
  pfix inst "$P.inst" .; git -C "$F/inst" init -q && git -C "$F/inst" add . && git -C "$F/inst" -c user.name=t -c user.email=t@t commit -qm t
  out=$(ob up "$B" --net isolated --ro-bind "$F" --plugin "$F/good" --plugin "$F/schema" --plugin "$F/entry" --plugin "$F/qml" 2>&1) ||
    { no "up with plugins the shell refuses still comes up (warnings, not a refusal)" "$out"; return; }
  ok "up with plugins the shell refuses still comes up (warnings, not a refusal)"
  check_match "a bad manifest: the validator's message" "plugin $P.schema: omarchy-plugin-validate: unsupported or missing schemaVersion" "$out"
  check_match "...and the shell's: not loaded" "plugin $P.schema not loaded: PluginRegistry: unsupported schemaVersion" "$out"
  check_match "a missing entry point: the validator's message" "plugin $P.entry: omarchy-plugin-validate: entry point file not found" "$out"
  check_match "...and the shell's: failed" "plugin $P.entry failed: Plugin widget $P.entry failed: .*No such file" "$out"
  check_match "a QML error (the validator passes it): the shell's line" "plugin $P.qml failed: Plugin widget $P.qml failed: .*Syntax error" "$out"
  check_match "...and where the rest is" "omabox log -b $B shell" "$out"
  check_fails "a plugin that loads: no line" grep -q "$P.good" <<<"$out"
  check_eq "ls --json: each plugin's state" "loaded failed failed not loaded" \
    "$(ob ls --json | jq -r --arg n "$B" --arg p "$P" '.[] | select(.name == $n) | .plugin_status | [.[$p + ".good", $p + ".qml", $p + ".entry", $p + ".schema"] | .state] | join(" ")')"
  check_match "...a checkout's commit, +dirty with uncommitted files (finding 139)" "^[0-9a-f]{7,}\+dirty$" \
    "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.good" '.[] | select(.name == $n) | .plugin_status[$i].commit')"
  check_eq "...none for a plugin outside git" null "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.qml" '.[] | select(.name == $n) | .plugin_status[$i].commit')"
  check_eq "ls --json: the box's theme" "$(cat "$(ob path "$B")/home/.local/state/omarchy/current/theme.name")" \
    "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .theme')"
  cp "$F/W.qml.fixed" "$F/qml/W.qml"
  out=$(ob restart-shell -b "$B" 2>&1)
  check_fails "fixed, restart-shell: no line for it" grep -q "$P.qml failed" <<<"$out"
  check_match "...still one for the others" "plugin $P.entry failed" "$out"
  check_eq "...and ls --json has it loaded" loaded "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.qml" '.[] | select(.name == $n) | .plugin_status[$i].state')"
  # The install path (a plugin's README way), on the committed checkout: into the box HOME and its shell.
  local H; H=$(ob path "$B")/home
  check "omarchy plugin add from a checkout in the box" ob run -b "$B" -- omarchy plugin add "$F/inst" --yes --enable
  check "...cloned into the box HOME" test -d "$H/.config/omarchy/plugins/$P.inst/.git"
  check_fails "...not into yours" test -e "$HOME/.config/omarchy/plugins/$P.inst"
  check_match "...placed in the box's bar" "\"$P.inst\"" "$(jq -c .bar.layout "$H/.config/omarchy/shell.json")"
  check "omarchy plugin remove in the box" ob run -b "$B" -- omarchy plugin remove "$P.inst" --yes
  check_fails "...leaves nothing in its shell.json" grep -q "$P.inst" "$H/.config/omarchy/shell.json"
  check_fails "...nor its plugins dir" test -e "$H/.config/omarchy/plugins/$P.inst"
  ob down "$B" >/dev/null
}

# Saves (finding 100): a box HOME kept for up/run --from. In the suite's own data dir, never the user's.
# The user's aquamarine build (setup --aquamarine) lives in the data dir too: linked in, so a box from a
# save finds it where the user's own boxes do (finding 130: on NVIDIA it was refused without it).
sv() {
  local aq=${XDG_DATA_HOME:-$HOME/.local/share}/omabox/aquamarine
  [ ! -d "$aq" ] || [ -e "$TMP/data/omabox/aquamarine" ] ||
    { mkdir -p "$TMP/data/omabox" && ln -sfn "$aq" "$TMP/data/omabox/aquamarine"; } 2>/dev/null
  XDG_DATA_HOME=$TMP/data "$CLI" "$@"
}
t_unit_saves() {
  check_eq "no saves: an empty list" "[]" "$(sv saves --json)"
  check_match "a save needs a name" "name it" "$(sv save 2>&1)"
  check_match "a bad save name is refused" "bad save name" "$(sv save ../x -b "$P-x" 2>&1)"
  check_match "...before looking for the box" "bad save name" "$(sv save 'a b' 2>&1)"
  check_match "up --from a save that does not exist is refused" "no save '$P-none'" "$(sv up "$P-nf" --from "$P-none" 2>&1)"
  check_fails "...before the box dir is made" test -e "$XDG_RUNTIME_DIR/omabox/$P-nf"
  check_match "saves rm of none says so" "no save" "$(sv saves rm "$P-none" 2>&1)"
  check_match "saves: an unknown word refused" "rm SAVE" "$(sv saves junk 2>&1)"
}
t_saves() {
  local B=$P-sv B2=$P-sv2 S=$P-s d=$TMP/data/omabox/saves/$P-s
  check "up" ob up "$B" --no-shell
  ob run -b "$B" -- sh -c 'mkdir -p ~/.local/share/app ~/.cache/app && echo hello > ~/.local/share/app/data &&
    echo junk > ~/.cache/app/x && echo "# mine" >> ~/.config/btop/btop.conf && echo "{\"junk\": 1}" > ~/.config/omarchy/shell.json &&
    printf pw | secret-tool store --label t service omabox-test' >/dev/null 2>&1
  check_match "save" "saved box '$B' as '$S'.*keyring secrets included" "$(sv save "$S" -b "$B" 2>&1)"
  check_eq "...nothing in the box left stopped" "" "$(ob run -b "$B" -- ps -eo stat= | grep '^T')"
  check_match "a second save under the name is refused" "exists" "$(sv save "$S" -b "$B" 2>&1)"
  check "...with --force it replaces it" sv save "$S" -b "$B" --force
  check_eq "saves --json lists it, from its box, with its keyring" "$B true" "$(sv saves --json | jq -r --arg s "$S" '.[] | select(.name == $s) | "\(.box) \(.keyring)"')"
  check_eq "the saves dir is private" 700 "$(stat -c %a "$TMP/data/omabox/saves")"
  check_fails "...no .cache in the save" test -e "$d/home/.cache"
  check_eq "...nor the box's logs" "" "$(cd "$d/home" && ls -- *.log 2>/dev/null)"
  check "down" ob down "$B"
  check "...starts" sv up "$B2" --from "$S" --no-shell
  check_eq "...with the app's data" hello "$(ob run -b "$B2" -- cat /home/sbx/.local/share/app/data)"
  check_eq "...and its keyring secret" pw "$(ob run -b "$B2" -- secret-tool lookup service omabox-test)"
  check_eq "...an app config changed in the box kept (no /etc/skel over it)" "# mine" "$(ob run -b "$B2" -- tail -n 1 /home/sbx/.config/btop/btop.conf)"
  check_eq "...the bar's config seeded again, not the save's" null "$(ob run -b "$B2" -- jq .junk /home/sbx/.config/omarchy/shell.json)"
  check "...the theme seeded again (not copied into the save's)" ob run -b "$B2" -- sh -c 't=/home/sbx/.local/state/omarchy/current/theme; test -d $t && test ! -e $t/theme'
  check_eq "...box.json says where it came from" "$S" "$(jq -r .from "$(ob path "$B2")/box.json")"
  check "down" ob down "$B2"
  check_eq "run --from: a throwaway box with the save" hello \
    "$(cd "$(tmp_repo sv)" && env -u OMABOX XDG_DATA_HOME="$TMP/data" "$CLI" run --from "$S" --no-shell -- cat /home/sbx/.local/share/app/data 2>/dev/null)"
  check "saves rm" sv saves rm "$S"
  check_fails "...it is gone" test -e "$d"
}

# finding 87: an app installed per user into a running box (a .desktop with DBusActivatable=true and
# its D-Bus service in ~/.local/share) is listed by the bus and starts from the launcher, as on the host.
dbus_user_app() {
  local B=$1 d; d=$(ob path -b "$B")/home/.local/share
  mkdir -p "$d/applications" "$d/dbus-1/services"
  printf '[Desktop Entry]\nType=Application\nName=Probe\nExec=/bin/true\nDBusActivatable=true\n' > "$d/applications/org.omabox.Probe.desktop"
  # It leaves a mark, and its name appears for a moment (gdbus's own connection claims it, then exits),
  # so the activation completes at once and gtk-launch returns.
  printf '[D-BUS Service]\nName=org.omabox.Probe\nExec=/usr/bin/bash -c "touch /tmp/probe-started; gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.RequestName org.omabox.Probe 0 >/dev/null; sleep 5"\n' \
    > "$d/dbus-1/services/org.omabox.Probe.service"
  check "an app installed after up is activatable" until_ok 5 bash -c "'$CLI' run -b '$B' -- busctl --user list --activatable | grep -q org.omabox.Probe"
  ob run -b "$B" -- timeout 10 gtk-launch org.omabox.Probe >/dev/null 2>&1
  check "it starts from the launcher" until_ok 5 ob run -b "$B" -- test -e /tmp/probe-started
}

t_dbus_user_app() {
  local B=$P-dbusapp
  check "up" ob up "$B" --no-shell
  check_eq "XDG_DATA_HOME as in a session" /home/sbx/.local/share "$(ob run -b "$B" -- printenv XDG_DATA_HOME)"
  dbus_user_app "$B"
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
  # The browser bind's shape with a real user manager: omabox's uwsm-app detaches the app, and a unit
  # around it ended with the app inside (finding 131).
  ob run -b "$B" -- systemd-run --user --quiet --collect --unit=omarchy-browser-1 uwsm-app -- foot --app-id=omabox.sdrun sleep 60 >/dev/null
  check "systemd-run --user uwsm-app -- APP: the app stays" until_ok 10 \
    bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox.sdrun\")'"
  dbus_user_app "$B"
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
  wait "$a"; wait "$b"
  # One bwrap is two processes (its monitor, and the box's PID 1), under one pasta. Anchored: pasta's
  # own command line holds bwrap's too.
  check_eq "one box" 2 "$(pgrep -fc "^bwrap .*--bind $XDG_RUNTIME_DIR/omabox/$B/run " || true)"
  check_eq "...behind one pasta" 1 "$(pgrep -fc "^pasta .* -P $XDG_RUNTIME_DIR/omabox/$B/pasta\.pid " || true)"
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

# A box whose pasta died (OOM, a killall) ends with it rather than stay up with no network (finding 89).
t_pasta_dies() {
  local B=$P-pdie
  check "up" ob up "$B" --no-shell
  kill -KILL "$(cat "$XDG_RUNTIME_DIR/omabox/$B/pasta.pid")"
  check "the box ends with its pasta" until_ok 10 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\" and .state == \"dead\")'"
  check "down" ob down "$B"
}

# From another user namespace (a sandbox that makes its own) a live box's pid namespace is unreadable:
# box_pid read that as dead, and `up` cleared the live box's dir, orphaning the box. It stops instead.
t_other_userns() {
  local B=$P-uns D=$XDG_RUNTIME_DIR/omabox/$P-uns out rc created
  check "up" ob up "$B" --no-shell
  created=$(jq -r .created "$D/box.json")
  rc=0; out=$(unshare -Ur "$CLI" up "$B" 2>&1) || rc=$?
  check_match "up from another user namespace stops: it cannot tell" "cannot tell whether box '$B' is up from this user namespace" "$out"
  check_eq "...with an error" 1 "$rc"
  check_eq "...and the box is the same one, still up" "$created up" "$(jq -r .created "$D/box.json" 2>&1) $(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  check "...which run still reaches" ob run -b "$B" -- true
  check "down" ob down "$B"
}

# A no_new_privs process (an agent's sandbox) cannot start pasta's bwrap (finding 89): a connected box
# started from one gets a network namespace with only a loopback, as a nested box does, and says so.
t_no_new_privs() {
  local B=$P-nnp D=$XDG_RUNTIME_DIR/omabox/$P-nnp before out rc ns
  before=$(grep -o '@/tmp/\.X11-unix/X[0-9]*' /proc/net/unix | LC_ALL=C sort -u)
  rc=0; out=$(setpriv --no-new-privs "$CLI" up "$B" --no-shell 2>&1) || rc=$?
  check_eq "up from a no_new_privs process" 0 "$rc"
  check_match "...says the box has no network, and why" "box '$B' has no network: .*no_new_privs" "$out"
  check_match "ls shows it with none" "$B +headless .* up +none " "$(ob ls)"
  check_fails "...and it has no pasta" test -e "$D/pasta.pid"
  ns=$(ob run -b "$B" -- readlink /proc/self/ns/net)
  if [[ $ns == net:* ]] && [ "$ns" != "$(readlink /proc/self/ns/net)" ]; then ok "the box has a network namespace of its own"
  else no "the box has a network namespace of its own" "box [$ns], host [$(readlink /proc/self/ns/net)]"; fi
  check_eq "no abstract X11 socket of the box's on the host" "" "$(new_x11 "$before" "$(jq -r .pidns "$D/box.json")")"
  check "run works in it from a no_new_privs process too" setpriv --no-new-privs "$CLI" run -b "$B" -- true
  check "down" ob down "$B"
}

# --no-shell is a bare compositor.
t_no_shell() {
  local B=$P-bare
  check "up --no-shell" ob up "$B" --no-shell
  check "no quickshell starts" holds 2 bash -c "! '$CLI' run -b '$B' -- pgrep -x quickshell"
  check "down" ob down "$B"
}

# finding 122: an NVIDIA GPU whose /dev/nvidiaN is missing (switched to the driver at runtime) gets it
# from nvidia-modprobe -c MINOR; still missing, the error says what to run. A fake /proc/driver/nvidia
# and /dev (a link to /dev/null is a character device to `test -c`), and a stub nvidia-modprobe.
t_unit_nvidia() {
  local d=$TMP/$P-nv slot=0000:01:00.0 out
  mkdir -p "$d/proc/gpus/$slot" "$d/dev" "$d/stub" "$d/none"
  printf 'Model: \t\t NVIDIA GeForce RTX 5070 Ti\nIRQ:   \t\t 180\nDevice Minor: \t 3\n' > "$d/proc/gpus/$slot/information"
  ln -s /dev/null "$d/dev/nvidiactl"
  printf '#!/bin/sh\necho "$*" >> "%s/calls"\n[ "$NV_STUB" = fail ] || ln -sf /dev/null "%s/dev/nvidia$2"\n' "$d" "$d" > "$d/stub/nvidia-modprobe"
  chmod +x "$d/stub/nvidia-modprobe"
  # (Always with the stub first in PATH: the real nvidia-modprobe is setuid root and makes /dev nodes.)
  nv() { PATH=$1:$PATH lib nvidia_node "$slot" "$2" "$d/proc" "$d/dev" 2>&1; }
  export NV_STUB=fail
  out=$(nv "$d/stub" 1)
  check_match "a missing node the helper does not create: what to run" "no usable $d/dev/nvidia3 .*run \`nvidia-modprobe -c 3\`" "$out"
  check_eq "...after asking it once, for that minor" "-c 3" "$(cat "$d/calls" 2>&1)"
  rm -f "$d/calls"
  check_match "CREATE 0 (an interactive box's other GPUs): not asked" "no usable" "$(nv "$d/stub" 0)"
  check_fails "...no call" test -e "$d/calls"
  NV_STUB=ok
  check_eq "a missing node: nvidia-modprobe -c MINOR creates it" "$d/dev/nvidia3" "$(nv "$d/stub" 1)"
  check_eq "...one call" "-c 3" "$(cat "$d/calls" 2>&1)"
  check_eq "a node there: used, the helper not asked" "$d/dev/nvidia3|-c 3" "$(nv "$d/stub" 1)|$(cat "$d/calls")"
  rm "$d/dev/nvidiactl" "$d/calls"
  check_match "a missing control node asks too" "no usable .*nvidiactl" "$(NV_STUB=fail nv "$d/stub" 1)"
  check_eq "...(for the GPU's minor)" "-c 3" "$(cat "$d/calls" 2>&1)"
  unset NV_STUB
  check_match "no device minor in the driver's file" "no device minor" \
    "$(printf 'Model: x\n' > "$d/proc/gpus/$slot/information"; nv "$d/none" 1)"
  check_match "a GPU the driver does not list" "has no $d/proc/gpus/0000:02:00.0/information" \
    "$(PATH=$d/none:$PATH lib nvidia_node 0000:02:00.0 1 "$d/proc" "$d/dev" 2>&1)"
}

# finding 126 (issue #49): omabox setup makes a user's links, settings dir and guard; --remove undoes
# them, only links that still point here. In a temp HOME and XDG dirs: never the user's.
t_unit_setup() {
  local h=$TMP/$P-setup out
  mkdir -p "$h/.codex" "$h/.config/omarchy"
  echo '{"bar": {"layout": {"right": [{"id": "x.y"}]}}}' > "$h/.config/omarchy/shell.json"
  # (The XDG dirs too: the user's session sets them, and --remove deletes the aquamarine build there.)
  in_h() { HOME=$h XDG_DATA_HOME=$h/.local/share XDG_CACHE_HOME=$h/.cache XDG_CONFIG_HOME=$h/.config XDG_STATE_HOME=$h/.local/state "$CLI" "$@" 2>&1; }
  out=$(in_h setup); local rc=$?
  check_eq "setup in a clean HOME" 0 "$rc"
  # (No ~/.local/bin/omabox for a system install: its command is /usr/bin/omabox.)
  local bin="$ROOT/bin/omabox "; [[ $ROOT != /usr/* ]] || bin=""
  check_eq "...omabox, the skill (Claude Code, the shared dir, Codex: it is there) and the widget linked" \
    "$bin$ROOT/skill $ROOT/skill $ROOT/skill $ROOT/plugin" \
    "$(readlink "$h/.local/bin/omabox" "$h/.claude/skills/omabox" "$h/.agents/skills/omabox" "$h/.codex/skills/omabox" "$h/.config/omarchy/plugins/chaves.omabox" | tr '\n' ' ' | sed 's/ $//')"
  [[ $ROOT != /usr/* ]] || check_fails "...no ~/.local/bin/omabox for a system install" test -e "$h/.local/bin/omabox"
  check_fails "...not for an agent that is not installed" test -e "$h/.hermes"
  check "...the settings dir" test -d "$h/.config/omabox"
  check_match "...the guard not turned on without a terminal" "not asked \(no terminal\): omabox guard on" "$out"
  check_eq "setup again: the same (idempotent)" 0 "$(in_h setup >/dev/null; echo $?)"
  check_match "setup --force alone is refused" "--force is for --aquamarine" "$(in_h setup --force)"
  check_match "...and --aquamarine with --remove" "--aquamarine or --remove, not both" "$(in_h setup --aquamarine --remove)"
  # What --remove takes away, and what it leaves: a link moved elsewhere, a real dir, the settings.
  in_h guard on claude >/dev/null
  ln -sfn /elsewhere "$h/.codex/skills/omabox"
  mkdir -p "$h/.pi/agent/skills/omabox" "$h/.local/share/omabox/aquamarine/lib" "$h/.cache/omabox-aquamarine" "$h/.local/share/omabox/saves/s1" "$h/.cache/omabox"
  echo bar-icon=auto > "$h/.config/omabox/config"
  out=$(in_h setup --remove); rc=$?
  check_eq "setup --remove" 0 "$rc"
  local f gone=""
  for f in .local/bin/omabox .claude/skills/omabox .agents/skills/omabox .config/omarchy/plugins/chaves.omabox .local/share/omabox/aquamarine .cache/omabox-aquamarine; do
    [ -e "$h/$f" ] || [ -L "$h/$f" ] || gone+="$f "
  done
  check_eq "...removes the links and the user's aquamarine build" ".local/bin/omabox .claude/skills/omabox .agents/skills/omabox .config/omarchy/plugins/chaves.omabox .local/share/omabox/aquamarine .cache/omabox-aquamarine " "$gone"
  check_eq "...not a link that points elsewhere now" /elsewhere "$(readlink "$h/.codex/skills/omabox")"
  check "...nor a real dir" test -d "$h/.pi/agent/skills/omabox"
  check_eq "...keeps the settings and saves, saying so" "bar-icon=auto|yes" \
    "$(cat "$h/.config/omabox/config")|$([ -d "$h/.local/share/omabox/saves/s1" ] && grep -q "kept: your settings" <<<"$out" && echo yes)"
  check_fails "...turns the guard off" grep -q omabox-guard "$h/.claude/settings.json"
  check_fails "...and leaves a widget this HOME has not enabled alone (no omarchy call)" grep -q "plugin disable" <<<"$out"
  check_fails "...and the boxes' home dir, empty (finding 130)" test -e "$h/.cache/omabox"
  check_eq "--remove again: nothing left to do" 0 "$(in_h setup --remove >/dev/null; echo $?)"
  mkdir -p "$h/.cache/omabox/b/home"; in_h setup --remove >/dev/null
  check "...a box's home still in it stays" test -d "$h/.cache/omabox/b/home"
}

# finding 130: a user Omarchy has set no theme for (an account that never logged in) is told why,
# before anything of the box is made (it once died on a bare cp, leaving the box's dirs behind).
t_unit_no_theme() {
  local h=$TMP/$P-notheme rt=$TMP/$P-notheme-run out rc
  mkdir -p "$h/.local/state/omarchy" "$rt"; chmod 700 "$rt"
  out=$(HOME=$h XDG_RUNTIME_DIR=$rt XDG_CACHE_HOME=$h/.cache XDG_DATA_HOME=$h/.local/share XDG_CONFIG_HOME=$h/.config XDG_STATE_HOME=$h/.local/state \
    OMABOX_AQUAMARINE=system "$CLI" up nt 2>&1); rc=$?
  check_eq "up with no Omarchy theme fails" 1 "$rc"
  check_match "...saying why and what to do" "has not set a theme for you yet .*log into Omarchy once" "$out"
  check_fails "...before the box's dirs are made" test -e "$h/.cache/omabox/nt"
  check_fails "...or its runtime dir" test -e "$rt/omabox/nt"
}

# finding 125 (issue #47): which aquamarine a box runs (aq_pick), and what is refused without the fix.
t_unit_aquamarine() {
  local d=$TMP/$P-aq so=libaquamarine.so.14
  mkdir -p "$d/sys" "$d/co/lib" "$d/user/lib" "$d/stale/lib" "$d/home"
  : > "$d/sys/libaquamarine.so.0.15.0"; ln -s libaquamarine.so.0.15.0 "$d/sys/$so"
  : > "$d/user/lib/libaquamarine.so.0.15.1"; ln -s libaquamarine.so.0.15.1 "$d/user/lib/$so"; echo abc1234 > "$d/user/.omabox-commit"
  pick() { OMABOX_AQUAMARINE="" lib aq_pick "$@" 2>&1; }
  check_eq "no private build: the system's, 0.15.0 lacks the fix" "system||0.15.0|0" "$(pick "$so" "$d/sys" "$d/co/lib")"
  check_eq "a private build with the soname: used, with its commit, fixed" "private|$d/user/lib|0.15.1@abc1234|1" \
    "$(pick "$so" "$d/sys" "$d/co/lib" "$d/user/lib")"
  : > "$d/co/lib/libaquamarine.so.0.15.1"; ln -s libaquamarine.so.0.15.1 "$d/co/lib/$so"
  check_eq "...the first one listed (a checkout's before the user's)" "private|$d/co/lib|0.15.1|1" \
    "$(pick "$so" "$d/sys" "$d/co/lib" "$d/user/lib")"
  check_eq "OMABOX_AQUAMARINE=system: the system's all the same" "system||0.15.0|0" \
    "$(OMABOX_AQUAMARINE=system lib aq_pick "$so" "$d/sys" "$d/co/lib" 2>&1)"
  check_match "...anything else refused" "OMABOX_AQUAMARINE is system or unset" "$(OMABOX_AQUAMARINE=yes lib aq_pick "$so" "$d/sys" 2>&1)"
  if command -v cc >/dev/null; then
    echo 'int aq(void) { return 0; }' > "$d/aq.c"
    cc -shared -fPIC -Wl,-soname,libaquamarine.so.13 -o "$d/stale/lib/libaquamarine.so.0.14.0" "$d/aq.c"
  else : > "$d/stale/lib/libaquamarine.so.0.14.0"; fi
  local out; out=$(pick "$so" "$d/sys" "$d/stale/lib")
  check_match "a private build with another soname (stale): skipped, saying so" \
    "skipping the aquamarine in $d/stale/lib: Hyprland links $so, it has (libaquamarine.so.13|another)" "$out"
  check_match "...and the system's used" $'\n''system\|\|0.15.0\|0$' "$out"
  local v
  for v in 0.15.1:0 0.15.2:1 0.15.10:1 0.16.0:1 1.0:1 0.9.9:0; do
    : > "$d/sys/libaquamarine.so.${v%:*}"; ln -sf "libaquamarine.so.${v%:*}" "$d/sys/$so"
    check_eq "the system's ${v%:*}: fixed ${v#*:}" "system||${v%:*}|${v#*:}" "$(pick "$so" "$d/sys")"
  done
  check_match "a soname the system lacks" "Hyprland links libaquamarine.so.99, which is not in $d/sys" "$(pick libaquamarine.so.99 "$d/sys")"
  check_fails "no soname at all: no answer (not a match on the dir)" lib aq_pick "" "$d/sys" "$d/user/lib"
  # What needs the fix, on this machine's system aquamarine (OMABOX_AQUAMARINE=system).
  if aq_unfixed env OMABOX_AQUAMARINE=system "$CLI"; then
    check_match "without the fix: up --confirm-close refused, saying why and what to run" \
      "--confirm-close .*needs aquamarine's fix .*hyprwm/aquamarine#415.*omabox setup --aquamarine" \
      "$(OMABOX_AQUAMARINE=system ob up "$P-aq" --interactive --confirm-close 2>&1)"
    check_fails "...nothing made" test -e "$XDG_RUNTIME_DIR/omabox/$P-aq"
    check_match "...config confirm-close on: set, with a note" "note: confirm-close needs aquamarine's fix.*confirm-close=on" \
      "$(HOME=$d/home OMABOX_AQUAMARINE=system "$CLI" config confirm-close on 2>&1 | tr '\n' ' ')"
    local n nv=""
    for n in /dev/dri/renderD*; do [ "$(lib render_driver "$n")" != nvidia ] || { nv=$n; break; }; done
    if [ -n "$nv" ]; then
      check_match "...a headless box on NVIDIA refused, saying what to run" "a headless box on NVIDIA \($nv\) needs aquamarine's fix.*omabox setup --aquamarine" \
        "$(OMABOX_AQUAMARINE=system OMABOX_RENDER_NODE=$nv ob up "$P-aq" 2>&1)"
      check_fails "...nothing made" test -e "$XDG_RUNTIME_DIR/omabox/$P-aq"
    else
      skip "a headless box on NVIDIA without the fix is refused" "no NVIDIA render node"
    fi
  else
    skip "what needs aquamarine's fix is refused without it" "the system's aquamarine has it"
  fi
  # The widget's confirm-close switch reads this (greyed while false).
  if aq_unfixed env OMABOX_AQUAMARINE=system "$CLI"; then
    check_eq "config --json: confirm-close unavailable without the fix" false \
      "$(HOME=$d/home OMABOX_AQUAMARINE=system "$CLI" config --json | jq '."confirm-close-available"')"
  fi
  if aq_unfixed "$CLI"; then
    skip "config --json: confirm-close available with the fix" "boxes use an aquamarine without it"
  else
    check_eq "config --json: confirm-close available with the fix" true "$(HOME=$d/home "$CLI" config --json | jq '."confirm-close-available"')"
  fi
  check_match "setup: an unknown option, refused before anything is done" "unknown option --nope" "$("$CLI" setup --aquamarine --nope 2>&1)"
}

# finding 116: up --hyprland PATH refuses what the box could not run, before anything is made.
t_unit_hyprland() {
  local d=$TMP/$P-hyp out
  mkdir -p "$d/fh" "$d/proj/build" "$d/other"
  printf '#!/bin/sh\nexec /usr/bin/Hyprland "$@"\n' > "$d/wrapper"; chmod +x "$d/wrapper"
  cp /usr/bin/true "$d/noaq"; cp /usr/bin/true "$d/fh/Hyprland"; : > "$d/plain"
  check_match "a missing file" "no such file" "$(ob up "$P-h1" --hyprland "$d/nosuch" 2>&1)"
  check_match "a file that is not executable" "is not executable" "$(ob up "$P-h1" --hyprland "$d/plain" 2>&1)"
  check_match "a script is not an ELF binary" "is not an ELF binary" "$(ob up "$P-h1" --hyprland "$d/wrapper" 2>&1)"
  check_match "an ELF with no libaquamarine" "links no libaquamarine" "$(ob up "$P-h1" --hyprland "$d/noaq" 2>&1)"
  check_match "its folder is refused as --ro-bind's are (here: all of HOME)" "refusing to mount $d/fh \(the binary's folder\).*it contains" \
    "$(HOME=$d/fh "$CLI" up "$P-h1" --hyprland "$d/fh/Hyprland" 2>&1)"
  check_match "run takes it for a throwaway (and refuses the same)" "is not an ELF binary" "$(ob run --hyprland "$d/wrapper" -- true 2>&1)"
  if command -v cc >/dev/null; then
    # A build against another aquamarine: a stub library with another soname, and a program linking it.
    echo 'int aq(void) { return 0; }' > "$d/aq.c"; echo 'int aq(void); int main(void) { return aq(); }' > "$d/m.c"
    mkdir -p "$d/aq99" "$d/aq14" "$d/b99" "$d/b14"
    local so; so=$(lib hypr_aq_soname)
    cc -shared -fPIC -Wl,-soname,libaquamarine.so.99 -o "$d/aq99/libaquamarine.so" "$d/aq.c" &&
      cc -o "$d/b99/Hyprland" "$d/m.c" -L"$d/aq99" -laquamarine
    check_match "a build against another libaquamarine soname is refused, saying which" \
      "links libaquamarine.so.99, which neither the system nor a private build of omabox's has \(the system has $so\)" \
      "$(ob up "$P-h1" --hyprland "$d/b99/Hyprland" 2>&1)"
    # The right soname, but a library the box does not have.
    cc -shared -fPIC -Wl,-soname,"$so" -o "$d/aq14/libaquamarine.so" "$d/aq.c" &&
      cc -shared -fPIC -Wl,-soname,libomaboxmissing.so.1 -o "$d/aq14/libomaboxmissing.so" "$d/aq.c" &&
      cc -o "$d/b14/Hyprland" "$d/m.c" -L"$d/aq14" -laquamarine -lomaboxmissing -Wl,--no-as-needed
    check_match "...and one needing a library the box lacks" "needs libomaboxmissing.so.1, which the box lacks" \
      "$(ob up "$P-h1" --hyprland "$d/b14/Hyprland" 2>&1)"
  else
    skip "builds against another aquamarine are refused" "no C compiler"
  fi
  # A jailed agent (finding 99): the binary's folder is one of the jail's, whole, or the binary is in its
  # project (in the box already). Past that, --size junk stops `up` before anything starts.
  git -C "$d/proj" init -q
  cp /usr/bin/true "$d/proj/build/Hyprland"; cp /usr/bin/true "$d/proj/Hyprland"; cp /usr/bin/true "$d/other/Hyprland"
  local J; J=$(jq -nc --arg p "$d/proj" --arg c "$d/proj" '{id: "1 2", net: false, cwd: $c, roots: [{path: $p, masked: false}]}')
  check_match "jail: a build outside the jail's folders is refused" "not in this jail's project nor in a folder it was given whole" \
    "$(OMABOX_JAIL=$J "$CLI" up "$P-h2" --hyprland "$d/other/Hyprland" --size huge 2>&1)"
  check_match "jail: one in its project passes" "a size is WxH" "$(OMABOX_JAIL=$J "$CLI" up "$P-h2" --hyprland "$d/proj/build/Hyprland" --size huge 2>&1)"
  check_match "jail: one in a folder of its, whole, passes" "a size is WxH" "$(OMABOX_JAIL=$J "$CLI" up "$P-h2" --hyprland "$d/proj/Hyprland" --size huge 2>&1)"
  # The jailed caller's omabox sends the path absolute (the broker is not in its directory).
  printf '#!/bin/sh\nprintf "%%s|" "$@"\n' > "$d/relay"; chmod +x "$d/relay"
  out=$(cd "$d/proj" && bash -c 'source "$1"; RELAY=$2; relay_call up --hyprland build/Hyprland' _ "$TMP/lib/bin/omabox" "$d/relay")
  check_match "relay: --hyprland goes absolute" "\|up\|--hyprland\|$d/proj/build/Hyprland\|" "$out"
  # Another version than the installed hyprctl's: a warning (the box's answers stubbed).
  mkdir -p "$d/box"; echo '{}' > "$d/box/box.json"
  out=$(bash -c 'source "$1"; D=$2 NAME=x
    on_box() { if [ "${3:-}" = -j ]; then echo "{\"version\": \"0.0.1\"}"; else echo "Hyprland 0.0.1 built from branch x"; fi; }
    hypr_version' _ "$TMP/lib/bin/omabox" "$d/box" 2>&1)
  check_match "another Hyprland version than hyprctl's: a warning" "warning: box 'x' runs Hyprland 0.0.1, but hyprctl and hyprpm are the installed" "$out"
  check_eq "...and box.json has its version line" "Hyprland 0.0.1 built from branch x" "$(jq -r .hyprland_version "$d/box/box.json")"
  local left; left=$(ob ls --json | jq -r '.[].name' | grep -c "^$P-h[12]$" || true)
  check_eq "refusals left no box behind" 0 "$left"
}

# finding 116: a box on a Hyprland build of its own (a copy of the installed one stands in for it).
t_hyprland() {
  local B=$P-hyp h=$TMP/$P-hbuild/Hyprland D=$XDG_RUNTIME_DIR/omabox/$P-hyp
  mkdir -p "${h%/*}"; cp /usr/bin/Hyprland "$h"
  check "up --hyprland (a copy of the installed one), shell and bar up" ob up "$B" --hyprland "$h"
  check_eq "the box's Hyprland runs it" "$h" "$(ob run -b "$B" -- sh -c 'readlink /proc/$(pgrep -x Hyprland)/exe')"
  check_match "ls says so, under its line" "^  Hyprland: $h \(Hyprland [0-9.]+ built from" "$(ob ls | grep -A1 "^$B " | tail -n 1)"
  check_eq "ls --json has the binary" "$h" "$(ob ls --json | jq -r --arg b "$B" '.[] | select(.name == $b) | .hyprland')"
  check_match "windows says so first" "^box $B runs Hyprland $h \(Hyprland " "$(ob windows -b "$B" | head -n 1)"
  check "box.log has hyprctl version's first line, from inside" until_ok 10 grep -qF "session.sh: Hyprland $h: Hyprland " "$D/box.log"
  check "its folder is read-only in the box" bash -c "! '$CLI' run -b '$B' -- touch '${h%/*}/x' 2>/dev/null"
  check "restart-shell" ob restart-shell -b "$B"
  check "down" ob down "$B"
}

# finding 91: no global git identity (a fresh machine) is no reason for `up` to fail.
t_no_git_identity() {
  local B=$P-nogit
  check "up with no git identity" env GIT_CONFIG_GLOBAL=/dev/null "$CLI" up "$B" --no-shell
  check_fails "...and the box has none" ob run -b "$B" -- git config --global user.email
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
  # Nor a pasta.pid whose pid is now another process named pasta: pasta never removes its pid file,
  # and a name alone proves nothing (finding 89): box_pasta also wants the command line to name the
  # pid file.
  cp /usr/bin/sleep "$TMP/pasta"
  "$TMP/pasta" 300 & local fake=$!
  mkdir -p "$D"
  echo '{"child-pid": 2}' > "$D/info.json"
  echo '{"name": "x", "mode": "headless", "net": "connected"}' > "$D/box.json"
  echo "$fake" > "$D/pasta.pid"
  check_eq "a stale pasta.pid reads dead" dead "$(ob ls --json | jq -r ".[] | select(.name == \"$B\") | .state")"
  ob down "$B" >/dev/null 2>&1
  check "down did not kill another process named pasta" kill -0 "$fake"
  kill "$fake" 2>/dev/null
  # Nor one whose command line names the pid file as pasta's does, but which is not pasta (its comm).
  bash -c 'sleep 300 & wait' decoy -P "$D/pasta.pid" x & local decoy=$!
  until_ok 5 grep -qF decoy "/proc/$decoy/cmdline"
  mkdir -p "$D"
  echo '{"child-pid": 2}' > "$D/info.json"
  echo '{"name": "x", "mode": "headless", "net": "connected"}' > "$D/box.json"
  echo "$decoy" > "$D/pasta.pid"
  ob down "$B" >/dev/null 2>&1
  check "down did not kill a process that only names the pid file" kill -0 "$decoy"
  pkill -P "$decoy"; kill "$decoy" 2>/dev/null
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
  held "$P-exp" check_match "after expiry run reports it (no throwaway)" "went down after 10s idle" "$(cd "$repo" && "$CLI" run -- true 2>&1)"
  ob down "$P-exp" >/dev/null 2>&1
}

# A throwaway run from ~ (no repo) does not put HOME in the box (finding 63).
t_throwaway_home() {
  # From ~ (no repo) the throwaway is named after "default": a box of the user's with that name
  # would take the run instead.
  # An expiry note from a 'default' box that idled out makes `run` refuse the same way.
  if [ -e "$XDG_RUNTIME_DIR/omabox/default" ] || [ -e "$XDG_RUNTIME_DIR/omabox/.expired-default" ]; then
    no "a box named 'default' is up or idled out: run it again after omabox down default"; return
  fi
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
  # keys --pass (finding 84): the caller's variable is typed, and no process's argv has it meanwhile.
  ob run -b "$B" -d -- foot sh -c 'cat > /tmp/secret' >/dev/null
  until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '[.[] | select(.class == \"foot\")] | length == 1'"
  local seen=""
  T_PW="pw-$P x=y Ü" "$CLI" keys -b "$B" -t a --pass T_PW -s 1500 -t -T --pass T_PW Return ctrl+d & local kp=$!
  if until_ok 10 pgrep -f 'omabox-keyboard .* -T -s 1500'; then
    seen=$(grep -l "pw-$P" /proc/[0-9]*/cmdline 2>/dev/null)
    check_eq "keys --pass: the value is in no /proc/*/cmdline while typing" "" "$seen"
  else no "keys --pass: the keyboard ran (to scan while typing)"; fi
  wait "$kp"; check_eq "keys --pass exits 0" 0 $?
  until_ok 5 ob run -b "$B" -- test -s /tmp/secret
  check_eq "keys --pass types the value, in order with -t (and -t -T is text)" "apw-$P x=y Ü-Tpw-$P x=y Ü" "$(ob run -b "$B" -- cat /tmp/secret)"
  check_match "keys --pass: an unset variable refused" "not in this command's environment" "$(env -u T_NONE "$CLI" keys -b "$B" --pass T_NONE 2>&1)"
  check_match "keys --pass: a bad name refused" "takes a variable name" "$(ob keys -b "$B" --pass 'a b' 2>&1)"
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
  wait "$k"; check_eq "keys exits 1 when the box goes mid-run" 1 $?
  wait "$p"; check_eq "pointer exits 1 when the box goes mid-run" 1 $?
}

# peek, run inside a box on that box's own screen (never on the host): it draws, and hidden on another
# workspace it stops capturing (finding 64; the old one scaled 30 frames a second nobody saw).
# Marks (finding 85): what `click`/`keys` did, drawn by that peek over its view. The CLI writes them
# only for a host peek of the box, so a stand-in named like one relays them to the peek in the box.
t_peek() {
  local B=$P-peek
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  local D; D=$(ob path "$B")
  local wd; wd=$(ob run -b "$B" -- sh -c 'echo $WAYLAND_DISPLAY')
  ob run -b "$B" -d -- "$ROOT/tools/peek/omabox-peek" --box "/run/user/$UID/$wd" --fps 30 --marks "/run/user/$UID/marks-in" >/dev/null 2>&1
  check "peek opens a window" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\")'"
  local pid; pid=$(ob hyprctl -b "$B" -j clients | jq '.[] | select(.class == "omabox-peek") | .pid')
  ticks() { ob run -b "$B" -- sh -c "a=\$(cut -d' ' -f14,15 /proc/$pid/stat | tr ' ' +); sleep 2; b=\$(cut -d' ' -f14,15 /proc/$pid/stat | tr ' ' +); echo \$(( (b) - (a) ))"; }
  local shown; shown=$(ticks)
  check "peek draws while shown (CPU ticks: $shown)" test "$shown" -gt 5
  # A quarter of the screen, bottom right: a box point X,Y is at 940 + X/2, 520 + Y/2 in the view, and
  # the view's copy of itself (it shows its own screen) lies well away from any mark.
  ob hyprctl -b "$B" dispatch "hl.dsp.window.float({ window = 'class:omabox-peek' })" >/dev/null
  ob hyprctl -b "$B" dispatch "hl.dsp.window.resize({ window = 'class:omabox-peek', x = 960, y = 540 })" >/dev/null
  ob hyprctl -b "$B" dispatch "hl.dsp.window.move({ window = 'class:omabox-peek', x = 940, y = 520 })" >/dev/null
  check "peek floats at 940,520 960x540" until_ok 5 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\") | .at == [940, 520] and .size == [960, 540]'"
  sleep 0.5; local base; base=$(ticks)   # at this size, before any mark
  # Accent pixels (#ff3cc8) in the box's screen within X0 Y0 X1 Y1: "COUNT CX CY", the centre of their
  # bounding box.
  # shellcheck disable=SC2329 # called through accent_ok and no_accent
  accent() {
    ob run -b "$B" -- grim -t ppm - > "$TMP/peek.ppm"
    python3 - "$TMP/peek.ppm" "$@" <<'PY'
import re, sys
d = open(sys.argv[1], "rb").read()
m = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", d)
w, px = int(m[1]), d[m.end():]
x0, y0, x1, y1 = map(int, sys.argv[2:])
hits = [(x, y) for y in range(y0, y1) for x in range(x0, x1)
        if px[(y * w + x) * 3] > 200 and px[(y * w + x) * 3 + 1] < 110 and px[(y * w + x) * 3 + 2] > 150]
if not hits: print(0, 0, 0)
else: print(len(hits), (min(x for x, _ in hits) + max(x for x, _ in hits)) // 2, (min(y for _, y in hits) + max(y for _, y in hits)) // 2)
PY
  }
  # shellcheck disable=SC2329 # called through until_ok and holds
  # For until_ok: at least MIN accent pixels there (centred within 8 px of CX,CY when given); prints them.
  accent_ok() {
    local n=$1 a; shift; read -ra a <<< "$(accent "$1" "$2" "$3" "$4")"; echo "${a[*]}"
    [ "${a[0]}" -ge "$n" ] || return 1
    [ $# -lt 6 ] || (( a[1] - $5 >= -8 && a[1] - $5 <= 8 && a[2] - $6 >= -8 && a[2] - $6 <= 8 ))
  }
  # shellcheck disable=SC2329
  no_accent() { local a; read -ra a <<< "$(accent "$@")"; echo "${a[*]}"; [ "${a[0]}" = 0 ]; }
  HELD=$B   # (a peek of yours at this box would get these marks too: held)
  ob click -b "$B" 100 100 >/dev/null
  check "no marks file without a peek window" test ! -e "$D/marks"
  # The stand-in: a process named as `omabox peek` runs it, relaying $D/marks into the box.
  (exec -a "$ROOT/tools/peek/omabox-peek --box $D/run/stand-in" tail -n +1 -F "$D/marks" >> "$D/run/marks-in" 2>/dev/null) & local relay=$!
  until_ok 5 pgrep -f "^$ROOT/tools/peek/omabox-peek --box $D/run/stand-in"
  ob click -b "$B" 700 400 >/dev/null
  check_eq "click writes a mark" "ptr 1920x1080 move 700 400 click left" "$(cat "$D/marks" 2>&1)"
  check_eq "the marks file is the user's only" 600 "$(stat -c %a "$D/marks")"
  until_ok 5 grep -q "move 700 400" "$D/run/marks-in"
  check "a ring where the click went" until_ok 3 accent_ok 21 1230 660 1350 780 1290 720
  T_PW="pw-$P" "$CLI" keys -b "$B" -t hi --pass T_PW >/dev/null
  until_ok 5 grep -q "^secret" "$D/run/marks-in"
  check_eq "keys marks: text, and a secret as its length only" "text hi|secret $((3 + ${#P}))|" "$(grep -v ^ptr "$D/marks" | tr '\n' '|')"
  check_fails "...never its value" grep -q "pw-$P" "$D/marks"
  check "a key caption at the bottom" until_ok 3 accent_ok 51 1100 990 1740 1060
  { head -c 70000 /dev/zero | tr '\0' x; echo; } >> "$D/marks"
  ob click -b "$B" 700 400 >/dev/null
  check_eq "past 64 KB the marks file starts over" "ptr 1920x1080 move 700 400 click left" "$(cat "$D/marks")"
  sleep 3.5
  check "marks gone ~3 s after the last one" no_accent 940 520 1900 1060
  # Junk is ignored whole (and does not stop what follows): unknown kinds, out-of-range and bad
  # numbers, control characters, a line too long to be one, then a good line.
  printf '%s\n' 'junk' 'ptr 1920x1080 move 1920 10' 'ptr 1920x1080 move 10' 'ptr 0x1080 click' 'ptr 1920x1080 hover 1 1' \
    'combo a b' 'secret -1' 'text' "text $(printf 'x%.0s' {1..201})" "$(printf 'y%.0s' {1..2000})" $'text a\tb' >> "$D/run/marks-in"
  check "junk marks draw nothing" holds 1 no_accent 940 520 1900 1060
  printf 'ptr 1920x1080 move 300 200 click\n' >> "$D/run/marks-in"
  check "...and a good line after it still does" until_ok 3 accent_ok 21 1030 560 1150 680   # around 1090,620
  check "peek is still running" kill -0 "$(pgrep -f "omabox-peek --box /run/user/$UID/$wd" | head -1)"
  kill "$relay" 2>/dev/null; wait "$relay" 2>/dev/null
  ob click -b "$B" 10 10 >/dev/null
  check "the marks file goes with the peek window" test ! -e "$D/marks"
  HELD=""
  sleep 3.2
  local after; after=$(ticks)
  check "marks cost nothing once gone (CPU ticks: $after, before any: $base)" test "$after" -le $((base + base / 2 + 3))
  ob hyprctl -b "$B" dispatch "hl.dsp.window.move({ workspace = '5', follow = false })" >/dev/null; sleep 0.5
  printf 'combo super+space\nptr 1920x1080 move 5 5 click\n' >> "$D/run/marks-in"   # nothing to show them on
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
  # The box's PID 1 as the CLI finds it: behind pasta, info.json's child-pid is pasta's numbering (2,
  # which on the host is kthreadd), and the box would stay up for the reaper's "run is gone" branch.
  local pid; pid=$(D=$XDG_RUNTIME_DIR/omabox/$n lib box_pid)
  kill -KILL "$r"
  check "its box is killed before the reaper sees it" kill -KILL "$pid"
  check "the dead box goes within 15 s" until_ok 15 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$n\")'"
}

# install.sh with a temporary HOME and stubs first on PATH (sudo refuses, so it never installs a
# package): failures stop it with a message (finding 64), and it links, never nests.
t_unit_install() {
  # install.sh is a checkout's (it builds the tools into ROOT): a package builds them itself.
  [ "$INSTALLED" = 0 ] || { skip "install.sh" "an installed omabox has no install.sh to run"; return; }
  local h=$TMP/ih stub=$TMP/ih-stub out
  # The XDG dirs too: the session sets them, and install.sh's `setup --aquamarine` builds under the
  # data dir when ROOT is no checkout (it once did so in the user's own, from an --installed run).
  local -x XDG_DATA_HOME=$h/.local/share XDG_CACHE_HOME=$h/.cache XDG_CONFIG_HOME=$h/.config XDG_STATE_HOME=$h/.local/state
  mkdir -p "$h" "$stub"
  printf '#!/bin/sh\nexit 1\n' > "$stub/sudo"
  printf '#!/bin/sh\ncase "$*" in *tools/keyboard*) exit 1 ;; esac\nexec /usr/bin/make "$@"\n' > "$stub/make"
  # Never the user's shell: setup's widget question (finding 133) would reach the running one, which
  # is the real desktop's whatever HOME says. Answered no up front, and these fail if called.
  printf '#!/bin/sh\necho "$0 $*" >> "%s/shell-called"; exit 1\n' "$stub" > "$stub/omarchy"
  cp "$stub/omarchy" "$stub/omarchy-shell"
  mkdir -p "$h/.config/omabox"; date -Is > "$h/.config/omabox/widget-declined"
  chmod +x "$stub/sudo" "$stub/make" "$stub/omarchy" "$stub/omarchy-shell"
  out=$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1); local rc=$?
  check_match "a failed tool build stops install.sh" "building tools/keyboard failed" "$out"
  check_eq "...with a failure" 1 "$rc"
  rm "$stub/make"
  mkdir -p "$h/.claude/skills/omabox"
  check_match "a real dir where a link goes is refused" "is not a link" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
  rmdir "$h/.claude/skills/omabox"
  check "install.sh in a clean HOME" env HOME="$h" PATH="$stub:$PATH" "$ROOT/install.sh"
  check_eq "the skill is a link" "$ROOT/skill" "$(readlink "$h/.claude/skills/omabox")"
  check "...of the directory: the reference SKILL.md points to comes with it, for every agent" \
    bash -c "grep -q 'reference.md' '$h/.agents/skills/omabox/SKILL.md' && test -f '$h/.agents/skills/omabox/reference.md' && test -f '$h/.claude/skills/omabox/reference.md'"
  check_fails "no dir for an agent that is not installed" test -e "$h/.codex"
  check_fails "the agent guard is never turned on without asking" test -e "$h/.claude/settings.json"
  printf '#!/bin/sh\necho "  -g <geometry>   Set the region to capture."\n' > "$stub/grim"; chmod +x "$stub/grim"
  check_match "a grim with no -T (window capture, finding 81) stops it" "no -T" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
  rm "$stub/grim"
  # finding 92: an older hook of ours is an update to offer, even after a "no" to turning it on.
  jq -n '{hooks: {SessionStart: [{hooks: [{type: "command", command: "echo old omabox-guard"}]}]}}' > "$h/.claude/settings.json"
  date -Is > "$h/.config/omabox/guard-declined"
  out=$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)
  check_match "an outdated guard: install.sh offers to update it" "guard there is outdated.*not asked \(no terminal\)" "$(tr '\n' ' ' <<<"$out")"
  check_eq "...and changes nothing without a terminal" "echo old omabox-guard" "$(jq -r '.hooks.SessionStart[0].hooks[0].command' "$h/.claude/settings.json")"
  # One from a checkout that is gone reads outdated too, though no older omabox wrote it.
  local old; old=$(cat "$h/.claude/settings.json")
  jq --arg c 'echo x omabox-guard PATH="/nonexistent/co/share/guard:$PATH"' '.hooks.SessionStart[0].hooks[0].command = $c' \
    <<<"$old" > "$h/.claude/settings.json"
  check_match "...and one from a checkout that is gone, in words that fit it too" \
    "/nonexistent/co/share/guard is gone.*The guard there is outdated.*not asked \(no terminal\): omabox guard on claude" \
    "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1 | tr '\n' ' ')"
  printf '%s\n' "$old" > "$h/.claude/settings.json"
  # ...for the agents that have it, whatever the others' guard (Codex off here) and an earlier "no".
  # "Turn it on?" is for the others, and only a "no" to that is remembered.
  local guard=(env HOME="$h" "$CLI" guard) tty=(env SHELL=/bin/bash script -qec "$(printf %q "$ROOT/install.sh")" /dev/null)
  mkdir -p "$h/.codex"; rm "$h/.config/omabox/guard-declined"
  out=$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)
  check_match "Claude Code outdated, Codex off: an update for Claude Code, turning it on for Codex" \
    "guard there is outdated.*not asked \(no terminal\): omabox guard on claude .*not asked \(no terminal\): omabox guard on codex" "$(tr '\n' ' ' <<<"$out")"
  date -Is > "$h/.config/omabox/guard-declined"
  out=$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)
  check_match "...the update even after a no to turning it on" \
    "guard there is outdated.*not asked \(no terminal\): omabox guard on claude .*you said no before.*: omabox guard on codex" "$(tr '\n' ' ' <<<"$out")"
  printf 'y\n' | HOME=$h PATH=$stub:$PATH "${tty[@]}" >/dev/null 2>&1
  check_match "...in a terminal, yes updates Claude Code's and leaves Codex off" "Claude Code .*: on Codex .*: off" \
    "$("${guard[@]}" | grep -E '^(Claude Code|Codex)' | tr '\n' ' ')"
  rm "$h/.config/omabox/guard-declined"; printf '%s\n' "$old" > "$h/.claude/settings.json"
  printf 'n\ny\n' | HOME=$h PATH=$stub:$PATH "${tty[@]}" >/dev/null 2>&1
  check_match "...no to the update and yes to turning it on: only Codex's changes" "Claude Code .*: outdated Codex .*: on" \
    "$("${guard[@]}" | grep -E '^(Claude Code|Codex)' | tr '\n' ' ')"
  check_fails "...and the no to the update is not remembered" test -e "$h/.config/omabox/guard-declined"
  rm -rf "$h/.codex"
  rm -f "$h/.claude/settings.json" "$h/.config/omabox/guard-declined"
  printf '#!/bin/sh\necho Hyprland dev build\n' > "$stub/Hyprland"; chmod +x "$stub/Hyprland"
  check_match "an unreadable Hyprland version says so (was silent)" "too old" "$(HOME=$h PATH=$stub:$PATH "$ROOT/install.sh" 2>&1)"
  check_fails "...and none of it reached a shell (omarchy, omarchy-shell)" test -e "$stub/shell-called"
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
  # systemd-run and systemd-cat stand-ins (finding 131): the browser bind's shape, with foot for the browser.
  ob run -b "$B" -- systemd-run --user --quiet --collect --unit=omarchy-browser-1 --property=StandardOutput=null \
    uwsm-app -- foot --app-id=omabox.sdrun sleep 60 >/dev/null
  check "systemd-run --user without a user manager starts the app (the browser bind)" until_ok 10 \
    bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox.sdrun\")'"
  check_eq "...--wait runs it in the foreground, its exit code passed on" "out 3" \
    "$(ob run -b "$B" -- sh -c 'o=$(systemd-run --user --wait -E V=out sh -c "echo \$V; exit 3"); echo "$o $?"')"
  check_match "...a timer says it needs --systemd" "omabox up --systemd" "$(ob run -b "$B" -- systemd-run --user --on-active=1m true 2>&1)"
  check_eq "systemd-cat -t ID writes to ~/ID.log" "hi" "$(ob run -b "$B" -- sh -c 'echo hi | systemd-cat -t omabox-t; cat ~/omabox-t.log')"
  # omarchy-version (finding 137): Omarchy's asks pacman, which a box has no database for (exit 1).
  local ov; ov=$(pacman -Q omarchy-dev 2>/dev/null || pacman -Q omarchy 2>/dev/null) || ov=${OMABOX_OMARCHY_VERSION:-}
  check_eq "omarchy-version says the installed Omarchy's version" "${ov#* }" "$(ob run -b "$B" -- omarchy-version)"
  check_eq "...in a terminal's bash too (Omarchy's bin is on its PATH)" "${ov#* }" "$(ob run -b "$B" -- bash -ic omarchy-version 2>/dev/null)"
  check_eq "...and ls --json has it (finding 139)" "${ov#* }" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy_version')"
  ob down "$B" >/dev/null
}

# `omarchy restart shell` (Omarchy's own, finding 131) in a box: its launcher runs the bar under
# systemd-cat, which a box lacked, and the bar was gone. It and `omabox restart-shell`, in turns, leave
# one shell each time.
t_omarchy_restart() {
  local B=$P-or
  ob up "$B" --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  # shellcheck disable=SC2329 # called through check
  one_shell() { [ "$(ob run -b "$B" -- pgrep -cx quickshell)" = 1 ] && ob run -b "$B" -- omarchy-shell shell ping >/dev/null 2>&1; }
  check "omarchy-restart-shell succeeds in a box" timeout 30 "$CLI" run -b "$B" -- omarchy-restart-shell
  check "...one shell, answering" until_ok 5 one_shell
  check_eq "...its pid where omabox restart-shell looks" "$(ob run -b "$B" -- pgrep -x quickshell)" "$(ob run -b "$B" -- sh -c 'cat "$XDG_RUNTIME_DIR/omabox-shell.pid"')"
  check "omabox restart-shell after it" timeout 30 "$CLI" restart-shell -b "$B"
  check "...one shell, answering, Omarchy's launcher gone" until_ok 5 one_shell
  check_fails "...no omarchy-launch-shell left to start another" ob run -b "$B" -- pgrep -f '[o]marchy-launch-shell'
  check "omarchy-restart-shell again" timeout 30 "$CLI" run -b "$B" -- omarchy-restart-shell
  check "...one shell" until_ok 5 one_shell
  ob down "$B" >/dev/null
}

# The bar widget (plugin/) in a box's own bar, against a stand-in omabox (a list from a file, actions
# logged) and an xdg-open that blocks like an image viewer left open (finding 64).
t_widget() {
  local B=$P-wg
  ob up "$B" --net isolated --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up" "failed"; return; }
  local H; H=$(ob path "$B")/home
  printf '#!/bin/sh\ncase "$1" in\n  ls) echo ls >> "$HOME/polls"; cat "$HOME/list.json" ;;\n  shot) echo "$*" >> "$HOME/actions"; echo "$HOME/x.png" ;;\n  config) echo "$*" >> "$HOME/actions"; cat "$HOME/config.json" 2>/dev/null || echo "{}" ;;\n  up) echo "$*" >> "$HOME/actions"; echo box-9 ;;\n  clip) echo "$*" >> "$HOME/actions"; echo "omabox: handed text (text/plain;charset=utf-8, 3 bytes) from your clipboard to box ia" >&2 ;;\n  *) echo "$*" >> "$HOME/actions" ;;\nesac\n' > "$H/.local/bin/omabox"
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
  # The file itself, not <(...): a process substitution is read once, so a retry of until_ok saw an
  # empty pipe and the check failed whenever the viewer started after the first poll.
  check "the viewer is started" until_ok 3 grep -qx "xdg-open /home/sbx/x.png" "$H/actions"
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
  # clip (issue #23, finding 119): v pastes in, c copies out, on an interactive box's row only
  local irow='{"mode":"interactive","size":"window","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"connected","state":"up","peeking":false}'
  jq -n "[$irow + {name: \"ia\"}]" > "$H/list.json"; polled
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down v >/dev/null
  check "v on an interactive box: clip -b NAME" until_ok 3 grep -qx "clip -b ia" "$H/actions"
  check "...what it handed over is notified (type and size)" until_ok 3 grep -q "^notify-send -a omabox omabox clip ia handed text (text/plain" "$H/actions"
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down c >/dev/null
  check "c: clip --from-box" until_ok 3 grep -qx "clip -b ia --from-box" "$H/actions"
  jq -n "[$row + {name: \"hb\"}]" > "$H/list.json"; polled
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down v c >/dev/null
  check "...neither on a headless box's" holds 1 bash -c "! grep -q 'clip -b hb' '$H/actions'"
  ob keys -b "$B" Escape >/dev/null
  # keys-to-box (finding 117): f turns it on or off for the selected interactive box; the icon is lit
  # while the host's Lua names a box in $XDG_RUNTIME_DIR/omabox/.keys (renamed into place, as it does).
  local irow='{"mode":"interactive","size":"window","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  jq -n "[$irow + {name: \"i\", keys_to_box: false}, $irow + {name: \"j\", keys_to_box: true}]" > "$H/list.json"; polled
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down f >/dev/null
  check "f turns keys-to-box on for an interactive box" until_ok 3 grep -qx "keys-to-box -b i on" "$H/actions"
  ob keys -b "$B" Down f >/dev/null
  check "...and off for one that has it" until_ok 3 grep -qx "keys-to-box -b j off" "$H/actions"
  ob keys -b "$B" Escape >/dev/null
  # The bar's right side (the widget's section) as it is now, the same as FILE or not.
  # shellcheck disable=SC2329 # called through until_ok
  bar_is() { ob shot -b "$B" -g "1420,0 500x30" -o "$TMP/keys-now.png" >/dev/null 2>&1 && if [ "$1" = same ]; then cmp -s "$2" "$TMP/keys-now.png"; else ! cmp -s "$2" "$TMP/keys-now.png"; fi; }
  local run; run=$(ob path "$B")/run
  # No omabox dir when the shell started: the file is not watched then, and each poll reads it.
  mkdir -p "$run/omabox"
  ob wait -b "$B" still >/dev/null 2>&1
  ob shot -b "$B" -g "1420,0 500x30" -o "$TMP/keys-off.png" >/dev/null 2>&1
  echo i > "$run/omabox/.keys.tmp" && mv "$run/omabox/.keys.tmp" "$run/omabox/.keys"
  check "the icon is lit while keys go to a box (read at a poll)" until_ok 8 bar_is other "$TMP/keys-off.png"
  cp "$TMP/keys-now.png" "$TMP/keys-on.png"
  echo > "$run/omabox/.keys.tmp" && mv "$run/omabox/.keys.tmp" "$run/omabox/.keys"
  check "...and not once they are the desktop's again (watched)" until_ok 1.5 bar_is same "$TMP/keys-off.png"
  echo i > "$run/omabox/.keys.tmp" && mv "$run/omabox/.keys.tmp" "$run/omabox/.keys"
  check "...lit again at once (the file watched)" until_ok 1.5 bar_is same "$TMP/keys-on.png"
  echo gone > "$run/omabox/.keys.tmp" && mv "$run/omabox/.keys.tmp" "$run/omabox/.keys"
  check "...not for a box that is not up (a file a Hyprland left)" until_ok 1.5 bar_is same "$TMP/keys-off.png"
  # A widget older than the CLI (finding 133: the shell does not reload it after an upgrade) says to
  # restart the shell; one of the CLI's version, or a CLI that names none, says nothing.
  echo '[]' > "$H/list.json"; polled
  reads=$(grep -c "^config --json$" "$H/actions")
  echo '{}' > "$H/config.json"; : > "$H/.config/omabox/config"   # bar-icon always again: it opens with none
  until_ok 5 lines_over "$H/actions" "$reads" "^config --json$"; sleep 0.3
  local pv; pv=$(sed -n 's/.*pluginVersion: "\([^"]*\)".*/\1/p' "$ROOT/plugin/Panel.qml")
  # shellcheck disable=SC2329 # called through check
  panel_shot() {
    echo "$1" > "$H/config.json"
    local n; n=$(grep -c "^config --json$" "$H/actions")
    ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
    until_ok 3 lines_over "$H/actions" "$n" "^config --json$"; sleep 0.3   # read when opened, taken in
    ob pointer -b "$B" -- move 10 1000 >/dev/null; ob wait -b "$B" still >/dev/null 2>&1
    ob shot -b "$B" -g "1220,0 700x400" -o "$2" >/dev/null 2>&1
    ob keys -b "$B" Escape >/dev/null; until_ok 3 bash -c "! '$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-keyboard-panel"
  }
  panel_shot '{}' "$TMP/wg-nover.png"
  panel_shot "{\"version\":\"$pv\"}" "$TMP/wg-same.png"
  panel_shot '{"version":"9.9.9"}' "$TMP/wg-newer.png"
  check "a widget of the CLI's version says nothing more than one told no version" cmp -s "$TMP/wg-nover.png" "$TMP/wg-same.png"
  check_fails "...an older one says to restart the shell (finding 133)" cmp -s "$TMP/wg-same.png" "$TMP/wg-newer.png"
  # (Not with omabox installed on the host: the box sees its /usr/bin/omabox, so there is always one.)
  if [ ! -e /usr/bin/omabox ]; then
    mv "$H/.local/bin/omabox" "$H/.local/bin/omabox.off"
    check "a list command that cannot run is notified" until_ok 20 grep -q "notify-send .*cannot run omabox" "$H/actions"
  else
    skip "a list command that cannot run is notified" "the box has the host's /usr/bin/omabox"
  fi
  ob down "$B" >/dev/null
}

# The agent guard (finding 65): the fake display agents' shells get, and omabox still finding the
# user's session from such a shell. Sessions are faked in a runtime dir of our own where it matters.
GUARDED=(env WAYLAND_DISPLAY=omabox-guard HYPRLAND_INSTANCE_SIGNATURE=omabox-guard DISPLAY= QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1
  BROWSER="$ROOT/share/guard/xdg-open" GH_BROWSER="$ROOT/share/guard/xdg-open" PATH="$ROOT/share/guard:$PATH")
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
  jq --arg c 'echo x omabox-guard PATH="/nonexistent/co/share/guard:$PATH"' '.hooks.SessionStart[-1].hooks[0].command = $c' "$s" > "$s.new" && mv "$s.new" "$s"
  check_match "...and one from a checkout that is gone says so (finding 92)" "outdated: its /nonexistent/co/share/guard is gone" "$(g | head -1)"
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
  # The hooks of an omabox at $co (a copy of the CLI, as a checkout or package would have it).
  local co=$TMP/guard-co hook chook f=$TMP/envfile
  mkdir -p "$co/bin"; cp "$TMP/lib/bin/omabox" "$co/bin/omabox"; chmod +x "$co/bin/omabox"
  hook=$(bash -c 'source "$1"; printf %s "$GUARD_HOOK"' _ "$co/bin/omabox")
  chook=$(bash -c 'source "$1"; printf %s "$GUARD_CODEX_HOOK"' _ "$co/bin/omabox")
  check_match "the hook says so when it cannot apply the guard (finding 74)" "NOT applied" "$(env -u CLAUDE_ENV_FILE sh -c "$hook")"
  check_eq "...and applies it when it can" 1 "$(CLAUDE_ENV_FILE=$f sh -c "$hook" >/dev/null; grep -c 'WAYLAND_DISPLAY=omabox-guard' "$f")"
  check_eq "...with the guard's xdg-open first on PATH (finding 92)" "$co/share/guard" \
    "$(bash -c '. "$1"; echo "${PATH%%:*}"' _ "$f")"
  check_match "Codex's hook gives the note" "^omabox guard: shell commands here have no display" "$(sh -c "$chook")"
  # Issue #51: omabox deleted without `guard off` (a checkout removed, a package uninstalled).
  rm -rf "$co" "$f"
  check_match "omabox gone: the hook says so, in one line" "^omabox guard: omabox is gone \($co/bin/omabox\), so the guard is not applied" "$(CLAUDE_ENV_FILE=$f sh -c "$hook")"
  check_eq "...one line" 1 "$(CLAUDE_ENV_FILE=$f sh -c "$hook" | wc -l)"
  check_fails "...and leaves the session's environment untouched" test -e "$f"
  check_match "...Codex's says how to take its guard out" "omabox is gone .*Codex still applies its guard.*# >>> omabox guard" "$(sh -c "$chook")"
  check_fails "guard junk refused" g maybe
  check_fails "guard on for an unknown agent refused" g on vim
  # Codex (finding 67): a marked block in config.toml, checked as TOML; only when Codex is installed.
  local c=$h/.codex/config.toml
  check_fails "no Codex dir: Codex not listed" grep -q Codex <<<"$(g)"
  mkdir -p "$h/.codex"; printf 'model = "x"\n\n[features]\nhooks = true\n' > "$c"; orig=$(cat "$c"; echo .)
  local hj=$h/.codex/hooks.json
  jq -n '{hooks: {SessionStart: [{hooks: [{type: "command", command: "herdr.sh session", timeout: 10}]}]}}' > "$hj"
  out=$(g on codex)
  check_eq "Codex: on sets the variables for its commands" omabox-guard \
    "$(python3 -c 'import tomllib, sys; print(tomllib.load(open(sys.argv[1], "rb"))["shell_environment_policy"]["set"]["WAYLAND_DISPLAY"])' "$c")"
  check_eq "...adds its hook (issue #51), keeping the others" "herdr.sh session|omabox" \
    "$(jq -r '[.hooks.SessionStart[].hooks[].command | if contains("omabox-guard") then "omabox" else . end] | join("|")' "$hj")"
  check_match "...says Codex asks you to trust it" "trust its new hook once: /hooks" "$out"
  check_match "...and reads on" "Codex .*: on$" "$(g | grep '^Codex')"
  check_fails "...on again: no trust to ask for" grep -q trust <<<"$(g on codex)"
  jq '.hooks.SessionStart |= map(select(any(.hooks[]; .command | contains("omabox-guard")) | not))' "$hj" > "$hj.new" && mv "$hj.new" "$hj"
  check_match "Codex: the block without its hook (an install from before #51) reads outdated" "Codex .*: outdated \(its hook in .*hooks.json: off\)" "$(g | grep '^Codex')"
  g on codex >/dev/null
  check_match "...and on adds it" "Codex .*: on$" "$(g | grep '^Codex')"
  echo '{"hooks":' > "$hj"
  check_match "Codex: a hooks.json that is not JSON is refused" "hooks.json is not a JSON object" "$(g off codex >/dev/null; g on codex)"
  check_fails "...before anything is written" grep -q omabox "$c"
  echo '{}' > "$hj"; g on codex >/dev/null
  check_match "a broken Claude Code file is reported, not fatal for the rest" "claude: .*not a JSON object" "$(g | grep claude)"
  rm "$s"
  check_match "on codex leaves Claude Code alone" "Claude Code .*: off$" "$(g | grep '^Claude')"
  g off codex >/dev/null
  check_eq "Codex: off gives back the same file (trailing newline too)" "$(printf 'model = "x"\n\n[features]\nhooks = true\n.')" "$(cat "$c"; echo .)"
  check_eq "...and takes its hook out" '{}' "$(jq -c . "$hj")"
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
  printf '%s\n' "# >>> omabox guard (\`omabox guard off\` removes this block)" "[shell_environment_policy.set]" \
    'WAYLAND_DISPLAY = "omabox-guard"' 'BROWSER = "/nonexistent/co/share/guard/xdg-open"' "# <<< omabox guard" > "$c"
  check_match "Codex: a block from a checkout that is gone says so (finding 92)" "outdated: its /nonexistent/co/share/guard is gone" "$(g | grep '^Codex')"
  printf 'x = \n' > "$c"
  check_match "Codex: a file that is not TOML is refused" "not valid TOML" "$(g on codex)"
  check_eq "...untouched" 'x = ' "$(cat "$c")"
}

# guard exec (any agent, whole) and host (one command on the real desktop, from a guarded shell).
t_unit_guard_exec_host() {
  check_eq "guard exec: the guard's display" omabox-guard "$("$CLI" guard exec -- sh -c 'echo $WAYLAND_DISPLAY')"
  check_eq "guard exec: a core limit of 1 byte (no crash notification)" 1 "$("$CLI" guard exec -- sh -c 'prlimit --pid $$ --core -o SOFT --noheadings | tr -d " "')"
  # finding 92: no links or files opened on the desktop, however they are asked for. xdg-open runs
  # only when `command -v` under guard exec gives the guard's: were a dir with the real one put ahead
  # of it, xdg-open would open a tab on the desktop. A stub right after the guard's on PATH is a
  # second net, should the lookup that runs xdg-open ever differ from `command -v`'s: the checks
  # then reach the stub and fail.
  local gx=(env PATH="$TMP/fakeopen:$PATH" "$CLI" guard exec --) xo
  mkdir -p "$TMP/fakeopen"
  printf '#!/bin/sh\necho "real xdg-open reached"\n' > "$TMP/fakeopen/xdg-open"; chmod +x "$TMP/fakeopen/xdg-open"
  xo=$("${gx[@]}" sh -c 'command -v xdg-open')
  check_eq "guard exec: xdg-open is the guard's" "$ROOT/share/guard/xdg-open" "$xo"
  if [ "$xo" = "$ROOT/share/guard/xdg-open" ]; then
    check_match "...which refuses" "omabox guard: not opening https://example.invalid" \
      "$("${gx[@]}" xdg-open https://example.invalid 2>&1)"
    check_eq "...with exit 4, as xdg-open for a failed action" 4 "$("${gx[@]}" xdg-open https://example.invalid >/dev/null 2>&1; echo $?)"
  else
    no "...which refuses, with exit 4" "not run: xdg-open under guard exec is [$xo], not the guard's"
  fi
  check_eq "...and BROWSER, GH_BROWSER name it" "$ROOT/share/guard/xdg-open $ROOT/share/guard/xdg-open" \
    "$("$CLI" guard exec -- sh -c 'echo $BROWSER $GH_BROWSER')"
  # The session omabox finds, not $HYPRLAND_INSTANCE_SIGNATURE: under the guard (an agent running the
  # suite) that is the guard's.
  local sig want; sig=$(bash -c 'source "$1"; host_session; echo "$HOST_SIG"' _ "$TMP/lib/bin/omabox")
  want=$(hyprctl -j instances | jq -r --arg s "$sig" '.[] | select(.instance == $s) | .wl_socket')
  check_eq "host from a guarded shell: the real Wayland display" "$want" "$("${GUARDED[@]}" "$CLI" host -- sh -c 'echo $WAYLAND_DISPLAY' 2>/dev/null)"
  check "host: and hyprctl reaches it (read-only)" "${GUARDED[@]}" "$CLI" host -- hyprctl -j version
  local open; open=$("${GUARDED[@]}" "$CLI" host -- sh -c 'command -v xdg-open; echo "${BROWSER-unset}"' 2>/dev/null)
  check_match "host: the real xdg-open (finding 92)" '^/' "$(head -1 <<<"$open")"
  check_fails "...not the guard's" grep -q share/guard <<<"$open"
  check_fails "...nor another checkout's" grep -q elsewhere <<<"$("${GUARDED[@]}" PATH="/elsewhere/share/guard:$PATH" BROWSER=/elsewhere/share/guard/xdg-open "$CLI" host -- sh -c 'echo "$PATH ${BROWSER-}"' 2>/dev/null)"
  check_eq "...nor one written with a trailing slash (up and run leave it out too)" "/usr/bin:/bin" \
    "$(PATH=/x/share/guard/:/usr/bin:/y/share/guard:/bin lib caller_path)"
  check_eq "host: a BROWSER the guard did not set stays (Omarchy sets it in the shell)" firefox \
    "$("${GUARDED[@]}" BROWSER=firefox "$CLI" host -- sh -c 'echo "${BROWSER-unset}"' 2>/dev/null)"
  # The user manager's, through a stand-in systemctl: a plain value is taken, one it quotes is not.
  mkdir -p "$TMP/sysenv"
  cat > "$TMP/sysenv/systemctl" <<'EOF'
#!/bin/sh
[ "$*" = "--user show-environment" ] || exit 1
echo "BROWSER=\$'/opt/my browser'"
echo GH_BROWSER=firefox
EOF
  chmod +x "$TMP/sysenv/systemctl"
  check_eq "host: the stand-in gives way to the user manager's GH_BROWSER, not to a quoted BROWSER" "unset firefox" \
    "$("${GUARDED[@]}" PATH="$TMP/sysenv:$ROOT/share/guard:$PATH" "$CLI" host -- sh -c 'echo "${BROWSER-unset} ${GH_BROWSER-unset}"' 2>/dev/null)"
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
  # finding 92: the guard's xdg-open stays out of a box (this repo is mounted in it, so it could be seen).
  check_match "a box's xdg-open is its own" '^/usr/' "$("${GUARDED[@]}" "$CLI" run -b "$B" -- sh -c 'command -v xdg-open')"
  # (Its PATH line first: a failed read would have no share/guard in it either.)
  local spath; spath=$("$CLI" run -b "$B" -- sh -c 'tr "\0" "\n" < /proc/$(pgrep -x Hyprland)/environ | grep "^PATH="')
  check_match "...and the box session's PATH" '^PATH=/' "$spath"
  check_fails "...lacks the guard's" grep share/guard <<<"$spath"
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
  # finding 95: it renders on the GPU its host names, so it gets every render node the host has (and
  # an NVIDIA one's userspace nodes), not just the one a headless box would pick.
  local gpu_nodes='ls /dev/dri/renderD* /dev/nvidiactl /dev/nvidia[0-9]* 2>/dev/null; true'
  check_eq "...with every GPU node of its host" "$("$CLI" run -b "$B" -- sh -c "$gpu_nodes")" \
    "$("${in[@]}" "$CLI" run -b inner -- sh -c "$gpu_nodes")"
  check_eq "...its window is on workspace 9" 9 "$(ob hyprctl -b "$B" -j clients | jq -r '.[] | select(.class == "aquamarine") | .workspace.name')"
  check_eq "...without focus" null "$(ob hyprctl -b "$B" -j activewindow | jq -r '.class')"
  # render_unfocused (finding 90): shots while its window is hidden; the stand-in's workspace and
  # focused window stay as they were. The first shot right after `up` gets a frame even from a box
  # that is not drawn while hidden, so a window opened in the box must show in a second one.
  local aw0; aw0=$(ob hyprctl -b "$B" -j activewindow | jq -r '.address // ""')
  check "...shot while its window is hidden" "${in[@]}" "$CLI" shot -b inner -o /tmp/hidden-inner.png
  local app=foot; command -v es2gears_wayland >/dev/null && app=es2gears_wayland
  "${in[@]}" "$CLI" run -b inner -d -- "$app" >/dev/null 2>&1
  # shellcheck disable=SC2329 # called through until_ok
  mapped() { "${in[@]}" "$CLI" hyprctl -b inner -j clients | jq -e 'length > 0' >/dev/null; }
  check "...a window opens in it" until_ok 10 mapped
  check "...shot again, still hidden" "${in[@]}" "$CLI" shot -b inner -o /tmp/hidden-inner2.png
  check "...which shows that window: the box is drawn while hidden" \
    "${in[@]}" sh -c '! cmp -s /tmp/hidden-inner.png /tmp/hidden-inner2.png && test -s /tmp/hidden-inner2.png'
  check_eq "...the host's workspace unchanged" 1 "$(ob hyprctl -b "$B" -j activeworkspace | jq -r '.name')"
  check_eq "...and its focused window" "$aw0" "$(ob hyprctl -b "$B" -j activewindow | jq -r '.address // ""')"
  # ...so wait sees it too (finding 82: it said "not rendered" before finding 90). Its answer depends
  # on the app (animated gears never hold still: 124), but it is never "unknown".
  local out rc=0; out=$("${in[@]}" "$CLI" wait -b inner still --timeout 3s 2>&1) || rc=$?
  check_match "wait on a hidden interactive box sees it drawn (an answer, not unknown)" "^(satisfied|unsatisfied): " "$out"
  check_match "...exit 0 or 124, never 1" "^(0|124)$" "$rc"
  "${in[@]}" "$CLI" down inner >/dev/null 2>&1
  # The workspace setting (finding 70): a number, the scratchpad; neither takes focus
  check "up --interactive --workspace 3" "${in[@]}" "$CLI" up ws3 --interactive --no-shell --workspace 3
  check "up --interactive on the scratchpad (config)" "${in[@]}" bash -c "'$CLI' config workspace special >/dev/null && '$CLI' up wsp --interactive --no-shell; '$CLI' config workspace default >/dev/null"
  check_eq "...on workspace 3 and the scratchpad" "3 special:scratchpad" "$(ob hyprctl -b "$B" -j clients | jq -r '[.[] | select(.class == "aquamarine") | .workspace.name] | sort | join(" ")')"
  check_eq "...without focus" null "$(ob hyprctl -b "$B" -j activewindow | jq -r '.class')"
  local b; for b in ws3 wsp; do
    check "...shot of $b while hidden" "${in[@]}" "$CLI" shot -b "$b" -o "/tmp/hidden-$b.png"
  done
  check_eq "...the host's workspace unchanged" 1 "$(ob hyprctl -b "$B" -j activeworkspace | jq -r '.name')"
  check_eq "...and its focused window" "$aw0" "$(ob hyprctl -b "$B" -j activewindow | jq -r '.address // ""')"
  "${in[@]}" "$CLI" down ws3 >/dev/null 2>&1; "${in[@]}" "$CLI" down wsp >/dev/null 2>&1
  # confirm-close: the first close opens a new window and asks, a second one ends the box; off, one
  # close ends it; `omabox config` changes a running box that took it from the config
  local cl=(ob hyprctl -b "$B" dispatch "hl.dsp.window.close({ window = 'class:aquamarine' })")
  state() { "${in[@]}" "$CLI" ls --json | jq -r --arg n "$1" '[.[] | select(.name == $n) | .state][0] // "gone"'; }
  # shellcheck disable=SC2329 # called through until_ok and holds
  gone() { [ "$(state "$1")" = gone ]; }
  # shellcheck disable=SC2329
  is() { [ "$(state "$1")" = "$2" ]; }
  if aq_unfixed "${in[@]}" "$CLI"; then
    # finding 125: without aquamarine's fix the box cannot open its window again
    check_match "no aquamarine fix: --confirm-close refused, saying why" "--confirm-close .*needs aquamarine's fix" \
      "$("${in[@]}" "$CLI" up cc --interactive --no-shell --confirm-close 2>&1)"
    check_eq "...before anything started" gone "$(state cc)"
    "${in[@]}" "$CLI" config confirm-close on >/dev/null 2>&1
    check_match "...on in the settings: off for the box, saying so" "confirm-close is on in your settings, but it needs" \
      "$("${in[@]}" "$CLI" up lv --interactive --no-shell 2>&1)"
    "${cl[@]}" >/dev/null
    check "...so one close ends it, cleared" until_ok 8 gone lv
    "${in[@]}" "$CLI" config confirm-close off >/dev/null
    # The flag on all the same (the library changed under a running box): confirm-close.sh finds no
    # new window and ends the box rather than leave it with none.
    "${in[@]}" "$CLI" up fb --interactive --no-shell >/dev/null 2>&1
    "${in[@]}" sh -c "echo on > '$("${in[@]}" "$CLI" path fb)/run/omabox.confirm-close'"
    "${cl[@]}" >/dev/null
    check "...and a box whose flag is on anyway ends when no window comes" until_ok 12 gone fb
  else
    "${in[@]}" "$CLI" up cc --interactive --no-shell --confirm-close >/dev/null 2>&1
    # issue #24: a window on 1 and one on 2, 2 shown: the new window must show 2 again, not a new one
    local cch=("${in[@]}" "$CLI" hyprctl -b cc)
    # shellcheck disable=SC2329 # called through until_ok
    ccwins() { "${cch[@]}" -j clients | jq -e --argjson n "$1" 'length == $n' >/dev/null; }
    "${in[@]}" "$CLI" run -b cc -d -- foot >/dev/null 2>&1
    until_ok 10 ccwins 1
    "${cch[@]}" dispatch 'hl.dsp.focus({ workspace = "2" })' >/dev/null
    "${in[@]}" "$CLI" run -b cc -d -- foot >/dev/null 2>&1
    until_ok 10 ccwins 2
    "${cl[@]}" >/dev/null
    check "confirm-close: the box stays after a close" holds 2 is cc up
    check_eq "...with a new window" 1 "$(ob hyprctl -b "$B" -j clients | jq '[.[] | select(.class == "aquamarine")] | length')"
    check_eq "...showing the workspace it showed (issue #24)" 2 "$("${cch[@]}" -j activeworkspace | jq -r .name)"
    check_eq "...its window focused" "foot 2" "$("${cch[@]}" -j activewindow | jq -r '"\(.class) \(.workspace.name)"')"
    check_eq "...its windows where they were, no empty workspace left" "1:1 2:1" \
      "$("${cch[@]}" -j workspaces | jq -r '[.[] | "\(.name):\(.windows)"] | sort | join(" ")')"
    # finding 90: that window has no render_unfocused; the box says so for `shot`'s message
    check "...marked as not drawn while hidden" "${in[@]}" test -f "$("${in[@]}" "$CLI" path cc)/run/omabox.reopened"
    "${cl[@]}" >/dev/null
    check "...and a second close ends it, cleared: no dead box left (finding 71)" until_ok 8 gone cc
    "${in[@]}" "$CLI" up lv --interactive --no-shell >/dev/null 2>&1
    "${in[@]}" "$CLI" config confirm-close on >/dev/null
    "${cl[@]}" >/dev/null
    check "config confirm-close on reaches a running box" holds 2 is lv up
    "${in[@]}" "$CLI" config confirm-close off >/dev/null
    "${cl[@]}" >/dev/null
    check "...and off again: one close ends it, cleared" until_ok 8 gone lv
  fi
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

# `omabox clip` (issue #23, finding 119) with a box standing in for the host (finding 26) and interactive
# boxes nested in it: the clipboard is the stand-in's, never the user's. No agent runs in the box, so
# what clip refuses there is what it refuses, not the suite's own caller.
t_clip() {
  local B=$P-cl
  ob up "$B" --net isolated --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up (the stand-in host)" "failed"; return; }
  local in=("$CLI" run -b "$B" --) H; H=$(ob path "$B")/home
  "${in[@]}" "$CLI" up ib --interactive --no-shell >/dev/null 2>&1 || { no "up --interactive in the stand-in" "failed"; ob down "$B" >/dev/null; return; }
  local inb=("${in[@]}" "$CLI" run -b ib --) out rc
  local wins0 aw0; wins0=$(ob hyprctl -b "$B" -j clients | jq length); aw0=$(ob hyprctl -b "$B" -j activewindow | jq -r '.address // ""')
  "${in[@]}" sh -c 'printf "p4ss w\303\266rd\n" | wl-copy'
  out=$("${in[@]}" "$CLI" clip -b ib 2>&1)
  check_eq "clip: text into an interactive box, said by type and size" "omabox: handed text (text/plain;charset=utf-8, 11 bytes) from your clipboard to box 'ib'" "$out"
  check_eq "...on its clipboard byte for byte (the newline too)" "$(printf 'p4ss w\303\266rd\n' | od -An -c)" "$("${inb[@]}" sh -c 'wl-paste -n | od -An -c')"
  check_eq "...offered as text by its usual names" "STRING TEXT UTF8_STRING text/plain text/plain;charset=utf-8" "$("${inb[@]}" wl-paste --list-types | LC_ALL=C sort -u | xargs)"
  "${in[@]}" sh -c 'printf "pw" | wl-copy --sensitive'
  check_match "clip: a password manager's secret stays marked" "\(text/plain;charset=utf-8, 2 bytes, marked sensitive\)" "$("${in[@]}" "$CLI" clip -b ib 2>&1)"
  check "...for the box's clipboard history to leave out" "${inb[@]}" sh -c 'wl-paste --list-types | grep -qx x-kde-passwordManagerHint'
  "${in[@]}" sh -c 'printf "second" | wl-copy'
  check_match "with no -b: the only interactive box" "to box 'ib'$" "$("${in[@]}" "$CLI" clip 2>&1)"
  check_eq "...which has it" second "$("${inb[@]}" wl-paste -n)"
  # An image, by its type; a PNG of the stand-in's screen
  "${in[@]}" sh -c 'grim /tmp/clip.png && wl-copy --type image/png < /tmp/clip.png'
  check_match "clip: an image" "handed an image \(image/png, [0-9.]+ [KM]iB\)" "$("${in[@]}" "$CLI" clip -b ib 2>&1)"
  check_eq "...the same bytes, as image/png" "image/png $("${in[@]}" sh -c 'md5sum < /tmp/clip.png' | xargs)" "$("${inb[@]}" sh -c 'wl-paste --list-types | xargs; wl-paste -n | md5sum' | xargs)"
  "${in[@]}" sh -c 'wl-copy --type image/jpeg < /tmp/clip.png'
  "${in[@]}" "$CLI" clip -b ib >/dev/null 2>&1
  check_eq "...a JPEG as image/jpeg" image/jpeg "$("${inb[@]}" wl-paste --list-types)"
  # What is refused: nothing to hand over, a type that is neither text nor an image
  "${in[@]}" sh -c 'printf x | wl-copy --type application/x-omabox-test'
  out=$("${in[@]}" "$CLI" clip -b ib 2>&1); rc=$?
  check_match "clip: another type refused" "holds application/x-omabox-test : only text and images go into a box" "$out"
  check_eq "...exit 1, the box's clipboard as it was" "1 image/jpeg" "$rc $("${inb[@]}" wl-paste --list-types)"
  "${in[@]}" wl-copy --clear
  check_match "clip: an empty clipboard" "your clipboard is empty: nothing handed over" "$("${in[@]}" "$CLI" clip -b ib 2>&1)"
  "${in[@]}" sh -c 'printf "" | wl-copy'
  check_match "clip: an empty item" "the item is empty: nothing handed over" "$("${in[@]}" "$CLI" clip -b ib 2>&1)"
  # --from-box: the mirror
  "${inb[@]}" sh -c 'printf "from the box" | wl-copy'
  check_eq "clip --from-box: text" "omabox: handed text (text/plain;charset=utf-8, 12 bytes) from box 'ib' to your clipboard" "$("${in[@]}" "$CLI" clip --from-box -b ib 2>&1)"
  check_eq "...on the host's clipboard" "from the box" "$("${in[@]}" wl-paste -n)"
  "${inb[@]}" sh -c 'printf "box secret" | wl-copy --sensitive'
  "${in[@]}" "$CLI" clip --from-box -b ib >/dev/null 2>&1
  check "clip --from-box: a secret stays marked on the host" "${in[@]}" sh -c 'wl-paste --list-types | grep -qx x-kde-passwordManagerHint'
  local hist=$H/.local/state/omarchy/clipboard-history.json
  check "...the host's clipboard history (Omarchy's) has the box's text" until_ok 5 grep -q "from the box" "$hist"
  check_fails "...not its secret" grep -q "box secret" "$hist"
  "${inb[@]}" sh -c 'grim /tmp/b.png && wl-copy --type image/png < /tmp/b.png'
  "${in[@]}" "$CLI" clip --from-box -b ib >/dev/null 2>&1
  check_eq "clip --from-box: an image, the same bytes" "image/png $("${inb[@]}" sh -c 'md5sum < /tmp/b.png' | xargs)" "$("${in[@]}" sh -c 'wl-paste --list-types | xargs; wl-paste -n | md5sum' | xargs)"
  "${inb[@]}" wl-copy --clear
  check_match "clip --from-box: an empty clipboard" "box 'ib''s clipboard is empty" "$("${in[@]}" "$CLI" clip --from-box -b ib 2>&1)"
  # One shot: on the host only the wl-copy serving the item (its own session, the fds of no file of
  # clip's), no wl-paste or omabox left; the host's windows and focus as they were.
  # shellcheck disable=SC2329 # called through until_ok
  one_server() { [ "$("${in[@]}" sh -c 'for p in $(pgrep -x wl-copy); do [ "$(readlink /proc/$p/ns/pid)" != "$(readlink /proc/self/ns/pid)" ] || echo $p; done' | wc -l)" = 1 ]; }
  check "clip left one wl-copy on the host: the one serving the item" until_ok 5 one_server
  # (the stand-in's shell watches its clipboard for Omarchy's history: wl-paste --watch)
  check_eq "...no wl-paste and no clip running, in the stand-in or its boxes" "" "$("${in[@]}" sh -c 'pgrep -a wl-paste | grep -v -- " --watch "; pgrep -af "omabox cli[p]"; true')"
  check_eq "...no process holds the item's file (wl-copy's stderr file: its own)" 0 "$("${in[@]}" sh -c 'ls -l /proc/[0-9]*/fd 2>/dev/null | grep -c omabox-clip-item')"
  check_eq "...the host's windows" "$wins0" "$(ob hyprctl -b "$B" -j clients | jq length)"
  check_eq "...and its focus unchanged" "$aw0" "$(ob hyprctl -b "$B" -j activewindow | jq -r '.address // ""')"
  # The bar widget's rows (Paste in, Copy out) run the real CLI in the stand-in
  printf '#!/bin/sh\nexec %q "$@"\n' "$CLI" > "$H/.local/bin/omabox"; chmod +x "$H/.local/bin/omabox"
  "${in[@]}" sh -c 'printf "via the widget" | wl-copy'
  # shellcheck disable=SC2329 # called through until_ok
  panel() { ob hyprctl -b "$B" -j layers | grep -q omarchy-keyboard-panel; }
  # shellcheck disable=SC2329 # called through until_ok
  has() { [ "$("${inb[@]}" wl-paste -n 2>/dev/null)" = "$1" ]; }
  # shellcheck disable=SC2329 # called through until_ok
  listed() { ob run -b "$B" -- omarchy-shell chaves.omabox open >/dev/null 2>&1; until_ok 3 panel >/dev/null && ob keys -b "$B" Escape >/dev/null && sleep 2.5; }
  listed   # (the panel polls every 2 s while open: ib is in its list after that)
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down v >/dev/null
  check "the widget's v pastes your clipboard into the box" until_ok 10 has "via the widget"
  "${inb[@]}" sh -c 'printf "out via the widget" | wl-copy'
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down c >/dev/null
  check "...its c copies the box's out" until_ok 10 bash -c "[ \"\$('$CLI' run -b '$B' -- wl-paste -n)\" = 'out via the widget' ]"
  # Refused: a headless box (an agent's); several interactive ones and none focused; an agent
  "${in[@]}" "$CLI" up hb --no-shell >/dev/null 2>&1
  check_match "clip: a headless box refused" "box 'hb' is headless: clip is for interactive boxes only" "$("${in[@]}" "$CLI" clip -b hb 2>&1)"
  "${in[@]}" "$CLI" down hb >/dev/null 2>&1
  local pids0 pid2
  pids0=$(ob hyprctl -b "$B" -j clients | jq '[.[] | select(.class == "aquamarine") | .pid]')
  "${in[@]}" "$CLI" up ib2 --interactive --no-shell >/dev/null 2>&1
  pid2=$(ob hyprctl -b "$B" -j clients | jq --argjson o "$pids0" '[.[] | select(.class == "aquamarine") | .pid] - $o | .[0]')
  ob hyprctl -b "$B" dispatch "hl.dsp.focus({ window = 'class:^foot\$' })" >/dev/null 2>&1   # nothing there: no focus change
  check_match "with no -b, several interactive boxes and none focused: which one?" "interactive boxes ib ib2 are up and none has focus" "$("${in[@]}" "$CLI" clip 2>&1)"
  ob hyprctl -b "$B" dispatch "hl.dsp.focus({ window = 'pid:$pid2' })" >/dev/null
  "${in[@]}" sh -c 'printf "to the focused one" | wl-copy'
  check_match "...the one whose window has focus" "to box 'ib2'$" "$("${in[@]}" "$CLI" clip 2>&1)"
  check_eq "...which has it" "to the focused one" "$("${in[@]}" "$CLI" run -b ib2 -- wl-paste -n)"
  "${in[@]}" sh -c 'printf "secret" | wl-copy'; "${inb[@]}" wl-copy --clear
  check_match "clip refused to an agent's shell" "not for agents \(CLAUDECODE set for " "$("${in[@]}" env CLAUDECODE=1 "$CLI" clip -b ib 2>&1)"
  check_match "...with the variable dropped (env -u) under that shell" "not for agents \(CLAUDECODE set for bash" \
    "$("${in[@]}" env CLAUDECODE=1 bash -c 'env -u CLAUDECODE "$0" clip -b ib; true' "$CLI" 2>&1)"
  check_match "...under a program named claude, whatever the environment" "not for agents \(claude, pid" \
    "$("${in[@]}" sh -c 'cp /usr/bin/bash /tmp/claude && /tmp/claude -c "env -i PATH=/usr/bin HOME=\$HOME XDG_RUNTIME_DIR=\$XDG_RUNTIME_DIR \$0 clip -b ib; true" "$0"' "$CLI" 2>&1)"
  check_match "...under the guard" "not for agents" "$("${in[@]}" "${GUARDED[@]}" "$CLI" clip -b ib 2>&1)"
  check_match "...under guard exec" "not for agents" "$("${in[@]}" "$CLI" guard exec -- "$CLI" clip -b ib 2>&1)"
  check_match "...through omabox host" "not for agents" "$("${in[@]}" "${GUARDED[@]}" CLAUDECODE=1 "$CLI" host -- "$CLI" clip -b ib 2>&1)"
  check_eq "...and the box's clipboard got nothing" "" "$("${inb[@]}" wl-paste -n 2>/dev/null)"
  "${in[@]}" "$CLI" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# keys-to-box and the passthrough indicator (issue #22, finding 117), in a stand-in host (finding 26):
# its Hyprland runs the host side (share/passthrough.lua), two interactive boxes are windows in it, and
# the stand-in's own keyboard presses the keys. Nothing reaches the real desktop.
t_keys_to_box() {
  local B=$P-kb
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up (the stand-in host)" "failed"; return; }
  local in=("$CLI" run -b "$B" -- "$CLI")
  check_match "keys-to-box refuses a headless box" "is headless" "$(ob keys-to-box -b "$B" on 2>&1)"
  if ! "${in[@]}" up ka --interactive --no-shell >/dev/null 2>&1 || ! "${in[@]}" up kb --interactive --no-shell >/dev/null 2>&1; then
    no "up --interactive twice in the stand-in" "failed"; ob down "$B" >/dev/null; return
  fi
  ob run -b "$B" -d -q -- foot
  # shellcheck disable=SC2329 # called through until_ok
  has_foot() { ob hyprctl -b "$B" -j clients | jq -e 'any(.class == "foot")' >/dev/null; }
  until_ok 10 has_foot
  # Each box's window by its client's pid: the box's outer bwrap, whose command line binds its run dir
  # (the first such bwrap; the one it forks has the same command line).
  local pa pb
  pa=$(ob run -b "$B" -- sh -c 'for p in $(pgrep -x bwrap); do tr "\0" " " < /proc/$p/cmdline | grep -q "/omabox/$1/run " && { echo "$p"; break; }; done' _ ka)
  pb=$(ob run -b "$B" -- sh -c 'for p in $(pgrep -x bwrap); do tr "\0" " " < /proc/$p/cmdline | grep -q "/omabox/$1/run " && { echo "$p"; break; }; done' _ kb)
  check_eq "...each box's window found by its pid" "aquamarine aquamarine" \
    "$(ob hyprctl -b "$B" -j clients | jq -r --argjson a "${pa:-0}" --argjson b "${pb:-0}" '[.[] | select(.pid == $a or .pid == $b) | .class] | join(" ")')"
  # shellcheck disable=SC2329 # called through check
  focus() { ob hyprctl -b "$B" dispatch "hl.dsp.focus({ window = '$1' })" >/dev/null; }
  # The stand-in's state: submap|focused class|border (our tag on it)|the box .keys names.
  # shellcheck disable=SC2329
  kstate() {
    printf '%s|%s' "$(ob lua -b "$B" 'local w = hl.get_active_window()
      local t = w and table.concat(w.tags, ",") or ""
      return hl.get_current_submap() .. "|" .. (w and w.class or "none") .. "|" .. (t:find("omabox%-keys") and "border" or "plain")')" \
      "$(ob run -b "$B" -- sh -c 'cat "$XDG_RUNTIME_DIR/omabox/.keys" 2>/dev/null')"
  }
  # shellcheck disable=SC2329
  kis() { local s; s=$(kstate); [ "$s" = "$1" ] || { echo "$s"; return 1; }; }
  # shellcheck disable=SC2329
  kre() { local s; s=$(kstate); [[ $s =~ $1 ]] || { echo "$s"; return 1; }; }
  # shellcheck disable=SC2329
  box_ws() { [ "$("${in[@]}" hyprctl -b "$1" -j activeworkspace | jq -r .name)" = "$2" ]; }
  # shellcheck disable=SC2329
  host_ws() { [ "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .name)" = "$1" ]; }
  # The stand-in's pointer to the middle of its window pid:N.
  # shellcheck disable=SC2329
  to_mid() {
    local xy; xy=$(ob hyprctl -b "$B" -j clients | jq -r --arg s "$1" '[.[] | select("pid:\(.pid)" == $s)][0]
      | "\(.at[0] + (.size[0] / 2 | floor)) \(.at[1] + (.size[1] / 2 | floor))"')
    ob pointer -b "$B" -- move "${xy% *}" "${xy#* }" >/dev/null
  }
  local red; red=$(sed -n 's/^red *= *"#\([0-9a-fA-F]\{6\}\)".*/\1/p' "$(ob path "$B")/home/.local/state/omarchy/current/theme/colors.toml" | tr 'A-F' 'a-f')
  red=ff${red:-ff5555}
  check_eq "keys-to-box is off by default" off "$("${in[@]}" keys-to-box -b ka)"
  check "keys-to-box -b ka on" "${in[@]}" keys-to-box -b ka on
  check_eq "...ls --json says so, for that box only" "ka:true kb:false" \
    "$("${in[@]}" ls --json | jq -r '[.[] | "\(.name):\(.keys_to_box)"] | join(" ")')"
  focus "pid:$pa"
  check "focusing its window enters the submap, border and widget file on" until_ok 5 kis "omabox|aquamarine|border|ka"
  check_eq "...its border the theme's red" "$red 0deg" "$(ob hyprctl -b "$B" getprop "pid:$pa" active_border_color)"
  focus class:foot
  check "focus elsewhere leaves it, border and file back" until_ok 5 kis "|foot|plain|"
  check_fails "...its border no longer red" grep -q "^$red" <<<"$(ob hyprctl -b "$B" getprop "pid:$pa" active_border_color)"
  focus "pid:$pa"
  check "...focused again: the submap again" until_ok 5 kis "omabox|aquamarine|border|ka"
  ob keys -b "$B" super+2 >/dev/null
  check "SUPER+2 goes to the box" until_ok 5 box_ws ka 2
  check_eq "...not to the host" 9 "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .name)"
  ob keys -b "$B" super+alt+Escape >/dev/null
  check "SUPER+ALT+ESCAPE gets the keys back, the box still focused" until_ok 5 kis "|aquamarine|plain|"
  ob pointer -b "$B" -- move 2 2 >/dev/null; to_mid "pid:$pa"
  check "...the pointer leaving the box and coming back does not undo that" holds 1 kis "|aquamarine|plain|"
  ob keys -b "$B" super+1 >/dev/null
  check "...SUPER+1 is the host's then" until_ok 5 host_ws 1
  focus "pid:$pa"
  check "...until the box loses focus and gets it back" until_ok 5 kis "omabox|aquamarine|border|ka"
  focus "pid:$pb"
  check "a box without keys-to-box: focus does not enter it" until_ok 5 kis "|aquamarine|plain|"
  # One-shot, as before, with the indicator too.
  ob keys -b "$B" super+alt+Escape >/dev/null
  check "one-shot (SUPER+ALT+ESCAPE on kb): submap, border and file" until_ok 5 kis "omabox|aquamarine|border|kb"
  ob pointer -b "$B" -- move 2 2 >/dev/null   # the stand-in's corner: off every box window
  ob keys -b "$B" a >/dev/null
  check "...a key with the pointer off the box ends it (finding 29)" until_ok 5 kis "|aquamarine|plain|"
  # issue #55: in keys-to-box the pointer decides too. The box alone on a workspace of the stand-in's
  # (5): the pointer on an empty corner (where a bar would be) gives the keys back, keys-to-box on.
  ob hyprctl -b "$B" dispatch "hl.dsp.window.move({ workspace = '5', window = 'pid:$pa' })" >/dev/null
  focus "pid:$pa"; to_mid "pid:$pa"
  check "keys-to-box, the box alone on its workspace, the pointer over it: the submap" until_ok 5 kis "omabox|aquamarine|border|ka"
  ob pointer -b "$B" -- move 2 2 >/dev/null
  check "...the pointer off it: submap, border and file off, the box still focused" until_ok 5 kis "|aquamarine|plain|"
  check_eq "...keys-to-box still on" on "$("${in[@]}" keys-to-box -b ka)"
  ob keys -b "$B" super+1 >/dev/null
  check "...SUPER+1 switches the stand-in's workspace, on the first press" until_ok 5 host_ws 1
  check "...not the box's" box_ws ka 2
  focus "pid:$pa"; ob pointer -b "$B" -- move 2 2 >/dev/null
  until_ok 5 kis "|aquamarine|plain|" >/dev/null
  to_mid "pid:$pa"
  check "...the pointer back over the box: the submap, border and file again" until_ok 5 kis "omabox|aquamarine|border|ka"
  ob keys -b "$B" super+3 >/dev/null
  check "...SUPER+3 goes to the box" until_ok 5 box_ws ka 3
  check_eq "...not to the stand-in" 5 "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .name)"
  # The key pressed right after the pointer moved, before the timer has looked: the key hook runs
  # before Hyprland looks the key up in the binds, so it decides for that very key (the timer off here).
  ob lua -b "$B" 'omabox_pass_timer:set_enabled(false)' >/dev/null
  ob pointer -b "$B" -- move 2 2 >/dev/null
  check "...with no timer the submap is still on" holds 0.5 kis "omabox|aquamarine|border|ka"
  ob keys -b "$B" super+1 >/dev/null
  check "...and SUPER+1 still switches the stand-in's workspace, on the first press" until_ok 5 host_ws 1
  focus "pid:$pa"; ob pointer -b "$B" -- move 2 2 >/dev/null
  until_ok 5 kis "|aquamarine|plain|" >/dev/null
  ob lua -b "$B" 'omabox_pass_timer:set_enabled(false)' >/dev/null
  to_mid "pid:$pa"
  ob keys -b "$B" super+4 >/dev/null
  check "...the other way: SUPER+4 goes to the box" until_ok 5 box_ws ka 4
  check_eq "...not to the stand-in" 5 "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .name)"
  focus class:foot; focus "pid:$pa"   # the timer back (focus starts it again)
  check "...focus starts the timer again" until_ok 3 test "$(ob lua -b "$B" 'return omabox_pass_timer:is_enabled()')" = true
  focus class:foot
  check "focus off the box stops it" until_ok 3 test "$(ob lua -b "$B" 'return omabox_pass_timer:is_enabled()')" = false
  focus "pid:$pa"; to_mid "pid:$pa"
  # A config reload drops the hooks, the binds and the rule; the box's reaper puts them back.
  ob hyprctl -b "$B" reload >/dev/null
  # shellcheck disable=SC2329
  reinstalled() { [ "$(ob lua -b "$B" 'return omabox_pass_version')" = 4 ]; }
  check "after a host reload the hooks come back (the reaper)" until_ok 6 reinstalled
  check "...with the submap, border and file as focus says" until_ok 3 kis "omabox|aquamarine|border|ka"
  check_eq "...one toggle bind in each submap" "omabox:1 :1" "$(ob hyprctl -b "$B" -j binds |
    jq -r '[.[] | select(.description | startswith("omabox:"))] | group_by(.submap) | map("\(.[0].submap):\(length)") | reverse | join(" ")')"
  focus class:foot; focus "pid:$pa"
  check "...and focus still drives it" until_ok 5 kis "omabox|aquamarine|border|ka"
  check "keys-to-box off with the box focused: the keys are the host's at once" "${in[@]}" keys-to-box -b ka off
  check "...submap, border and file off" until_ok 5 kis "|aquamarine|plain|"
  if aq_unfixed "${in[@]}"; then
    skip "a confirm-close keep keeps the keys-to-box box (issue #24)" "aquamarine without the fix for nested Wayland outputs: no keep (t_guard checks that)"
  else
    # issue #24: a confirm-close keep recreates the box's output (a new window, the same client).
    "${in[@]}" keys-to-box -b ka on >/dev/null 2>&1
    "${in[@]}" config confirm-close on >/dev/null
    ob hyprctl -b "$B" dispatch "hl.dsp.window.close({ window = 'pid:$pa' })" >/dev/null
    # shellcheck disable=SC2329
    reopened() { "${in[@]}" path ka >/dev/null && ob run -b "$B" -- test -f "$("${in[@]}" path ka)/run/omabox.reopened"; }
    check "confirm-close keeps the box, with a new window (issue #24)" until_ok 8 reopened
    "${in[@]}" config confirm-close off >/dev/null
    focus class:foot; focus "pid:$pa"
    check "after a confirm-close keep, its new window is still the keys-to-box box" until_ok 5 kis "omabox|aquamarine|border|ka"
  fi
  "${in[@]}" down ka >/dev/null 2>&1
  check "down with the box focused: nobody is left in the submap" until_ok 5 kre '^\|[a-z]*\|plain\|$'
  "${in[@]}" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# Keys held when focus leaves an interactive box (finding 132), in a stand-in host (finding 26): SUPER
# held down in the box while the stand-in's focus moves to its foot, released there. aquamarine never
# heard the release, so the box's Hyprland kept SUPER down and a later W was SUPER+W (closed the window).
# omabox's aquamarine build releases what is held when the keyboard leaves.
t_held_keys() {
  local B=$P-hk
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up (the stand-in host)" "failed"; return; }
  local in=("$CLI" run -b "$B" -- "$CLI")
  # (Asked of this omabox: the stand-in's own sees the build it binds at /opt/omabox/lib, not its name.)
  if ! ob --version 2>/dev/null | grep -q '^aquamarine: a private build, .*+keys'; then
    skip "a key held when focus leaves an interactive box is released there" "not omabox's aquamarine build (omabox setup --aquamarine)"
    ob down "$B" >/dev/null; return
  fi
  "${in[@]}" up hk --interactive --no-shell >/dev/null 2>&1 || { no "up --interactive in the stand-in" "failed"; ob down "$B" >/dev/null; return; }
  ob run -b "$B" -d -q -- foot
  # What the box's foot is given, byte by byte, in its /tmp.
  "${in[@]}" run -b hk -d -q -- foot --app-id=omabox.typed sh -c 'stty raw -echo; dd bs=1 of=/tmp/typed 2>/dev/null'
  # shellcheck disable=SC2329 # called through until_ok
  ready() { ob hyprctl -b "$B" -j clients | jq -e 'any(.class == "foot")' >/dev/null &&
    "${in[@]}" hyprctl -b hk -j clients | jq -e 'any(.class == "omabox.typed")' >/dev/null; }
  until_ok 10 ready
  local pa
  pa=$(ob run -b "$B" -- sh -c 'for p in $(pgrep -x bwrap); do tr "\0" " " < /proc/$p/cmdline | grep -q "/omabox/hk/run " && { echo "$p"; break; }; done')
  # shellcheck disable=SC2329
  focus() { ob hyprctl -b "$B" dispatch "hl.dsp.focus({ window = '$1' })" >/dev/null; }
  # shellcheck disable=SC2329
  typed() { [ "$("${in[@]}" run -b hk -- cat /tmp/typed 2>/dev/null)" = "$1" ]; }
  focus "pid:$pa"
  ob keys -b "$B" -t a >/dev/null
  check "the box's foot gets a key typed in its window" until_ok 5 typed a
  # SUPER down in the box (-m), the stand-in's focus moved off it while held (-p waits), released there.
  ob run -b "$B" -- sh -c 'sleep 3 | /opt/omabox/bin/omabox-keyboard -m super -p 2000' >/dev/null 2>&1 &
  local kb=$!
  sleep 0.5; focus class:foot; wait "$kb"
  focus "pid:$pa"
  ob keys -b "$B" -t w >/dev/null
  check "...after SUPER was held while focus left: W is a W, not SUPER+W" until_ok 5 typed aw
  check "...and the window is still there" bash -c "'$CLI' run -b '$B' -- '$CLI' hyprctl -b hk -j clients | jq -e 'any(.class == \"omabox.typed\")' >/dev/null"
  "${in[@]}" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# `up` on a box that is up (finding 134): the options given that it lacks are refused, saying which;
# a bare up, or one with what it has, is fine. --json prints it as ls --json lists it.
t_up_again() {
  local B=$P-ua out
  ob up "$B" --net isolated --no-shell >/dev/null 2>&1 || { no "up" "failed"; return; }
  check "up again, no options: fine" ob up "$B"
  check "...with the options it has" ob up "$B" --net isolated --no-shell
  out=$(ob up "$B" --size 1280x720 --systemd 2>&1); local rc=$?
  check_eq "...with others: refused" 1 "$rc"
  check_match "...saying what it lacks" "--size 1280x720@60 \(it is 1920x1080@60\); --systemd \(it has no user manager\)" "$out"
  check_match "...and what to do" "omabox down $B first, or omabox up --new" "$out"
  check_match "--net connected on an isolated box: refused" "--net connected \(it is isolated\)" "$(ob up "$B" --net connected 2>&1)"
  check_match "--plugin it has not: refused" "--plugin $ROOT/plugin \(not mounted\)" "$(ob up "$B" --plugin "$ROOT/plugin" 2>&1)"
  check_eq "up --json: the box as ls --json has it" "$B up isolated" "$(ob up "$B" --json 2>/dev/null | jq -r '"\(.name) \(.state) \(.net)"')"
  check_eq "...the same object" "$(ob ls --json | jq -c --arg n "$B" '.[] | select(.name == $n)')" "$(ob up "$B" --json 2>/dev/null)"
  ob down "$B" >/dev/null
}

# The host's submap after the last interactive box goes (finding 134), in a stand-in host (finding
# 26): a config reload there drops passthrough's hooks and keeps the submap, and only a box's reaper
# puts the hooks back. With that reaper gone too, `down` resets the submap, or the host's binds
# would stay dead.
t_submap_release() {
  local B=$P-sr
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up (the stand-in host)" "failed"; return; }
  local in=("$CLI" run -b "$B" -- "$CLI")
  # Another tool's host rule (finding 136): every aquamarine window floating, on a workspace of its own.
  ob hyprctl -b "$B" eval 'hl.window_rule({ name = "nested-float", match = { class = "aquamarine" }, float = true, workspace = "name:nested silent" })' >/dev/null
  "${in[@]}" up sa --interactive --no-shell >/dev/null 2>&1 || { no "up --interactive in the stand-in" "failed"; ob down "$B" >/dev/null; return; }
  check_eq "under a host rule for every aquamarine window, the box's is still tiled on workspace 9" "9 false" \
    "$(ob hyprctl -b "$B" -j clients | jq -r '.[] | select(.class == "aquamarine") | "\(.workspace.name) \(.floating)"')"
  local pa
  pa=$(ob run -b "$B" -- sh -c 'for p in $(pgrep -x bwrap); do tr "\0" " " < /proc/$p/cmdline | grep -q "/omabox/sa/run " && { echo "$p"; break; }; done')
  ob hyprctl -b "$B" dispatch "hl.dsp.focus({ window = 'pid:$pa' })" >/dev/null
  # shellcheck disable=SC2329 # called through until_ok
  submap_is() { [ "$(ob lua -b "$B" 'return hl.get_current_submap()')" = "$1" ]; }
  ob keys -b "$B" super+alt+Escape >/dev/null
  check "SUPER+ALT+ESCAPE: the stand-in in the omabox submap" until_ok 5 submap_is omabox
  ob run -b "$B" -- sh -c 'pkill -f "_reap sa "'
  ob hyprctl -b "$B" reload >/dev/null
  check "a reload with the box's reaper gone: still in it, the hooks gone" holds 2.5 bash -c \
    "[ \"\$('$CLI' lua -b '$B' 'return hl.get_current_submap() .. \",\" .. tostring(omabox_pass_version)')\" = omabox,nil ]"
  "${in[@]}" down sa >/dev/null 2>&1
  check "down of the last interactive box resets it" until_ok 3 submap_is ""
  ob keys -b "$B" super+1 >/dev/null
  check "...and the stand-in's SUPER+1 works again" until_ok 3 bash -c "[ \"\$('$CLI' hyprctl -b '$B' -j activeworkspace | jq -r .name)\" = 1 ]"
  ob down "$B" >/dev/null
}

# setup's questions answered through a terminal (script(1)), in a box with the shell (finding 133):
# the widget put in the box's bar, a "no" remembered; under a system install, a ~/.local/bin/omabox
# left from a checkout offered to go.
t_setup_prompts() {
  local B=$P-sp
  ob up "$B" --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  local H; H=$(ob path "$B")/home
  # shellcheck disable=SC2329
  in_tty() { printf '%s' "$1" | ob run -b "$B" -- script -qec "$CLI setup" /dev/null 2>&1; }
  local answers=$'y\n' sys=0
  if [[ $ROOT == /usr/* ]]; then
    sys=1; answers=$'y\ny\n'; mkdir -p "$H/.local/bin"; ln -sfn /nowhere/omabox/bin/omabox "$H/.local/bin/omabox"
  fi
  local out; out=$(in_tty "$answers")
  check_match "setup in a terminal asks to show the widget" "Show the omabox widget in your bar\? \[Y/n\]" "$out"
  check "...yes: it is in the box's bar" until_ok 5 grep -q '"chaves.omabox"' "$H/.config/omarchy/shell.json"
  if [ "$sys" = 1 ]; then
    check_match "...a checkout's ~/.local/bin/omabox: said" "links to /nowhere/omabox/bin/omabox, which is gone" "$out"
    check_fails "...yes: removed" test -L "$H/.local/bin/omabox"
  else
    skip "a checkout's ~/.local/bin/omabox under a system install: said and removed" "a checkout run (--installed checks it)"
  fi
  check_match "setup again: on, not asked" "bar widget: on in your bar" "$(in_tty "")"
  ob run -b "$B" -- omarchy plugin disable chaves.omabox >/dev/null 2>&1
  until_ok 5 bash -c "! grep -q '\"chaves.omabox\"' '$H/.config/omarchy/shell.json'"
  out=$(in_tty $'n\n')
  check "...no: not in the bar, and remembered" bash -c "! grep -q '\"chaves.omabox\"' '$H/.config/omarchy/shell.json' && test -e '$H/.config/omabox/widget-declined'"
  check_match "...then not asked again" "bar widget: not asked \(you said no before" "$(in_tty "")"
  ob down "$B" >/dev/null
}

# `up --omarchy DIR` (finding 135): a box on another Omarchy tree, a copy of the installed one with a
# marker in its bin and its Hyprland bootstrap. The session, the bar, `run` and a terminal's bash (its
# /etc/omarchy.conf) all use it; a box started inside that one, without --omarchy, does not follow the
# outer box's /etc/omarchy.conf (as a box does not follow the host's dev link).
t_omarchy_tree() {
  local B=$P-ot T=$TMP/omarchy-tree f
  mkdir -p "$T"
  for f in /usr/share/omarchy/*; do
    case ${f##*/} in themes) ln -s "$f" "$T/themes" ;; *) cp -a "$f" "$T/" ;; esac
  done
  chmod -R u+w "$T"
  # (The package's bin/ links to /usr/bin: a file of its own in the link's place.)
  rm -f "$T/bin/omarchy-version"; printf '#!/bin/sh\necho omabox-tree\n' > "$T/bin/omarchy-version"; chmod +x "$T/bin/omarchy-version"
  printf '\nomabox_tree_marker = "tree"\n' >> "$T/default/hypr/bootstrap.lua"
  check_match "--omarchy refuses a folder that is not an Omarchy tree" "not an Omarchy tree \(no bin\)" "$(ob up "$B" --omarchy "$TMP" 2>&1)"
  ob up "$B" --net isolated --omarchy "$T" >/dev/null 2>&1 || { no "up --omarchy" "failed"; return; }
  check_eq "the box's Hyprland loaded the tree's config" tree "$(ob lua -b "$B" 'return omabox_tree_marker')"
  check_eq "...run has it as OMARCHY_PATH" "$T" "$(ob run -b "$B" -- sh -c 'echo "$OMARCHY_PATH"')"
  check_eq "...and its bin first" omabox-tree "$(ob run -b "$B" -- omarchy-version)"
  check_match "...the bar runs from it" "quickshell -n -p $T/shell" "$(ob run -b "$B" -- pgrep -a quickshell)"
  check_eq "...a terminal's bash too (the box's /etc/omarchy.conf)" "$T omabox-tree" "$(ob run -b "$B" -- bash -ic 'echo "$OMARCHY_PATH $(omarchy-version)"' 2>/dev/null)"
  check_eq "ls --json names it" "$T" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy')"
  check_eq "...and its version as dev (no git: no commit)" dev "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy_version')"
  check_match "up again with another tree: refused" "--omarchy /usr/share/omarchy" "$(ob up "$B" --omarchy /usr/share/omarchy 2>&1)"
  # A box in that box, on the installed Omarchy: the outer /etc/omarchy.conf names the tree.
  local in=("$CLI" run -b "$B" -- "$CLI")
  if "${in[@]}" up oi --no-shell >/dev/null 2>&1; then
    check_eq "a box started where /etc/omarchy.conf names another tree runs the installed Omarchy" "/usr/share/omarchy" \
      "$("${in[@]}" run -b oi -- bash -ic 'echo "$OMARCHY_PATH"' 2>/dev/null)"
    "${in[@]}" down oi >/dev/null 2>&1
  else
    no "up inside the --omarchy box" "failed"
  fi
  ob down "$B" >/dev/null
}

# Window selectors and what covers what (finding 81), on hyprctl JSON made up for it: floats above
# tiled windows whatever their order, later above earlier otherwise, fullscreen and a shown special
# workspace above the rest; off-screen workspaces, inactive group tabs and unmapped windows.
t_unit_window_select() {
  local c m m_sp act='{"address":"0xb"}'
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
     w("0xh"; "org.gnome.Nautilus"; "Home"; 3; 0; 0; 800; 600; false),
     w("0xd"; "foot"; "scratch"; -98; 500; 500; 200; 200; true),
     w("0xe"; "foot"; "gone"; 1; 0; 0; 10; 10; false) + {mapped: false}]')
  m='[{"activeWorkspace":{"id":1},"specialWorkspace":{"id":0}}]'
  m_sp='[{"activeWorkspace":{"id":1},"specialWorkspace":{"id":-98}}]'
  local W Wsp; W=$(lib win_annotate "$c" "$m" "$act") Wsp=$(lib win_annotate "$c" "$m_sp" "$act")
  sel() { lib win_select "$W" "$@" 2>/dev/null | jq -r .address; }
  check_eq "title:RE" 0xa "$(sel 'title:^A$')"
  check_eq "a word: part of the title, any case" 0xc "$(sel docs)"
  check_eq "a word: the class, any case" 0xc "$(sel chromium)"
  check_eq "a word: a reverse-DNS class's last part (#20)" 0xh "$(sel nautilus)"
  check_eq "...whole, not a prefix of it" 2 "$(lib win_select "$W" naut >/dev/null 2>&1; echo $?)"
  check_eq "...the whole class still works" 0xh "$(sel org.gnome.nautilus)"
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
  # wait window (#37): any of several matching windows is an answer, and all are named.
  check_match "wait window: several match, satisfied, each named" '^yes 4 windows: 0xa foot "A" at 0,0 1000x1000; 0xb foot "B" at 1000,0 900x1000, focused; 0xg .*; 0xd ' "$(lib win_answer present "$W" foot)"
  check_match "...one: as before" '^yes 0xh org.gnome.Nautilus "Home" at 0,0 800x600$' "$(lib win_answer present "$W" nautilus)"
  check_eq "...none" no "$(lib win_answer present "$W" nope)"
  check_eq "--focused: the focused one of several" 'yes 0xb foot "B" at 1000,0 900x1000, focused (1 of 4 matching)' "$(lib win_answer focused "$W" foot)"
  check_eq "...several, none focused" "no 2 matching, none focused" "$(lib win_answer focused "$W" zenity)"
  check_eq "...one, not focused" 'no 0xa foot "A" is there' "$(lib win_answer focused "$W" 'title:^A$')"
  check_eq "--gone: several still there" "no 4 matching" "$(lib win_answer gone "$W" foot)"
  check_eq "a bad regex: exit 2 (never answerable)" 2 "$(lib win_answer present "$W" 'title:(' >/dev/null 2>&1; echo $?)"
  cov() { jq -r --arg a "$2" '.[] | select(.address == $a) | [.cover[].address] | join(" ")' <<<"$1"; }
  on() { jq -r --arg a "$2" '.[] | select(.address == $a) | .onscreen' <<<"$1"; }
  check_eq "floats cover the tiled window under them, though one is earlier in the list" "0xf 0x2" "$(cov "$W" 0xa)"
  check_eq "...a tiled window never covers a float" "" "$(cov "$W" 0xf)"
  check_eq "...a later float covers an earlier one" "0x2" "$(jq -r '.[] | select(.address == "0xf") | .cover[0].address // ""' <<<"$(lib win_annotate "$(jq -c 'map(if .address == "0x2" then .at = [150, 150] else . end)' <<<"$c")" "$m" "$act")")"
  check_eq "...and not the other way" "" "$(cov "$W" 0x2)"
  check_eq "another workspace is off screen" false "$(on "$W" 0xc)"
  check_eq "an inactive group tab is off screen" false "$(on "$W" 0xg)"
  check_eq "...and covers nothing" "" "$(cov "$W" 0xb)"
  check_eq "a hidden special workspace is off screen" false "$(on "$W" 0xd)"
  check_eq "a shown one is on screen" true "$(on "$Wsp" 0xd)"
  check_eq "...above the workspace under it" "0xf 0x2 0xd" "$(cov "$Wsp" 0xa)"
  local Wfs; Wfs=$(lib win_annotate "$(jq -c 'map(if .address == "0xa" then .fullscreen = 1 else . end)' <<<"$c")" "$m" "$act")
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
  check_eq "click --window, ambiguous: exit 2 too" 2 "$(ob click -b "$B" --window foot 10 10 >/dev/null 2>&1; echo $?)"
  check_eq "click --window, no match: exit 2" 2 "$(ob click -b "$B" --window 0xdead 10 10 >/dev/null 2>&1; echo $?)"
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
  # --fit and --in on the screen
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
  # -g inside --window (#27): in the window's coordinates, and --in maps from the crop's origin.
  err=$(ob shot -b "$B" -w 'title:^C$' -g "10,20,100,50" -o "$o/new/dir/cg.png" 2>&1 >/dev/null)
  check_match "shot -w -g: the crop's size (X,Y,W,H taken too; -o's folder made)" "100 x 50," "$(file "$o/new/dir/cg.png")"
  check_match "...stderr says where in the window" "100x50 of it at 10,20" "$err"
  ob click -b "$B" --in "$o/new/dir/cg.png" 5 6 >/dev/null 2>&1
  check_eq "click --in a window crop: from the crop's origin in the window" "715, 326" "$(pos)"
  check_match "-g outside the window refused" "not inside window" "$(ob shot -b "$B" -w 'title:^C$' -g "390,10 20x20" 2>&1)"
  # pointer --window (#25): moves in the window's coordinates; --hold is omabox's own.
  ob pointer -b "$B" --window 'title:^C$' -- move 3 4 >/dev/null 2>&1
  check_eq "pointer --window: window coordinates" "703, 304" "$(pos)"
  check_match "pointer --hold refused" "omabox's own" "$(ob pointer -b "$B" -- --hold 2>&1)"
  # drag (#25): foot selects the text it is dragged across, into the primary selection.
  term T 'echo DRAGME-12345-67890; sleep 600'
  disp "hl.dsp.window.float({ action = 'enable', window = 'address:$(addr T)' })"
  disp "hl.dsp.window.move({ x = 50, y = 600, window = 'address:$(addr T)' })"
  until_ok 5 bash -c "'$CLI' windows -b '$B' --json | jq -e '.[] | select(.title == \"T\") | .at == [50, 600] and .cover == []'"
  ob wait -b "$B" still >/dev/null   # the move is animated: input goes where the window is drawn
  ob drag -b "$B" --window 'title:^T$' 2 10 300 10 --shot "$o/mid.png" >/dev/null 2>&1
  check_match "drag --window: the text dragged across is selected" "^DRAGME-12345" "$(ob run -b "$B" -- wl-paste -p -n 2>&1)"
  check_match "...--shot while the button was down" "PNG image" "$(file "$o/mid.png")"
  ob pointer -b "$B" --window 'title:^T$' -- move 2 10 down move 60 10 up >/dev/null 2>&1
  check_match "pointer down/up with no button: left" "^DRAGME$|^DRAGME-" "$(ob run -b "$B" -- wl-paste -p -n 2>&1)"
  check_match "drag --shot with --wait refused" "not both" "$(ob drag -b "$B" 1 1 5 5 --shot "$o/x.png" --wait 2>&1)"
  check_match "drag needs two points" "need X1 Y1 X2 Y2" "$(ob drag -b "$B" 1 1 5 2>&1)"
  ob mode -b "$B" 1280x720 >/dev/null
  check_match "--in after a mode change: refused" "mode is 1280x720@60" "$(ob click -b "$B" --in "$o/full.png" 1 1 2>&1)"
  ob down "$B" >/dev/null
}

# Travel (#38, finding 111) and --mod (#25, finding 112): the pure parts and what is refused before
# any box is asked.
t_unit_pointer() {
  check_eq "steps_to: N moves, the last on the target" "move 3 7 move 6 14 move 10 21" "$(lib eval 'SEQ=(); steps_to 0 0 10 21 3; echo "${SEQ[*]}"')"
  check_eq "...leftwards and up too" "move 5 5 move 0 0" "$(lib eval 'SEQ=(); steps_to 10 10 0 0 2; echo "${SEQ[*]}"')"
  check_eq "--mod: names, any case, each once, in order given" "ctrl+shift+super" "$(lib mods_add "" Control,shift+CTRL+win)"
  check_eq "...added to earlier ones" "alt+ctrl" "$(lib mods_add alt ctrl+alt)"
  check_match "...not a modifier: refused" "'hyper' is not a modifier" "$(lib mods_add "" ctrl+hyper 2>&1)"
  check_match "click --steps 0 refused" "--steps is 1-999" "$(ob click -b "$P-x" --steps 0 1 1 2>&1)"
  check_match "pointer --steps 1000 refused" "--steps is 1-999" "$(ob pointer -b "$P-x" --steps 1000 -- move 1 1 2>&1)"
  check_match "click --mod junk refused" "not a modifier" "$(ob click -b "$P-x" --mod meta 1 1 2>&1)"
  check_match "drag --mod junk refused" "not a modifier" "$(ob drag -b "$P-x" --mod ctrl,x 1 1 2 2 2>&1)"
  check_match "keys -m is omabox's own" "click, drag or pointer --mod" "$(ob keys -b "$P-x" -m ctrl a 2>&1)"
}

# Travel and modifier clicks in a box (findings 111, 112): two floating terminals side by side on an
# empty desktop, the pointer resting on the left one (L). A jump past the right one (R) leaves focus
# on L; travel in steps passes over R, and focus follows the mouse (Omarchy's input:follow_mouse = 1)
# on the way. R reports what its pointer does (foot's SGR mouse mode 1003: ESC[<CODE;X;YM for a
# motion, CODE 32 and up, or a press, below that, with ctrl 16 and alt 8 added; a release ends in m;
# shift is foot's own, never reported).
t_pointer() {
  local B=$P-ptr
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  term() { ob run -b "$B" -d -- foot -T "$1" sh -c "$2" >/dev/null; until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.title == \"$1\")'" >/dev/null; }
  addr() { ob hyprctl -b "$B" -j clients | jq -r --arg t "$1" '.[] | select(.title == $t) | .address'; }
  place() {   # place TITLE X Y: floating, 500x400 at X,Y
    local a; a=$(addr "$1")
    ob hyprctl -b "$B" dispatch "hl.dsp.window.float({ action = 'enable', window = 'address:$a' })" >/dev/null
    ob hyprctl -b "$B" dispatch "hl.dsp.window.resize({ x = 500, y = 400, window = 'address:$a' })" >/dev/null
    ob hyprctl -b "$B" dispatch "hl.dsp.window.move({ x = $2, y = $3, window = 'address:$a' })" >/dev/null
    until_ok 5 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.address == \"$a\") | .at == [$2, $3] and .size == [500, 400]'" >/dev/null
  }
  active() { ob hyprctl -b "$B" -j activewindow | jq -r '.title // ""'; }
  pos() { ob hyprctl -b "$B" cursorpos; }
  reports() { ob run -b "$B" -- cat /tmp/mouse | grep -ao '<[0-9;]*M' | sed 's/^<\([0-9]*\);.*/\1/'; }   # (M: not releases)
  motions() { reports | awk '$1 >= 32' | wc -l; }
  presses() { reports | awk '$1 < 32' | tr '\n' ' '; }
  term L 'sleep 600'
  term R 'printf "\033[?1003h\033[?1006h"; stty raw -echo; exec cat > /tmp/mouse'
  place L 100 300; place R 700 300
  ob wait -b "$B" still >/dev/null   # the moves are animated: input goes where the windows are drawn
  check_eq "follow_mouse is on (what travel is for)" 1 "$(ob hyprctl -b "$B" -j getoption input:follow_mouse | jq .int)"
  ob pointer -b "$B" -- move 350 500 >/dev/null
  check_eq "resting on L focuses it" L "$(active)"
  local m0; m0=$(motions)
  ob pointer -b "$B" -- move 1500 500 >/dev/null
  check_eq "a jump past R to the empty desktop: the focus stays on L" L "$(active)"
  check_eq "...and R saw no motion" "$m0" "$(motions)"
  ob pointer -b "$B" -- move 350 500 >/dev/null
  ob pointer -b "$B" --steps 20 -- move 1500 500 >/dev/null
  check_eq "pointer --steps: the way passes over R, which takes the focus" R "$(active)"
  check "...and R saw the pointer cross it (hover)" test "$(motions)" -gt "$m0"
  check_eq "...ending on the target" "1500, 500" "$(pos)"
  ob pointer -b "$B" -- move 350 500 move 350 900 move 1500 900 --steps 10 >/dev/null
  check_eq "a path around R (move A move B --steps N): the focus stays on L" L "$(active)"
  ob pointer -b "$B" -- move 350 500 move 1500 500 --steps 20 >/dev/null
  check_eq "one move's own --steps crosses R" R "$(active)"
  ob pointer -b "$B" -- move 350 500 >/dev/null
  ob click -b "$B" --steps 20 1500 500 >/dev/null
  check_eq "click --steps travels there too" R "$(active)"
  # Modifiers held across the click (#25).
  ob click -b "$B" --window R 50 50 >/dev/null
  ob click -b "$B" --window R 50 50 --mod ctrl >/dev/null
  ob click -b "$B" --window R 50 50 --mod alt --mod ctrl >/dev/null
  ob pointer -b "$B" --window R --mod ctrl -- move 60 60 click >/dev/null
  ob drag -b "$B" --window R 50 50 150 50 --mod ctrl >/dev/null
  ob click -b "$B" --window R 50 50 --mod super >/dev/null
  ob click -b "$B" --window R 50 50 >/dev/null
  check_eq "click, --mod ctrl, alt+ctrl, pointer and drag --mod ctrl, --mod super (Hyprland's move bind: not for the app), then none held" \
    "0 16 24 16 16 0 " "$(presses)"
  check_eq "...SUPER+click moved nothing (no motion)" "700 300" "$(ob hyprctl -b "$B" -j clients | jq -r '.[] | select(.title == "R") | "\(.at[0]) \(.at[1])"')"
  # Never left down: a pause that times out lets go; the tool killed outright leaves them down until
  # the next keyboard event, which omabox then sends.
  local out
  out=$(ob run -b "$B" -- sh -c 'sleep 2 | /opt/omabox/bin/omabox-keyboard -m ctrl -p 300; echo "rc=$?"' 2>&1)
  check_match "a pause with nothing on stdin times out, letting go" "letting go.*rc=1" "$(tr '\n' ' ' <<<"$out")"
  check_eq "-p and -T together refused" 2 "$(ob run -b "$B" -- /opt/omabox/bin/omabox-keyboard -m ctrl -p 300 -T </dev/null >/dev/null 2>&1; echo $?)"
  ob click -b "$B" --window R 50 50 >/dev/null
  check_match "...after it, a click has no modifier" " 0 $" "$(presses)"
  tree() { local c; for c in $(pgrep -P "$1"); do echo "$c"; tree "$c"; done; }
  ob pointer -b "$B" --window R --mod ctrl -- move 50 50 sleep 3000 click >/dev/null 2>"$TMP/ptr.err" & local cp=$! kp="" i
  for i in $(seq 50); do
    for kp in $(tree "$cp"); do [ "$(cat "/proc/$kp/comm" 2>/dev/null)" = omabox-keyboard ] && break; kp=""; done
    [ -z "$kp" ] || break; sleep 0.2
  done
  if [ -n "$kp" ]; then
    kill -KILL "$kp"
    wait "$cp"; check_eq "the keyboard killed mid-click: omabox exits 1" 1 $?
    check_match "...saying it let go early" "let go of ctrl before the end" "$(cat "$TMP/ptr.err")"
    check_match "...the click in that run still had ctrl (SIGKILL cannot be caught)" " 16 $" "$(presses)"
    ob click -b "$B" --window R 50 50 >/dev/null
    check_match "...and the next click has none: omabox cleared it" " 0 $" "$(presses)"
  else wait "$cp"; no "pointer --mod: its keyboard tool found (to kill it)"; fi
  ob down "$B" >/dev/null
}

# omabox wait and --wait (finding 82): the pure parts, and the tool's refusal outside a box, checked
# in a bare namespace with no /opt/omabox and no display (where a broken check could reach nothing).
t_unit_wait() {
  # -g (#27): grim's form, and the ones agents type.
  check_eq "-g X,Y WxH" "1 2 30 40" "$(lib geom_parse "1,2 30x40")"
  check_eq "-g X,Y,W,H" "1 2 30 40" "$(lib geom_parse "1,2,30,40")"
  check_eq "-g X,Y W,H" "-1 2 30 40" "$(lib geom_parse "-1,2 30,40")"
  check_fails "-g junk refused" lib geom_parse "1,2 0x4"
  check_eq "300ms" 300 "$(lib ms_duration 300ms)"
  check_eq "1.5s" 1500 "$(lib ms_duration 1.5s)"
  check_eq "2m" 120000 "$(lib ms_duration 2m)"
  check_eq "a bare number is seconds" 10000 "$(lib ms_duration 10)"
  check_fails "0 refused" lib ms_duration 0
  check_fails "junk refused" lib ms_duration 2h
  check_eq "the cursor's rectangle" "944,524,64,64" "$(lib cursor_rect "960 540")"
  check_match "wait --timeout over 10 min refused" "at most 10m" "$(ob wait -b "$P-x" --timeout 11m still 2>&1)"
  check_match "keys --timeout over 10 min refused" "at most 10m" "$(ob keys -b "$P-x" --wait --timeout 601s a 2>&1)"
  check_match "keys --quiet over 10 min refused" "--quiet is at most 10m" "$(ob keys -b "$P-x" --wait --quiet 20m a 2>&1)"
  check_match "click --start over 10 min refused" "--start is at most 10m" "$(ob click -b "$P-x" --wait --start 11m 1 1 2>&1)"
  check_match "wait --quiet over 10 min refused" "--quiet is at most 10m" "$(ob wait -b "$P-x" still --quiet 11m 2>&1)"
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
  # Several windows match (A and R): any is an answer, each named (#37).
  out=$(ob wait -b "$B" window foot); rc=$?
  check_eq "several windows match: satisfied (#37)" 0 "$rc"
  check_match "...each named" '^satisfied: window foot after [0-9.]+s: 2 windows: 0x[0-9a-f]+ foot "[AR]" at .*; 0x[0-9a-f]+ foot "[AR]" at ' "$out"
  out=$(ob wait -b "$B" window foot --focused --json); rc=$?
  check_eq "...--focused: the focused one of them" "0 satisfied" "$rc $(jq -r .result <<<"$out")"
  check_match "...named, with how many match" 'foot "[AR]" .*, focused \(1 of 2 matching\)$' "$(jq -r .detail <<<"$out")"
  check_eq "...--gone: 124 while they are there" 124 "$(ob wait -b "$B" --timeout 500ms window foot --gone >/dev/null; echo $?)"
  check_eq "...an action on one window still refuses several: exit 2" 2 "$(ob shot -b "$B" -w foot >/dev/null 2>&1; echo $?)"
  # cmd: a condition inside the box, here one that becomes true a second later
  ob run -b "$B" -d -- sh -c 'sleep 1; touch /tmp/late' >/dev/null 2>&1
  out=$(ob wait -b "$B" cmd -- test -e /tmp/late); rc=$?
  check_eq "wait cmd: 0 once it succeeds" 0 "$rc"
  check_match "...after it did" "^satisfied: cmd after (0\.[5-9]|[1-9])" "$out"
  check_eq "wait cmd that keeps failing: 124" 124 "$(ob wait -b "$B" --timeout 500ms cmd -- false >/dev/null; echo $?)"
  # A region off the screen watches nothing: unknown (1), never still
  out=$(ob wait -b "$B" still -g "5000,5000 10x10"); rc=$?
  check_eq "wait still -g off the screen: exit 1" 1 "$rc"
  check_match "...said" "^unknown: the region watched is not on the screen" "$out"
  # The box going down mid-wait is unknown (1), never satisfied
  ob wait -b "$B" still --quiet 30s > "$TMP/wait.out" 2>&1 & local w=$!
  sleep 1.5; ob down "$B" >/dev/null 2>&1
  wait "$w"; rc=$?
  check_eq "the box went down during wait: exit 1" 1 "$rc"
  check_match "...said" "^unknown: box '$B' went down after" "$(cat "$TMP/wait.out")"
}

# run -d -q / --print-log (issue #42, finding 109) and run -d --replace (issue #29, finding 110).
t_replace() {
  local B=$P-rep out err rc D old new
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  D=$(ob path "$B")
  out=$(ob run -b "$B" -d -q --print-log --wait -- foot -T R sh -c 'sleep 600' 2>"$TMP/rep.err"); rc=$?
  check_eq "run -d -q --print-log --wait: settled" 0 "$rc"
  check_eq "...-q: nothing on stderr" "" "$(cat "$TMP/rep.err")"
  check_match "...the log path first on stdout, then the wait's line" "^$D/home/run-[0-9]+\.log"$'\n'"satisfied: " "$out"
  check "...that log exists" test -f "$(head -1 <<<"$out")"
  old=$(ob windows -b "$B" --json | jq -r '.[] | select(.title == "R") | .pid')
  check "...its job is recorded" test -s "$D/jobs/$old"
  out=$(ob run -b "$B" -d --replace --wait -- foot -T R sh -c 'sleep 600' 2>"$TMP/rep.err"); rc=$?; err=$(cat "$TMP/rep.err")
  check_eq "--replace --wait: settled" 0 "$rc"
  check_match "...said: the earlier job stopped, its window gone" "stopped the earlier job \(1 window\)" "$err"
  check_match "...and the new one's log" "started in box '$B' \(log: " "$err"
  new=$(ob windows -b "$B" --json | jq -r '[.[] | select(.title == "R") | .pid] | join(" ")')
  check "...one window R, a new process ($old -> $new)" test -n "$new" -a "$new" != "$old" -a "${new// /}" = "$new"
  check_fails "...the old record is gone" test -e "$D/jobs/$old"
  # The issue's race: the old window still starting when the new launch comes
  ob run -b "$B" -d -q -- foot -T S sh -c 'sleep 600'
  ob run -b "$B" -d -q --replace --wait -- foot -T S sh -c 'sleep 600' >/dev/null
  check_eq "replaced while still starting: one window S" 1 "$(ob windows -b "$B" --json | jq '[.[] | select(.title == "S")] | length')"
  # A launcher that exits and leaves its app: the app is the job's (its session)
  ob run -b "$B" -d -q -- sh -c 'foot -T L sh -c "sleep 600" & exit'
  ob wait -b "$B" window 'title:^L$' >/dev/null
  old=$(ob windows -b "$B" --json | jq -r '.[] | select(.title == "L") | .pid')
  ob run -b "$B" -d -q --replace --wait -- sh -c 'foot -T L sh -c "sleep 600" & exit' >/dev/null
  new=$(ob windows -b "$B" --json | jq -r '[.[] | select(.title == "L") | .pid] | join(" ")')
  check "a launcher's app replaced ($old -> $new)" test -n "$new" -a "$new" != "$old" -a "${new// /}" = "$new"
  # Only what run -d started: the same command started otherwise is left alone
  ob run -b "$B" -- setsid -f sleep 601
  ob run -b "$B" -d -q -- sleep 601
  ob run -b "$B" -d -q --replace -- sleep 601
  check_eq "...one sleep 601 not run -d's, one new: both there" 2 "$(ob run -b "$B" -- pgrep -cxf 'sleep 601')"
  # Nothing to replace: said; a job that ended: its record dropped
  err=$(ob run -b "$B" -d --replace -- sleep 602 2>&1 >/dev/null)
  check_match "no earlier job: said" "no earlier job of this command" "$err"
  ob run -b "$B" -d -q -- sh -c 'sleep 0.3'; sleep 0.6
  old=$(grep -l '"sleep 0.3"' "$D"/jobs/*)
  err=$(ob run -b "$B" -d --replace -- sh -c 'sleep 0.3' 2>&1 >/dev/null)
  check_match "a job that exited: nothing to replace" "no earlier job of this command" "$err"
  check "...its record dropped (${old##*/})" test -n "$old" -a ! -e "$old"
  # A job that ignores SIGTERM: killed after 5 s
  ob run -b "$B" -d -q -- sh -c 'trap "" TERM; sleep 603'
  local t0=$SECONDS
  err=$(ob run -b "$B" -d --replace -- sh -c 'trap "" TERM; sleep 603' 2>&1 >/dev/null); rc=$?
  check_eq "SIGTERM ignored: replaced all the same" 0 "$rc"
  check_match "...killed, said" "ignored SIGTERM for 5 s: killed" "$err"
  check "...after about 5 s ($((SECONDS - t0)) s)" test $((SECONDS - t0)) -ge 4 -a $((SECONDS - t0)) -le 9
  check_eq "...one sleep 603 left" 1 "$(ob run -b "$B" -- pgrep -cxf 'sleep 603')"
  # A missing command still says so in its log
  out=$(ob run -b "$B" -d -q --print-log -- omabox-no-such-command)
  check "a missing command: said in its log" until_ok 5 grep -q 'setsid: failed to execute omabox-no-such-command' "$out"
  ob down "$B" >/dev/null 2>&1
}

# lua, log and events (issues #40, #41, #39): what needs no box.
t_unit_inspect() {
  check_match "lua: nothing to evaluate" "nothing to evaluate" "$(ob lua -b "$P-x" ' ' 2>&1)"
  # No EXPR is refused, not read from stdin: an open stdin would otherwise hang it (and this suite did).
  # The stdin is held open by a sleep killed right after: with `sleep 30 |` the capture (and with a
  # bare process substitution the suite's end) waited out the sleep, 30 s every run.
  local in sl; exec {in}< <(exec sleep 30); sl=$!
  check_match "lua: no EXPR is refused, stdin left alone" "nothing to evaluate" "$(timeout 10 "$CLI" lua -b "$P-x" <&"$in" 2>&1)"
  exec {in}<&-; kill "$sl" 2>/dev/null
  check_match "lua: a source starting with - goes after --" "goes after --" "$(ob lua -b "$P-x" -1 2>&1)"
  check "lua is a jailed agent's, as hyprctl is" lib broker_check lua
  check_match "log: an unknown log named, with the ones there are" "no log called nope \(hyprland shell" "$(ob log -b "$P-x" nope 2>&1)"
  check_match "log -n takes a number or all" "-n takes a number" "$(ob log -b "$P-x" -n 5x 2>&1)"
  check_match "log --grep: a bad expression said" "takes a regular expression" "$(ob log -b "$P-x" --grep '(' 2>&1)"
  check "log is a jailed agent's (its own boxes only: select_box)" lib broker_check log
  check_eq "events --since OFFSET" "123 0" "$(lib events_since 123)"
  check_match "events --since 30s: from then on" "^0 [0-9]{13}$" "$(lib events_since 30s)"
  mkdir -p "$TMP/ev"; printf 'm1 10\nm2 20\nm1 30\n' > "$TMP/ev/events.marks"
  check_eq "events --since MARK: its latest offset" "30 0" "$(D=$TMP/ev NAME=x lib events_since m1)"
  check_match "...one there is not, said" "no mark 'm9'" "$(D=$TMP/ev NAME=x lib events_since m9 2>&1)"
  check_match "events --since junk refused" "takes a mark's name" "$(lib events_since '-x' 2>&1)"
  check_match "events --mark: a tame name" "starts with a letter" "$(ob events -b "$P-x" --mark '9;x' 2>&1)"
  check_match "events --mark goes alone" "goes alone" "$(ob events -b "$P-x" --mark m --grep x 2>&1)"
  check_match "events --until or -f" "not both" "$(ob events -b "$P-x" --until x -f 2>&1)"
  check_match "events: a bad expression said" "not a regular expression" "$(ob events -b "$P-x" --until '(' 2>&1)"
  check "events is a jailed agent's (its own boxes only)" lib broker_check events
  local out rc=0
  out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --tmpfs /tmp --die-with-parent \
        env -i "$ROOT/tools/events/omabox-events" /tmp/ev.log 2>&1) || rc=$?
  check_eq "omabox-events refuses outside a box" 2 "$rc"
  check_match "...and says so" "only runs inside an omabox box" "$out"
}

# lua, log and events (issues #40, #41, #39) in a box of their own.
t_inspect() {
  local B=$P-insp out rc
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  # lua (finding 106): the value, not hyprctl's "ok".
  check_eq "lua: an expression's value" 2 "$(ob lua -b "$B" '1 + 1')"
  check_eq "lua: statements, what they return, one line each" $'3\nb' "$(ob lua -b "$B" 'local a = 3; return a, "b"')"
  check_eq "lua: a table as JSON" '{"a":[1,2],"b":true}' "$(ob lua -b "$B" '{a = {1, 2}, b = true}')"
  check_eq "lua: nil" nil "$(ob lua -b "$B" 'nil')"
  check_eq "lua: a statement returning nothing prints nothing" "" "$(ob lua -b "$B" 'omabox_t = 1')"
  check_eq "...the global is there for the next call" 1 "$(ob lua -b "$B" 'omabox_t')"
  check_eq "lua: brackets, quotes and a newline in the source are not quoting" $'x]]\n]=]"\'' "$(ob lua -b "$B" '"x]]\n]=]\"'"'"'"')"
  check_eq "lua: a source that ends in ]" 5 "$(ob lua -b "$B" '({5})[1]')"
  check_eq "lua --json: strings quoted" $'"s"\n1' "$(ob lua -b "$B" --json '"s", 1')"
  check_eq "lua: a NUL survives the trip (JSON)" '"a\u0000b"' "$(ob lua -b "$B" --json '"a\0b"')"
  check_eq "lua: a Hyprland object's fields (from its stubs)" 1920 "$(ob lua -b "$B" 'hl.get_monitors()[1]' | jq .width)"
  check_match "...an object inside one is its name" '^"HL\.Workspace' "$(ob lua -b "$B" 'hl.get_monitors()[1]' | jq .active_workspace)"
  check_eq "lua: the source on stdin" 42 "$(echo 'return 40 + 2' | ob lua -b "$B" -)"
  out=$(ob lua -b "$B" 'error("boom")' 2>&1); rc=$?
  check_eq "lua: a Lua error is exit 1" 1 "$rc"
  check_eq "...with its message" "lua: lua:1: boom" "$out"
  check_match "lua: a syntax error too" "^lua: lua:1: .*near" "$(ob lua -b "$B" '1 +' 2>&1)"
  # Calls at once share nothing (no file between them).
  local i pids=(); for i in 1 2 3 4 5 6; do ob lua -b "$B" "$i * 11" > "$TMP/lua.$i" 2>&1 & pids+=($!); done
  wait "${pids[@]}"   # (not a bare wait: the suite's host watcher is a job too)
  check_eq "lua: six calls at once each get their own answer" "11 22 33 44 55 66" "$(cat "$TMP"/lua.[1-6] | paste -sd' ')"
  # events (finding 108): recorded from the box's start, stamped, marks as byte offsets.
  local E; E=$(ob path -b "$B")/home/events.log
  check_match "events: recorded from the start" '^[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{3} omabox>>listening$' "$(ob events -b "$B" | head -1)"
  check "...each line stamped in the file" bash -c "! grep -qvE '^[0-9]+\.[0-9]{3} [^ ]' '$E'"
  local m1; m1=$(ob events -b "$B" --mark m1 2>/dev/null)
  check_match "events --mark prints the offset" '^[0-9]+$' "$m1"
  ob run -b "$B" -d --wait -- foot -T evt sleep 600 >/dev/null 2>&1
  check_match "events --since MARK --grep --json" ',evt$' "$(ob events -b "$B" --since m1 --grep '^openwindow>>' --json | jq -r .data)"
  check_eq "...--since OFFSET is the same" "$(ob events -b "$B" --since m1)" "$(ob events -b "$B" --since "$m1")"
  check_fails "...nothing from before the mark" bash -c "'$CLI' events -b '$B' --since m1 | grep -q 'omabox>>listening'"
  ob events -b "$B" --mark m2 >/dev/null 2>&1
  (sleep 1; ob hyprctl -b "$B" dispatch "hl.dsp.focus({ workspace = '4' })" >/dev/null) & local d=$!
  out=$(ob events -b "$B" --until '^workspace>>4$' --timeout 5s); rc=$?
  wait "$d"
  check_eq "events --until: an event to come, exit 0" 0 "$rc"
  check_match "...printed" ' workspace>>4$' "$out"
  check_match "events --since MARK --until: one that came already" ' workspacev2>>4,4$' "$(ob events -b "$B" --since m2 --until '^workspacev2>>' --timeout 1s)"
  check_eq "events --until none in time: 124" 124 "$(ob events -b "$B" --until '^nothing' --timeout 500ms >/dev/null 2>&1; echo $?)"
  check_match "events --since 1m" 'workspace>>4' "$(ob events -b "$B" --since 1m)"
  # Marks taken while events are written fall between lines (each is one append, never padded).
  ob run -b "$B" -d -- sh -c 'for i in $(seq 40); do hyprctl dispatch "hl.dsp.focus({ workspace = \"$((i % 3 + 1))\" })"; done' >/dev/null 2>&1
  local marks=() k; for k in 1 2 3 4 5 6 7 8; do marks+=("$(ob events -b "$B" --mark 2>/dev/null)"); done
  sleep 1
  check_eq "marks taken during a burst are at line ends" "" "$(for k in "${marks[@]}"; do [ "$k" = 0 ] || [ "$(head -c "$k" "$E" | tail -c 1 | od -An -tx1 | tr -d ' ')" = 0a ] || echo "$k"; done)"
  check_eq "...and the file has no NUL" "$(wc -c < "$E")" "$(tr -d '\0' < "$E" | wc -c)"
  "$CLI" events -b "$B" -f > "$TMP/ev.f" 2>&1 & local ef=$!   # ($CLI: $! is omabox itself, not a subshell running ob)
  sleep 1; kill -TERM "$ef"; wait "$ef" 2>/dev/null
  local bpid; bpid=$(cat "$(ob path -b "$B")/pid")
  check "events -f stopped: its tail goes too" until_ok 3 bash -c "! pgrep -f '^tail -c [+][0-9]+ -F --pid=$bpid '"
  check_match "log events: the file as it is" '^[0-9]+\.[0-9]{3} ' "$(ob log -b "$B" events -n 1)"
  "$CLI" events -b "$B" -f > "$TMP/ev2.f" 2>&1 & ef=$!
  until_ok 5 pgrep -f "^tail -c [+][0-9]+ -F --pid=$bpid " >/dev/null
  ob hyprctl -b "$B" dispatch "hl.dsp.focus({ workspace = '5' })" >/dev/null
  # log (finding 107): Hyprland's by default, the others by name, followed until the box goes.
  check_eq "log: Hyprland's, -n lines" 5 "$(ob log -b "$B" -n 5 | wc -l)"
  check_match "log --grep" "^DEBUG \]: Creating the " "$(ob log -b "$B" --grep 'creating the' -i -n 1)"
  check_match "path --logs: where each is" "^hyprland +$(ob path -b "$B")/run/hypr/[^/]+/hyprland\.log$" "$(ob path -b "$B" --logs | grep '^hyprland')"
  ob run -b "$B" -d -- sh -c 'echo omabox-log-1; sleep 1.5; echo omabox-log-2; sleep 600' >/dev/null 2>&1
  check "log run: the latest run -d's" until_ok 5 bash -c "'$CLI' log -b '$B' run | grep -qx omabox-log-1"
  ob log -b "$B" -f -n all run keyring > "$TMP/log.f" 2>&1 & local lf=$!
  check "log -f: a line that comes later" until_ok 5 grep -qx omabox-log-2 "$TMP/log.f"
  check_match "...under the log's name (several followed)" "^==> run <==" "$(head -1 "$TMP/log.f")"
  # A log the box swapped for a link to a host file: read as the box sees it, or not at all.
  echo omabox-host-secret > "$TMP/log.secret"
  ob run -b "$B" -- ln -sf "$TMP/log.secret" /home/sbx/labwc.log
  check_fails "log: a link out of the box is not followed (box up)" bash -c "'$CLI' log -b '$B' labwc 2>&1 | grep -q omabox-host-secret"
  local t0=$SECONDS
  ob run -b "$B" -- pkill -x Hyprland >/dev/null 2>&1
  wait "$lf"; rc=$?
  check_eq "log -f ends when the box goes down: exit 0" 0 "$rc"
  check "...within seconds, said" test $((SECONDS - t0)) -le 5 -a -n "$(grep "box '$B' went down" "$TMP/log.f")"
  wait "$ef"; rc=$?
  check_eq "events -f too" 0 "$rc"
  check_match "...having seen the box's events" ' workspace>>5' "$(cat "$TMP/ev2.f")"
  check_eq "log: a dead box's logs are still read" omabox-log-1 "$(ob log -b "$B" run -n 2 | head -1)"
  check_fails "...a link out of it is not followed (box down)" bash -c "'$CLI' log -b '$B' labwc 2>&1 | grep -q omabox-host-secret"
  ob down "$B" >/dev/null 2>&1
}

# --- runner --------------------------------------------------------------------------------------

UNIT=(t_unit_agent_session t_unit_clip t_unit_keys_to_box t_unit_shot_hidden t_unit_config t_unit_bar_filter t_unit_wait t_unit_pointer t_unit_window_select t_unit_guard_exec_host t_unit_live_edit t_unit_parse_mode t_unit_duration t_unit_mount_rules t_unit_refusals t_unit_run_named_dead t_unit_cli t_unit_uwsm_guard t_unit_install t_unit_host_session t_unit_guard_settings t_unit_seed_copy t_unit_version t_unit_saves
  t_unit_nvidia t_unit_aquamarine t_unit_setup t_unit_no_theme t_unit_hyprland t_unit_registry t_unit_leak_scan t_unit_jail_policy t_unit_relay t_unit_broker_units t_unit_inspect t_unit_parallel)
BOX=(t_leak_control t_main t_window t_keys_to_box t_pointer t_wait t_replace t_dbus_user_app t_agent_session t_mode_lock t_new t_keys t_peek t_guard t_uwsm_app t_widget t_throwaway t_throwaway_home t_throwaway_killed t_throwaway_dead t_isolated t_connected t_isolated_no_pidfile t_idle t_reap_race t_run_idle t_stock_bar t_saves
  t_clip t_systemd t_omarchy_restart t_held_keys t_up_again t_plugin_check t_submap_release t_setup_prompts t_omarchy_tree t_hostile t_race t_failed_up t_hyprland_dies t_pasta_dies t_other_userns t_no_new_privs t_no_shell t_hyprland t_no_git_identity t_stale_pid t_jail t_inspect)

# Box tests run in parallel (-j N; issue #60): each in a subshell of its own, its output shown whole
# when it ends. By default half the CPUs, at most one per 2 GB available and 8 (on 16 CPUs: 8, a full
# run in ~1.5 min; -j 1: ~8.5 min). No test ran slower beside 7 others than alone. These run alone first: none so far (a test that cannot share the machine,
# with a reason, goes here). In parallel, the slowest start first, so the run ends with short ones.
SERIAL=()
default_jobs() {
  local n m; n=$(($(nproc) / 2)) m=$(awk '/^MemAvailable:/ {print int($2 / 2097152)}' /proc/meminfo)
  [ "$m" -ge "$n" ] || n=$m; [ "$n" -le 8 ] || n=8; [ "$n" -ge 1 ] || n=1; echo "$n"
}
SLOW=(t_agent_session t_widget t_run_idle t_guard t_peek t_reap_race t_wait t_pointer t_clip t_replace t_idle)

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

# par_run JOBS TEST...: the tests, JOBS at a time, t_leak_control and the SLOW ones first. Each runs
# in a subshell (par_test) with counts of its own, which it hands back in a file; the runner shows its
# output whole when it ends, adds its counts and scans the host's events of its time (host_scan: the
# tests beside it share them). Then the events no test's time holds (gap_events).
par_run() {
  local jobs=$1 left=() t pid; shift
  local -A run=()
  for t in t_leak_control "${SLOW[@]}"; do [[ " $* " != *" $t "* ]] || left+=("$t"); done
  for t; do [[ " ${left[*]} " == *" $t "* ]] || left+=("$t"); done
  mkdir -p "$TMP/par"
  printf '%s == parallel\n' "$(date +%s.%3N)" >> "$EVID/host-events.log"
  while [ ${#left[@]} -gt 0 ] || [ ${#run[@]} -gt 0 ]; do
    while [ ${#left[@]} -gt 0 ] && [ ${#run[@]} -lt "$jobs" ]; do
      t=${left[0]} left=("${left[@]:1}")
      printf '%s == %s\n' "$(date +%s.%3N)" "$t" >> "$EVID/host-events.log"
      par_test "$t" > "$TMP/par/$t.out" 2>&1 &
      run[$!]=$t
    done
    wait -n -p pid "${!run[@]}"
    t=${run[$pid]}; unset "run[$pid]"
    printf '%s == /%s\n' "$(date +%s.%3N)" "$t" >> "$EVID/host-events.log"
    par_done "$t"
  done
  printf '%s == /parallel\n' "$(date +%s.%3N)" >> "$EVID/host-events.log"
  CUR=between
  local out; out=$(gap_events | leak_scan "$P-" "$HWS")
  [[ $out != *leak:* ]] || no "nothing reached the host desktop between the parallel tests" "$(grep '^leak:' <<<"$out")"
}
# par_test TEST (in its subshell): runs it with counts, notes and waits of its own; stops the servers
# it started (cleanup, in the runner, never sees them); hands back its counts, also when it stops
# midway (an exit, an unset variable), saying whether it reached its end.
par_test() {
  CUR=$1 pass=0 fail=0 failed=() skips=() SERVERS=() HELD="" LEAK_PROVEN="" PAR_T0=$SECONDS PAR_END=0
  NOTES=$TMP/par/$1.notes UNTIL=$TMP/par/$1.until
  trap par_handback EXIT
  "$CUR"; notes
  [ $((pass + fail + ${#skips[@]})) -gt 0 ] || no "the test ran checks" "none: it returned before its first"
  PAR_END=1
}
par_handback() {
  local _f=("${failed[@]}") _s=("${skips[@]}")
  [ ${#SERVERS[@]} = 0 ] || kill "${SERVERS[@]}" 2>/dev/null
  { echo "_rp=$pass _rf=$fail _rl=${LEAK_PROVEN:-} _rt=$((SECONDS - PAR_T0)) _re=$PAR_END"; declare -p _f _s; } > "$TMP/par/$CUR.res"
}
# par_done TEST (in the runner): its output, its counts, its host scan.
par_done() {
  local _rp=0 _rf=0 _rl="" _rt="?" _re=0 _f=() _s=()
  CUR=$1
  if [ -f "$TMP/par/$1.res" ]; then
    # shellcheck disable=SC1090 # written by par_test
    source "$TMP/par/$1.res"
  fi
  echo "${1#t_}"
  cat "$TMP/par/$1.out"
  echo "       (${1#t_}: ${_rt}s)"
  pass=$((pass + _rp)) fail=$((fail + _rf))
  failed+=("${_f[@]}") skips+=("${_s[@]}")
  [ -z "$_rl" ] || LEAK_PROVEN=$_rl
  [ "$_re" = 1 ] || no "the test ran to its end" "it stopped midway (an exit, an unset variable?): its output is above"
  host_scan "$1" "/$1"
}

# The runner as one function, read whole before it starts: an edit to this file during a run (another
# agent's, in the same checkout) cannot change what the rest of the run does (as finding 66 did for
# bin/omabox).
main() {
  # (Underscored: the tests run inside this function and see its locals.)
  local _a _pats=() _filtered=0 _t _p _sel=() _n _unit_n=0 _jobs=${OMABOX_TEST_JOBS:-$(default_jobs)} _next=""
  for _a; do
    if [ -n "$_next" ]; then _jobs=$_a _next=""; continue; fi
    case $_a in --strict) STRICT=1 ;; -j|--jobs) _next=1 ;; -j*) _jobs=${_a#-j} ;; *) _pats+=("$_a") ;; esac
  done
  [[ $_jobs =~ ^[1-9][0-9]?$ ]] || { echo "-j: a number of parallel tests, 1-99 (1: one at a time), got '$_jobs'"; exit 2; }
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
  # omabox's workspace coming up is a leak (from #13's watch), unless you were on it already.
  # (A special one, special:NAME, is shown over the focused monitor's workspace.)
  HWS=$("$CLI" config workspace 2>/dev/null) || HWS=9
  ! hostctl -j monitors | jq -e --arg w "$HWS" '.[] | select(.focused) | select(.activeWorkspace.name == $w or .specialWorkspace.name == $w)' >/dev/null || HWS=
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

  # One box first (finding 130): when none can start here (refused: a headless box on NVIDIA without
  # aquamarine's fix, no pasta, ...), say why once and run the unit tests only, instead of every box
  # test failing at its `up` (166 checks did, on NVIDIA from an install).
  local _box=0 _out
  for _t in "${tests[@]}"; do [[ $_t == t_unit_* ]] || _box=1; done
  if [ $_box = 1 ]; then
    if _out=$("$CLI" up "$P-probe" 2>&1); then "$CLI" down "$P-probe" >/dev/null 2>&1
    else
      echo probe; CUR=probe
      no "a box starts here" "$_out"
      echo "       (no box can start here: the box tests are not run)"
      tests=(); for _t in "${UNIT[@]}"; do [[ " ${_sel[*]:-${UNIT[*]}} " == *" $_t "* ]] && tests+=("$_t"); done
    fi
  fi

  # One at a time: the unit tests, SERIAL, and everything with -j 1. The rest in parallel after them.
  local _one=() _par=()
  for _t in "${tests[@]}"; do
    if [ "$_jobs" = 1 ] || [[ $_t == t_unit_* ]] || [[ " ${SERIAL[*]} " == *" $_t "* ]]; then _one+=("$_t"); else _par+=("$_t"); fi
  done
  local _t0
  for CUR in "${_one[@]}"; do
    _n=$((pass + fail + ${#skips[@]})) _t0=$SECONDS
    rm -f "$UNTIL"
    printf '%s == %s\n' "$(date +%s.%3N)" "$CUR" >> "$EVID/host-events.log"
    echo "${CUR#t_}"; HELD=""; "$CUR"; notes
    [[ $CUR == t_unit_* ]] || echo "       (${CUR#t_}: $((SECONDS - _t0))s)"
    [ $((pass + fail + ${#skips[@]})) -gt "$_n" ] || no "the test ran checks" "none: it returned before its first"
    [[ $CUR != t_unit_* ]] || _unit_n=$((_unit_n + pass + fail + ${#skips[@]} - _n))
    host_scan "$CUR"
  done
  [ ${#_par[@]} = 0 ] || par_run "$_jobs" "${_par[@]}"

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

# exit: after main returns, bash would read on from where it was in this file, so an edit that grew
# it during the run would run what sits there now. With it, shellcheck takes every test (called by
# name, "$CUR") for dead code: SC2329 is off for the file (finding 124).
main "$@"; exit
