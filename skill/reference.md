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

## Pointer, in detail

- `omabox drag [--window SEL | --in SHOT] X1 Y1 X2 Y2 [left|right|middle] [--steps 10] [--hold MS]
  [--shot FILE | --wait]`: press at the first point, move to the second in steps, hold there `--hold`
  (a drop target reacting to the hover), release. `--shot FILE` takes a shot while the button is still
  down (a drag's own feedback). Both points are mapped as for `click`.
- `omabox pointer [--window SEL | --in SHOT] -- move X Y, click [BTN], down [BTN], up [BTN], scroll
  DY, sleep MS` in one run: raw, it raises nothing (a `--window` must be on screen and uncovered). The
  button defaults to left. A button pressed with `down` stays down after the call, until an `up` (in
  a later call too): end every `down` with an `up`, or the box's next clicks are drags.
- Not yet: a click with a modifier held (ctrl-click, shift-click; issue #25).

## Waiting, in detail

- `omabox wait [--timeout 10s] [--json] COND`, one condition per call (chain with `&&`): `still
  [--quiet 300ms] [-g GEOM | --window SEL] [--strict]`, `change [-g | --window]`, `window SEL [--gone
  | --focused]`, `layer NAMESPACE [--gone]` (`omarchy-menu`, `omarchy-notifications`, ...), `cmd --
  CMD` (exit 0 inside the box).
- `keys`, `click`, `drag` and `run -d` take `--wait [--start 2s] [--quiet 300ms] [--timeout 10s] [--json]`:
  the screen before the action, a change within `--start` (5 s for `run -d`), then `--quiet` with none.
- A caret (a change 4 px or thinner) and the software cursor (in every frame; it hides on a key
  press) are not changes; the line says what was ignored. `--strict` counts them (a thin progress bar
  or spinner is ignored like a caret otherwise).
- Exit 0 satisfied, 124 not in time, 1 unknown (the box went down; an interactive box that is not
  drawn while hidden: an older one, or one whose window confirm-close replaced). Waiting counts as use
  for the idle timeout.

## Hyprland's Lua, logs and events

- `omabox lua EXPR` evaluates Lua in the box's Hyprland and prints what it returns, where `omabox
  hyprctl eval` prints only `ok`: `omabox lua 'hl.get_cursor_pos()'`, `omabox lua
  'hl.get_active_window().class'`, `omabox lua 'local w = hl.get_active_window(); return w.title,
  w.pid'` (statements need `return`). One line per value: strings and numbers as they are, `nil`,
  tables and Hyprland's objects (a window, a monitor, a layer) as JSON, objects inside them by name
  (`HL.Workspace(1:1)`); `--json` quotes strings too. A long script: `omabox lua - < script.lua`. A Lua
  error is exit 1 with its message. Globals you set stay for the next call (until a config reload).
- An error inside a callback (`hl.on`, `hl.timer`) is not in `lua`'s answer, and when the callback
  runs later (a timer, an app's event) it is logged nowhere, not even in the Hyprland log: seen
  nothing, check with `pcall` inside the callback and keep the error in a global to read with
  `omabox lua`. (An `hl.on` callback that a `hyprctl dispatch` sets off errors in that dispatch's
  answer.)

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
- Saves: `omabox save NAME [-b BOX]` keeps the box HOME (app data, keyring; not `.cache` or logs);
  `omabox up --from NAME` / `run --from NAME -- CMD` start with it, the user's theme and bar seeded on
  top. Quit the app first for a clean save (the box is paused for the copy, so it is at least what a
  crash would leave). `omabox saves` lists them, `omabox saves rm NAME` deletes. Name saves after
  what they hold (`myapp-signed-in`); they stay until removed, so remove the ones you no longer need.
- No Xwayland unless `omabox up --xwayland`. Omarchy's `uwsm-app` launching always goes through a
  stand-in, `--systemd` or not: apps start as plain processes, not units (output in
  `<box dir>/home/apps.log`).
- The XDG base dirs are set as in a session (`XDG_DATA_HOME=/home/sbx/.local/share`, ...). An app
  installed into the box HOME the per-user way (`~/.local/share/applications`, a D-Bus service in
  `~/.local/share/dbus-1/services`) starts from the launcher and by D-Bus activation, as on the host.
  To test with another HOME (`env HOME=$(mktemp -d) app`), set the `XDG_*_HOME` vars too.
- `--no-shell` starts Hyprland only (no bar, tray or notifications): faster for plain app work.
- A crash in a box leaves no core file and no crash notification on the user's desktop (the core
  limit is 1 byte). To get a core: `omabox run -- bash -c 'ulimit -c unlimited; exec ./app'`.
- Logs: `<box dir>/home/*.log` (shell, keyring, labwc, runs), Hyprland's in
  `<box dir>/run/hypr/*/hyprland.log`, bwrap's in `<box dir>/box.log`.

## Network

- Each box has its own network namespace. By default it reaches the internet, the LAN and the user's
  servers on the host's 127.0.0.1; `--net isolated --allow PORTS` reaches only those host ports.
- Across the box boundary use `127.0.0.1`, not `localhost`, with a server listening on IPv4
  (`127.0.0.1`, `0.0.0.0` or `::`): from a box, `localhost` is reset on an IPv4-only server, and
  between boxes always. A box's server is reachable from the host (and other boxes) on
  `127.0.0.1:PORT` within about a second of listening (poll for it); one on `::1` only is not.
  Inside a box the host's LAN address is the box itself. A connected box started inside a box has
  no network.

## Box lifetime

- Your session's box goes down when your agent (Claude Code, Codex) exits. A peek window or an
  `omabox run` still going then keeps it until they end; a `run -d` job does not. After `/clear` the
  agent's process lives on and the old box only idles out.

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
  (SUPER+ALT+ESCAPE sends SUPER keys to it). The host keeps drawing it while it is hidden, so `shot`,
  `click`, `keys` and `wait` work on it. Once the user closed that window and kept the box running,
  the new one is drawn only while shown: `click` and `keys` still reach it, but `shot` gets no frame,
  and the new window can open on an empty workspace, where they reach no app. When the user has to
  act in it (a login), tell them which workspace it is on and let them go there.
