#!/usr/bin/env bash
# A Quickshell crash dialog opened in the box (hyprland.lua runs this; #126 and #303, NOTES findings 183
# and 408): closed when the crash was the box shell's, whose dialog nobody reads and which took the keys
# meant for the app under test; left alone when it was another instance's, a project's own Quickshell
# whose crash handling may be the very thing under test. The reporter writes the crash's report before
# it shows the dialog; the report says the crashed instance's config ("Config Path: DIR/shell.qml", the
# box shell's DIR being $OMARCHY_PATH/shell) and the pid in the reporter's __QUICKSHELL_CRASH_DUMP_PID
# ("Crashed process ID", the child the handler forked to dump). No such report (none written): the
# dialog is the shell's when it has a plain box's shell's slot (#438), or when the reporter's parent,
# the crashed process, is the box shell. A plain box's shell (`up --shell CMD`, #247) has no Omarchy
# tree: its configs are the ones omabox noted in omabox-shell.configs once it drew (a folder a line).
# crash-dialog.sh PID DUMP: PID the dialog's process, DUMP its __QUICKSHELL_CRASH_DUMP_PID.
set -uo pipefail
pid=${1:-} dump=${2:-}
[[ $pid =~ ^[0-9]+$ && $dump =~ ^[0-9]+$ ]] || exit 0
dir=${XDG_CACHE_HOME:-$HOME/.cache}/quickshell/crashes
want=$(printf '%s' "${OMARCHY_PATH:-/usr/share/omarchy}/shell" | tr -s /); want=${want%/}
ours=""
# A shell in the slot: a plain box's `--shell CMD`, or a desk box's (#321, finding 703), the command
# its config started in omabox-shell.cmd.
slotted=0
if [ -n "${OMABOX_SHELL_CMD:-}" ] || { [ "${OMABOX_DESKTOP:-}" = desk ] && [ -s "$XDG_RUNTIME_DIR/omabox-shell.cmd" ]; }; then
  slotted=1
fi
# (A moment for the report, should the dialog come first.)
for _ in 1 2 3 4 5 6 7 8 9 10; do
  for r in "$dir"/*/report.txt; do
    grep -aqx "Crashed process ID: $dump" "$r" 2>/dev/null || continue
    cfg=$(sed -n '/^Config Path: /{s///p;q}' "$r" | tr -s /)
    if [ "$slotted" = 1 ]; then
      ours=0; grep -qxF -- "${cfg%/*}" "$XDG_RUNTIME_DIR/omabox-shell.configs" 2>/dev/null && ours=1
    elif [ "${cfg%/*}" = "$want" ]; then ours=1; else ours=0; fi
    break 2
  done
  sleep 0.1
done
# A plain box's shell's dialog (#438): forked from the crashed shell, it has its slot.
slot=$(cat "$XDG_RUNTIME_DIR/omabox-shell.slot" 2>/dev/null) || slot=""
if [ -z "$ours" ] && [ "$slotted" = 1 ] && [[ $slot =~ ^[A-Za-z0-9._-]+$ ]] &&
  grep -qsFxz -e "OMABOX_SHELL_SLOT=$slot" "/proc/$pid/environ"; then
  ours=1
fi
if [ -z "$ours" ]; then
  ppid=$(sed -n 's/^PPid:[[:space:]]*//p' "/proc/$pid/status" 2>/dev/null)
  shell=$(cat "$XDG_RUNTIME_DIR/omabox-shell.pid" 2>/dev/null)
  [ -n "$ppid" ] && [ "$ppid" = "$shell" ] && ours=1
fi
[ "$ours" = 1 ] || exit 0
kill "$pid" 2>/dev/null
echo "$(date '+%F %T.%3N') omabox: closed the shell's crash dialog (pid $pid); the report is in ~/.cache/quickshell/crashes" >> "$HOME/shell.log"
