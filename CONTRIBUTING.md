# Contributing

omabox is young, and it has only run on a couple of machines. You can help without writing any code:
try it on a setup it has not seen (NVIDIA, another GPU, several monitors, a fresh Omarchy install)
and say what happened. Bug reports, fixes, tests, docs and ideas are all welcome.

## Reporting a problem

Open an issue with:

- `omabox --version`, `Hyprland --version` and your Omarchy version (`omarchy version`);
- your GPU (`ls -l /sys/class/drm/renderD*/device/driver`);
- the command you ran and what it printed;
- for a box that failed or died, its logs: `omabox ls` names it, and `omabox path -b NAME` is where
  they are (`box.log`, `home/*.log`, `run/hypr/*/hyprland.log`). Look them over before you attach
  them: they can hold paths and names from your machine.

A problem with safety (a box reaching your real desktop, your files or your session) matters most:
say so in the title.

## Changing it

```bash
./install.sh           # builds the tools and the patched aquamarine
test/run.sh unit       # the fast tier, no box
test/run.sh            # the whole suite in real, headless boxes (~5 min); run it before a pull request
shellcheck bin/omabox install.sh test/run.sh docs/demo.sh
```

Read [AGENTS.md](AGENTS.md) first, whether you are a person or an agent: it has the rules the code
keeps. The ones that matter most:

- **Never disturb the real desktop.** Test input and UI changes in a box, or in a box standing in
  for the desktop (how the suite checks interactive mode, peek and the guard). Never run
  `tools/keyboard` or `tools/pointer` from a host shell.
- **The box safety invariant**: a box never gets `/dev/dri/card*`, `/dev/input`, seatd, the system
  bus or your real `$XDG_RUNTIME_DIR`. Those are what keep its Hyprland off your seat.
- A box's HOME is seeded without secrets, and host code is mounted read-only.

Then:

- Keep pull requests small and focused, one change each, with a commit message that says why.
- A fix comes with a check in `test/run.sh` that fails on the old code, when a box can show it.
- Every change of behaviour gets a line in [NOTES.md](NOTES.md) (a finding, or an item under Open), and
  user-visible ones a line under Unreleased in [CHANGELOG.md](CHANGELOG.md).
- Say what you verified in a real box and what you only reasoned about.

If you want to work on something bigger, open an issue first so we can talk it over. NOTES.md's
"Open" list and [UPSTREAM.md](UPSTREAM.md) are good places to start.
