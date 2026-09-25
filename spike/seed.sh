#!/usr/bin/env bash
# spike/seed.sh <dir> : build the sandbox's fake HOME from the real user's Omarchy look (no secrets).
set -euo pipefail
H=$1/home; mkdir -p "$H/.config/omarchy/plugins" "$H/.local/state/omarchy/current"
cp ~/.config/omarchy/shell.json ~/.config/omarchy/shell.toml "$H/.config/omarchy/"
cp -rL ~/.local/state/omarchy/current/theme "$H/.local/state/omarchy/current/theme"
cp ~/.local/state/omarchy/current/theme.name "$H/.local/state/omarchy/current/"
bg=$(readlink ~/.local/state/omarchy/current/background); ln -sfn "/home/sbx/.local/state/omarchy/current/theme/${bg#*/current/theme/}" "$H/.local/state/omarchy/current/background"
mkdir -p "$H/.local/state/omarchy/indicators" && touch "$H/.local/state/omarchy/indicators/stay-awake"   # no idle/screensaver/lock in the box
