# Known limitations

What is untested, and what a box does differently from your desktop. Back to the
[README](../README.md).

- Other NVIDIA models and drivers are untested. Of the agents the skill is installed for, Claude
  Code and OpenCode were checked end to end; Codex, pi and Hermes find the skill, but no run of
  theirs reached a model here.
- Headless boxes on NVIDIA and confirm-close need aquamarine's fix (PR #415, in aquamarine 0.16.0;
  for an older system aquamarine built into `build/prefix` by `install.sh`, `omabox setup
  --aquamarine`, unless your system's package is patched with it); without it `up` refuses them, saying what to run. omabox's
  own fixes for interactive boxes (held keys, the pointer and cursor in monitor windows, a class for
  each monitor's window) are in that build only: no system aquamarine has them. What omabox carries until upstream releases land, and
  what to drop then: `UPSTREAM.md`.
- A hidden interactive box draws at the host's `misc.render_unfocused_fps` (15 by default), so
  `omabox shot` works with its window off screen, just at that rate. Not after you closed its window
  and kept the box running (`confirm-close`): the new window is only drawn while it is on screen.
- The file chooser portal was checked (xdg-desktop-portal-gtk); other portals are untested.
- Apps that need system services over the system bus (GNOME Disks/udisks, NetworkManager, bluetooth,
  power) open with errors or not at all: a box has no system bus, by design.
- Omarchy's app launching goes through a stand-in for `uwsm-app` in every box, `--systemd` or not
  (apps start as plain processes, not units); their output lands in the box's `home/apps.log`.
  Logging out of the box ends it. Without `--systemd`, `systemd-run --user` (the browser bind) runs
  its command directly too, timers excepted, and `systemd-cat` writes to `~/IDENTIFIER.log`, so
  `omarchy restart shell` works in a box. For the box's journal (`omabox log journal`), use
  `/usr/bin/systemd-cat` by its full path; `logger` and `logger --journald` reach it by name
  (`logger -t omarchy-shell`, the shell launcher's, goes to the shell's log).
- A `run -d` job does not count as use for idle expiry: a server the agent only polls over HTTP needs
  `--idle 0` (or a longer one), and the agent then takes that box down itself when done. Nor does it
  keep an agent session's box once the agent exits.
- On NVIDIA, whose driver reports no per-process DRM counters, `omabox gpu` takes the driver's own
  sampling through `nvidia-smi pmon`: whole seconds, and a process the driver did not sample in a
  second counts 0 for it. Intel's i915 and xe are untested (#223). Tools that sum GPU time by process
  name add a box's Hyprland to yours: use `--size host` and `omabox gpu` to see what an animation costs.
