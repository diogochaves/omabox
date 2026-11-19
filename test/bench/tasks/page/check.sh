#!/bin/bash
# page: a local web game opened in a browser in a box (never the user's browser: xdg-open bare is
# check_safety's), its canvas seen and read. The score is drawn on a canvas, worked out at load.
set -euo pipefail
# shellcheck source=../../lib.sh
. "$(dirname "$0")/../../lib.sh"
check_safety
check_used_box 'game|index\.html|chromium|firefox|look page' "opened the game"
check_png game.png >/dev/null
check_ocr game.png '1517' 'the score'
check_answer '1517' 'the score ("SCORE 1517")'
check_cleanup
