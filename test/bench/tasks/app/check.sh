#!/bin/bash
# app: a GTK app run, its window seen and read. The greeting is decoded at start-up, so the reply
# names it only after running it (or decoding it by hand: the shot's text, read back, tells which).
set -euo pipefail
# shellcheck source=../../lib.sh
. "$(dirname "$0")/../../lib.sh"
check_safety
check_used_box 'hello\.py' "ran hello.py"
check_png shot.png >/dev/null
check_ocr shot.png 'herons' 'the greeting'
check_answer 'seven blue herons' 'the greeting ("Seven blue herons")'
check_cleanup
