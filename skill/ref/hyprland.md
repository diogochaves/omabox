# omabox: Hyprland's Lua, logs and events

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for `omabox lua`, `hyprctl` in a box, dispatchers, the box's logs and events.

Sections:

- Hyprland's Lua, logs and events

## Hyprland's Lua, logs and events

- `omabox lua EXPR` evaluates Lua in the box's Hyprland and prints what it returns, where `omabox
  hyprctl eval` prints only `ok`: `omabox lua 'hl.get_cursor_pos()'`, `omabox lua
  'hl.get_active_window().class'`, `omabox lua 'local w = hl.get_active_window(); return w.title,
  w.pid'` (statements need `return`). One line per value: strings and numbers as they are, `nil`,
  tables and Hyprland's objects (a window, a monitor, a layer) as JSON, objects inside them by name
  (`HL.Workspace(1:1)`); `--json` quotes strings too. A long script: `omabox lua - < script.lua`. A Lua
  error is exit 1 with its message. Globals you set stay for the next call (until a config reload).
- **Lua dispatchers.** A box's Hyprland runs a Lua config, where `hyprctl dispatch X` runs
  `hl.dispatch(X)`: the old syntax (`dispatch fullscreen 0`, `dispatch togglefloating`) dispatches
  nothing. `omabox hyprctl` fails it (hyprctl's 7; a `--batch`'s 1, where hyprctl says 0) and
  prints the Lua for it, for the common ones: `fullscreen [0|1] [set|unset]` →
  `hl.dsp.window.fullscreen({ mode = "fullscreen" })` (1: `"maximized"`; it toggles, as the old one
  did: to set it, `omabox window --fullscreen` or `--maximize`), `togglefloating`/`setfloating`/`settiled [SEL]` → `hl.dsp.window.float({ action = "toggle"
  | "enable" | "disable", window = SEL })`, `workspace WS` → `hl.dsp.focus({ workspace = "WS" })`,
  `focuswindow SEL` → `hl.dsp.focus({ window = SEL })`, `killactive` / `closewindow SEL` →
  `hl.dsp.window.close()`, `movetoworkspace[silent] WS[,SEL]` → `hl.dsp.window.move({ workspace =
  "WS", window = SEL, follow = false })`, `exec CMD` → `hl.dsp.exec_cmd("CMD")`. Run one with
  `omabox lua 'hl.dispatch(hl.dsp.window.fullscreen({ mode = "fullscreen" }))'` or `omabox hyprctl
  dispatch 'hl.dsp.window.fullscreen({ mode = "fullscreen" })'`; the rest are in `omabox lua 'local t
  = {} for k in pairs(hl.dsp.window) do t[#t + 1] = k end return t'`. A Lua dispatch Hyprland
  refuses (no such window) is exit 1 with its answer as it came: `hyprctl` prints `warning: …`
  (`error: …` keeps hyprctl's 7; with `-q` nothing; in a `--batch`, a dispatch between two other
  commands is not read), `lua` the `{"ok":false,"error":…}` that `hl.dispatch` returned. `lua`
  fails only on what `hl.dispatch` itself handed back, not on a table of yours that reads `ok =
  false`, nor on a refusal your source read (`local r = hl.dispatch(…) return r.ok`); an
  `hl.dispatch` given no dispatcher (an `hl.dsp` call with arguments it does not take is nil) is
  exit 1 too, said, as Hyprland's words for it only come back through `hyprctl dispatch`.
- A config reload (`hyprctl reload`, `omabox reload`, a theme switch, `omabox mode`, an interactive
  box's window resize) starts a fresh Lua state: binds, rules, `hl.config` values and plugins added
  with `lua`, `eval` or `plugin load` are gone. Re-apply them on `configreloaded`. Nothing reloads
  because a file changed: a box runs its own copy of omabox's config, made at `up`, with autoreload
  off, so an update of omabox or Omarchy leaves it as it is; `omabox reload` takes the newer one.
  A test that must see what a file change does, as a desktop would (a helper that rewrites a file
  the config loads, on a Hyprland event, reloads for ever there): `omabox up --autoreload`.
- The box HOME's `~/.config/hypr/bindings.lua`, `input.lua` and `looknfeel.lua` (`$(omabox
  path)/home/.config/hypr/`) load as on a desktop: append a bind or a setting, `omabox hyprctl
  reload`, and it applies (a plugin's "add this to bindings.lua", its Set up button). A new box's are
  Omarchy's comment-only files, not your desktop's. Never loaded: `monitors.lua` (the screen is
  `--size`/`--monitor`/`mode`'s), `autostart.lua`, and `hyprland.lua` itself. A file with an error
  does not stop the rest loading; `up`, `reload` and `hyprctl reload` print the config's errors
  (`omabox hyprctl configerrors`). A box draws no red "Your config has errors" bar
  (`debug:suppress_errors` is on, whatever `looknfeel.lua` says: drawing it can hang Hyprland 0.56):
  a shot never shows the errors, those commands do. A Hyprland that answers the reload and then
  stops answering is said there ("did not answer hyprctl configerrors"), exit 1: `omabox gdb`.
- Hyprland's own notifications (`omabox hyprctl notify`, and the ones it raises itself, such as "Your
  monitor layout is set up incorrectly") are drawn at the top right of the focused monitor, below
  the bar, sliding in over 0.6 s: a shot of the screen's left (`-g 0,0,...`) never has them.
  `hyprctl notify` says the `shot -g` of that corner. The layout warning is in the Hyprland log as
  "Monitor NAME: detected overlap with layout" (`omabox log --grep overlap`); a notification's own
  text is not logged.
- An error inside a callback (`hl.on`, `hl.timer`) is not in `lua`'s answer, and when the callback
  runs later (a timer, an app's event) it is logged nowhere, not even in the Hyprland log: seen
  nothing, check with `pcall` inside the callback and keep the error in a global to read with
  `omabox lua`. (An `hl.on` callback that a `hyprctl dispatch` sets off errors in that dispatch's
  answer.)
- `omabox log [LOG...|all] [-n 100|all] [--grep RE [-i]] [--width N | --full] [-f]`: the box's logs, the last 100 lines
  of each, every line cut at 400 characters (`… (+N chars)`; `--width N` another cut, `--full` or
  `--width 0` whole lines; `--json` records are never cut). `hyprland` (the default), `shell` (the bar, plugins, QML errors; each line with its
  time to the millisecond, `YYYY-MM-DD HH:MM:SS.mmm`, for timing what the shell does), `apps` (what the
  launcher and binds started), `autostart` (a desk box's: what its Hyprland config started and what
  the box dropped, ref: `--desktop desk` in builds.md), `run` (the latest `run -d`; `run JOB`: that job's, by the pid `run
  -d` and `jobs` say, ended too, however many jobs came after it), `keyring`, `labwc`, `systemd` (with
  `--systemd`), `audio` (PipeWire's, with `--audio`), `polkit` (the stand-in authority's, with `--polkit`), `events` (Hyprland's events, which `omabox events` reads), `journal` (what programs
  sent the box's journal sockets: dbus-broker, `logger` and `logger --journald` (not `-t omarchy-shell`: `shell`), `/usr/bin/systemd-cat` (the bare name writes `~/ID.log`), Qt with `QT_FORCE_STDERR_LOGGING=0`, units' output; at most 4 MB, then
  `journal.1.log`), `notifications` (what reached the notification server: ref: Toasts and dock in shell.md), `box` (bwrap), `reap` (the
  reaper's: idle and agent checks, the down it did). A box that died keeps its logs until `down`: read them to see why.
  `-f` follows until the box goes down (exit 0). `-i` (ignore case) goes anywhere, `--grep -i RE`
  too. Hyprland writes its log file 8 KiB at a time: `log hyprland` of a live box adds the lines
  not in the file yet from `omabox hyprctl rollinglog` (its last 4 KiB), and says on stderr when the
  file is further behind than that (the lines between come later). `-f` follows the file, so a line
  about what just happened can come many lines later; `omabox hyprctl rollinglog` has it at once.
- `omabox events`: Hyprland's event stream (`activewindow>>`, `urgent>>`, `openlayer>>`,
  `workspace>>`, ...) as the box recorded it from its start, one stamped line each (`--json`:
  `{time, event, data}`). Never hand-roll a socat on `.socket2.sock`, and never truncate a log
  something is writing (NUL-padded files, "no events" when there were some). To look at what one
  step caused: `omabox events --mark m1` (a byte offset, kept by name; nothing is cleared), act, then
  `omabox events --since m1 [--grep '^urgent>>']`. `--since 30s` also works. `--until RE [--timeout
  10s]` waits for the first matching event from the mark (one that came already counts) or from
  now: 0 with the event printed, 124 none in time, 1 the box went down. `-f` follows. E.g. "did the
  app ask for activation when relaunched?": mark, relaunch, `events --since m1 --until '^urgent>>'`.
