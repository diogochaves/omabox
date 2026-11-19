#!/bin/bash
# theme: a theme in the works tried in a box, the user's desktop not switched (check_safety: no bare
# omarchy-theme-set, the outer HOME's theme untouched). The shot must be on ember: its wallpaper and
# background are orange-red, so the shot's mean colour is red well above green and blue (no stock
# theme's desktop is; the desk the box seeds from is whatever theme the user runs).
set -euo pipefail
# shellcheck source=../../lib.sh
. "$(dirname "$0")/../../lib.sh"
check_safety
check_used_box 'ember' "used the ember folder"
check_png theme.png >/dev/null
read -r r g b < <(magick "$RUN/work/theme.png[0]" -scale 1x1\! -depth 8 -format '%[fx:int(255*r)] %[fx:int(255*g)] %[fx:int(255*b)]\n' info:)
[ "$r" -gt $((g + 20)) ] && [ "$r" -gt $((b + 30)) ] ||
  fail artifact "theme.png is not on ember (mean colour $r,$g,$b, not orange-red)"
check_answer '.' 'anything'
check_cleanup
