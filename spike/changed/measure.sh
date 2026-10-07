#!/usr/bin/env bash
# Phase 1 of #145 (finding 243): how much smaller a crop to what changed is than the shot an agent
# takes now. Two frames of a case (cases.sh), compared offline.
#   measure.sh NAME BEFORE.png AFTER.png [MARGIN]   one TSV line: case, full size and tokens, the
#                                                   changed box, the crop with its margin (16) and
#                                                   tokens, its share of the area; then the same as
#                                                   separate crops (changes over 2 x MARGIN apart):
#                                                   how many, their tokens summed
#   measure.sh --all DIR                            every NAME-0.png / NAME-1.png pair in DIR
# Image tokens: Claude's estimate, (w x h) / 750, after the image is scaled to at most 1568 px a side
# and about 1.15 megapixels (what an over-sized shot costs is its scaled size's).
set -euo pipefail

tokens() {
  awk -v w="$1" -v h="$2" 'BEGIN {
    s = 1; l = (w > h ? w : h)
    if (l > 1568) s = 1568 / l
    if (w * s * h * s > 1150000) s = sqrt(1150000 / (w * h))
    printf "%d", int(w * s) * int(h * s) / 750 + 0.999 }'
}
# The difference of two frames, black and white on stdout (white: changed), the pointer left out: a
# 64x64 box at each frame's pointer (BEFORE.cursor / AFTER.cursor, "X Y", for screen shots), as
# wait's cursor_rect.
diff_png() {
  local draw=() c x y
  for c in "${1%.png}.cursor" "${2%.png}.cursor"; do
    [ -s "$c" ] || continue
    read -r x y < "$c"
    draw+=(-draw "rectangle $((x - 16)),$((y - 16)) $((x + 47)),$((y + 47))")
  done
  magick "$1" "$2" -fill black "${draw[@]}" -compose difference -composite -alpha off -colorspace gray -threshold "${FUZZ:-0}%" png:-
}

if [ "$1" = --all ]; then
  for f in "$2"/*-0.png; do n=${f##*/}; n=${n%-0.png}; [ ! -f "$2/$n-1.png" ] || "$0" "$n" "$f" "$2/$n-1.png" "${3:-16}"; done
  exit
fi
name=$1 a=$2 b=$3 m=${4:-16}
read -r iw ih < <(magick identify -format '%w %h\n' "$b")
# A black border first: %@ trims the colour of the corners, which may have changed themselves.
g=$(diff_png "$a" "$b" | magick png:- -bordercolor black -border 1 -format '%@' info: 2>/dev/null) || g=""
if [[ $g =~ ^([0-9]+)x([0-9]+)\+([0-9]+)\+([0-9]+)$ ]] && [ "${BASH_REMATCH[1]}" != 0 ]; then
  dw=${BASH_REMATCH[1]} dh=${BASH_REMATCH[2]} dx=$((BASH_REMATCH[3] - 1)) dy=$((BASH_REMATCH[4] - 1))
  x0=$((dx - m < 0 ? 0 : dx - m)) y0=$((dy - m < 0 ? 0 : dy - m))
  x1=$((dx + dw + m > iw ? iw : dx + dw + m)) y1=$((dy + dh + m > ih ? ih : dy + dh + m))
  cw=$((x1 - x0)) ch=$((y1 - y0))
  # Separate crops: the difference grown by the margin (changes 2 x MARGIN apart join), each
  # connected part's box.
  n=0 sum=0
  while read -r bw bh p; do n=$((n + 1)) sum=$((sum + $(tokens "$bw" "$bh"))); [ -z "${PARTS:-}" ] || echo "  part $p" >&2; done < <(
    diff_png "$a" "$b" | magick png:- -morphology Dilate "Rectangle:$((2 * m + 1))x$((2 * m + 1))" \
      -define connected-components:verbose=true -connected-components 8 null: 2>/dev/null |
      awk 'NR > 1 && $NF == "gray(255)" { split($2, g, /[x+]/); print g[1], g[2], $2 }')
  printf '%s\t%sx%s\t%s\t%sx%s+%s+%s\t%sx%s\t%s\t%.1f%%\t%s parts\t%s\n' "$name" "$iw" "$ih" "$(tokens "$iw" "$ih")" \
    "$dw" "$dh" "$dx" "$dy" "$cw" "$ch" "$(tokens "$cw" "$ch")" \
    "$(awk -v a=$((cw * ch)) -v b=$((iw * ih)) 'BEGIN { print 100 * a / b }')" "$n" "$sum"
else
  printf '%s\t%sx%s\t%s\tsame\t-\t0\t0%%\t0 parts\t0\n' "$name" "$iw" "$ih" "$(tokens "$iw" "$ih")"
fi
