#!/usr/bin/env bash
# Spike #144: what `shot --text` would give: tesseract on a window shot (a file; runs on the host,
# it reads a PNG and touches no desktop). Prints the text, then a stats line on stderr.
#   ocr.sh SHOT.png [--words]     --words: one line per word with its box (what `click --text` needs)
# Dark themes read badly as they are: the shot is scaled 2x, greyed and negated first (ImageMagick).
set -euo pipefail
png=${1:?usage: ocr.sh SHOT.png [--words]}
mode=${2:-}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
start=$(date +%s%N)
magick "$png" -resize 200% -colorspace Gray -negate "$tmp/in.png"
if [ "$mode" = --words ]; then
  # TSV: level ... left top width height conf text; keep words (level 5) with some confidence and
  # map the 2x boxes back to the shot's pixels.
  tesseract "$tmp/in.png" - --psm 11 tsv 2>/dev/null |
    awk -F'\t' 'NR > 1 && $1 == 5 && $11 > 30 && $12 ~ /[[:alnum:]]/ {
      printf "%s @%d,%d %dx%d\n", $12, $7/2, $8/2, $9/2, $10/2 }' >"$tmp/out.txt"
else
  tesseract "$tmp/in.png" - --psm 11 2>/dev/null | grep -v '^[[:space:]]*$' >"$tmp/out.txt" || true
fi
end=$(date +%s%N)
cat "$tmp/out.txt"
bytes=$(wc -c <"$tmp/out.txt")
wh=$(magick identify -format '%w %h' "$png")
w=${wh% *} h=${wh#* }
# An image's tokens: about w*h/750 after scaling its long side down to 1568 (the model's limit).
img=$(awk -v w="$w" -v h="$h" 'BEGIN { s = (w > h ? w : h); f = (s > 1568 ? 1568 / s : 1);
  printf "%d", (w * f) * (h * f) / 750 }')
echo "stats: shot=${w}x${h} shot_tokens~$img text_bytes=$bytes ~tokens=$((bytes / 3)) ms=$(((end - start) / 1000000))" >&2
