# omabox: reference

Detail that `SKILL.md` points to. The safety rules are all in `SKILL.md`; nothing here relaxes them.

## Screen size and rendering cost

- `omabox up --size 3440x1440` for another screen size (@60), `--size 3440x1440@144` for a refresh
  rate, `--size host` for the user's own monitor; `omabox mode` shows or changes it on a running box.
- **Measuring rendering cost** (GPU time of an animation, a repaint loop): `omabox up --size host`,
  put the UI in the state to measure, then `omabox gpu 10` (% of wall time per process, this box
  only; `--json`). Never read host-wide tools (nvtop, radeontop, scripts summing `/proc/*/fdinfo` by
  name) while a box is up: they add the box's Hyprland and quickshell to the user's. Match the mode:
  anything that repaints per frame costs ~2.4x more at 144 Hz than at the default 60. NVIDIA driver
  615.71.09 does not expose the per-process DRM counters `gpu` needs; on that driver it reports no
  percentages.

## Waiting, in detail

- `omabox wait [--timeout 10s] [--json] COND`, one condition per call (chain with `&&`): `still
  [--quiet 300ms] [-g GEOM | --window SEL] [--strict]`, `change [-g | --window]`, `window SEL [--gone
  | --focused]`, `layer NAMESPACE [--gone]` (`omarchy-menu`, `omarchy-notifications`, ...), `cmd --
  CMD` (exit 0 inside the box).
- `keys`, `click` and `run -d` take `--wait [--start 2s] [--quiet 300ms] [--timeout 10s] [--json]`:
  the screen before the action, a change within `--start` (5 s for `run -d`), then `--quiet` with none.
- A caret (a change 4 px or thinner) and the software cursor (in every frame; it hides on a key
  press) are not changes; the line says what was ignored. `--strict` counts them (a thin progress bar
  or spinner is ignored like a caret otherwise).
- Exit 0 satisfied, 124 not in time, 1 unknown (the box went down; an interactive box whose window is
  hidden renders nothing). Waiting counts as use for the idle timeout.

## Mounts, HOME and the session

- The repo you ran `omabox up` from is visible **read-only** at the same path, plus any dirs listed in
  `~/.config/omabox/ro-bind` or passed with `--ro-bind` (`DIR:DEST` for another path, e.g. testing
  path mapping with `--ro-bind ~/nas:/mnt/nas`), mise's toolchains (`omabox run` keeps your PATH, so
  node/python/uv are the host's), `--plugin` dirs and the user's git name/email; nothing else of the
  user's HOME. Outside a repo a throwaway `run` mounts nothing of the current dir.
- The box HOME is `omabox path` → `<dir>/home`, readable and writable from the host: put outputs there
  or in `/tmp` inside, and seed a widget's data files (a usage record, a store) there while the box runs.
- The box has the host's programs, not more: an app the host does not have fails in the box too
  (`setsid: failed to execute alacritty`); Omarchy's terminal is `foot` or what the host has.
- The session's PATH is yours (mise's tools, as in `omabox run`) with the box HOME's `~/.local/bin`
  first: drop a stub CLI there to fake one a plugin calls. `--env KEY=VAL` on `up` sets a variable for
  the whole session (the bar included), e.g. a plugin's API base pointed at a stub.
- `omabox up --systemd` gives the box a real systemd user manager: `systemctl --user`, units in the box
  HOME's `~/.config/systemd/user`, `systemd-run --user` timers (for plugins that manage their own
  service or schedule alarms). No journald (`journalctl --user` is empty) and no logind either way.
- No Xwayland unless `omabox up --xwayland`. Omarchy's `uwsm-app` launching always goes through a
  stand-in, `--systemd` or not: apps start as plain processes, not units (output in
  `<box dir>/home/apps.log`).
- `--no-shell` starts Hyprland only (no bar, tray or notifications): faster for plain app work.
- A crash in a box leaves no core file and no crash notification on the user's desktop (the core
  limit is 1 byte). To get a core: `omabox run -- bash -c 'ulimit -c unlimited; exec ./app'`.
- Logs: `<box dir>/home/*.log` (shell, keyring, labwc, runs), Hyprland's in
  `<box dir>/run/hypr/*/hyprland.log`, bwrap's in `<box dir>/box.log`.

## The Omarchy shell's IPC (direct routes)

`omabox run -- omarchy-shell TARGET METHOD [ARGS]` talks to the box's shell (never the user's):

- `shell summon ID`, `shell toggle ID`, `shell hide ID`: open or close a plugin's panel, third-party
  plugins included (`shell summon chaves.omabox` opens that bar widget's panel).
- `shell call ID METHOD ARG`: a function of a loaded panel/overlay/menu plugin (`unknown` when there
  is none).
- `shell listPlugins` (JSON: id, kinds, enabled), `shell listShellConfig`, `shell ping`.
- A plugin's own `IpcHandler` answers to its `target` (a panel's `ipcTarget`, e.g. `omarchy.audio
  toggle`: open, close, show, hide, toggle). An unknown target or method prints `Target not found.` / `Function not found.`.

## Showing the user

- `omabox peek` opens a live, view-only window of your headless box on the user's workspace 9 (or the
  one they set with `omabox config workspace`) without taking focus. It does not affect the box. Your
  `click`, `pointer` and `keys` show on it for ~3 s (a ring, key captions; `--pass` values as `*`),
  never in your shots.
- `omabox up --interactive` makes the box a real window on that workspace that the user drives
  (SUPER+ALT+ESCAPE sends SUPER keys to it). `shot` and `wait still` do not work while that window is
  hidden; agents use headless boxes.
