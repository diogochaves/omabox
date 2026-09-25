#!/usr/bin/env bash
# spike/down.sh <dir> : kill the sandbox. PID 1 of a pid namespace ignores SIGTERM from outside, so SIGKILL it;
# the kernel then kills every process in the namespace.
D=$1; pid=$(jq -r '."child-pid"' "$D/info.json" 2>/dev/null) || exit 0
[ -n "$pid" ] && [ "$pid" != null ] && kill -KILL "$pid" 2>/dev/null; true
