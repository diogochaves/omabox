# Waiting on upstream

What omabox carries only until an upstream release has it. When one lands in Arch, drop the
workaround, re-run `./install.sh --check`, and move the entry to "Dropped" with the date.

## aquamarine: configure fix (PR #415)

- **Needed for:** headless boxes on an NVIDIA render node and confirm-close (a Wayland output made
  after the backend started, or whose parent is labwc: NOTES findings 4, 125). Everything else runs
  on the system's aquamarine.
- **Waiting for:** a release containing commit `7bb8bdf4` ("wayland: fix configure not applying
  sometimes (#415)", 2026-09-22), after v0.15.1. Arch has `aquamarine 0.15.0` (2026-09-23).
- **Until then:** `omabox setup --aquamarine` builds `AQ_COMMIT` (a checkout's `build/prefix`,
  `install.sh` calls it; else `~/.local/share/omabox/aquamarine`); without one, `up` refuses what
  needs it, saying so.
- **Check:** `git -C build/aquamarine fetch -q --tags && git -C build/aquamarine tag --contains 7bb8bdf4`,
  then `pacman -Q aquamarine` at or past that tag. Once it is, `omabox --version` names the system's
  copy with no warning and `setup --aquamarine` builds nothing (`AQ_FIXED_AFTER` in `bin/omabox`,
  0.15.1: correct it if that release lacks the fix).
- **Then drop:** `cmd_setup`'s build (or all of `setup`, if #49 has not given it other parts), the
  private-build half of `aq_pick` (`AQ_COMMIT`, `AQ_USER`, the stale-soname note), the refusals and
  the confirm-close downgrade in `cmd_up`/`cmd_config` with `aq_lacks`/`AQ_HOWTO`, the
  `/opt/omabox/lib` bind and `LD_LIBRARY_PATH` in `share/start-hyprland.sh`, the aquamarine step and
  its build packages in `install.sh` (cmake, ninja, hyprwayland-scanner, ...), `OMABOX_AQUAMARINE`
  and `aq_unfixed` in the suite, and the mentions in README.md, CONTRIBUTING.md, NOTES "Reproduce",
  the skill's `reference.md` and AGENTS.md's "Developed against". Keep `confirm-close.sh`'s fallback.

## passt: `pasta --no-pidns`

- **Needed for:** simpler bookkeeping of every top-level box, which runs behind pasta (NOTES findings
  44, 45, 89). Not for correctness: without it, pasta adds its own pid namespace, so bwrap's
  `child-pid` is in pasta's numbering and omabox looks up the host pid itself.
- **Waiting for:** a release containing commit `588b545` ("pasta: Add --no-pidns to keep spawned
  command in caller's PID namespace", 2026-09-06). Arch has `passt 2026_07_28` (2026-09-23).
- **Check:** `pasta --help | grep -- --no-pidns`.
- **Then drop:** in `bin/omabox`, `pasta_pid`, `box_pasta`, the `pasta.pid` branch of `box_pid`,
  `kill_box`'s pasta fallback and pasta's `-P "$D/pasta.pid"`; add `--no-pidns` to the pasta
  command. Also: the `pid` and `pasta.pid` mentions in README, NOTES' architecture and AGENTS.md's
  teardown line, `t_isolated_no_pidfile`, and `t_stale_pid`'s pasta.pid checks. `t_race` and
  `t_pasta_dies` find pasta by its `-P` file: match bwrap's arguments in its command line instead.
  pasta stays bwrap's parent, so `--die-with-parent` stays. Test headless connected and isolated
  boxes, and an interactive one on the real desktop (with the user's go-ahead): a stand-in cannot
  run an interactive box behind pasta (NOTES finding 89).

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
