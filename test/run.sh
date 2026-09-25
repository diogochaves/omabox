#!/usr/bin/env bash
# shellcheck disable=SC2016 # single-quoted $VARS are expanded inside the box
# omabox's regression suite: what NOTES' findings verified, as checks that run in real boxes.
#
#   test/run.sh              every test (~2-3 min; boxes are named t<pid>-*, all torn down)
#   test/run.sh unit         only the fast ones (no box)
#   test/run.sh PATTERN...   tests whose name matches any PATTERN (e.g. isolated systemd)
#
# Never touches the real desktop: every box is headless, and the last test checks that the host's
# focused workspace and window are what they were before. Needs a Hyprland session (read-only hyprctl)
# and the host ports 8093/8094 free (a throwaway HTTP server for the network tests).
set -uo pipefail
# The guard tests point HOME at a temp dir; these would still lead them to the real settings.
unset CLAUDE_CONFIG_DIR CODEX_HOME

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CLI=$ROOT/bin/omabox
P=t$$                      # box name prefix
TMP=$(mktemp -d)
pass=0 fail=0 failed=()

cleanup() {
  local b
  for b in $("$CLI" ls --json 2>/dev/null | jq -r '.[].name' | grep "^$P-"); do "$CLI" down "$b" >/dev/null 2>&1; done
  [ -n "${HTTP_PID:-}" ] && kill "$HTTP_PID" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

ob() { "$CLI" "$@"; }
ok() { pass=$((pass + 1)); printf '  \e[32mok\e[0m   %s\n' "$1"; }
no() { fail=$((fail + 1)); failed+=("$CUR: $1"); printf '  \e[31mFAIL\e[0m %s\n' "$1"; [ -n "${2:-}" ] && printf '       %s\n' "${2:0:300}"; }
# check NAME CMD...: the command must succeed. check_eq NAME WANT GOT. check_fails NAME CMD...
check() { local n=$1; shift; local out; if out=$("$@" 2>&1); then ok "$n"; else no "$n" "$out"; fi; }
check_eq() { if [ "$2" = "$3" ]; then ok "$1"; else no "$1" "want [$2] got [$3]"; fi; }
check_fails() { local n=$1; shift; local out; if out=$("$@" 2>&1); then no "$n" "succeeded: $out"; else ok "$n"; fi; }
check_match() { if [[ $3 =~ $2 ]]; then ok "$1"; else no "$1" "want /$2/ got [$3]"; fi; }
# Poll a command until it succeeds (SECONDS timeout).
until_ok() { local t=$1; shift; local end=$((SECONDS + t)); until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || return 1; sleep 0.2; done; }
# A fresh git repo named $P-SUFFIX: a throwaway `run` from it gets a name no box of the user's has
# (from $ROOT it would be "omabox", which `omabox up` in the checkout also takes).
tmp_repo() { local r=$TMP/$P-$1; mkdir -p "$r" && git -C "$r" init -q && echo "$r"; }

# The CLI's functions without its dispatch at the end, for the pure ones (a file: it finds its ROOT
# from its own path).
mkdir -p "$TMP/lib/bin"
sed '/^main "\$@"; exit$/d' "$CLI" > "$TMP/lib/bin/omabox"
lib() { bash -c 'source "$1"; shift; "$@"' lib "$TMP/lib/bin/omabox" "$@"; }

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
  (OMABOX_T66=$secret ob run -b "$B" --pass OMABOX_T66 -- sleep 2 >/dev/null 2>&1 &)
  sleep 1
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
  [ -f /usr/share/omarchy/applications/foot.desktop ] &&
    check "foot's desktop entry passes --app-id" grep -q '^X-TerminalArgAppId=' "$D/home/.local/share/applications/foot.desktop"
  [ -f "$HOME/.config/foot/foot.ini" ] && check "the user's foot config (theme colours)" test -f "$D/home/.config/foot/foot.ini"
  # ...on top of the stock HOME (/etc/skel), never the user's secrets, and Omarchy's toggles apply
  [ -d /etc/skel/.config/omarchy ] && check "the box HOME starts from /etc/skel" test -f "$D/home/.local/state/omarchy/toggles/hypr/flags.lua"
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
  sleep 0.5
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
  exec {lock}>&-; sleep 3
  check_eq "...and leaves the new box alone" up "$(ob ls --json | jq -r --arg n "$B" '.[] | select(.name == $n) | .state')"
  ob down "$B" >/dev/null 2>&1
  B=$P-rc
  ob up "$B" --idle 20s --no-shell >/dev/null 2>&1 || { no "up" "failed"; return; }
  ob run -b "$B" -- sh -c 'echo 1 > "$XDG_RUNTIME_DIR/omabox.closed"; hyprctl dispatch "hl.dsp.exit()"' >/dev/null 2>&1
  sleep 12   # two polls
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
  sleep 1
  check_eq "host scope gone" "" "$(systemctl --user list-units --no-legend "omabox-$B-*")"
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
  sleep 0.5
  check_eq "nothing of it left running" 0 "$(pgrep -fc "$XDG_RUNTIME_DIR/omabox/$B/" || true)"
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
  sleep 2
  check_fails "no quickshell running" ob run -b "$B" -- pgrep -x quickshell
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
  sleep 0.5
  check_eq "nothing of it left running" 0 "$(pgrep -fc "$XDG_RUNTIME_DIR/omabox/$B/" || true)"
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
  fi
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
  printf '#!/bin/sh\ncase "$1" in\n  ls) cat "$HOME/list.json" ;;\n  shot) echo "$*" >> "$HOME/actions"; echo "$HOME/x.png" ;;\n  config) echo "$*" >> "$HOME/actions"; cat "$HOME/config.json" 2>/dev/null || echo "{}" ;;\n  up) echo "$*" >> "$HOME/actions"; echo box-9 ;;\n  *) echo "$*" >> "$HOME/actions" ;;\nesac\n' > "$H/.local/bin/omabox"
  printf '#!/bin/sh\necho "xdg-open $*" >> "$HOME/actions"; exec sleep 300\n' > "$H/.local/bin/xdg-open"
  printf '#!/bin/sh\necho "notify-send $*" >> "$HOME/actions"\n' > "$H/.local/bin/notify-send"
  chmod +x "$H/.local/bin/omabox" "$H/.local/bin/xdg-open" "$H/.local/bin/notify-send"
  local row='{"mode":"headless","size":"1920x1080@60","created":"2026-09-24T02:00:00-03:00","plugins":[],"net":"host","state":"up","peeking":false}'
  jq -n "[$row + {name: \"a\"}, $row + {name: \"b\"}]" > "$H/list.json"; : > "$H/actions"
  # shellcheck disable=SC2329 # called through until_ok and check_fails
  panel() { ob hyprctl -b "$B" -j layers | grep -q omarchy-keyboard-panel; }
  sleep 6   # a poll with the list
  ob run -b "$B" -- omarchy-shell chaves.omabox open
  check "the panel opens" until_ok 3 panel
  check "...and reads the settings from the CLI (finding 70)" until_ok 3 grep -qx "config --json" "$H/actions"
  ob keys -b "$B" Down Down >/dev/null
  jq "[$row + {name: \"0new\"}] + ." "$H/list.json" > "$H/l" && mv "$H/l" "$H/list.json"
  sleep 3   # a poll (2 s while open) puts 0new above b
  ob keys -b "$B" s >/dev/null
  check "the selection follows its box (shot b, not a)" until_ok 3 grep -qx "shot -b b" "$H/actions"
  check "the viewer is started" until_ok 3 grep -q "^xdg-open $H" <(sed "s|/home/sbx|$H|" "$H/actions")
  ob run -b "$B" -- omarchy-shell chaves.omabox open; sleep 1
  ob keys -b "$B" Down p >/dev/null
  check "an action runs while the viewer is open" until_ok 3 grep -q "^peek -b " "$H/actions"
  # n: a new interactive box under the name the CLI picks, then brought forward (finding 73)
  ob run -b "$B" -- omarchy-shell chaves.omabox open; sleep 1
  ob keys -b "$B" n >/dev/null; sleep 1
  check_fails "one n starts nothing (a stray key; finding 74)" grep -qx "up --interactive --new" "$H/actions"
  ob keys -b "$B" n >/dev/null
  check "n n starts a new interactive box (up --interactive --new)" until_ok 3 grep -qx "up --interactive --new" "$H/actions"
  check "...and shows the box it printed" until_ok 3 grep -qx "peek -b box-9 --focus" "$H/actions"
  # bar-icon (finding 72), read again whenever the settings file changes. always (the default):
  # the panel opens with no boxes, for its settings; auto: opened with none, it stays shut, and
  # does not pop up later when a box comes
  echo '[]' > "$H/list.json"; sleep 6
  ob run -b "$B" -- omarchy-shell chaves.omabox open
  check "bar-icon always (default): the panel opens with no boxes" until_ok 3 panel
  ob keys -b "$B" Escape >/dev/null
  echo '{"bar-icon":"auto"}' > "$H/config.json"; echo "bar-icon=auto" > "$H/.config/omabox/config"
  sleep 2
  ob run -b "$B" -- omarchy-shell chaves.omabox open; sleep 1
  check_fails "bar-icon auto: opened with no boxes, it stays shut" panel
  jq -n "[$row + {name: \"a\"}]" > "$H/list.json"; sleep 6
  check_fails "...and does not pop up when one comes" panel
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
  # shellcheck disable=SC2329 # called through until_ok
  gone() { [ "$(state "$1")" = gone ]; }
  "${in[@]}" "$CLI" up cc --interactive --no-shell --confirm-close >/dev/null 2>&1
  "${cl[@]}" >/dev/null; sleep 2
  check_eq "confirm-close: the box stays after a close" up "$(state cc)"
  check_eq "...with a new window" 1 "$(ob hyprctl -b "$B" -j clients | jq '[.[] | select(.class == "aquamarine")] | length')"
  "${cl[@]}" >/dev/null
  check "...and a second close ends it, cleared: no dead box left (finding 71)" until_ok 8 gone cc
  "${in[@]}" "$CLI" up lv --interactive --no-shell >/dev/null 2>&1
  "${in[@]}" "$CLI" config confirm-close on >/dev/null
  "${cl[@]}" >/dev/null; sleep 2
  check_eq "config confirm-close on reaches a running box" up "$(state lv)"
  "${in[@]}" "$CLI" config confirm-close off >/dev/null
  "${cl[@]}" >/dev/null
  check "...and off again: one close ends it, cleared" until_ok 8 gone lv
  # A box that dies without being closed stays dead, logs kept, until `down`
  "${in[@]}" "$CLI" up cr --interactive --no-shell >/dev/null 2>&1
  "${in[@]}" "$CLI" run -b cr -- pkill -KILL -x Hyprland >/dev/null 2>&1; sleep 5
  check_eq "an interactive box that crashed stays dead" dead "$(state cr)"
  "${in[@]}" "$CLI" down cr >/dev/null 2>&1
  check "up --size host under the guard" "${in[@]}" "$CLI" up inner2 --size host --no-shell --idle 0
  check_eq "...the stand-in's monitor" "$(ob mode -b "$B")" "$("${in[@]}" "$CLI" mode -b inner2)"
  check "peek under the guard" "${in[@]}" "$CLI" peek inner2
  check "...its window appears" until_ok 10 bash -c "'$CLI' hyprctl -b '$B' -j clients | jq -e '.[] | select(.class == \"omabox-peek\") | select(.workspace.name == \"9\")'"
  "${in[@]}" "$CLI" down --all >/dev/null 2>&1
  ob down "$B" >/dev/null
}

# --- runner --------------------------------------------------------------------------------------

UNIT=(t_unit_config t_unit_guard_exec_host t_unit_live_edit t_unit_parse_mode t_unit_duration t_unit_mount_rules t_unit_refusals t_unit_run_named_dead t_unit_cli t_unit_uwsm_guard t_unit_install t_unit_host_session t_unit_guard_settings t_unit_seed_copy t_unit_version)
BOX=(t_main t_new t_keys t_peek t_guard t_uwsm_app t_widget t_throwaway t_throwaway_home t_throwaway_killed t_throwaway_dead t_isolated t_isolated_no_pidfile t_idle t_reap_race t_run_idle t_stock_bar
  t_systemd t_hostile t_race t_failed_up t_hyprland_dies t_no_shell t_stale_pid)

# The host's session through the CLI's own lookup, so the suite runs from a guarded shell too.
hostctl() { lib host_hyprctl "$@"; }
hostctl -j version >/dev/null 2>&1 || { echo "run from the Hyprland session (read-only hyprctl)"; exit 2; }
ws0=$(hostctl -j activeworkspace | jq .id) win0=$(hostctl -j activewindow | jq -r '.address // ""')

tests=("${UNIT[@]}" "${BOX[@]}")
if [ "${1:-}" = unit ]; then tests=("${UNIT[@]}")
elif [ $# -gt 0 ]; then
  sel=(); for t in "${tests[@]}"; do for p in "$@"; do [[ $t == *"$p"* ]] && { sel+=("$t"); break; }; done; done
  tests=("${sel[@]}")
fi

for CUR in "${tests[@]}"; do
  echo "${CUR#t_}"
  "$CUR"
done

echo "host"
CUR=host
check_eq "host workspace untouched" "$ws0" "$(hostctl -j activeworkspace | jq .id)"
check_eq "host focused window untouched (fails if you switched windows meanwhile)" "$win0" "$(hostctl -j activewindow | jq -r '.address // ""')"

echo
echo "$pass passed, $fail failed"
for f in "${failed[@]}"; do echo "  FAIL $f"; done
[ $fail = 0 ]
