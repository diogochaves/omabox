# omabox: pointer and waiting

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for clicks, drags, scrolls, the pointer, and waiting for something to happen.

Sections:

- Pointer, in detail
- Waiting, in detail

## Pointer, in detail

- **The pointer is test state.** Under Omarchy's focus-follows-mouse the window the pointer rests on,
  or last passed over, takes focus, and gets it back when a menu or panel closes. A box's pointer
  starts at the screen's centre and stays wherever the last command left it (`omabox hyprctl
  cursorpos`). `click` and `pointer -- move` jump: they cross nothing on the way. Before a test whose
  result depends on focus, put the pointer where a user's would be (`omabox pointer -- move X Y`), and
  when the way there matters (to the bar, across other windows) travel: `click --steps 20 X Y`,
  `pointer --steps 20 -- move X Y`. `--mod ctrl` (shift, alt; `ctrl+shift`) holds modifiers across a
  click or drag; SUPER with a button is Hyprland's own (move, resize), never the app's.
- `omabox drag [--window SEL | --in SHOT] X1 Y1 X2 Y2 [BTN] [--steps 10] [--hold DURATION]
  [--mod MODS] [--shot FILE | --wait]`: press at the first point, move to the second in steps, hold
  there `--hold` (a drop target reacting to the hover), release. `--hold` is a duration, `300ms` or
  `2s` (a bare number is seconds, up to 60s), as every duration is: `300ms`, `1.5s`, `2m`, `1h`.
  `keys -s 300`, `pointer sleep 300` and `up --idle 45` are refused (milliseconds and minutes up to
  0.9): write the unit.
  `--shot FILE` takes a shot while the button is still down (a drag's own feedback); `--wait`
  ignores the cursor along the path, so a change wholly under it may not count. Both points are
  mapped as for `click`. It jumps to the first point: to get there on foot, `pointer --steps N --
  move X1 Y1` first.
  It does real drag-and-drop between widgets and windows: checked in GTK 4 (Nautilus: a folder onto a
  sidebar bookmark), Chromium (an HTML5 `draggable`) and Qt Quick (`Drag.Automatic`), with any
  `--steps` or `--glide`, held or not. The path takes 150 ms at least (GTK starts a drag only on a
  motion 100 ms after the press) and ends with a 1 px nudge and back (Hyprland puts a new drag at the
  window's centre until the pointer moves). A drop target may change under the hover (Nautilus
  opens a space for a new bookmark above the rows): `--shot` shows where it will land. A raw
  `pointer -- down … up` gets none of this.
- `omabox pointer [--window SEL | --in SHOT] [--steps N] [--mod MODS] -- move X Y [--steps N], click
  [BTN], down [BTN], up [BTN], scroll DY, hscroll DX, source wheel|finger|continuous|tilt, sleep DURATION` in
  one run: raw, it raises nothing (a `--window` must be on screen and uncovered). BTN is left (the default), right, middle, back or forward (a mouse's thumb buttons, also `side` and `extra`; `click X Y back` too). A button pressed with `down` stays
  down after the call, until an `up` (in a later call too): end every `down` with an `up`, or the
  box's next clicks are drags.
- `omabox pointer hide` / `show`: the pointer out of every shot and recording of the box until `show`,
  over reloads and theme switches. It still moves, clicks and gives focus (focus follows it).
- `omabox scroll [--window SEL | --in SHOT] [--wait] [--mod MODS] X Y DY [DX] [--source wheel|finger]`:
  the pointer there, then a scroll; DY down, DX right (negative: up, left), 15 a wheel notch. `--source
  wheel` is a mouse's notches (one per frame: apps counting them see each; a DY that is not whole
  notches is rounded to the nearest, at least one, and said: 20 scrolls 15), `finger` a touchpad's
  smooth scroll ending in a stop (kinetic scrolling); none is one plain event. Hyprland binds see the
  wheel as `mouse_down`/`mouse_up`/`mouse_left`/`mouse_right`, at most one per
  `binds.scroll_event_delay` (300 ms) as from a real wheel; `--mod super` reaches Omarchy's
  SUPER+wheel binds (workspaces, groups). `--source tilt` (a wheel pushed sideways) and `finger` reach
  apps but no Hyprland bind (it binds the wheel source only): test binds with `wheel`.
  Qt apps get every wheel, a real mouse's too, from a device of type TouchPad (Qt's Wayland plugin
  registers no other, QTBUG-112432): a Qt Quick `WheelHandler` at its default `acceptedDevices`
  (Mouse) never fires, in a box or on the desktop. Not omabox's to fix; for the app:
  `acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad`, or a `MouseArea`'s `onWheel`.
- `keys --pass VAR` types the value of a variable from your environment, shown nowhere: a secret
  never goes in `-t TEXT`, which the process list shows.
- `keys -t TEXT` and `--pass VAR` type a newline as Return and a tab as Tab; any other control
  character (backspace, escape, a carriage return) is refused, exit 2, before anything is typed. A
  `--pass` value's one trailing `\r` (a Windows line ending) is dropped.
- **A held key** (`keys down:KEY` … `keys up:KEY`): KEY stays down across the commands between
  (`pointer move`, `click`, other `keys`), for a bind on a key's release (`hl.bind(KEY, fn, {release =
  true})`) or a gesture made while a key is held. One key each (`down:XF86Tools`, `down:super`,
  `down:a`). It joins the other keys as on a real keyboard: `down:super` then `keys a` fires SUPER+A,
  `down:shift` types capitals; a held letter repeats in the focused app until another key is pressed.
  A second `down:` of the same key, or an `up:` of one not held, is exit 1, before any key of that call
  is sent (`keys a up:super` with nothing held types nothing). A holder killed outright leaves its keys
  down: the next `keys` lets go of them, once, and says so (an `up:` of one is that release). A key still held goes with
  the box (`down`, or your agent exiting, for a box named after your session): a box you keep (`up
  --new`) keeps it down until `up:KEY`.
- **Travel** (`--steps N` on `click` and `pointer`; 1-999, 1 is a jump): N moves in a straight line
  from where the pointer is, each seen by Hyprland and drawn before the next (~40 ms apart), so
  focus-follows-mouse and hover happen on the way. Pick N so a step (the distance over N) is narrower
  than the narrowest window to cross. `pointer --steps N -- move A move B` goes through A (a path
  around something: a waypoint beside it); `move X Y --steps N` sets one move's. `click --steps` travels
  first, then clicks: `--wait` watches the click, not the way.
- **Modifiers** (`--mod MODS` on `click`, `drag` and `pointer`: ctrl, shift, alt, super, altgr, or
  several as `ctrl+shift` or `--mod` again): held down on the box's keyboard from just before the
  press (for `pointer`, the whole run) to just after the release, then let go, however omabox ends.
  The app sees them as a real keyboard's (a table's ctrl-click and shift-click selection). SUPER with
  the left button is Hyprland's move window and with the right its resize (Omarchy's binds): the app
  gets no click, and `drag --mod super` moves the window. Shift-click in foot is its own selection,
  not reported to the program in it.

## Waiting, in detail

- `omabox wait [--timeout 10s] [--json] COND`, one condition per call (chain with `&&`): `still
  [--quiet 300ms | --like SHOT] [-g GEOM | --window SEL] [--ignore GEOM]... [--strict]`, `change
  [--since SHOT] [-g GEOM | --window SEL] [--ignore GEOM]...`, `window SEL [--gone | --focused]`,
  `layer NAMESPACE [--gone] [--monitor NAME]` (`omarchy-menu`, `omarchy-notifications`, ...), `job
  [PID] [--gone]` (a `run -d` job's end), `log [PID] RE [-i]` (a line in its log), `log LOG RE [-i]
  [--since OFFSET]` (a new line in a box log: shell, hyprland, ...), `tree TEXT [--window SEL |
  --layer SEL] [--gone] [--shot FILE]`, `until -- CMD` (exit 0 inside the box); `wait for DURATION`
  is a fixed time.
  Options go before or after COND. `window SEL` is satisfied by any window SEL matches, and the line
  names them all (`--focused`: when one of them has focus); only commands acting on one window
  refuse a SEL that matches several. `layer`'s line names the monitor of each one drawn (`at
  0,0 1920x1080 on HEADLESS-2`; a bar on every monitor is each named): which screen a popup opened
  on, without parsing `hyprctl layers`. `--monitor NAME` waits for one on that monitor (`--gone`:
  none left there, after `monitor remove` or `output drop`); `--json` has it in `detail`.
- A label, a state, a sheet: `omabox wait tree TEXT [--window SEL]` (none: the active window) until
  the accessibility tree holds TEXT, matched as `tree --find` matches it (any case; `[STATE ...]` at
  its end: each among the line's states, any order: `'"Dark mode" [checked]'` finds `[focused
  checked]`, never `[unchecked]`), read about every 100 ms (each read given the time left: a hung
  app keeps `--timeout`): satisfied at the first read
  holding it (a toast or a sheet that lasts a second is caught), the line giving that line. `--gone`:
  two reads in a row without it, or its window gone. A 124 says what the tree had instead (the lines
  holding TEXT's name, else why). `--shot FILE` takes a shot the moment it holds. No hand-rolled
  `tree --find` loops.
- `keys`, `click`, `scroll` and `drag` `--wait` take `--expect tree TEXT`: once the screen settled, the
  tree must hold TEXT (in the window the input went to: `--window`'s, `click --find`'s; none: the
  active one) by `--timeout`, else 124 with what it had instead. `omabox click --find Save --wait
  --expect tree 'label "Saved"' && omabox keys ...` stops when the effect is not the one wanted; the
  input was sent either way.
- Did it land, without a shot: add `--tree` to an action's `--wait`. The window's tree (`--window`'s,
  `click --find`'s, else the one under the point, else the active one) is read before the input and
  once the screen settled; the line ends `; no change in the app's tree` or `; the app's tree: N
  changes`, then a line each, two spaces in, 20 at most (`and N more`): `entry "Name": '' → 'hello'`,
  `check box "Beta": +checked -unchecked`, `list item "Pear": +selected`, `label "Idle" → "Saving"`,
  `+ dialog "Confirm" (and 3 inside)`, `- menu "File"`. A password field's text (and any text after
  `keys --pass`) is never shown: "its text changed (not shown)". No tree (a terminal): `; no tree to
  compare (why)`. It costs a read before the input (the box's first: ~3 s), so only when the answer
  replaces a look.
- An app that redraws later, by itself (after a server sync, a timer): `omabox wait --timeout 30s
  change --window APP` (satisfied at the first change there; the window must be on screen), then
  `omabox wait still` for it to finish. Never `wait until -- false` (or `-- true`) as a sleep: the first
  is a fixed wait ending in 124, the second no wait at all.
- A change you cause without an omabox input (a file the shell applies live, an IPC call, a command
  in the box) can be drawn before a `wait change` starts, which then times out: shot first, act,
  then `omabox wait change --since BEFORE.png [-g GEOM]` (satisfied at once if it already differs,
  else at the first change). `omabox wait still --like BEFORE.png` waits until it looks as before
  (a panel closed, a theme back). SHOT: a full-size shot of the screen, a part of it, or a whole
  window (that window is watched); the pointer and a caret alone do not count, a clock does
  (`--ignore` it). Not for an agent inside ai-jail yet.
- A line in a box log: `omabox wait log shell 'RE'` (a plugin's line, a QML warning), `wait log
  hyprland '\[Lua\] ready'`, any of `log`'s logs by name (a `run -d` job's: `wait log [PID] RE`,
  from its first line); `-i` for case, RE as `log --grep` takes it (grep -E; `--grep RE` works too).
  Only a line written after the wait began counts, printed on the `satisfied:` line; a 124 says when
  one was there before (`--since 0` counts the whole log: after `restart-shell`, which begins the
  shell's anew). `--json` has `offset`, where the line found ends: a next `wait log shell RE --since
  OFFSET` takes the lines after it, so two lines in a row are never missed. A log begun anew during
  the wait is read from its start (said). Hyprland's lines come from its rolling log (its file is
  written 8 KiB late): no `--since` there.
- A notification an app sends: `omabox wait notification 'RE'`, RE (grep -E, `-i`) matched against
  its summary and its body (each with its newlines as spaces; not its app name), whatever server
  the box has. Only one sent after the wait began counts (`--since 0` counts the whole log; `--json`'s
  `offset` is the next wait's `--since`, `notification` the record: id, hints, actions). Trigger it
  first, then wait; a 124 says one that came before the wait. ref: Toasts and dock in shell.md.
- A time that really is fixed (a timer under test, a debounce of 2 s): `omabox wait for 2s`, where a
  foreground `sleep` is blocked. Its line says it was a fixed time; not for letting an app settle.
- Time that is part of a video is the only time you let pass, and never with a sleep: `omabox record
  -o cycle.mp4 --for 5s` records the screen for 5 s with no action (a panel cycling on a timer: no
  dummy action, the pointer left where it is); in a take, `omabox record hold 2s` between `record
  start` and `record stop` gives the viewer 2 s to read the screen, measured in the video's own clock
  (its frames' times), so a slow box or a slow script never shortens it. `record mark "scene 2" --hold
  2s` marks, then holds from the mark. `wait still` is no hold: it returns at once on a still screen
  and runs to its timeout under a clock or a spinner.
- `keys`, `click`, `scroll`, `drag` and `run -d` take `--wait [--start 2s] [--quiet 300ms] [--timeout 10s]
  [-g GEOM] [--ignore GEOM]... [--json]`: the screen before the action, a change within `--start` (5 s
  for `run -d`), then `--quiet` with none. `-g` watches only that part of the screen.
- The line then says what changed on the desktop since just before the input, from one Hyprland read
  before and one after (~25 ms): `window opened, focused: zenity "Save as"`, `window opened on
  workspace 2: ...` (where a shot shows nothing), `window closed: ...`, `layer omarchy-menu
  opened|closed` (a menu kept mapped at 1x1 is closed), `focus: ...` or `focus: none`, `foot title now
  "..."`, `workspace now 2`, `... moved to workspace 3`, `... now fullscreen` (or floating, tiled, `out of fullscreen`); 3 at most and
  ~160 characters, then `+N more (omabox events --since OFFSET)`. Since the input, not caused by it:
  a toast or a clock's retitle at the same moment is said too. Typing into a field changes none of
  these: the line is as before. A window that opened and closed within the wait is not seen.
- A 124 adds where the input went and whether anything moved: `the keys went to foot "K" (focused)`
  or `to layer omarchy-menu (it has the keyboard)`, `the click at 960,540 was on zenity "Info" (its
  129,113)` (its own coordinates, as `click --window` takes them) or `on layer omarchy-bar`, with
  `, or possibly on layer omarchy-notifications (it is full height at the right edge: …)` while a
  layer spanning the screen, or its whole height or width, is there with a toast up (it takes input
  only on its toasts; Omarchy 4.5's, mapped with none, is not named then), `no window from pid 1240 yet` (run -d); then `no window, layer or focus changed`, or what did (a window
  opened off screen). When it says both, look (`windows`, `tree`, a shot) before sending again; the
  input was sent. `unknown` (1) adds nothing. The exit code is the screen's, never the desktop's.
- `--json` adds `changes` (every change, up to 50: `window_opened` `{window, focused}`,
  `window_closed`, `layer_opened` `{namespace, at}`, `layer_closed`, `focus` `{from, to}`, `title`
  `{window, from, to}`, `workspace` `{from, to}`, `window_moved` `{window, from, to}`, `window_state`
  `{window, floating, fullscreen}`; a window is `address, class, title, workspace`; more kinds may
  come: ignore unknown ones), `[]` for none, `null` when the desktop could not be read; `target`
  (`kind` window, layer or none, `window`, `namespace`, `at`: what got the input; `certain` false
  when a layer spanning the screen may have taken it instead; on a 124 or with --json); `events_since` (`omabox events --since N`: everything from just before the input). `wait
  still|change` has no action and none of these.
- The screen watched is every monitor of the box. `-g` and `--ignore` are the layout's coordinates,
  as for a plain `click` and `shot -g`; beside `--window`, the window's own (0,0 its top left corner),
  as `click --window` takes X Y (0.9 took the layout's, with a warning). The regions the line names
  are the layout's; a 124's suggested `--ignore` is the window's when `--window` was given. One
  `--ignore` outside the region watched is said. To watch a part of the screen whatever the window,
  `-g` without `--window`.
- `--ignore "X,Y WxH"` (up to 16; `--strict` keeps them) leaves out an animation that never stops. A 124
  "still changing" whose late changes were all in one small region ends with `--ignore "X,Y WxH" if
  that is an animation`: add it if the shot shows a spinner or a glow there, not the app under test.
- A caret blinking in place (a change 4 px thin and 40 long at most, where the last one was) and the
  software cursor (in every frame; it hides on a key press) are not changes; the line says what was
  ignored. A longer thin change (a tab's underline, a progress bar) or a short one that moves is one.
- A blinker is not a change either (#307): a spot 48 px a side at most whose changes brought it back
  to one of its last 16 looks 3 times (a dot blinking or pulsing, a looping spinner, a bar widget's
  animated badge). Its first few changes count, then it is left out: stderr says `X,Y WxH kept coming
  back to how it looked: left out as a blinker`, `--json` has it as `blinking` (`places`: how many).
  It need not be `--ignore`d, and when a re-layout moves it, it is learnt again where it went; a look
  it never had there (a menu over it) is a change. A counter or a progress bar never comes back: a
  change. `--strict` counts carets, the cursor and blinkers.
- Exit 0 satisfied, 124 not in time, 1 unknown (the box went down; an interactive box that is not
  drawn while hidden: an older one, or one whose window confirm-close replaced). Waiting counts as use
  for the idle timeout.
