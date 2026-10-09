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
#   OMABOX_TEST_OMARCHY=DIR: the Omarchy tree t_unit_omarchy_contract reads (default /usr/share/omarchy):
#                            a checkout, to see what an Omarchy update moves before it is released
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
  # What its names left in the runtime dir (#158): a test's own lock, a down that found no box.
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$UID}/omabox"/.{lock,down,expired}-"$P"-* 2>/dev/null
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
    # Reading a box is using it (need_box touches `used`, `path` too): its idle clock is put back after,
    # or other tests' idle boxes (t_idle, t_reap_race) expired late and failed too (#111, finding 180).
    # After each read, and only from that read's own touch (finding 237): a test using its box meanwhile
    # got the older time back, and its box could expire early.
    d=$e/boxes/$b; mkdir -p "$d"; bd=$XDG_RUNTIME_DIR/omabox/$b
    [ ! -e "$bd/used" ] || touch -r "$bd/used" "$d/.used"
    ev_read "$bd" "$d" timeout 10 "$CLI" shot -b "$b" -o "$d/screen.png" >/dev/null 2>&1
    for q in clients layers activewindow activeworkspace devices; do ev_read "$bd" "$d" timeout 5 "$CLI" hyprctl -b "$b" -j "$q" > "$d/$q.json" 2>&1; done
    ev_read "$bd" "$d" timeout 5 "$CLI" hyprctl -b "$b" cursorpos > "$d/cursorpos" 2>&1
    for f in "$bd/box.log" "$bd"/home/*.log "$bd"/run/hypr/*/hyprland.log "$bd/run/events.log"; do [ -f "$f" ] && tail -n 100 "$f" > "$d/${f##*/}"; done
  done
  printf '       evidence: %s\n' "$e"
}
# ev_read BOXDIR EVDIR CMD...: one of evidence's reads, then the box's `used` as it was before (kept in
# EVDIR/.used), if the time there now is the read's own: omabox touches it as it starts (need_box), a
# second at most into the read here. A later time is another use, a test's meanwhile: kept, and the time
# put back after the next reads (finding 237). (A use in that first second is still taken for the read's.)
ev_read() {
  local bd=$1 d=$2 t0=${EPOCHREALTIME//[!0-9]/} m; shift 2   # (microseconds, whatever the locale's point)
  "$@"
  [ -e "$d/.used" ] && m=$(stat -c %.6Y "$bd/used" 2>/dev/null) || return 0
  m=${m//[!0-9]/}
  if [ "$m" -le $((t0 + 1000000)) ]; then touch -r "$d/.used" "$bd/used" 2>/dev/null
  else touch -r "$bd/used" "$d/.used"; fi
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
# check_box_safety NAME [RUNNER...]: AGENTS.md's box safety invariant on box NAME (#111, finding 180):
# none of the real seat's devices, seatd or the system bus, the runtime dir its own. RUNNER is how to
# reach the box's omabox (default: this one; a box nested in a stand-in: `omabox run -b S --` and
# its omabox). For a box of this host, also nothing of your runtime dir mounted in it but its own dir
# (on the tmpfs your runtime dir is, the only mount's root is /omabox/NAME/...).
check_box_safety() {
  local b=$1; shift
  local via=("$@"); [ ${#via[@]} -gt 0 ] || via=("$CLI")
  check_fails "$b: no /dev/input" "${via[@]}" run -b "$b" -- test -e /dev/input
  check_fails "$b: no DRM card node" "${via[@]}" run -b "$b" -- sh -c 'ls /dev/dri/card* >/dev/null 2>&1'
  check_fails "$b: no seatd socket" "${via[@]}" run -b "$b" -- test -e /run/seatd.sock
  check_fails "$b: no system bus" "${via[@]}" run -b "$b" -- test -e /run/dbus/system_bus_socket
  [ $# = 0 ] || return 0
  check_eq "$b: the runtime dir is the box's own, not yours" "$(stat -c %i "$XDG_RUNTIME_DIR/omabox/$b/run")" \
    "$("$CLI" run -b "$b" -- stat -c %i "/run/user/$UID")"
  local dev; dev=$(awk -v m="$XDG_RUNTIME_DIR" '$5 == m { print $3; exit }' /proc/self/mountinfo)
  check_eq "$b: nothing of your runtime dir is mounted in it but its own dir" "" \
    "$("$CLI" run -b "$b" -- awk -v d="$dev" '$3 == d { print $4 " at " $5 }' /proc/self/mountinfo | grep -v "^/omabox/$b/")"
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
  # (The wait succeeded: the note is not the point, its failure is not the check's. Finding 252.)
  [ $((now * 200)) -le "$lim" ] || note "slow wait: $((now / 10)).$((now % 10))s of ${t}s for: $*" || true
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
KEEP = ("openwindow>>", "closewindow>>", "activewindowv2>>", "workspacev2>>", "activespecial>>", "activelayout>>",
        "openlayer>>", "closelayer>>")
def hypr(what):
    return json.loads(subprocess.run(["hyprctl", "-j", what], capture_output=True, text=True, timeout=5).stdout)
def focused():
    try:
        return describe(hypr("activewindow"))
    except Exception as e:
        return "? %s" % e
def opened(addr):   # a window just opened: whose (its process environment says who asked)
    try:
        return describe(next(w for w in hypr("clients") if w.get("address") == "0x" + addr))
    except Exception as e:
        return "0x%s ? %s" % (addr, e)
def layer(ns):      # a layer just opened (the event names only its namespace): each of that name
    try:
        ls = [l for m in hypr("layers").values() for lv in m.get("levels", {}).values() for l in lv if l.get("namespace") == ns]
        return "; ".join(describe(dict(l, **{"class": "layer:" + ns})) for l in ls) or "? none named %s" % ns
    except Exception as e:
        return "? %s" % e
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
        # Every window and layer: whose process (#111: a suite window opened silently was only a note).
        if l.startswith("openwindow>>"): say("+ " + opened(l[12:].split(",")[0]))
        if l.startswith("openlayer>>"): say("+ " + layer(l[11:]))
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
  local pre=$1 ws=${2:-} line data cls title kb notes=() peek="" other="" wsup="" prev="" who
  local wsleak="leak: omabox's workspace $ws came up (if you went there yourself, run the suite again)"
  while IFS= read -r line; do
    line=${line#* }
    # What the next line decides: a peek window's `+` line; the focus omabox's workspace brought.
    if [ -n "$peek" ] && [[ $line != '+ '* ]]; then echo "leak: a window opened: $peek"; peek=""; fi
    if [ -n "$other" ] && [[ $line != '+ '* ]]; then notes+=("$other"); other=""; fi   # (a watcher with no + line)
    if [ -n "$wsup" ] && [[ $line != '~ '* && $line != activewindowv2\>\>?* ]]; then echo "$wsleak"; wsup=""; fi
    # What the focus line just before says, for a workspace line (whose, or `leak`); gone after any other.
    [[ $line == '~ '* || $line == workspacev2\>\>* || $line == activespecial\>\>* ]] || prev=""
    case $line in
      openwindow\>\>*)
        IFS=, read -r _ _ cls title <<<"${line#*>>}"
        if [ "$cls" = aquamarine ] && data=$(their_new_box "$pre"); then notes+=("window of your box $data")
        elif data=$(omabox_window "$pre" "$cls" "$title"); then
          if [ "$cls" = omabox-peek ]; then peek=$data; else echo "leak: a window opened: $data"; fi
        else other="window $cls"; fi ;;   # its + line says whose (#111)
      openlayer\>\>*) other="layer ${line#*>>}" ;;
      '+ '*)
        # Any other window or layer: the suite's process or a box's of this run is a leak, opened on
        # your desktop however quietly (no focus, a silent workspace); anything else is yours, a note.
        if [ -n "$other" ]; then
          if [[ " $line " == *" OMABOX_SUITE=${pre%-} "* || $line == *" OMABOX_NAME=$pre"* ]]; then
            echo "leak: a ${other%% *} opened, of this run's: ${line#+ }"
          else notes+=("$other"); fi
          other=""; continue
        fi
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
  [ -z "$other" ] || notes+=("$other")
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
  # finding 228: what is inside ~/.config/omarchy, as real files in a fake HOME (a missing path passed
  # here once for the wrong reason). Only plugins/ and themes/; seed_home's look (branding, extensions).
  local fh=$TMP/mr-home o; o=$fh/.config/omarchy
  # shellcheck disable=SC2329 # called below
  fhlib() { env HOME="$fh" bash -c 'source "$1"; shift; "$@"' lib "$TMP/lib/bin/omabox" "$@"; }
  mkdir -p "$o/plugins/x.y" "$o/themes/t" "$o/hooks" "$o/branding" "$fh/proj/fixtures"
  echo fake > "$o/api-keys.env"; echo 'echo hi' > "$o/hooks/post-update"
  check_match "a file inside ~/.config/omarchy refused (api-keys.env)" "it is inside .*/.config/omarchy/" "$(fhlib refuse_src "$o/api-keys.env")"
  check "...a hook too" fhlib refuse_src "$o/hooks/post-update"
  check "...branding, outside seed_home's look" fhlib refuse_src "$o/branding"
  check_fails "...in seed_home's look it is fine" fhlib refuse_src "$o/branding" look
  check_fails "...a plugin dir under it is fine" fhlib refuse_src "$o/plugins/x.y"
  check_fails "...a theme dir too" fhlib refuse_src "$o/themes/t"
  # ...and by the path as given: a link there (or in a secret store) to a file that would be fine.
  mkdir -p "$fh/dots" "$fh/.ssh"; echo fake > "$fh/dots/keys.env"; echo fake > "$fh/dots/id"
  ln -s ../../dots/keys.env "$o/linked-keys.env"; ln -s ../dots/id "$fh/.ssh/id_linked"
  check_match "a link inside ~/.config/omarchy to a dotfiles file refused" "it is inside .*/.config/omarchy/" "$(fhlib refuse_src "$o/linked-keys.env")"
  check_match "...a link in ~/.ssh too" "it is inside .*/.ssh/" "$(fhlib refuse_src "$fh/.ssh/id_linked")"
  check_match "...given relative, as named" "it is inside .*/.config/omarchy/" "$(cd "$o" && fhlib refuse_src linked-keys.env)"
  check_fails "...what they point at, named itself, is fine" fhlib refuse_src "$fh/dots/keys.env"
  mv "$o" "$fh/omarchy-real"; ln -s ../omarchy-real "$o"   # ~/.config/omarchy a dotfiles link
  check "...through a dotfiles link, api-keys.env still refused" fhlib refuse_src "$fh/omarchy-real/api-keys.env"
  check "...and the dir that holds it" fhlib refuse_src "$fh/omarchy-real"
  check_fails "...a plugin there still fine" fhlib refuse_src "$fh/omarchy-real/plugins/x.y"
  # A jailed caller's relative --seed SRC goes absolute (the broker runs in /); DEST is the box's.
  printf '#!/bin/sh\nprintf "%%s|" "$@"\n' > "$TMP/mr-relay"; chmod +x "$TMP/mr-relay"
  local out; out=$(cd "$fh/proj" && bash -c 'source "$1"; RELAY=$2; relay_call up --seed fixtures:.config/x --seed nowhere:y' _ "$TMP/lib/bin/omabox" "$TMP/mr-relay")
  check_match "relay: a relative --seed SRC goes absolute, DEST as given" "\|--seed\|$fh/proj/fixtures:\.config/x\|" "$out"
  check_match "...one that is not there as given, for the broker to say so" "\|--seed\|nowhere:y\|" "$out"
  out=$(cd "$fh" && bash -c 'source "$1"; RELAY=$2; relay_call up --theme-dir proj' _ "$TMP/lib/bin/omabox" "$TMP/mr-relay")
  check_match "relay: a relative --theme-dir goes absolute (#170)" "\|--theme-dir\|$fh/proj\|" "$out"
  # #98: omabox's own saves and box HOMEs, and other tools' tokens
  local x
  for x in .local/share/omabox .local/share/omabox/saves .cache/omabox .config/gh .config/gcloud .azure \
           .config/op .git-credentials .config/git/credentials; do
    check "HOME/$x refused" lib refuse_src "$HOME/$x"
  done
  check_fails "HOME/code is fine" lib refuse_src "$HOME/code"
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
  # --seed SRC:DEST (#80): DEST is in the box HOME, however written; never above it.
  check_eq "seed: DEST relative to the box HOME" "$ROOT/VERSION"$'\t'".config/x/v" "$(lib seed_spec "$ROOT/VERSION:.config/x/v")"
  check_eq "seed: ~/ and /home/sbx/ are the box HOME" ".config/v .config/v" \
    "$(lib seed_spec "$ROOT/VERSION:~/.config/v" | cut -f2) $(lib seed_spec "$ROOT/VERSION:/home/sbx/.config/v" | cut -f2)"
  check_match "seed: .. out of the HOME refused" "must be inside the box HOME" "$(lib seed_spec "$ROOT/VERSION:a/../../x" 2>&1)"
  check_match "seed: another absolute DEST refused" "DEST is in the box HOME" "$(lib seed_spec "$ROOT/VERSION:/etc/x" 2>&1)"
  check_match "seed: the HOME itself refused" "must be inside the box HOME" "$(lib seed_spec "$ROOT/VERSION:~/" 2>&1)"
  check_match "seed: no DEST refused" "SRC:DEST" "$(lib seed_spec "$ROOT/VERSION" 2>&1)"
  # --monitor SPEC (#122)
  check_eq "monitor: WxH, at 60, scale 1, right" "1080x1920@60 1 right" "$(lib monitor_spec 1080x1920)"
  check_eq "monitor: rate, scale, below" "2560x1440@144 1.6 below" "$(lib monitor_spec 2560x1440@144,scale=1.6,below)"
  check_eq "monitor: X,Y" "800x600@60 1 0,1080" "$(lib monitor_spec 800x600,0,1080)"
  check_eq "monitor: X,Y then scale" "800x600@60 2 100,0" "$(lib monitor_spec 800x600,100,0,scale=2)"
  local bad; for bad in 1080 0x100 1080x1920,scale=5 1080x1920,scale=0.5 1080x1920,scale=1.234 1080x1920,left 1080x1920,above 1080x1920,5 1080x1920,up ""; do
    check_fails "monitor: '$bad' refused" lib monitor_spec "$bad"
  done
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
  # A fake HOME's real files (finding 228): a path that is not there is refused for that alone.
  local fh=$TMP/rf-home; mkdir -p "$fh/.ssh" "$fh/.config/omarchy/hooks" "$fh/.local/state/omarchy/current/theme"
  echo fake > "$fh/.ssh/id_test"; echo fake > "$fh/.config/omarchy/api-keys.env"; echo 'echo hi' > "$fh/.config/omarchy/hooks/post-update"
  check_match "--seed of a secret store refused" "refusing to seed $fh/.ssh/id_test into a box: it is inside $fh/.ssh/" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --seed "$fh/.ssh/id_test:x" 2>&1)"
  check_match "--seed of api-keys.env refused" "refusing to seed $fh/.config/omarchy/api-keys.env into a box: it is inside" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --seed "$fh/.config/omarchy/api-keys.env:x" 2>&1)"
  ln -s "$TMP/rf-key" "$fh/.ssh/id_linked"; echo fake > "$TMP/rf-key"
  check_match "--seed of a link in a secret store refused, as named" "refusing to seed $fh/.ssh/id_linked into a box: it is inside $fh/.ssh/" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --seed "$fh/.ssh/id_linked:x" 2>&1)"
  check_match "--ro-bind of a hook's folder refused" "refusing to mount $fh/.config/omarchy/hooks into a box: it is inside" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --ro-bind "$fh/.config/omarchy/hooks:/mnt/h" 2>&1)"
  # --theme-dir (#170): refused as --ro-bind is, as named and where it leads; a theme's own name.
  mkdir -p "$fh/.config/omarchy/themes/t" "$fh/.gnupg/t" "$TMP/rf-td/a/t" "$TMP/rf-td/b/t" "$TMP/rf-td/.hidden"
  ln -s "$fh/.gnupg/t" "$TMP/rf-td/linked"
  check_match "--theme-dir of a hook's folder refused" "refusing to mount $fh/.config/omarchy/hooks into a box: it is inside" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --theme-dir "$fh/.config/omarchy/hooks" 2>&1)"
  check_match "...of a dir in a secret store" "refusing to mount $fh/.gnupg/t into a box: it is inside $fh/.gnupg/" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --theme-dir "$fh/.gnupg/t" 2>&1)"
  check_match "...of a link to one" "refusing to mount $TMP/rf-td/linked into a box: it is inside $fh/.gnupg/" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --theme-dir "$TMP/rf-td/linked" 2>&1)"
  check_match "...of ~/.config/omarchy itself" "refusing to mount $fh/.config/omarchy into a box: it contains" \
    "$(HOME=$fh ob up "$P-r30" --no-shell --theme-dir "$fh/.config/omarchy" 2>&1)"
  check_match "...of a dir that is not there" "--theme-dir: no such directory: $TMP/rf-td/none" "$(ob up "$P-r30" --theme-dir "$TMP/rf-td/none" 2>&1)"
  check_match "...of a file" "--theme-dir: no such directory" "$(ob up "$P-r30" --theme-dir "$fh/.ssh/id_test" 2>&1)"
  check_match "...two of one name" "--theme-dir: two themes named t" "$(ob up "$P-r30" --theme-dir "$TMP/rf-td/a/t" --theme-dir "$TMP/rf-td/b/t" 2>&1)"
  check_match "...a name omarchy-theme-set refuses" "cannot start with a dot" "$(ob up "$P-r30" --theme-dir "$TMP/rf-td/.hidden" 2>&1)"
  check_fails "...no box dir for any of them" test -e "$XDG_RUNTIME_DIR/omabox/$P-r30"
  # (#123) Only where it is refused: with the fix it would open a window on the real desktop.
  if aq_unfixed env OMABOX_AQUAMARINE=system "$CLI"; then
    check_match "--monitor on an interactive box without aquamarine's fix: refused" "opens a window per monitor, which needs aquamarine's fix" \
      "$(OMABOX_AQUAMARINE=system ob up "$P-r28" --interactive --monitor 800x600 2>&1)"
  fi
  check_match "--monitor with a bad SPEC refused" "a size is WxH" "$(ob up "$P-r29" --monitor big 2>&1)"
  check_fails "--seed of ~/.config/omarchy refused (api-keys.env)" ob up "$P-r20" --seed "$HOME/.config/omarchy:x"
  check_fails "--plugin HOME refused" bash -c "mkdir -p '$TMP/fakehome' && echo '{\"id\":\"x.y\"}' > '$TMP/fakehome/manifest.json' && HOME='$TMP/fakehome' '$CLI' up '$P-r18' --plugin '$TMP/fakehome'"
  # (/dev/net/tun hidden in a mount namespace of its own: pasta would fail inside the box, 10 s later)
  check_match "a connected box without /dev/net/tun is refused up front" "needs /dev/net/tun" \
    "$(unshare -Urm bash -c 'mount -t tmpfs none /dev/net && exec "$0" up "$1"' "$CLI" "$P-r19" 2>&1)"
  # (bwrap would fail at its uid map behind pasta, and only box.log would say so, 10 s later; a
  # connected box falls back to none there instead, t_no_new_privs)
  check_match "an isolated box is refused up front from a no_new_privs process" \
    "cannot start from a no_new_privs process" "$(setpriv --no-new-privs "$CLI" up "$P-r20" --net isolated 2>&1)"
  check_fails "...before its box dir is made" test -e "$XDG_RUNTIME_DIR/omabox/$P-r20"
  # #106, finding 175: what can only break the box is refused up front, named.
  check_match "--env of the box session's own refused" "--env cannot set XDG_RUNTIME_DIR: it is the box session's own" \
    "$(ob up "$P-r21" --no-shell --env XDG_RUNTIME_DIR=/nowhere 2>&1)"
  check_match "...LD_PRELOAD too" "cannot set LD_PRELOAD" "$(ob up "$P-r22" --env LD_PRELOAD=/x.so 2>&1)"
  check_match "--allow 0 refused" "ports from 1 to 65535, got 0" "$(ob up "$P-r23" --net isolated --allow 0 2>&1)"
  check_match "--allow 70000 refused" "ports from 1 to 65535, got 8081,70000" "$(ob up "$P-r24" --net isolated --allow 8081,70000 2>&1)"
  mkdir -p "$TMP/badid"; echo '{"id":"../x"}' > "$TMP/badid/manifest.json"
  check_match "a plugin whose manifest id is a path refused" "manifest id '../x' must be letters, digits" "$(ob up "$P-r25" --plugin "$TMP/badid" 2>&1)"
  echo '{"id":"a b"}' > "$TMP/badid/manifest.json"
  check_match "...or has a space" "manifest id 'a b' must be" "$(ob up "$P-r26" --plugin "$TMP/badid" 2>&1)"
  check_match "run --pass of the box session's own refused" "--pass cannot set PATH" "$(ob run -b "$P-x" --pass PATH -- true 2>&1)"
  printf 'A=1\nexport HOME=/x\n' > "$TMP/reserved.env"
  check_match "...--env-file's too, by line" "line 2 sets HOME: it is the box session's own" "$(ob run -b "$P-x" --env-file "$TMP/reserved.env" -- true 2>&1)"
  check_eq "allow_ports: sorted, each once" "22,8081,8082" "$(lib allow_ports 8082,22,8081,08081)"
  check_eq "...none for none" "" "$(lib allow_ports "")"
  check_match "a box name with characters it cannot have: said as written (#107)" "note: box name '$P-r27 x!' written as '$P-r27-x-'" \
    "$(ob up "$P-r27 x!" --net isolated --allow 0 2>&1)"
  local r; for r in 21 22 23 24 25 26 27-x-; do check_fails "...no box dir or lock for $P-r$r" test -e "$XDG_RUNTIME_DIR/omabox/$P-r$r" -o -e "$XDG_RUNTIME_DIR/omabox/.lock-$P-r$r"; done
  local left; left=$(ob ls --json | jq -r '.[].name' | grep -c "^$P-r" || true)
  check_eq "refusals left no box behind" 0 "$left"
}

# kill_box under set -e, as up's failure trap once ran it (#91, finding 157): every step runs, so
# pasta is killed and PID 1 waited for. PID 1 and pasta are two sleeps that are not this shell's
# children (a killed child stays a zombie, alive to kill -0). On the old kill_box the pkill that found
# no stopped launcher ended it, pasta left running (t_failed_up's state check could not see that).
t_unit_kill_box() {
  local one pasta out
  read -r one pasta < <(bash -c 'sleep 300 & a=$!; sleep 300 & echo "$a $!"')
  out=$(ONE=$one PASTA=$pasta bash -c 'source "$1"; NAME=kb; box_pid() { echo "$ONE"; }; box_pasta() { echo "$PASTA"; }
    kill_box; echo "ran to its end"' _ "$TMP/lib/bin/omabox" 2>&1)
  check_eq "kill_box under set -e runs every step: PID 1 and pasta killed, then waited for" \
    "ran to its end, PID 1 gone, pasta gone" \
    "$out, PID 1 $(kill -0 "$one" 2>/dev/null && echo alive || echo gone), pasta $(kill -0 "$pasta" 2>/dev/null && echo alive || echo gone)"
  kill "$one" "$pasta" 2>/dev/null || true
}

t_unit_run_named_dead() {
  check_fails "run -b on a box that is not up fails (no throwaway)" ob run -b "$P-nope" -- true
  # #115, finding 178: a reaper that exits between down's pgrep and its kill (it saw the box dead)
  # failed that kill, the last of an || list, and errexit ended `down` silently before rm_box.
  # The real cmd_down under the CLI's set -e; pgrep names a pid that is gone.
  local b=$TMP/downrace; mkdir -p "$b/racebox"; echo '{"mode":"headless"}' > "$b/racebox/box.json"
  check_eq "down: a reaper gone since pgrep is no failure, the box is cleared" $'rm_box\nomabox: box \'racebox\' down\nrc 0' \
    "$(bash -c 'source "$1"; BOXES=$2; kill_box() { :; }; peek_pid() { return 1; }; pgrep() { echo 999999999; }
      rm_box() { echo rm_box; }; cmd_down racebox; echo "rc $?"' _ "$TMP/lib/bin/omabox" "$b" 2>&1)"
}

# #166: a command through on_box or box_exec on a box going down as it entered printed nsenter's raw
# "cannot open /proc/N/ns/user", or bash's "Killed" line for nsenter, before any words (run's came
# after them; keys and the rest had none). Stand-ins for nsenter and the box; nothing runs in a box.
t_unit_box_gone() {
  local L=$TMP/lib/bin/omabox out z
  out=$(bash -c 'source "$1"; NAME=bg; box_pid() { echo 1; }; box_env() { echo x; }; box_alive() { return 1; }
    nsenter() { echo "nsenter: cannot open /proc/1/ns/user: No such file or directory" >&2; return 1; }
    on_box hyprctl version || echo "rc $?"' _ "$L" 2>&1)
  check_eq "on_box on a box gone as it entered: said in words, without nsenter's line" \
    $'omabox: box \'bg\' went down (why: omabox log -b bg all; clear it with: omabox down bg)\nrc 1' "$out"
  out=$(bash -c 'source "$1"; NAME=bg; box_pid() { echo 1; }; box_env() { echo x; }; box_alive() { return 0; }; sleep() { :; }
    nsenter() { echo "nsenter: reassociate to namespace failed: Operation not permitted" >&2; return 1; }
    on_box hyprctl version || echo "rc $?"' _ "$L" 2>&1)
  check_eq "...on a box that is up, nsenter's line as it came" $'nsenter: reassociate to namespace failed: Operation not permitted\nrc 1' "$out"
  # The command's own stderr is never held back: it is the caller's, through the fd the command takes.
  out=$(bash -c 'source "$1"; NAME=bg; box_pid() { echo 1; }; box_env() { echo x; }; box_alive() { return 1; }
    nsenter() { while [ "$1" != -- ]; do shift; done; shift; "$@"; }
    on_box sh -c "echo said >&2; exit 2" || echo "rc $?"' _ "$L" 2>&1)
  check_eq "...a command's own stderr passes, the box's end or not" $'said\nrc 2' "$out"
  mkdir -p "$TMP/boxgone/run"; : > "$TMP/boxgone/run/omabox.env"
  out=$(bash -c 'source "$1"; NAME=bg D=$2; box_pid() { echo 1; }; box_alive() { return 1; }; nocore() { "$@"; }; caller_path() { :; }
    nsenter() { case " $* " in *" -p "*) sh -c "kill -9 \$\$" ;; *) return 1 ;; esac; }
    box_exec true || echo "rc $?"' _ "$L" "$TMP/boxgone" 2>&1)
  check_eq "box_exec on a box gone: no \"Killed\" line, left to run's words" "rc 137" "$out"
  # A PID 1 that exited and is not reaped yet is no box: its namespaces are gone.
  # (Its parent execs a sleep, which never reaps it.)
  sh -c 'sleep 0 & echo $! > "$1"; exec sleep 10' _ "$TMP/boxgone-zpid" & local zp=$! i
  for i in $(seq 40); do z=$(cat "$TMP/boxgone-zpid" 2>/dev/null) && [ "$(cut -d' ' -f3 "/proc/$z/stat" 2>/dev/null)" = Z ] && break; sleep 0.05; done
  check_eq "a zombie PID 1 is not alive (nor up in ls)" "Z gone dead" \
    "$(Z=$z bash -c 'source "$1"; NAME=bg; box_pid() { echo "$Z"; }; printf "%s " "$(cut -d" " -f3 "/proc/$Z/stat")"
      box_alive && printf "alive " || printf "gone "; box_state' _ "$L" 2>&1)"
  kill "$zp" 2>/dev/null; wait "$zp" 2>/dev/null
}

# #158: the runtime dir kept a .lock- per box name ever used and a .down- per down that found none.
# down removes the lock it holds once the box is gone; lock_file opens again a lock file removed
# while it waited, so it and a newcomer never both hold one; a down sweeps markers past 10 min.
t_unit_lock_markers() {
  local b=$TMP/lockm out; mkdir -p "$b/gone"; echo '{"mode":"headless"}' > "$b/gone/box.json"
  : > "$b/.lock-gone"; : > "$b/.lock-nodir"
  out=$(bash -c 'source "$1"; BOXES=$2; kill_box() { :; }; peek_pid() { return 1; }; pgrep() { :; }
    rm_box() { rm -rf "$D"; }; cmd_down gone nodir' _ "$TMP/lib/bin/omabox" "$b" 2>&1)
  check_match "down takes the box down" "box 'gone' down" "$out"
  check_fails "...and removes its lock file" test -e "$b/.lock-gone"
  check_fails "a lock file with no box: down removes it too" test -e "$b/.lock-nodir"
  : > "$b/.down-old"; touch -d '-11 min' "$b/.down-old"; : > "$b/.down-new"
  : > "$b/.expired-old"; touch -d '-25 hours' "$b/.expired-old"; : > "$b/.expired-new"; touch -d '-23 hours' "$b/.expired-new"
  bash -c 'source "$1"; BOXES=$2; cmd_down none' _ "$TMP/lib/bin/omabox" "$b" >/dev/null 2>&1
  check_fails "a down that finds no box sweeps a .down- marker older than 10 minutes" test -e "$b/.down-old"
  check "...keeps a fresh one" test -e "$b/.down-new"
  check "...and writes its own" test -s "$b/.down-none"
  check_fails "...and an idle box's .expired- note older than a day (finding 237)" test -e "$b/.expired-old"
  check "...keeping a younger one" test -e "$b/.expired-new"
  # Finding 237: a down that finds the lock but no box writes its marker before it lets go of the lock,
  # so an `up` waiting on the lock reads it. Here the down holds it a second (its rm of the lock file
  # slowed) and its marker takes half a second to write: a waiter that gets the lock reads it, or not.
  out=$(timeout 30 bash -c 'source "$1"; BOXES=$2; f=$BOXES/.lock-raced; : > "$f"
    rm() { [ "${!#}" != "$f" ] || sleep 1; command rm "$@"; }
    proc_start() { sleep 0.5; echo 12345; }
    ( cmd_down raced ) >/dev/null 2>&1 &
    until ! flock -n "$f" true; do sleep 0.02; done
    ( lock_file w "$f" -w 10; cat "$BOXES/.down-raced" 2>/dev/null || echo "no marker" )
    wait' _ "$TMP/lib/bin/omabox" "$b" 2>&1)
  check_eq "an up waiting on the lock of a down that finds no box reads its marker once it has the lock" 12345 "$out"
  # ...and one that finds neither box nor lock looks for the lock again once its marker is written: an
  # `up` that opened it between the two read no marker, and the down goes through the lock (here the
  # lock appears as the marker is written; the down removes it once it holds it).
  bash -c 'source "$1"; BOXES=$2; proc_start() { : > "$BOXES/.lock-late"; echo 12345; }; cmd_down late' _ "$TMP/lib/bin/omabox" "$b" >/dev/null 2>&1
  check_fails "a lock that appears as a down writes its marker: the down goes through it" test -e "$b/.lock-late"
  # A holder removes the file while a waiter waits on it, as down does: the waiter must end up on
  # the path's file, so a newcomer's -n fails while it holds it.
  out=$(timeout 20 bash -c 'source "$1"; f=$2/.lock-x
    ( lock_file h "$f" -w 5; sleep 1; rm -f "$f"; sleep 0.5 ) & sleep 0.3
    ( lock_file w "$f" -w 10; echo "waiter on path: $([ "$(stat -Lc %i /dev/fd/$w)" = "$(stat -c %i "$f")" ] && echo yes || echo no)"
      ( lock_file n "$f" -n && echo "newcomer locked it too" || echo "newcomer refused" ); sleep 0.2 )
    wait' _ "$TMP/lib/bin/omabox" "$b" 2>&1)
  check_match "a lock file removed under a waiter: the waiter locks the new one" "waiter on path: yes" "$out"
  check_match "...and holds it alone" "newcomer refused" "$out"
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
  check_eq "defaults" "workspace=9 confirm-close=off bar-icon=always gpu=auto" "$(cfg | tr '\n' ' ' | sed 's/ $//')"
  check_eq "set a workspace" workspace=4 "$(cfg workspace 4)"
  check_eq "special is the scratchpad" workspace=special:scratchpad "$(cfg workspace special)"
  check_eq "special:NAME" workspace=special:omabox "$(cfg workspace special:omabox)"
  check_eq "confirm-close yes reads on" confirm-close=on "$(cfg confirm-close yes | tail -1)"
  check_eq "bar-icon auto" bar-icon=auto "$(cfg bar-icon auto)"
  check_fails "bar-icon takes auto or always" env HOME="$h" "$CLI" config bar-icon sometimes
  check_eq "--json for the widget" '{"workspace":"special:omabox","confirm-close":"on","bar-icon":"auto","gpu":"auto"}' \
    "$(HOME=$h "$CLI" config --json | jq -c 'del(.["confirm-close-available"], .version, .gpus, .["gpu-auto"], .["gpu-now"])')"
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
  lib seed_copy "$s/theme" "$s/out" theme
  lib seed_copy "$s/term" "$s/out" term
  check_eq "a linked dir is copied" colors "$(cat "$s/out/theme/colors.toml")"
  check_fails "...without its hidden files" test -e "$s/out/theme/.env"
  check_fails "...or .git" test -e "$s/out/theme/.git"
  check "a link inside stays a link" test -L "$s/out/term/keys"
  check_fails "...with nothing of its target in the box HOME" grep -rqs secret "$s/out"
  # finding 168: a HOME from a save has the box's links where seed_home writes, which it never follows:
  # a dir on the way (absolute or relative, to a dir or nowhere), the dir itself, or a file in it.
  local o=$s/host; mkdir -p "$o/cfg/x" "$s/h2/.local/a" "$s/h3/c" "$s/h4/d"; echo host > "$o/file"
  ln -s "$o/cfg" "$s/h2/.config"; ln -s ../../../host/cfg "$s/h2/.local/a/b"; ln -s "$o/none" "$s/h2/leaf"
  mkfifo "$s/h2/fifo"; ln -s "$o/file" "$s/h3/c/f"; ln -s "$o/cfg" "$s/h4/d/term"
  lib home_unlink "$s/h2" .config/omarchy/shell.json .local/a/b/c leaf fifo
  check "home_unlink: no link or FIFO left on the paths" bash -c "! find '$s/h2' -type l -o -type p | grep -q ."
  lib home_unlink "$s/h3" c/f
  check "...a link at the file itself goes" test ! -e "$s/h3/c/f" -a ! -L "$s/h3/c/f"
  check_eq "...its target untouched" host "$(cat "$o/file")"
  lib seed_copy "$s/term" "$s/h4" d/term
  mkdir -p "$s/h4/e/term/x"; ln -s "$o/file" "$s/h4/e/term/foot.ini"; ln -s ../../../../host/cfg "$s/h4/e/term/x/y"
  mkdir -p "$s/term/x/y"; echo deep > "$s/term/x/y/z"
  lib seed_copy "$s/term" "$s/h4" e/term
  check "seed_copy into a linked dir: a real dir now" test -f "$s/h4/d/term/foot.ini" -a ! -L "$s/h4/d/term"
  check "...over links inside it: real files and dirs" test -f "$s/h4/e/term/foot.ini" -a ! -L "$s/h4/e/term/foot.ini" -a -f "$s/h4/e/term/x/y/z"
  check_eq "...nothing written outside the HOME" "cfg cfg/x file host" "$(find "$o" -mindepth 1 -printf '%P\n' | sort | xargs) $(cat "$o/file")"
  # finding 171: the current theme keeps the links of a theme the user wrote; never followed, a
  # dangling one no failure, and a save's theme (a link to a host dir here) replaced, not merged.
  local th=$s/state/theme cur=.local/state/omarchy/current; mkdir -p "$th/backgrounds" "$s/h5/$cur"
  echo colors > "$th/colors.toml"; echo bg > "$th/backgrounds/1.jpg"
  ln -s "$s/api-keys.env" "$th/keys.toml"; ln -s "$s/none" "$th/backgrounds/2.jpg"
  ln -s "$o/cfg" "$s/h5/$cur/theme"
  check "seed_theme: a theme with a link to a secret and a dangling link" lib seed_theme "$th" "$s/h5"
  check_eq "...copied" "colors bg" "$(cat "$s/h5/$cur/theme/colors.toml" "$s/h5/$cur/theme/backgrounds/1.jpg" | xargs)"
  check "...its links still links" test -L "$s/h5/$cur/theme/keys.toml" -a -L "$s/h5/$cur/theme/backgrounds/2.jpg"
  check_fails "...nothing of their targets in the box HOME" grep -rqs secret "$s/h5"
  check_eq "...a save's theme replaced, its link's target untouched" "x" "$(find "$o/cfg" -mindepth 1 -printf '%P\n' | xargs)"
  mkdir -p "$s/empty"
  check_eq "...nothing to copy: up says so" 1 "$(lib seed_theme "$s/empty" "$s/h5" 2>&1 | grep -c 'could not copy your current theme')"
  # finding 228: up --seed's folder copy into a DEST that holds links already (an earlier --seed's, or
  # a save's): replaced, never written through, whichever comes first; a link to a dir as well.
  local v=$s/victim sd=$s/sd; mkdir -p "$v/dir" "$sd/A" "$sd/B/d" "$sd/B/e" "$s/b1/home" "$s/b2/home" "$s/b3/home/.cfg/sub" "$s/b4/home"
  echo victim > "$v/file"; echo victim > "$v/dir/g"
  ln -s "$v/file" "$sd/A/f"; ln -s "$v/dir" "$sd/A/d"; ln -s ../../../victim/dir "$sd/A/e"
  echo seeded > "$sd/B/f"; echo seeded > "$sd/B/d/g"; echo seeded > "$sd/B/e/h"
  # shellcheck disable=SC2329 # called below
  seed_in() { bash -c 'source "$1"; D=$2; shift 2; for x; do seed_copy_in "$x" .cfg; done' lib "$TMP/lib/bin/omabox" "$@"; }
  check "seed: a folder over a folder's links" seed_in "$s/b1" "$sd/A" "$sd/B"
  check_eq "...its files, real ones" "seeded seeded seeded" "$(cat "$s/b1/home/.cfg/f" "$s/b1/home/.cfg/d/g" "$s/b1/home/.cfg/e/h" | xargs)"
  check "...no link left where they went" test ! -L "$s/b1/home/.cfg/f" -a ! -L "$s/b1/home/.cfg/d" -a ! -L "$s/b1/home/.cfg/e"
  mkdir -p "$sd/A2"; ln -s "$v/file" "$sd/A2/f"
  check "seed: the other order" seed_in "$s/b2" "$sd/B" "$sd/A2"
  check "...the link in place of the file, as copied" test -L "$s/b2/home/.cfg/f"
  check_match "...a link where a dir is: refused, as cp did" "could not copy" "$(seed_in "$s/b4" "$sd/B" "$sd/A" 2>&1)"
  ln -s "$v/file" "$s/b3/home/.cfg/f"; ln -s "$v/dir" "$s/b3/home/.cfg/sub/d"; mkdir -p "$sd/C/sub/d"; echo seeded > "$sd/C/sub/d/g"
  cp "$sd/B/f" "$sd/C/f"; echo own > "$s/b3/home/.cfg/kept"
  check "seed: a save's links under DEST" seed_in "$s/b3" "$sd/C"
  check_eq "...nothing written outside the HOME" "dir dir/g file victim victim" "$(find "$v" -mindepth 1 -printf '%P\n' | sort | xargs) $(cat "$v/file" "$v/dir/g" | xargs)"
  check_eq "...merged with what DEST had (finding 229)" "own seeded" "$(cat "$s/b3/home/.cfg/kept" "$s/b3/home/.cfg/f" | xargs)"
}

# up --theme-dir's pieces (#170, finding 240): where a dir is in the box at its own path, the link in
# the box HOME, and the note for a theme seed_home copies from a link into this repo or another
# checkout of it (a worktree's main one).
t_unit_theme_dir() {
  local d=$TMP/$P-td r w out
  r=$(tmp_repo td-main); mkdir -p "$r/themes/x" "$d/elsewhere/y" "$d/home/.config/omarchy/themes" "$d/victim"
  echo c > "$r/themes/x/colors.toml"
  # shellcheck disable=SC2329 # called below
  vis() { (cd "$1" && bash -c 'source "$1"; robinds=("${@:3}"); own_path_visible "$2"' lib "$TMP/lib/bin/omabox" "$2" "${@:3}"); }
  check "own_path_visible: under the repo up runs from" vis "$r" "$r/themes/x"
  check_fails "...not a dir outside it" vis "$r" "$d/elsewhere/y"
  check "...one under a same-path mount" vis "$r" "$d/elsewhere/y" "$d/elsewhere"$'\t'"$d/elsewhere"
  check_fails "...not one mounted at another path" vis "$r" "$d/elsewhere/y" "$d/elsewhere"$'\t'/mnt/e
  check_fails "...nor a dir whose name only starts like the mount's" vis "$r" "$d/elsewhere-not" "$d/elsewhere"$'\t'"$d/elsewhere"
  check_fails "...nor the repo, from outside a repo" vis "$d" "$r/themes/x"
  # home_link: a link in place of seed_home's copy; a link on the way (a save's) cleared, not followed.
  local h=$d/home; mkdir -p "$h/.config/omarchy/themes/x"; echo copy > "$h/.config/omarchy/themes/x/colors.toml"
  lib home_link "$h" .config/omarchy/themes/x "$r/themes/x"
  check_eq "home_link: a link in place of the copy" "$r/themes/x" "$(readlink "$h/.config/omarchy/themes/x")"
  mkdir -p "$d/h2"; ln -s "$d/victim" "$d/h2/.config"
  lib home_link "$d/h2" .config/omarchy/themes/x "$r/themes/x"
  check "...a link on the way cleared: a real dir now" test -d "$d/h2/.config" -a ! -L "$d/h2/.config" -a -L "$d/h2/.config/omarchy/themes/x"
  check_eq "...nothing written through it" "" "$(ls -A "$d/victim")"
  # The note: a theme linked into the repo up runs from; from a worktree, into the main checkout.
  # (snap_note, not note: that is the suite's own, and a unit test's helper redefines it for every test
  # after it; finding 252.)
  # shellcheck disable=SC2329 # called below
  snap_note() { bash -c 'source "$1"; theme_snapshot_note "$2" "$3"' lib "$TMP/lib/bin/omabox" "$@" 2>&1; }
  ln -s "$r/themes/x" "$h/.config/omarchy/themes/lx"
  check_match "note: a theme linked into this repo is a copy made now" \
    "note: theme lx is a copy of $r/themes/x made now: edits after this do not reach the box. up --theme-dir $r/themes/x gives a live one" \
    "$(snap_note "$h/.config/omarchy/themes/lx/" "$r")"
  git -C "$r" add -A && git -C "$r" -c user.name=t -c user.email=t@t commit -qm t
  w=$TMP/$P-td-wt; git -C "$r" worktree add -q "$w" 2>/dev/null
  out=$(snap_note "$h/.config/omarchy/themes/lx/" "$w")
  check_match "...from a worktree: the main checkout's, not this one's" "copy of $r/themes/x, in another checkout \($r\), not this one \($w\)" "$out"
  check_match "...and this checkout's own for a live one" "up --theme-dir $w/themes/x gives a live one" "$out"
  rm -rf "$w/themes"
  check_match "...or DIR, when this checkout has no such theme" "up --theme-dir DIR gives" "$(snap_note "$h/.config/omarchy/themes/lx/" "$w")"
  ln -s "$d/elsewhere/y" "$h/.config/omarchy/themes/ly"
  check_eq "...nothing for a theme linked from elsewhere" "" "$(snap_note "$h/.config/omarchy/themes/ly/" "$r")"
  # up --theme NAME (finding 245): named as omarchy-theme-set names it, and there.
  local om=$d/om th=$d/th
  mkdir -p "$om/themes/tokyo-night" "$th/.config/omarchy/themes/mine" "$d/dev/wip"
  # shellcheck disable=SC2329 # called below
  tn() { HOME=$th lib theme_named "$1" "$om" "${@:2}" 2>&1; }
  check_eq "theme_named: Omarchy's, named as omarchy-theme-set does" tokyo-night "$(tn '<b>Tokyo Night</b>')"
  check_eq "...one of the user's" mine "$(tn Mine)"
  check_eq "...a --theme-dir's, by its dir's name" wip "$(tn wip "$d/dev/wip")"
  check_match "...an unknown one: refused, saying which there are" \
    "no theme zz in $om/themes, ~/.config/omarchy/themes or --theme-dir \(there are: mine tokyo-night wip\)" "$(tn zz "$d/dev/wip")"
  check_match "...a path: refused" "not a theme's name: ../tokyo-night" "$(tn ../tokyo-night)"
  check_match "...a dot name: refused" "not a theme's name: .mine" "$(tn .mine)"
  mkdir -p "$h/.config/omarchy/themes/z"
  check_eq "...nor for a theme that is a dir of its own" "" "$(snap_note "$h/.config/omarchy/themes/z/" "$r")"
  check_eq "...nor outside a repo" "" "$(snap_note "$h/.config/omarchy/themes/lx/" "")"
  git -C "$r" worktree remove --force "$w" 2>/dev/null
}

# The box's shell.json (finding 21) and its workspace numbers (issue #21, finding 115): a bar left
# with no omarchy.workspaces gets one where a left-out plugin's workspace widget was, else after the
# menu; never a second one, nor next to a mounted plugin that shows workspaces.
# finding 241: a --plugin the box has at its own path (the repo `up` runs from, a same-path --ro-bind)
# is linked to, not mounted; own_path_visible decides, home_link makes the link.
t_unit_plugin_link() {
  local d=$TMP/upl J
  mkdir -p "$d/repo/sub/plug" "$d/repo/hid/plug" "$d/other/plug" "$d/h/.config" "$d/out/cfg"
  git -C "$d/repo" init -q
  # opv CWD ROBINDS PATH: own_path_visible from CWD, with `up`'s robinds as lines of "src<TAB>dest".
  opv() {
    (cd "$1" && bash -c 'source "$1"; robinds=(); [ -z "$2" ] || mapfile -t robinds <<<"$2"; own_path_visible "$3"' \
      _ "$TMP/lib/bin/omabox" "$2" "$3")
  }
  local T=$'\t'
  check "inside the repo up runs from: visible at its own path" opv "$d/repo" "" "$d/repo/sub/plug"
  check_fails "outside it, nothing else mounted: not" opv "$d/repo" "" "$d/other/plug"
  check "inside a same-path --ro-bind" opv / "$d/other$T$d/other" "$d/other/plug"
  check_fails "inside a --ro-bind mounted elsewhere (DIR:DEST): not" opv / "$d/other$T/opt/other" "$d/other/plug"
  check_fails "in the repo, under another folder mounted over it: not" opv "$d/repo" "$d/other$T$d/repo/hid" "$d/repo/hid/plug"
  check_fails "a path that does not exist: not" opv "$d/repo" "" "$d/repo/none"
  # An --overlay is at its own path too (writable there): a plugin in one is linked to it.
  opvo() { (cd / && bash -c 'source "$1"; robinds=(); overlays=("$2"); own_path_visible "$3"' _ "$TMP/lib/bin/omabox" "$1" "$2"); }
  check "inside an --overlay" opvo "$d/other" "$d/other/plug"
  check_fails "...not one beside it" opvo "$d/other/plug/x" "$d/other/plug"
  # Under ai-jail the repo is the jail's project (its cwd), never git's from where the broker runs.
  ln -sfn "$d/other/plug" "$d/repo/out"
  J=$(jq -nc --arg p "$d/repo" '{id: "1 2", net: false, cwd: $p, roots: [{path: $p, masked: false}]}')
  opvj() { OMABOX_JAIL=$J opv "$@"; }
  check "ai-jail: inside the jail's project" opvj / "" "$d/repo/sub/plug"
  check_fails "ai-jail: not through a link out of it" opvj / "$d/other$T$d/other" "$d/repo/out"
  # home_link: whatever is at REL goes (a save's dir), links on the way are cleared, never followed.
  mkdir -p "$d/h/.config/omarchy/plugins/x"; echo old > "$d/h/.config/omarchy/plugins/x/f"
  lib home_link "$d/h" .config/omarchy/plugins/x "$d/repo/sub/plug"
  check_eq "home_link: a link to TARGET in place of a dir" "$d/repo/sub/plug" "$(readlink "$d/h/.config/omarchy/plugins/x")"
  rm -rf "$d/h/.config"; ln -s "$d/out/cfg" "$d/h/.config"
  lib home_link "$d/h" .config/omarchy/plugins/y "$d/repo/sub/plug"
  check "...a link on the way is replaced by a dir" test -d "$d/h/.config/omarchy/plugins" -a ! -L "$d/h/.config"
  check_eq "...and nothing written where it led" "" "$(ls -A "$d/out/cfg")"
}

t_unit_bar_filter() {
  # shellcheck disable=SC2329 # called below
  bar() { lib shell_json_filter "$1" true "$2" <<<"$3" | jq -c "${4:-.bar.layout}"; }
  # A mounted bar widget (finding 151): placed where its manifest says unless the bar, or a mounted
  # plugin's own layout, has it; its settings entry under plugins, or a place in a plugin the box
  # lacks, is not a place.
  local w='["bar-widget"]' side='{"bar":{"layout":{"right":[]}},"plugins":[{"id":"x.side","bottom":["x.w"]},{"id":"x.w","n":1}]}'
  check_eq "a widget the user keeps in a sidebar plugin the box lacks: in the bar" '[{"id":"x.w"}]' \
    "$(lib place_plugin x.w right "$w" '["x.w"]' <<<"$side" | jq -c .bar.layout.right)"
  check_eq "...its settings kept" '{"id":"x.w","n":1}' "$(lib place_plugin x.w right "$w" '["x.w"]' <<<"$side" | jq -c '.plugins[] | select(.id == "x.w")')"
  check_eq "...not when that plugin is mounted too: it holds the widget" '[]' \
    "$(lib place_plugin x.w right "$w" '["x.w","x.side"]' <<<"$side" | jq -c .bar.layout.right)"
  check_eq "...nor when the bar has it" '[{"id":"x.w"}]' \
    "$(lib place_plugin x.w right "$w" '["x.w"]' <<<'{"bar":{"layout":{"right":[{"id":"x.w"}]}}}' | jq -c .bar.layout.right)"
  check_eq "a panel plugin: an entry under plugins, once" '[{"id":"x.p"}]' \
    "$(lib place_plugin x.p right '["panel"]' '["x.p"]' <<<'{"plugins":[{"id":"x.p"}]}' | jq -c .plugins)"
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
  # #155: a bar rebuild leaves the old widget's `bar` null while its bindings still run.
  check_eq "the widget reads its bar only where it may be null (bar ? bar.x : ...; #155)" "" \
    "$(grep -nE "(root\.|[^.A-Za-z_])bar\.[A-Za-z]" "$ROOT"/plugin/*.qml | grep -vE "bar \? bar\.|^[^:]*:[0-9]+: *//")"
  check_match "...and the changelog" "^## $v " "$(grep "^## $v " "$ROOT/CHANGELOG.md")"
  check_eq "config --json names it, for a widget left from before an upgrade (finding 133)" "$v" "$("$CLI" config --json | jq -r .version)"
  # Agent Skills hosts cap a skill's description at 1024 characters (it grows with each trigger), and
  # some parse the frontmatter as strict YAML: there an unquoted one with `: ` in it is an error and
  # the skill is skipped without a word (pi did, finding 145). A double-quoted string, \" escapes only,
  # reads the same as JSON.
  local desc; desc=$(sed -n 's/^description: //p' "$ROOT/skill/SKILL.md" | head -n 1 |
    python3 -c 'import json, sys; v = sys.stdin.read().strip(); assert v.startswith("\""); print(len(json.loads(v)))' 2>&1)
  check_match "the skill's description is a quoted string" '^[0-9]+$' "$desc"
  check "...of at most 1024 characters" test "$desc" -le 1024
  # #143, finding 215: SKILL.md is short and names the rest as "ref: SECTION", a heading of reference.md.
  local refs="" r gone=""
  while read -r r; do refs+="$r|"; grep -qxF "## $r" "$ROOT/skill/reference.md" || gone+="$r; "; done \
    < <(grep -oE 'ref: [A-Z][^.:)]*[a-z]' "$ROOT/skill/SKILL.md" | sed 's/^ref: //' | sort -u)
  check "SKILL.md points to reference.md by section" test -n "$refs"
  check_eq "...every section it names is there" "" "$gone"
  check "...and stays short (under 14 KB: #143)" test "$(wc -c < "$ROOT/skill/SKILL.md")" -lt 14000
  if command -v omarchy-plugin-validate >/dev/null; then
    check "the widget passes omarchy-plugin-validate" omarchy-plugin-validate "$ROOT/plugin"
  else
    skip "the widget passes omarchy-plugin-validate" "no omarchy-plugin-validate here"
  fi
}

# The Omarchy omabox relies on (issue #82). Omarchy updates on its own schedule: an IPC method, a
# shell.json key, a log line or a file that moves under omabox showed up as a box failing somewhere
# later, naming nothing. Each item here says where omabox uses it, so a move is one named failure.
# omarchy_contract DIR: one line per item of the Omarchy tree DIR: its name, a tab, and what is wrong
# with it there (nothing when it holds).
# The shell's IPC: the methods of each block with a literal target (an IpcHandler, or a component made
# of one: Omarchy's ShellIpc), as "TARGET METHOD ARGS". String literals and // comments are left out of
# the brace count.
IPC_METHODS='
import re, sys
for path in sys.argv[1:]:
    stack = []   # one [target, functions] per open brace
    for line in open(path):
        bare = re.sub(r"\"(\\.|[^\"\\])*\"|\x27(\\.|[^\x27\\])*\x27", "\"\"", line).split("//")[0]
        if stack:
            m = re.match(r"\s*target:\s*\"([^\"]*)\"", line)
            if m: stack[-1][0] = m.group(1)
            m = re.match(r"\s*function\s+(\w+)\s*\(([^)]*)\)", line)
            if m: stack[-1][1].append((m.group(1), len([a for a in m.group(2).split(",") if a.strip()])))
        for c in re.findall(r"[{}]", bare):
            if c == "{": stack.append([None, []])
            elif stack:
                target, fns = stack.pop()
                for name, n in fns if target is not None else []: print(target, name, n)
'
# The shell's methods omabox and its skill use, METHOD:ARGS:WHO. Every one the skill names is here
# (t_unit_omarchy_contract checks).
OMARCHY_IPC='listPlugins:0:plugin_check rescanPlugins:0:setup ping:0:skill summon:2:skill toggle:2:skill
  hide:1:skill call:3:skill listShellConfig:0:skill moveBarWidget:2:skill putBarWidget:2:skill
  setBarWidget:4:skill enablePlugin:2:skill setPluginEnabled:2:skill reloadConfig:0:skill
  debugBarGeometry:0:skill togglePanelAt:2:skill'
omarchy_contract() {
  local o=$1 s=$1/shell ipc e m n who have ids
  # oc NAME WHY CMD...: the item holds when CMD succeeds.
  oc() { local n=$1 w=$2; shift 2; if "$@" >/dev/null 2>&1; then printf '%s\t\n' "$n"; else printf '%s\t%s\n' "$n" "$w"; fi; }
  # oc_has FILE FIXED-STRING
  oc_has() { grep -qF -- "$2" "$1"; }
  # Hyprland: share/hyprland.lua runs Omarchy's config by these modules; `up --omarchy` wants a tree
  # with bootstrap.lua and shell.qml.
  oc "default/hypr/bootstrap.lua (share/hyprland.lua runs it)" "missing" test -f "$o/default/hypr/bootstrap.lua"
  oc "default/hypr/omarchy.lua (share/hyprland.lua requires default.hypr.omarchy)" "missing" test -f "$o/default/hypr/omarchy.lua"
  oc "default.hypr.autostart, required by name (share/hyprland.lua marks it loaded: no session autostart in a box)" \
    "no default/hypr/autostart.lua, or nothing requires \"default.hypr.autostart\": a renamed one would run Omarchy's autostart in every box" \
    bash -c 'test -f "$1/autostart.lua" && grep -qF "require(\"default.hypr.autostart\")" "$1"/*.lua' _ "$o/default/hypr"
  oc "default.hypr.paths' omarchy_path (share/hyprland.lua puts omabox's stand-ins before its bin)" "not in default/hypr/paths.lua" \
    oc_has "$o/default/hypr/paths.lua" omarchy_path
  oc "default/hypr/toggles.lua (share/hyprland.lua requires default.hypr.toggles)" "missing" test -f "$o/default/hypr/toggles.lua"
  oc "default/hypr/envs.lua (caller_path tells an Omarchy checkout's bin by it, finding 149)" "missing" test -f "$o/default/hypr/envs.lua"
  oc "default/uwsm/default (share/session.sh: TERMINAL, EDITOR)" "missing" test -f "$o/default/uwsm/default"
  oc "shell/shell.qml (share/shell.sh runs quickshell -p \$OMARCHY_PATH/shell)" "missing" test -f "$s/shell.qml"
  oc "config/omarchy/shell.json, a bar.layout of sections (up --stock-bar, shell_json_filter)" \
    "missing, or its bar.layout is not an object of arrays" \
    jq -e '(.bar.layout | type == "object") and all(.bar.layout[]; type == "array") and (.idle | type == "object")' "$o/config/omarchy/shell.json"
  # Commands omabox runs, and the ones its stand-ins (share/bin) take the place of.
  oc "omarchy-shell (plugin_check, setup)" "not in bin/" test -x "$o/bin/omarchy-shell"
  oc "omarchy-plugin-validate (plugin_check, up's warnings)" "not in bin/" test -x "$o/bin/omarchy-plugin-validate"
  oc "omarchy plugin enable says \"is not known\" for a plugin the shell has not scanned yet (setup_widget waits on it)" \
    "no omarchy-plugin-enable, or it says something else now" oc_has "$o/bin/omarchy-plugin-enable" "is not known"
  oc "omarchy-plugin-disable (setup --remove)" "not in bin/" test -x "$o/bin/omarchy-plugin-disable"
  oc "omarchy-launch-shell runs the shell under systemd-cat -t omarchy-shell (share/bin/systemd-cat logs it to ~/shell.log)" \
    "no omarchy-launch-shell, or it starts the shell another way: omarchy restart shell would leave a box with no bar (finding 131)" \
    oc_has "$o/bin/omarchy-launch-shell" "systemd-cat -t omarchy-shell"
  oc "the plugin registry watches pluginsDir with this inotifywait (share/bin/inotifywait keeps that watch idle, #129)" \
    "PluginRegistry.qml runs inotifywait another way: in a box a mounted plugin reloads by itself on every save again" \
    bash -c 'tr -d " \n" < "$1" | grep -qF "command:[\"inotifywait\",\"-m\",\"-r\",\"-q\",\"-e\",\"close_write,create,delete,move\",\"--format\",\"%w%f\",registry.pluginsDir]"' _ "$s/services/PluginRegistry.qml"
  oc "omarchy-launch-browser uses systemd-run --user (share/bin/systemd-run runs it without a user manager)" \
    "no omarchy-launch-browser, or it starts the browser another way" oc_has "$o/bin/omarchy-launch-browser" "systemd-run --user"
  oc "omarchy-theme-set-browser-policy, called by an omarchy-* (share/bin's stand-in for it)" \
    "nothing in bin/ calls it: the stand-in is stale, or the policy is set another way (with sudo, which a box lacks)" \
    bash -c 'grep -lF omarchy-theme-set-browser-policy "$1"/bin/* | grep -qv "/omarchy-theme-set-browser-policy$"' _ "$o"
  oc "omarchy-version (share/bin's stand-in for it)" "not in bin/" test -x "$o/bin/omarchy-version"
  oc "omarchy-menu-select (share/confirm-close.sh)" "not in bin/" test -x "$o/bin/omarchy-menu-select"
  oc "omarchy-theme-set-gnome (share/session.sh: the theme's colour mode, finding 148)" "not in bin/" test -x "$o/bin/omarchy-theme-set-gnome"
  oc "omarchy-theme-set honours OMARCHY_THEME_HEADLESS (share/session.sh runs it so for up --theme, finding 245)" \
    "not named in bin/omarchy-theme-set: it would talk to a shell and restart apps that are not there yet" \
    oc_has "$o/bin/omarchy-theme-set" OMARCHY_THEME_HEADLESS
  for e in current/theme current/theme.name current/background; do
    oc "omarchy-theme-set writes ~/.local/state/omarchy/$e (seed_home copies it)" "not named in bin/omarchy-theme-set" \
      oc_has "$o/bin/omarchy-theme-set" ".local/state/omarchy/$e"
  done
  # The shell's IPC.
  ipc=$(python3 -c "$IPC_METHODS" "$s/shell.qml" 2>&1) || ipc=""
  for e in $OMARCHY_IPC; do
    IFS=: read -r m n who <<<"$e"
    have=$(awk -v m="$m" '$1 == "shell" && $2 == m { print $3; exit }' <<<"$ipc")
    oc "omarchy-shell shell $m, $n argument(s) ($who)" \
      "$([ -n "$have" ] && echo "takes $have argument(s) now" || echo "not a method of shell.qml's IpcHandler (target \"shell\")")" \
      test "$have" = "$n"
  done
  for e in id kinds enabled; do
    oc "shell listPlugins entries have $e (plugin_check)" "not in listPlugins' entries in shell.qml" \
      bash -c 'sed -n "/function listPlugins(/,/return JSON.stringify/p" "$1" | grep -qE "^ *$2:"' _ "$s/shell.qml" "$e"
  done
  # The shell's log lines plugin_check reads (finding 138).
  oc "\"PluginRegistry: \" warnings for a refused manifest (plugin_check: not loaded)" "no console.warn(\"PluginRegistry: ...) in services/PluginRegistry.qml" \
    oc_has "$s/services/PluginRegistry.qml" 'console.warn("PluginRegistry: '
  oc "\"Plugin widget ID failed: \" (plugin_check: failed)" "not logged so in shell.qml" \
    grep -qE 'console\.warn\("Plugin widget " \+ [A-Za-z_.]+ \+ " failed: "' "$s/shell.qml"
  oc "\"has no barWidget entry point\" (plugin_check: failed)" "not logged in shell.qml" oc_has "$s/shell.qml" "has no barWidget entry point"
  oc "\"failed to load\" (plugin_check: failed)" "not logged in shell.qml" oc_has "$s/shell.qml" " failed to load"
  # Layers: the bar is what `up` waits for; the others are the skill's `wait layer` examples.
  for e in omarchy-bar omarchy-menu omarchy-notifications; do
    oc "layer $e ($([ "$e" = omarchy-bar ] && echo "up waits for it" || echo "the skill's wait layer"))" "no WlrLayershell.namespace \"$e\" under shell/" \
      grep -RqF --include="*.qml" "WlrLayershell.namespace: \"$e\"" "$s"
  done
  # Built-in plugins: shell_json_filter keeps every omarchy.* id as built-in, and names these.
  ids=$(find "$s" -name '*manifest.json' -exec jq -r '.id // empty' {} + 2>/dev/null)
  for e in omarchy.menu omarchy.workspaces omarchy.tray omarchy.notifications omarchy.clock; do
    oc "built-in plugin $e (shell_json_filter, wait_ready, the skill)" "no manifest under shell/ has this id" grep -qxF "$e" <<<"$ids"
  done
  oc "every built-in plugin's id starts with omarchy. (shell_json_filter keeps those)" \
    "$(grep -v '^omarchy\.' <<<"$ids" | tr '\n' ' ')" bash -c '[ -n "$1" ] && ! grep -qv "^omarchy\." <<<"$1"' _ "$ids"
  oc "third-party plugins in ~/.config/omarchy/plugins (up mounts --plugin there)" "PluginRegistry's pluginsDir is elsewhere" \
    oc_has "$s/services/PluginRegistry.qml" '"/.config/omarchy/plugins"'
  # Plugin kinds plugin_check tells apart, and the manifest key place_plugin reads.
  for e in bar bar-widget service; do
    oc "plugin kind \"$e\" (plugin_check)" "\"$e\" not in shell.qml" oc_has "$s/shell.qml" "\"$e\""
  done
  oc "barWidget.defaultSection (seed_home places a mounted widget by it)" "not read by services/PluginRegistry.qml" \
    oc_has "$s/services/PluginRegistry.qml" defaultSection
  # shell.json keys seed_home writes: read by the shell under these names.
  for e in disabledPlugins:services/PluginRegistry.qml centerAnchor:plugins/bar/Bar.qml screensaver:plugins/services/idle/Service.qml; do
    oc "shell.json ${e%%:*} (shell_json_filter)" "not read in shell/${e#*:}" oc_has "$s/${e#*:}" "${e%%:*}"
  done
}

t_unit_omarchy_contract() {
  local o=${OMABOX_TEST_OMARCHY:-/usr/share/omarchy} name why
  if [ ! -f "$o/shell/shell.qml" ]; then skip "the Omarchy contract (issue #82)" "no Omarchy at $o (OMABOX_TEST_OMARCHY names a tree)"; return; fi
  note "Omarchy at $o"
  while IFS=$'\t' read -r name why; do
    if [ -z "$why" ]; then ok "Omarchy: $name"; else no "Omarchy: $name" "$why (in $o)"; fi
  done < <(omarchy_contract "$o")
  local m missing="" named=0
  while read -r m; do
    named=$((named + 1))
    [[ " ${OMARCHY_IPC//$'\n'/ } " == *" $m:"* ]] || missing+="$m "
  done < <(grep -ohE '`(omarchy-shell )?shell [a-z][A-Za-z]+' "$ROOT"/skill/*.md | sed 's/.* //' | sort -u)
  check "the skill names shell methods (#110: else the check below has nothing to compare)" test "$named" -gt 0
  check_eq "every shell method the skill names is in the contract (OMARCHY_IPC)" "" "$missing"
  # The check itself, on a copy of that tree (links, two files real) with one method's arguments
  # changed and the bar's layer renamed: those two fail, nothing else.
  local t=$TMP/omarchy; rm -rf "$t"; cp -rs "$(readlink -f "$o")" "$t"
  rm "$t/shell/shell.qml" "$t/shell/plugins/bar/Bar.qml"
  sed 's/function moveBarWidget(id: string, /function moveBarWidget(/' "$o/shell/shell.qml" > "$t/shell/shell.qml"
  sed 's/"omarchy-bar"/"omarchy-topbar"/' "$o/shell/plugins/bar/Bar.qml" > "$t/shell/plugins/bar/Bar.qml"
  check_eq "...a moved method and a renamed layer: those two fail, with what changed" \
    "omarchy-shell shell moveBarWidget, 2 argument(s) (skill): takes 1 argument(s) now|layer omarchy-bar (up waits for it): no WlrLayershell.namespace \"omarchy-bar\" under shell/" \
    "$(omarchy_contract "$t" | awk -F'\t' '$2 != "" { print $1 ": " $2 }' | paste -sd '|')"
  rm -rf "$t"
}

# evidence() puts back a box's idle clock after its reads (#111, finding 180), but not over another
# test's use of that box meanwhile (finding 237). A stand-in omabox lists one box; each read touches its
# `used` as omabox does; with USE set, the shot read is followed 1.5 s on by another use (USE's time).
t_unit_evidence() {
  local r=$TMP/ev; mkdir -p "$r/rt/omabox/$P-ev" "$r/bin"
  cat > "$r/bin/omabox" <<EOF
#!/bin/bash
u=$r/rt/omabox/$P-ev/used
case \$1 in
  ls) echo '[{"name": "$P-ev", "state": "up"}]' ;;
  shot) touch "\$u"; if [ -n "\${USE:-}" ]; then sleep 1.5; touch -d "\$USE" "\$u"; fi ;;
  *) touch "\$u" ;;
esac
EOF
  chmod +x "$r/bin/omabox"
  ev() { CLI=$r/bin/omabox EVID=$r/evid-$1 CUR=t_x XDG_RUNTIME_DIR=$r/rt USE=${2:-} evidence "a check" "" >/dev/null 2>&1; }
  touch -d '-30 seconds' "$r/rt/omabox/$P-ev/used"; local before; before=$(stat -c %.9Y "$r/rt/omabox/$P-ev/used")
  ev 1
  check_eq "evidence puts a box's idle clock back after its reads (finding 180)" "$before" "$(stat -c %.9Y "$r/rt/omabox/$P-ev/used")"
  # Another use during the shot (its time a fixed one in the future, told apart from every touch here):
  # kept, not the time from before the reads.
  local later; later=$(date -d '+1 hour' +%s)
  ev 2 "@$later"
  check_eq "...but not over a test's use of the box meanwhile (finding 237)" "$later" "$(stat -c %Y "$r/rt/omabox/$P-ev/used")"
}

# The leak detector's reading of events (finding 80), on lines as the watcher logs them (the
# interactive window's openwindow is Hyprland 0.56's, seen in a stand-in box).
t_unit_leak_scan() {
  # In a runtime dir with no boxes: an interactive box of the user's whose box.json changed during the
  # run (a restart-shell) would read as one they started, and the fixture's window as its (#87).
  scan() { printf '1.000 %s\n' "$@" | XDG_RUNTIME_DIR=$TMP/no-boxes leak_scan t1-; }
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
  # #111, finding 180: any window or layer opened on the host is looked up (its + line); the suite's,
  # or a process of this run's box, is a leak even with no focus (a silent workspace, no_initial_focus).
  check_match "a window of the suite's opened silently: a leak" "^leak: a window opened, of this run's: 0xb pid=7 OMABOX_SUITE=t1 class=foot" \
    "$(scan 'openwindow>>b,3,foot,x' '+ 0xb pid=7 OMABOX_SUITE=t1 class=foot')"
  check_match "...one of this run's box's processes too" "^leak: a window opened, of this run's: .*OMABOX_NAME=t1-main" \
    "$(scan 'openwindow>>b,3,foot,x' '+ 0xb pid=7 OMABOX_NAME=t1-main class=foot')"
  check_eq "...yours is a note" "note: not the suite's: window firefox" "$(scan 'openwindow>>b,3,firefox,x' '+ 0xb pid=7 class=firefox')"
  check_eq "...and so is one with no + line (an older watcher)" "note: not the suite's: window foot" "$(scan 'openwindow>>b,3,foot,x' 'closewindow>>b')"
  check_match "a layer of this run's box on the host: a leak" "^leak: a layer opened, of this run's: .*OMABOX_NAME=t1-main class=layer:notifications" \
    "$(scan 'openlayer>>notifications' '+ 0xc pid=8 OMABOX_NAME=t1-main class=layer:notifications')"
  check_eq "...your own layer a note" "note: not the suite's: layer notifications" "$(scan 'openlayer>>notifications' '+ 0xc pid=8 class=layer:notifications')"
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

# A throwaway run takes up's options (#111, finding 180): --overlay (the box writes over the folder,
# the folder is unchanged), --net isolated --allow (that port reached, another not), --size, --plugin.
# Two throwaway servers of the suite's own on free ports.
t_run_options() {
  local repo ov=$TMP/run-ov port other s1 s2 out
  repo=$(tmp_repo ro); mkdir -p "$ov"; echo host > "$ov/f"
  read -r port other < <(python3 -c 'import socket
a, b = socket.socket(), socket.socket(); a.bind(("127.0.0.1", 0)); b.bind(("127.0.0.1", 0))
print(a.getsockname()[1], b.getsockname()[1])')
  python3 -m http.server --bind 127.0.0.1 "$port" --directory "$ov" >/dev/null 2>&1 & s1=$!
  python3 -m http.server --bind 127.0.0.1 "$other" --directory "$ov" >/dev/null 2>&1 & s2=$!
  until_ok 5 curl -fs -o /dev/null "http://127.0.0.1:$port/" >/dev/null; until_ok 5 curl -fs -o /dev/null "http://127.0.0.1:$other/" >/dev/null
  # shellcheck disable=SC2016 # expanded in the box
  out=$(cd "$repo" && env -u OMABOX "$CLI" run --overlay "$ov" --net isolated --allow "$port" --size 800x600 --plugin "$ROOT/plugin" -- sh -c '
    echo box > "$1/f" && cat "$1/f"
    curl -s -o /dev/null -w "%{http_code}\n" --max-time 3 "http://127.0.0.1:$2/" || true
    curl -s -o /dev/null -w "%{http_code}\n" --max-time 3 "http://127.0.0.1:$3/" || true
    hyprctl -j monitors | jq -r ".[0] | \"\(.width)x\(.height)\""
    test -f ~/.config/omarchy/plugins/chaves.omabox/manifest.json && echo plugin' sh "$ov" "$port" "$other" 2>/dev/null)
  kill "$s1" "$s2" 2>/dev/null || true
  check_eq "run --overlay: the box writes over the folder" box "$(sed -n 1p <<<"$out")"
  check_eq "...the folder itself unchanged" host "$(cat "$ov/f")"
  check_eq "run --net isolated --allow PORT: that port reached" 200 "$(sed -n 2p <<<"$out")"
  check_eq "...another host port not" 000 "$(sed -n 3p <<<"$out")"
  check_eq "run --size 800x600" 800x600 "$(sed -n 4p <<<"$out")"
  check_eq "run --plugin: mounted in the box HOME" plugin "$(sed -n 5p <<<"$out")"
}

# The leak detector, proven (finding 80): the watcher the host gets, on a box standing in for the
# host. Quiet, it reports nothing; then a box's window taking focus, a workspace switch and back,
# omabox's workspace coming up, a key from `omabox keys` and a window of the suite's opened silently
# are leaked into the stand-in on purpose, and each must be reported. A clean
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
  # (Started with a file open as fd 7, which no process of the nested box may hold: #112.)
  "${in[@]}" bash -c 'echo x > /tmp/omabox-t112; exec 7</tmp/omabox-t112; exec "$@"' _ "$CLI" up "$C" --no-shell --idle 0 >/dev/null 2>&1 ||
    no "up a box in the stand-in" "failed"
  check_box_safety "$C" "${in[@]}" "$CLI"   # a nested box (#111)
  check_eq "a nested box (no pasta) holds none of its caller's fds (#112)" 0 \
    "$("${in[@]}" "$CLI" run -b "$C" -- sh -c 'for p in /proc/[0-9]*; do ls -l "$p/fd" 2>/dev/null; done | grep -c omabox-t112')"
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
  # #111, finding 180: a window of the suite's that opens silently (no focus, a silent workspace), as a
  # regressed exec in up, peek or run -d would, is reported from its process, not noted as yours.
  mark silent
  ob hyprctl -b "$S" eval "hl.exec_cmd('env OMABOX_SUITE=$P foot -a silentleak sleep 60', { workspace = '3 silent', no_initial_focus = true })" >/dev/null
  until_ok 10 seen silent "" "^[0-9.]* + .*class=silentleak"
  mark end3
  check "a window of the suite's opened silently is reported" scanned silent end3 "^leak: a window opened, of this run's: .*OMABOX_SUITE=$P .*class=silentleak"
  ob run -b "$S" -- pkill -f 'foot -a silentleak' >/dev/null 2>&1
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
  check "allowed: config KEY, a read (#114)" lib broker_check config workspace
  for c in "config workspace 5" "config workspace default" "config nokey" "config --json workspace"; do
    # shellcheck disable=SC2086 # the command and its arguments
    check_match "...refused: $c" "does not change" "$(lib broker_check $c 2>&1)"
  done
  check "allowed: ports (#93)" lib broker_check ports --json
  # ports for a jailed caller: the host side by state only (#93). The stubs of t_unit_cli's ports check:
  # one box, its port held by this shell (host), then by another uid.
  # (The box's server is socket 424242; the host's listener on the port, socket 515151 of uid UID.)
  local pr=$TMP/rt-jports/omabox; mkdir -p "$pr/$P-jp"; echo '{"net":"connected","jail":"j1"}' > "$pr/$P-jp/box.json"
  jports() {   # JAILED(0|1) UID ARGS...: cmd_ports with the port held by uid UID
    local j=$1 u=$2; shift 2
    env -u OMABOX_JAIL XDG_RUNTIME_DIR="$TMP/rt-jports" bash -c 'source "$1"; P=$2 u=$3; [ "$4" = 0 ] || OMABOX_JAIL={\"id\":\"j1\"}; shift 4
      list_names() { echo "$P-jp"; }; box_state() { echo up; }; box_pid() { echo $$; }; box_pids() { echo $$; }
      sock_owners() { echo "424242 $$"; echo "515151 $$"; }
      tcp_listeners() { if [ "$1" = /proc/self/net ]; then echo "40001 0.0.0.0 $u 515151"; else echo "40001 0.0.0.0 0 424242"; fi; }
      cmd_ports "$@"' lib "$TMP/lib/bin/omabox" "$P" "$u" "$j" "$@" 2>&1
  }
  check_match "ports, not jailed: the host process named" "not this box: bash \\(pid [0-9]+\\) holds it" "$(jports 0 "$(id -u)")"
  check_eq "ports, jailed: no pid or process in --json" '{"state":"host"}' "$(jports 1 "$(id -u)" --json | jq -c '.[0].host')"
  check_match "...nor in the text" "not this box: a process of the user's holds it" "$(jports 1 "$(id -u)")"
  check_eq "...another user's: no uid" '{"state":"other-user"}' "$(jports 1 4242 --json | jq -c '.[0].host')"
  check_match "...said so" "not this box: another user's process holds it" "$(jports 1 4242)"
  # Finding 237: the port held by a socket no readable process has (a pasta's), with a box outside the
  # jail ($P-jo, socket 434343) serving on it too: taken, not `this`; with none, `this` as before.
  mkdir -p "$pr/$P-jo"; echo '{"net":"connected","jail":"j2"}' > "$pr/$P-jo/box.json"
  jports_out() {   # OUTSIDE(0|1) ARGS...: jailed, the port's holder unreadable
    local o=$1; shift
    env -u OMABOX_JAIL XDG_RUNTIME_DIR="$TMP/rt-jports" bash -c 'source "$1"; P=$2 o=$3; OMABOX_JAIL={\"id\":\"j1\"}; shift 3
      list_names() { echo "$P-jp"; [ "${1:-}" != all ] || echo "$P-jo"; }
      box_state() { echo up; }; box_pid() { echo $$; }; box_pids() { echo $$; }
      sock_owners() { if [ "$NAME" = "$P-jo" ]; then echo "434343 $$"; else echo "424242 $$"; fi; }
      tcp_listeners() {
        if [ "$1" = /proc/self/net ]; then echo "40001 0.0.0.0 $(id -u) 515151"
        elif [ "$NAME" = "$P-jo" ]; then [ "$o" = 0 ] || echo "40001 0.0.0.0 0 434343"
        else echo "40001 0.0.0.0 0 424242"; fi; }
      cmd_ports "$@"' lib "$TMP/lib/bin/omabox" "$P" "$o" "$@" 2>&1
  }
  check_eq "ports, jailed: a port a box outside the jail serves on too reads as host, not this (finding 237)" \
    '{"state":"host"}' "$(jports_out 1 --json | jq -c '.[0].host')"
  check_match "...said, without naming that box" "taken: a box outside this jail serves on it too" "$(jports_out 1)"
  check_fails "...whose name is not in it" grep -q "$P-jo" <<<"$(jports_out 1; jports_out 1 --json)"
  check_eq "...with no box outside on it: this, as before" "this" "$(jports_out 0 --json | jq -r '.[0].host.state')"
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
  if command -v systemd-analyze >/dev/null; then
    check "the units are valid" systemd-analyze --user verify "$u/omabox-broker.socket" "$u/omabox-broker.service"
  else skip "the units are valid" "no systemd-analyze"; fi
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
    [ "$1" != sleepq ] || exec -a omabox-t109-'"$P"' sleep 30
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
  # Callers killed as soon as they start (#109): some go before the command's setsid(), whose group a
  # kill then did not find. None of the commands left once the relay's 3 s grace is over.
  local i; for i in $(seq 20); do "$R" call "$d/sock" -- sleepq >/dev/null 2>&1 & kill -KILL $! 2>/dev/null; done
  check "callers killed at once: none of their commands left" until_ok 5 bash -c "! pgrep -f '^omabox-t109-$P' >/dev/null"
  # At most 64 connections served at once (#109): a same-uid flood no longer forks without bound.
  python3 -I -c 'import socket, sys, time
s = [socket.socket(socket.AF_UNIX) for _ in range(64)]
for c in s: c.connect(sys.argv[1])
time.sleep(4)' "$d/sock" & local flood=$!
  sleep 1
  check_match "...a call past 64 held connections is turned away (not served: 7; nor left waiting: 124)" '^[1-9]$|^[1-9][0-9]$' \
    "$(timeout 3 "$R" call "$d/sock" -- x >/dev/null 2>&1; rc=$?; [ "$rc" != 7 ] && [ "$rc" != 124 ] && echo "$rc")"
  wait "$flood"
  check_eq "...and served once they close" 7 "$(timeout 15 "$R" call "$d/sock" -- x >/dev/null 2>&1; echo $?)"
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
r changed \$O shot --changed -o ./changed.png
r since \$O shot --since ./out.png
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
  # (#110: not by matching around the user's box with it stripped out first, which passed either way)
  check_match "...its own box listed" "^NAME .*$P-jail .*rc=0 $" "$(sect ls)"
  check_fails "...and not the user's box" grep -q -- "$P-host" <<<"$(sect ls)"
  check_eq "run starts in the jail's project, mounted" "$repo hi rc=0 " "$(sect pwd)"
  check_match "a shot is written into the jail" "211PNG" "$(sect shotfile)"
  check_match "shot -o into the jail's project" "out.png rc=0" "$(sect shot-o)"
  check "...there, on the host too" test -s "$repo/out.png"
  # #145, finding 243: the broker keeps the last frame; nothing changed is no file in the jail.
  check_match "shot --changed through the broker, nothing changed since: said, rc 0" "nothing changed in the screen since $repo/out\.png .*rc=0" "$(sect changed)"
  check_fails "...no image written" test -e "$repo/changed.png"
  check_match "...--since refused there (a path of the jail's)" "not for a jailed agent.* rc=1" "$(sect since)"
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
  local dn0; dn0=$(cd "$ROOT" && lib default_name)
  check_match "default_name gives a name (#110: compared below)" '^[A-Za-z0-9]' "$dn0"
  check_eq "inside a box OMABOX=1 is not a box name" "$dn0" "$(cd "$ROOT" && OMABOX=1 OMABOX_NAME=x lib default_name)"
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
  check_match "...ports' names its states (#94)" "this-resets .*shared " "$(ob help ports | tr '\n' ' ')"
  check_match "...down's says the session's box is taken from outside any repo too (#136, finding 237)" \
    "\`down\` too, so \`omabox down\` there takes that box down" "$(ob help down | tr '\n' ' ')"
  check_match "...drag's: --hold is a duration, a bare number seconds, as ms_duration reads it (#103)" \
    "--hold DURATION.*like 300ms or 2s: a bare number is seconds" "$(ob help drag | tr '\n' ' ')"
  check_eq "...which it does" 2000 "$(lib ms_duration 2)"
  check "confirm-close calls the box's own hyprctl and jq (#113: a stub must not end the box)" \
    grep -qE '^hyprctl\(\) \{ /usr/bin/hyprctl .*$' "$ROOT/share/confirm-close.sh"
  check "...and jq" grep -qE '^jq\(\) \{ /usr/bin/jq ' "$ROOT/share/confirm-close.sh"
  check_match "...up's: --new starts the box, so its options go on that call (#148)" "--new\] +start it under a free name" "$(ob help up)"
  check_match "...as the skill says" "up --new \[--plugin …\]\` starts a box" "$(cat "$ROOT/skill/SKILL.md")"
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
  # ports while a box goes down (#92, finding 164): box.json gone after the up check ($P-pb, from
  # the start of its scan) or during its scan ($P-pa, while its processes are read). No live PID 1:
  # the box's state and sockets are stubbed, one server on 0.0.0.0:40001 held by this shell.
  local pr=$TMP/rt-ports/omabox; mkdir -p "$pr/$P-pa" "$pr/$P-pb"
  echo '{"net":"connected"}' > "$pr/$P-pa/box.json"
  check_eq "ports: a box going down meanwhile is listed as it was or left out, exit 0" "$P-pa 40001 nothing"$'\n'0 \
    "$(env -u OMABOX_JAIL XDG_RUNTIME_DIR="$TMP/rt-ports" bash -c 'source "$1"; P=$2
      list_names() { echo "$P-pa $P-pb"; }; box_state() { echo up; }; box_pid() { echo $$; }
      box_pids() { [ "$NAME" != "$P-pa" ] || rm "$D/box.json"; echo $$; }
      sock_owners() { echo "424242 $$"; }
      tcp_listeners() { [ "$1" = /proc/self/net ] || echo "40001 0.0.0.0 0 424242"; }
      cmd_ports --json' lib "$TMP/lib/bin/omabox" "$P" 2>&1 | jq -r '.[] | "\(.box) \(.port) \(.host.state)"'; echo "${PIPESTATUS[0]}")"
  # A dir an up left with box.json and no info.json (finding 163): listed, dead, so down --all sees it.
  mkdir -p "$TMP/rt-ls2/omabox/$P-lsd"; echo '{"mode":"headless","size":"1x1@60","net":"none","idle":0}' > "$TMP/rt-ls2/omabox/$P-lsd/box.json"
  check_eq "ls: a dir with box.json and no info.json is listed, dead" "$P-lsd dead" \
    "$(XDG_RUNTIME_DIR=$TMP/rt-ls2 "$CLI" ls --json | jq -r '.[] | "\(.name) \(.state)"')"
  # --from's save dir is resolved once, in an assignment (finding 163): a $(save_dir ...) inside cp's
  # argument is not an error under set -e, and a save removed meanwhile made the source /home/.
  check_fails "no \$(save_dir ...) inside a cp argument" grep -n 'cp .*\$(save_dir' "$ROOT/bin/omabox"
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
# omabox which PID (#172), with no box: what is not in one, and what cannot be told.
t_unit_which() {
  local out rc
  out=$(ob which $$ 2>&1); rc=$?
  check_eq "which: a host shell is not in a box, exit 1" "not in a box 1" "$out $rc"
  out=$(ob which 1 2>/dev/null); rc=$?
  check_eq "...nor another user's process (pid 1, root's)" "not in a box 1" "$out $rc"
  check_match "...said whose it is" "process 1 is root's" "$(ob which 1 2>&1 >/dev/null)"
  sh -c 'exit 0' & local gone=$!; wait "$gone"
  out=$(ob which "$gone" 2>&1); rc=$?
  check_match "...a pid that is gone: said, exit 2" "no process $gone .* 2\$" "$out $rc"
  out=$(ob which x 2>&1); rc=$?
  check_match "...not a pid: exit 2" "one PID .* 2\$" "$out $rc"
  # A box this omabox does not list (another state dir, user namespace): by OMABOX_BOX, said so.
  env OMABOX_BOX=elsewhere sleep 30 & local e=$!
  until_ok 5 grep -qa OMABOX_BOX=elsewhere "/proc/$e/environ"
  check_eq "...by OMABOX_BOX when no listed box has it" elsewhere "$(ob which "$e" 2>/dev/null)"
  check_match "...said as only its environment" "by its environment" "$(ob which "$e" 2>&1 >/dev/null)"
  kill "$e" 2>/dev/null
  env OMABOX_BOX='../x y' sleep 30 & e=$!
  until_ok 5 grep -qa OMABOX_BOX= "/proc/$e/environ"
  check_eq "...but not a name a box cannot have" "not in a box" "$(ob which "$e" 2>/dev/null)"
  kill "$e" 2>/dev/null
  check_match "up --env OMABOX_BOX refused: it names the box" "--env cannot set OMABOX_BOX" "$(ob up "$P-w0" --env OMABOX_BOX=x 2>&1)"
  check_match "...run --env too" "--env cannot set OMABOX_BOX" "$(ob run -b "$P-x" --env OMABOX_BOX=x -- true 2>&1)"
}

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
  # (#110: each side a number first; two empty extractions compared equal)
  local lv cv; lv=$(sed -n 's/^local VERSION = \([0-9]*\)$/\1/p' "$ROOT/share/passthrough.lua"); cv=$(sed -n 's/^PASS_VERSION=\([0-9]*\) .*/\1/p' "$CLI")
  check_match "passthrough.lua's VERSION read" '^[0-9]+$' "$lv"
  check_match "PASS_VERSION read" '^[0-9]+$' "$cv"
  check_eq "PASS_VERSION is passthrough.lua's VERSION" "$lv" "$cv"
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
  # finding 162: a throwaway's owner (`run`) by its pid and start time; a reused pid is not it, and a
  # box.json from before owner_start goes by the pid alone. Every reaper down may fail and is retried:
  # none is a bare command, which would end the reaper under set -e.
  local ostart; ostart=$(lib proc_start $$)
  check "the owner that runs is alive" lib owner_alive $$ "$ostart"
  check_fails "...not a pid that runs with another start time (reused)" lib owner_alive $$ "1$ostart"
  check "...a pid alone when no start was recorded" lib owner_alive $$ ""
  check_fails "...and none for a pid gone" lib owner_alive "$nopid" ""
  check_fails "no bare cmd_down in the reaper (it would end it when the down fails)" \
    grep -nE '^\s*DOWN_IF_CREATED=.*cmd_down "\$NAME"\s*$' "$lib"
  check_fails "agent_proc fails when its agent's start time cannot be read (it exited meanwhile)" \
    env CLAUDE_CODE_SESSION_ID="t-$P-session" bash -c 'source "$1"; proc_start() { :; }; CLAUDE_PID=$$; agent_proc' _ "$lib"
  # A command run outside any repo (default-SESSION) while the session's box came from its repo (#136):
  # boxes in a runtime dir of their own, "up" as the stub box_state says. In $TMP: no repo.
  local sr=$TMP/rt-sess sid=5cc72cdc b
  for b in "$P-r-$sid" "$P-q-$sid" "$P-r-0000beef"; do mkdir -p "$sr/omabox/$b"; echo '{}' > "$sr/omabox/$b/box.json"; done
  # sel UP CMD [OMABOX=] FN...: as command CMD with the boxes in UP up, run FN... after select_box ""
  sel() {
    local up=$1 cmd=$2; shift 2
    (cd "$TMP" && env -u OMABOX -u OMABOX_SESSION -u CODEX_THREAD_ID XDG_RUNTIME_DIR="$sr" CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc \
      bash -c 'source "$1"; up=$2; MAIN_CMD=$3; shift 3
        box_state() { if [[ " $up " == *" $NAME "* ]]; then echo up; else echo dead; fi; }
        box_alive() { [ "$(box_state)" = up ]; }
        select_box ""; eval "$*"' _ "$lib" "$up" "$cmd" "$@" 2>&1)
  }
  check_eq "no repo: this session's one box is used, said once (#136)" \
    "omabox: note: no box 'default-$sid' (this directory is in no repo); using this session's box '$P-r-$sid', started from its repo (-b $P-r-$sid names it)"$'\n'"$P-r-$sid" \
    "$(sel "$P-r-$sid $P-r-0000beef" wait echo '$NAME')"
  check_eq "...once" "$P-r-$sid" "$(sel "$P-r-$sid" wait echo '$NAME' | tr -d '\n')"
  rm -f "$sr/omabox/$P-r-$sid/noted-default"
  check_eq "...not for up" "default-$sid" "$(sel "$P-r-$sid" up echo '$NAME')"
  check_eq "...not with OMABOX naming the box" "x" "$(sel "$P-r-$sid" wait 'OMABOX=x; select_box ""; echo $NAME' | tail -1)"
  check_eq "...not with -b" "default-$sid" "$(sel "$P-r-$sid" wait select_box "default-$sid" \; echo '$NAME' 2>&1 | tail -1)"
  check_eq "...nor another session's box" "default-$sid" "$(sel "$P-r-0000beef" wait echo '$NAME')"
  check_match "...two of the session's boxes: neither, both named" \
    "no box 'default-$sid' is up; this session's boxes are $P-(r|q)-$sid $P-(r|q)-$sid \(started from other directories\)" \
    "$(sel "$P-r-$sid $P-q-$sid" wait need_box)"
  check_match "in another repo: the session's box named first" \
    "no box '$P-as-$sid' is up; this session's box is '$P-r-$sid' \(started from another directory\): -b $P-r-$sid" \
    "$(cd "$repo" && env -u OMABOX -u OMABOX_SESSION XDG_RUNTIME_DIR="$sr" CLAUDE_CODE_SESSION_ID=70178a3a-6f6c-4da8-bf69-f4935cc72cdc \
      bash -c 'source "$1"; want=$2; box_state() { if [ "$NAME" = "$want" ]; then echo up; else echo dead; fi; }; box_alive() { false; }; MAIN_CMD=wait; select_box ""; need_box' _ "$lib" "$P-r-$sid" 2>&1)"
  check_eq "up there: the session's box named, the new one still starts"     "omabox: note: this session already has box $P-r-$sid (started from another directory); -b NAME to use it"     "$(sel "$P-r-$sid" up up_session_note)"
  check_eq "...nothing when it has none" "" "$(sel "$P-r-0000beef" up up_session_note)"
  check_match "...a name given: as before" "no box 'nope' is up \(up: " "$(sel "$P-r-$sid" wait select_box nope \; need_box)"
}

# findings 88 and 93: two agent sessions in one repo get a box each, and one's `down` leaves the
# other's up. A session's box goes when its agent exits, but not while it is in use, and a dead one
# keeps its logs. The "agent" here is a shell that exports its own pid as CLAUDE_PID, as Claude Code does.
t_agent_session() {
  local repo; repo=$(tmp_repo ag)
  # Session ids of this run's own (#116): another run's (a second suite at once) would carry the same id
  # tail, and the session-scoped lookups (#136) would see its boxes as this session's too.
  local u; u=$(printf '%06d' $(( $$ % 1000000 )))
  local s1=11111111-2222-4333-8444-5555${u}01 s2=11111111-2222-4333-8444-5555${u}02
  local s3=11111111-2222-4333-8444-5555${u}03 s4=11111111-2222-4333-8444-5555${u}04
  local s5=11111111-2222-4333-8444-5555${u}05 s6=11111111-2222-4333-8444-5555${u}06
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
  local b1=$P-ag-${u}01 b2=$P-ag-${u}02 b3=$P-ag-${u}03 b4=$P-ag-${u}04 named=$P-ag-named
  local b5=$P-ag-${u}05 b6=$P-ag-${u}06 s7=11111111-2222-4333-8444-5555${u}07 b7=$P-ag-${u}07
  local s8=11111111-2222-4333-8444-5555${u}08 b8=$P-ag-${u}08 s9=11111111-2222-4333-8444-5555${u}09
  local s10=11111111-2222-4333-8444-5555${u}10 b10=$P-ag-${u}10
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
  check_eq "...from a directory in no repo too (#136)" "$b1" \
    "$(cd "$TMP" && env -u OMABOX -u OMABOX_IDLE CLAUDE_CODE_SESSION_ID="$s1" "$CLI" run -- sh -c 'echo $OMABOX_NAME' 2>/dev/null)"
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
  # Busy until told (#116: a fixed sleep 15 that took 9 s to start under load ended before the check).
  ob run -b "$b3" -- sh -c 'until [ -e /tmp/t116-done ]; do sleep 0.2; done' & local busy=$!
  # The run is use once its nsenter is up; a check before that would find the box unused.
  until_ok 10 pgrep -f "^nsenter -t $(pid_of "$b3") "; kill "$a3" 2>/dev/null
  local pid4; pid4=$(pid_of "$b4")
  kill -KILL "$pid4" 2>/dev/null; kill "$a4" 2>/dev/null
  sleep 10
  check_eq "its agent gone, a box in use stays" up "$(state_of "$b3")"
  check_eq "a box that died stays dead, logs and all, when its agent goes" dead "$(state_of "$b4")"
  ob run -b "$b3" -- touch /tmp/t116-done; wait "$busy" 2>/dev/null
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
  local s11=11111111-2222-4333-8444-5555${u}11 b11=$P-ag-${u}11 uf tf
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
  # (The box's Hyprland answers for its screen size, finding 81; only grim gets no frame: it blocks
  # until its timeout, 124. Any other failure is grim's to explain, finding 176.)
  shot_msg() { bash -c 'source "$1"; BOXES=$2; need_box() { :; }
    on_box() { [[ "$*" == *"hyprctl -j monitors" ]] || { echo "grim: ${GRIM_SAYS:-}" >&2; return "${GRIM_RC:-124}"; }; echo "[{\"x\": 0, \"y\": 0, \"width\": 1920, \"height\": 1080, \"scale\": 1}]"; }
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
  out=$(GRIM_RC=1 GRIM_SAYS="bad geometry" shot_msg)
  check_match "...grim failing otherwise: its reason, not ask the user" "shot: grim failed: grim: bad geometry$" "$out"
  check_match "a box whose Hyprland does not answer: said, not a silent exit" "box 'oldbox': its Hyprland did not answer \(hung\?" \
    "$(bash -c 'source "$1"; BOXES=$2; need_box() { :; }; box_alive() { :; }; on_box() { return 1; }; cmd_shot -b oldbox -o "$2/x.png"' _ "$TMP/lib/bin/omabox" "$TMP/boxes" 2>&1)"
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
  # (A secret-looking variable in up's environment, which the box session must not get: #111.)
  check "up" env T111_API_TOKEN="leak-$P" "$CLI" up "$B" --env OMABOX_TEST=yes --seed "$ROOT/VERSION:.config/seeded/v" --seed "$ROOT/plugin:~/seeded-dir"
  local D; D=$(ob path -b "$B")
  check_eq "box.json mode headless" headless "$(jq -r .mode "$D/box.json")"
  check_eq "box.json size WxH@HZ" "1920x1080@60" "$(jq -r .size "$D/box.json")"
  check_eq "idle default 2h" 7200 "$(jq -r .idle "$D/box.json")"
  check_eq "--seed: a file in the box HOME (#80)" "$(cat "$ROOT/VERSION")" "$(ob run -b "$B" -- cat /home/sbx/.config/seeded/v)"
  check_eq "...a folder's contents" "$(cat "$ROOT/plugin/manifest.json")" "$(ob run -b "$B" -- cat /home/sbx/seeded-dir/manifest.json)"
  check_match "...up again without it: already up, said" "is already up, without what you asked for: --seed" "$(ob up "$B" --seed "$ROOT/VERSION:x" 2>&1)"
  check_eq "ls --json has the box's GPU, the node it renders on (#118)" "$(ob run -b "$B" -- printenv OMABOX_RENDER_NODE)" \
    "$("$CLI" ls --json | jq -r --arg b "$B" '.[] | select(.name == $b) | .render.node')"
  check "ls --json lists it up" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\" and .state == \"up\")'"
  # finding 62: up waits for the shell (bar layer + notification server) before returning
  check "bar is up when up returns" bash -c "'$CLI' hyprctl -b '$B' -j layers | grep -q omarchy-bar"
  check "notify-send works right after up" ob run -b "$B" -- notify-send omabox-test
  # What the box's Hyprland starts (the bar, binds, terminals) has omabox's stand-ins ahead of
  # Omarchy's bin, which Omarchy's envs.lua puts first (finding 149).
  ob hyprctl -b "$B" dispatch "hl.dsp.exec_cmd('sh -c \"command -v omarchy-version > ~/t149\"')" >/dev/null
  ob wait -b "$B" --timeout 5s cmd -- test -s /home/sbx/t149 >/dev/null
  check_eq "a bind's omarchy-version is omabox's stand-in" /opt/omabox/share/bin/omarchy-version "$(ob run -b "$B" -- cat /home/sbx/t149)"
  # The theme's light or dark mode in the box's dconf (finding 148), for portal dialogs, GTK and Qt.
  local mode; mode=$(ob run -b "$B" -- sh -c 'omarchy-theme-color --file ~/.local/state/omarchy/current/theme/colors.toml mode')
  [ "$mode" = light ] || mode=dark
  ob wait -b "$B" --timeout 5s cmd -- sh -c "gsettings get org.gnome.desktop.interface color-scheme | grep -q prefer-$mode" >/dev/null
  check_eq "dconf has the theme's colour mode" "'prefer-$mode'" "$(ob run -b "$B" -- gsettings get org.gnome.desktop.interface color-scheme)"
  check "up on a running box is a no-op" ob up "$B"
  # From another user namespace (a sandbox's, the suite's --installed): unknown, the list not refused (#87).
  if unshare -Ur true 2>/dev/null; then
    check_eq "ls from another user namespace: the box is unknown" unknown \
      "$(unshare -Ur "$CLI" ls --json 2>/dev/null | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  else skip "ls from another user namespace" "unshare -Ur is not allowed here"; fi
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
  # #130: run --env on a box that is up was dropped ("already up: --env ... ignored"); now the command's.
  local e130; e130=$(ob run -b "$B" --env T130=bar -- sh -c 'echo "T130=${T130-unset}"' 2>&1)
  check_eq "run --env on a box that is up: the command's (#130)" "T130=bar" "$e130"
  check_eq "...after --env-file's, so the flag wins; --pass wins over both" "flag|pass" \
    "$(T30D=pass ob run -b "$B" --env-file "$TMP/t30.env" --env T30A=flag --env T30D=flag --pass T30D -- sh -c 'echo "$T30A|$T30D"')"
  check_match "...not the box session's own" "--env cannot set HOME" "$(ob run -b "$B" --env HOME=/x -- true 2>&1)"
  check_match "...KEY=VAL only" "--env wants KEY=VAL" "$(ob run -b "$B" --env T130 -- true 2>&1)"
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
  check_box_safety "$B"
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
  check_eq "gpu --json names the box's GPU (finding 237)" "$(jq -c .render "$D/box.json")" "$(jq -c .render <<<"$gpu")"
  check_match "...and so does its text" "$(jq -r '.render | "\nGPU: \(.driver) \(.pci) \\(\(.node | sub(".*/"; ""))\\)"' "$D/box.json")" "$(ob gpu -b "$B" 1)"
  check_eq "mode @60.0 is accepted and kept as @60" "$screen_name 1280x720@60" "$(ob mode -b "$B" 1280x720@60.0)"
  check_eq "box.json keeps the normalised mode" "1280x720@60" "$(jq -r .size "$D/box.json")"
  check_eq "shot into a missing dir makes it (#27)" "$TMP/nope/x.png" "$(ob shot -b "$B" -o "$TMP/nope/x.png" 2>/dev/null)"
  check_match "...unless it cannot" "cannot make the directory" "$(ob shot -b "$B" -o "/proc/nope/x.png" 2>&1)"
  check "keys takes -b after the tokens" ob keys Escape -b "$B"
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
  # #111, finding 180: nor any other secret store a HOME has, nor a secret from up's environment.
  local sec found=""
  for sec in .ssh .gnupg .config/gh .aws .netrc .git-credentials; do [ ! -e "$D/home/$sec" ] || found+="$sec "; done
  check_eq "...nor ~/.ssh, ~/.gnupg, gh, aws, .netrc, .git-credentials" "" "$found"
  # (The box has a keyring of its own, by design: never a copy of yours.)
  for sec in "$HOME"/.local/share/keyrings/*.keyring; do
    [ -f "$sec" ] && [ -f "$D/home/.local/share/keyrings/${sec##*/}" ] && cmp -s "$sec" "$D/home/.local/share/keyrings/${sec##*/}" && found+="${sec##*/} "
  done
  check_eq "...nor a copy of your keyrings" "" "$found"
  local senv; senv=$(ob run -b "$B" -- sh -c 'tr "\0" "\n" < /proc/$(pgrep -x Hyprland)/environ')
  check_match "...the session's environment read (--env's variable is there)" "OMABOX_TEST=yes" "$senv"
  check_fails "...nor a token from up's environment in it" grep -q "leak-$P" <<<"$senv"
  # The box writes the theme.name ls --json reads (finding 159): a FIFO there does not hang it, a link
  # to a host file does not show that file's first line.
  local tn=/home/sbx/.local/state/omarchy/current/theme.name th
  ob run -b "$B" -- mv "$tn" "$tn.t159"
  ob run -b "$B" -- mkfifo "$tn"
  check "ls --json with a FIFO as theme.name returns" bash -c "timeout -k 1 5 '$CLI' ls --json > '$D/t159.json' 2>/dev/null"
  # (Without the fix a reader stays blocked on it, holding the check's output: opened read-write
  # here, it gets EOF and ends.)
  : 9<>"$D/home/.local/state/omarchy/current/theme.name"
  check_eq "...lists the box with no theme" "up " "$(jq -r --arg n "$B" '.[] | select(.name == $n) | "\(.state) \(.theme // "")"' "$D/t159.json" 2>&1)"
  ob run -b "$B" -- mv "$tn" "$tn.fifo"
  printf 'theme159\n' > "$D/t159.name"
  ob run -b "$B" -- ln -s "$D/t159.name" "$tn"
  th=$(timeout -k 1 5 "$CLI" ls --json 2>/dev/null | jq -r --arg n "$B" '.[] | select(.name == $n) | .theme // ""') || th=failed
  check_eq "...nor a link's target's first line" "" "$th"
  ob run -b "$B" -- mv -f "$tn.t159" "$tn"
  th=$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .theme // ""') || th=failed
  check_eq "...and the file back, its name" "$(head -n 1 "$D/home/.local/state/omarchy/current/theme.name")" "$th"
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
  check_box_safety "$B"
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
  check_match "ls shows it connected" "$B +headless .* up +[^ ]+ +connected " "$(ob ls)"
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
  else skip "DNS resolves in the box" "the host resolves no names"; fi
  # host -> box as localhost: the host tries ::1 first, which must be refused, not reset (pasta
  # binds the host side on 127.0.0.1 only); forwards take up to about a second to appear.
  port=$(free_port) tok=box-$P-$RANDOM
  mkdir -p "$D/home/www" && echo "$tok" > "$D/home/www/index.html"
  ob run -b "$B" -d -- python3 -m http.server "$port" --bind 127.0.0.1 --directory /home/sbx/www >/dev/null
  check "the host reaches a box server as localhost" until_ok 10 bash -c "curl -fsS --max-time 2 http://localhost:$port/ | grep -qx '$tok'"
  check_eq "...which is bound on the host's 127.0.0.1 only" "127.0.0.1:$port" "$(ss -Htuln "sport = :$port" | awk '{print $5}' | sort -u)"
  # ...and on a port the kernel chose (the ephemeral range has a rule of its own: auto alone skips it)
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
  else skip "the box's gateway is not the host's loopback" "the box has no default route"; fi
  # Stopped now, and dropped from SERVERS (cleanup's, for a suite stopped midway): by the end of the
  # suite their pids may be another process's.
  kill "${SERVERS[@]}" 2>/dev/null; SERVERS=()
  check "down" ob down "$B"
}

# finding 156 (#88): `ports` lists a box's servers and what holds each port on the host's 127.0.0.1:
# the box itself, or every box serving on it when there are several (only one gets it, and which is
# not readable). The host's servers pasta mirrors into a box are not the box's. And the box that lost
# the shared port still forwards the servers it starts later, on higher ports: with one strong rule
# for every port, pasta gave up on the ports after one it could not bind.
t_ports() {
  local A=$P-pa B=$P-pb own shared local6 hostp out later b two at
  if ! ob up "$A" --no-shell >/dev/null 2>&1 || ! ob up "$B" --no-shell >/dev/null 2>&1; then
    no "ports: up two boxes" "failed"; ob down "$A" >/dev/null 2>&1; ob down "$B" >/dev/null 2>&1; return
  fi
  # The shared port low, so the ones checked after it are above it.
  while shared=$((20000 + RANDOM % 1000)); ss -Htuln "sport = :$shared" | grep -q .; do :; done
  until own=$(free_port); [ "$own" != "$shared" ]; do :; done
  until local6=$(free_port); [ "$local6" != "$own" ] && [ "$local6" != "$shared" ]; do :; done
  until hostp=$(free_port); [ "$hostp" != "$own" ] && [ "$hostp" != "$shared" ] && [ "$hostp" != "$local6" ]; do :; done
  mkdir -p "$TMP/ports"
  python3 -m http.server "$hostp" --bind 127.0.0.1 --directory "$TMP/ports" >/dev/null 2>&1 & SERVERS+=($!)
  ob run -b "$A" -d -- python3 -m http.server "$own" --bind 127.0.0.1 --directory /home/sbx >/dev/null
  ob run -b "$A" -d -- python3 -m http.server "$shared" --bind 127.0.0.1 --directory /home/sbx >/dev/null
  ob run -b "$B" -d -- python3 -m http.server "$shared" --bind 0.0.0.0 --directory /home/sbx >/dev/null
  ob run -b "$A" -d -- python3 -m http.server "$local6" --bind ::1 --directory /home/sbx >/dev/null
  # A's and B's servers on $shared start ~50 ms apart and both run only because no pasta rescan (each
  # second) falls between them: one started after the other's port reached the host finds it mirrored
  # into its own box and fails, address in use (finding 156, #89). A rare flake, so say which one.
  for b in "$A" "$B"; do
    check "ports: $b's server on the shared port listens in its box (no rescan between the two starts)" \
      until_ok 10 ob run -b "$b" -- bash -c "ss -Htlnp 'sport = :$shared' | grep -q python3"
  done
  # Once the host has both forwards and B has the mirror of the host server (pasta rescans each second).
  check "ports: the host has the box servers' ports" until_ok 10 bash -c "ss -Htln 'sport = :$own' | grep -q . && ss -Htln 'sport = :$shared' | grep -q ."
  check "...and pasta mirrored the host server into a box" until_ok 10 ob run -b "$B" -- bash -c "ss -Htln 'sport = :$hostp' | grep -q ."
  # A server on ::1 only still has its pasta take the port here (finding 165), and resets connections.
  check "...a server on ::1 only takes its port on the host too" until_ok 10 bash -c "ss -Htln 'sport = :$local6' | grep -q ."
  check_fails "...where a connection to it fails" curl -sS --max-time 2 -o /dev/null "http://127.0.0.1:$local6/"
  out=$(ob ports --json)
  q() { jq -r --arg b "$1" --argjson p "$2" '.[] | select(.box == $b and .port == $p) | .host | [.state, ((.boxes // []) | join(","))] | join(" ")' <<<"$out"; }
  check_eq "...a box's server reaches the host as its own" "this $A" "$(q "$A" "$own")"
  check_eq "...a port two boxes serve on names both, for each" "shared $A,$B|shared $A,$B" "$(q "$A" "$shared")|$(q "$B" "$shared")"
  check_eq "...a server on ::1 only: this box holds the port, and resets" "this-resets $A" "$(q "$A" "$local6")"
  check_eq "...the host's server mirrored into a box is not the box's" "" "$(q "$B" "$hostp")"
  out=$(ob ports -b "$B")
  check_match "ports -b lists that box only" "^BOX .*"$'\n'"$B +$shared +0\.0\.0\.0 +python3? +one of $A, $B" "$out"
  check_eq "...one line" 2 "$(wc -l <<<"$out")"
  # Both boxes serve on the shared port, so whichever lost it holds a port pasta cannot bind.
  for b in "$A" "$B"; do
    until later=$(free_port); [ "$later" -gt "$shared" ] && [ "$later" != "$own" ] && [ "$later" != "$local6" ] && [ "$later" != "$hostp" ]; do :; done
    ob run -b "$b" -d -- python3 -m http.server "$later" --bind 127.0.0.1 --directory /home/sbx >/dev/null
    check "...a box serving on the shared port still forwards a later server, above it ($b)" \
      until_ok 10 bash -c "ss -Htln 'sport = :$later' | grep -q ."
  done
  # A ::1 server in B and a 127.0.0.1 one in A on one port (finding 165): B's pasta competes for the
  # port here as A's does, so both are named, B as the one that resets. Both start at one instant: a
  # server started after the other's pasta took the port finds that port mirrored into its own box.
  until two=$(free_port); [ "$two" != "$own" ] && [ "$two" != "$shared" ] && [ "$two" != "$local6" ] && [ "$two" != "$hostp" ]; do :; done
  at=$(($(date +%s%N) + 2000000000))
  for b in "$B ::1" "$A 127.0.0.1"; do
    ob run -b "${b% *}" -d -- sh -c "while [ \$(date +%s%N) -lt $at ]; do sleep 0.01; done; exec python3 -m http.server $two --bind ${b#* } --directory /home/sbx" >/dev/null
  done
  both_on() { [ "$(ob ports --json | jq --argjson p "$two" '[.[] | select(.port == $p)] | length')" = 2 ] && ss -Htln "sport = :$two" | grep -q .; }
  check "...two boxes' servers on one port, ::1 and 127.0.0.1, both listen and the host has the port" until_ok 10 both_on
  out=$(ob ports --json)
  check_eq "...each names both" "shared $A,$B|shared $A,$B" "$(q "$A" "$two")|$(q "$B" "$two")"
  check_eq "...and the ::1 one as the one that resets" "$B|$B" \
    "$(jq -r --argjson p "$two" '[.[] | select(.port == $p) | .host.resets | join(",")] | join("|")' <<<"$out")"
  check_match "...and says so" "^$A +$two +127\.0\.0\.1 +python3? +one of $A, $B: .* \($B's server is on ::1 only and would reset\)$" \
    "$(ob ports -b "$A" | grep "^$A  *$two ")"
  kill "${SERVERS[@]}" 2>/dev/null; SERVERS=()
  check "down" ob down "$A"
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
# #127: a widget placed in another mounted plugin's own layout (a sidebar that hosts widgets) is that
# plugin's to load: the shell lists it not enabled, and up/restart-shell called it disabled (38 times).
t_plugin_hosted() {
  local B=$P-ph F=$TMP/hosted out H
  hfix() {   # DIR ID: a bar widget
    mkdir -p "$F/$1"
    jq -n --arg id "$2" '{schemaVersion: 1, id: $id, name: $id, version: "0.1.0", kinds: ["bar-widget"],
      entryPoints: {barWidget: "W.qml"}, barWidget: {displayName: $id, defaultSection: "right"}}' > "$F/$1/manifest.json"
    printf 'import QtQuick\nimport qs.Ui\nBarWidget {\n  id: root\n  implicitWidth: b.implicitWidth\n  implicitHeight: b.implicitHeight\n  WidgetButton { id: b; bar: root.bar; text: "%s" }\n}\n' "$1" > "$F/$1/W.qml"
  }
  hfix a "$P.ha"; hfix b "$P.hb"; hfix c "$P.hc"
  ob up "$B" --net isolated --plugin "$F/a" --plugin "$F/b" --plugin "$F/c" >/dev/null 2>&1 || { no "up" "failed"; return; }
  H=$(ob path "$B")/home
  # A out of the bar into B's own entry; C out of the bar, in nothing's.
  jq --arg a "$P.ha" --arg b "$P.hb" --arg c "$P.hc" \
    '.bar.layout |= map_values(map(select((if type == "object" then .id else . end) as $x | $x != $a and $x != $c)))
     | .plugins = [(.plugins // [])[] | select(.id != $b)] + [{id: $b, widgets: [{id: $a}]}]' \
    "$H/.config/omarchy/shell.json" > "$TMP/hosted.json" && cp "$TMP/hosted.json" "$H/.config/omarchy/shell.json"
  out=$(ob restart-shell -b "$B" 2>&1)
  check_fails "a widget hosted in another mounted plugin's layout: no \"disabled\" warning (#127)" grep -q "$P.ha disabled" <<<"$out"
  check_eq "...ls --json: hosted, and by which" "hosted $P.hb" \
    "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.ha" '.[] | select(.name == $n) | .plugin_status[$i] | "\(.state) \(.host)"')"
  check_match "a widget off the bar and in no plugin's layout: still disabled" "plugin $P.hc disabled: listed but not enabled" "$out"
  ob down "$B" >/dev/null 2>&1
}

# finding 241: a plugin inside a larger repo, linked into the plugins dir on a desk, whose helper finds
# a sibling file of the repo through readlink -f "$0". In a box the plugin is a link to its own path
# when the box has that path (the repo `up` runs from, a same-path --ro-bind), else a mount.
t_plugin_link() {
  local B=$P-pl R O=$TMP/pl-out M=$TMP/pl-rb V=$TMP/pl-ov H out f=/home/sbx/.config/omarchy/plugins
  R=$(tmp_repo pl)
  lfix() {   # DIR ID: a bar widget with a helper that reads ../../tools/sibling.txt from its real path
    mkdir -p "$1/helpers"
    jq -n --arg id "$2" '{schemaVersion: 1, id: $id, name: $id, version: "0.1.0", kinds: ["bar-widget"],
      entryPoints: {barWidget: "W.qml"}, barWidget: {displayName: $id, defaultSection: "right"}}' > "$1/manifest.json"
    printf 'import QtQuick\nimport qs.Ui\nBarWidget {\n  id: root\n  implicitWidth: b.implicitWidth\n  implicitHeight: b.implicitHeight\n  WidgetButton { id: b; bar: root.bar; text: "%s" }\n}\n' "$2" > "$1/W.qml"
    printf '#!/bin/bash\nhere=$(dirname "$(readlink -f "$0")")\ncat "$here/../../tools/sibling.txt"\n' > "$1/helpers/find.sh"
  }
  lfix "$R/widget" "$P.lw"; lfix "$O/widget" "$P.lo"; lfix "$M/widget" "$P.lm"; lfix "$V/widget" "$P.lv"
  mkdir -p "$R/tools" "$M/tools" "$V/tools"; echo sibling > "$R/tools/sibling.txt"; echo sibling > "$M/tools/sibling.txt"; echo sibling > "$V/tools/sibling.txt"
  out=$(cd "$R" && ob up "$B" --net isolated --ro-bind "$M" --overlay "$V" --plugin "$R/widget" --plugin "$O/widget" --plugin "$M/widget" \
    --plugin "$V/widget" 2>&1) ||
    { no "up with a plugin in its repo, one outside, one in a --ro-bind, one in an --overlay" "$out"; return; }
  H=$(ob path "$B")/home
  check_eq "a plugin inside the repo up runs from: its helper reaches the repo through readlink -f" sibling \
    "$(ob run -b "$B" -- bash "$f/$P.lw/helpers/find.sh" 2>&1)"
  check_eq "...a link in the box HOME to its own path, as on a desk" "$R/widget" "$(readlink "$H/.config/omarchy/plugins/$P.lw")"
  check_eq "one inside a same-path --ro-bind: the same" sibling "$(ob run -b "$B" -- bash "$f/$P.lm/helpers/find.sh" 2>&1)"
  check_fails "one outside anything mounted: no link" test -L "$H/.config/omarchy/plugins/$P.lo"
  check "...mounted there" ob run -b "$B" -- sh -c "test -f $f/$P.lo/manifest.json && mountpoint -q $f/$P.lo"
  # An --overlay is at its own path, writable: the plugin is linked to the overlay's view, which the
  # box's writes reach (mounted from the host's copy, they did not).
  check_eq "one inside an --overlay: a link to its own path" "$V/widget" "$(readlink "$H/.config/omarchy/plugins/$P.lv")"
  ob run -b "$B" -- sh -c "echo written-in-box > $V/tools/sibling.txt"
  check_eq "...its helper reads the overlay as the box wrote it" written-in-box "$(ob run -b "$B" -- bash "$f/$P.lv/helpers/find.sh" 2>&1)"
  check_eq "...the host's copy untouched" sibling "$(cat "$V/tools/sibling.txt")"
  check_eq "ls --json: linked, mounted, linked, linked" "linked mounted linked linked" \
    "$(ob ls --json | jq -r --arg n "$B" --arg p "$P" '.[] | select(.name == $n) | .plugin_status | [.[$p + ".lw", $p + ".lo", $p + ".lm", $p + ".lv"] | .via] | join(" ")')"
  check_eq "...and all four loaded through it" "loaded loaded loaded loaded" \
    "$(ob ls --json | jq -r --arg n "$B" --arg p "$P" '.[] | select(.name == $n) | .plugin_status | [.[$p + ".lw", $p + ".lo", $p + ".lm", $p + ".lv"] | .state] | join(" ")')"
  # #129 through a link: the registry's watch is still the box's idle stand-in, and restart-shell
  # reads the plugin's files as they are now.
  check_match "the registry's watch is the box's stand-in" "/opt/omabox/share/bin/inotifywait -m -r -q" \
    "$(ob run -b "$B" -- pgrep -af '[i]notifywait')"
  cp "$R/widget/W.qml" "$TMP/pl-W.qml"; echo 'BarWidget {' >> "$R/widget/W.qml"
  check_match "an edit in the repo, restart-shell: the linked plugin as edited" "plugin $P.lw failed" "$(ob restart-shell -b "$B" 2>&1)"
  cp "$TMP/pl-W.qml" "$R/widget/W.qml"
  check_fails "...fixed, restart-shell: loaded again" grep -q "$P.lw" <<<"$(ob restart-shell -b "$B" 2>&1)"
  ob down "$B" >/dev/null
}

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
  # Only the checkout `omarchy plugin add` reads below is in the box at its own path: with all of $F
  # mounted, these would be linked to, not mounted (finding 241; t_plugin_link has that case).
  out=$(ob up "$B" --net isolated --ro-bind "$F/inst" --plugin "$F/good" --plugin "$F/schema" --plugin "$F/entry" --plugin "$F/qml" 2>&1) ||
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
  # #129: Omarchy's plugin registry reloaded a mounted plugin on every save on the host, several times
  # each; in a box nothing reloads by itself (share/bin/inotifywait keeps its watch idle).
  echo '// t129' >> "$F/good/W.qml"; sleep 3
  check_fails "an edit on the host to a mounted plugin reloads nothing by itself (#129)" grep -q "Local plugin changed" \
    <<<"$(ob log -b "$B" shell -n all)"
  check_match "...the registry's watch is the box's stand-in, waiting" "/opt/omabox/share/bin/inotifywait -m -r -q" \
    "$(ob run -b "$B" -- pgrep -af '[i]notifywait')"
  check_eq "ls --json: the box's theme" "$(cat "$(ob path "$B")/home/.local/state/omarchy/current/theme.name")" \
    "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .theme')"
  cp "$F/W.qml.fixed" "$F/qml/W.qml"
  out=$(ob restart-shell -b "$B" 2>&1)
  check_fails "fixed, restart-shell: no line for it" grep -q "$P.qml failed" <<<"$out"
  check_match "...still one for the others" "plugin $P.entry failed" "$out"
  check_eq "...and ls --json has it loaded" loaded "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.qml" '.[] | select(.name == $n) | .plugin_status[$i].state')"
  # finding 161: a shell.log the host cannot read (write-only, so the shell still writes it) does not
  # end restart-shell after "shell restarted".
  ob run -b "$B" -- chmod 200 /home/sbx/shell.log
  check "restart-shell with a shell.log the host cannot read" ob restart-shell -b "$B"
  ob run -b "$B" -- chmod 644 /home/sbx/shell.log
  check_eq "...ls --json still has plugin_status" loaded "$(ob ls --json | jq -r --arg n "$B" --arg i "$P.good" '.[] | select(.name == $n) | .plugin_status[$i].state')"
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
  check_box_safety "$B2"
  check_eq "...with the app's data" hello "$(ob run -b "$B2" -- cat /home/sbx/.local/share/app/data)"
  check_eq "...and its keyring secret" pw "$(ob run -b "$B2" -- secret-tool lookup service omabox-test)"
  check_eq "...an app config changed in the box kept (no /etc/skel over it)" "# mine" "$(ob run -b "$B2" -- tail -n 1 /home/sbx/.config/btop/btop.conf)"
  check_eq "...the bar's config seeded again, not the save's" null "$(ob run -b "$B2" -- jq .junk /home/sbx/.config/omarchy/shell.json)"
  check "...the theme seeded again (not copied into the save's)" ob run -b "$B2" -- sh -c 't=/home/sbx/.local/state/omarchy/current/theme; test -d $t && test ! -e $t/theme'
  check_eq "...box.json says where it came from" "$S" "$(jq -r .from "$(ob path "$B2")/box.json")"
  # finding 168: a save keeps the links the box made in its HOME, and seed_home, on the host, wrote
  # through them. Links where it writes, to a file and a dir of the suite's: up --from leaves both as
  # they were.
  local B3=$P-sv3 S2=$P-s2 hf=$TMP/sv-host-file hd=$TMP/sv-host-dir H
  echo "the host's line" > "$hf"; mkdir -p "$hd/theme"; echo sentinel > "$hd/theme/keep"
  ob run -b "$B2" -- sh -c 'ln -sfn "$1" ~/.config/omarchy/shell.json && rm -rf ~/.local/state/omarchy/current &&
    ln -s "$2" ~/.local/state/omarchy/current && ln -s "$2" ~/.config/omarchy/plugins/stale' _ "$hf" "$hd" >/dev/null 2>&1
  check "a save of a box with links where seed_home writes" sv save "$S2" -b "$B2"
  check "...starts" sv up "$B3" --from "$S2" --no-shell
  check_eq "...the file a link pointed at untouched" "the host's line" "$(cat "$hf")"
  check_eq "...the dir a link pointed at untouched" "theme theme/keep sentinel" "$(find "$hd" -mindepth 1 -printf '%P\n' | sort | xargs) $(cat "$hd/theme/keep")"
  H=$(ob path "$B3")/home
  check "...its shell.json a real file" test -f "$H/.config/omarchy/shell.json" -a ! -L "$H/.config/omarchy/shell.json"
  check "...its current theme a real dir, seeded" test -d "$H/.local/state/omarchy/current/theme" -a ! -L "$H/.local/state/omarchy/current"
  # finding 241: a linked --plugin's link stays in a save; without that plugin asked for, it would load.
  check_fails "...no link of the save's left in its plugins dir" test -L "$H/.config/omarchy/plugins/stale"
  check "...and the box runs" ob run -b "$B3" -- test -s /home/sbx/.local/state/omarchy/current/theme.name
  check "down" ob down "$B3"
  check "...saves rm" sv saves rm "$S2"
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
  # #153: a helper that checks its bus wants it in a 0700 runtime dir of its user, as a session has.
  check_eq "the runtime dir is 0700, its user's (#153)" "700 $(id -u)" "$(ob run -b "$B" -- sh -c 'stat -c "%a %u" "$XDG_RUNTIME_DIR"')"
  check_eq "...the session bus is the socket in it" "unix:path=/run/user/$UID/bus socket" \
    "$(ob run -b "$B" -- sh -c 'echo "${DBUS_SESSION_BUS_ADDRESS%%,*} $(stat -c %F "$XDG_RUNTIME_DIR/bus")"')"
  dbus_user_app "$B"
  check "down" ob down "$B"
}

t_systemd() {
  local B=$P-sd
  check "up --systemd --net isolated" ob up "$B" --systemd --net isolated
  check_box_safety "$B"
  check_match "user manager up" '^(running|degraded)$' "$(ob run -b "$B" -- systemctl --user is-system-running 2>&1)"
  check_eq "only the document portal may fail" "" \
    "$(ob run -b "$B" -- systemctl --user --failed --no-legend --plain | awk '{print $1}' | grep -v '^xdg-document-portal.service$')"
  check "manager has the session environment" bash -c "'$CLI' run -b '$B' -- systemctl --user show-environment | grep -q '^WAYLAND_DISPLAY='"
  check "...with the box's name, for what its units start (#172)" bash -c "'$CLI' run -b '$B' -- systemctl --user show-environment | grep -qx 'OMABOX_BOX=$B'"
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
  else
    # (#110) A failure counts only as the one expected: grim could not reach that display.
    check_match "shot does not follow the box's symlink to another screen (it failed: no display)" \
      "grim failed: .*(connect|display)" "$out"
  fi
  check "...and the honest box's shot still works" ob shot -b "$B" -o "$TMP/honest.png"
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
  # An up holds the name's lock a moment before it makes the box dir (finding 147): a down then waits
  # for it, not "no box" while the box comes up behind it. The suite holds the lock, standing in for it.
  local lk out; exec {lk}>"$XDG_RUNTIME_DIR/omabox/.lock-$B"; flock "$lk"
  ob down "$B" > "$TMP/race-down" 2>&1 & a=$!
  sleep 1; out=$(cat "$TMP/race-down")
  flock -u "$lk"; exec {lk}>&-; wait "$a"
  check_match "a down while an up holds the lock, before its box dir, waits for it" "waiting for its up" "$out"
  check_match "...then says no box when that up made none" "no box '$B'" "$(cat "$TMP/race-down")"
  # Earlier still, before the up has the lock: the down finds nothing, and the up cancels itself.
  local C=$P-race2 rc
  ob up "$C" --no-shell > "$TMP/race-up" 2>&1 & a=$!
  ob down "$C" >/dev/null 2>&1; wait "$a"; rc=$?
  if [ "$rc" = 0 ]; then
    check_eq "a down while its up starts: the box is gone (the down waited for it)" "" "$(ob ls --json | jq -r --arg n "$C" '.[] | select(.name == $n) | .name')"
  else
    check_match "a down while its up starts: the up says it was cancelled" "cancelled: omabox down $C came while it was starting" "$(cat "$TMP/race-up")"
  fi
  check "...nothing of it running" until_ok 5 none_running "$C"
  check "a down before an up does not cancel it" ob up "$C" --no-shell
  check "...down" ob down "$C"
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
  # kill_box waits for PID 1 (up to 10 s, saying so past that): a failed up returns with its box gone.
  # Its own message is in the evidence when not (#86). (This passed on the old trap too, #91:
  # t_unit_kill_box is what guards finding 157's fix.)
  check_eq "...and returns with the box dead (up said: ${out//$'\n'/ | })" dead \
    "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  # The namespace and bwrap parent can take a moment to exit after the failed command returns.
  # shellcheck disable=SC2329 # called through until_ok
  failed_box_dead() { [ "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')" = dead ]; }
  # shellcheck disable=SC2329 # called through until_ok
  failed_box_processes_gone() { [ "$(pgrep -fc "bwrap .*--bind $XDG_RUNTIME_DIR/omabox/$B/run " || true)" = 0 ]; }
  check "the box becomes dead, not running" until_ok 10 failed_box_dead
  check "nothing of it remains running" until_ok 10 failed_box_processes_gone
  check "down clears it" ob down "$B"
  # A throwaway that fails the same way keeps its logs outside the box dir its own `down` removes, and
  # says where (finding 167). The suite's own state dir: never the user's failed-runs.
  local repo st=$TMP/state-fr kept; repo=$(tmp_repo fr)
  out=$(cd "$repo" && exec env -u OMABOX XDG_STATE_HOME="$st" OMABOX_READY_TIMEOUT=2 "$CLI" run --env OMABOX_SHELL=0 -- true 2>&1) &&
    no "a throwaway whose up fails fails" "$out"
  check_match "a failed throwaway says where its logs are kept" "logs are kept in $st/omabox/failed-runs/$P-fr-run[0-9]+-[0-9]{8}-[0-9]{6}$" "$out"
  kept=${out##*logs are kept in }
  check "...a dir with its Hyprland log or box.log" bash -c '[ -f "$1/hyprland.log" ] || [ -f "$1/box.log" ]' _ "$kept"
  check_eq "...and no box of it is left" "" "$(ob ls --json | jq -r --arg p "$P-fr-run" '.[] | select(.name | startswith($p)) | .name')"
  # Past 10 kept dirs the oldest go, and only dirs a box name can have.
  rm -rf "$st"
  local d=$TMP/fr-box i; mkdir -p "$d/home" "$st/omabox/failed-runs/.x"; echo '{}' > "$d/box.json"; echo log > "$d/box.log"
  for i in 01 02 03 04 05 06 07 08 09 10 11; do mkdir "$st/omabox/failed-runs/old-$i"; touch -d "2026-01-$i" "$st/omabox/failed-runs/old-$i"; done
  touch -d 2025-01-01 "$st/omabox/failed-runs/.x"
  kept=$(D=$d NAME=new XDG_STATE_HOME=$st lib keep_run_logs) || true
  check_eq "...the 10 newest kept dirs stay" "$kept old-03 old-04 old-05 old-06 old-07 old-08 old-09 old-10 old-11" \
    "$kept $(cd "$st/omabox/failed-runs" && printf '%s\n' old-* | paste -sd ' ')"
  check "...and a name no box has is left alone" test -d "$st/omabox/failed-runs/.x"
  rm -rf "$st"
}

# An up that fails before box.json leaves no half-made dir that ls and down --all would not see
# (finding 163). /dev/dri/card9 is only a path render_node refuses (not a render node): never bound.
t_up_aborted() {
  local B=$P-abort S=$P-abs out
  out=$(OMABOX_RENDER_NODE=/dev/dri/card9 "$CLI" up "$B" --no-shell 2>&1) && no "up with no usable render node fails" "$out"
  check_eq "up with no usable render node fails, saying so once" "omabox: no usable GPU render node (/dev/dri/renderD*); set OMABOX_RENDER_NODE" "$out"
  check_fails "...and leaves no box dir" test -e "$XDG_RUNTIME_DIR/omabox/$B"
  check_fails "...nor a box HOME" test -e "${XDG_CACHE_HOME:-$HOME/.cache}/omabox/$B"
  check_eq "...nor a box ls lists" "" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .name')"
  # A failure after the dir is made (a save cp cannot read) is cleared by up's EXIT trap.
  mkdir -p "$TMP/data/omabox/saves/$S/home/locked" && chmod 700 "$TMP/data/omabox/saves"
  chmod 000 "$TMP/data/omabox/saves/$S/home/locked"
  check_match "up --from a save it cannot copy fails" "could not copy save '$S'" "$(sv up "$B" --no-shell --from "$S" 2>&1)"
  chmod 700 "$TMP/data/omabox/saves/$S/home/locked"
  check_fails "...and leaves no box dir" test -e "$XDG_RUNTIME_DIR/omabox/$B"
  check_fails "...nor a box HOME" test -e "${XDG_CACHE_HOME:-$HOME/.cache}/omabox/$B"
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
  check_match "ls shows it with none" "$B +headless .* up +[^ ]+ +none " "$(ob ls)"
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
  # up --theme (finding 245) needs no theme of the user's: past that check, it stops at the next one
  # (more monitors on an interactive box, which the system's aquamarine cannot have), before anything is made.
  out=$(HOME=$h XDG_RUNTIME_DIR=$rt XDG_CACHE_HOME=$h/.cache XDG_DATA_HOME=$h/.local/share XDG_CONFIG_HOME=$h/.config XDG_STATE_HOME=$h/.local/state \
    OMABOX_AQUAMARINE=system "$CLI" up nt --theme tokyo-night --interactive --monitor 800x600 2>&1)
  check_match "with --theme: not refused for want of a theme of the user's" "up: --monitor on an interactive box" "$out"
  out=$(HOME=$h XDG_RUNTIME_DIR=$rt XDG_CACHE_HOME=$h/.cache XDG_DATA_HOME=$h/.local/share XDG_CONFIG_HOME=$h/.config XDG_STATE_HOME=$h/.local/state \
    OMABOX_AQUAMARINE=system "$CLI" up nt --theme nope 2>&1)
  check_match "...one that is not there refused" "--theme: no theme nope in " "$out"
  check_fails "...neither makes the box's dirs" test -e "$h/.cache/omabox/nt" -o -e "$rt/omabox/nt"
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
  aq_symbol_checks "$d"
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

# finding 246 (issue #48): the system's aquamarine has the fix when it exports what #415 added, whatever
# its version says (a package patched with #415 keeps the version it patched); what needs the fix
# follows. On real libraries: omabox's build (0.15.1@7bb8bdf4 and its patches) and the stock 0.15.0.
aq_symbol_checks() {
  local d=$1/sym so out real="" c common
  so=$(lib hypr_aq_soname) || { no "the soname Hyprland links"; return; }
  mkdir -p "$d/stub" "$d/patched" "$d/stock" "$d/home/.local/state/omarchy/current/theme" "$d/boxes" \
    "$d/dri" "$d/drm/renderD129" "$d/pci/0000:01:00.0" "$d/drivers/nvidia"
  # sys DIR: which aquamarine boxes run when DIR is the system's (no private build).
  sys() { OMABOX_AQUAMARINE=system lib eval "AQ_SYSDIR='$1'; aq_resolve '$so' && echo \"\$AQ_KIND|\$AQ_VER|\$AQ_FIXED|\$(aq_desc)\"" 2>&1; }
  # A library of the stock version exporting #415's CWaylandOutput::applyConfigure, and nothing else.
  if command -v cc >/dev/null; then
    printf 'void f(void) __asm__("%s");\nvoid f(void) {}\n' "$(lib eval 'echo $AQ_FIX_SYMBOL')" > "$d/fix.c"
    cc -shared -fPIC -Wl,-soname,"$so" -o "$d/stub/libaquamarine.so.0.15.0" "$d/fix.c"
    ln -s libaquamarine.so.0.15.0 "$d/stub/$so"
    check_match "a system 0.15.0 exporting #415's applyConfigure: fixed, said patched" \
      "^system\|0\.15\.0\|1\|the system's, 0\.15\.0, patched with the fix" "$(sys "$d/stub")"
    aq_setup_checks "$d" "$so"
  else
    skip "a stub 0.15.0 exporting #415's symbol has the fix" "no C compiler"
  fi
  # omabox's own build: this checkout's, the main checkout's for a worktree, or the user's.
  common=$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || common=""
  for c in "$ROOT/build/prefix/lib/$so" "${common:+${common%/.git}/build/prefix/lib/$so}" \
    "${XDG_DATA_HOME:-$HOME/.local/share}/omabox/aquamarine/lib/$so"; do
    [ -n "$c" ] && [ -e "$c" ] && { real=$(readlink -f "$c"); break; }
  done
  if [ -n "$real" ]; then
    check "omabox's build ($real) exports #415's symbol" lib aq_has_fix "$real"
    # As the system's, under the stock version's name: the version says unfixed, the symbol fixed.
    cp "$real" "$d/patched/libaquamarine.so.0.15.0"; ln -s libaquamarine.so.0.15.0 "$d/patched/$so"
    check_match "...as the system's 0.15.0 (a patched package): fixed, not by its version" "^system\|0\.15\.0\|1\|" "$(sys "$d/patched")"
  else
    skip "omabox's aquamarine build has the fix by its symbol" "no build (omabox setup --aquamarine)"
  fi
  # The system's own, whatever its package revision: a release up to AQ_FIXED_AFTER lacks the fix unless
  # a package patched it in, which is news (UPSTREAM.md's drop list), not a failure; a later release
  # has it by its version, the symbol or not.
  local sysv=""
  if [ -e "/usr/lib/$so" ]; then
    sysv=$(readlink -f "/usr/lib/$so"); sysv=${sysv##*.so.}
    cp "$(readlink -f "/usr/lib/$so")" "$d/stock/libaquamarine.so.$sysv"; ln -s "libaquamarine.so.$sysv" "$d/stock/$so"
    if lib aq_ver_fixed "$sysv"; then
      note "the system's aquamarine is $sysv: time for UPSTREAM.md's drop list"
      check_match "the system's $sysv, a release after $(lib eval 'echo $AQ_FIXED_AFTER'): fixed by its version" "^system\|[^|]*\|1\|" "$(sys "$d/stock")"
    elif lib aq_has_fix "/usr/lib/$so"; then
      note "the system's aquamarine $sysv exports #415's symbol (a patched package): time for UPSTREAM.md's drop list"
      check_match "the system's $sysv, patched with #415: fixed, said patched" "^system\|[^|]*\|1\|.*patched with the fix" "$(sys "$d/stock")"
    else
      check_match "the system's stock $sysv lacks #415's symbol, so unfixed, said so" "^system\|[^|]*\|0\|.*without the fix" "$(sys "$d/stock")"
    fi
  else
    skip "the system's aquamarine has the fix as its library says" "no /usr/lib/$so here"
  fi
  # What follows the answer: confirm-close in the settings (the widget's switch), and `up` of a headless
  # box on NVIDIA (t_unit_gpu's fake GPU), stopped at the box's lock, which comes after the refusals.
  : > "$d/dri/renderD129"; ln -s "$d/pci/0000:01:00.0" "$d/drm/renderD129/device"; ln -s "$d/drivers/nvidia" "$d/pci/0000:01:00.0/driver"
  gate() {
    HOME=$d/home OMABOX_AQUAMARINE=system OMABOX_RENDER_NODE=$d/dri/renderD129 lib eval "AQ_SYSDIR='$1' DRI='$d/dri' SYSDRM='$d/drm'
      BOXES='$d/boxes' CONFIG='$d/home/config' KEYBOARD=/usr/bin/true POINTER=/usr/bin/true STILL=/usr/bin/true EVENTS=/usr/bin/true; $2" 2>&1
  }
  local dir avail refused
  for dir in stub patched stock; do
    [ -e "$d/$dir/$so" ] || continue
    if [ "$(sys "$d/$dir" | cut -d'|' -f3)" = 1 ]; then avail=true refused=0; else avail=false refused=1; fi
    check_eq "the $dir system's: config --json confirm-close-available $avail" "$avail" \
      "$(gate "$d/$dir" 'cmd_config --json 2>/dev/null' | jq '."confirm-close-available"')"
    out=$(gate "$d/$dir" "lock_file() { echo 'at the lock'; exit 0; }; cmd_up '$P-aqs'")
    if [ "$refused" = 1 ]; then
      check_match "...up of a headless box on NVIDIA refused" "a headless box on NVIDIA \($d/dri/renderD129\) needs aquamarine's fix" "$out"
    else
      check_match "...up of a headless box on NVIDIA not refused (stopped at its lock)" "at the lock$" "$out"
    fi
  done
  check_fails "...no box made" test -e "$d/boxes/$P-aqs"
}

# finding 247 (issue #186): `setup --aquamarine` builds whenever boxes would run without omabox's own
# patches, a system aquamarine with #415 (aq_symbol_checks's stub) included; only this omabox's build
# is a no-op. Stopped at the clone: the build tools on PATH are stubs that log and fail, and the copy
# of the CLI `lib` runs has no .git, so it would build into the scratch data dir, never a real one.
aq_setup_checks() {
  local d=$1 so=$2 t out build
  local tools=$d/setup-tools data=$d/setup-data cache=$d/setup-cache
  mkdir -p "$tools" "$data" "$cache"
  for t in git cmake ninja hyprwayland-scanner pkg-config c++; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/log"\nexit 1\n' "$t" "$tools" > "$tools/$t"; chmod +x "$tools/$t"
  done
  # setup_aq [FORCE]: setup --aquamarine with the stub as the system's 0.15.0, on stub build tools.
  setup_aq() {
    : > "$tools/log"
    XDG_DATA_HOME=$data XDG_CACHE_HOME=$cache PATH=$tools:$PATH \
      lib eval "AQ_SYSDIR='$d/stub'; setup_aquamarine ${1:-0}" 2>&1
  }
  out=$(setup_aq)
  check_fails "setup --aquamarine over a system aquamarine with #415: not \"nothing to build\"" grep -q "nothing to build" <<<"$out"
  check_match "...says it lacks omabox's own fixes, and builds them" \
    "has the fix for nested Wayland outputs but not omabox's own \(keys held .*\).*building aquamarine [^ ]+ .* into $data/omabox/aquamarine" "$out"
  check_match "...as far as the clone (the stub git), into the scratch cache" \
    "^git clone -q https://github.com/hyprwm/aquamarine $cache/omabox-aquamarine$" "$(cat "$tools/log")"
  # A build of this omabox's (AQ_BUILD) in the data dir: the only no-op, and --force builds all the same.
  build=$(lib eval 'echo $AQ_BUILD')
  mkdir -p "$data/omabox/aquamarine/lib"
  cp "$d/stub/libaquamarine.so.0.15.0" "$data/omabox/aquamarine/lib/libaquamarine.so.0.15.1"
  ln -s libaquamarine.so.0.15.1 "$data/omabox/aquamarine/lib/$so"
  echo "$build" > "$data/omabox/aquamarine/.omabox-commit"
  out=$(setup_aq)
  check_match "...over this omabox's build ($build): nothing to build" "this omabox's build: nothing to build" "$out"
  check_eq "...no tool run" "" "$(cat "$tools/log")"
  out=$(setup_aq 1)
  check_match "...--force builds it all the same" "building aquamarine .*could not clone" "$(tr '\n' ' ' <<<"$out")"
  echo 7bb8bdf4+keys > "$data/omabox/aquamarine/.omabox-commit"
  out=$(setup_aq)
  check_match "...over an older build of omabox's: builds" "building aquamarine .*could not clone" "$(tr '\n' ' ' <<<"$out")"
  rm -rf "$data/omabox/aquamarine"
  # What `setup` says of each (aq_note), and `monitor add` of a box's aquamarine (aq_monitor_note).
  note_of() { lib eval "AQ_KIND='$1' AQ_LIB='$data/lib' AQ_VER='$2' AQ_FIXED='$3'; aq_note" 2>&1; }
  check_match "setup's note on a system aquamarine with #415: omabox's own fixes, setup --aquamarine" \
    "^note: the system's aquamarine has the fix .*, but not omabox's own fixes \(keys held .*\): omabox setup --aquamarine$" "$(note_of system 0.15.0 1)"
  check_match "...without #415: both" "^note: headless boxes on NVIDIA and confirm-close need .*, and interactive boxes omabox's own fixes .*: omabox setup --aquamarine$" \
    "$(note_of system 0.15.0 0)"
  check_match "...an older build of omabox's: older" "^note: the aquamarine build in $data is older than this omabox's" "$(note_of private 0.15.1@7bb8bdf4+keys 1)"
  check_eq "...this omabox's build: nothing" "" "$(note_of private "0.15.1@$build" 1)"
  mnote() { lib eval "NAME=b; aq_monitor_note '$1'" 2>&1; }
  check_match "monitor add on a system aquamarine (#415 or not): the pointer's note, sending to setup --aquamarine" \
    "aquamarine \(0\.15\.0\) puts your pointer in a monitor's window in the wrong place.*: omabox setup --aquamarine, then a new box$" "$(mnote 0.15.0)"
  check_match "...on a build without +cursor: the cursor's" "hides your cursor .*: omabox setup --aquamarine, then a new box$" "$(mnote 0.15.1@7bb8bdf4+keys+layout)"
  check_eq "...on this omabox's build: none" "" "$(mnote "0.15.1@$build")"
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
  check_box_safety "$B"
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

# An up killed once its box started (SIGKILL: no EXIT trap; a killed suite run, a harness's timeout)
# left a box with no reaper, up for good (#137). Its reaper starts with the box now, and does not
# count a slow start as idle time.
t_up_killed() {
  local B=$P-uk
  "$CLI" up "$B" --idle 20s --net isolated >/dev/null 2>"$TMP/uk.err" & local up=$!   # $!: omabox itself, not a subshell
  until_ok 30 jq -e .pidns "$XDG_RUNTIME_DIR/omabox/$B/box.json" >/dev/null
  kill -KILL "$up" 2>/dev/null; sleep 0.2
  check_fails "the up killed once its box started, before it was done" grep -q "box '$B' up (" "$TMP/uk.err"
  check "...its box has a reaper all the same (#137)" until_ok 3 pgrep -f "omabox _reap $B "
  # shellcheck disable=SC2329 # called through until_ok
  gone() { ! "$CLI" ls --json | jq -e --arg n "$B" '.[] | select(.name == $n)' >/dev/null; }
  check "...which takes it down at its idle limit" until_ok 60 gone
  ob down "$B" >/dev/null 2>&1
  # Finding 237: killed sooner, once bwrap has started (info.json) but before up has found the box's
  # PID 1 and recorded its pid namespace (up waits up to 10 s for that): stopped there, then killed.
  B=$P-uk3
  local bd=$XDG_RUNTIME_DIR/omabox/$B end=$((SECONDS + 30))
  "$CLI" up "$B" --idle 20s --no-shell --net isolated >/dev/null 2>&1 & up=$!
  until [ -s "$bd/info.json" ] || [ "$SECONDS" -ge "$end" ]; do sleep 0.002; done
  kill -STOP "$up" 2>/dev/null
  if [ ! -s "$bd/info.json" ]; then no "up ($B): bwrap started" "no info.json in 30 s"
  elif jq -e .pidns "$bd/box.json" >/dev/null 2>&1; then
    kill -KILL "$up" 2>/dev/null
    skip "an up killed before it recorded its box's pid namespace" "it recorded it before it could be stopped (load)"
  else
    kill -KILL "$up" 2>/dev/null
    check "an up killed before it recorded its box's pid namespace: the box has a reaper (finding 237)" until_ok 3 pgrep -f "omabox _reap $B "
    check "...which records it" until_ok 20 jq -e .pidns "$bd/box.json"
    check "...and takes the box down at its idle limit" until_ok 60 gone
  fi
  ob down "$B" >/dev/null 2>&1
  # While its up still runs (here: this shell, as up writes itself in `starting`), idle time is not
  # counted; once that process is gone, it is.
  B=$P-uk2
  ob up "$B" --idle 20s --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  local D; D=$(ob path "$B")
  sleep 30 & local starter=$!
  echo "$starter $(lib proc_start "$starter")" > "$D/starting"; touch -d '-1 hour' "$D/used"
  check "a box whose up still runs is not idle, whatever its last use" holds 12 bash -c "'$CLI' ls --json | jq -e --arg n '$B' '.[] | select(.name == \$n and .state == \"up\")'"
  kill "$starter" 2>/dev/null; wait "$starter" 2>/dev/null
  check "...its up gone, the idle limit counts" until_ok 20 gone
  ob down "$B" >/dev/null 2>&1
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
  # 15 s: a run every 3 s, and room for one slow under load (#116: 10 s left a gap near the limit).
  (cd "$repo" && "$CLI" up --idle 15s --no-shell --net isolated >/dev/null 2>&1)
  for _ in 1 2 3 4 5; do (cd "$repo" && "$CLI" run -- true); sleep 3; done
  check "a box used only through run stays up" bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$P-exp\" and .state == \"up\")'"
  until_ok 40 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$P-exp\")'"
  held "$P-exp" check_match "after expiry run reports it (no throwaway)" "went down after 15s idle" "$(cd "$repo" && "$CLI" run -- true 2>&1)"
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
  local j=("$XDG_RUNTIME_DIR"/omabox/"$P"-tk-run*/box.json)
  check "its box.json records its owner's start time (a reused pid is not it)" \
    jq -e --arg s "$(lib proc_start "$r")" '.owner_start == $s and $s != ""' "${j[0]}"
  kill -KILL "$r"
  check "its box goes within 15 s" until_ok 15 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name | startswith(\"$P-tk-run\"))'"
}

# The keyboard and pointer tools (finding 64): keys named as binds name them, input checked before any
# of it is sent, spare keycodes X11 apps can see, and a lost box is a failure.
t_keys() {
  local B=$P-keys
  ob up "$B" --no-shell --net isolated --xwayland >/dev/null 2>&1 || { no "up" "failed"; return; }
  check_box_safety "$B"
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
  # Control characters (#104, finding 173): xkb maps them to keys (BackSpace, Escape, Delete, Return).
  check_eq "a control character in -t refused: exit 2" 2 "$(ob keys -b "$B" -t $'ab\x08c' >/dev/null 2>&1; echo $?)"
  check_match "...said, with where" "control character \(U\+0008\) at byte 2; only newline and tab are typed" "$(ob keys -b "$B" -t $'ab\x08c' 2>&1)"
  check_eq "...escape, delete and a carriage return too" "2 2 2" "$(for c in $'\x1b' $'\x7f' $'\r'; do ob keys -b "$B" -t "x${c}y" >/dev/null 2>&1; printf '%s ' $?; done | xargs)"
  check_eq "...in a --pass value too" 2 "$(T_BS=$'a\x08b' "$CLI" keys -b "$B" --pass T_BS >/dev/null 2>&1; echo $?)"
  ob keys -b "$B" + - / shift+a -t $'\t1' Return ctrl+d >/dev/null
  until_ok 5 ob run -b "$B" -- test -s /tmp/typed
  check_eq "+ - / and a tab typed, nothing from the refused runs" $'+-/A\t1' "$(ob run -b "$B" -- cat /tmp/typed)"
  # keys --pass (finding 84): the caller's variable is typed, and no process's argv has it meanwhile.
  ob run -b "$B" -d -- foot sh -c 'cat > /tmp/secret' >/dev/null
  until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '[.[] | select(.class == \"foot\")] | length == 1'"
  local seen=""
  T_PW="pw-$P x=y Ü" T_CR=$'cr\r' "$CLI" keys -b "$B" -t a --pass T_PW -s 1500 -t -T --pass T_PW --pass T_CR Return ctrl+d & local kp=$!
  if until_ok 10 pgrep -f 'omabox-keyboard .* -T -s 1500'; then
    seen=$(grep -l "pw-$P" /proc/[0-9]*/cmdline 2>/dev/null)
    check_eq "keys --pass: the value is in no /proc/*/cmdline while typing" "" "$seen"
  else no "keys --pass: the keyboard ran (to scan while typing)"; fi
  wait "$kp"; check_eq "keys --pass exits 0" 0 $?
  until_ok 5 ob run -b "$B" -- test -s /tmp/secret
  check_eq "keys --pass types the value, in order with -t (and -t -T is text; a trailing \\r dropped, not a Return)" "apw-$P x=y Ü-Tpw-$P x=y Ücr"$'\n.' "$(ob run -b "$B" -- sh -c 'cat /tmp/secret; echo .')"
  check_match "keys --pass: an unset variable refused" "not in this command's environment" "$(env -u T_NONE "$CLI" keys -b "$B" --pass T_NONE 2>&1)"
  check_match "keys --pass: a bad name refused" "takes a variable name" "$(ob keys -b "$B" --pass 'a b' 2>&1)"
  if command -v zenity >/dev/null; then
    ob run -b "$B" -d -- sh -c 'GDK_BACKEND=x11 zenity --entry --text t > /tmp/x11' >/dev/null
    until_ok 15 bash -c "'$CLI' hyprctl -b '$B' -j activewindow | jq -e '.class == \"zenity\" and .xwayland'"
    sleep 1   # mapped is not yet taking keys
    # € too (#85: it did not arrive here in 2026-09, with Ü and α on the same kind of spare keycode).
    ob keys -b "$B" -t 'aÜb€α😀' -s 300 Return >/dev/null
    until_ok 5 ob run -b "$B" -- test -s /tmp/x11
    check_eq "an X11 app gets characters outside the layout" 'aÜb€α😀' "$(ob run -b "$B" -- cat /tmp/x11)"
  else skip "an X11 app gets characters outside the layout" "no zenity"; fi
  check "pointer: click, then move" ob pointer -b "$B" -- click move 10 10
  check_eq "pointer: sleep -1 refused" 2 "$(timeout 5 "$CLI" pointer -b "$B" -- sleep -1 >/dev/null 2>&1; echo $?)"
  # A compositor alive but stopped (#108, finding 177): each tool gives up after 10 s with exit 3
  # instead of waiting forever (timeout 40: on the old tools, 124). Stopped once they are past the
  # CLI's own hyprctl calls, in their -s / sleep.
  timeout 40 "$CLI" keys -b "$B" -s 3003 a >/dev/null 2>"$TMP/stop.k" & local k=$!
  timeout 40 "$CLI" pointer -b "$B" -- sleep 3004 move 10 10 >/dev/null 2>"$TMP/stop.p" & local p=$!
  until_ok 10 pgrep -f 'omabox-keyboard .* -s 3003 a$' >/dev/null; until_ok 10 pgrep -f 'omabox-pointer .* sleep 3004 ' >/dev/null
  ob run -b "$B" -- sh -c 'kill -STOP $(pidof Hyprland)'
  local t0=$SECONDS rk=0 rp=0
  wait "$k" || rk=$?; wait "$p" || rp=$?
  local took=$((SECONDS - t0))
  ob run -b "$B" -- sh -c 'kill -CONT $(pidof Hyprland)'
  check_eq "keys and pointer give up on a stopped compositor: exit 3" "3 3" "$rk $rp"
  check_match "...said" "keyboard: .*no answer in 10 s.*pointer: .*no answer in 10 s" "$(cat "$TMP/stop.k" "$TMP/stop.p" | tr '\n' ' ')"
  check "...in about 10 s ($took s after the stop)" test "$took" -le 16
  check "...and the box answers again once it goes on" until_ok 10 ob keys -b "$B" shift
  ob keys -b "$B" -s 3001 a >/dev/null 2>&1 & k=$!
  ob pointer -b "$B" -- sleep 3002 move 10 10 >/dev/null 2>&1 & p=$!
  # (#110) Once each tool runs in the box: before that, "not up" is exit 1 too.
  until_ok 10 pgrep -f 'omabox-keyboard .* -s 3001 a$' >/dev/null; until_ok 10 pgrep -f 'omabox-pointer .* sleep 3002 ' >/dev/null
  ob down "$B" >/dev/null
  wait "$k"; check_eq "keys exits 1 when the box goes mid-run" 1 $?
  wait "$p"; check_eq "pointer exits 1 when the box goes mid-run" 1 $?
}

# A box whose Hyprland is alive but does not answer (stopped here; in use, a plugin under test's
# deadlock): what asks it ends, saying so in words (#124, #125, findings 181-182). Continued, it
# answers again.
t_hung() {
  local B=$P-hung out rc t0 took
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  ob run -b "$B" -d -q -- foot -T F sleep 600
  ob wait -b "$B" window 'title:^F$' >/dev/null
  ob run -b "$B" -- pkill -STOP -x Hyprland
  # #166: the first call to a Hyprland just stopped, its listen backlog still empty, is not left
  # waiting in connect: hyprctl gives up by itself after 5 s, exit 6, and printed its own "IPC didn't
  # respond in time" on stdout (#139 caught only the 124 of a full backlog, which the calls below fill).
  t0=$(now_ms); out=$(timeout 40 "$CLI" hyprctl -b "$B" clients 2>&1); rc=$?; took=$(($(now_ms) - t0))
  check_eq "hyprctl clients, the first call to a stopped Hyprland: exit 1 (#166)" 1 "$rc"
  check_match "...said in words" "^omabox: box '$B': its Hyprland did not answer( in 10 s)? \(hung\? omabox log -b $B; omabox down $B\)$" "$out"
  check "...within 12 s ($took ms)" test "$took" -lt 12000
  # The tool `wait` runs, in the box: its first round trip has a deadline (on 0.4.8 it never returned).
  t0=$(now_ms); out=$(timeout 40 "$CLI" run -b "$B" -- /opt/omabox/bin/omabox-still still --timeout 3000 2>&1); rc=$?
  took=$(($(now_ms) - t0))
  check_eq "omabox-still on a stopped Hyprland: unknown, exit 1 (not timeout's 124)" 1 "$rc"
  check_match "...hung" "^unknown hung " "$out"
  check "...in about 10 s ($took ms)" test "$took" -lt 16000
  t0=$(now_ms); out=$(timeout 40 "$CLI" wait -b "$B" still --timeout 3s 2>&1); rc=$?; took=$(($(now_ms) - t0))
  check_eq "wait still --timeout 3s: exit 1, not timeout's 124" 1 "$rc"
  check_match "...said in words" "box '$B': its Hyprland did not answer( in 10 s)? \(hung\? omabox log -b $B; omabox down $B\)$" "$out"
  check "...within 9 s ($took ms; its first hyprctl's 5 s, finding 182)" test "$took" -lt 9000
  # Each of these ends in a few seconds with those words, never a line of jq's (#125, finding 182):
  # on 0.4.8 windows printed jq's usage after 15 s, shot -g "grim failed" after 10, and ls said up.
  hung_says() {
    local n=$1 t0 out rc took; shift
    t0=$(now_ms); out=$(timeout 40 "$CLI" "$@" 2>&1); rc=$?; took=$(($(now_ms) - t0))
    if [ "$rc" != 0 ] && [ "$rc" != 124 ] && [[ $out == *"box '$B': its Hyprland did not answer"* ]] && [[ $out != *jq:* ]] &&
       [ "$took" -lt "${HUNG_MAX:-9000}" ]; then ok "$n ($took ms)"; else no "$n" "exit $rc after $took ms: $out"; fi
  }
  hung_says "keys --wait: said, within 9 s" keys -b "$B" --wait a
  hung_says "windows: said, no jq line, within 9 s" windows -b "$B"
  hung_says "click --window: said" click -b "$B" --window foot 1 1
  hung_says "shot -g: said, not \"grim failed\"" shot -b "$B" -g "0,0 100x100"
  hung_says "shot: said" shot -b "$B"
  hung_says "wait window: said" wait -b "$B" window foot
  hung_says "wait layer: said" wait -b "$B" layer omarchy-bar
  # #139: these two passed straight to hyprctl, which then waited in connect for good (on 804f2d5 the
  # outer timeout ended them). Their limit is 10 s.
  HUNG_MAX=16000 hung_says "hyprctl clients: said, within 16 s" hyprctl -b "$B" clients
  HUNG_MAX=16000 hung_says "lua 'return 1': said, within 16 s" lua -b "$B" 'return 1'
  check_match "ls: hung, not up" "^$B +headless +[^ ]+ +hung " "$(ob ls | grep "^$B ")"
  check_eq "ls --json: hung, its state still up (the bar widget offers Down for any other)" "up true" \
    "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | "\(.state) \(.hung)"')"
  ob run -b "$B" -- pkill -CONT -x Hyprland
  check_match "continued: wait still is satisfied again" "^satisfied: still" "$(ob wait -b "$B" still --timeout 5s 2>&1)"
  check_match "...ls says up" "^$B +headless +[^ ]+ +up " "$(ob ls | grep "^$B ")"
  check_eq "...ls --json: not hung" "up false" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | "\(.state) \(.hung)"')"
  check "...and windows answers" ob windows -b "$B"
  check_eq "...hyprctl and lua answer as before" "1" "$(ob lua -b "$B" 'return 1')"
  # ...and what follows is not cut at hyprctl's limit: still following past it, ended from outside.
  out=$(timeout 13 "$CLI" hyprctl -b "$B" rollinglog -f 2>&1); rc=$?
  check_eq "hyprctl rollinglog -f still follows past the 10 s limit (#139)" 124 "$rc"
  check_fails "...and is not called hung" grep -q "did not answer" <<<"$out"
  ob down "$B" >/dev/null
}

# How up and restart-shell tell a shell that crashed as it started (#126, finding 183): a new report
# folder, a crash line in shell.log, or its pid gone; then what they say.
t_unit_shell_crash() {
  local d=$TMP/sc
  mkdir -p "$d/home/.cache/quickshell/crashes/old" "$d/run"; : > "$d/home/shell.log"; echo 42 > "$d/run/omabox-shell.pid"
  sc() {   # STAT: what /proc/PID/stat says in the box ("" for no such process)
    STAT=$1 bash -c 'source "$1"; NAME=u D=$2; SHELL_BEFORE=old
      on_box() { [ -n "$STAT" ] && echo "$STAT"; }
      shell_crash "$(shell_pid)" && echo "rc 0" || echo "rc $?"' _ "$TMP/lib/bin/omabox" "$d" 2>&1 | tr '\n' ' '
  }
  check_eq "no new report, no crash line, the shell running: no crash" "rc 1 " "$(sc "42 (quickshell) S 1")"
  mkdir "$d/home/.cache/quickshell/crashes/q1k4dmt"
  check_eq "a new report folder: crashed, its report named" "crashed $d/home/.cache/quickshell/crashes/q1k4dmt/report.txt rc 0 " "$(sc "42 (quickshell) S 1")"
  check_eq "...said with it" "warning: the shell crashed after starting (report: /r/report.txt; omabox log -b u shell)" \
    "$(bash -c 'source "$1"; NAME=u; shell_crash_text after "crashed /r/report.txt"' _ "$TMP/lib/bin/omabox")"
  rmdir "$d/home/.cache/quickshell/crashes/q1k4dmt"; mkdir "$d/home/.cache/quickshell/crashes/a b"
  check_eq "a folder named oddly (the box names it): crashed, no path" "crashed - rc 0 " "$(sc "42 (quickshell) S 1")"
  rmdir "$d/home/.cache/quickshell/crashes/a b"
  printf ' ERROR: Quickshell has crashed under pid 43\n' > "$d/home/shell.log"
  check_eq "only the log's crash line: crashed" "crashed - rc 0 " "$(sc "42 (quickshell) S 1")"
  check_eq "...said without a report" "warning: the shell crashed while starting (omabox log -b u shell)" \
    "$(bash -c 'source "$1"; NAME=u; shell_crash_text while "crashed -"' _ "$TMP/lib/bin/omabox")"
  : > "$d/home/shell.log"
  check_eq "its pid gone: exited" "exited rc 0 " "$(sc "")"
  check_eq "...or a zombie" "exited rc 0 " "$(sc "42 (quickshell) Z 1")"
  check_eq "...said as exited" "warning: the shell exited after starting (omabox log -b u shell)" \
    "$(bash -c 'source "$1"; NAME=u; shell_crash_text after exited' _ "$TMP/lib/bin/omabox")"
  check_eq "the report folders before a restart are not a crash" "rc 1 " "$(sc "42 (quickshell) S 1")"
}

# A shell that crashes as restart-shell starts it (#126, finding 183): no "restarted", exit 1, the
# report named; Quickshell's crash dialog, which took the box's keys, closed as it opens (the box's
# Hyprland does it), the report kept. Also past 10 s, when Quickshell restarts the shell itself.
t_shell_crash() {
  local B=$P-crash D H out rc old p i rs rep
  ob up "$B" --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  D=$XDG_RUNTIME_DIR/omabox/$B H=$(ob path "$B")/home
  old=$(cat "$D/run/omabox-shell.pid")
  ob restart-shell -b "$B" > "$TMP/crash.out" 2>&1 & rs=$!
  # The new shell (its pid written, its log started), crashed once its config has loaded.
  p=""; for i in $(seq 300); do
    p=$(cat "$D/run/omabox-shell.pid" 2>/dev/null)
    [ -n "$p" ] && [ "$p" != "$old" ] && grep -aq 'Configuration Loaded' "$H/shell.log" 2>/dev/null && break
    sleep 0.05
  done
  ob run -b "$B" -- kill -SEGV "$p"
  wait "$rs"; rc=$?; out=$(cat "$TMP/crash.out")
  check_eq "restart-shell, the new shell crashing: exit 1 (was 0)" 1 "$rc"
  check_fails "...never \"shell restarted\"" grep -q "shell restarted" <<<"$out"
  check_match "...says it crashed, naming the report" \
    "(warning|omabox): the shell crashed (while|after) starting \(report: $H/\.cache/quickshell/crashes/[A-Za-z0-9_.-]+/report\.txt; omabox log -b $B shell\)" "$out"
  rep=$(sed -n 's/.*(report: \([^;]*\);.*/\1/p' <<<"$out")
  check "...which is there (${rep##*/crashes/})" test -s "$rep"
  check "the crash dialog is closed as it opens (said in shell.log)" until_ok 10 grep -q "closed the shell's crash dialog" "$H/shell.log"
  # #151: as on a user's desktop, Omarchy's launcher starts it again (Quickshell does not, so soon after
  # its start): the box had no bar left.
  check "...the bar back: Omarchy's launcher started the shell again (#151)" until_ok 20 ob wait -b "$B" layer omarchy-bar
  check "...a new shell, its pid recorded" until_ok 5 bash -c "p=\$(cat '$D/run/omabox-shell.pid'); [ -n \"\$p\" ] && [ \"\$p\" != '$p' ]"
  check "...the launcher's reason in shell.log (the box's logger)" grep -q "omarchy-shell: Omarchy shell exited with status [0-9]*; relaunching" "$H/shell.log"
  check_eq "...no org.quickshell window is left to take keys" 0 "$(ob windows -b "$B" --json | jq '[.[] | select(.class == "org.quickshell")] | length')"
  out=$(ob restart-shell -b "$B" 2>&1); rc=$?
  check_eq "a restart that works: exit 0, \"restarted\" (the earlier report is not a new crash)" "0 yes" "$rc $(grep -q 'shell restarted' <<<"$out" && echo yes)"
  # Past 10 s Quickshell restarts the shell itself; its dialog came all the same.
  p=$(cat "$D/run/omabox-shell.pid"); sleep 11
  ob windows -b "$B" >/dev/null 2>&1   # (any crash before is said by now)
  ob run -b "$B" -- kill -SEGV "$p"
  check "a crash past 10 s: Quickshell restarts the shell" until_ok 15 grep -q "Quickshell has been restarted" "$H/shell.log"
  # #147: it keeps its pid, so nothing else would tell; the next command says it, once.
  sleep 1
  check_match "...the next command says it crashed (#147)" "warning: the shell crashed since the last command and was started again" \
    "$(ob windows -b "$B" 2>&1 >/dev/null)"
  check_fails "...once" grep -q "crashed since" <<<"$(ob windows -b "$B" 2>&1 >/dev/null)"
  check "...its dialog closed too" until_ok 10 grep -q "closed the shell's crash dialog" "$H/shell.log"
  check "...the bar back" ob wait -b "$B" layer omarchy-bar
  check_eq "...no org.quickshell window" 0 "$(ob windows -b "$B" --json | jq '[.[] | select(.class == "org.quickshell")] | length')"
  check_match "ls: the shell running, and its crashes since up (#147)" "^$B +headless +[^ ]+ +up +running\+2 " "$(ob ls | grep "^$B ")"
  # #147: a box with no shell left read as healthy to every command. Its launcher stopped, it is gone.
  # shellcheck disable=SC2329 # called through until_ok
  shell_is() { [ "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .shell_state')" = "$1" ]; }
  ob run -b "$B" -- pkill -f '[o]marchy-launch-shell'
  check "the launcher stopped, the shell with it: ls --json says gone" until_ok 10 shell_is gone
  check_match "...ls too" "^$B +headless +[^ ]+ +up +gone\+2 " "$(ob ls | grep "^$B ")"
  check_match "...and shot warns" "warning: box '$B' has no shell \(gone after 2 crash\(es\); omabox log -b $B shell; omabox restart-shell -b $B\)" \
    "$(ob shot -b "$B" -o "$TMP/gone.png" 2>&1 >/dev/null)"
  check_eq "...every time" 1 "$(ob shot -b "$B" -o "$TMP/gone.png" 2>&1 >/dev/null | grep -c 'has no shell')"
  check "restart-shell brings it back" ob restart-shell -b "$B"
  check "...running" shell_is running
  check_fails "...shot warns no more" grep -q shell <<<"$(ob shot -b "$B" -o "$TMP/back.png" 2>&1 >/dev/null)"
  ob down "$B" >/dev/null
}

# Accent pixels (#ff3cc8, peek's marks) in the screen of box $B (the caller's) within X0 Y0 X1 Y1:
# "COUNT CX CY", the centre of their bounding box. With ACCENT_KEEP set, in the screen the last call
# captured (two regions of one frame).
# shellcheck disable=SC2329 # called through accent_ok and no_accent
accent() {
  [ -n "${ACCENT_KEEP:-}" ] || ob run -b "$B" -- grim -t ppm - > "$TMP/peek-$B.ppm"
  python3 - "$TMP/peek-$B.ppm" "$@" <<'PY'
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

# #164, finding 233: `omabox peek` of a box with two monitors, a window each, in a box standing in for
# the host (finding 26: peek is a host window). Marks are in layout coordinates: each window draws the
# ones on its own monitor, at that monitor's place and scale; both windows get them (a second peek
# made the marks file anew, lost to the first); `down` closes every one (it closed the first).
t_peek_monitors() {
  local n S=$P-pkm C=$P-pkmc
  n=$(gpu_node other)
  [ -n "$n" ] || { skip "peek with monitors" "no render node here but NVIDIA's (a stand-in there is not tried; finding 234)"; return; }
  env OMABOX_RENDER_NODE="$n" "$CLI" up "$S" --no-shell --net isolated >/dev/null 2>&1 || { no "up the stand-in" "failed"; return; }
  local in=("$CLI" run -b "$S" -- "${GUARDED[@]}" "$CLI")
  # (The stand-in renders on that node, and so does a box in it.) The second monitor is 1280x720
  # logical, 1600x900 pixels, at 1920,0.
  check "up a box with a second monitor at scale 1.25 in the stand-in" "${in[@]}" up "$C" --no-shell --idle 0 --monitor 1600x900,scale=1.25
  check_eq "...right of the main screen" "1.25 1920 0" \
    "$("${in[@]}" monitor -b "$C" list --json | jq -r '.[] | select(.name == "HEADLESS-3") | "\(.scale) \(.x) \(.y)"')"
  check "peek of the main screen in the stand-in" "${in[@]}" peek "$C"
  check "peek --monitor HEADLESS-3" "${in[@]}" peek "$C" --monitor HEADLESS-3
  # shellcheck disable=SC2329 # called through until_ok
  peeks() { [ "$(ob hyprctl -b "$S" -j clients | jq '[.[] | select(.class == "omabox-peek")] | length')" = "$1" ]; }
  check "two peek windows" until_ok 10 peeks 2
  # Floated where the views map simply: the main screen's at half size at 20,20 (layout X,Y at
  # 20 + X/2, 20 + Y/2), HEADLESS-3's at half its logical size at 1000,600 (1000 + (X-1920)/2, 600 + Y/2).
  local a1 a2
  a1=$(ob hyprctl -b "$S" -j clients | jq -r '.[] | select(.class == "omabox-peek" and (.title | endswith("HEADLESS-3") | not)) | .address')
  a2=$(ob hyprctl -b "$S" -j clients | jq -r '.[] | select(.class == "omabox-peek" and (.title | endswith("HEADLESS-3"))) | .address')
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '9' })" >/dev/null
  place() {   # ADDRESS W H X Y
    ob hyprctl -b "$S" dispatch "hl.dsp.window.float({ window = 'address:$1' })" >/dev/null
    ob hyprctl -b "$S" dispatch "hl.dsp.window.resize({ window = 'address:$1', x = $2, y = $3 })" >/dev/null
    ob hyprctl -b "$S" dispatch "hl.dsp.window.move({ window = 'address:$1', x = $4, y = $5 })" >/dev/null
  }
  place "$a1" 960 540 20 20; place "$a2" 640 360 1000 600
  check_eq "...placed" "[20,20] [960,540] [1000,600] [640,360]" \
    "$(ob hyprctl -b "$S" -j clients | jq -r --arg a "$a1" --arg b "$a2" '[(.[] | select(.address == $a)), (.[] | select(.address == $b))] | map("\(.at | tojson) \(.size | tojson)") | join(" ")')"
  local B=$S   # (accent's box: the stand-in's screen)
  # One frame of the stand-in: a ring around CX,CY in one window, no accent in the other (MIN X0 Y0
  # X1 Y1 CX CY, then the other's X0 Y0 X1 Y1); a caption in both.
  # shellcheck disable=SC2329 # called through until_ok
  ring_only() { accent_ok "$1" "$2" "$3" "$4" "$5" "$6" "$7" && ACCENT_KEEP=1 no_accent "$8" "$9" "${10}" "${11}"; }
  # shellcheck disable=SC2329
  captions() { accent_ok 51 20 440 980 560 && ACCENT_KEEP=1 accent_ok 51 1000 840 1640 960; }
  sleep 0.5
  "${in[@]}" click -b "$C" 700 400 >/dev/null
  check "a click on the main screen: a ring in its peek where it went, none in HEADLESS-3's" \
    until_ok 3 ring_only 21 310 160 430 280 370 220 1000 600 1640 960
  check "...nor after" holds 1 no_accent 1000 600 1640 960
  sleep 3.5
  "${in[@]}" click -b "$C" 2240 360 >/dev/null
  # (The pointer glides 120 ms from the main screen: its ring shows there until it leaves.)
  check "a click on HEADLESS-3: a ring in its peek at its place and scale, none in the main screen's" \
    until_ok 3 ring_only 21 1100 720 1220 840 1160 780 20 20 980 560
  check "...nor after" holds 1 no_accent 20 20 980 560
  sleep 3.5
  "${in[@]}" keys -b "$C" -t hi >/dev/null
  check "keys: a caption in both peeks" until_ok 3 captions
  # down closes every peek window of the box: a process named as a third one (a later pid than the
  # two) goes too, though nothing ties it to the box's socket.
  local d; d=$("${in[@]}" path "$C")
  ob run -b "$S" -d -- bash -c 'exec -a "$1" sleep 300' _ "$ROOT/tools/peek/omabox-peek --box $d/run/x --output HEADLESS-9 --fps 1" >/dev/null 2>&1
  until_ok 5 ob run -b "$S" -- pgrep -f "^$ROOT/tools/peek/omabox-peek --box $d/run/x "
  "${in[@]}" down "$C" >/dev/null 2>&1
  check "down closes both peek windows" until_ok 5 peeks 0
  # shellcheck disable=SC2329 # called through until_ok
  none_left() { ! ob run -b "$S" -- pgrep -f "^$ROOT/tools/peek/omabox-peek --box $d/run/"; }
  check "...and every peek process of the box" until_ok 3 none_left
  ob down "$S" >/dev/null
}

# A throwaway whose run is gone and whose box died before the reaper saw it (a teardown that gave up
# under load left one): the reaper clears it rather than leave a dead box in ls.
t_throwaway_dead() {
  local repo; repo=$(tmp_repo td)
  (cd "$repo" && exec env -u OMABOX "$CLI" run -- sleep 300) & local r=$!
  until_ok 30 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name | startswith(\"$P-td-run\")) | select(.state == \"up\")'"
  local n; n=$(ob ls --json | jq -r ".[] | select(.name | startswith(\"$P-td-run\")) | .name")
  # Its reaper starts with the box (#137), so its `up` may still be waiting for the bar: the box must go
  # within 15 s that way too (#157: up waited out READY_TIMEOUT on a dead box, holding its lock).
  until_ok 30 pgrep -f "omabox _reap $n "
  # The box's PID 1 as the CLI finds it: behind pasta, info.json's child-pid is pasta's numbering (2,
  # which on the host is kthreadd), and the box would stay up for the reaper's "run is gone" branch.
  local pid; pid=$(D=$XDG_RUNTIME_DIR/omabox/$n lib box_pid)
  kill -KILL "$r"
  check "its box is killed before the reaper sees it" kill -KILL "$pid"
  check "the dead box goes within 15 s" until_ok 15 bash -c "! '$CLI' ls --json | jq -e '.[] | select(.name == \"$n\")'"
}

# The still tool's answer split across settle_end's 1 s reads (finding 217): bash keeps what a timed-out
# read had of a line; it was dropped, and "nsatisfied ..." read as no answer ("lost the box's screen").
t_unit_settle_read() {
  local out
  out=$(timeout 20 bash -c 'source "$1"; NAME=u D=$2; box_alive() { return 0; }
    exec {SETTLE_OUT}< <(printf u; sleep 1.5; printf "%s\n" "nsatisfied changing t=2002 first=2 last=1970 change=27,31,7,10 ignored=- why=- late=- frames=42")
    exec {SETTLE_IN}>/dev/null; sleep 0.1 & SETTLE_PID=$!
    SETTLE_LINE="ready 1920x1080" SETTLE_ERR="" SETTLE_T0=$(date +%s%3N); rc=0; settle_end still 0 "" || rc=$?; echo "rc $rc"' _ "$TMP/lib/bin/omabox" "$TMP" 2>&1)
  check_match "an answer split across a read timeout is read whole" "^unsatisfied: still changing after 2\.00s" "$out"
  check_match "...124" "rc 124$" "$out"
  out=$(timeout 20 bash -c 'source "$1"; NAME=u D=$2; on_box() { { printf r; sleep 1.5; printf "eady 1920x1080\n"; sleep 5; }; }
    settle_begin still; echo "line [$SETTLE_LINE] err [$SETTLE_ERR]"' _ "$TMP/lib/bin/omabox" "$TMP" 2>&1)
  check_match "settle_begin: a ready line split across a read timeout too" "line \[ready 1920x1080\] err \[\]" "$out"
}

# up's waits after the bar, on a box that died meanwhile (#157, finding 216): bar_settle retried its
# captures until READY_TIMEOUT (30 s), holding the box's lock, so the reaper's down of the dead box
# waited that long. Here box_alive says gone and every capture fails: it must say so at once.
t_unit_up_dies_late() {
  local out rc=0 t0=$SECONDS
  out=$(timeout 20 bash -c 'source "$1"; NAME=u D=$2; box_alive() { return 1; }; on_box() { return 1; }
    bar_settle "0,0 10x10" $((SECONDS + 15))' _ "$TMP/lib/bin/omabox" "$TMP" 2>&1) || rc=$?
  check_eq "bar_settle on a dead box: exit 1" 1 "$rc"
  check_match "...said: died while starting" "box 'u' died while starting" "$out"
  check "...at once, not at the deadline ($((SECONDS - t0)) s)" test $((SECONDS - t0)) -le 3
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
    "$("${guard[@]}" | grep -iE '^(Claude Code|Codex)' | tr '\n' ' ')"
  rm "$h/.config/omabox/guard-declined"; printf '%s\n' "$old" > "$h/.claude/settings.json"
  printf 'n\ny\n' | HOME=$h PATH=$stub:$PATH "${tty[@]}" >/dev/null 2>&1
  check_match "...no to the update and yes to turning it on: only Codex's changes" "Claude Code .*: outdated Codex .*: on" \
    "$("${guard[@]}" | grep -iE '^(Claude Code|Codex)' | tr '\n' ' ')"
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
  check_eq "...--level-prefix VALUE spaced: VALUE is not the command (#113)" "lp" \
    "$(ob run -b "$B" -- sh -c 'systemd-cat -t omabox-lp --level-prefix false echo lp; cat ~/omabox-lp.log' 2>&1)"
  check_match "systemd-run --shell: not supported, said (#113)" "--shell is not supported in a box" \
    "$(ob run -b "$B" -- systemd-run --user --shell 2>&1)"
  ob run -b "$B" -- sh -c 'printf "[Desktop Entry]\nType=Application\nName=t2\nExec=foot --app-id=omabox.desk2 sleep 60\nActions=x;\n[Desktop Action x]\nName=x\nExec=true\n" > /tmp/t2.desktop'
  ob run -b "$B" -- uwsm-app -- /tmp/t2.desktop:x >/dev/null
  check "uwsm-app launches a desktop file by path with an action (dropped) (#113)" until_ok 10 \
    bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox.desk2\")'"
  # omarchy-version (finding 137): Omarchy's asks pacman, which a box has no database for (exit 1).
  local ov; ov=$(pacman -Q omarchy-dev 2>/dev/null || pacman -Q omarchy 2>/dev/null) || ov=${OMABOX_OMARCHY_VERSION:-}
  check_eq "omarchy-version says the installed Omarchy's version" "${ov#* }" "$(ob run -b "$B" -- omarchy-version)"
  check_eq "...in a terminal's bash too (Omarchy's bin is on its PATH)" "${ov#* }" "$(ob run -b "$B" -- bash -ic omarchy-version 2>/dev/null)"
  check_eq "...and ls --json has it (finding 139)" "${ov#* }" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy_version')"
  ob down "$B" >/dev/null
}

# #140: a box keeps the config it started with: an update of omabox (share/ is live) reloaded every
# running box. `omabox reload` is the way to a newer one.
t_config_kept() {
  local B=$P-cfg E n0 out want
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  want=$(md5sum < "$ROOT/share/hyprland.lua")
  # shellcheck disable=SC2016 # the box's XDG_RUNTIME_DIR
  local copy=(ob run -b "$B" -- sh -c 'md5sum < "$XDG_RUNTIME_DIR/omabox-hyprland.lua"')
  check_match "its Hyprland runs the box's own copy of the config (#140)" "--config /run/user/[0-9]+/omabox-hyprland\.lua$" \
    "$(ob run -b "$B" -- pgrep -ax Hyprland)"
  check_eq "...a copy of omabox's" "$want" "$("${copy[@]}")"
  ob lua -b "$B" 'omabox_t140 = 1' >/dev/null
  # shellcheck disable=SC2016
  ob run -b "$B" -- sh -c 'echo "-- changed" >> "$XDG_RUNTIME_DIR/omabox-hyprland.lua"'
  sleep 2
  check_eq "a change to that file reloads nothing (autoreload off): the Lua state stays" 1 "$(ob lua -b "$B" 'return omabox_t140')"
  E=$(ob path -b "$B")/home/events.log; n0=$(grep -c ' configreloaded' "$E")
  out=$(ob reload -b "$B" 2>&1)
  check_eq "omabox reload: said" "omabox: box '$B': config reloaded from $ROOT/share/hyprland.lua (binds, rules and Lua state added since are gone)" "$out"
  check_eq "...the copy is omabox's again" "$want" "$("${copy[@]}")"
  check_eq "...the Lua state fresh" nil "$(ob lua -b "$B" 'return omabox_t140')"
  sleep 2
  check_eq "...one reload, not two (the file's change is not one)" 1 "$(($(grep -c ' configreloaded' "$E") - n0))"
  check "...a jailed agent's, as restart-shell is" lib broker_check reload
  check_match "up --autoreload on it: refused, it has autoreload off (finding 207)" "--autoreload \(its autoreload is off\)" \
    "$(ob up "$B" --autoreload 2>&1)"
  ob down "$B" >/dev/null 2>&1
}

# Finding 207: `up --autoreload` keeps Hyprland's autoreload on, as on a desktop, so a project
# can see what a file change does (a helper that rewrites a file the config loads reloads for ever).
t_autoreload() {
  local B=$P-are E n0 n1
  ob up "$B" --no-shell --net isolated --autoreload >/dev/null 2>&1 || { no "up --autoreload" "failed"; return; }
  check_eq "up --autoreload: its Hyprland's autoreload is on (finding 207)" false \
    "$(ob hyprctl -b "$B" -j getoption misc:disable_autoreload | jq .bool)"
  check_eq "...ls --json says so" true "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .autoreload')"
  check_match "...up --autoreload again takes it as it is" "box '$B' is already up$" "$(ob up "$B" --autoreload 2>&1)"
  E=$(ob path -b "$B")/home/events.log
  ob lua -b "$B" 'omabox_t74 = 1' >/dev/null
  n0=$(grep -c ' configreloaded' "$E")
  # shellcheck disable=SC2016
  ob run -b "$B" -- sh -c 'echo "-- changed" >> "$XDG_RUNTIME_DIR/omabox-hyprland.lua"'
  check "a change to a file its config loaded reloads it" until_ok 10 \
    bash -c "[ \"\$(grep -c ' configreloaded' '$E')\" -gt $n0 ]"
  check_eq "...a fresh Lua state" nil "$(ob lua -b "$B" 'return omabox_t74')"
  check_eq "...and autoreload is still on after it" false "$(ob hyprctl -b "$B" -j getoption misc:disable_autoreload | jq .bool)"
  ob reload -b "$B" >/dev/null 2>&1   # the copy is omabox's again (a write: maybe two reloads)
  sleep 2
  n1=$(grep -c ' configreloaded' "$E")
  ob reload -b "$B" >/dev/null 2>&1
  sleep 2
  check_eq "omabox reload with the copy unchanged: one reload, no write to set off another" 1 \
    "$(($(grep -c ' configreloaded' "$E") - n1))"
  ob down "$B" >/dev/null 2>&1
}

# #128: the box's own compositor is not a test's to end by name, and a box ended from inside is said so.
t_own_processes() {
  local B=$P-own out rc
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  check_eq "the box's labwc is named omabox-labwc (#128)" 1 "$(ob run -b "$B" -- pgrep -cx omabox-labwc)"
  check_fails "...so nothing in it is named labwc" ob run -b "$B" -- pgrep -x labwc
  check_eq "a --no-shell box: its shell none in ls --json, - in ls (#147)" "none -" \
    "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .shell_state') $(ob ls | awk -v n="$B" '$1 == n { print $5 }')"
  ob run -b "$B" -- pkill -x labwc >/dev/null 2>&1
  # shellcheck disable=SC2329 # called through holds
  is_up() { [ "$("$CLI" ls --json | jq -r --arg n "$B" '[.[] | select(.name == $n) | .state][0] // "gone"')" = up ]; }
  check "pkill -x labwc in a box leaves it up" holds 2 is_up
  check "...its Hyprland answering" ob hyprctl -b "$B" version
  out=$(ob run -b "$B" -- sh -c 'pkill -x Hyprland; sleep 10' 2>&1); rc=$?
  check_match "a box ended from inside: said in words" "box '$B' went down while this command ran" "$out"
  check "...not exit 0" test "$rc" != 0
  # #166: instead of nsenter's line or bash's "Killed" one for it, not after them.
  check_eq "...in those words alone" 1 "$(grep -c . <<<"$out")"
  ob down "$B" >/dev/null 2>&1
}

# omabox which PID (#172): a box's shell has the desktop's command line on the host; OMABOX_BOX in
# every process of its session and `which` tell them apart.
t_which() {
  local B=$P-which out rc sp="" p ns
  ob up "$B" --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  ns=$(jq -r '.pidns // empty' "$XDG_RUNTIME_DIR/omabox/$B/box.json")
  for p in $(pgrep -x quickshell); do [ "$(readlink "/proc/$p/ns/pid" 2>/dev/null)" != "$ns" ] || sp=$p; done
  [ -n "$sp" ] || { no "the box's shell on the host" "no quickshell in $ns"; ob down "$B" >/dev/null 2>&1; return; }
  check "the box shell's environment names its box" grep -qaFx "OMABOX_BOX=$B" <(tr '\0' '\n' < "/proc/$sp/environ")
  out=$(ob which "$sp" 2>&1); rc=$?
  check_eq "which: the box shell's host pid gives the box, exit 0" "$B 0" "$out $rc"
  check_eq "...what run starts in it carries the name too" "$B" "$(ob run -b "$B" -- sh -c 'echo "$OMABOX_BOX"')"
  # A pid namespace of its own inside the box (Chromium's sandbox, a nested box): its ancestor's.
  ob run -b "$B" -d -q -- unshare -Urpf --mount-proc sleep 61.72 >/dev/null 2>&1
  check "...a process in a pid namespace of its own in the box" until_ok 5 pgrep -fx 'sleep 61.72'
  p=$(pgrep -fx 'sleep 61.72' | head -1)
  check_eq "...is in the box, by its ancestors" "$B" "$(ob which "${p:-0}" 2>/dev/null)"
  check_fails "...not by its own pid namespace" test "$(readlink "/proc/${p:-0}/ns/pid" 2>/dev/null)" = "$ns"
  check_eq "...the host shell is not in it" "not in a box" "$(ob which $$ 2>/dev/null)"
  ob down "$B" >/dev/null 2>&1
}

# `omarchy restart shell` (Omarchy's own, finding 131) in a box: its launcher runs the bar under
# systemd-cat, which a box lacked, and the bar was gone. It and `omabox restart-shell`, in turns, leave
# one shell each time.
t_omarchy_restart() {
  local B=$P-or
  ob up "$B" --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  # shellcheck disable=SC2329 # called through check
  one_shell() { [ "$(ob run -b "$B" -- pgrep -cx quickshell)" = 1 ] && ob run -b "$B" -- omarchy-shell shell ping >/dev/null 2>&1; }
  check_eq "up runs the shell under omarchy-launch-shell, as Omarchy does (#151)" "1 1" \
    "$(ob run -b "$B" -- pgrep -cf '[o]marchy-launch-shell') $(ob run -b "$B" -- pgrep -cx quickshell)"
  check "omarchy-restart-shell succeeds in a box" timeout 30 "$CLI" run -b "$B" -- omarchy-restart-shell
  check "...one shell, answering" until_ok 5 one_shell
  check_eq "...its pid where omabox restart-shell looks" "$(ob run -b "$B" -- pgrep -x quickshell)" "$(ob run -b "$B" -- sh -c 'cat "$XDG_RUNTIME_DIR/omabox-shell.pid"')"
  check "omabox restart-shell after it" timeout 30 "$CLI" restart-shell -b "$B"
  check "...one shell, answering" until_ok 5 one_shell
  # Under one launcher, its own (#151): Omarchy's stopped with the shell it ran, none left to start another.
  check_eq "...under one omarchy-launch-shell" 1 "$(ob run -b "$B" -- pgrep -cf '[o]marchy-launch-shell')"
  check "omarchy-restart-shell again" timeout 30 "$CLI" run -b "$B" -- omarchy-restart-shell
  check "...one shell" until_ok 5 one_shell
  # #141: under the guard (an agent's shell, the box standing in for the host), `omarchy restart shell`
  # stopped the shell over Quickshell's IPC, then could not start it again (its hyprctl is guarded).
  local g=("$CLI" run -b "$B" -- "${GUARDED[@]}") qpid; qpid=$(ob run -b "$B" -- pgrep -x quickshell)
  check_fails "guarded omarchy-restart-shell fails" timeout 30 "${g[@]}" omarchy-restart-shell
  check_eq "...the shell still running, the same one" "$qpid" "$(ob run -b "$B" -- pgrep -x quickshell)"
  check_match "guarded quickshell kill --any-display refused" "omabox guard: not running quickshell kill" \
    "$("${g[@]}" sh -c 'quickshell kill -p "$OMARCHY_PATH/shell" --any-display' 2>&1)"
  check_match "guarded qs list --all still lists it" "$qpid" "$("${g[@]}" qs list --all 2>&1)"
  ob down "$B" >/dev/null
}

# Whether the box's keyboard panel (Omarchy's KeyboardPanel, the widget's) is open and takes keys
# (#116): a closing one stays mapped through its fade-out with no keyboard interactivity, so a layer
# there is not enough: keys sent then were lost, and a keyed row of the widget's flaked under load.
panel_takes_keys() {
  [ "$(ob lua -b "$1" 'for _, l in ipairs(hl.get_layers()) do
    if l.namespace == "omarchy-keyboard-panel" and l.mapped and l.interactivity > 0 then return true end
  end return false')" = true ]
}

# The bar widget (plugin/) in a box's own bar, against a stand-in omabox (a list from a file, actions
# logged) and an xdg-open that blocks like an image viewer left open (finding 64).
t_widget() {
  local B=$P-wg
  # Omarchy's stock bar, not the user's (finding 151): the crops below look for the widget at the top
  # right, and a user's own layout may have the bar on a side, or the widget in a plugin of theirs.
  ob up "$B" --net isolated --stock-bar --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up" "failed"; return; }
  local H; H=$(ob path "$B")/home
  printf '#!/bin/sh\ncase "$1" in\n  ls) echo ls >> "$HOME/polls"; n=$(cat "$HOME/hang" 2>/dev/null || echo 0); if [ "$n" -gt 0 ]; then echo $((n - 1)) > "$HOME/hang"; [ ! -e "$HOME/hang-deaf" ] || trap "" TERM; x=$(sleep 600 | cat); fi; cat "$HOME/list.json" ;;\n  shot) echo "$*" >> "$HOME/actions"; echo "$HOME/x.png" ;;\n  config) echo "$*" >> "$HOME/actions"; cat "$HOME/config.json" 2>/dev/null || echo "{}" ;;\n  up) echo "$*" >> "$HOME/actions"; echo box-9 ;;\n  clip) echo "$*" >> "$HOME/actions"; echo "omabox: handed text (text/plain;charset=utf-8, 3 bytes) from your clipboard to box ia" >&2 ;;\n  *) echo "$*" >> "$HOME/actions" ;;\nesac\n' > "$H/.local/bin/omabox"
  printf '#!/bin/sh\necho "xdg-open $*" >> "$HOME/actions"; exec sleep 300\n' > "$H/.local/bin/xdg-open"
  printf '#!/bin/sh\necho "notify-send $*" >> "$HOME/actions"\n' > "$H/.local/bin/notify-send"
  chmod +x "$H/.local/bin/omabox" "$H/.local/bin/xdg-open" "$H/.local/bin/notify-send"
  local row='{"mode":"headless","size":"1920x1080@60","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  jq -n "[$row + {name: \"a\"}, $row + {name: \"b\"}]" > "$H/list.json"; : > "$H/actions"
  # shellcheck disable=SC2329 # called through until_ok and check_fails
  panel() { panel_takes_keys "$B"; }
  # shellcheck disable=SC2329 # called through until_ok
  lines_over() { [ "$(grep -c -- "$3" "$1" 2>/dev/null)" -gt "$2" ]; }
  # A list poll that started after this call: the stub logs each `ls` before it reads the list, so
  # that poll read the list as it is now (then a moment for the widget to take it in).
  polled() { local n; n=$(grep -c . "$H/polls" 2>/dev/null); until_ok 12 lines_over "$H/polls" "${n:-0}" .; sleep 0.3; }
  # The widget's own settings, in its shell.json entry (read live; finding 169): a poll each second,
  # and 2 s for an `ls --json` to answer (finding 170's checks below).
  local sj=$H/.config/omarchy/shell.json
  local n; n=$(grep -c . "$H/polls" 2>/dev/null)
  jq '.bar.layout[] |= map(if .id == "chaves.omabox" then . + {refreshIntervalSec: 1, listTimeoutSec: 2} else . end)' "$sj" > "$TMP/wg-shell.json" &&
    cat "$TMP/wg-shell.json" > "$sj"
  check "the widget reads its own settings: two polls in 4 s (finding 169; 5 s apart by default)" \
    until_ok 4 lines_over "$H/polls" "$((${n:-0} + 1))" .
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
  # Finding 170: an `ls --json` that never answers is stopped after the widget's limit (2 s, set at
  # the top) with all it started, and counted as one failed poll; the next poll runs. The stub hangs
  # as many polls as $H/hang says, in a $(... | ...) the way `head` on a FIFO did (#95).
  echo 3 > "$H/hang"
  check "an ls --json that hangs is stopped: three in a row are notified so (finding 170)" \
    until_ok 20 grep -qx "notify-send -a omabox omabox: cannot list boxes omabox ls --json did not answer in 2 s" "$H/actions"
  check "...with what it started (no sleep of its left)" until_ok 5 bash -c "! '$CLI' run -b '$B' -- pgrep -fx 'sleep 600'"
  until_ok 5 grep -qx 0 "$H/hang"; polled   # one that answered: the failures count from 0 again
  # One deaf to SIGTERM, its children too, is killed 2 s on; each counts once: two in a row (not
  # four, with a "cannot run" each) notify nothing, and the list the poll after reads is shown.
  jq -n "[$row + {name: \"after\"}]" > "$H/list.json"
  : > "$H/hang-deaf"; echo 2 > "$H/hang"
  until_ok 15 grep -qx 0 "$H/hang"   # the second one started (the first took 2 + 2 s)
  n=$(grep -c . "$H/polls")
  check "...one deaf to SIGTERM is killed, and the poll after it runs" until_ok 12 lines_over "$H/polls" "$n" .
  sleep 0.3; rm "$H/hang-deaf"
  check "...with what it started" until_ok 5 bash -c "! '$CLI' run -b '$B' -- pgrep -fx 'sleep 600'"
  check_eq "...two in a row are two failures, not a notification" 1 "$(grep -c "cannot list boxes" "$H/actions")"
  ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel
  ob keys -b "$B" Down s >/dev/null
  check "...and the list after them is read (shot -b after)" until_ok 3 grep -qx "shot -b after" "$H/actions"
  # (Not with omabox installed on the host: the box sees its /usr/bin/omabox, so there is always one.)
  if [ ! -e /usr/bin/omabox ]; then
    mv "$H/.local/bin/omabox" "$H/.local/bin/omabox.off"
    check "a list command that cannot run is notified" until_ok 20 grep -q "notify-send .*cannot run omabox" "$H/actions"
  else
    skip "a list command that cannot run is notified" "the box has the host's /usr/bin/omabox"
  fi
  ob down "$B" >/dev/null
}

# The widget's box list with 20 made-up boxes (#117 the list held still under the pointer, #121 only
# the rows scroll, #120 the same action slots on every row and a confirm that a double-click does not
# give), read through the panel's inspect IPC: its rows, where each is on screen, their action slots.
t_widget_list() {
  local B=$P-wl
  ob up "$B" --net isolated --stock-bar --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up" "failed"; return; }
  local H; H=$(ob path "$B")/home
  printf '#!/bin/sh\ncase "$1" in\n  ls) echo ls >> "$HOME/polls"; cat "$HOME/list.json" ;;\n  config) if [ "$2" = --json ]; then cat "$HOME/config.json" 2>/dev/null || echo "{}"; else echo "$*" >> "$HOME/actions"; fi ;;\n  *) echo "$*" >> "$HOME/actions" ;;\nesac\n' > "$H/.local/bin/omabox"
  chmod +x "$H/.local/bin/omabox"
  local row='{"mode":"headless","size":"1920x1080@60","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  local irow='{"mode":"interactive","size":"window","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  jq -n "[range(1; 21) as \$i | (if \$i % 3 == 0 then $irow else $row end) + {name: (\"box-\" + (\$i | tostring | if length < 2 then \"0\" + . else . end))}]" > "$H/list.json"
  : > "$H/actions"
  local sj=$H/.config/omarchy/shell.json
  jq '.bar.layout[] |= map(if .id == "chaves.omabox" then . + {refreshIntervalSec: 1} else . end)' "$sj" > "$TMP/wl-shell.json" &&
    cat "$TMP/wl-shell.json" > "$sj"
  # shellcheck disable=SC2329 # called through until_ok, holds and check
  insp() { ob run -b "$B" -- omarchy-shell chaves.omabox.panel inspect; }
  # shellcheck disable=SC2329
  q() { insp | jq -r "$1"; }
  # shellcheck disable=SC2329
  wl_polled() { local n; n=$(grep -c . "$H/polls" 2>/dev/null); until_ok 12 bash -c "[ \$(grep -c . '$H/polls') -gt $((${n:-0} + 1)) ]"; sleep 0.3; }
  # shellcheck disable=SC2329
  open_panel() { ob run -b "$B" -- omarchy-shell chaves.omabox open; until_ok 3 panel_takes_keys "$B"; }
  # shellcheck disable=SC2329
  no_down() { ! grep -q "^down " "$H/actions"; }
  # shellcheck disable=SC2329
  slot() { q ".rows[] | select(.name == \"$1\") | .slots.$2 | \"\(.x) \(.y)\""; }
  wl_polled
  ob pointer -b "$B" -- move 10 1000 >/dev/null
  open_panel
  check_eq "20 rows" 20 "$(q '.rows | length')"
  # #121: only the rows scroll; the New button and the hints stay on screen.
  check "the list scrolls (taller than the room it has; #121)" test "$(q '.list.contentHeight > .list.height')" = true
  check "...the New button and the hints stay on screen" test "$(q '.hintsBottom <= .screen and .newButton > 0')" = true
  check_eq "...the last row is out of view" false "$(q '.rows[-1].visible')"
  local x y; read -r x y < <(q '.rows[4] | "\(.x + 100) \(.y + 10)"')
  ob scroll -b "$B" "$x" "$y" 300 --source wheel >/dev/null
  check "the wheel over the list brings the last row into view" until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r '.rows[-1].visible')\" = true ]"
  # Finding 237: a list scrolled with the wheel stays there once the pointer leaves the card; every poll
  # scrolled back to the selected row. Here the first row, selected by the pointer on it, then scrolled
  # out of view under the pointer.
  ob scroll -b "$B" "$x" "$y" -300 --source wheel >/dev/null
  until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r '.list.contentY | round')\" = 0 ]"
  read -r x y < <(q '.rows[0] | "\(.x + 100 | round) \(.y + 10 | round)"')
  ob pointer -b "$B" -- move "$x" "$y" >/dev/null
  ob scroll -b "$B" "$x" "$y" 300 --source wheel >/dev/null
  until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r '.list.contentY | round')\" != 0 ]"
  sleep 0.5   # (the wheel's scroll ends)
  local cy; cy=$(q '"\(.list.contentY | round) \(.selected as $s | .rows[] | select(.name == $s) | .visible)"')
  ob pointer -b "$B" -- move 10 1000 >/dev/null
  wl_polled; wl_polled
  check_eq "...scrolled away from the selected row, it stays there once the pointer leaves the card (finding 237)" "$cy" \
    "$(q '"\(.list.contentY | round) \(.selected as $s | .rows[] | select(.name == $s) | .visible)"')"
  check_match "...(the selected row was out of view)" " false$" "$cy"
  # #120: the same six slots on every row, in the same place; empty where they do not apply.
  check_eq "a headless and an interactive row have their slots at the same x (#120)" \
    "$(q '.rows[0].slots | [.[] | .x] | @text')" "$(q '.rows[2].slots | [.[] | .x] | @text')"
  check_eq "...a headless row's keys and clipboard slots are empty" "false false false" "$(q '.rows[0].slots | "\(.keys.applies) \(.clipIn.applies) \(.clipOut.applies)"')"
  : > "$H/actions"
  # shellcheck disable=SC2046 # X Y
  ob click -b "$B" $(slot box-20 clipIn) >/dev/null
  check "...a click in an empty slot runs nothing (not the row's peek)" holds 1 bash -c "[ ! -s '$H/actions' ]"
  # shellcheck disable=SC2046
  ob click -b "$B" --double $(slot box-20 down) >/dev/null
  check "a double-click on Down does not confirm it" holds 1 no_down
  # shellcheck disable=SC2046
  ob click -b "$B" $(slot box-20 down) >/dev/null
  check "...a click after it does (down box-20)" until_ok 3 grep -qx "down box-20" "$H/actions"
  check_eq "...once" 1 "$(grep -c "^down " "$H/actions")"
  ob pointer -b "$B" -- move 10 1000 >/dev/null
  open_panel; : > "$H/actions"
  ob keys -b "$B" Down d d >/dev/null
  check "d d with no gap does not confirm" holds 1 no_down
  open_panel
  ob keys -b "$B" Down d >/dev/null; sleep 0.5; ob keys -b "$B" d >/dev/null
  check "d, a pause, d does (one down)" until_ok 3 bash -c "[ \$(grep -c '^down ' '$H/actions') = 1 ]"
  open_panel; : > "$H/actions"
  ob keys -b "$B" n n >/dev/null
  check "n n with no gap starts nothing" holds 1 bash -c "! grep -q '^up ' '$H/actions'"
  # #121: ↑ from the first row selects the last, scrolled into view.
  open_panel
  ob keys -b "$B" Down >/dev/null   # the cursor on the selected row; Up to the first, then once more
  local i ups=(); i=$(q '.selected as $s | [.rows[].name] | index($s)')
  for ((; i >= 0; i--)); do ups+=(Up); done
  ob keys -b "$B" "${ups[@]}" >/dev/null
  check "Up from the first row selects the last, in view (#121)" until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r '\"\(.selected) \(.rows[-1].visible)\"')\" = 'box-20 true' ]"
  # #117: with the pointer on a row, a box that sorts before it comes and one after it goes.
  open_panel
  read -r x y < <(q '.list as $l | .rows[4] | "\(.x + 100) \(.y + 10 + $l.contentY)"')   # where row 5 is unscrolled
  ob scroll -b "$B" "$x" "$y" -300 --source wheel >/dev/null
  until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r .list.contentY)\" = 0 ]"
  read -r x y < <(q '.rows[1] | "\(.x + 100) \(.y + 10)"')
  ob pointer -b "$B" -- move "$x" "$y" >/dev/null
  check_eq "the row under the pointer is selected" box-02 "$(q .selected)"
  local before; before=$(q '[.rows[] | {name, y}] | @json')
  jq '[.[0] + {name: "box-00"}] + [.[] | select(.name != "box-04")]' "$H/list.json" > "$H/l" && mv "$H/l" "$H/list.json"
  wl_polled
  check_eq "held under the pointer, the rows do not move (#117)" "$before" "$(q '[.rows[] | {name, y}] | @json')"
  check_eq "...the selection stays" box-02 "$(q .selected)"
  check_eq "...the new box is counted, not shown" "true 1" "$(q '"\(.held) \(.newCount)"')"
  check_eq "...the gone one stays in its slot, marked" true "$(q '.rows[] | select(.name == "box-04") | .gone')"
  : > "$H/actions"
  read -r x y < <(q '.rows[] | select(.name == "box-04") | "\(.x + 100) \(.y + 10)"')
  ob click -b "$B" "$x" "$y" >/dev/null
  check "...a click on it does nothing" holds 1 bash -c "[ ! -s '$H/actions' ]"
  ob pointer -b "$B" -- move 10 1000 >/dev/null
  check "the pointer off the panel: the list as it is (box-00 in, box-04 out)" until_ok 3 bash -c \
    "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r '\"\(.held) \(.newCount) \([.rows[].name] | index(\"box-00\")) \([.rows[].name] | index(\"box-04\"))\"')\" = 'false 0 0 null' ]"
  jq -n "[$row + {name: \"a\"}, $irow + {name: \"b\"}]" > "$H/list.json"; wl_polled
  check "with two boxes the list is as tall as its rows" test "$(q '.list.height == .list.contentHeight and (.rows | length) == 2')" = true
  # #118: Settings has a GPU picker on a machine with two GPUs: Auto naming its GPU, each GPU (one bound
  # to vfio-pci: unavailable), the fallback said when the chosen one is not there; a pick runs config.
  local gpus='[{"pci":"0000:01:00.0","driver":"vfio-pci","kind":"nvidia","name":"GeForce RTX 5070 Ti","node":null,"available":false,"why":"bound to vfio-pci"},
    {"pci":"0000:7a:00.0","driver":"amdgpu","kind":"amd","name":"Radeon Graphics","node":"/dev/dri/renderD128","available":true}]'
  jq -n --argjson g "$gpus" '{gpu: "0000:01:00.0", gpus: $g, "gpu-auto": "0000:7a:00.0", "gpu-now": "0000:7a:00.0"}' > "$H/config.json"
  ob keys -b "$B" Escape >/dev/null
  ob run -b "$B" -- omarchy-shell chaves.omabox.panel face settings
  until_ok 3 panel_takes_keys "$B"
  check_eq "Settings shows the GPU picker with two GPUs (#118)" \
    '["Auto (now: Radeon Graphics)","GeForce RTX 5070 Ti · unavailable (bound to vfio-pci)","Radeon Graphics · amdgpu"]' \
    "$(until_ok 3 bash -c "'$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -e '.gpu.shown' >/dev/null"; q '.gpu.options | tojson')"
  check_match "...the chosen one is not there: the fallback said" "^Not there now \(bound to vfio-pci\): new boxes render on Radeon Graphics" "$(q .gpu.line)"
  : > "$H/actions"
  # shellcheck disable=SC2046 # X Y
  ob click -b "$B" $(q '.gpu.at | "\(.x) \(.y)"') >/dev/null
  ob pointer -b "$B" -- move 10 1000 >/dev/null
  ob keys -b "$B" Down Return >/dev/null   # from the chosen one to the next: Radeon
  check "...a pick runs omabox config gpu SLOT" until_ok 3 grep -qx "config gpu 0000:7a:00.0" "$H/actions"
  # Finding 237: with gpu nvidia (a kind) the amber line gives the reason too, the vfio-bound card's kind
  # being NVIDIA's; a row's caption names its GPU as `ls` does: with one render node (the other GPU on
  # vfio-pci), only a fallback's, with its slot.
  local rr='{"node":"/dev/dri/renderD128","pci":"0000:7a:00.0","driver":"amdgpu"}'
  jq -n "[$row + {name: \"a\", render: ($rr + {fallback: true, wanted: \"nvidia\"})}, $row + {name: \"c\", render: ($rr + {fallback: false})}]" > "$H/list.json"
  jq -n --argjson g "$gpus" '{gpu: "nvidia", gpus: $g, "gpu-auto": "0000:7a:00.0", "gpu-now": "0000:7a:00.0"}' > "$H/config.json"
  ob keys -b "$B" Escape Escape >/dev/null
  ob run -b "$B" -- omarchy-shell chaves.omabox.panel face settings
  until_ok 3 bash -c "'$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -e '.gpu.value == \"nvidia\"' >/dev/null"
  check_match "...gpu nvidia, its card on vfio-pci: the reason said too (finding 237)" "^Not there now \(bound to vfio-pci\): new boxes render on Radeon" "$(q .gpu.line)"
  ob keys -b "$B" Escape >/dev/null; wl_polled
  check_eq "a row names its GPU as ls does: with one render node a fallback's only, its slot too (finding 237)" "true false" \
    "$(q '[.rows[] | {(.name): .caption}] | add | "\(.a | test("^headless · amdgpu 7a:00.0 \\(fallback\\) · ")) \(.c | test("amdgpu"))"')"
  jq -n --argjson g "[$(jq -c '.[1]' <<<"$gpus")]" '{gpu: "auto", gpus: $g, "gpu-auto": "0000:7a:00.0", "gpu-now": "0000:7a:00.0"}' > "$H/config.json"
  ob keys -b "$B" Escape Escape >/dev/null
  ob run -b "$B" -- omarchy-shell chaves.omabox.panel face settings
  check "...not with one GPU and gpu auto" until_ok 3 bash -c "[ \"\$('$CLI' run -b '$B' -- omarchy-shell chaves.omabox.panel inspect | jq -r .gpu.shown)\" = false ]"
  check_eq "the widget logged no QML errors" "" "$(ob log -b "$B" shell -n all --grep "Panel.qml|TypeError|ReferenceError" | grep -v IpcHandler)"
  ob down "$B" >/dev/null
}

# Monitors beyond the first in a headless box (#122): up --monitor, monitor add/remove/list, kept over a
# reload, the shell's bar on each, the pointer and clicks across them, a shot of one. On a GPU other
# than NVIDIA's (headless outputs), and on NVIDIA's (Wayland outputs, #165) in t_monitors_nvidia.
# gpu_node other|nvidia: the first render node not on the nvidia driver, or the first on it.
gpu_node() {
  local n d
  for n in $(lib render_nodes); do
    d=$(lib render_driver "$n")
    if [ "$1" = nvidia ]; then [ "$d" != nvidia ] || { echo "$n"; return; }
    else [ "$d" = nvidia ] || { echo "$n"; return; }; fi
  done
}
# nvidia_ready: NVIDIA's render node, for the tests of what a box on it does; else why not, exit 1.
nvidia_ready() {
  local n; n=$(gpu_node nvidia)
  [ -n "$n" ] || { echo "no render node on the nvidia driver here"; return 1; }
  ! aq_unfixed "$CLI" || { echo "a headless box on NVIDIA needs aquamarine's fix (omabox setup --aquamarine)"; return 1; }
  echo "$n"
}
t_monitors() {
  local n; n=$(gpu_node other)
  [ -n "$n" ] || { skip "monitors" "no render node here but NVIDIA's (t_monitors_nvidia runs there)"; return; }
  monitors_on "$n" "$P-mon" HEADLESS-2 HEADLESS-3 HEADLESS-4
}
# The same on NVIDIA (#165, finding 234): Wayland outputs, windows on the box's labwc, WAYLAND-N.
t_monitors_nvidia() {
  local n; n=$(nvidia_ready) || { skip "monitors on NVIDIA" "$n"; return; }
  monitors_on "$n" "$P-monnv" WAYLAND-1 WAYLAND-2 WAYLAND-3
}
# monitors_on NODE BOX MAIN SECOND THIRD: a box on NODE with two more monitors, by their names there.
monitors_on() {
  local n=$1 B=$2 m0=$3 m1=$4 m2=$5
  check "up --monitor 1080x1920 --monitor 2560x1440,scale=1.6,below" env OMABOX_RENDER_NODE="$n" "$CLI" up "$B" --stock-bar \
    --monitor 1080x1920 --monitor 2560x1440,scale=1.6,below
  local ml; ml=$(ob monitor -b "$B" list --json)
  check_eq "three monitors: modes, scales, positions" \
    "$m0 1920x1080@60 1 0 0|$m1 1080x1920@60 1 1920 0|$m2 2560x1440@60 1.6 1920 1920" \
    "$(jq -r 'map("\(.name) \(.mode) \(.scale) \(.x) \(.y)") | join("|")' <<<"$ml")"
  check_eq "ls --json has them" 3 "$("$CLI" ls --json | jq --arg b "$B" '.[] | select(.name == $b) | .monitors | length')"
  check_match "ls says 3 monitors" "^$B +headless +3 monitors " "$("$CLI" ls | grep "^$B ")"
  check "the shell has a bar on each" until_ok 10 bash -c "[ \"\$('$CLI' hyprctl -b '$B' -j layers | jq -r '[to_entries[] | select([.value.levels[][] | .namespace] | index(\"omarchy-bar\")) | .key] | sort | join(\" \")')\" = '$m0 $m1 $m2' ]"
  check_eq "-b NAME before monitor (#161)" "$ml" "$(ob -b "$B" monitor list --json 2>&1)"
  # A point in the gap below the main screen, left of the others (#161): grim alone failed, and beside
  # a point on a monitor it read black.
  check_match "pixel in a gap between monitors: refused, saying so" "100,1500 is not on any monitor \(.*$m1 1920,0 1080x1920" "$(ob pixel -b "$B" 100 1500 2>&1)"
  check_match "...with a point on a monitor too" "100,1500 is not on any monitor" "$(ob pixel -b "$B" 10 10 100 1500 2>&1)"
  check_match "...a point on the third: its colour" "^#[0-9a-f]{6}$" "$(ob pixel -b "$B" 2000 2000 2>&1)"
  ob pointer -b "$B" -- move 2500 2400 >/dev/null
  check_eq "the pointer onto the third: its workspace is the active one" "$m2" "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .monitor)"
  ob click -b "$B" 2400 900 >/dev/null
  check_eq "a click on the second lands there" "2400, 900" "$(ob hyprctl -b "$B" cursorpos)"
  check_eq "...and its workspace is active" "$m1" "$(ob hyprctl -b "$B" -j activeworkspace | jq -r .monitor)"
  ob shot -b "$B" --monitor "$m1" -o "$TMP/mon3.png" >/dev/null 2>&1
  check_match "shot --monitor $m1: that monitor, 1080x1920" "1080 x 1920" "$(file "$TMP/mon3.png")"
  check_match "...said, for --in" "screen's 1080x1920 at 1920,0|^omabox: .*1920,0" "$(ob shot -b "$B" --monitor "$m1" -o "$TMP/mon3b.png" 2>&1 >/dev/null)"
  check_match "shot --monitor of one it does not have: refused" "has no monitor HEADLESS-9" "$(ob shot -b "$B" --monitor HEADLESS-9 2>&1)"
  ob hyprctl -b "$B" reload >/dev/null
  sleep 1
  check_eq "a config reload keeps them" "$(jq -c 'map({name, mode, scale, x, y})' <<<"$ml")" "$(ob monitor -b "$B" list --json | jq -c 'map({name, mode, scale, x, y})')"
  # mode (#161): the ones placed right of (and below) it move with it, out of the larger main screen.
  local out; out=$(ob mode -b "$B" 2560x1440 2>&1 >/dev/null)
  check_eq "mode 2560x1440: the monitor right of it moves, and the one below that" "$m0 0 0|$m1 2560 0|$m2 2560 1920" \
    "$(ob monitor -b "$B" list --json | jq -r 'map("\(.name) \(.x) \(.y)") | join("|")')"
  check_match "...said" "moved the monitors placed next to it: $m1 to 2560,0, $m2 to 2560,1920" "$out"
  ob mode -b "$B" 1920x1080 >/dev/null 2>&1
  check_eq "...and back with mode 1920x1080, every mode as it was" "$(jq -c 'map({name, mode, scale, x, y})' <<<"$ml")" "$(ob monitor -b "$B" list --json | jq -c 'map({name, mode, scale, x, y})')"
  local ev0; ev0=$(ob log -b "$B" events -n all | grep -c monitorremoved)
  check "monitor remove $m1" ob monitor -b "$B" remove "$m1"
  check_eq "...gone from the list; the one placed below it goes below the main screen now" "$m0 0 0|$m2 0 1080" \
    "$(ob monitor -b "$B" list --json | jq -r 'map("\(.name) \(.x) \(.y)") | join("|")')"
  check "...an unplug the box's events saw" until_ok 3 bash -c "[ \$('$CLI' log -b '$B' events -n all | grep -c 'monitorremoved>>$m1') -gt $ev0 ]"
  check_eq "monitor add again: the free name" "$m1" "$(ob monitor -b "$B" add 800x600,0,3000 2>/dev/null)"
  check_eq "...at X,Y" "800x600@60 0 3000" "$(ob monitor -b "$B" list --json | jq -r --arg n "$m1" '.[] | select(.name == $n) | "\(.mode) \(.x) \(.y)"')"
  check_eq "...the others' modes as they were" "$m0 1920x1080@60|$m2 2560x1440@60" \
    "$(ob monitor -b "$B" list --json | jq -r --arg n "$m1" 'map(select(.name != $n) | "\(.name) \(.mode)") | join("|")')"
  check_match "output drop with other monitors: the main screen off, the others on (#161)" \
    "has its main screen \($m0\) off now, ($m2, $m1|$m1, $m2) still on" "$(ob output -b "$B" drop 2>&1)"
  ob output -b "$B" back >/dev/null 2>&1
  check_match "the main screen is not removed" "is the box's main screen" "$(ob monitor -b "$B" remove "$m0" 2>&1)"
  check_match "a monitor it does not have" "has no monitor HEADLESS-9" "$(ob monitor -b "$B" remove HEADLESS-9 2>&1)"
  if [ "$m0" = WAYLAND-1 ]; then
    check_match "NVIDIA: monitor add --name refused, saying why" "WAYLAND-N as its screen is: no --name" "$(ob monitor -b "$B" add 800x600 --name X 2>&1)"
    check_match "...labwc's config has each monitor's window size" "title=\"aquamarine - $m2\"><action name=\"ResizeTo\" width=\"2560\" height=\"1440\"/>" \
      "$(cat "$(ob path "$B")/run/labwc/rc.xml")"
  fi
  check_match "up again without them: already up, said" "is already up, without what you asked for: --monitor" "$(OMABOX_RENDER_NODE=$n ob up "$B" --monitor 800x600 2>&1)"
  ob down "$B" >/dev/null
}

# wait and --wait on a box with more monitors (#162, finding 231): the screen watched is all of them,
# in layout coordinates. One at scale 1.6 holds a cell that never stops changing, a terminal to type
# into is on the third; the same again after the main screen is dropped and back (it is advertised
# last then: the old tool watched the first output advertised, and named its pixels as the screen's).
t_monitors_wait() {
  local n; n=$(gpu_node other)
  [ -n "$n" ] || { skip "monitors wait" "no render node here but NVIDIA's (t_monitors_wait_nvidia runs there)"; return; }
  monitors_wait_on "$n" "$P-monw"
}
# The same on NVIDIA (#165, finding 234): its monitors are windows on the box's labwc.
t_monitors_wait_nvidia() {
  local n; n=$(nvidia_ready) || { skip "monitors wait on NVIDIA" "$n"; return; }
  monitors_wait_on "$n" "$P-monwnv"
}
monitors_wait_on() {
  local n=$1 B=$2 out rc g w when
  env OMABOX_RENDER_NODE="$n" "$CLI" up "$B" --no-shell --net isolated --monitor 1280x720,scale=1.6 \
    --monitor 1080x1920,below >/dev/null 2>&1 || { no "up with two more monitors" "failed"; return; }
  ob pointer -b "$B" -- move 2300 200 >/dev/null
  ob run -b "$B" -d -q -- foot -T N sh -c 'i=0; while :; do printf "\r%s" $((i++ % 10)); sleep 0.05; done'
  ob wait -b "$B" window 'title:^N$' >/dev/null
  w=$(ob windows -b "$B" --json | jq -r '.[] | select(.title == "N") | "\(.at[0]) \(.at[1]) \(.at[0] + .size[0]) \(.at[1] + .size[1])"')
  for when in "" " (after output drop and back)"; do
    out=$(ob wait -b "$B" still --timeout 2s); rc=$?
    check_eq "a cell changing on the second monitor: wait still 124$when" 124 "$rc"
    g=$(sed -n 's/.*--ignore "\([^"]*\)".*/\1/p' <<<"$out")
    check "...named as an --ignore in the layout, a cell inside its window ($g in $w)$when" cell_in "$g" "$w"
    check_eq "...--ignore it: still$when" 0 "$(ob wait -b "$B" still --ignore "$g" >/dev/null; echo $?)"
    check_eq "...--window on it: 124, and still with --ignore$when" "124 0" \
      "$(ob wait -b "$B" still --window 'title:^N$' --timeout 1500ms >/dev/null; echo $?) $(ob wait -b "$B" still --window 'title:^N$' --ignore "$g" >/dev/null; echo $?)"
    out=$(ob wait -b "$B" change -g "1920,0 800x450" --json)
    check_eq "...wait change -g on that monitor: changed there$when" "satisfied $g" \
      "$(jq -r '"\(.result) \(.change.x),\(.change.y) \(.change.w)x\(.change.h)"' <<<"$out")"
    check_eq "...-g on the main screen only: still$when" 0 "$(ob wait -b "$B" still -g "0,0 1920x1080" >/dev/null; echo $?)"
    check_eq "...-g across the main screen and that one: 124$when" 124 "$(ob wait -b "$B" still -g "1800,0 300x300" --timeout 1s >/dev/null; echo $?)"
    if [ -z "$when" ]; then
      ob pointer -b "$B" -- move 2400 1500 >/dev/null
      check_eq "run -d --wait of a window on the third monitor: settled" 0 \
        "$(ob run -b "$B" -d --wait --ignore "$g" -- foot -T T sh -c 'cat > /tmp/typed' >/dev/null 2>&1; echo $?)"
    fi
    out=$(ob keys -b "$B" --window 'title:^T$' --wait --ignore "$g" -t x); rc=$?
    check_eq "keys --wait into a window on the third monitor: settled$when" 0 "$rc"
    check_match "...on a change there (from 1920,450)$when" "last change .* at (19[2-9][0-9]|2[0-9]{3}),(4[5-9][0-9]|[5-9][0-9]{2}|[0-9]{4}) " "$out"
    check_eq "...with -g on that monitor: settled$when" 0 \
      "$(ob keys -b "$B" --window 'title:^T$' --wait -g "1920,450 1080x1920" -t y >/dev/null; echo $?)"
    [ -n "$when" ] || ob output -b "$B" drop --for 500ms >/dev/null
  done
  ob keys -b "$B" --window 'title:^T$' Return >/dev/null
  check "what was typed reached it" until_ok 5 bash -c "[ \"\$('$CLI' run -b '$B' -- cat /tmp/typed)\" = xyxy ]"
  check_match "wait --window --ignore outside the window: said, screen coordinates" "outside the region watched: it takes the screen's coordinates" \
    "$(ob wait -b "$B" still --window 'title:^N$' --ignore "10,10 8x11" --timeout 500ms 2>&1 >/dev/null)"
  ob down "$B" >/dev/null
}
# cell_in "X,Y WxH" "X0 Y0 X1 Y1": the rectangle is inside, and the size of a terminal cell.
cell_in() {
  [[ $1 =~ ^([0-9]+),([0-9]+)\ ([0-9]+)x([0-9]+)$ ]] || return 1
  local x=${BASH_REMATCH[1]} y=${BASH_REMATCH[2]} cw=${BASH_REMATCH[3]} ch=${BASH_REMATCH[4]} x0 y0 x1 y1
  read -r x0 y0 x1 y1 <<<"$2"
  [ "$x" -ge "$x0" ] && [ "$y" -ge "$y0" ] && [ $((x + cw)) -le "$x1" ] && [ $((y + ch)) -le "$y1" ] && [ "$cw" -le 20 ] && [ "$ch" -le 30 ]
}

# An interactive box's monitors (#123), each a window on the "desktop": all in a box standing in for
# the host (finding 26), with a terminal focused on its workspace 1 and the box on 9.
t_monitors_window() {
  # The stand-in at the size of the screen #174 was found on (3440x1440, scale 1).
  local S=$P-monh
  ob up "$S" --size 3440x1440 --no-shell --net isolated >/dev/null 2>&1 || { no "up the stand-in" "failed"; return; }
  local in=("$CLI" run -b "$S" -- "${GUARDED[@]}" "$CLI")
  if aq_unfixed "${in[@]}"; then skip "monitors as windows" "the stand-in's aquamarine lacks the fix"; ob down "$S" >/dev/null; return; fi
  ob run -b "$S" -d -- foot >/dev/null 2>&1
  ob wait -b "$S" window foot >/dev/null 2>&1
  local out
  out=$("${in[@]}" up wm --interactive --no-shell --monitor 1080x1920 --monitor 1280x720,below 2>&1)
  check_match "up --interactive --monitor 1080x1920 --monitor 1280x720,below in the stand-in (#174's)" "box 'wm' up" "$out"
  check_match "...the windows: the layout at 53% of its size, said" "its layout at 53% of its size" "$out"
  # shellcheck disable=SC2329 # called through until_ok
  mons() { [ "$("${in[@]}" monitor -b wm list --json | jq -r "$1")" = "$2" ]; }
  # shellcheck disable=SC2329 # called through check
  boxwins() { ob hyprctl -b "$S" -j clients | jq -c '[.[] | select(.class == "aquamarine") | {ws: .workspace.name, floating, at, size}] | sort_by(.at)'; }
  # How many pairs of the box's windows on the stand-in overlap.
  # shellcheck disable=SC2329 # called through check
  overlaps() { ob hyprctl -b "$S" -j clients | jq '[.[] | select(.class == "aquamarine")] as $w | [range(0; $w | length) as $i | range($i + 1; $w | length) as $j
    | select($w[$i].at[0] < $w[$j].at[0] + $w[$j].size[0] and $w[$j].at[0] < $w[$i].at[0] + $w[$i].size[0]
      and $w[$i].at[1] < $w[$j].at[1] + $w[$j].size[1] and $w[$j].at[1] < $w[$i].at[1] + $w[$i].size[1])] | length'; }
  # Each monitor's mode, scale, its size in the layout (the mode over the scale) and place.
  # shellcheck disable=SC2329 # called through check_eq
  layout() { "${in[@]}" monitor -b wm list --json | jq -r 'map(. as $m | ($m.mode | split("@")[0] | split("x") | map(tonumber)) as $p
    | "\($m.name) \($m.mode) \($m.scale) \($p[0] / $m.scale | round)x\($p[1] / $m.scale | round) \($m.x),\($m.y)") | join("|")'; }
  # The box's layout scaled as a whole (#174): 1920x1080 at 0,0, 1080x1920 right of it, 1280x720
  # below that, at 63/120 (each window whole pixels), with the stand-in's gap (10 + border 2) between
  # windows that touch in the box, the picture centred in the free area.
  check_eq "its windows: on workspace 9, floating, the box's layout scaled to 0.525 and centred" \
    '[{"ws":"9","floating":true,"at":[874,21],"size":[1008,567]},{"ws":"9","floating":true,"at":[1894,21],"size":[567,1008]},{"ws":"9","floating":true,"at":[1894,1041],"size":[672,378]}]' "$(boxwins)"
  check_eq "...none over another" 0 "$(overlaps)"
  check_eq "...the stand-in's focus and workspace unchanged" "foot 1" "$(ob hyprctl -b "$S" -j activewindow | jq -r .class) $(ob hyprctl -b "$S" -j activeworkspace | jq -r .id)"
  check_eq "...each monitor a view: its window's pixels at 0.525, so the size asked for, placed as asked" \
    "WAYLAND-1 1008x567@60 0.525 1920x1080 0,0|WAYLAND-2 567x1008@60 0.525 1080x1920 1920,0|WAYLAND-3 672x378@60 0.525 1280x720 1920,1920" "$(layout)"
  check_eq "...and the host rule is off again" false "$(ob lua -b "$S" 'omabox_monitor_rule:is_enabled()')"
  # The stand-in's pointer in each window, at its middle (on workspace 9 now, where the windows are):
  # the box's pointer in the middle of that monitor. aquamarine reports a point of the window; Hyprland
  # 0.56 placed it over the whole layout, so every middle was (1600, 1079); omabox's build places it.
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '9' })" >/dev/null
  if ob --version 2>/dev/null | grep -q '^aquamarine: a private build, .*+layout'; then
    local at="" shown="" x y
    # Each window, then back to the first two: a window entered again is where the cursor went (a
    # first visit gets a cursor set a frame later, which showed it again).
    for p in "1378 304" "2177 525" "2230 1230" "2177 525" "1378 304"; do
      # shellcheck disable=SC2086 # X Y
      ob pointer -b "$S" -- move $p >/dev/null 2>&1
      sleep 0.4
      at+="$("${in[@]}" hyprctl -b wm cursorpos 2>/dev/null)|"
      # The stand-in's cursor there (its headless screen draws it into its frames): more than the
      # box's plain background in a crop at the pointer.
      read -r x y <<<"$p"
      ob shot -b "$S" -g "$((x - 4)),$((y - 4)) 24x24" -o "$TMP/monh-cursor.png" >/dev/null 2>&1
      shown+="$(magick "$TMP/monh-cursor.png" -unique-colors -format %w info: 2>/dev/null || echo 0) "
    done
    # Each within a pixel of the middle (the window's pixel rounds).
    check_eq "the pointer in a monitor's window: on that monitor, where it is in the window (finding 238; $at)" ok \
      "$(awk -F'|' '{ split("960 540 2460 960 2560 2280", w, " "); bad = 0
        for (i = 1; i <= 3; i++) { split($i, p, ", "); if ((p[1] - w[2 * i - 1]) ^ 2 > 1 || (p[2] - w[2 * i]) ^ 2 > 1) bad = 1 }
        print bad ? "off" : "ok" }' <<<"$at")"
    # Crossing into another monitor's window hid the cursor: aquamarine hid it for the window the
    # pointer had left with a set_cursor the stand-in took for the one it entered (#175, finding 239).
    if ob --version 2>/dev/null | grep -q '^aquamarine: a private build, .*+cursor'; then
      check_eq "...and the cursor shown in each window, crossing from one to another and back (finding 239; colours at it: $shown)" ok \
        "$(awk '{ for (i = 1; i <= 5; i++) if ($i < 2) { print "hidden"; exit } print "ok" }' <<<"$shown")"
    else
      skip "the cursor shown after crossing monitor windows" "not omabox's aquamarine build with +cursor (omabox setup --aquamarine)"
    fi
  else
    skip "the pointer in a monitor's window lands on that monitor" "not omabox's aquamarine build (omabox setup --aquamarine)"
  fi
  ob hyprctl -b "$S" dispatch "hl.dsp.focus({ workspace = '1' })" >/dev/null
  out=$("${in[@]}" monitor -b wm add 800x600,0,1080 2>&1)
  check_match "monitor add 800x600,0,1080: a view too" "a monitor WAYLAND-4 420x315@60 0x1080 0.525" "$out"
  check_eq "...the picture laid out again with it, below the main window as in the box" \
    '[{"ws":"9","floating":true,"at":[874,15],"size":[1008,567]},{"ws":"9","floating":true,"at":[874,594],"size":[420,315]},{"ws":"9","floating":true,"at":[1894,15],"size":[567,1008]},{"ws":"9","floating":true,"at":[1894,1047],"size":[672,378]}]' "$(boxwins)"
  check_eq "...none over another" 0 "$(overlaps)"
  "${in[@]}" up wm2 --interactive --no-shell >/dev/null 2>&1
  check_eq "a box started after: its window is not taken for a monitor (tiled)" 1 \
    "$(ob hyprctl -b "$S" -j clients | jq '[.[] | select(.class == "aquamarine" and .floating == false)] | length')"
  "${in[@]}" down wm2 >/dev/null 2>&1
  out=$("${in[@]}" monitor -b wm remove WAYLAND-3 2>&1)
  check_match "monitor remove WAYLAND-3: the picture laid out again without it" "its layout at 73% of its size" "$out"
  check_eq "...bigger now" '[{"ws":"9","floating":true,"at":[626,24],"size":[1392,783]},{"ws":"9","floating":true,"at":[626,819],"size":[580,435]},{"ws":"9","floating":true,"at":[2030,24],"size":[783,1392]}]' "$(boxwins)"
  check "...the views at 0.725, the sizes as asked" until_ok 3 mons 'map("\(.name) \(.mode) \(.scale)") | join("|")' \
    "WAYLAND-1 1392x783@60 0.725|WAYLAND-2 783x1392@60 0.725|WAYLAND-4 580x435@60 0.725"
  check_eq "...the stand-in's focus and workspace unchanged" "foot 1" "$(ob hyprctl -b "$S" -j activewindow | jq -r .class) $(ob hyprctl -b "$S" -j activeworkspace | jq -r .id)"
  # A window the user resizes: its view's scale follows, the monitor keeps its size and place; a
  # window of another shape shows the monitor smaller along the side that does not fit.
  local a; a=$(ob hyprctl -b "$S" -j clients | jq -r '.[] | select(.class == "aquamarine" and .size == [783, 1392]) | .address')
  ob hyprctl -b "$S" dispatch "hl.dsp.window.resize({ window = 'address:$a', x = 810, y = 1440 })" >/dev/null
  check "resizing a monitor's window to 810x1440: its view at 0.75, the monitor still 1080x1920 at 1920,0" until_ok 3 mons \
    '.[] | select(.name == "WAYLAND-2") | "\(.mode) \(.scale) \(.x),\(.y)"' "810x1440@60 0.75 1920,0"
  ob hyprctl -b "$S" dispatch "hl.dsp.window.resize({ window = 'address:$a', x = 700, y = 700 })" >/dev/null
  check "...to 700x700: at 0.64815, the monitor 1080x1080" until_ok 3 mons '.[] | select(.name == "WAYLAND-2") | "\(.mode) \(.scale)"' "700x700@60 0.64815"
  check_eq "...its size in the layout" "1080x1080" "$(layout | tr '|' '\n' | awk '$1 == "WAYLAND-2" { print $4 }')"
  "${in[@]}" hyprctl -b wm reload >/dev/null 2>&1; sleep 1
  check_eq "a config reload keeps its monitors and views" "WAYLAND-1 0.725 WAYLAND-2 0.64815 WAYLAND-4 0.725" \
    "$("${in[@]}" monitor -b wm list --json | jq -r 'map("\(.name) \(.scale)") | join(" ")')"
  ob hyprctl -b "$S" dispatch "hl.dsp.window.close({ window = 'address:$a' })" >/dev/null
  check "closing a monitor's window: that monitor goes" until_ok 3 mons 'map(.name) | join(" ")' "WAYLAND-1 WAYLAND-4"
  check_match "...the box stays up" " up " "$("${in[@]}" ls | grep '^wm ')"
  "${in[@]}" monitor -b wm remove WAYLAND-4 >/dev/null 2>&1
  check_eq "removing the last monitor: the main window tiles again, filling the workspace" '[{"ws":"9","floating":false,"at":[12,12],"size":[3416,1416]}]' "$(boxwins)"
  check "...its screen its window's size again, at scale 1" until_ok 3 mons 'map("\(.name) \(.mode) \(.scale)") | join("|")' "WAYLAND-1 3416x1416@60 1"
  "${in[@]}" monitor -b wm add 800x600 >/dev/null 2>&1
  local main; main=$(ob hyprctl -b "$S" -j clients | jq -r '.[] | select(.class == "aquamarine" and .floating and .size[0] > 1000) | .address')
  ob hyprctl -b "$S" dispatch "hl.dsp.window.close({ window = 'address:$main' })" >/dev/null
  check "closing its first window with another open: it stays up on that one" until_ok 3 mons 'map(.name) | join(" ")' "WAYLAND-2"
  check_match "...up" " up " "$("${in[@]}" ls | grep '^wm ')"
  ob down "$S" >/dev/null
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
  # #152: Codex's config is read with the system's python, not PATH's (Omarchy's mise puts its shims
  # first, and one can fail: an untrusted config once the XDG dirs change).
  local ph=$TMP/gh-py; mkdir -p "$ph/.codex" "$TMP/badpy"
  printf '#!/bin/sh\necho "mise ERROR stub" >&2\nexit 1\n' > "$TMP/badpy/python3"; chmod +x "$TMP/badpy/python3"
  check_match "Codex's state with a failing python3 first on PATH (#152)" "^Codex \(.*\): off$" \
    "$(HOME=$ph PATH="$TMP/badpy:$PATH" "$CLI" guard 2>&1 | grep -i '^codex')"
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
  CLAUDE_ENV_FILE=$f sh -c "$hook" >/dev/null
  check_eq "...a second session start: the guard's dir once on PATH (#113)" 1 \
    "$(bash -c '. "$1"; tr : "\n" <<<"$PATH" | grep -cxF "$2"' _ "$f" "$co/share/guard")"
  # #142: the note says `!` commands are guarded, and gets through whole (it sits in the hooks'
  # single-quoted echo, so a ' in it would cut it short or break the hook).
  check_match "...whole, saying the user's ! commands are guarded (#142)" \
    "! commands the user runs are under this guard too.*omabox host -- CMD$" "$(sh -c "$chook")"
  check_match "...and so does Claude Code's" "! commands the user runs are under this guard too.*omabox host -- CMD$" \
    "$(CLAUDE_ENV_FILE=$f sh -c "$hook")"
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
  # #141: Quickshell's IPC needs no display, so under the guard `kill` and `ipc` reached the desktop's
  # shell. The guard's quickshell and qs refuse them; the rest goes on to the next quickshell on PATH,
  # here a stub (so a refusal that failed would reach it, never the real one).
  printf '#!/bin/sh\necho "real quickshell reached: $*"\n' > "$TMP/fakeopen/quickshell"; chmod +x "$TMP/fakeopen/quickshell"
  local qs; qs=$("${gx[@]}" sh -c 'command -v quickshell; command -v qs' | xargs)
  check_eq "guard exec: quickshell and qs are the guard's" "$ROOT/share/guard/quickshell $ROOT/share/guard/qs" "$qs"
  if [ "$qs" = "$ROOT/share/guard/quickshell $ROOT/share/guard/qs" ]; then
    check_match "...quickshell kill --any-display refused" "omabox guard: not running quickshell kill here" \
      "$("${gx[@]}" quickshell kill -p /usr/share/omarchy/shell --any-display 2>&1)"
    check_eq "...with exit 4" 4 "$("${gx[@]}" quickshell kill --any-display >/dev/null 2>&1; echo $?)"
    check_match "...qs ipc call refused, after an option with a value" "not running quickshell ipc here" \
      "$("${gx[@]}" qs -p /usr/share/omarchy/shell ipc --any-display call shell ping 2>&1)"
    check_match "...qs msg (ipc call's old name) refused" "not running quickshell msg here" "$("${gx[@]}" qs msg -i abc x y 2>&1)"
    check_eq "...list goes on, its arguments whole" "real quickshell reached: list --all --json" "$("${gx[@]}" qs list --all --json 2>&1)"
    check_eq "...so does --version" "real quickshell reached: --version" "$("${gx[@]}" quickshell --version 2>&1)"
    check_eq "...and a config whose path is a subcommand's name" "real quickshell reached: -p /tmp/kill log" "$("${gx[@]}" qs -p /tmp/kill log 2>&1)"
    # #166: quickshell's parser bundles short options (`quickshell -np PATH list` runs), and the guard
    # read -np as a flag and PATH as the subcommand, so kill and ipc after it reached the real one.
    local f a
    for f in -np -dp -pn -vnp -npX --path=/x; do
      a=("$f"); case $f in -npX|--path=*) ;; *) a+=(/usr/share/omarchy/shell) ;; esac
      check_match "...qs ${a[*]} kill refused (bundled options)" "not running quickshell kill here" "$("${gx[@]}" qs "${a[@]}" kill --any-display 2>&1)"
      check_match "...qs ${a[*]} ipc refused" "not running quickshell ipc here" "$("${gx[@]}" qs "${a[@]}" ipc --any-display call shell ping 2>&1)"
      check_eq "...qs ${a[*]} list goes on, whole" "real quickshell reached: ${a[*]} list --all" "$("${gx[@]}" qs "${a[@]}" list --all 2>&1)"
    done
    check_eq "...exit 4 for a bundled kill" 4 "$("${gx[@]}" quickshell -dp /x kill >/dev/null 2>&1; echo $?)"
    check_eq "...a value that is a subcommand's name, bundled, is a value" "real quickshell reached: -np kill list" \
      "$("${gx[@]}" qs -np kill list 2>&1)"
    check_match "...an unknown word before kill does not hide it" "not running quickshell kill here" "$("${gx[@]}" qs --unknown-opt value kill 2>&1)"
  else
    no "...which refuse kill and ipc" "not run: quickshell under guard exec is [$qs], not the guard's"
  fi
  # The session omabox finds, not $HYPRLAND_INSTANCE_SIGNATURE: under the guard (an agent running the
  # suite) that is the guard's.
  local sig want; sig=$(bash -c 'source "$1"; host_session; echo "$HOST_SIG"' _ "$TMP/lib/bin/omabox")
  want=$(hyprctl -j instances | jq -r --arg s "$sig" '.[] | select(.instance == $s) | .wl_socket')
  check_eq "host from a guarded shell: the real Wayland display" "$want" "$("${GUARDED[@]}" "$CLI" host -- sh -c 'echo $WAYLAND_DISPLAY' 2>/dev/null)"
  check "host: and hyprctl reaches it (read-only)" "${GUARDED[@]}" "$CLI" host -- hyprctl -j version
  local open; open=$("${GUARDED[@]}" "$CLI" host -- sh -c 'command -v xdg-open; echo "${BROWSER-unset}"' 2>/dev/null)
  check_match "host: the real xdg-open (finding 92)" '^/' "$(head -1 <<<"$open")"
  check_fails "...not the guard's" grep -q share/guard <<<"$open"
  check_fails "host: the real quickshell and qs, not the guard's (#141)" grep -q share/guard \
    <<<"$("${GUARDED[@]}" "$CLI" host -- sh -c 'command -v quickshell; command -v qs' 2>/dev/null)"
  check_fails "...nor another checkout's" grep -q elsewhere <<<"$("${GUARDED[@]}" PATH="/elsewhere/share/guard:$PATH" BROWSER=/elsewhere/share/guard/xdg-open "$CLI" host -- sh -c 'echo "$PATH ${BROWSER-}"' 2>/dev/null)"
  check_eq "...nor one written with a trailing slash (up and run leave it out too)" "/usr/bin:/bin" \
    "$(PATH=/x/share/guard/:/usr/bin:/y/share/guard:/bin lib caller_path)"
  # A host's `omarchy dev link` puts its checkout's bin first (finding 149): kept for host, not in a box.
  local dl=$TMP/devlink; mkdir -p "$dl/bin" "$dl/default/hypr"; : > "$dl/default/hypr/envs.lua"
  check_eq "a box's PATH leaves out an Omarchy checkout's bin (a dev link's)" "/usr/share/omarchy/bin:/usr/bin" \
    "$(PATH="$dl/bin:/usr/share/omarchy/bin:/usr/bin" lib caller_path box)"
  check_eq "...host keeps it" "$dl/bin/:/usr/bin" "$(PATH="$dl/bin/:/usr/bin" lib caller_path)"
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
  # The record shortens only long arguments (#150): an expanded PATH, a long word.
  local hl long; long=$(printf '/opt/long/install/dir/bin:%.0s' {1..30})
  hl=$("${GUARDED[@]}" "$CLI" host -- env "PATH=$long$PATH" true 2>&1)
  check_match "host: a long NAME=value is cut, env, PATH= and true still there" "on your real desktop: env PATH=/opt/long/install/dir/bin:/opt/long/inst…\(\+[0-9]+ chars\) true$" "$hl"
  check_match "...one line under 200 characters" "^1 ([0-9]|[0-9][0-9]|1[0-9][0-9])$" "$(wc -l <<<"$hl") ${#hl}"
  check_eq "...a short command exactly as it is" "omabox: on your real desktop: printf %s A=b x" "$("${GUARDED[@]}" "$CLI" host -- printf %s A=b x 2>&1 >/dev/null)"
  check_eq "...a long word cut too" "true $(printf 'a%.0s' {1..40})…(+90 chars) x" "$(lib host_record true "$(printf 'a%.0s' {1..130})" x)"
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
  check_box_safety inner "${in[@]}" "$CLI"
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
    # #111: --no-confirm-close wins over the setting
    "${in[@]}" "$CLI" config confirm-close on >/dev/null
    "${in[@]}" "$CLI" up nc --interactive --no-shell --no-confirm-close >/dev/null 2>&1
    "${in[@]}" "$CLI" config confirm-close off >/dev/null
    "${cl[@]}" >/dev/null
    check "up --no-confirm-close with the setting on: one close ends it" until_ok 8 gone nc
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
  "${in[@]}" pkill -x omabox-peek >/dev/null 2>&1
  check "peek --workspace 4 (#111)" "${in[@]}" "$CLI" peek inner2 --workspace 4
  check "...its window on 4" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\") | select(.workspace.name == \"4\")'"
  "${in[@]}" "$CLI" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# `omabox clip` (issue #23, finding 119) with a box standing in for the host (finding 26) and interactive
# boxes nested in it: the clipboard is the stand-in's, never the user's. No agent runs in the box, so
# what clip refuses there is what it refuses, not the suite's own caller.
t_clip() {
  local B=$P-cl
  ob up "$B" --net isolated --stock-bar --plugin "$ROOT/plugin" >/dev/null 2>&1 || { no "up (the stand-in host)" "failed"; return; }
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
  panel() { panel_takes_keys "$B"; }
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
  # A newer passthrough over an older one with no reload between (#113): the reaper installs it again;
  # its unbind takes the toggle out of both submaps.
  ob lua -b "$B" 'omabox_pass_version = 3' >/dev/null
  check "a newer passthrough over an older one: installed again" until_ok 6 reinstalled
  check_eq "...still one toggle bind in each submap" "omabox:1 :1" "$(ob hyprctl -b "$B" -j binds |
    jq -r '[.[] | select(.description | startswith("omabox:"))] | group_by(.submap) | map("\(.[0].submap):\(length)") | reverse | join(" ")')"
  focus class:foot; focus "pid:$pa"
  check "...focus drives it" until_ok 5 kis "omabox|aquamarine|border|ka"
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

# up --theme-dir (#170, finding 240): a theme under development goes in live, not as a copy made at
# `up`. One in the repo up runs from (in the box at its own path already), one elsewhere (mounted
# read-only at its own path); the box HOME's themes/ links to each, and omarchy-theme-set applies one.
t_theme_dir() {
  local B=$P-td r o=$TMP/$P-td-out/b D
  r=$(tmp_repo td-repo); mkdir -p "$r/themes/td-a" "$o"
  cp "$HOME/.local/state/omarchy/current/theme/colors.toml" "$r/themes/td-a/colors.toml"
  echo one > "$o/marker"
  (cd "$r" && ob up "$B" --no-shell --theme-dir themes/td-a --theme-dir "$o") >/dev/null 2>&1 || { no "up --theme-dir" "failed"; return; }
  D=$(ob path -b "$B")
  check_eq "box.json has the theme dirs, resolved" "[\"$r/themes/td-a\",\"$o\"]" "$(jq -c .theme_dirs "$D/box.json")"
  check_eq "the box HOME's themes/ links to the one in the repo" "$r/themes/td-a" "$(ob run -b "$B" -- readlink /home/sbx/.config/omarchy/themes/td-a)"
  check_eq "...and to the one elsewhere" "$o" "$(ob run -b "$B" -- readlink /home/sbx/.config/omarchy/themes/b)"
  check_eq "...which is in the box at its own path" one "$(ob run -b "$B" -- cat "$o/marker")"
  echo two > "$o/marker"; echo '# edited after up' >> "$r/themes/td-a/colors.toml"
  check_eq "an edit after up reaches the box: live" two "$(ob run -b "$B" -- cat /home/sbx/.config/omarchy/themes/b/marker)"
  check_eq "...in the repo's too" "# edited after up" "$(ob run -b "$B" -- tail -1 /home/sbx/.config/omarchy/themes/td-a/colors.toml)"
  check_fails "...read-only there" ob run -b "$B" -- sh -c "echo x > '$o/marker'"
  check "omarchy-theme-set applies the linked theme" ob run -b "$B" -- sh -c 'omarchy-theme-set td-a >/dev/null 2>&1'
  check_eq "...as it is now" "td-a # edited after up" \
    "$(ob run -b "$B" -- sh -c 'echo "$(cat ~/.local/state/omarchy/current/theme.name) $(tail -1 ~/.local/state/omarchy/current/theme/colors.toml)"')"
  check_match "up again with other theme dirs: refused, saying which it has" "already up, without what you asked for: --theme-dir \(it has $r/themes/td-a, $o\)" \
    "$(cd "$r" && ob up "$B" --theme-dir themes/td-a 2>&1)"
  check "...with the same ones: fine" bash -c "cd '$r' && '$CLI' up '$B' --theme-dir themes/td-a --theme-dir '$o'"
  ob down "$B" >/dev/null 2>&1
}

# up --theme NAME (finding 245): the box on that theme, not the desk's current one (a theme the desk is
# not on: tokyo-night, or nord on a desk on tokyo-night); over a save's look; refused when there is no
# such theme, before anything is made; a box whose omarchy-theme-set fails ends, saying why.
t_theme() {
  local B=$P-th B2=$P-th2 B3=$P-th3 S=$P-ths t=tokyo-night o=nord desk D H bg c T=$TMP/$P-th-tree
  desk=$(head -n 1 "$HOME/.local/state/omarchy/current/theme.name" 2>/dev/null) || desk=""
  [ "$desk" != "$t" ] || { t=nord; o=tokyo-night; }
  local cur=/home/sbx/.local/state/omarchy/current
  check_match "an unknown theme: refused" "--theme: no theme $P-nope in " "$(ob up "$B" --theme "$P-nope" 2>&1)"
  check_fails "...before the box dir is made" test -e "$XDG_RUNTIME_DIR/omabox/$B"
  ob up "$B" --net isolated --stock-bar --theme "$t" >/dev/null 2>&1 || { no "up --theme $t" "failed"; return; }
  check_box_safety "$B"
  D=$(ob path -b "$B"); H=$D/home
  check_eq "box.json has the theme" "$t" "$(jq -r .theme "$D/box.json")"
  check_eq "...the box's theme.name is it, not the desk's ($desk)" "$t" "$(cat "$H/.local/state/omarchy/current/theme.name")"
  check "...its colors.toml the theme's" cmp "$H/.local/state/omarchy/current/theme/colors.toml" "/usr/share/omarchy/themes/$t/colors.toml"
  check "...its templates made (hyprland.lua)" test -s "$H/.local/state/omarchy/current/theme/hyprland.lua"
  c=$(cd "/usr/share/omarchy/themes/$t/backgrounds" && find . -maxdepth 1 -type f | sort | head -n 1)
  check_eq "...its background the theme's first" "$cur/theme/backgrounds/${c#./}" "$(readlink "$H/.local/state/omarchy/current/background")"
  bg=$(sed -n 's/^background *= *"\(#[0-9a-fA-F]\{6\}\)".*/\1/p' "/usr/share/omarchy/themes/$t/colors.toml" | tr 'A-F' 'a-f')
  check "...the bar in the theme's background ($bg)" until_ok 15 bash -c "[ \"\$('$CLI' pixel -b '$B' 400 12)\" = '$bg' ]"
  check_eq "...ls --json says so" "$t" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .theme')"
  check_match "up again with another theme: refused, saying which it has" "already up, without what you asked for: --theme $o \(it started on $t\)" \
    "$(ob up "$B" --theme "$o" 2>&1)"
  check "...with the same one: fine" ob up "$B" --theme "${t^}"
  # --from SAVE: the flag wins over the save's look.
  check "a save of it" sv save "$S" -b "$B"
  ob down "$B" >/dev/null 2>&1
  sv up "$B2" --from "$S" --net isolated --no-shell --theme "$o" >/dev/null 2>&1 || { no "up --from with --theme" "failed"; sv saves rm "$S" >/dev/null 2>&1; return; }
  H=$(ob path -b "$B2")/home
  check_eq "up --from a save on $t with --theme $o: on $o" "$o" "$(cat "$H/.local/state/omarchy/current/theme.name")"
  check "...its colors.toml $o's" cmp "$H/.local/state/omarchy/current/theme/colors.toml" "/usr/share/omarchy/themes/$o/colors.toml"
  c=$(cd "/usr/share/omarchy/themes/$o/backgrounds" && find . -maxdepth 1 -type f | sort | head -n 1)
  check_eq "...its background $o's first" "$cur/theme/backgrounds/${c#./}" "$(readlink "$H/.local/state/omarchy/current/background")"
  ob down "$B2" >/dev/null 2>&1
  # Without the flag nothing changes: the desk's theme, from a save too.
  sv up "$B2" --from "$S" --net isolated --no-shell >/dev/null 2>&1 || { no "up --from without --theme" "failed"; sv saves rm "$S" >/dev/null 2>&1; return; }
  D=$(ob path -b "$B2")
  check_eq "up --from without --theme: the desk's theme, as before" "$desk" "$(cat "$D/home/.local/state/omarchy/current/theme.name")"
  check "...its colors.toml the desk's" cmp "$D/home/.local/state/omarchy/current/theme/colors.toml" "$HOME/.local/state/omarchy/current/theme/colors.toml"
  check_eq "...no theme in box.json" null "$(jq .theme "$D/box.json")"
  ob down "$B2" >/dev/null 2>&1
  sv saves rm "$S" >/dev/null 2>&1
  # An omarchy-theme-set that fails in the box ends it, saying why.
  copy_omarchy "$T"
  rm -f "$T/bin/omarchy-theme-set"; printf '#!/bin/sh\necho "broken on purpose" >&2\nexit 3\n' > "$T/bin/omarchy-theme-set"; chmod +x "$T/bin/omarchy-theme-set"
  check_match "a failing omarchy-theme-set: up fails, saying why" "died while starting \(up --theme: omarchy-theme-set $t failed: broken on purpose" \
    "$(ob up "$B3" --net isolated --no-shell --omarchy "$T" --theme "$t" 2>&1)"
  ob down "$B3" >/dev/null 2>&1
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
# copy_omarchy DIR: the installed Omarchy as a tree of files of our own (themes linked: the bulk).
copy_omarchy() {
  local T=$1 f
  mkdir -p "$T"
  for f in /usr/share/omarchy/*; do
    case ${f##*/} in themes) ln -s "$f" "$T/themes" ;; *) cp -a "$f" "$T/" ;; esac
  done
  chmod -R u+w "$T"
}

t_omarchy_tree() {
  local B=$P-ot T=$TMP/omarchy-tree
  copy_omarchy "$T"
  # (The package's bin/ links to /usr/bin: a file of its own in the link's place.)
  rm -f "$T/bin/omarchy-version"; printf '#!/bin/sh\necho omabox-tree\n' > "$T/bin/omarchy-version"; chmod +x "$T/bin/omarchy-version"
  printf '\nomabox_tree_marker = "tree"\n' >> "$T/default/hypr/bootstrap.lua"
  check_match "--omarchy refuses a folder that is not an Omarchy tree" "not an Omarchy tree \(no bin\)" "$(ob up "$B" --omarchy "$TMP" 2>&1)"
  ob up "$B" --net isolated --omarchy "$T" >/dev/null 2>&1 || { no "up --omarchy" "failed"; return; }
  check_box_safety "$B"
  check_eq "the box's Hyprland loaded the tree's config" tree "$(ob lua -b "$B" 'return omabox_tree_marker')"
  check_eq "...run has it as OMARCHY_PATH" "$T" "$(ob run -b "$B" -- sh -c 'echo "$OMARCHY_PATH"')"
  check_eq "...and its bin first" omabox-tree "$(ob run -b "$B" -- omarchy-version)"
  check_match "...the bar runs from it" "quickshell -n -p $T/shell" "$(ob run -b "$B" -- pgrep -a quickshell)"
  check_eq "...a terminal's bash too (the box's /etc/omarchy.conf)" "$T omabox-tree" "$(ob run -b "$B" -- bash -ic 'echo "$OMARCHY_PATH $(omarchy-version)"' 2>/dev/null)"
  check_eq "ls --json names it" "$T" "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy')"
  check_eq "...and its version as dev (no git: no commit)" dev "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy_version')"
  # Committed, then edited, after up: restart-shell records it as it is now.
  local ov; ov() { ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .omarchy_version'; }
  git -C "$T" init -q && git -C "$T" add -A && git -C "$T" -c user.name=t -c user.email=t@t commit -qm t
  ob restart-shell -b "$B" >/dev/null 2>&1
  check_eq "...its commit after restart-shell" "dev $(git -C "$T" rev-parse --short HEAD)" "$(ov)"
  echo '-- edited' >> "$T/default/hypr/bootstrap.lua"; ob restart-shell -b "$B" >/dev/null 2>&1
  check_eq "...+dirty once edited" "dev $(git -C "$T" rev-parse --short HEAD)+dirty" "$(ov)"
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

# The shell's lock screen in a box (finding 186): the system menu's Lock takes a real session lock,
# keys go to the lock and not the window under it, a wrong password keeps it, the right one gives the
# desktop back with that window focused. A box cannot read /etc/shadow, so the tree's lock asks a
# PAM config of the test's (pam_exec, a test password) instead of /etc/pam.d's.
t_lock() {
  local B=$P-lk T=$TMP/lock-tree K=$TMP/lockpam addr svc
  copy_omarchy "$T"
  svc=$T/shell/plugins/lock/Service.qml
  if ! grep -q '^    config: "omarchy-lock-password"$' "$svc"; then
    no "the lock's PamContext is where the test expects it" "no 'config: \"omarchy-lock-password\"' line in $svc"; return
  fi
  sed -i 's|^    config: "omarchy-lock-password"$|&\n    configDirectory: "/opt/omabox-lockpam"|' "$svc"
  mkdir -p "$K"
  printf '%s' omabox-lock-pw > "$K/password"
  printf '#!/bin/bash\nIFS= read -r -d "" pw || true\n[ "$pw" = "$(cat /opt/omabox-lockpam/password)" ]\n' > "$K/check"
  chmod +x "$K/check"
  printf 'auth required pam_exec.so expose_authtok quiet /opt/omabox-lockpam/check\naccount required pam_permit.so\n' \
    > "$K/omarchy-lock-password"
  ob up "$B" --net isolated --omarchy "$T" --ro-bind "$K:/opt/omabox-lockpam" >/dev/null 2>&1 || { no "up with the lock's test PAM" "failed"; return; }
  check_box_safety "$B"
  # A terminal whose input goes to a file: anything typed into it while locked would land there.
  ob run -b "$B" -d -q --wait -- foot sh -c 'cat > "$HOME/typed"' >/dev/null 2>&1
  addr=$(ob hyprctl -b "$B" -j activewindow | jq -r .address)
  ob keys -b "$B" super+escape >/dev/null
  check "SUPER+ESCAPE opens the system menu" ob wait -b "$B" --timeout 5s layer omarchy-menu
  ob keys -b "$B" -t Lock Return >/dev/null
  check "its Lock takes a session lock" ob wait -b "$B" --timeout 5s cmd -- omarchy-hyprland-session-locked
  ob wait -b "$B" --timeout 5s still --quiet 300ms >/dev/null
  OMABOX_T_PW=wrong-pw ob keys -b "$B" --pass OMABOX_T_PW Return >/dev/null
  ob wait -b "$B" --timeout 5s still --quiet 300ms >/dev/null
  check "a wrong password keeps it locked" ob run -b "$B" -- omarchy-hyprland-session-locked
  check_match "...the lock asked the test's PAM config" 'with config "omarchy-lock-password" in dir "/opt/omabox-lockpam"' "$(ob log -b "$B" shell -n all)"
  ob wait -b "$B" --timeout 10s still --quiet 1s >/dev/null
  OMABOX_T_PW=omabox-lock-pw ob keys -b "$B" --pass OMABOX_T_PW Return >/dev/null
  check "the right password unlocks it" ob wait -b "$B" --timeout 10s cmd -- sh -c '! omarchy-hyprland-session-locked'
  check_eq "...the window from before is focused again" "$addr" "$(ob hyprctl -b "$B" -j activewindow | jq -r .address)"
  check_eq "...and got none of the keys typed while locked" "" "$(ob run -b "$B" -- cat /home/sbx/typed 2>&1)"
  ob keys -b "$B" -t unlocked Return >/dev/null
  check "...though it gets them now (the check above can see a leak)" ob wait -b "$B" --timeout 5s cmd -- grep -qx unlocked /home/sbx/typed
  ob down "$B" >/dev/null
}

# Window selectors and what covers what (finding 81), on hyprctl JSON made up for it: floats above
# tiled windows whatever their order, later above earlier otherwise, fullscreen and a shown special
# workspace above the rest; off-screen workspaces, inactive group tabs and unmapped windows.
t_unit_window_select() {
  local c m m_sp act='{"address":"0xb"}'
  # windows' table (#105, finding 174): a title's quotes escaped once, as in --json; an empty class
  # keeps its column (the title starts at the 99th character, after the six padded ones).
  local rows; rows=$(lib windows_table <<<'[
    {"address":"0xa","pid":7,"workspace":1,"onscreen":true,"cover":[],"active":true,"size":[10,20],"at":[1,2],"class":"foot","title":"say \"hi\" \\ x"},
    {"address":"0xb","pid":8,"workspace":2,"onscreen":false,"cover":[],"active":false,"size":[3,4],"at":[5,6],"class":"","title":"no class"}]')
  check_eq "windows: a quoted title escaped once" '"say \"hi\" \\ x"' "$(sed -n 1p <<<"$rows" | cut -c99-)"
  check_eq "...an empty class keeps its column" '"no class"' "$(sed -n 2p <<<"$rows" | cut -c99-)"
  check_match "...the rest in theirs" '^0xb +8 +2 +off-screen +3x4 at 5,6 +"no class"$' "$(sed -n 2p <<<"$rows")"
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

# pixel and shot --zoom (#133, finding 209): what is refused before any box is asked.
# Multi-monitor edges (#161) that need no box.
t_unit_monitors() {
  # Every command whose usage takes -b NAME takes it before the command too: monitor was left out.
  local bcmds missing="" c n=0 out d=$TMP/maw
  bcmds=$(lib eval 'echo "$BOX_CMDS"')
  for c in $(ob help 2>&1 | sed -nE 's/^  omabox ([a-z-]+) .*-b NAME.*/\1/p' | sort -u); do
    n=$((n + 1)); [[ $bcmds == *" $c "* ]] || missing+=" $c"
  done
  check_eq "every command whose usage takes -b NAME is one -b NAME before it reaches ($n of them)" "" "$missing"
  check "...the usage has them" test "$n" -gt 20
  # An interactive box's layout as a picture on the host (#174): one scale for all, whole pixels when
  # a scale near gives them, the gap between windows that touch in the box, centred; never enlarged.
  pic() { printf '%s\n' "$@" | lib desk_picture | paste -sd'|'; }
  check_eq "picture: #174's layout on a 3440x1440 screen, at 63/120" "0.525 1|main 862 9 1008 567|WAYLAND-2 1882 9 567 1008|WAYLAND-3 1882 1029 672 378" \
    "$(pic '3416 1416 12' 'main 0 0 1920 1080' 'WAYLAND-2 1920 0 1080 1920' 'WAYLAND-3 1920 1920 1280 720')"
  check_eq "picture: its first two" "0.73333 1|main 602 4 1408 792|WAYLAND-2 2022 4 792 1408" \
    "$(pic '3416 1416 12' 'main 0 0 1920 1080' 'WAYLAND-2 1920 0 1080 1920')"
  check_eq "picture: no scale near gives 1366x768 whole pixels: rounded" "0.56667 1|main 11 222 1088 612|WAYLAND-2 1111 222 774 435" \
    "$(pic '1896 1056 12' 'main 0 0 1920 1080' 'WAYLAND-2 1920 0 1366 768')"
  check_eq "picture: an X,Y one below the main window, a tall one right of it (60/120: whole pixels for 800x600)" "0.5 1|main 380 24 960 540|WAYLAND-3 380 576 400 300|WAYLAND-2 1352 24 540 960" \
    "$(pic '2272 1008 12' 'main 0 0 1920 1080' 'WAYLAND-3 0 1080 800 600' 'WAYLAND-2 1920 0 1080 1920')"
  check_eq "picture: a lone monitor smaller than the area is not enlarged" "1 1|main 1308 408 800 600" "$(pic '3416 1416 12' 'main 0 0 800 600')"
  check_eq "picture: too big even at Hyprland's smallest scale (1/4): laid out at it, said" "0.25 0|main 0 15 480 270|WAYLAND-2 492 15 480 270|WAYLAND-3 984 15 480 270" \
    "$(pic '500 300 12' 'main 0 0 1920 1080' 'WAYLAND-2 1920 0 1920 1080' 'WAYLAND-3 3840 0 1920 1080')"
  # An interactive monitor that fails in `up` (a second --monitor): up's EXIT trap, which takes the box
  # down, still runs; monitor_add_window's own trap (the host rule off) took its place in up's shell.
  mkdir -p "$d"; echo '{}' > "$d/box.json"
  out=$(bash -c 'source "$1"; D=$2 NAME=t
    meta() { echo 9; }; ws_check() { echo "$1"; }; box_pid() { echo $$; }; hypr_json() { echo "[]"; }
    host_hyprctl() { case $* in "-j monitors") echo "[{\"id\": 0, \"focused\": true, \"x\": 0, \"y\": 0, \"width\": 1920, \"height\": 1080, \"scale\": 1}]" ;;
      -j\ *) echo "[]" ;; *) echo ok ;; esac; }
    host_eval() { die "host hyprctl eval failed: a stub"; }
    on_box() { case $* in *omabox_monitor_layout*) echo "error: omabox-layout:main 0 0 1920 1080,WAYLAND-2 1920 0 800 600" ;; *) echo ok ;; esac; }
    box_file() { :; }
    trap "echo up-trap" EXIT
    monitor_add_window 800x600; echo "went on"' _ "$TMP/lib/bin/omabox" "$d" 2>&1)
  check_match "a monitor add failing in up: up's own EXIT trap runs (it takes the box down)" "a stub.*up-trap" "$(tr '\n' ' ' <<<"$out")"
  check_fails "...and up stops there" grep -q "went on" <<<"$out"
}

t_unit_pixel() {
  check_match "pixel: X Y needed" "need X Y" "$(ob pixel -b "$P-x" 5 2>&1)"
  check_match "pixel: a bad coordinate" "bad argument" "$(ob pixel -b "$P-x" 1 x 2>&1)"
  check_match "pixel: --in or --window" "not both" "$(ob pixel -b "$P-x" --in a.png -w x 1 1 2>&1)"
  check_match "pixel: at most 32 points" "at most 32" "$(ob pixel -b "$P-x" $(seq 66) 2>&1)"
  check "pixel is a jailed agent's" lib broker_check pixel
  check_match "shot --zoom needs -g" "goes with -g" "$(ob shot -b "$P-x" --zoom 4 2>&1)"
  check_match "shot --zoom 2-16" "takes 2-16" "$(ob shot -b "$P-x" -g "0,0 9x9" --zoom 17 2>&1)"
  check_match "shot --zoom or --fit" "not both" "$(ob shot -b "$P-x" -g "0,0 9x9" --zoom 4 --fit 100 2>&1)"
  check_match "shot --zoom past 2000 px refused" "is 2400x80: at most 2000 px" "$(ob shot -b "$P-x" -g "0,0 300x10" --zoom 8 2>&1)"
  # shot --burst (#132, finding 212)
  check_match "shot --every without --burst refused" "go with --burst" "$(ob shot -b "$P-x" --every 1s 2>&1)"
  check_match "shot --burst 1 refused" "takes 2-200" "$(ob shot -b "$P-x" --burst 1 2>&1)"
  check_match "shot --burst with --zoom refused" "--burst or --zoom" "$(ob shot -b "$P-x" --burst 3 --zoom 2 -g "0,0 9x9" 2>&1)"
  check_match "shot --after: an omabox command that acts" "got ls" "$(ob shot -b "$P-x" --burst 3 --after -- ls 2>&1)"
  check_match "...scroll and monitor are (#167)" "no box '$P-x' is up.*"$'\n'"omabox: no box '$P-x' is up" "$(ob shot -b "$P-x" --burst 3 --after -- scroll 1 1 15 2>&1; ob shot -b "$P-x" --burst 3 --after -- monitor add 800x600 2>&1)"
  check_match "...on the burst's box" "no -b" "$(ob shot -b "$P-x" --burst 3 --after -- keys -b other a 2>&1)"
  check_match "shot --after needs --" "--after -- ACTION" "$(ob shot -b "$P-x" --burst 3 --after keys a 2>&1)"
  # shot --changed (#145, finding 243): what it refuses before any box is asked.
  local a
  for a in '-g 0,0,9,9' '--fit 100' '--zoom 4' '--burst 3' '--monitor HEADLESS-2'; do
    # shellcheck disable=SC2086 # the option and its value
    check_match "shot --changed with $a refused" "crops by itself" "$(ob shot -b "$P-x" --changed $a 2>&1)"
  done
  check_match "...--since is --changed" "crops by itself" "$(ob shot -b "$P-x" --since x.png -g 0,0,9,9 2>&1)"
  check_match "shot --ignore without --changed refused" "goes with --changed" "$(ob shot -b "$P-x" --ignore "0,0 9x9" 2>&1)"
  check_match "shot --ignore takes a region" 'is "X,Y WxH"' "$(ob shot -b "$P-x" --changed --ignore 9 2>&1)"
  check_match "shot --since from a jail refused (a path of the jail's)" "not for a jailed agent" "$(lib relay_shot x -- --since a.png 2>&1)"
  # diff_box: the box around what differs, a corner too (--burst --diff read a change there as none,
  # or as the box of what did not change); none when the same; a mask leaves a change out.
  local d=$TMP/diffbox; mkdir -p "$d"
  magick -size 100x80 xc:'#102030' "$d/a.png"
  magick "$d/a.png" -fill '#102031' -draw 'point 0,0' "$d/corner.png"
  magick "$d/a.png" -fill white -draw 'rectangle 0,0 99,40' "$d/half.png"
  magick "$d/a.png" -fill white -draw 'rectangle 10,10 19,19' -draw 'rectangle 80,60 89,69' "$d/two.png"
  check_eq "diff_box: the same image: nothing" "" "$(lib diff_box "$d/a.png" "$d/a.png")"
  check_eq "...one corner pixel, one level of blue" "0 0 1 1" "$(lib diff_box "$d/a.png" "$d/corner.png")"
  check_eq "...the top half" "0 0 100 41" "$(lib diff_box "$d/a.png" "$d/half.png")"
  check_eq "...two squares apart: one box around both" "10 10 80 60" "$(lib diff_box "$d/a.png" "$d/two.png")"
  check_eq "...one masked: the other" "10 10 10 10" "$(lib diff_box "$d/a.png" "$d/two.png" 75,55,20,20)"
  check_eq "diff_parts: each square, to 8 px outwards" $'8 8 16 16\n80 56 16 16' "$(lib diff_parts "$d/a.png" "$d/two.png" | sort -n)"
}

# shot --burst (#132, finding 212) in a box: frames at their times, a contact sheet, an action after the
# first frame and what it changed, and click --in a frame.
t_burst() {
  local B=$P-burst o=$TMP/burst out err rc
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  ob run -b "$B" -d -q --wait -- foot -T A cat >/dev/null 2>&1
  out=$(ob shot -b "$B" --burst 5 --every 100ms -g "0,0 200x200" -o "$o/a" 2>"$TMP/burst.err"); rc=$?; err=$(cat "$TMP/burst.err")
  check_eq "shot --burst 5 --every 100ms: exit 0, a line a frame" "0 5" "$rc $(grep -c "^$o/a/frame-00[1-5]\.png [0-9.]*s$" <<<"$out")"
  check_eq "...five 200x200 PNGs" 5 "$(file "$o"/a/frame-*.png | grep -c '200 x 200,')"
  check "...about 100 ms apart, in order" awk '{ t = $2 + 0; if (NR > 1 && (t - p < 0.08 || t - p > 0.5)) bad = 1; p = t } END { exit bad || NR != 5 }' <<<"$out"
  check_match "...said, with --in" "5 frames of 200x200 \(screen 0,0\) over .*click with --in a frame" "$err"
  out=$(ob shot -b "$B" --burst 12 --diff --sheet -g "0,0 600x200" -o "$o/b" --after -- keys -t xyz 2>/dev/null); rc=$?
  check_eq "--after -- keys, --diff, --sheet: exit 0" 0 "$rc"
  check_match "...the first frame before the keys, a later one changed" "frame-001\.png 0\.00s"$'\n'".*changed [0-9]+,[0-9]+ [0-9]+x[0-9]+" "$out"
  check_match "...one contact sheet, last" "^$o/b/sheet\.png$" "$(tail -n 1 <<<"$out")"
  check_match "...a PNG" "PNG image data" "$(file "$o/b/sheet.png")"
  ob click -b "$B" --in "$o/a/frame-003.png" 10 20 >/dev/null 2>&1
  check_eq "click --in a frame" "10, 20" "$(ob hyprctl -b "$B" cursorpos)"
  # #167, finding 236. --diff in the screen's coordinates: a crop at 60,30, halved by --fit, and the
  # pointer moved into it after the first frame (screen shots show it): its box holds 400,90 (its
  # tip; a few pixels' slack for the rounding), not the frame's 170,30.
  out=$(ob shot -b "$B" --burst 6 --every 100ms --diff -g "60,30 400x80" --fit 200 -o "$o/d" --after -- pointer -- move 400 90 2>/dev/null)
  check "--diff with -g and --fit: the change in screen coordinates, inside the crop" awk '
    $3 == "changed" { split($4, p, ","); split($5, s, "x"); n++
      if (p[1] < 60 || p[2] < 30 || p[1] + s[1] > 460 || p[2] + s[2] > 110) bad = 1
      if (p[1] > 403 || p[1] + s[1] < 397 || p[2] > 93 || p[2] + s[2] < 87) bad = 1 }
    END { exit bad || n < 1 }' <<<"$out"
  # --after -- scroll (and monitor) are actions; scroll was refused.
  ob shot -b "$B" --burst 3 -g "0,0 200x200" -o "$o/s" --after -- scroll 500 95 15 >/dev/null 2>&1; rc=$?
  check_eq "--after -- scroll: taken, the pointer there" "0 500, 95" "$rc $(ob hyprctl -b "$B" cursorpos)"
  # A folder used again: the 12 frames and sheet of $o/b go, a file of the caller's stays.
  echo mine > "$o/b/notes.txt"
  ob shot -b "$B" --burst 2 -g "0,0 200x200" -o "$o/b" >/dev/null 2>&1
  check_eq "-o a folder used before: only this burst's frames, the caller's file kept" "frame-001.png frame-002.png notes.txt" \
    "$(cd "$o/b" && echo *)"
  # Two bursts at once with no -o: a folder each (the default was the second's name).
  mkdir -p "$o/tmp"
  TMPDIR=$o/tmp ob shot -b "$B" --burst 2 -g "0,0 200x200" >/dev/null 2>"$TMP/burst1.err" & local b1=$!
  TMPDIR=$o/tmp ob shot -b "$B" --burst 2 -g "0,0 200x200" >/dev/null 2>"$TMP/burst2.err"
  wait "$b1"
  check_eq "two bursts started together: two folders of 2 frames" "2 2 2" \
    "$(find "$o/tmp" -mindepth 1 -maxdepth 1 -type d | wc -l) $(for d in "$o"/tmp/*/; do find "$d" -name 'frame-*.png' | wc -l; done | xargs)"
  ob down "$B" >/dev/null 2>&1
}

# shot --changed (#145, finding 243) in a box: the first one whole, then nothing changed (no image,
# nothing on stdout, exit 0), typed text cropped and `click --in` mapping the crop, the pointer's old
# place on the screen left out, --since, a resized window shot whole, a blinking block cursor left out
# by --ignore, a blinking beam caret alone not a change.
t_changed() {
  local B=$P-chg o=$TMP/chg out err rc W='title:^C$'
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  mkdir -p "$o"
  ob run -b "$B" -d -q --wait -- foot -T C -o cursor.blink=no cat >/dev/null 2>&1
  local wx wy ww wh
  read -r wx wy ww wh < <(ob windows -b "$B" --json | jq -r '.[] | select(.title == "C") | "\(.at[0]) \(.at[1]) \(.size[0]) \(.size[1])"')
  shoot() { out=$(ob shot -b "$B" "$@" 2>"$o/err"); rc=$?; err=$(cat "$o/err"); }
  shoot --changed -w "$W" -o "$o/a.png"
  check_eq "the first --changed: a whole shot, its path" "0 $o/a.png" "$rc $out"
  check_match "...said why" "no earlier shot of it to compare with: a whole shot of window" "$err"
  check_match "...the window's size" "$ww x $wh," "$(file "$o/a.png")"
  shoot --changed -w "$W" -o "$o/b.png"
  check_eq "nothing changed: exit 0, nothing on stdout, no image" "0||no" "$rc|$out|$([ -e "$o/b.png" ] && echo yes || echo no)"
  check_match "...said, since the last shot" "nothing changed in window .* since $o/a\.png \([0-9.]+s ago\): no image" "$err"
  ob keys -b "$B" --wait -t "hello" >/dev/null 2>&1
  shoot --changed -w "$W" -o "$o/c.png"
  check_eq "typed text: a crop" "0 $o/c.png" "$rc $out"
  local cw=0 ch=0 cx=-1 cy=-1
  [[ $err =~ since\ $o/a\.png\ .*:\ ([0-9]+)x([0-9]+)\ at\ ([0-9]+),([0-9]+)\ of\ window\ .*\(its\ coordinates\).*click\ with\ --in ]] &&
    cw=${BASH_REMATCH[1]} ch=${BASH_REMATCH[2]} cx=${BASH_REMATCH[3]} cy=${BASH_REMATCH[4]}
  check "...said where, in the window's coordinates: its top left, a tenth of it at most" \
    test "$cx" -ge 0 -a "$cx" -lt 60 -a "$cy" -ge 0 -a "$cy" -lt 60 -a "$((cw * ch * 10))" -lt "$((ww * wh))"
  check_match "...the image that size" "$cw x $ch," "$(file "$o/c.png")"
  ob click -b "$B" --in "$o/c.png" 5 6 >/dev/null 2>&1
  check_eq "click --in the crop: that point of the window" "$((wx + cx + 5)), $((wy + cy + 6))" "$(ob hyprctl -b "$B" cursorpos)"
  # The screen: where the pointer was is not a change, where it is now is drawn.
  shoot --changed -o "$o/s1.png"
  check_match "the screen's first --changed: whole" "no earlier shot of it.*a whole shot of the screen" "$err"
  ob pointer -b "$B" -- move $((wx + 300)) $((wy + 300)) >/dev/null 2>&1
  shoot --changed -o "$o/s2.png"
  local px=-1 py=-1 pw=0 ph=0
  [[ $err =~ :\ ([0-9]+)x([0-9]+)\ at\ ([0-9]+),([0-9]+)\ of\ the\ screen ]] &&
    pw=${BASH_REMATCH[1]} ph=${BASH_REMATCH[2]} px=${BASH_REMATCH[3]} py=${BASH_REMATCH[4]}
  check "the pointer moved: a crop around where it is now, not where it was" \
    test "$px" -le $((wx + 300)) -a $((px + pw)) -gt $((wx + 300)) -a "$py" -le $((wy + 300)) -a $((py + ph)) -gt $((wy + 300)) -a "$pw" -lt 120 -a "$ph" -lt 120
  check_match "...said: perhaps the pointer alone" "perhaps the pointer alone" "$err"
  # --since: an earlier whole shot; never a crop, nor the screen's for a window.
  shoot --since "$o/a.png" -o "$o/d.png"
  check_match "--since a whole shot of the window: what changed since it" "^0 $o/d\.png .*--changed since $o/a\.png .* of window .*\(its coordinates\)" "$rc $out $err"
  check_match "--since a crop refused" "is a crop" "$(ob shot -b "$B" --since "$o/c.png" 2>&1)"
  check_match "--since the screen's for a window refused" "is a shot of the screen, not of a window" "$(ob shot -b "$B" --since "$o/s1.png" -w "$W" 2>&1)"
  # A blinking block cursor in a second window (C is resized: its next --changed is whole, said why).
  ob run -b "$B" -d -q --wait -- foot -T K -o cursor.blink=yes cat >/dev/null 2>&1
  shoot --changed -w "$W" -o "$o/e.png"
  check_match "a resized window: a whole shot, said why" "^0 $o/e\.png .*it was ${ww}x$wh at the last shot of it: a whole shot" "$rc $out $err"
  ob shot -b "$B" -w 'title:^K$' -o "$o/k.png" >/dev/null 2>&1
  local i crops=0 none=0
  for i in 1 2 3 4 5 6; do
    sleep 0.35; shoot --changed -w 'title:^K$' -o "$o/k$i.png"
    if [ -n "$out" ]; then crops=$((crops + 1)); fi
  done
  check "a blinking block cursor: a change (crops: $crops of 6)" test "$crops" -ge 1
  for i in 1 2 3 4 5 6; do
    sleep 0.35; shoot --changed -w 'title:^K$' --ignore "0,0 80x80" -o "$o/ki$i.png"
    if [ -z "$out" ] && [ "$rc" = 0 ]; then none=$((none + 1)); fi
  done
  check_eq "...left out by --ignore (the window's coordinates): nothing changed each time" 6 "$none"
  # A blinking beam: a caret alone, not a change.
  ob run -b "$B" -d -q --wait -- foot -T L -o cursor.blink=yes -o cursor.style=beam cat >/dev/null 2>&1
  ob shot -b "$B" -w 'title:^L$' -o "$o/l.png" >/dev/null 2>&1
  local carets=0; none=0
  for i in 1 2 3 4 5 6; do
    sleep 0.35; shoot --changed -w 'title:^L$' -o "$o/l$i.png"
    if [ -z "$out" ] && [ "$rc" = 0 ]; then none=$((none + 1)); fi
    if [[ $err == *"a caret? "*" left out: no image"* ]]; then carets=$((carets + 1)); fi
  done
  check_eq "a blinking beam caret alone: nothing changed each time" 6 "$none"
  check "...the caret said (at least once: $carets of 6)" test "$carets" -ge 1
  ob down "$B" >/dev/null 2>&1
}

# pixel and shot --zoom (#133, finding 209) in a box: a known background, a terminal of a known colour.
t_pixel() {
  local B=$P-pix o=$TMP/pix out
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  mkdir -p "$o"
  ob lua -b "$B" 'hl.config({ misc = { background_color = 0xff345678 } })' >/dev/null
  check_eq "pixel X Y: the screen's colour there" "#345678" "$(ob pixel -b "$B" 3 3)"
  ob run -b "$B" -d -q --wait -- foot -T K -o colors-dark.background=123456 -o colors-dark.alpha=1.0 sleep 600 >/dev/null 2>&1
  check_eq "pixel --window: the window's own pixels" $'0,0 #123456\n40,30 #123456' "$(ob pixel -b "$B" -w 'title:^K$' 0 0 40 30)"
  check_eq "...--json, a line each" '{"x":40,"y":30,"hex":"#123456","r":18,"g":52,"b":86}' "$(ob pixel -b "$B" -w 'title:^K$' --json 40 30)"
  check_match "a point off the screen refused" "outside the screen" "$(ob pixel -b "$B" 5000 1 2>&1)"
  check_match "...off the window too" "outside the window" "$(ob pixel -b "$B" -w 'title:^K$' 5000 1 2>&1)"
  # --zoom: a crop across the window's corner, each pixel 8x8 and no colour blended between them.
  local ax ay; read -r ax ay < <(ob windows -b "$B" --json | jq -r '.[] | select(.title == "K") | "\(.at[0]) \(.at[1])"')
  ob pointer -b "$B" -- move 900 900 >/dev/null 2>&1
  local err; err=$(ob shot -b "$B" -g "$((ax - 4)),$((ay - 4)) 10x10" --zoom 8 -o "$o/z.png" 2>&1 >/dev/null)
  check_match "shot --zoom 8 of 10x10: 80x80" "80 x 80," "$(file "$o/z.png")"
  check_match "...said, with --in" "80x80 image of the screen's 10x10 at .*\(zoomed 8 times\); click with --in" "$err"
  ob shot -b "$B" -g "$((ax - 4)),$((ay - 4)) 10x10" -o "$o/z1.png" >/dev/null 2>&1
  check_eq "...the colours of the plain crop, none blended" "$(magick "$o/z1.png" -unique-colors -format %w info:)" "$(magick "$o/z.png" -unique-colors -format %w info:)"
  local px; px=$(ob pixel -b "$B" --in "$o/z.png" 44 44)
  check_match "pixel --in the zoomed shot: a colour" "^#[0-9a-f]{6}$" "$px"
  check_eq "...the screen's pixel there" "$(ob pixel -b "$B" $((ax + 1)) $((ay + 1)))" "$px"
  ob click -b "$B" --in "$o/z.png" 44 44 >/dev/null 2>&1
  check_eq "click --in the zoomed shot" "$((ax + 1)), $((ay + 1))" "$(ob hyprctl -b "$B" cursorpos)"
  ob down "$B" >/dev/null 2>&1
}

# output drop/back (#146, finding 210; #163, finding 232): a screen gone and back, under its name,
# mode, place and scale: the main screen, or a monitor by name, never one taken for the other (a
# second drop took the monitor, `mode` named it); a config reload while one is away does not bring it
# back; the shell sees it go (its log) and survives; the refusals. On a box with a monitor below the
# main screen at scale 1.6, on a GPU other than NVIDIA's (its boxes refuse both).
t_output() {
  local B=$P-out n
  check_match "output: drop or back" "drop or back" "$(ob output -b "$P-x" 2>&1)"
  check_match "output back takes no --for" "takes no --for" "$(ob output -b "$P-x" back --for 1s 2>&1)"
  check_match "output --for junk" "takes a duration" "$(ob output -b "$P-x" drop --for soon 2>&1)"
  check_match "output --cycles 0" "takes 1-999" "$(ob output -b "$P-x" drop --cycles 0 2>&1)"
  check_match "output drop of two names" "one output's name, got A and B" "$(ob output -b "$P-x" drop A B 2>&1)"
  check_match "output drop of a name that is none" "not an output's name" "$(ob output -b "$P-x" drop 'a b' 2>&1)"
  check "output is a jailed agent's" lib broker_check output
  n=$(gpu_node other)
  [ -n "$n" ] || { skip "output drop and back" "no render node here but NVIDIA's (t_output_nvidia runs there)"; return; }
  output_on "$n" "$B" HEADLESS-2 HEADLESS-3
}
# The same on NVIDIA (#165, finding 234), whose screens are Wayland outputs, windows on its labwc: one
# dropped is that window closed, one back a window again, sized by labwc's rule for it.
t_output_nvidia() {
  local n; n=$(nvidia_ready) || { skip "output drop and back on NVIDIA" "$n"; return; }
  output_on "$n" "$P-outnv" WAYLAND-1 WAYLAND-2
}
# output_on NODE BOX MAIN MONITOR: drop and back on a box on NODE with a monitor below the main screen.
output_on() {
  local n=$1 B=$2 m0=$3 m1=$4 out rc ml before
  env OMABOX_RENDER_NODE="$n" "$CLI" up "$B" --size 1280x720 --net isolated --monitor 1280x800,scale=1.6,below >/dev/null 2>&1 ||
    { no "up with a monitor" "failed"; return; }
  local list='map({name, mode, scale, x, y}) | sort_by(.name)'
  ml=$(ob monitor -b "$B" list --json | jq -c "$list")
  check_eq "two monitors, the second below at 1.6" "[{\"name\":\"$m0\",\"mode\":\"1280x720@60\",\"scale\":1,\"x\":0,\"y\":0},{\"name\":\"$m1\",\"mode\":\"1280x800@60\",\"scale\":1.6,\"x\":0,\"y\":720}]" "$ml"
  before=$(ob mode -b "$B")
  check_match "output back with nothing dropped: refused" "has its screen" "$(ob output -b "$B" back 2>&1)"
  out=$(ob output -b "$B" drop 2>&1); rc=$?
  check_eq "output drop: exit 0" 0 "$rc"
  check_match "...the main screen off, the monitor still on, said" "has its main screen \($m0\) off now, $m1 still on: omabox output back $m0" "$out"
  check_eq "...the box lists the monitor only" "[\"$m1\"]" "$(ob hyprctl -b "$B" -j monitors | jq -c '[.[] | select(.name != "FALLBACK") | .name]')"
  check_match "mode while the main screen is away: said, not the monitor's mode (#163)" "main screen \($m0\) is away" "$(ob mode -b "$B" 2>&1)"
  check_match "...nor set on the monitor" "main screen \($m0\) is away" "$(ob mode -b "$B" 1024x768 2>&1)"
  check_match "a second drop: refused, the main screen is away already (#163)" "main screen \($m0\) is dropped already" "$(ob output -b "$B" drop 2>&1)"
  check_eq "...the monitor still on, as it was" "[\"$m1 1280x800 0 720\"]" \
    "$(ob hyprctl -b "$B" -j monitors | jq -c '[.[] | select(.name != "FALLBACK") | "\(.name) \(.width)x\(.height) \(.x) \(.y)"]')"
  check_match "monitor list: the main screen there, marked dropped" "$m0 +1280x720@60 +1 +0,0 +\(dropped\)" "$(ob monitor -b "$B" list)"
  ob hyprctl -b "$B" reload >/dev/null; sleep 1
  check_eq "a config reload while it is away: not made again, the monitor where it was" "[\"$m1 1280x800 0 720\"]" \
    "$(ob hyprctl -b "$B" -j monitors | jq -c '[.[] | select(.name != "FALLBACK") | "\(.name) \(.width)x\(.height) \(.x) \(.y)"]')"
  out=$(ob output -b "$B" back); rc=$?
  check_eq "output back: the same name and mode ($before)" "0 $before" "$rc $out"
  check_eq "...every monitor as before" "$ml" "$(ob monitor -b "$B" list --json | jq -c "$list")"
  # A monitor by its name (#163): its mode, place and scale kept, its omabox.monitors line too.
  out=$(ob output -b "$B" drop "$m1" 2>&1); rc=$?
  check_eq "output drop $m1: exit 0" 0 "$rc"
  check_match "...said" "has its monitor $m1 off now, $m0 still on: omabox output back $m1" "$out"
  check_eq "...the main screen on, its mode as before" "$before" "$(ob mode -b "$B" 2>&1)"
  check_match "...dropped again: refused" "$m1 is dropped already" "$(ob output -b "$B" drop "$m1" 2>&1)"
  check_match "...monitor remove of it: refused, said" "$m1 is dropped" "$(ob monitor -b "$B" remove "$m1" 2>&1)"
  if [ "$m0" = HEADLESS-2 ]; then
    check_match "...monitor add under its name: refused" "already has a monitor $m1, dropped" "$(ob monitor -b "$B" add 800x600 --name "$m1" 2>&1)"
  else
    check_match "...monitor add: not under its name, the next free one" "^WAYLAND-3$" "$(ob monitor -b "$B" add 800x600 2>/dev/null)"
    ob monitor -b "$B" remove WAYLAND-3 >/dev/null 2>&1
  fi
  check_match "output back of one not dropped: refused" "$m0 is not dropped \(dropped: $m1\)" "$(ob output -b "$B" back "$m0" 2>&1)"
  check_match "output drop of one it does not have" "has no monitor HEADLESS-9" "$(ob output -b "$B" drop HEADLESS-9 2>&1)"
  ob hyprctl -b "$B" reload >/dev/null; sleep 1
  check_eq "a config reload while it is away: not made again" "[\"$m0\"]" "$(ob hyprctl -b "$B" -j monitors | jq -c '[.[] | select(.name != "FALLBACK") | .name]')"
  out=$(ob output -b "$B" back "$m1"); rc=$?
  check_eq "output back $m1: its name and mode" "0 $m1 1280x800@60" "$rc $out"
  check_eq "...its place and scale as before" "$ml" "$(ob monitor -b "$B" list --json | jq -c "$list")"
  # Both away: no screen; back brings both, the main screen first.
  ob output -b "$B" drop "$m1" >/dev/null 2>&1
  check_match "both dropped: no screen, said" "has no screen now \($m1, $m0 dropped\)" "$(ob output -b "$B" drop 2>&1)"
  check_eq "...none listed" "[]" "$(ob hyprctl -b "$B" -j monitors | jq -c '[.[] | select(.name != "FALLBACK")]')"
  out=$(ob output -b "$B" back); rc=$?
  check_eq "output back: both, the main screen first" "0 $before"$'\n'"$m1 1280x800@60" "$rc $out"
  check_eq "...modes, places and scales as before" "$ml" "$(ob monitor -b "$B" list --json | jq -c "$list")"
  check_match "...and nothing left to bring back" "nothing to bring back" "$(ob output -b "$B" back 2>&1)"
  out=$(ob output -b "$B" drop "$m1" --for 200ms --cycles 2); rc=$?
  check_eq "drop $m1 --for 200ms --cycles 2: that one, back each time" \
    "0 $(for i in 1 2; do echo "cycle $i/2: gone for 0.20s, back as $m1 1280x800@60; shell running"; done)" "$rc $out"
  check_eq "...every monitor as before" "$ml" "$(ob monitor -b "$B" list --json | jq -c "$list")"
  # One back while the main screen is away goes below it by its mode as `mode` set it, not as the
  # config was loaded with (1280x720: it went to 0,720, inside the main screen once that was back).
  ob mode -b "$B" 1600x900 >/dev/null 2>&1
  ob output -b "$B" drop >/dev/null 2>&1; ob output -b "$B" drop "$m1" >/dev/null 2>&1; ob output -b "$B" back "$m1" >/dev/null 2>&1
  check_eq "after mode 1600x900, both dropped, the monitor back alone: below the main screen's 900" "0,900" \
    "$(ob monitor -b "$B" list --json | jq -r --arg n "$m1" '.[] | select(.name == $n) | "\(.x),\(.y)"')"
  ob output -b "$B" back >/dev/null 2>&1; ob mode -b "$B" 1280x720 >/dev/null 2>&1
  check_eq "...the main screen back, mode 1280x720: all as before" "$ml" "$(ob monitor -b "$B" list --json | jq -c "$list")"
  out=$(ob output -b "$B" drop --for 300ms --cycles 3); rc=$?
  check_eq "drop --for 300ms --cycles 3: exit 0" 0 "$rc"
  check_eq "...three cycles, each back as before, the shell running" \
    "$(for i in 1 2 3; do echo "cycle $i/3: gone for 0.30s, back as $before; shell running"; done)" "$out"
  check_eq "...the box's mode as before" "$before" "$(ob mode -b "$B")"
  check "...the shell saw the screen go (its log)" until_ok 5 bash -c "'$CLI' log -b '$B' shell | grep -q 'There are no outputs'"
  check "...a shot of the main screen is the size it was" bash -c "'$CLI' shot -b '$B' --monitor '$m0' -o '$TMP/out.png' >/dev/null 2>&1 && file '$TMP/out.png' | grep -q '1280 x 720,'"
  # A shell crash ends the cycles, its report named (a SIGSEGV stands in for a plugin's crash).
  ( sleep 2.5; ob run -b "$B" -- pkill -SEGV -x quickshell ) & local k=$!
  out=$(ob output -b "$B" drop --for 1s --cycles 8 2>&1); rc=$?
  wait "$k"
  check_eq "a shell crash during the cycles: exit 1" 1 "$rc"
  check_match "...said, with its report" "shell crashed"$'\n'"omabox: output: the shell crashed in cycle [1-8] \(report: $(ob path "$B")/home/\.cache/quickshell/crashes/[^/]+/report\.txt;" "$out"
  check_eq "...the screen is back all the same" "$before" "$(ob mode -b "$B")"
  ob down "$B" >/dev/null 2>&1
}

# omabox gdb (#135, finding 211): a backtrace of the box's Hyprland, running or stopped, of a process
# of the box, and --watch catching a crash; a gdb of the box's own (run) is still refused.
t_gdb() {
  local B=$P-gdb out rc D
  check_match "gdb: --watch and -- ARGS refused" "--watch runs its own" "$(ob gdb -b "$P-x" --watch -- -ex bt 2>&1)"
  check_match "gdb: --pid junk refused" "--pid takes a pid" "$(ob gdb -b "$P-x" --pid x 2>&1)"
  check_match "gdb: gdb's options after --" "after --" "$(ob gdb -b "$P-x" -ex bt 2>&1)"
  check "gdb is a jailed agent's" lib broker_check gdb
  ob up "$B" --no-shell --net isolated >/dev/null 2>&1 || { no "up" "failed"; return; }
  D=$(ob path "$B")
  out=$(ob gdb -b "$B" 2>&1); rc=$?
  check_eq "gdb: exit 0" 0 "$rc"
  check_match "...every thread's backtrace, down to main" $'\n#[0-9]+ +0x[0-9a-f]+ in main \\(\\)' "$out"
  check_match "a gdb run in the box itself is still refused (ptrace_scope)" "ptrace: Operation not permitted" \
    "$(ob run -b "$B" -- gdb -nx -batch -p "$(ob run -b "$B" -- pgrep -xo Hyprland)" 2>&1)"
  ob run -b "$B" -- pkill -STOP -xo Hyprland
  out=$(timeout 30 "$CLI" gdb -b "$B" 2>&1); rc=$?
  check_match "a stopped Hyprland: backtraced (exit $rc)" $'\n#[0-9]+ +0x[0-9a-f]+ in main \\(\\)' "$out"
  check_match "...and left stopped" "^State:.*stopped" "$(ob run -b "$B" -- grep State "/proc/$(ob run -b "$B" -- pgrep -xo Hyprland)/status")"
  ob run -b "$B" -- pkill -CONT -xo Hyprland
  check "...it answers again" until_ok 5 ob hyprctl -b "$B" version
  ob run -b "$B" -d -q -- sleep 600
  local sp; sp=$(ob run -b "$B" -- pgrep -xn sleep)
  check_match "gdb --pid: a process of the box" "in (clock_nanosleep|__GI___clock_nanosleep|nanosleep)|#0 " "$(ob gdb -b "$B" --pid "$sp" 2>&1)"
  check_match "...one it has not: refused" "has no process 99999" "$(ob gdb -b "$B" --pid 99999 2>&1)"
  check_match "gdb --shell with no shell: refused" "has no shell running" "$(ob gdb -b "$B" --shell 2>&1)"
  # finding 229: the box's gdb.log linked to a host file, and a process named with a newline and an
  # escape. The watch's header is written in the box: the host file untouched, the log a file again.
  echo keep > "$TMP/gdb-victim"
  ob run -b "$B" -- ln -sf "$TMP/gdb-victim" /home/sbx/gdb.log
  # (in one write: bash's printf writes up to the newline first, and each write replaces the name)
  ob run -b "$B" -d -q -- python3 -c 'import os, time
os.write(os.open("/proc/self/comm", os.O_WRONLY), b"x\x1b[1my\nz"); open("/tmp/t229.pid", "w").write(str(os.getpid())); time.sleep(600)'
  local wp; until_ok 5 ob run -b "$B" -- test -s /tmp/t229.pid >/dev/null; wp=$(ob run -b "$B" -- cat /tmp/t229.pid)
  out=$(ob gdb -b "$B" --pid "$wp" --watch 2>&1); rc=$?
  check_eq "gdb --watch over a gdb.log linked to a host file: exit 0" 0 "$rc"
  check_eq "...that file untouched" keep "$(cat "$TMP/gdb-victim")"
  check "...the box's gdb.log a file of its own" test -f "$D/home/gdb.log" -a ! -L "$D/home/gdb.log"
  check_match "...a name with a newline and an escape: printed tamed" "gdb watches x\?\?1my\?z \(pid $wp in box" "$out"
  check_match "...in the header too" "== omabox gdb --watch x\?\?1my\?z \(pid $wp in the box\)" "$(ob log -b "$B" gdb -n all 2>&1)"
  out=$(ob gdb -b "$B" --watch 2>&1); rc=$?
  check_eq "gdb --watch: exit 0" 0 "$rc"
  check_match "...said, with where the backtrace goes" "gdb watches Hyprland .*omabox log -b $B gdb" "$out"
  # ...from the box's network namespace (finding 229), not the host's: the gdb that is the box's
  # (its pid namespace) and traces.
  local bp g gn=""; bp=$(cat "$D/pid")
  for g in $(pgrep -x gdb); do
    [ "$(readlink "/proc/$g/ns/pid")" = "$(readlink "/proc/$bp/ns/pid")" ] && gn+="$(readlink "/proc/$g/ns/net") "
  done
  check_eq "...gdb in the box's network namespace" "$(readlink "/proc/$bp/ns/net") $(readlink "/proc/$bp/ns/net") " "$gn"
  check_match "...a second gdb: refused, naming the watch" "traced already, by pid [0-9]+" "$(ob gdb -b "$B" 2>&1)"
  ob run -b "$B" -- pkill -SEGV -xo Hyprland
  check "...a crash: the box goes down as it would have" until_ok 15 bash -c "'$CLI' ls --json | jq -e '.[] | select(.name == \"$B\") | .state == \"dead\"'"
  out=$(ob log -b "$B" gdb -n all 2>&1)
  check_match "...the gdb log has the signal" "received signal SIGSEGV" "$out"
  check_match "...and every thread's backtrace, down to main" $'\n#[0-9]+ +0x[0-9a-f]+ in main \\(\\)' "$out"
  ob down "$B" >/dev/null 2>&1
}

# The gpu setting (#156, finding 214) on a fake /dev/dri and /sys/class/drm: an AMD and an NVIDIA GPU.
t_unit_gpu() {
  local f=$TMP/gpu
  mkdir -p "$f/dri" "$f/drm/renderD128" "$f/drm/renderD129" "$f/pci/0000:0a:00.0" "$f/pci/0000:01:00.0" "$f/drivers/amdgpu" "$f/drivers/nvidia" "$f/home"
  : > "$f/dri/renderD128"; : > "$f/dri/renderD129"
  ln -sfn "$f/pci/0000:0a:00.0" "$f/drm/renderD128/device"; ln -sfn "$f/pci/0000:01:00.0" "$f/drm/renderD129/device"
  ln -sfn "$f/drivers/amdgpu" "$f/pci/0000:0a:00.0/driver"; ln -sfn "$f/drivers/nvidia" "$f/pci/0000:01:00.0/driver"
  echo 0x030000 > "$f/pci/0000:0a:00.0/class"; echo 0x030200 > "$f/pci/0000:01:00.0/class"
  echo 0x1002 > "$f/pci/0000:0a:00.0/vendor"; echo 0x10de > "$f/pci/0000:01:00.0/vendor"
  # gpu VALUE: render_node with that setting in a config of its own (and no OMABOX_RENDER_NODE).
  pick() { HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' SYSPCI='$f/pci' CONFIG='$f/config'; unset OMABOX_RENDER_NODE; printf 'gpu=%s\n' '$1' > \$CONFIG; render_node" 2>&1; }
  check_eq "gpu auto: the first GPU" "$f/dri/renderD128" "$(pick auto)"
  check_eq "gpu nvidia: its render node" "$f/dri/renderD129" "$(pick nvidia)"
  check_eq "gpu amd" "$f/dri/renderD128" "$(pick amd)"
  check_eq "gpu by PCI slot" "$f/dri/renderD129" "$(pick 0000:01:00.0)"
  check_eq "gpu intel, none here: the first GPU, said, with why and which (finding 237)" \
    "omabox: gpu intel (omabox config gpu): none usable here (no intel GPU on this machine), so the first GPU: amdgpu 0000:0a:00.0 (renderD128)"$'\n'"$f/dri/renderD128" "$(pick intel)"
  rm "$f/dri/renderD129"; mkdir -p "$f/drivers/vfio-pci"; ln -sfn "$f/drivers/vfio-pci" "$f/pci/0000:01:00.0/driver"
  check_eq "gpu nvidia, the card on vfio-pci (no node): the first GPU, said, the card's reason given (finding 237)" \
    "omabox: gpu nvidia (omabox config gpu): none usable here (0000:01:00.0: bound to vfio-pci), so the first GPU: amdgpu 0000:0a:00.0 (renderD128)"$'\n'"$f/dri/renderD128" "$(pick nvidia)"
  check_match "...a slot that is no GPU: said" "none usable here \(no GPU at 0000:05:00.0\)" "$(pick 0000:05:00.0)"
  # The record a fallback leaves (box.json's render, as up writes it) and `ls`'s line for it on a
  # machine with one usable GPU, where the GPU line was shown only with two (finding 237).
  local lb=$TMP/gpu-ls; mkdir -p "$lb/fb" "$lb/plain"
  jq -n '{mode: "headless", size: "1x1@60", net: "none", idle: 0, render: {node: "/dev/dri/renderD128", pci: "0000:0a:00.0", driver: "amdgpu", fallback: true, wanted: "nvidia", why: "0000:01:00.0: bound to vfio-pci"}}' > "$lb/fb/box.json"
  jq -n '{mode: "headless", size: "1x1@60", net: "none", idle: 0, render: {node: "/dev/dri/renderD128", pci: "0000:0a:00.0", driver: "amdgpu", fallback: false}}' > "$lb/plain/box.json"
  local lsout; lsout=$(lib eval "BOXES='$lb' DRI='$f/dri' SYSDRM='$f/drm'; cmd_ls" 2>&1)
  check_match "ls with one usable GPU: a fallback box's GPU line, with why" \
    $'\nfb [^\n]*\n  GPU: amdgpu 0000:0a:00.0 \\(renderD128\\), a fallback: gpu nvidia was not usable \\(0000:01:00.0: bound to vfio-pci\\)' "$lsout"
  check_eq "...none for a box that is not one" "" "$(grep -A1 '^plain ' <<<"$lsout" | sed -n 2p | grep GPU)"
  local rec; rec=$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' SYSPCI='$f/pci' CONFIG='$f/config'; unset OMABOX_RENDER_NODE
    printf 'gpu=nvidia\n' > \$CONFIG; render_fallback '$f/dri/renderD128' && gpu_why_not nvidia" 2>&1)
  check_eq "gpu_why_not: the reason up records" "0000:01:00.0: bound to vfio-pci" "$rec"
  ln -sfn "$f/drivers/nvidia" "$f/pci/0000:01:00.0/driver"
  check_eq "OMABOX_RENDER_NODE still first" "$f/dri/renderD128" \
    "$(HOME=$f/home OMABOX_RENDER_NODE=$f/dri/renderD128 lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; printf 'gpu=nvidia\n' > \$CONFIG; render_node" 2>&1)"
  check_eq "config gpu: a slot written short is stored whole" "0000:03:00.0" "$(lib conf_check gpu 03:00.0)"
  check_eq "...a kind in any case" "nvidia" "$(lib conf_check gpu NVIDIA)"
  check_fails "...junk refused" lib conf_check gpu geforce
  check_eq "...a driver's name, as the list prints it, is its kind (finding 237)" "nvidia amd amd intel intel" \
    "$(for d in nouveau amdgpu Radeon i915 xe; do lib conf_check gpu "$d"; done | xargs)"
  check_eq "config gpu amdgpu: stored as amd" "gpu=amd" "$(HOME=$f/home ob config gpu amdgpu 2>/dev/null; rm -f "$f/home/.config/omabox/config")"
  check_match "config gpu junk: refused, saying what it takes" "bad gpu: geforce \(auto .*PCI slot" "$(HOME=$f/home ob config gpu geforce 2>&1)"
  check "...and nothing written" test ! -e "$f/home/.config/omabox/config"
  HOME=$f/home ob config gpu amd >/dev/null
  check_eq "config gpu amd: set" "amd" "$(HOME=$f/home ob config gpu)"
  check_eq "...in config --json" "amd" "$(HOME=$f/home ob config --json | jq -r .gpu)"
  # The list (config gpu, on stderr): every node, its driver and slot, what names it, * where boxes go.
  : > "$f/dri/renderD129"
  local list; list=$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; unset OMABOX_RENDER_NODE
    printf 'gpu=nvidia\n' > \$CONFIG; cmd_config gpu 2>&1 >/dev/null")
  check_match "config gpu lists the GPUs, on stderr" "^GPUs \(\* = where a headless box renders now" "$list"
  check_match "...the AMD one, with what names it" $'\n  renderD128 +amdgpu +0000:0a:00.0 +gpu amd or gpu 0000:0a:00.0' "$list"
  check_match "...the NVIDIA one marked: gpu nvidia is set" $'\n\\* renderD129 +nvidia +0000:01:00.0 +gpu nvidia or gpu 0000:01:00.0' "$list"
  check_eq "...stdout stays the value" nvidia "$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; cmd_config gpu 2>/dev/null")"
  check_match "config gpu intel with no Intel GPU: a note" "note: no usable GPU here is intel now" \
    "$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; cmd_config gpu intel 2>&1 >/dev/null")"
  check_eq "...none for one that is there" "" "$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; cmd_config gpu amd 2>&1 >/dev/null")"
  # #118: the GPUs on the bus for the widget's picker (config --json), a vfio-bound one (no render
  # node) too; lspci's name when there is one.
  mkdir -p "$f/pci/0000:02:00.0" "$f/pci/0000:00:1f.0" "$f/drivers/vfio-pci"
  echo 0x030000 > "$f/pci/0000:0a:00.0/class"; echo 0x030200 > "$f/pci/0000:01:00.0/class"
  echo 0x030000 > "$f/pci/0000:02:00.0/class"; echo 0x060100 > "$f/pci/0000:00:1f.0/class"
  ln -sfn "$f/drivers/vfio-pci" "$f/pci/0000:02:00.0/driver"; echo 0x8086 > "$f/pci/0000:02:00.0/vendor"
  printf '#!/bin/sh\ncase "$3" in\n  0000:01:00.0) echo "01:00.0 \\"3D controller\\" \\"NVIDIA Corporation\\" \\"GB203 [GeForce RTX 5070 Ti]\\" -ra1" ;;\n  0000:02:00.0) echo "02:00.0 \\"VGA compatible controller\\" \\"Vendor\\" \\"Plain Model\\"" ;;\nesac\n' > "$f/lspci"
  chmod +x "$f/lspci"
  local gj; gj=$(HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' SYSPCI='$f/pci' LSPCI='$f/lspci' CONFIG='$f/config'; unset OMABOX_RENDER_NODE
    printf 'gpu=0000:02:00.0\n' > \$CONFIG; cmd_config --json" 2>/dev/null)
  check_eq "config --json lists the display GPUs on the bus, not other devices (#118)" "0000:01:00.0 0000:02:00.0 0000:0a:00.0" "$(jq -r '[.gpus[].pci] | join(" ")' <<<"$gj")"
  check_eq "...with lspci's bracketed name, or the whole model" "GeForce RTX 5070 Ti|Plain Model|" "$(jq -r '[.gpus[].name] | join("|")' <<<"$gj")"
  check_eq "...one bound to vfio-pci: no node, unavailable, why" 'null false "bound to vfio-pci"' \
    "$(jq -r '.gpus[] | select(.pci == "0000:02:00.0") | "\(.node) \(.available) \(.why | tojson)"' <<<"$gj")"
  check_eq "...each one's kind: by its driver, or its vendor when bound to vfio-pci (finding 237)" "nvidia intel amd" "$(jq -r '[.gpus[].kind] | join(" ")' <<<"$gj")"
  check_eq "...a usable one: its node" "$f/dri/renderD129 true" "$(jq -r '.gpus[] | select(.pci == "0000:01:00.0") | "\(.node) \(.available)"' <<<"$gj")"
  check_eq "...gpu-auto, and gpu-now (the vfio one asked for: the fallback)" "0000:0a:00.0 0000:0a:00.0" "$(jq -r '"\(.["gpu-auto"]) \(.["gpu-now"])"' <<<"$gj")"
  fb() { HOME=$f/home lib eval "DRI='$f/dri' SYSDRM='$f/drm' CONFIG='$f/config'; unset OMABOX_RENDER_NODE
    printf 'gpu=%s\n' '$1' > \$CONFIG; render_fallback '$f/dri/$2' && echo yes || echo no"; }
  check_eq "render_fallback: auto never, the GPU asked for no, another yes" "no no yes" "$(fb auto renderD128) $(fb nvidia renderD129) $(fb 0000:02:00.0 renderD128)"
  # gpu release on boxes of a fake runtime dir (never the real ones: it takes boxes down): headless
  # boxes on that GPU only, by slot or kind; a box with no record by the node it holds.
  local bx=$TMP/gpu-boxes; mkdir -p "$bx"/{a,b,c,d,e}
  jq -n '{mode: "headless", render: {node: "/dev/dri/renderD129", pci: "0000:01:00.0", driver: "nvidia", fallback: false}}' > "$bx/a/box.json"
  jq -n '{mode: "headless", render: {node: "/dev/dri/renderD128", pci: "0000:0a:00.0", driver: "amdgpu", fallback: false}}' > "$bx/b/box.json"
  jq -n '{mode: "interactive"}' > "$bx/c/box.json"
  jq -n '{mode: "headless"}' > "$bx/d/box.json"
  jq -n '{mode: "headless", render: {node: "/dev/dri/renderD129", pci: "0000:01:00.0", driver: "nvidia", fallback: false}}' > "$bx/e/box.json"
  rel() { lib eval "BOXES='$bx' DRI='$f/dri' SYSDRM='$f/drm'; unset OMABOX_JAIL
    box_alive() { [ \"\$NAME\" != e ]; }   # e is dead
    box_render_held() { [ \"\$NAME\" = d ] && [ \"\$1\" = nvidia ] && echo 'nvidia 0000:01:00.0'; }
    cmd_down() { echo \"down \$*\"; }
    gpu_release $1" 2>&1; }
  check_eq "gpu release nvidia: the live headless boxes on it, one with no record by what it holds" \
    "omabox: gpu release: taking box 'a' down (it renders on nvidia 0000:01:00.0)|omabox: gpu release: taking box 'd' down (it renders on nvidia 0000:01:00.0)|down a d" "$(rel nvidia | paste -sd '|')"
  check_eq "...by slot, written short" "down b" "$(rel 0a:00.0 | tail -1)"
  check_match "...none on it: said, exit 0" "no headless box renders on intel" "$(rel intel)"
  check_match "...junk refused" "not a GPU: geforce" "$(rel geforce)"
  check_match "...refused to a jailed agent" "not a jailed agent's" "$(OMABOX_JAIL='{"id":"x"}' lib eval "gpu_release nvidia" 2>&1)"
  # An `up` in progress (its lock held, no box.json yet) is waited for (finding 237): here box f's up
  # writes its box.json, on nvidia, and lets go of the lock a second on.
  : > "$bx/.lock-f"
  flock "$bx/.lock-f" bash -c "sleep 1; mkdir -p '$bx/f'; jq -n '{mode: \"headless\", render: {node: \"/dev/dri/renderD129\", pci: \"0000:01:00.0\", driver: \"nvidia\", fallback: false}}' > '$bx/f/box.json'" &
  local holder=$!
  until_ok 5 bash -c "! flock -n '$bx/.lock-f' true"
  local out; out=$(rel nvidia | paste -sd '|'); wait "$holder"
  check_match "gpu release waits for an up in progress, said" "waiting for box 'f' \(an up or down in progress\)" "$out"
  check_match "...and takes its box down too" "\|down a d f$" "$out"
}

# Travel (#38, finding 111) and --mod (#25, finding 112): the pure parts and what is refused before
# any box is asked.
t_unit_pointer() {
  # omabox scroll (#134, finding 213)
  check_match "scroll: X Y DY needed" "need X Y DY" "$(ob scroll -b "$P-x" 1 2 2>&1)"
  check_match "scroll: not both 0" "not both 0" "$(ob scroll -b "$P-x" 1 2 0 0 2>&1)"
  check_match "scroll: a bad source" "--source is wheel" "$(ob scroll -b "$P-x" 1 2 3 --source mouse 2>&1)"
  check_match "scroll: --in or --window" "not both" "$(ob scroll -b "$P-x" --in a -w b 1 2 3 2>&1)"
  check_match "scroll: --quiet goes with --wait" "go with --wait" "$(ob scroll -b "$P-x" --quiet 1s 1 2 3 2>&1)"
  check "scroll is a jailed agent's" lib broker_check scroll
  check_eq "pointer marks: peek's words only" "move 1 2 scroll 15 scroll -3" "$(lib pointer_marks move 1 2 source wheel scroll 15 hscroll -3)"
  # #167: scroll --mod as click's; a wheel's rounding to whole notches said.
  check_match "scroll --mod junk refused" "not a modifier" "$(ob scroll -b "$P-x" --mod meta 1 2 3 2>&1)"
  check_eq "wheel_note: 20 is one notch" "DY 20 is 1 notch (15)" "$(lib wheel_note 20 0)"
  check_eq "...whole notches: nothing" "" "$(lib wheel_note 45 -30)"
  check_eq "...both, a small one at least a notch, negative" "DY 5 is 1 notch (15), DX -37 is 2 notches (-30)" "$(lib wheel_note 5 -37)"
  check_eq "...a fraction" "DX 7.5 is 1 notch (15)" "$(lib wheel_note 0 7.5)"
  check_eq "steps_to: N moves, the last on the target" "move 3 7 move 6 14 move 10 21" "$(lib eval 'SEQ=(); steps_to 0 0 10 21 3; echo "${SEQ[*]}"')"
  check_eq "...leftwards and up too" "move 5 5 move 0 0" "$(lib eval 'SEQ=(); steps_to 10 10 0 0 2; echo "${SEQ[*]}"')"
  check_eq "path_rects (drag --wait): the cursor's rect at each point" "84,84,64,64 34,34,64,64 -16,-16,64,64" \
    "$(lib eval 'SEQ=(); steps_to 100 100 0 0 2; path_rects 100 100 "${SEQ[@]}" | xargs')"
  check_eq "...999 steps: 30 rects (omabox-still takes 32), from the start to the end" "30 -16,-16 984,984" \
    "$(lib eval 'SEQ=(); steps_to 0 0 1000 1000 999; path_rects 0 0 "${SEQ[@]}"' | awk -F, '{ n++; if (n == 1) a = $1 "," $2; e = $1 + $3 - 64 "," $2 + $4 - 64 } END { print n, a, e }')"
  check_eq "--mod: names, any case, each once, in order given" "ctrl+shift+super" "$(lib mods_add "" Control,shift+CTRL+win)"
  check_eq "...added to earlier ones" "alt+ctrl" "$(lib mods_add alt ctrl+alt)"
  check_match "...not a modifier: refused" "'hyper' is not a modifier" "$(lib mods_add "" ctrl+hyper 2>&1)"
  check_match "click --steps 0 refused" "--steps is 1-999" "$(ob click -b "$P-x" --steps 0 1 1 2>&1)"
  check_match "pointer --steps 1000 refused" "--steps is 1-999" "$(ob pointer -b "$P-x" --steps 1000 -- move 1 1 2>&1)"
  check_match "click --mod junk refused" "not a modifier" "$(ob click -b "$P-x" --mod meta 1 1 2>&1)"
  check_match "drag --mod junk refused" "not a modifier" "$(ob drag -b "$P-x" --mod ctrl,x 1 1 2 2 2>&1)"
  check_match "drag --hold 500 refused, saying the unit (#103)" "like 300ms or 2s \\(a bare number is seconds\\), got 500" "$(ob drag -b "$P-x" --hold 500 1 1 2 2 2>&1)"
  check_match "keys -m is omabox's own" "click, drag or pointer --mod" "$(ob keys -b "$P-x" -m ctrl a 2>&1)"
  # The input tools refuse outside a box before they connect (SECURITY.md): run from a host shell they
  # would drive the real desktop. Checked in a sandbox with no /opt/omabox, no runtime dir, no display.
  local t out rc
  local arg
  for t in keyboard pointer still events; do
    arg=(); [ "$t" != events ] || arg=(/dev/null)   # (events wants its file first, or it says its usage)
    rc=0; out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --tmpfs /run \
      --die-with-parent env -i "$ROOT/tools/$t/omabox-$t" "${arg[@]}" 2>&1) || rc=$?
    check_eq "omabox-$t refuses outside a box" 2 "$rc"
    check_match "...and says so" "only runs inside an omabox box" "$out"
  done
  # (#108, finding 177) Not a directory test: a plain /opt/omabox/share (an install, a stray mkdir) is
  # no box, nor is a mount there where the system bus is (a stand-in file for its socket, never the bus).
  : > "$TMP/fakebus"
  for t in keyboard pointer; do
    rc=0; out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --dir /opt/omabox/share \
      --tmpfs /run --die-with-parent env -i "$ROOT/tools/$t/omabox-$t" 2>&1) || rc=$?
    check_eq "omabox-$t refuses where /opt/omabox/share is a plain dir" "2 yes" "$rc $(grep -q 'only runs inside' <<<"$out" && echo yes)"
    rc=0; out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --ro-bind "$ROOT/share" /opt/omabox/share \
      --tmpfs /run --dir /run/dbus --ro-bind "$TMP/fakebus" /run/dbus/system_bus_socket --die-with-parent env -i "$ROOT/tools/$t/omabox-$t" 2>&1) || rc=$?
    check_eq "...and where the system bus is, a mount there or not" "2 yes" "$rc $(grep -q 'only runs inside' <<<"$out" && echo yes)"
  done
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
  # drag --hold is a duration (#103): 300ms holds 300 ms, not 300 s (nor is a bare 2 two ms).
  local t0 took; t0=$(now_ms)
  ob drag -b "$B" --window R 50 50 150 50 --hold 300ms >/dev/null
  took=$(( $(now_ms) - t0 ))
  check_eq "drag --hold 300ms: held 300 ms, under 2 s in all" yes "$( ((took >= 300 && took < 2000)) && echo yes || echo "took $took ms")"
  # Never left down: a pause that times out lets go; the tool killed outright leaves them down until
  # the next keyboard event, which omabox then sends.
  local out
  out=$(ob run -b "$B" -- sh -c 'sleep 2 | /opt/omabox/bin/omabox-keyboard -m ctrl -p 300; echo "rc=$?"' 2>&1)
  check_match "a pause with nothing on stdin times out, letting go" "letting go.*rc=1" "$(tr '\n' ' ' <<<"$out")"
  check_eq "-p and -T together refused" 2 "$(ob run -b "$B" -- /opt/omabox/bin/omabox-keyboard -m ctrl -p 300 -T </dev/null >/dev/null 2>&1; echo $?)"
  check_eq "the pointer tool's --extent: WxH with junk after it refused (#109)" 2 \
    "$(ob run -b "$B" -- /opt/omabox/bin/omabox-pointer --extent 1920x1080abc sleep 1 >/dev/null 2>&1; echo $?)"
  check_eq "...a sign refused" 2 "$(ob run -b "$B" -- /opt/omabox/bin/omabox-pointer --extent -1920x1080 sleep 1 >/dev/null 2>&1; echo $?)"
  check_eq "...WxH taken" 0 "$(ob run -b "$B" -- /opt/omabox/bin/omabox-pointer --extent 1920x1080 sleep 1 >/dev/null 2>&1; echo $?)"
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
  # #111, finding 180: what no check sent before. R's codes since a count: presses (0 left, 1 middle,
  # 2 right) and the wheel (64 up, 65 down); motions are 32 and up below 64 (32 + a held button).
  local n0
  # shellcheck disable=SC2329 # called below
  pr() { reports | tail -n +$((n0 + 1)) | awk '$1 < 32 || $1 >= 64' | tr '\n' ' '; }
  # shellcheck disable=SC2329
  dragged() { reports | tail -n +$((n0 + 1)) | awk '$1 == 32' | wc -l; }   # motions, left held
  n0=$(reports | wc -l); ob click -b "$B" --window R 50 50 right >/dev/null
  check_eq "click right: button 2" "2 " "$(pr)"
  n0=$(reports | wc -l); ob click -b "$B" --window R 50 50 middle >/dev/null
  check_eq "click middle: button 1" "1 " "$(pr)"
  n0=$(reports | wc -l); ob click -b "$B" --window R 50 50 --double >/dev/null
  check_eq "click --double: two presses" "0 0 " "$(pr)"
  n0=$(reports | wc -l); ob pointer -b "$B" --window R -- move 60 60 scroll 3 >/dev/null
  check_match "pointer scroll 3: the wheel down" '^(65 )+$' "$(pr)"
  n0=$(reports | wc -l); ob pointer -b "$B" --window R -- move 60 60 scroll -3 >/dev/null
  check_match "...scroll -3: up" '^(64 )+$' "$(pr)"
  # #134, finding 213: sideways, and a wheel's notches (foot reports each notch as several lines: its
  # scroll multiplier; the notches are counted by Hyprland's binds below).
  n0=$(reports | wc -l); ob scroll -b "$B" --window R 60 60 0 30 --source wheel >/dev/null
  check_match "scroll 0 30 --source wheel: right" '^(67 )+$' "$(pr)"
  n0=$(reports | wc -l); ob scroll -b "$B" --window R 60 60 0 -15 --source wheel >/dev/null
  check_match "...0 -15: left" '^(66 )+$' "$(pr)"
  n0=$(reports | wc -l); ob scroll -b "$B" --window R 60 60 45 --source wheel >/dev/null
  check_match "...45: down" '^(65 )+$' "$(pr)"
  n0=$(reports | wc -l); ob pointer -b "$B" --window R -- move 60 60 hscroll 15 >/dev/null
  check_match "pointer hscroll 15: right" '^(67 )+$' "$(pr)"
  n0=$(reports | wc -l); ob scroll -b "$B" --window R 60 60 0 30 --source tilt >/dev/null 2>&1
  check_match "scroll 0 30 --source tilt: right, the app sees it (#167; Hyprland's binds see few)" '^(67 )+$' "$(pr)"
  n0=$(reports | wc -l); ob scroll -b "$B" --window R 60 60 -60 --source finger >/dev/null
  check_match "scroll -60 --source finger: up" '^(64 )+$' "$(pr)"
  # Hyprland's wheel binds: mouse_left/right too (with no delay between scroll events: binds take
  # one in 300 ms by default, a real wheel's too).
  ob lua -b "$B" 'hl.config({ binds = { scroll_event_delay = 0 } })
    for _, k in ipairs({ "mouse_left", "mouse_right", "mouse_down" }) do
      hl.bind(k, function() local f = io.open("/tmp/" .. k, "a"); f:write("x\n"); f:close() end)
    end' >/dev/null
  ob scroll -b "$B" 960 540 0 30 --source wheel >/dev/null; ob scroll -b "$B" 960 540 0 -15 >/dev/null
  ob scroll -b "$B" 960 540 45 --source wheel >/dev/null
  check_eq "Hyprland binds: mouse_right twice (two notches), mouse_left once (one plain event), mouse_down 3" "2 1 3" \
    "$(ob run -b "$B" -- wc -l /tmp/mouse_right /tmp/mouse_left /tmp/mouse_down | awk 'NR <= 3 { printf "%s%s", (NR > 1 ? " " : ""), $1 }')"
  # scroll --mod (#167, finding 236): Omarchy's SUPER+wheel binds, held as click holds them; the plain
  # mouse_down bind does not fire under SUPER.
  ob lua -b "$B" 'hl.bind("SUPER + mouse_down", function() local f = io.open("/tmp/super_down", "a"); f:write("x\n"); f:close() end)' >/dev/null
  ob scroll -b "$B" 960 540 30 --source wheel --mod super >/dev/null 2>&1
  check_eq "scroll --mod super: SUPER+mouse_down twice (two notches), mouse_down still 3" "2 3" \
    "$(ob run -b "$B" -- wc -l /tmp/super_down /tmp/mouse_down | awk 'NR <= 2 { printf "%s%s", (NR > 1 ? " " : ""), $1 }')"
  check_match "...and a wheel's DY not in whole notches is said" "a wheel turns in notches of 15: DY 20 is 1 notch \(15\)" \
    "$(ob scroll -b "$B" 960 540 20 --source wheel --mod super 2>&1 >/dev/null)"
  n0=$(reports | wc -l); ob pointer -b "$B" --window R -- move 60 60 down right move 90 90 up right >/dev/null
  check_eq "pointer down right ... up right: button 2" "2 " "$(pr)"
  n0=$(reports | wc -l); ob drag -b "$B" --window R 50 50 150 50 middle >/dev/null
  check_eq "drag middle: button 1" "1 " "$(pr)"
  local d3 d12
  n0=$(reports | wc -l); ob drag -b "$B" --window R 50 100 250 100 --steps 3 >/dev/null; d3=$(dragged)
  n0=$(reports | wc -l); ob drag -b "$B" --window R 50 100 250 100 --steps 12 >/dev/null; d12=$(dragged)
  check "drag --steps: 3 steps move less often than 12 ($d3, $d12 motions)" test "$d3" -ge 3 -a "$d3" -lt "$d12"
  n0=$(reports | wc -l)
  check "click --mod altgr" ob click -b "$B" --window R 50 50 --mod altgr
  ob click -b "$B" --window R 50 50 >/dev/null
  check_eq "...and nothing held after it (the next click plain)" "0 0 " "$(pr)"
  check_match "env: the box's display to export" "^export WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/omabox/$B/run/wayland-[0-9]+$" "$(ob env -b "$B" | head -1)"
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
  # wait layer (finding 146): newer Omarchy keeps a hidden menu mapped at 1x1; that is not the menu.
  local lj='{"HEADLESS-1":{"levels":{"2":[{"namespace":"omarchy-bar","x":0,"y":0,"w":1920,"h":34}],
    "3":[{"namespace":"omarchy-menu","x":0,"y":0,"w":1,"h":1},{"namespace":"omarchy-osd","x":0,"y":0,"w":0,"h":0}]}}}'
  check_eq "wait layer: a drawn layer is there" "0,0 1920x34" "$(lib layer_at omarchy-bar <<<"$lj")"
  check_eq "...a hidden 1x1 one is not" "" "$(lib layer_at omarchy-menu <<<"$lj")"
  check_eq "...nor an empty one" "" "$(lib layer_at omarchy-osd <<<"$lj")"
  check_eq "...the drawn one when both are mapped" "660,300 600x480" \
    "$(lib layer_at omarchy-menu <<<"${lj/\"w\":1,\"h\":1\}/\"w\":1,\"h\":1\},{\"namespace\":\"omarchy-menu\",\"x\":660,\"y\":300,\"w\":600,\"h\":480\}}")"
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
  # --ignore and -g (#131, finding 208)
  check_match "--ignore without --wait refused" "go with --wait" "$(ob keys -b "$P-x" --ignore "1,1 5x5" a 2>&1)"
  check_match "-g without --wait refused" "go with --wait" "$(ob click -b "$P-x" -g "1,1 5x5" 1 1 2>&1)"
  check_match "--ignore junk refused" '--ignore is "X,Y WxH"' "$(ob wait -b "$P-x" still --ignore 1,2,3 2>&1)"
  check_match "...on --wait too" '--ignore is "X,Y WxH"' "$(ob run -b "$P-x" -d --wait --ignore 1x2 -- true 2>&1)"
  check_match "-g twice on --wait refused" "-g once" "$(ob keys -b "$P-x" --wait -g "1,1 5x5" -g "2,2 5x5" a 2>&1)"
  check_match "wait window --ignore refused" "go with still and change" "$(ob wait -b "$P-x" window x --ignore "1,1 5x5" 2>&1)"
  check_match "--wait --ignore outside -g's region: said (#162)" 'ignore "100,100 5x5" is outside -g' \
    "$(ob keys -b "$P-x" --wait -g "0,0 10x10" --ignore "100,100 5x5" a 2>&1)"
  check_eq "...one inside it is not" "" "$(ob keys -b "$P-x" --wait -g "0,0 10x10" --ignore "5,5 5x5" a 2>&1 | grep outside)"
  local ig=() i; for i in $(seq 17); do ig+=(--ignore "$i,1 5x5"); done
  check_match "17 --ignore refused" "at most 16" "$(ob wait -b "$P-x" still "${ig[@]}" 2>&1)"
  # The 124's hint: changes kept to a small region late in the wait are named as an --ignore; a large
  # one is not (ignoring most of the screen is not waiting on it).
  local hint='source "$1"; NAME=u D=$2; exec {SETTLE_IN}>/dev/null {SETTLE_OUT}< <(echo "$3"); sleep 0.1 & SETTLE_PID=$!
    SETTLE_LINE="ready 1920x1080" SETTLE_ERR="" SETTLE_T0=$(date +%s%3N); settle_end still 0 ""'
  out=$(bash -c "$hint" _ "$TMP/lib/bin/omabox" "$TMP" \
    "unsatisfied changing t=10000 first=10 last=9990 change=28,90,14,900 ignored=- why=- late=27,81,15,918 frames=500" 2>&1)
  check_eq "a 124 still changing in one small region: named as an --ignore" \
    'unsatisfied: still changing after 10.00s (last change 9.99s at 28,90 14x900); from 5.00s on it changed only at 27,81 15x918: --ignore "27,81 15x918" if that is an animation' "$out"
  out=$(bash -c "$hint" _ "$TMP/lib/bin/omabox" "$TMP" \
    "unsatisfied changing t=10000 first=10 last=9990 change=0,0,1920,1080 ignored=- why=- late=0,0,1000,1080 frames=500" 2>&1)
  check_eq "...over a quarter of the screen: no hint" 'unsatisfied: still changing after 10.00s (last change 9.99s at 0,0 1920x1080)' "$out"
  # Several monitors (#162, finding 231): a quarter of what they cover, not of the box around them
  # (3000x2370 here, with gaps); the tool says so as area=N.
  out=$(bash -c "${hint/ready 1920x1080/ready 3000x2370 area=3240000}" _ "$TMP/lib/bin/omabox" "$TMP" \
    "unsatisfied changing t=10000 first=10 last=9990 change=1920,450,1080,900 ignored=- why=- late=1920,450,1080,900 frames=500" 2>&1)
  check_eq "...several monitors: a quarter of their own area (area=), not of the box around them" \
    'unsatisfied: still changing after 10.00s (last change 9.99s at 1920,450 1080x900)' "$out"
  check_match "help scroll and help drag have the --wait section (#162)" "take --wait.*take --wait" \
    "$(ob help scroll | tr '\n' ' ') $(ob help drag | tr '\n' ' ')"
  local out rc=0
  out=$(bwrap --ro-bind / / --dev /dev --proc /proc --unshare-pid --unshare-net --tmpfs /opt --die-with-parent \
        env -i "$ROOT/tools/still/omabox-still" still --timeout 100 2>&1) || rc=$?
  check_eq "omabox-still refuses outside a box" 2 "$rc"
  check_match "...and says so" "only runs inside an omabox box" "$out"
  # #124, finding 181: settle_end waited on the tool with no limit. One that never ends (and never
  # answered) is killed after 2 s, with what runs it; a box not answering either is said so.
  out=$(timeout 30 bash -c 'source "$1"; NAME=u D=$2; box_alive() { return 0; }; hypr_answers() { return 1; }
    sleep 300 & SETTLE_PID=$!; exec {SETTLE_IN}>/dev/null {SETTLE_OUT}</dev/null
    SETTLE_LINE="" SETTLE_ERR="" SETTLE_T0=$(date +%s%3N); t0=$SECONDS rc=0
    settle_end still 0 "" || rc=$?
    echo "rc $rc in $((SECONDS - t0))s, the tool $(kill -0 "$SETTLE_PID" 2>/dev/null && echo alive || echo gone)"' \
    _ "$TMP/lib/bin/omabox" "$TMP" 2>&1)
  check_match "settle_end: a tool that never ends is not waited for (#124)" \
    "^unknown: box 'u': its Hyprland did not answer \(hung\? omabox log -b u; omabox down u\)$" "$(head -n 1 <<<"$out")"
  check_match "...exit 1 within seconds, the tool killed" "^rc 1 in [2-6]s, the tool gone$" "$(tail -n 1 <<<"$out")"
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
  # omabox-still names the last thing it ignored: the cursor the key press hid, or foot's caret when a
  # later frame had it (#116: seen under load). Either way not a change that kept it from settling.
  check_match "...the cursor hidden by the key press (or the caret) is ignored, said" "ignored .*: (cursor|caret\?)" "$out"
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
  # (#110) The wait's own elapsed, not the clock around the CLI: its start-up under load took 1-2 s.
  local tw; tw=$(ob wait -b "$B" --timeout 1s --json window 'title:^nope$'); rc=$?
  check_eq "a window that never comes: 124" "124 unsatisfied" "$rc $(jq -r .result <<<"$tw")"
  check "...at the deadline (1 s asked, $(jq -r .elapsed <<<"$tw") s)" jq -e '.elapsed >= 1 and .elapsed < 3' <<<"$tw"
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
  # A steady animation (#131, finding 208): one cell that never stops changing. The 124 names it as an
  # --ignore; --ignore'd, wait still and keys --wait are satisfied; -g watches elsewhere.
  ob run -b "$B" -- pkill -x foot >/dev/null
  ob wait -b "$B" window foot --gone >/dev/null
  ob run -b "$B" -d -q -- foot -T N sh -c 'i=0; while :; do printf "\r%s" $((i++ % 10)); sleep 0.05; done'
  ob wait -b "$B" window 'title:^N$' >/dev/null
  out=$(ob wait -b "$B" still --timeout 2s); rc=$?
  check_eq "a cell that never stops changing: 124" 124 "$rc"
  check_match "...named as an --ignore" '; from 1\.[0-9]+s on it changed only at [0-9]+,[0-9]+ [0-9]+x[0-9]+: --ignore "[0-9]+,[0-9]+ [0-9]+x[0-9]+" if that is an animation$' "$out"
  local g; g=$(sed -n 's/.*--ignore "\([^"]*\)".*/\1/p' <<<"$out")
  out=$(ob wait -b "$B" still --ignore "$g"); rc=$?
  check_eq "wait still --ignore \"$g\": satisfied" 0 "$rc"
  check_match "...saying what it ignored" "ignored [0-9]+x[0-9]+ at [0-9]+,[0-9]+: --ignore\)$" "$out"
  out=$(ob wait -b "$B" still --ignore "$g" --strict --json); rc=$?
  check_eq "...--strict keeps an --ignore" "0 satisfied" "$rc $(jq -r .result <<<"$out")"
  out=$(ob keys -b "$B" --wait --timeout 2s -t x 2>/dev/null); rc=$?
  check_eq "keys --wait without it: 124" 124 "$rc"
  check_match "...still changing" "^unsatisfied: still changing" "$out"
  out=$(ob keys -b "$B" --wait --ignore "$g" -t y); rc=$?
  check_eq "keys --wait --ignore it: settled" 0 "$rc"
  out=$(ob keys -b "$B" --wait --start 1s -g "1000,600 200x200" --json shift 2>/dev/null); rc=$?
  check_eq "keys --wait -g elsewhere: nothing changed there (not still changing)" "124 unsatisfied nothing changed" \
    "$rc $(jq -r '"\(.result) \(.message | split(" in ")[0])"' <<<"$out")"
  check "wait --json: late_changes is the region" jq -e '.late_changes.w > 0' <<<"$(ob wait -b "$B" still --timeout 1s --json)"
  # drag --wait (#102, finding 172): its own cursor crossing the screen is not a change; what the drag
  # does beside it (foot's selection across several lines) is.
  ob run -b "$B" -- pkill -x foot >/dev/null
  ob wait -b "$B" window foot --gone >/dev/null; ob wait -b "$B" still >/dev/null
  out=$(ob drag -b "$B" --wait --json --start 1s 200 200 900 700 2>/dev/null); rc=$?
  check_eq "drag --wait across an empty screen: 124" "124 unsatisfied" "$rc $(jq -r .result <<<"$out")"
  ob run -b "$B" -d --wait -- foot -T D sh -c 'for i in $(seq 60); do echo "line $i: the quick brown fox jumps over the lazy dog"; done; exec sleep 600' >/dev/null 2>&1
  out=$(ob drag -b "$B" --wait 200 200 900 700); rc=$?
  check_eq "drag --wait selecting text: settled" 0 "$rc"
  check_match "...on a change that is not the cursor" "^satisfied: settled after .*last change" "$out"
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
  # finding 229: the log is opened in the box. A box that knows the name (a `date` of the suite's says
  # it here) and links it to a host file: that file untouched, the log a file of the box's.
  mkdir -p "$TMP/rep-bin"; echo keep > "$TMP/rep-victim"
  printf '#!/bin/sh\n[ "$1" = +%%s%%N ] && { echo 1229; exit; }\nexec /usr/bin/date "$@"\n' > "$TMP/rep-bin/date"; chmod +x "$TMP/rep-bin/date"
  ob run -b "$B" -- ln -sf "$TMP/rep-victim" /home/sbx/run-1229.log
  out=$(PATH=$TMP/rep-bin:$PATH ob run -b "$B" -d -q --print-log -- echo omabox-t229 2>&1)
  check_eq "run -d over a link the box put at its log's name: the path" "$D/home/run-1229.log" "$out"
  check "...the log is the box's, with the output" until_ok 5 bash -c "[ -f '$D/home/run-1229.log' ] && [ ! -L '$D/home/run-1229.log' ] && grep -qx omabox-t229 '$D/home/run-1229.log'"
  check_eq "...the link's target untouched" keep "$(cat "$TMP/rep-victim")"
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
  # #130: `--grep -i RE` took -i as the expression, and RE as a log's name.
  check_fails "log --grep -i RE: -i is the flag, RE the expression (#130)" grep -q "no log called" \
    <<<"$(ob log -b "$P-x" --grep -i 'plugin|error' 2>&1)"
  check_match "log --grep -x: an option taken for the expression, refused" "--grep takes RE" "$(ob log -b "$P-x" --grep -x 2>&1)"
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
  check_eq "lua: a byte that is not UTF-8 is \\u00XX (here as jq reads it), not U+FFFD (#113); UTF-8 stays" '"aÿbétéÃ"' \
    "$(ob lua -b "$B" --json '"a\xffb\xe9t\xc3\xa9\xc3"')"
  check_eq "...a stray byte after a 4-byte character" '"😀'$'\xc2\x80''"' "$(ob lua -b "$B" --json '"\xf0\x9f\x98\x80\x80"')"
  check_eq "lua: a Hyprland object's fields (from its stubs)" 1920 "$(ob lua -b "$B" 'hl.get_monitors()[1]' | jq .width)"
  check_match "...an object inside one is its name" '^"HL\.Workspace' "$(ob lua -b "$B" 'hl.get_monitors()[1]' | jq .active_workspace)"
  check_eq "lua: the source on stdin" 42 "$(echo 'return 40 + 2' | ob lua -b "$B" -)"
  out=$(ob lua -b "$B" 'error("boom")' 2>&1); rc=$?
  check_eq "lua: a Lua error is exit 1" 1 "$rc"
  check_eq "...with its message" "lua: lua:1: boom" "$out"
  check_match "lua: a syntax error too" "^lua: lua:1: .*near" "$(ob lua -b "$B" '1 +' 2>&1)"
  # #107, finding 176: a metamethod's error while encoding is a Lua error too; -1 after the source is source.
  check_eq "lua: an error in a value's metamethod is a Lua error" "1 lua: lua:1: boom" \
    "$(out=$(ob lua -b "$B" 'return setmetatable({}, {__len = function() error("boom") end})' 2>&1); echo "$? $out")"
  check_eq "lua return -1" -1 "$(ob lua -b "$B" return -1)"
  # #130: a dispatcher returned is not run, and agents read its value as "ran": said on stderr.
  ob run -b "$B" -- rm -f /tmp/omabox-t130
  out=$(ob lua -b "$B" 'hl.dsp.exec_cmd("touch /tmp/omabox-t130")' 2>&1 >/dev/null)
  check_match "lua: a dispatcher returned is said not run (#130)" "a dispatcher, returned and not run: omabox lua 'hl\.dispatch\(EXPR\)'" "$out"
  check_eq "...its value as before, exit 0" "0 HL.Dispatcher" "$(v=$(ob lua -b "$B" 'hl.dsp.exec_cmd("true")' 2>/dev/null); echo "$? $v")"
  sleep 0.5
  check_fails "...and it did not run" ob run -b "$B" -- test -e /tmp/omabox-t130
  check_match "lua: a function, said not called" "a function, returned and not called" "$(ob lua -b "$B" 'function() end' 2>&1 >/dev/null)"
  check_eq "...a plain value says nothing more" "" "$(ob lua -b "$B" '1, "HL.Dispatcher"' 2>&1 >/dev/null)"
  check_match "...a source starting with - still needs --" "unknown option -1" "$(ob lua -b "$B" -1 2>&1)"
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
  # The same start: the first read is where the second begins (#116: the end of run -d --wait's
  # screen watch, screencast>>0, could be logged between the two reads).
  local e1 e2; e1=$(ob events -b "$B" --since m1); e2=$(ob events -b "$B" --since "$m1")
  check "...--since OFFSET reads from the same place" test -n "$e1" -a "${e2:0:${#e1}}" = "$e1"
  check_fails "...nothing from before the mark" bash -c "'$CLI' events -b '$B' --since m1 | grep -q 'omabox>>listening'"
  ob events -b "$B" --mark v1x0 >/dev/null 2>&1; ob events -b "$B" --mark v1.0 >/dev/null 2>&1
  check "events --mark v1.0 leaves the mark v1x0 (a name, not a regex)" ob events -b "$B" --since v1x0
  # #107, finding 176: the other small refusals, said in words.
  check_match "keys -T is omabox's own" "-T is omabox's own" "$(ob keys -b "$B" -T 2>&1)"
  check_match "keys ü: -t named for a character outside the layout" "no key for 'ü' in this layout \(omabox keys -t 'ü' types" "$(ob keys -b "$B" ü 2>&1)"
  check_match "click off the screen: said" "click: 99999,99999 is off the 1920x1080 screen" "$(ob click -b "$B" 99999 99999 2>&1)"
  check_match "...pointer and drag too" "pointer: 1920,5 is off.*drag: 5,5000 is off" "$(ob pointer -b "$B" -- move 1920 5 2>&1; ob drag -b "$B" 5 5 5 5000 2>&1)"
  check_match "shot -g off the screen: grim's reason" "shot: grim failed: .*did not intersect" "$(ob shot -b "$B" -g "5000,5000 10x10" 2>&1)"
  # #111, finding 180: shot FILE as an argument, events --grep -i, mode host on a box (then back).
  ob shot -b "$B" "$TMP/positional.png" >/dev/null 2>&1
  check_match "shot FILE (an argument, not -o)" "PNG image data, 1920 x 1080" "$(file "$TMP/positional.png")"
  check_match "events --grep -i: any case" "openwindow>>" "$(ob events -b "$B" --grep '^OPENWINDOW>>' -i)"
  check_eq "...without -i, the case as given" "" "$(ob events -b "$B" --grep '^OPENWINDOW>>')"
  ob mode -b "$B" host >/dev/null 2>&1
  check_eq "mode host: the box takes your focused monitor's mode" "$(lib parse_mode host)" "$(ob mode -b "$B" | awk '{print $2}')"
  ob mode -b "$B" 1920x1080 >/dev/null 2>&1
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
  check_match "log --grep -i RE, as grep users type it (#130)" "^DEBUG \]: Creating the " "$(ob log -b "$B" --grep -i 'creating the' -n 1)"
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

UNIT=(t_unit_lock_markers t_unit_box_gone t_unit_monitors t_unit_agent_session t_unit_clip t_unit_keys_to_box t_unit_shot_hidden t_unit_config t_unit_bar_filter t_unit_wait t_unit_pixel t_unit_gpu t_unit_up_dies_late t_unit_settle_read t_unit_pointer t_unit_window_select t_unit_guard_exec_host t_unit_live_edit t_unit_parse_mode t_unit_duration t_unit_mount_rules t_unit_refusals t_unit_run_named_dead t_unit_kill_box t_unit_cli t_unit_uwsm_guard t_unit_install t_unit_host_session t_unit_guard_settings t_unit_seed_copy t_unit_theme_dir t_unit_plugin_link t_unit_version t_unit_omarchy_contract t_unit_saves
  t_unit_nvidia t_unit_aquamarine t_unit_setup t_unit_no_theme t_unit_hyprland t_unit_registry t_unit_leak_scan t_unit_evidence t_unit_jail_policy t_unit_relay t_unit_broker_units t_unit_inspect t_unit_parallel t_unit_shell_crash t_unit_which)
BOX=(t_leak_control t_run_options t_main t_window t_keys_to_box t_pointer t_pixel t_burst t_changed t_output t_output_nvidia t_gdb t_wait t_replace t_dbus_user_app t_agent_session t_mode_lock t_new t_keys t_peek t_peek_monitors t_guard t_uwsm_app t_widget t_widget_list t_monitors t_monitors_nvidia t_monitors_window t_monitors_wait t_monitors_wait_nvidia t_throwaway t_throwaway_home t_throwaway_killed t_throwaway_dead t_isolated t_connected t_ports t_isolated_no_pidfile t_up_killed t_idle t_reap_race t_run_idle t_stock_bar t_saves
  t_clip t_systemd t_omarchy_restart t_own_processes t_config_kept t_autoreload t_held_keys t_up_again t_theme_dir t_theme t_plugin_check t_plugin_hosted t_plugin_link t_submap_release t_setup_prompts t_omarchy_tree t_lock t_hostile t_race t_failed_up t_hung t_shell_crash t_up_aborted t_hyprland_dies t_pasta_dies t_other_userns t_no_new_privs t_no_shell t_hyprland t_no_git_identity t_stale_pid t_jail t_inspect t_which)

# Box tests run in parallel (-j N; issue #60): each in a subshell of its own, its output shown whole
# when it ends. By default half the CPUs, at most one per 2 GB available and 8 (on 16 CPUs: 8, a full
# run in ~1.5 min; -j 1: ~8.5 min). No test ran slower beside 7 others than alone. These run alone first: none so far (a test that cannot share the machine,
# with a reason, goes here). In parallel, the slowest start first, so the run ends with short ones.
SERIAL=()
default_jobs() {
  local n m; n=$(($(nproc) / 2)) m=$(awk '/^MemAvailable:/ {print int($2 / 2097152)}' /proc/meminfo)
  [ "$m" -ge "$n" ] || n=$m; [ "$n" -le 8 ] || n=8; [ "$n" -ge 1 ] || n=1; echo "$n"
}
SLOW=(t_agent_session t_widget t_widget_list t_run_idle t_guard t_peek t_peek_monitors t_reap_race t_wait t_pointer t_clip t_replace t_idle)

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
  # The suite's own functions as they are now. These tests run in this shell, so a helper of a test's
  # under a name the suite uses (note, check, ...) stays redefined for every test after it, the parallel
  # ones forked later too (#189, finding 252: t_unit_theme_dir's note() failed every slow wait after it).
  # Each such test fails, and the suite's are put back.
  local _t0 _fns _fdefs _f _was _chg
  mapfile -t _fns < <(compgen -A function)
  _fdefs=$(declare -f "${_fns[@]}")
  for CUR in "${_one[@]}"; do
    _n=$((pass + fail + ${#skips[@]})) _t0=$SECONDS
    rm -f "$UNTIL"
    printf '%s == %s\n' "$(date +%s.%3N)" "$CUR" >> "$EVID/host-events.log"
    echo "${CUR#t_}"; HELD=""; "$CUR"; notes
    if [ "$(declare -f "${_fns[@]}")" != "$_fdefs" ]; then
      _was=() _chg=()
      for _f in "${_fns[@]}"; do _was+=("$(declare -f "$_f")"); done
      eval "$_fdefs"
      for _f in "${!_fns[@]}"; do [ "${_was[_f]}" = "$(declare -f "${_fns[_f]}")" ] || _chg+=("${_fns[_f]}"); done
      no "the test leaves the suite's functions as they were" "it redefined ${_chg[*]} (put back for the tests after it)"
    fi
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
