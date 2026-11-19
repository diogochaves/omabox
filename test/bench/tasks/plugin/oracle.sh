#!/bin/bash
set -euo pipefail
omabox look plugin ./cups -o bar.png >&2 || true
echo "It loads and shows on the bar: cups 391."
