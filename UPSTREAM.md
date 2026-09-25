# Waiting on upstream

What omabox carries only until an upstream release has it. When one lands in Arch, drop the
workaround, re-run `./install.sh --check`, and move the entry to "Dropped" with the date.

## aquamarine: configure fix (PR #415)

- **Needed for:** a nested Hyprland that applies its window's configure (NOTES finding 4).
- **Waiting for:** a release containing commit `7bb8bdf4` ("wayland: fix configure not applying
  sometimes (#415)", 2026-09-22). Arch has `aquamarine 0.15.0` (2026-09-23).
- **Check:** `git -C build/aquamarine fetch -q --tags && git -C build/aquamarine tag --contains 7bb8bdf4`,
  then `pacman -Q aquamarine` at or past that tag.
- **Then drop:** the aquamarine step, `AQ_*` and the soname check in `install.sh` (and its header
  comment), its build packages in `PKGS` (cmake, ninja, hyprwayland-scanner, ...), in `bin/omabox`
  `AQUAMARINE`, the soname check in `check_install` and `--ro-bind "$AQUAMARINE" /opt/omabox/lib`,
  `LD_LIBRARY_PATH=/opt/omabox/lib` in `share/session.sh` (every process in a box inherits it today:
  harmless, the dir holds only libaquamarine, but a box's environment differs from the host's there),
  `build/prefix`, and the mentions in README.md ("patched aquamarine"), NOTES "Reproduce" and
  AGENTS.md's "Developed against".

## passt: `pasta --no-pidns`

- **Needed for:** simpler bookkeeping of `--net isolated` boxes (NOTES findings 44, 45). Not for
  correctness: without it, pasta adds its own pid namespace, so bwrap's `child-pid` is in pasta's
  numbering and omabox looks up the host pid itself.
- **Waiting for:** a release containing commit `588b545` ("pasta: Add --no-pidns to keep spawned
  command in caller's PID namespace", 2026-09-06). Arch has `passt 2026_07_28` (2026-09-23).
- **Check:** `pasta --help | grep -- --no-pidns`.
- **Then drop:** in `bin/omabox`, `isolated_pid`, the `net = isolated` branch of `box_pid`, the
  `isolated_pid` call in `cmd_up`'s wait loop, and pasta's `-P "$D/pasta.pid"`; add `--no-pidns` to
  the pasta command. Test a headless and (in a stand-in first) an interactive isolated box.

## Hyprland: a nested Hyprland ignores its window being resized

- **Needed for:** interactive boxes following their window's size.
- **Waiting for:** a fix; not reported yet. Brief with repro, source locations and a fix hypothesis:
  `docs/hyprland-nested-resize-bug.md`.
- **Workaround:** a size watcher in `share/hyprland.lua` that reloads the box's config (NOTES finding
  28; brief black flicker).
- **Then drop:** that watcher, after checking a resize in an interactive box.

## Could be reported (nothing waiting on it)

- aquamarine: fixed protocol versions (NOTES finding 2).

## Dropped

(none yet)
