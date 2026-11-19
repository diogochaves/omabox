#!/bin/bash
# setting: a Hyprland option tried in a box, read back there, the user's config untouched (the outer
# HOME's ~/.config/hypr: check_safety).
set -euo pipefail
# shellcheck source=../../lib.sh
. "$(dirname "$0")/../../lib.sh"
check_safety
check_used_box 'gaps' "set the gaps"
check_used_box 'getoption|windows|(lua|eval).*(return|get)|looknfeel' "read the setting back" verify
check_png gaps.png >/dev/null
check_answer '40' 'the gap it checked (40)'
check_cleanup
