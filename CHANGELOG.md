# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

### Fixed

- Headless boxes now start and resize on NVIDIA GPUs with a render node, without exposing the host
  display or DRM card. A custom bar without a tray no longer holds startup at the tray check.

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
