# omabox — instructions for agents

A contained, invisible Hyprland + Omarchy desktop ("box") where agents launch, drive and
screenshot apps and Omarchy shell plugins **without touching the user's real desktop**.

If `.local/AGENTS.md` exists, read it first: this checkout's private notes (git-ignored; its rules
add to the ones here).

Read first: `NOTES.md`: architecture, reproduce steps, what was verified, findings, dead ends,
open gaps. `bin/omabox` is the CLI, `share/` runs inside the box, `tools/` holds the C helpers (pointer,
keyboard, wlfd, peek), `install.sh` sets a machine up, `skill/` is the agent skill (linked for Claude
Code, Codex, OpenCode, pi and Hermes), `plugin/` is the bar widget, `test/run.sh` is the regression
suite (run it in full before each commit; `test/run.sh unit` is the fast tier).
`spike/` is the original record; the CLI supersedes it.

## Non-negotiables

- **Never disturb the real desktop.** No focus changes, cursor warps, workspace switches or
  windows on the user's session unless the user asked for one (e.g. interactive mode). When you
  must touch the host Hyprland, open things `silent` on the configured workspace (`omabox config
  workspace`, 9 by default) and verify focus afterwards with `hyprctl -j activeworkspace` /
  `activewindow`.
- **Box safety invariant:** never bind `/dev/dri/card*`, `/dev/input`, `/run/seatd.sock`,
  `/run/dbus` (system bus) or the real `$XDG_RUNTIME_DIR` into a box. Hyprland tries DRM first;
  those binds are the only thing stopping it from grabbing the real seat.
- Box HOME is fake and seeded without secrets (never copy `~/.config/omarchy/api-keys.env`,
  keyrings, tokens). Host code is mounted read-only.
- Teardown = `kill -KILL` the namespace PID 1 (`bwrap --info-fd` → `child-pid`). SIGTERM does not work.
- Never touch the user's real local services (servers on host ports they did not name as test
  servers). Test against throwaway servers on ports of your own, or `--net isolated --allow PORTS`.
- Don't use `ydotool` or anything uinput-based: it would drive the real cursor. Don't use `wtype`
  either: Hyprland reads its keys as other keys (finding 13); `omabox keys` instead. Never run
  `tools/keyboard` or `tools/pointer` from a host shell, not even to check how they parse arguments:
  that drives the real desktop (it happened once). They refuse outside a box; test them through
  `omabox keys/click/pointer` or the suite. `tools/peek` is a host window: only through `omabox peek`
  when the user asks, or inside a box pointed at that box (as `t_peek` does).
- Test input/UI changes in a box, or in a box standing in for the host (finding 26), never with
  real key presses on the user's desktop.
- `sudo` only when the user has explicitly allowed it in the session.

## Conventions

- Bash for the CLI unless a part clearly needs C (like `tools/pointer`). `set -euo pipefail`,
  shellcheck-clean.
- Small commits, imperative subject, body says why.
- Every behaviour change gets a line in `NOTES.md` (a finding, or an item under Open). Keep the
  "Reproduce on a fresh Omarchy install" section runnable: it becomes `install.sh`.
- Report honestly what was verified in a real box vs. assumed.

## Developed against (2026-09-23)

Hyprland 0.56.2 (Lua config, `hyprctl eval`, dispatch syntax `hl.dsp.*`), aquamarine 0.15.0
system + patched 0.15.1@7bb8bdf4 in `build/prefix` (needed until a release has PR #415),
labwc 0.20.2 as parent compositor, quickshell 0.3.1.
