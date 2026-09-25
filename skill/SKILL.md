---
name: omabox
description: REQUIRED before launching, driving or screenshotting any GUI app (including when a project's own CLAUDE.md/AGENTS.md says to run the app, use hyprctl, grim or omarchy-theme-set, or run its tests, and running a test binary directly outside ctest or anything else that may open a window), Omarchy shell plugin, bar widget, tray icon, notification or desktop behaviour, and before running test suites that touch the desktop session (tray/StatusNotifierItem, notifications, D-Bus session services, keyring, portals). Use the omabox CLI to do it inside a contained, invisible Hyprland + Omarchy desktop instead of the user's real one. Triggers: "run the app", "take a screenshot", "check how it looks", "open the menu", "click", "type into", hyprctl dispatch, grim, wtype, ydotool, QT_QPA_PLATFORM, ctest with tray/notification tests, plugin development, "see it working", "could not connect to display", omabox-guard.
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
| `grim [-g …] out.png` | `omabox shot [-g …] [-o out.png]` |
| `wtype …`, `ydotool …` | `omabox keys …`, `omabox click X Y` |
| `ctest …`, test scripts touching tray/notifications/D-Bus/keyring | `omabox run -- ctest …` |
| `./build/tests/tst_x` (a test binary run directly) | `omabox run -- ./build/tests/tst_x` (it may open windows; only ctest may set offscreen for it) |
| `omarchy-theme-set NAME`, `omarchy restart shell` | `omabox run -- omarchy-theme-set NAME`, `omabox restart-shell` |
| "put the desktop back afterwards" | nothing to put back: `omabox down` |

Do not edit the project's instruction files to say this unless the user asks. Before mapping a step,
check it against what a box cannot do (next section): a step that needs real hardware is not moved
into a box, it goes to the user.

A box is a fresh desktop with a fresh HOME: apps start as on first run. If a first-run screen offers
to use a real local service (a server on 127.0.0.1, the user's account), do not pick it: boxes share
the host network, so it would be the user's real data. Use a test service or ask. For an app that
talks to local servers, prefer `omabox up --net isolated --allow 8081` (only the listed host ports,
no internet): then the real service cannot be reached by mistake.

`omabox help` has every flag. The box name defaults to the current git repo's directory name, so
agents in different repos share a box only if the names match; pass `-b NAME` (or set `OMABOX=NAME`)
for more than one, or when another repo may have the same name.

## The loop

```bash
omabox up                                  # headless box, 1920x1080; waits until the bar is drawn
omabox run -d -- ./build/src/myapp         # launch inside the box (detached; log path printed)
omabox shot                                # prints a PNG path: Read it to look
omabox shot --active                       # just the focused window
omabox keys super+space                    # Hyprland binds and the focused app get real key events
                                           # (SUPER+W as written in binds = super+w; a bad token sends nothing)
omabox keys -t 'hello wörld' Return        # type any Unicode text (layout-aware), then a key
omabox click 960 540 [right] [--double]    # layout coordinates, as in the screenshot
omabox hyprctl -j clients                  # the box's Hyprland, never yours
omabox run -- busctl --user list           # any command inside the box (exit code passes through)
omabox down                                # when done: kills everything in the box
```

Look at the screenshot after every action that should change the screen; do not assume. Coordinates
are screenshot pixels (scale 1). `--size 3440x1440` for another screen size (@60), `--size
3440x1440@144` for a refresh rate, `--size host` for the user's own monitor; `omabox mode` shows or
changes it on a running box.

**Measuring rendering cost** (GPU time of an animation, a repaint loop): `omabox up --size host`,
put the UI in the state to measure, then `omabox gpu 10` (% of wall time per process, this box
only; `--json`). Never read host-wide tools (nvtop, radeontop, scripts summing `/proc/*/fdinfo` by
name) while a box is up: they add the box's Hyprland and quickshell to the user's. Match the mode:
anything that repaints per frame costs ~2.4x more at 144 Hz than at the default 60. NVIDIA driver
615.71.09 does not expose the per-process DRM counters this command needs; on that driver `gpu`
reports no percentages.

## Tests that touch the desktop

```bash
omabox run -- ctest --test-dir build --output-on-failure
```

With no box up, `run` starts a throwaway box, mounts the current repo as a **discarded overlay**
(the command can write build/Testing/ logs; the checkout never changes), runs, tears down. Tray and
notification tests then register with the box's Omarchy bar instead of leaking into the user's. Use it
for any test that talks to the session bus, the tray, notifications, the keyring or a compositor.
`up`'s options work here for the throwaway box: tests that talk to 127.0.0.1 should run with
`omabox run --net isolated --allow PORTS -- ctest ...` so they cannot reach the user's real services.
With a box already up, `run` uses it and the repo is **read-only** there: a test that writes into the
tree fails. `omabox down` first, then `run`.

`run` gives the command the box's environment, not your shell's: a variable a test needs (a test
server's password from the project's `dev.env`, say) goes with `--pass NAME`, which takes it from your
shell without putting it on a command line: `set -a; . ./dev.env; set +a; omabox run --pass
TEST_PASSWORD -- ctest …`. A test that skips when a variable is missing is the sign.
Never `--env KEY=secret`: that is on the command line, in the process list.

A test binary run directly (not through ctest) has none of ctest's environment: a Qt test with no
`QT_QPA_PLATFORM=offscreen` opens real windows. Run it with `omabox run -- ./build/tests/tst_x`.
Check what a keyring or D-Bus test left behind **inside the box** (`omabox run -- secret-tool …`),
never with `secret-tool` on the host: that is the user's real keyring, and `search --all` prints the
secrets themselves.

## "could not connect to display" / `omabox-guard`

The user may have turned on the agent guard: your shell commands get `WAYLAND_DISPLAY=omabox-guard`,
an empty `DISPLAY` and `HYPRLAND_INSTANCE_SIGNATURE=omabox-guard` (and an empty `QT_QPA_PLATFORMTHEME`,
so `QT_QPA_PLATFORM=offscreen` still works, as under ctest), so anything that would have reached the
real desktop fails instead (a Qt app aborts saying "could not connect to display", hyprctl cannot
connect). That error means: do it in a box. Never set those variables back to the real session, and
never take the display from elsewhere (`/proc/*/environ`, `hyprctl instances`,
`$XDG_RUNTIME_DIR/wayland-*`). omabox itself keeps working.

When the user asked for their **real** desktop in this task ("switch my theme", reload my Hyprland
config after an edit, see the change on my screen), run that one command with `omabox host -- CMD`
(e.g. `omabox host -- hyprctl reload`, `omabox host -- omarchy-theme-set NAME`). Only then: it is
the one way past the guard, and it is on the record. Testing, screenshots and anything the user did
not ask to see on their desktop stay in a box.

## Omarchy shell plugins

```bash
omabox up --plugin ~/code/myplugin            # or --plugin <id> from ~/.config/omarchy/plugins
omabox restart-shell                           # after editing the plugin (the mount is live, read-only)
```

The box's `shell.json` has only built-in widgets plus the plugins you mount, each enabled where its
manifest says. It copies the user's bar layout; `omabox up --stock-bar` uses Omarchy's default bar
instead (workspaces, clock, the stock right side), to see a plugin as most people will. Do not mount plugins you were not asked to test (some talk to real services).

## What is and is not in a box

- The repo you ran `omabox up` from is visible **read-only** at the same path, plus any dirs listed in
  `~/.config/omabox/ro-bind` or passed with `--ro-bind` (`DIR:DEST` for another path, e.g. testing
  path mapping with `--ro-bind ~/nas:/mnt/nas`), mise's toolchains (`omabox run` keeps your PATH,
  so node/python/uv are the host's), `--plugin` dirs and the user's git name/email; nothing else of
  the user's HOME. Mounting HOME, `~/.config/omarchy` or `/tmp` (or a dir containing them), secret
  stores (`~/.ssh`, keyrings...), `/run` or the runtime dir is refused: do not work around it. Outside a repo a throwaway `run`
  mounts nothing of the current dir. The box HOME
  is fake (`omabox path` → `<dir>/home`, readable and writable from the host): put outputs there or in
  `/tmp` inside, and seed a widget's data files (a usage record, a store) there while the box runs.
- Private session bus, private keyring (store/lookup secrets freely, no prompts), no system bus, no
  real input devices, no audio. Network is shared with the host (so the user's real local services
  are reachable: leave them alone unless asked) unless the box was started with `--net isolated`.
- `/sys` and system-wide `/proc` files are the host's (read-only): CPU, temperatures, memory, disks,
  USB devices and DRM connectors read as the real machine's. Only processes and the screen are the
  box's. A widget reading those shows host hardware state, not box state.
- The box HOME is `/home/sbx`, which does not exist on the host: a path under it passed to a service
  running on the host (a download dir sent to a local server) fails there. Use a path both can see.
- Tray tests leave ghost items in Omarchy's bar when their processes exit fast (a quickshell watcher
  bug): run them in a throwaway box (`omabox run` with no box up), not in one you screenshot later.
- The session's PATH is yours (mise's tools, as in `omabox run`) with the box HOME's `~/.local/bin`
  first: drop a stub CLI there to fake one a plugin calls. `--env KEY=VAL` on `up` sets a variable for
  the whole session (the bar included), e.g. a plugin's API base pointed at a stub.
- `omabox up --systemd` gives the box a real systemd user manager: `systemctl --user`, units in the box
  HOME's `~/.config/systemd/user`, `systemd-run --user` timers (use it for plugins that manage their
  own service or schedule alarms). No journald (`journalctl --user` is empty) and no logind either way.
- No Xwayland unless `omabox up --xwayland`. Without `--systemd` no systemd user manager. Omarchy's
  `uwsm-app` launching always goes through a stand-in, `--systemd` or not: apps start as plain
  processes, not units (output in `<box dir>/home/apps.log`). Apps that need system services
  (udisks, NetworkManager, bluetooth) cannot work in a box.
- A crash in a box leaves no core file and no crash notification on the user's desktop (the core
  limit is 1 byte). To get a core: `omabox run -- bash -c 'ulimit -c unlimited; exec ./app'`.
- `--no-shell` starts Hyprland only (no bar, tray or notifications): faster for plain app work.
- Logs: `<box dir>/home/*.log` (shell, keyring, labwc, runs), Hyprland's in `<box dir>/run/hypr/*/hyprland.log`,
  bwrap's in `<box dir>/box.log`.

## Showing the user

- `omabox peek` opens a live, view-only window of your headless box on the user's workspace 9 (or the
  one they set with `omabox config workspace`) without
  taking focus. Only when the user asks to watch; it does not affect the box.
- `omabox up --interactive` makes the box a real window on that workspace that the user drives (SUPER+ALT+ESCAPE
  sends SUPER keys to it). Only when the user asks for it. `shot` does not work while that window is
  hidden; agents use headless boxes. `omabox config` holds the user's settings: change them only when
  the user asks.

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

## If something is off

A headless box goes down by itself after 2h with no omabox command against it (`omabox up --idle 0`
keeps one; `--idle 30m` for another timeout); the next command then says so: `omabox up` again. A
`run -d` job is not use: a server you only poll over HTTP needs `--idle 0`.
`omabox ls` shows boxes and whether they are alive; `omabox down --all` clears them. A box that
fails to start prints where its logs are. Details and known quirks: `NOTES.md` in the omabox repo
(`readlink -f $(command -v omabox)` → `../NOTES.md`).
