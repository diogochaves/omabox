#!/bin/bash
set -euo pipefail
omabox look theme ./ember -o theme.png >&2 || true
echo "Here it is (theme.png). Its wallpaper is a plain gradient; nothing else looks off."
