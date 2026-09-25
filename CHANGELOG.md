# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

### Added

- **Windows as targets**: `omabox windows` lists the box's windows (where, on screen or covered);
  `shot --window SEL` captures one window's own pixels, covered or on another workspace too;
  `click --window SEL X Y` takes window coordinates and focuses the window first when needed;
  `keys --window SEL` focuses it, then types. A selector never guesses: none or several is exit 2.
- **`shot --fit N`** scales a shot down; `click --in SHOT X Y` and `pointer --in` take the pixels of
  a cropped or scaled shot, so nobody does the arithmetic.
- **`omabox wait`** instead of `sleep`: until the screen holds still (`still`) or changes (`change`),
  a window or a shell layer is there or gone, or a command in the box succeeds. `keys`, `click` and
  `run -d` take `--wait`: they return once what they caused has settled, and say so when nothing
  changed. Exit 0, 124 at the deadline, 1 when it cannot be seen (never 0). A blinking caret and the
  cursor are not changes. Costs nothing while the screen is idle (`tools/still`).

### Changed

- `shot --active` is `shot --window active`: the window's own pixels, not a crop of the screen.
- install.sh checks that grim can capture a window (`-T`, grim 1.5).

## 0.1.1 — 2026-09-25

NVIDIA GPUs, thanks to [@btsouth](https://github.com/btsouth) ([#1](https://github.com/diogochaves/omabox/pull/1)).

### Fixed

- Headless boxes now start and resize on NVIDIA GPUs with a render node, without exposing the host
  display or DRM card. A custom bar without a tray no longer holds startup at the tray check.
- On NVIDIA, startup checks for the screen resize helper (`wlr-randr`) before creating a box.

### Changed

- `install.sh` installs one more package, `wlr-randr` (NVIDIA boxes size their screen with it), so
  the next update may ask for sudo once. AMD and Intel boxes keep working without it.

## 0.1.0 — 2026-09-25

The first release.

### Added

- **Boxes**: `omabox up` starts a contained, invisible Hyprland + Omarchy desktop (Omarchy's
  Hyprland config and shell, your theme and bar) on a private screen and a private D-Bus session bus,
  in 3-4 s. `run`, `shot`, `keys`, `click`, `pointer`, `hyprctl`, `restart-shell`, `mode`, `gpu`,
  `env`, `path`, `down` and `ls` drive it.
- **Tests that touch the desktop**: `omabox run -- ctest ...` with no box up runs them in a throwaway
  box, the repo as a discarded overlay. `--pass VAR` hands the command one of your variables without
  putting it on a command line.
- **Box options**: `--size WxH[@HZ]|host`, `--plugin`, `--ro-bind DIR[:DEST]`, `--net isolated
  --allow PORTS` (no internet or LAN, only the listed host ports), `--env`, `--idle`, `--stock-bar`,
  `--systemd` (a real systemd user manager), `--xwayland`, `--no-shell`.
- **Seeing a box**: `omabox peek` (a live, view-only window of a headless box) and `omabox up
  --interactive` (a box as a window you use; SUPER+ALT+ESCAPE sends SUPER keys to it).
- **Settings**: `omabox config` for where windows open (`workspace`), `confirm-close` and the widget's
  `bar-icon`.
- **Bar widget** (`chaves.omabox`): the boxes on this machine, with Peek, Screenshot and Down, a New
  interactive box button and a Settings face.
- **Agent skill** for Claude Code, Codex, OpenCode, pi and Hermes, so agents do GUI work in a box.
- **Agent guard** (opt-in): `omabox guard on` gives agents' shell commands a display that does not
  exist, so a window opened by mistake fails instead of reaching your desktop; `omabox host -- CMD`
  for what you ask for on the real one.
- `install.sh` for a fresh Omarchy, with `--check`; `test/run.sh`, the regression suite.
