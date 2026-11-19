#!/bin/bash
# The benchmark (#464): how small local models drive omabox. Each run is one task done by one model
# through one agent harness with one skill folder, inside an omabox box of its own (README.md).
set -euo pipefail

usage() {
  cat <<'EOF'
usage: test/bench/run.sh [--model ID]... [--harness pi|opencode|oracle]... [--skill [NAME=]DIR]...
                         [--task NAME]... [--runs N] [--limit MINUTES] [--no-guard]
                         [--no-omarchy-skills] [--keep-model] [--out DIR] [--list]

Every combination of the given tasks, models, harnesses and skills, N times each (each flag can be
repeated; defaults: every task, qwen3.8-27b, pi, this checkout's skill/ as "current", 1 run, 20
minutes a run). --harness oracle runs each task's oracle.sh instead of a model (proves the task).
--list prints the runs and exits. Results: OUT/results.tsv, one folder per run beside it
(default OUT: ~/.local/state/omabox/bench/STAMP).
EOF
}

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
PORT=${LOCAL_MODEL_PORT:-8095}
# Every box of the bench (the agent's and each it starts) draws on this render node: BENCH_RENDER_NODE,
# else the first whose driver is not nvidia (the iGPU), never the card the model fills. A box there
# competes with the model for VRAM: shells "not ready after 30s" in a suite run beside the bench.
RENDER=${BENCH_RENDER_NODE:-}
if [ -z "$RENDER" ]; then
  for r in /dev/dri/renderD*; do
    [ -e "$r" ] || continue
    drv=$(readlink -f "/sys/class/drm/${r##*/}/device/driver" 2>/dev/null) || drv=""
    [ "${drv##*/}" = nvidia ] || { RENDER=$r; break; }
  done
fi
models=() harnesses=() skills=() tasks=() runs=1 limit=20 guard=1 omarchy_skills=1 keep_model=0
out="" list=0

need() { [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "run.sh: $1 needs a value" >&2; exit 2; }; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --model) need "$@"; models+=("$2"); shift 2 ;;
    --harness)
      need "$@"
      case "$2" in pi | opencode | oracle) harnesses+=("$2") ;; *) echo "run.sh: unknown harness: $2 (pi, opencode, oracle)" >&2; exit 2 ;; esac
      shift 2 ;;
    --skill) need "$@"; skills+=("$2"); shift 2 ;;
    --task) need "$@"; tasks+=("$2"); shift 2 ;;
    --runs) need "$@"; runs=$2; shift 2 ;;
    --limit) need "$@"; limit=$2; shift 2 ;;
    --out) need "$@"; out=$2; shift 2 ;;
    --no-guard) guard=0; shift ;;
    --no-omarchy-skills) omarchy_skills=0; shift ;;
    --keep-model) keep_model=1; shift ;;
    --list) list=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "run.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "run.sh: --runs takes a positive number, not $runs" >&2; exit 2; }
[[ "$limit" =~ ^[1-9][0-9]*$ ]] || { echo "run.sh: --limit takes minutes, not $limit" >&2; exit 2; }
[ "${#models[@]}" -gt 0 ] || models=(qwen3.8-27b)
[ "${#harnesses[@]}" -gt 0 ] || harnesses=(pi)
[ "${#skills[@]}" -gt 0 ] || skills=("current=$REPO/skill")
if [ "${#tasks[@]}" -eq 0 ]; then
  for d in "$HERE"/tasks/*/; do tasks+=("$(basename "$d")"); done
fi
for t in "${tasks[@]}"; do
  [ -f "$HERE/tasks/$t/prompt.md" ] && [ -x "$HERE/tasks/$t/check.sh" ] ||
    { echo "run.sh: no task $t (a folder in $HERE/tasks with prompt.md and check.sh)" >&2; exit 2; }
done
# A skill: NAME=DIR, or DIR (named after its folder). Each must hold a SKILL.md.
skill_names=() skill_dirs=()
for s in "${skills[@]}"; do
  if [[ "$s" == *=* ]]; then n=${s%%=*} d=${s#*=}; else d=$s n=$(basename "$s"); fi
  [ -f "$d/SKILL.md" ] || { echo "run.sh: no SKILL.md in $d" >&2; exit 2; }
  [[ "$n" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "run.sh: skill name '$n': letters, digits, . _ - only" >&2; exit 2; }
  skill_names+=("$n") skill_dirs+=("$(cd "$d" && pwd)")
done

combos=()
for t in "${tasks[@]}"; do for m in "${models[@]}"; do for h in "${harnesses[@]}"; do
  for i in "${!skill_names[@]}"; do for n in $(seq "$runs"); do combos+=("$t $m $h $i $n"); done; done
done; done; done
if [ "$list" = 1 ]; then
  for c in "${combos[@]}"; do read -r t m h i n <<<"$c"; printf '%s\t%s\t%s\t%s\t%s\n' "$t" "$m" "$h" "${skill_names[$i]}" "$n"; done
  exit 0
fi

# The omabox under test: the one on PATH, and the checkout it runs from (mounted read-only in each
# outer box at its own path, so the agent's omabox is the same binary with its share/ and tools/).
CLI=$(readlink -f "$(command -v omabox)") || { echo "run.sh: no omabox on PATH" >&2; exit 2; }
CLI_REPO=$(cd "$(dirname "$CLI")/.." && pwd)
for c in jq magick; do command -v "$c" >/dev/null || { echo "run.sh: needs $c" >&2; exit 2; }; done
for h in "${harnesses[@]}"; do
  [ "$h" = oracle ] || command -v "$h" >/dev/null || { echo "run.sh: no $h on PATH" >&2; exit 2; }
done

stamp=$(date +%Y%m%d-%H%M%S)
[ -n "$out" ] || out=${XDG_STATE_HOME:-$HOME/.local/state}/omabox/bench/$stamp
mkdir -p "$out"
results=$out/results.tsv
printf 'task\tmodel\tharness\tskill\trun\tresult\ttools\tturns\tseconds\tstep\twhy\n' > "$results"
{
  echo "omabox: $("$CLI" --version 2>&1 | head -1) ($CLI)"
  echo "checkout: $(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo '?')"
  for i in "${!skill_names[@]}"; do echo "skill ${skill_names[$i]}: ${skill_dirs[$i]} ($(du -sb "${skill_dirs[$i]}" | cut -f1) bytes)"; done
  echo "guard: $guard; omarchy skills: $omarchy_skills; limit: ${limit} min; render node: ${RENDER:-the default}"
} > "$out/meta.txt"
echo "bench: $out (${#combos[@]} runs)" >&2
work_root=${XDG_CACHE_HOME:-$HOME/.cache}/omabox-bench/$stamp-$$

# make_repo DIR: DIR as a git repo with one commit (no signing, no identity of the user's).
make_repo() {
  git -C "$1" init -q
  git -C "$1" add -A
  git -C "$1" -c user.name=user -c user.email=user@example.invalid -c commit.gpgsign=false \
    commit -q --allow-empty -m start
}

started_model=0 current_model=""
cleanup() {
  [ -z "${box:-}" ] || "$CLI" down -b "$box" >/dev/null 2>&1 || true
  [ -z "${work_root:-}" ] || rm -rf "$work_root"
  if [ "$started_model" = 1 ] && [ "$keep_model" = 0 ] && command -v local-model >/dev/null; then
    local-model stop >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# use_model ID: the served model is ID (local-model starts it, stopping any other).
use_model() {
  [ "$1" != "$current_model" ] || return 0
  if command -v local-model >/dev/null; then
    local-model start "$1" >&2 || { echo "run.sh: local-model could not start $1" >&2; exit 1; }
    started_model=1
  fi
  curl -fsS -m 5 "http://127.0.0.1:$PORT/health" >/dev/null ||
    { echo "run.sh: nothing answers on 127.0.0.1:$PORT" >&2; exit 1; }
  current_model=$1
}

# seed_home H HARNESS SKILLDIR: what an agent's HOME holds in the box: the harness's `local` provider
# and nothing else of its config (no auth, no keys), the skill under test, Omarchy's own skills (as on
# every Omarchy machine), omabox on PATH, and the task's folder.
seed_home() {
  local h=$1 harness=$2 sk=$3 skills_dir
  mkdir -p "$h/.local/bin"
  ln -sfn "$CLI" "$h/.local/bin/omabox"
  case "$harness" in
    pi)
      mkdir -p "$h/.pi/agent"
      jq '{providers: {local: .providers.local}}' "$HOME/.pi/agent/models.json" > "$h/.pi/agent/models.json"
      skills_dir=$h/.pi/agent/skills ;;
    opencode)
      mkdir -p "$h/.config/opencode"
      jq '{"$schema": "https://opencode.ai/config.json", autoupdate: false, share: "disabled",
          permission: {"*": "allow"}, provider: {local: .provider.local}}' \
        "$HOME/.config/opencode/opencode.json" > "$h/.config/opencode/opencode.json"
      skills_dir=$h/.config/opencode/skills ;;
    oracle) skills_dir=$h/.agents/skills ;;
  esac
  mkdir -p "$skills_dir"
  cp -rL "$sk" "$skills_dir/omabox"
  if [ "$omarchy_skills" = 1 ] && [ -d /usr/share/omarchy/default/agents/skills ]; then
    for s in /usr/share/omarchy/default/agents/skills/*/; do
      [ "$(basename "$s")" = omabox ] || cp -rL "$s" "$skills_dir/"
    done
  fi
}

# digest DIR HARNESS: commands.txt (each shell command the agent ran, one a line; other tools as
# "TOOL PATH"), answer.txt (its last reply), and "TOOLS TURNS" on stdout.
digest() {
  local d=$1 j=$1/transcript.jsonl
  case "$2" in
    pi)
      jq -R -r 'fromjson? | select(.type == "tool_execution_start") |
        if .toolName == "bash" then .args.command else "\(.toolName) \(.args.path // .args.file_path // "")" end |
        gsub("\n"; " ; ")' "$j" > "$d/commands.txt" || true
      jq -R -s -r '[split("\n")[] | fromjson? | select(.type == "message_end" and .message.role == "assistant")] |
        last | (.message.content // []) | map(select(.type == "text") | .text) | join("\n")' "$j" > "$d/answer.txt" || true
      echo "$(wc -l < "$d/commands.txt") $(jq -R 'fromjson? | select(.type == "turn_start")' "$j" | grep -c '^{' || true)" ;;
    opencode)
      jq -R -r 'fromjson? | select(.type == "tool_use") | .part |
        if (.tool == "shell" or .tool == "bash") then .state.input.command
        else "\(.tool) \(.state.input.filePath // .state.input.path // .state.input.name // .state.input.id // "")" end |
        gsub("\n"; " ; ")' "$j" > "$d/commands.txt" || true
      jq -R -s -r '[split("\n")[] | fromjson?] as $e | ($e | map(.type) | indices("step_start") | last // 0) as $i |
        $e[$i:] | map(select(.type == "text") | .part.text) | join("\n")' "$j" > "$d/answer.txt" || true
      echo "$(wc -l < "$d/commands.txt") $(jq -R 'fromjson? | select(.type == "step_start")' "$j" | grep -c '^{' || true)" ;;
    oracle)
      grep -v -E '^\s*(#|$)' "$d/oracle.sh" > "$d/commands.txt" || true
      cp "$j" "$d/answer.txt"
      echo "$(wc -l < "$d/commands.txt") 1" ;;
  esac
}

k=0
for c in "${combos[@]}"; do
  read -r t m h i n <<<"$c"
  k=$((k + 1))
  sn=${skill_names[$i]} sd=${skill_dirs[$i]}
  rd=$out/$t.$m.$h.$sn.$n
  mkdir -p "$rd"
  echo "[$k/${#combos[@]}] $t $m $h $sn #$n" >&2
  [ "$h" = oracle ] || use_model "$m"
  box=bench-$$-$k
  # The task's folder: a git repo of its fixture, as a user's project is, outside any HOME a box
  # has (/home/sbx is both the outer box's and every box's started inside it), overlaid into the
  # outer box at its own path: writable there, discarded at down, copied out before.
  proj=$work_root/$k/$t
  mkdir -p "$proj"
  [ ! -d "$HERE/tasks/$t/fixture" ] || cp -r "$HERE/tasks/$t/fixture/." "$proj/"
  make_repo "$proj"
  # The outer box: the agent's whole world. --net isolated reaches only the model's port on the host
  # (not the user's services, not another harness's background server); its own Hyprland stands for
  # the user's desktop, so what the agent opens outside a box of its own lands there, seen by
  # check_safety. Started from the run's folder, which is no repo: nothing else of HOME is mounted.
  # --stock: a new Omarchy user's HOME, not this machine's desk, so the boxes the agent starts in it
  # (which seed from it) look the same on every machine.
  if ! (cd "$rd" && OMABOX_RENDER_NODE=$RENDER "$CLI" up "$box" --stock --no-shell --net isolated --allow "$PORT" --ro-bind "$CLI_REPO" \
    --overlay "$proj") > "$rd/up.log" 2>&1; then
    printf '%s\t%s\t%s\t%s\t%s\tfail\t0\t0\t0\tbench\tthe outer box did not come up (up.log)\n' "$t" "$m" "$h" "$sn" "$n" >> "$results"
    box=""; continue
  fi
  bh=$("$CLI" path -b "$box" --home)
  seed_home "$bh" "$h" "$sd"
  prompt=$(cat "$HERE/tasks/$t/prompt.md")
  cp "$HERE/tasks/$t/prompt.md" "$rd/"
  BOX=$box BOX_HOME=$bh RUN=$rd OMABOX=$CLI bash -c '. "$1"; home_sums' _ "$HERE/lib.sh" > "$rd/home-before.sums"
  agent=()
  [ "$guard" = 0 ] || agent=("$CLI" guard exec --)
  case "$h" in
    pi) agent+=(pi -p --no-session --mode json --provider local --model "$m" "$prompt") ;;
    opencode) agent+=(opencode run --standalone --format json -m "local/$m" "$prompt") ;;
    oracle)
      cp "$HERE/tasks/$t/oracle.sh" "$bh/.oracle.sh"; cp "$HERE/tasks/$t/oracle.sh" "$rd/"
      agent+=(bash /home/sbx/.oracle.sh) ;;
  esac
  t0=$(date +%s) rc=0
  # shellcheck disable=SC2016 # (expanded by the box's bash)
  timeout -k 15 "${limit}m" "$CLI" run -b "$box" -- env PATH="/home/sbx/.local/bin:$PATH" OMABOX_RENDER_NODE="$RENDER" \
    bash -c 'cd "$1" && shift && exec "$@"' _ "$proj" "${agent[@]}" > "$rd/transcript.jsonl" 2> "$rd/stderr.log" < /dev/null || rc=$?
  secs=$(($(date +%s) - t0))
  read -r tools turns < <(digest "$rd" "$h")
  # What the agent left in the task's folder (the overlay's upper layer is the box's).
  mkdir -p "$rd/work"
  { "$CLI" run -b "$box" -- tar -C "$proj" --exclude=.git -cf - . 2>/dev/null | tar -x -C "$rd/work" 2>/dev/null; } || true
  step="" why=""
  cl=$(BOX=$box BOX_HOME=$bh RUN=$rd OMABOX=$CLI "$HERE/tasks/$t/check.sh" 2>&1) || {
    step=${cl##*$'\n'} step=${step%%:*}
    why=$(tail -1 <<<"$cl" | cut -d: -f2- | sed 's/^ //')
    [ -n "$why" ] || { step=check why="check.sh failed: $(tail -1 <<<"$cl" | cut -c1-120)"; }
  }
  echo "$cl" > "$rd/check.log"
  if [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
    step=timeout why="still running after $limit min${step:+ (then $(tail -1 <<<"$cl"))}"
  elif [ "$rc" != 0 ] && [ -z "$step" ]; then
    step=harness why="the agent exited with $rc (stderr.log)"
  fi
  result=pass; [ -z "$step" ] || result=fail
  "$CLI" down -b "$box" > "$rd/down.log" 2>&1 || true
  box=""
  rm -rf "${work_root:?}/$k"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$t" "$m" "$h" "$sn" "$n" "$result" "$tools" "$turns" "$secs" "$step" "${why//$'\t'/ }" >> "$results"
  echo "  $result${step:+ ($step: $why)} — $tools tools, ${secs}s" >&2
done
column -t -s $'\t' "$results" >&2 || cat "$results" >&2
