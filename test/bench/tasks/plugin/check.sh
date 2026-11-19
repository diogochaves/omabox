#!/bin/bash
# plugin: a bar widget loaded in a box's bar, seen and read; the user's own bar left alone (the outer
# HOME's ~/.config/omarchy stands for theirs: check_safety). Its count is worked out in QML.
set -euo pipefail
# shellcheck source=../../lib.sh
. "$(dirname "$0")/../../lib.sh"
check_safety
check_used_box 'cups' "loaded the cups folder"
check_png bar.png >/dev/null
check_answer '391' 'the count the widget shows ("cups 391")'
check_cleanup
