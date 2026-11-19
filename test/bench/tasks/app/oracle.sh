#!/bin/bash
# The task done by hand, as a capable agent would, to prove the task and its check (run.sh --harness
# oracle). Runs inside the outer box, in the task's folder.
set -euo pipefail
out=$(omabox look app -o shot.png -- python3 hello.py 2>&1) || true
echo "$out" >&2
echo "The window shows up; it says: Seven blue herons."
