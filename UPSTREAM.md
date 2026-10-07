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

## aquamarine: keys held when keyboard focus leaves (not submitted)

- **Needed for:** interactive boxes: a key held when the box's window loses focus stays down in the
  box (NOTES finding 132).
- **Waiting for:** an aquamarine release that releases held keys on `leave`. omabox does not plan to
  submit the patch itself; if someone fixes it upstream, check the fix with `t_held_keys`.
- **Until then:** `patches/aquamarine/0001-*.patch`, applied by `setup --aquamarine` on top of
  `AQ_COMMIT` (`AQ_BUILD` names the result). Boxes on the system's aquamarine keep the bug.
- **Then drop:** the patch, `+keys` in `AQ_BUILD` (and the suite's checks for it); with no other
  patch left (the pointer's, below), the `git apply` loop in `setup_aquamarine`, `AQ_BUILD` (back to
  `AQ_COMMIT`) and the stale-build note in `setup_user`; `t_held_keys` then checks any aquamarine
  that has the fix.

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

## Hyprland: a nested Hyprland puts the pointer over its whole layout

- **Needed for:** an interactive box with more monitors: the pointer in a monitor's window (NOTES
  finding 238).
- **Waiting for:** a Hyprland that maps an absolute pointer event to the output it names; not
  reported yet. Hyprland 0.56.2's `src/devices/Mouse.cpp` passes aquamarine's `SWarpEvent` on
  without its `output`, and `CPointerManager::warpAbsolute` maps the point over the box around every
  monitor (a pointer has no `m_boundOutput` setting), so a point of any window but a lone one lands
  in the wrong place: the middle of each of three windows put the box's pointer at (1600, 1079).
- **Until then:** `patches/aquamarine/0002-*.patch` (`AQ_BUILD` `+layout`): aquamarine reads each
  output's box in the layout from the file `AQ_WAYLAND_LAYOUT` names (the box's
  `share/hyprland.lua` writes it at each layout change; `share/start-hyprland.sh` sets the variable
  for interactive boxes) and gives the point in the whole layout's terms. omabox-specific: not to
  submit. Boxes on another aquamarine keep the bug; `monitor add` says so.
- **Then drop:** the patch, `AQ_WAYLAND_LAYOUT` and `write_layout` in `share/hyprland.lua`,
  `+layout` in `AQ_BUILD` and in `monitor_add_window`'s note; `t_monitors_window`'s pointer check
  then checks any build.

## Omarchy: the shell's crash dialog takes focus (not reported yet)

- **Needed for:** driving a box after its shell crashed. Quickshell's crash reporter opens a dialog
  (class `org.quickshell`, title `quickshell`) that takes focus; a following `omabox keys` Return hit
  its "Open report page" and opened a browser in the box (NOTES finding 183).
- **Waiting for:** Omarchy keeping that dialog from opening or from taking focus. Checked on 4.0.4-1
  and `quattro` 81145eb (2026-10-05): the shell is relaunched by `omarchy-launch-shell` (about 1.5 s
  after a crash within Quickshell's 10 s window), the dialog still opens, and no window rule covers
  it. Not reported upstream yet.
- **Until then:** `share/hyprland.lua`'s `window.open` hook closes the reporter's window (its environ
  has `__QUICKSHELL_CRASH_DUMP_PID`).
- **Then drop:** nothing while omabox supports an Omarchy without the fix: the hook costs nothing
  when no dialog opens.

## Omarchy: idle timeout 0 means off (#12538): no change planned

- omabox sets `idle.screensaver` and `idle.lock` to 1000000 s in a box's `shell.json` (NOTES
  finding 51). `quattro` 7901d7d (merged 2026-10-04, after v4.0.4) makes 0 mean off, but on 4.0.4
  and before, 0 starts the screensaver and lock at once. 1000000 holds idle off on both, and stays
  under the Qt timer's limit of about 24.8 days (#13920). Do not switch to 0 while omabox supports
  4.0.4, an installed one or an `--omarchy` tree that old.

## Omarchy: `plugin enable` right after `rescanPlugins` (#9304): no change planned

- setup's widget prompt (`setup_widget` in `bin/omabox`) retries `omarchy plugin enable` (20 x
  0.2 s) while it says "is not known": `rescanPlugins` returns before the shell has discovered the
  plugin (NOTES finding 133). Upstream: #9304 and #11115, with fixes in #11117, #11118 and #7754 (a
  targeted `discoverPlugins`; `plugin add --enable` discovers first). The retry is harmless where
  that is fixed: keep it. If #7754 lands, `discoverPlugins` could replace rescan plus retry, only
  where the shell has the method (`t_unit_omarchy_contract` can tell).

## Could be reported (nothing waiting on it)

- aquamarine: fixed protocol versions (NOTES finding 2).
- passt: in a strong `-t` auto rule (`1-65535,auto`, `32768-60999,auto`), a port pasta cannot bind
  on the host (another namespace's forward holds it) stops the namespace's later ports, on each
  rescan, while it is held (`fwd_sync_one` returns -1; a weak rule skips the port). Standalone
  repro, no omabox: two pasta namespaces. omabox uses `auto`, which is weak, plus a
  strong rule for the ephemeral range only, where the bug remains (NOTES finding 156). A weak
  explicit range, or a skip in auto rules, would let that rule go too.

## Dropped

(none yet)
