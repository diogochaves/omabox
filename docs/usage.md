# Using omabox

Every everyday command, testing that touches the desktop, shell plugins, a Hyprland or Omarchy
change, several monitors. Agents get the same from the skill and `omabox help`. Back to the
[README](../README.md).

## Everyday commands

```bash
omabox up                              # headless box, 1920x1080; returns when the bar is drawn
omabox run -d -- ./build/src/myapp     # launch an app inside (detached; prints its pid and log path)
omabox jobs                            # the box's run -d jobs; omabox stop PID stops one, its windows gone
omabox wait log 'listening on'         # until a line in the latest job's log; wait job: until it ends
omabox shot                            # screenshot; prints the PNG path (--fit 2000: scaled down)
omabox windows                         # the box's windows, where they are, what covers them
omabox shot --window myapp             # one window's own pixels, covered or on another workspace too
omabox pixel 40 12                     # the colour there, #rrggbb; shot -g … --zoom 8: 1 px detail
omabox tree --find 'dark mode'         # text, not a shot: check box "Dark mode" [checked] @12,148 1872x26
omabox tray click myapp sync           # a tray menu's row clicked over the box's bus; omabox tray lists them
omabox shot --changed --window myapp   # only what changed since its last shot, cropped
omabox shot --burst 12 --sheet --after -- keys super+space   # a transition in frames
omabox up --clean                      # a desktop where only what is tested moves (no clock, pointer, network)
omabox check run --approve -- ./visual.sh   # a scenario's checkpoints (omabox check point ID) as a baseline
omabox check run -- ./visual.sh        # ...then: same, or what changed (regions, and the tree's moved elements)
omabox record start -o demo.mp4        # a video of the screen; omabox record stop prints the file
omabox record start --monitor all      # ...of every monitor, laid out in one video as shot takes them
omabox record mark "scene 2"           # a mark in the video's clock: demo.marks.tsv and demo.vtt captions
omabox record hold 2s                  # a pause in a take, 2 s of the video's own clock, never a sleep
omabox record stop --sheet             # ...and demo.sheet.png: frames with their times, to check a take
omabox record stop --show-input --cut-idle   # clicks, pointer and keys drawn in, still stretches cut
omabox record -o cycle.mp4 --for 5s    # the screen for 5 s, no action (something that moves by itself)
omabox audio 3                         # an up --audio box (sound nobody hears): did the app make one, how loud
omabox record start --audio            # ...and the video with its sound, in step with the picture
omabox shot --no-pointer               # a still for people (pointer hide: none until pointer show)
omabox output drop [NAME] --for 300ms  # a screen gone and back, as a monitor waking up
omabox gdb [--watch]                   # backtraces of the box's Hyprland: a hang, or a crash
omabox keys super+space                # key combos reach Hyprland binds and the focused app
omabox keys -t 'hello world' Return    # type text, then press a key
omabox keys --wait super+space         # ...and return once the screen has settled (no sleeps)
omabox wait window myapp               # or: still, change, layer NAMESPACE, until -- CMD (--gone too)
omabox click 960 540 [right] [--double]
omabox scroll 960 540 0 30 --source wheel   # sideways (DX), a wheel's notches or a touchpad's
omabox click --window myapp 40 12      # window coordinates; --in SHOT X Y: that shot's pixels
omabox drag --window myapp 10 10 200 80   # press, move, release; --shot FILE while it is held
omabox hyprctl -j clients              # the box's Hyprland, never yours
omabox lua 'hl.get_active_window()'    # Lua in the box's Hyprland, and what it returns (JSON)
omabox run -- busctl --user list       # any command inside the box; exit code passes through
omabox log shell --grep qml -n 20      # the box's logs (Hyprland's by default; -f follows)
omabox events --since 30s --grep urgent   # Hyprland's events, stamped; --mark, --until RE, -f
omabox down                            # kill everything in the box
omabox ls                              # boxes, mode, size, state, plugins
omabox which 12345                     # the box a host pid is in, a bar like yours in pgrep
omabox brief -b box-3                  # ~15 lines for a subagent's prompt: the box, its windows, the rules
```

- **Names**: a box is named after the current git repo. Inside a Claude Code or Codex session (or
  an agent started with `omabox guard exec`) the name also gets the session's id, `myrepo-5cc72cdc`,
  so two agents in one repo each get their own box, and it goes down when its agent exits (not while
  you peek at it or an `omabox run` is going). An agent does not pick up a box you started yourself
  unless told `-b myrepo`. A command the agent runs outside any repo uses its session's box when it
  has one, `omabox down` included. `-b NAME` or `OMABOX=NAME` picks a box; `OMABOX_SESSION=` (empty) turns
  the per-session names off. Two repos with the same directory name need `-b`.
- **Screen**: `--size 3440x1440`, `3440x1440@144`, or `host` for your focused monitor;
  `omabox mode` changes it live; `omabox window --size 760x600` floats one window at that size.
- **Idle**: a headless box goes down after 2 hours with no `omabox` command, peek or `omabox run`
  against it (`--idle 30m`, `--idle 0` for never, or `OMABOX_IDLE`). A `run -d` job does not count.
  A box at `--idle 0` that goes with no agent stays up until `omabox down`: once what started it (a
  script, an agent whose process omabox could not find) is gone, `omabox ls` says `orphan` under IDLE,
  and `omabox down --orphans` takes those down. Nothing does by itself, and a box you start at a
  terminal is never one.
- **Visual checks**: `omabox check run -- SCENARIO` runs a script of yours in a fresh `up --clean`
  box (Omarchy's bar, one theme, a `#ff00ff` wallpaper, the clock frozen, no pointer, network or
  animations) and compares each `omabox check point ID` it takes with a baseline (`--approve`: three
  runs, what most agree on, and what was not the same from one run to the next left out). It names
  the regions that changed and, for an app with an accessibility tree, the elements that moved,
  resized or changed; shots are compared exactly, as users see them, leaving out only specks.
  Baselines are per GPU (an AMD iGPU and an NVIDIA card draw text differently).
- **More**: `--stock-bar` (Omarchy's default bar instead of a copy of yours, which `up` names when it
  is not Omarchy's; `--theme` does not reset it), `--systemd` (a real systemd user manager, for
  plugins that manage a service or schedule alarms), `--audio` (the box's
  own PipeWire with a speaker nobody hears and a silent mic: `omabox audio` and `record --audio` read
  what played; never your speakers), `--from SAVE` (start
  with a HOME kept by `omabox save SAVE`: an app already signed in or set up), `--env KEY=VAL` (for
  the whole box session), `--xwayland`, `--autoreload` (Hyprland reloads its config when a file it
  loaded changes, as on your desktop), `omabox gpu 10` (GPU time of one box's
  processes; on NVIDIA, its driver's own sampling) and `omabox cpu 10` (their CPU, as % of one
  core, and memory; `--app NAME` or `--pid PID` for one). `omabox help` lists every flag,
  `omabox help CMD` (or `omabox CMD --help`) one command's; `--long` adds what each one does.

## Tests that touch the desktop

```bash
omabox run -- ctest --test-dir build --output-on-failure
```

With no box up, `run` starts a throwaway box, mounts the current repo as a **discarded overlay**
(the tests can write `build/Testing/`, your checkout never changes), runs the command and tears the
box down. Notification tests register with the box's bar instead of piling up in yours; tray tests
need a tray in it (`--stock-bar` if your bar omits one). `up`'s options apply:
`omabox run --net isolated --allow 8081 -- ctest ...` keeps tests away from your real local services.
With `-b NAME` the box must be up. With a box already up, `run` uses it, and the repo is read-only
there: for tests that write into the tree, `omabox down` first. The command gets the box's
environment, not your shell's; `--pass NAME` hands it one of your variables, a password too, through
a pipe, never on a command line, and `--env-file dev.env` a whole file of them.

## Shell plugins

```bash
omabox up --plugin ~/code/myplugin          # or an id from ~/.config/omarchy/plugins
omabox restart-shell                          # after editing the plugin
```

The plugin is mounted read-only and turned on in the box's `shell.json` where its manifest says. One
inside the repo you run `up` from (or a same-path `--ro-bind` or `--overlay`) is a link to where it
is instead, as on your desk, so a helper that finds the rest of its repo through `readlink -f` works.
The box's bar has the built-in widgets plus the plugins you mount, nothing else. When that leaves it
with no workspace numbers, it gets Omarchy's: where your plugin's were, or after the menu.
When the shell does not load a plugin (a manifest it refuses, a QML error), `up` and `restart-shell`
say why, with Omarchy's plugin validator's message when it has one; the box comes up anyway.

To check a plugin against your real desk (a bar plugin that hosts others, plugins that read their
own files), `omabox up --desk-shell [--plugin ~/code/myplugin]` mirrors your whole shell: every
plugin in `~/.config/omarchy/plugins` (but omabox's own widget), your `shell.json` as it is, and the other files in
`~/.config/omarchy` your plugins read, copied. Never `api-keys.env` or anything named or holding a
key, token, secret or password, nor your hooks; `up` lists what went in and what did not, and why.

## A Hyprland change

```bash
omabox up patched --hyprland ~/code/Hyprland/build/Hyprland   # your build, never installed
omabox up stock                                               # the installed one, to compare
```

The box runs your build instead of `/usr/bin/Hyprland` (its folder mounted read-only), with the
rest of the box as usual: the Omarchy shell, your bar, `hyprctl` and `hyprpm` from your system (a
warning when their version is not the build's). The build must link a `libaquamarine` soname the
box has (your system's, or omabox's private build); `up` refuses one that does not, or a file that is not an executable
ELF, before the box starts. `omabox ls` and `omabox windows` name the build, so a box on it is never
taken for a stock one. A box runs the compositor's logic for real (layouts, focus, input routing, the
Lua config, IPC, protocols) on a virtual output with virtual input devices; the DRM/KMS backend
(modesetting, real monitors, HDR/VRR, multi-GPU), libinput with real devices and the session and
suspend paths never run in one (the lock screen does, but takes no real password).

## An Omarchy change

```bash
omabox up dev --omarchy ~/src/omarchy      # your Omarchy checkout instead of /usr/share/omarchy
```

The box runs that tree as `omarchy dev link` would, without touching your system: its Hyprland
config, its shell (bar), its `bin/` first on the box's PATH (binds, menus, `omabox run`), and
`OMARCHY_PATH` in a terminal's bash. The tree is mounted read-only at its own path, so an edit shows
after `omabox restart-shell` (the shell) or `omabox hyprctl reload` (the config). It must have
`bin/`, `default/hypr/bootstrap.lua` and `shell/shell.qml`. Files Omarchy installs outside its tree
(`/etc`, systemd units, `/etc/skel`) stay the installed ones. A box never follows your own
`omarchy dev link`: without `--omarchy` it runs the packaged Omarchy.

## Several monitors with one

A headless box can have as many virtual monitors as a test needs, at any size, scale and place, so a
plugin or app can be tried on a desk you do not have: a bar on each screen, windows moving between
them, a portrait or ultrawide screen, mixed scales, a monitor unplugged.

```bash
omabox up --monitor 1080x1920 --monitor 2560x1440,scale=1.6,below   # portrait right, a scaled one below it
omabox monitor list                       # name, mode, scale, position, workspace
omabox monitor add 3440x1440,0,3000       # another, at X,Y in the layout
omabox monitor remove HEADLESS-3          # an unplug: its workspaces move to the others
omabox shot --monitor HEADLESS-4          # one monitor; omabox peek --monitor HEADLESS-4 to watch it
```

The first monitor is `--size`'s. Each one goes right of (or below) the last one made, and moves with
it: `omabox mode` on the first, or the one before it unplugged (an X,Y one stays). Click, drag,
pointer and pixel coordinates are the layout's (`shot` of the whole layout shows where each monitor
is), and so are `wait`'s and `--wait`'s, which watch every monitor. They survive a config reload. A
box rendering on NVIDIA names them WAYLAND-1 (the first), WAYLAND-2, ... instead of HEADLESS-2, -3, ...
A peek window shows one monitor (the first, or `peek --monitor NAME`): one window per monitor, tiled
on the workspace, each showing the agent's pointer and clicks on its own monitor (its keys on all);
the whole layout at once is `omabox shot`.

In a box you drive (`up --interactive --monitor 1080x1920 --monitor 1280x720,below`, or `monitor
add`), each monitor is a window on your desktop, opened on the box's workspace without taking focus.
The windows are the box's layout scaled as a whole, the picture display settings draw: on a
3440x1440 screen that layout (1920x1080, a portrait one right of it, a 1280x720 one below that)
becomes windows of 1008x567, 567x1008 and 672x378, arranged as in the box, centred. Each window is
a view of its monitor: the bar and apps see the size asked for (the main screen's `--size`, 1920x1080
by default, and each SPEC), drawn at the window's scale (0.525 here), so text is small. `monitor add`
and `remove` lay the picture out again. Resizing a window changes its view's scale, not the
monitor; closing it unplugs it. For an exact large resolution at full size use a headless box and
`peek --monitor`. This needs omabox's build of aquamarine (`omabox setup --aquamarine`, whatever
your system's aquamarine has; `monitor add` refuses a box without it): it gives each monitor's
window a class of its own, `aquamarine-omabox-<token>`, with a rule of your Hyprland's for that class
alone, so the window opens on the box's workspace however the add ends, unless your Hyprland reloads
its config (a theme change, a saved config) between that rule and the window's first map: a reload
drops the rule; the pointer lands on the monitor whose window it is in, and your cursor
stays shown from one window to the next. Two adds on one box take turns.
