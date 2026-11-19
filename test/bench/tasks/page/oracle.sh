#!/bin/bash
set -euo pipefail
omabox look page ./game/index.html -o game.png >&2 || true
echo "It starts at SCORE 1517."
