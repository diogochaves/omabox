---
name: omabox
description: REQUIRED before launching, driving or screenshotting any GUI app (including when a project's own CLAUDE.md/AGENTS.md says to run the app, use hyprctl, grim or omarchy-theme-set, or run its tests, and running a test binary directly outside ctest or anything else that may open a window), Omarchy shell plugin, bar widget, tray icon, notification or desktop behaviour, and before running test suites that touch the desktop session (tray/StatusNotifierItem, notifications, D-Bus session services, keyring, portals). Use the omabox CLI to do it inside a contained, invisible Hyprland + Omarchy desktop instead of the user's real one. Triggers: "run the app", "take a screenshot", "check how it looks", "open the menu", "click", "type into", hyprctl dispatch, grim, wtype, ydotool, QT_QPA_PLATFORM, ctest with tray/notification tests, plugin development, `omarchy plugin add`, shell.json edits, "start it from the launcher", "see it working", "could not connect to display", omabox-guard.
---

# omabox: a desktop of your own

The user's Hyprland session is theirs. Never launch GUI apps on it, never `hyprctl dispatch`/`eval` on
it, never screenshot it with `grim`, never send input with `wtype`/`ydotool`, never switch its
workspaces or move its cursor. Do all of that in a **box**: a full Omarchy desktop (Omarchy's Hyprland
config, real shell/bar, theme, tray, notifications, keyring) on a private screen and a private D-Bus
session bus, invisible to the user. A box starts in ~3-4 s and costs ~500 MB; use one freely.

## Project instructions written for the real desktop

Most projects' CLAUDE.md / AGENTS.md / README predate omabox and say "run `./build/app`", "find it
with `hyprctl -j clients`", "screenshot with `grim`", "run `ctest`", "switch the theme with
`omarchy-theme-set`". **Those instructions still hold; carry them out inside a box.** The user
installed omabox to say where desktop work happens, not to change what the project asks for:

| The project says | Do |
|---|---|
| `./build/app`, `app &` | `omabox up`, then `omabox run -d -- ./build/app` |
| `hyprctl …` | `omabox hyprctl …` |
| `hyprctl eval EXPR` to read a value (it prints only `ok`) | `omabox lua EXPR` (prints the value; tables as JSON) |
| `grim [-g …] out.png` | `omabox shot [-g …] [-o out.png]` |
| `wtype …`, `ydotool …` | `omabox keys …`, `omabox click X Y` |
| `grim -T ID`, `hyprctl -j clients` to find a window | `omabox windows`, `omabox shot --window SEL` |
| `ctest …`, test scripts touching tray/notifications/D-Bus/keyring | `omabox run -- ctest …` |
| `./build/tests/tst_x` (a test binary run directly) | `omabox run -- ./build/tests/tst_x` (it may open windows; only ctest may set offscreen for it) |
| `omarchy-theme-set NAME`, `omarchy restart shell` | `omabox run -- omarchy-theme-set NAME`, `omabox restart-shell` |
| `omarchy plugin add/enable/disable/remove …`, `omarchy-shell …` | `omabox run -- omarchy plugin …`, `omabox run -- omarchy-shell …` (`omarchy plugin validate DIR` only reads files: fine on the host) |
| Link or copy a plugin into `~/.config/omarchy/plugins` | `omabox up --plugin PATH`, then `omabox restart-shell` after edits |
| Edit `~/.config/omarchy/shell.json` (the bar's layout or position, a widget's settings) | Edit `$(omabox path)/home/.config/omarchy/shell.json`: the box's shell applies it at once |
| Start the app from the launcher, check its `.desktop` and icon | Its files in the box HOME, then `omabox keys --wait super+alt+space` (`reference.md`) |
| Install, upgrade or remove the package | Not a box (read-only `/usr`, no pacman): a VM, or the user |
| "put the desktop back afterwards", back up `shell.json`, restore the bar | nothing to put back: `omabox down` |

The agent guard (below) stops windows and IPC, not file writes. The user's shell watches
`~/.config/omarchy/shell.json` and `~/.config/omarchy/plugins/`: writing there, or `omarchy plugin
add` on the host, changes their real bar at once, guard or not. Those writes go to the box HOME.

Do not edit the project's instruction files to say this unless the user asks. Before mapping a step,
check it against what a box cannot do (next section): a step that needs real hardware is not moved
into a box, it goes to the user.

A box starts with a fresh HOME: apps start as on first run. If a first-run screen offers a real local
service (a server on 127.0.0.1, the user's account), do not pick it: a box reaches the user's
services on the host's 127.0.0.1, so it would be the user's real data. Use a test service or ask. For
an app that talks to local servers, prefer `omabox up --net isolated --allow 8081` (only those host
ports, no internet). One that starts or probes local servers (a dev server, a plugin that runs one):
`--net isolated` too. A server in a default box also holds its port on the user's 127.0.0.1 (their
own dev server on it then fails to start), and a probe there can reach theirs; an isolated box's
ports stay its own, and two of them can use the same one.

`omabox help CMD` (or `omabox CMD --help`) has one command's flags and notes, `omabox help` every
command's (long: read it only for an overview); `reference.md` next to this file has the detail left
out here. The box name defaults to the repo's directory name plus your session's id
(`myrepo-5cc72cdc` in a Claude Code or Codex session), so other sessions never share your box or take it down; `omabox ls` shows its
name. It goes down by itself when your agent exits, not on `/clear` or `/resume`: `omabox down` when
you are done, and before `/clear`. In a git worktree the name is the worktree's folder, so a
worktree agent has its own box with no `-b`. Subagents in one checkout share the session's box: for
one of its own, `omabox up --new` prints a free name (`box-3`); pass it as `-b box-3` on every call,
typed out (your shell does not keep a variable between commands). To use a box the user started,
pass `-b NAME` (see `omabox ls`); it goes before the command too (`omabox -b box-3 shot`).

## The loop

```bash
omabox up                                  # headless box, 1920x1080; waits until the bar is drawn
omabox up --new                            # or one of your own: prints box-N, then -b box-N on each call
omabox run -d --wait -- ./build/src/myapp  # launch, detached (log path printed); returns once drawn
omabox run -d --replace --wait -- ./myapp  # after a rebuild: stops the one run -d started, then as above
omabox shot                                # prints a PNG path: Read it to look
omabox windows                             # address, workspace, on screen or covered
omabox shot --window myapp                 # one window's own pixels, even covered or elsewhere
omabox click --window myapp 40 12          # window coordinates (0,0 = its corner, as in its shot)
omabox keys --window myapp --wait -t hi    # focus that window, type, return once it has settled
omabox drag --window myapp 10 10 200 80    # press, move, release (--shot F: a shot while held)
omabox keys --wait super+space             # real key events for binds and apps (SUPER+W in binds = super+w)
omabox keys -t 'hello wörld' Return        # type any Unicode text (layout-aware), then a key
omabox keys --pass PASSWORD Return         # type a secret from your environment: never -t (ps shows it)
omabox click 960 540 [right] [--double]    # layout coordinates, as in the screenshot (--wait too)
omabox click --steps 20 --mod ctrl 960 540 # travel there (hovering what it crosses), then ctrl-click
omabox wait window myapp                   # or --gone; wait layer omarchy-menu; wait cmd -- CMD; wait still
omabox run -- busctl --user list           # any command inside the box (exit code passes through)
omabox log [shell|apps|run|…] [--grep RE]  # the box's logs, Hyprland's by default (-f follows)
omabox events --mark m1                    # then act, then: events --since m1 [--grep RE | --until RE]
omabox down                                # when done: kills everything in the box
```

## Driving an app

1. **Look, act once, look again.** Exit 0 from `keys`/`click` means sent, not landed: a shot (or
   the line `--wait` prints) says what happened.
2. **Never resend input you have not seen land.** Every call is delivered; sending again types the
   text twice or toggles back. 124 after `--wait` means it WAS sent and the screen did not settle as expected: shot first.
3. **Type only into a field you have seen focused** (a caret in the last shot), or `keys --window
   SEL`. Focus does not always come back (after a panel closes, say).
4. **Set state directly; keys and clicks only when the gesture is under test.** A shell plugin's
   panel: `omabox run -- omarchy-shell shell summon PLUGIN_ID` (`hide`, `toggle`); data files seeded
   in the box HOME; `omabox run -- omarchy-theme-set NAME`; `omabox hyprctl dispatch`. Setup that
   took many steps (signed in, a PIN, a first-run wizard): `omabox save NAME` once, then start from
   it with `omabox up --from NAME` or `omabox run --from NAME -- …` (`reference.md`).
5. **Stop when a shot shows the goal.** Report what a box cannot show (below), then `omabox down`.

No `sleep` between actions: `--wait` returns once what the action caused has settled; `omabox wait`
waits for a window, a layer, a command or a still screen (0 yes, 124 not in time, 1 cannot tell).
Late content passes `still`: wait for a title (`wait window title:RE`) or `wait cmd -- …`. A shot
right after a `click` or `keys` without `--wait` can show the frame before the redraw: use `--wait`,
or `omabox wait still`, before the shot.

`--window SEL`: `myapp` is a class, its last part (`nautilus` for `org.gnome.Nautilus`) or part of a title; `title:RE`, `class:RE`, `pid:N` or an address
(`0x…`) narrow it; `wait window SEL` is satisfied by any window it matches. Coordinates are screenshot pixels; a cropped or scaled shot says so on stderr:
then `click --in SHOT X Y`, X Y read from that image, no arithmetic of your own. After any `-g` or
`--fit` shot, click (or `drag`) with `--in THAT.png`, and never discard `shot`'s stderr: it says so.
`shot --window SEL -g "X,Y WxH"` crops the window in its own coordinates. 1920x1080 is read 1:1; on a
bigger screen (a "multiply by" note, or over 2000 px) `shot --fit 2000` and `click --in` it. Screen
shots show the pointer (hover evidence: a `-g` crop of the screen); window shots never do.

**The pointer is test state.** Under Omarchy's focus-follows-mouse the window the pointer rests on,
or last passed over, takes focus, and gets it back when a menu or panel closes. A box's pointer starts
at the screen's centre and stays wherever the last command left it (`omabox hyprctl cursorpos`).
`click` and `pointer -- move` jump: they cross nothing on the way. Before a test whose result depends
on focus, put the pointer where a user's would be (`omabox pointer -- move X Y`), and when the way
there matters (to the bar, across other windows) travel: `click --steps 20 X Y`, `pointer --steps 20
-- move X Y`. `--mod ctrl` (shift, alt; `ctrl+shift`) holds modifiers across a click or drag; SUPER
with a button is Hyprland's own (move, resize), never the app's.

| Symptom | Next step |
|---|---|
| "could not connect to display", `omabox-guard` | The agent guard: do it in a box (below). |
| Your own shell tool died after `pkill -f PATTERN` | The pattern matched its command line: kill by PID, or `omabox run -- pkill -x NAME`. |
| After a rebuild the app still shows the old build (a single-instance app raised the old window) | Restart it with `omabox run -d --replace --wait -- CMD`, not a kill and a new `run -d`. |
| `unsatisfied: nothing changed` (124) after `--wait` | Shot; right window focused (`omabox windows`)? Do not resend. |
| `click --wait` 124 on a checkbox or small toggle | A change under the cursor (from ~16 px above and left of the click to ~48 px below and right) is ignored as the cursor: shot, do not click again. |
| Text went to the wrong window | `keys --window SEL`, or click the field and see it focused. |
| A click missed a cropped or scaled shot | `click --in THAT.png X Y`. |
| The window is not in the shot (covered, other workspace) | `shot --window SEL`; `click --window` raises it. |
| `unknown: … not rendered` (exit 1) | An interactive box started by an older omabox, or whose window confirm-close replaced, is not drawn while hidden: ask the user; never show its window yourself. |
| `box 'x' is already up, without what you asked for: …` (exit 1) | It lacks those options. Yours: `omabox down` it, then `up` again. Not yours: `omabox up --new`, then `-b box-N` as it printed. |
| `setsid: failed to execute APP` | The box has the host's programs only (`foot`, not `alacritty`). |
| Tray items that stay after their process exits; no tray at all | Quickshell bug: tray tests in a throwaway box (`omabox run`, no box up); `--stock-bar` if the user's bar has no tray. |

## Tests that touch the desktop

```bash
omabox run -- ctest --test-dir build --output-on-failure
```

With no box up, `run` starts a throwaway box with the current repo as a **discarded overlay** (writes
succeed, the checkout never changes), runs, tears down: tray and notification tests register with the
box's bar, not the user's. Use it for any test that talks to the session bus, the tray, notifications,
the keyring or a compositor; tests that talk to 127.0.0.1 with `omabox run --net isolated --allow
PORTS -- ctest ...` (`up`'s options work here). With a box already up, `run` uses it and the repo is
**read-only** there: a test that writes into the tree fails; `omabox down` first.

`run` gives the command the box's environment, not your shell's: a variable a test needs (a test
server's password from `dev.env`, say; a test that skips is the sign) goes with `--pass NAME`, off
the command line, or a whole file with `--env-file`: `omabox run --env-file ./dev.env -- ctest …`
(KEY=VAL lines, read as data). Never `--env KEY=secret`: that is in the process list. A `run -d`
command's output, a Qt app's warnings and QML errors too, is in the log file it prints (`-q`: no
line; `--print-log`: only the path, on stdout, for a script).

A test binary run directly (not through ctest) has none of ctest's environment: a Qt test with no
`QT_QPA_PLATFORM=offscreen` opens real windows. Run it with `omabox run -- ./build/tests/tst_x`.
Check what a keyring or D-Bus test left behind **inside the box** (`omabox run -- secret-tool …`),
never with `secret-tool` on the host: that is the user's real keyring, and `search --all` prints the
secrets themselves.

## Inside ai-jail

In an ai-jail sandbox omabox works through the user's broker: same commands, with a few limits (your
boxes have no network if the jail has none; only the jail's project is mounted; no `host`, `peek` or
`--interactive`). "cannot start a box here: this sandbox refuses new namespaces" means the broker is
not set up: ask the user to run `omabox broker on` outside the jail and add the lines it prints to
`~/.ai-jail`, then restart the jail. Never try to get around the jail.

## "could not connect to display" / `omabox-guard`

The user may have turned on the agent guard: your shell commands get `WAYLAND_DISPLAY=omabox-guard`,
an empty `DISPLAY` and `HYPRLAND_INSTANCE_SIGNATURE=omabox-guard` (`QT_QPA_PLATFORM=offscreen` still
works), so anything that would have reached the real desktop fails instead (a Qt app aborts saying
"could not connect to display", hyprctl cannot connect). That error means: do it in a box. Never set those variables back to the real session, and
never take the display from elsewhere (`/proc/*/environ`, `hyprctl instances`,
`$XDG_RUNTIME_DIR/wayland-*`). omabox itself keeps working. Opening a link or file on the user's
desktop (`xdg-open`, `gh … --web`, anything using `$BROWSER`) fails too ("omabox guard: not opening"):
give the user the link. To look at a page yourself, open it in your box (`omabox run -d -- xdg-open
URL`, then `omabox shot`).

When the user asked for their **real** desktop in this task ("switch my theme", reload my Hyprland
config after an edit, see the change on my screen), run that one command with `omabox host -- CMD`
(e.g. `omabox host -- hyprctl reload`, `omabox host -- omarchy-theme-set NAME`). Only then: it is
the one way past the guard, and it is on the record. Testing, screenshots and anything the user did
not ask to see on their desktop stay in a box. Never use it to switch the user's workspace or focus
so you can see a box: `shot` works on a hidden interactive box, and when `shot` gets no frame, ask
the user (see Showing the user).

## Omarchy shell plugins

```bash
omabox up --plugin ~/code/myplugin            # or --plugin <id> from ~/.config/omarchy/plugins
omabox restart-shell                           # after editing the plugin (the mount is live, read-only)
```

The box's `shell.json` has only built-in widgets plus the plugins you mount, each enabled where its
manifest says. It copies the user's bar layout (Omarchy's workspace numbers in place of a plugin's
that is left out); `omabox up --stock-bar` uses Omarchy's default bar
instead (workspaces, clock, the stock right side), to see a plugin as most people will. When the
shell does not load a plugin, `up` and `restart-shell` print why (`warning: plugin ID failed: <QML
error>`, or Omarchy's validator's message): read that before looking for the widget in a shot.
Settings, placement, a vertical bar, data states, processes the plugin leaves behind and the
install path (`omarchy plugin add` in the box): `reference.md`, "Testing a shell plugin". Do not mount plugins you were not asked to test (some talk to real services).

## A Hyprland change

`omabox up --hyprland ~/code/Hyprland/build/Hyprland` runs that build in the box instead of the
installed Hyprland (never install it, never run it on the host): before/after of a compositor fix in
two boxes. It must link the libaquamarine soname omabox provides (`up` refuses it otherwise, saying
which); hyprctl stays the installed one (`up` warns when the versions differ). `omabox ls` and
`windows` name the build. A box proves layouts, focus, input routing, the Lua config, IPC and
protocols; not DRM/KMS, real monitors, HDR/VRR, multi-GPU, real input devices, suspend or lock
(`reference.md`).

## An Omarchy change

`omabox up --omarchy ~/src/omarchy` runs that Omarchy checkout in the box instead of
`/usr/share/omarchy` (as `omarchy dev link` does on a host, without touching the user's): its
Hyprland config, shell, `bin/` and `OMARCHY_PATH`. Never `omarchy dev link` on the host to test a
change. Edits show after `omabox restart-shell` or `omabox hyprctl reload`. To review a pull
request (two boxes, the change and the base; UI that needs hardware): `reference.md`, "Reviewing an
Omarchy change".

## What is and is not in a box

- Read-only in the box: the repo you ran `omabox up` from (same path), `--ro-bind` and `--plugin` dirs,
  mise's toolchains; nothing else of the user's HOME. Mounting HOME, `~/.config/omarchy` or `/tmp` (or
  a dir containing them), secret stores (`~/.ssh`, keyrings...), `/run` or the runtime dir is refused:
  do not work around it.
- The box HOME is fake: `/home/sbx` inside, `omabox path` → `<dir>/home` on the host (seed a widget's
  data files there). `/home/sbx` does not exist on the host: a path under it passed to a service
  running on the host (a download dir sent to a local server) fails there. Use a path both can see.
- Private session bus and keyring (store/lookup secrets freely), no system bus, no real input
  devices, no audio. Each box has its own network: by default it reaches the internet, the LAN and
  the user's servers on the host's 127.0.0.1 (leave those alone unless asked); `--net isolated`
  reaches only the host ports you list. Across the box boundary use `127.0.0.1`, not `localhost`
  (detail: `reference.md`).
- `/sys` and system-wide `/proc` files are the host's (read-only): CPU, temperatures, memory, disks,
  USB devices and DRM connectors read as the real machine's. A widget reading those shows host
  hardware state, not box state.
- Only the host's programs; no Xwayland unless `--xwayland`. Stub CLIs, `--env`, `--systemd`, the XDG
  dirs, logs, cores, `--no-shell`, screen size: `reference.md`.

## Showing the user

Only when the user asks: `omabox peek` (a live, view-only window of your box on their workspace 9,
without focus; your input shows on it for ~3 s, never in your shots) or `omabox up --interactive` (a
box they drive). Agents use headless boxes. `shot`, `click` and `keys` work on an interactive box
while its window is hidden: never bring that window forward yourself; when `shot` gets no frame, ask
the user. A box the user started has its own name (the repo's, or box-N from the bar widget): pass
`-b NAME`. `omabox config` holds the user's settings: change them only when asked. `omabox clip`
(the user's clipboard into or out of a box) refuses agents, `omabox host` included: never try it or
work around it; the user runs it themselves (a key binding, the bar widget), or you put the text
into the box yourself (`keys -t`, `run -- wl-copy`).

## When a box cannot test it: real hardware and the real session

A box is a desktop without hardware. It has:

- one **virtual screen** (any size and refresh rate, scale 1): no real monitor, so no real modes, HDR,
  VRR, 10-bit, colour management, scale, multi-monitor, hotplug or DPMS;
- no **system bus**: no NetworkManager, bluetooth, UPower/power profiles, udisks, logind, polkit;
- no **devices**: no audio (PipeWire), no `/dev/i2c` (DDC/CI monitor brightness), no backlight, no
  real keyboards/mice/touchpads/tablets, cameras, USB, printers;
- no **session integration**: no systemd user manager unless `omabox up --systemd` (and never
  journald or logind), no installed .desktop files/URL handlers, no lock/idle/suspend, and a fresh
  HOME instead of the user's data.

So before testing, look at what the change touches: the project's code and instructions. Signals:
`hl.monitor`/`monitors.lua`/`hyprctl … monitor`, refresh/HDR/VRR/bitdepth/`cm`, `ddcutil`, backlight,
`wpctl`/`pactl`, `nmcli`, `bluetoothctl`, `powerprofilesctl`, `udisksctl`, `journalctl`, `loginctl`,
input device config, `/dev/…`, the user's real accounts, servers or data. If what you must verify
depends on any of those, a box cannot verify it; do not run it there and report it as tested.
(Units and timers, `systemctl --user`, `systemd-run --user`: use `omabox up --systemd`.)

- **Split the work.** Do in a box what a box can show (a panel's layout and text, an app's UI with
  fake data, logic, unit tests), and name the part that needs the real machine.
- **Ask once before touching the real desktop.** Say exactly what will change (e.g. "switch your
  monitor to 120 Hz for 15 s, then revert"), wait for a yes, and treat that yes as covering the rest
  of this task, not the next one. The user can also start with "test on my real desktop".
- Under the agent guard, those commands go through `omabox host -- CMD`.
- On the real desktop, follow the project's own safety rules (revert timers, previews) and put
  everything back.

## Reporting what a box showed

Say where: "in an omabox box (Omarchy 4.0.4-1, Hyprland 0.56.2, 1920x1080, scale 1, theme NAME)",
with the commit of the plugin or app you tested. `omabox ls --json` has them: `omarchy_version`,
`theme`, `hyprland_version`, and each plugin's `commit` in `plugin_status` (`+dirty`: uncommitted
edits). A box runs the real Hyprland with Omarchy's config, shell, binds and theme, so layout, focus,
input routing, the launcher, theme switches, notifications, the tray, the session bus and the keyring
were tested, not just rendered. Not tested, and said so: real monitors, scale, several outputs, input
methods, the system bus, devices, the installed package, the user's own HOME. Never write "tested on
the desktop" or "on Omarchy" for a box result without "in a box".

## If something is off

A headless box goes down after 2h with no omabox command against it (`up --idle 0` keeps one,
`--idle 30m`); the next command says so: `omabox up` again. Your session's box still goes when your
agent exits, `--idle 0` or not; a box with another name (`-b NAME`) stays. A `run -d` job is not use:
a server you only poll over HTTP needs `--idle 0`. `omabox ls` shows boxes and whether they are alive
(`down --all` takes other agents' and the user's too). A box that fails to start prints where its logs are. Details and known quirks: `NOTES.md` in the omabox repo
(`readlink -f $(command -v omabox)` → `../NOTES.md`).
