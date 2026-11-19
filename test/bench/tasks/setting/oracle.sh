#!/bin/bash
set -euo pipefail
omabox up >&2
omabox lua 'hl.config({ general = { gaps_out = 40 } })' >&2
omabox run -d --wait -- foot >&2
omabox hyprctl getoption general:gaps_out >&2
omabox shot -o gaps.png >&2
omabox down >&2
echo "Set gaps_out to 40 in a box: getoption reads 40 40 40 40, and the terminal sits 40 px in."
