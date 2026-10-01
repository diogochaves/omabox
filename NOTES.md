# omabox: design notes and findings

How omabox works and why it is built the way it is. The README says how to use it; this is for
whoever changes it, person or agent.

- **Architecture**: the process tree of a box, and how the host drives it.
- **Findings**: everything learned while building it, numbered in the order found: the traps
  (a nested Hyprland, input, D-Bus, sandboxing), the safety rules and why each exists, the bugs the
  review passes found and how they were closed. Code comments and tests cite them as "finding N",
  so a number never changes and never goes away (47 and 71 were each given twice by mistake; both
  entries stand). A change of behaviour adds a finding.
- **Dead ends**: what was tried and dropped, so nobody tries it again.

omabox started on 2026-09-23 as a spike, a set of hand-run scripts kept in `spike/`, and became the
CLI the same day. The spike ran on Omarchy 4.0.4, kernel 7.2.5, Hyprland 0.56.2, aquamarine 0.15.0
(system), quickshell 0.3.1, labwc 0.20.2, wlroots 0.20.2, bubblewrap 0.12.0 and wayvnc 0.10.1 (gone
since finding 34), on an AMD iGPU (`/dev/dri/renderD128`); a discrete GPU with no render node was
not used. Findings from before the CLI still describe the spike's way of doing things; later ones
say what replaced it.

## Architecture

```
$XDG_RUNTIME_DIR/omabox/<name>/   box dir: run/ (the box's /run/user/$UID), home/ (-> ~/.cache/omabox/<name>/home),
                                  box.json (options, net, pidns), info.json (bwrap child-pid), pid + pasta.pid
                                  (boxes behind pasta), used (idle clock), launch.sh, box.log, reap.log,
                                  shots.tsv (what each shot is of, for click --in: 81)
[systemd-run --user --scope]      --systemd only: a delegated cgroup the box's user manager owns (61)
[pasta]                           top-level boxes: own netns, connected (default) or isolated (44, 45, 89);
                                  a nested connected box gets bwrap's --unshare-net instead (net: none)
bwrap sandbox            fake HOME=/home/sbx, private /run/user/$UID and /tmp, pid/ipc/uts namespaces
│                        binds: /usr /etc /sys ro, the repo + ro-bind file + --ro-bind ro, mise installs ro,
│                        --hyprland's folder ro (116),
│                        one render node (plus its NVIDIA render-side nodes on NVIDIA);
│                        an interactive box every render node (95),
│                        share/ → /opt/omabox/share, a private aquamarine (125) →
│                        /opt/omabox/lib, keyboard/pointer → /opt/omabox/bin, --plugin dirs ro →
│                        ~/.config/omarchy/plugins/<id>, --overlay dirs (discarded writes); every source
│                        and DEST checked (refuse_src/refuse_dest, finding 63)
└ share/session.sh       the session (bwrap's init is PID 1): private session bus (dbus-daemon, or
  │                      systemd's dbus.service with --systemd), gnome-keyring, PATH, env, then:
  ├ [systemd --user]     --systemd only
  └ labwc -S (headless)  invisible parent compositor (WLR_BACKENDS=headless, 1 output); ends with Hyprland
    └ Hyprland (nested)  real Omarchy config minus autostart; LD_LIBRARY_PATH → patched aquamarine
                         when one is used (finding 125);
      │                  /usr/bin/Hyprland, or --hyprland's build (116)
      ├ HEADLESS-2       screen on AMD/Intel; WAYLAND-1 bootstrap disabled
      │ WAYLAND-1        screen on NVIDIA; labwc's private headless output is resized to --size
      ├ quickshell       the Omarchy shell (bar, menu, tray host, notifications): share/shell.sh;
      │                  not with --no-shell
      ├ keyboard, pointer --hold: idle devices so focus changes work (41)
      └ apps             `omabox run [-d]` (nsenter into the namespaces, Hyprland's env)
interactive: no labwc; Hyprland nests in the host Hyprland through one fd (tools/wlfd, WAYLAND_SOCKET),
             launched by the host's hl.exec_cmd with rules → workspace 9 silent; WAYLAND-1 is the screen
```

Driving it from the host is `omabox` (`omabox help`): `hyprctl`, `grim` (shot), the keyboard, the
pointer and `omabox-still` (`wait`, finding 82) all run inside the box's mount namespace (`nsenter -U -m`), so they only ever see the box's
sockets (finding 63). The spike's manual way (a symlinked runtime dir, `wtype`, the pointer run from a
host shell) is history and unsafe: `wtype` sends the wrong keys (13), and a tool run from a host shell
drives the real desktop; both tools now refuse to run outside a box.

## Reproduce on a fresh Omarchy install

`./install.sh` (add `--check` to start a box, screenshot it and tear it down). Idempotent; sudo only
if packages are missing. Verified from scratch on this machine (build/ removed, tools cleaned): clones
and builds aquamarine at the pinned commit, checks Hyprland still links the soname the private build
provides, builds `tools/`, links `~/.local/bin/omabox` and `~/.claude/skills/omabox`: 17 s, check passed.
Then: `omabox up && omabox shot`. It also links the bar widget (`plugin/`) as
`~/.config/omarchy/plugins/chaves.omabox`, off until `omarchy plugin enable chaves.omabox` (finding 40).

What it does, step by step (each is safe to repeat; `install.sh` is the source of truth):

1. Packages (`PKGS` in install.sh): labwc, bubblewrap, passt, jq, grim, gnome-keyring, the tools'
   build deps (wayland, libxkbcommon, base-devel), aquamarine's build deps, and what the box runs
   that Omarchy already has (quickshell, gtk3, xdg-terminal-exec, dbus). `sudo pacman -S --needed`
   only for the missing ones.
2. aquamarine's fix into a private prefix, `omabox setup --aquamarine` (only headless NVIDIA boxes
   and confirm-close need it, finding 125; a no-op once the system's aquamarine is past 0.15.1):
   ```
   git clone https://github.com/hyprwm/aquamarine build/aquamarine
   git -C build/aquamarine checkout 7bb8bdf4      # "wayland: fix configure not applying sometimes (#415)"
   cmake -S build/aquamarine -B build/aquamarine/out -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=$PWD/build/prefix
   cmake --build build/aquamarine/out && cmake --install build/aquamarine/out
   ```
   (installed beside the old one, then swapped in). It must provide the soname the installed Hyprland
   links (`ldd $(command -v Hyprland)`), or `up` skips it for the system's; only the nested Hyprland
   loads it. Outside a checkout it goes to `~/.local/share/omabox/aquamarine`.
3. Tools: `make -C tools/pointer`, `keyboard`, `wlfd`, `peek`, `still`, `relay`, `events`
   (need `wayland-scanner`; protocol XML is vendored).
4. `omabox setup` (finding 126): links `~/.local/bin/omabox` → `bin/omabox`; `skill/` as
   `skills/omabox` in `~/.agents` and `~/.claude` (and `~/.codex`, `~/.pi/agent`, `~/.hermes` when
   those exist); `plugin/` as `~/.config/omarchy/plugins/chaves.omabox`; makes `~/.config/omabox`;
   asks about the agent guard. A real directory where a link goes stops it. `omabox setup --remove`
   undoes it.
5. Try it: `omabox up && omabox shot`, then `omabox down`. The spike's hand-run scripts (`spike/*.sh`,
   wayvnc, gvncviewer) are history: VNC went in finding 34.

## What the spike verified

| Check | Result |
|---|---|
| Real desktop focus / cursor / workspace | untouched throughout (verified with host `hyprctl` before/after) |
| Nested Hyprland renders | yes, with patched aquamarine |
| Arbitrary screen size | `hyprctl output create headless` + `hl.monitor` rule (3440x1440 tested) |
| Omarchy shell | bar, wallpaper, menu (SUPER+SPACE), tray host all run inside |
| App inside | the Qt app tiled with Omarchy borders/gaps/blur |
| Tray | the Qt app registers with the **box's** StatusNotifierWatcher; real watcher unaffected |
| System bus | unreachable inside (`busctl --system`: no such file) |
| Network | shared; `127.0.0.1:8081` test server reachable |
| Keyboard | `tools/keyboard` → Hyprland binds (SUPER+SPACE menu, Escape closes) and focused client (typed `echo "Hello, omabox: $((6*7)) ~/\|?"` into foot exactly); wtype is wrong (finding 13) |
| Mouse | `omabox-pointer` → wl_pointer enter/button/axis delivered (checked with foot + WAYLAND_DEBUG) |
| Theme switch | `omarchy-theme-set lupine` inside re-themes bar, wallpaper and app live; real theme file mtime unchanged |
| Parallel boxes | two boxes side by side, independent Hyprland instances |
| Startup | ~2.8 s from seed to bar visible |
| Memory | ~450 MB PSS idle, ~580 MB with the Qt app (RSS ~950 MB, shared libs double-counted) |

## Findings (in the order discovered)

1. **Hyprland cannot run headless on its own.** Aquamarine only starts with a DRM or a Wayland
   primary backend; in a sandbox with no card node: `CBackend::create() failed!`. It must be
   nested inside a parent Wayland compositor.
2. **Aquamarine binds protocol versions at fixed numbers**, not clamped to what the parent
   advertises. The parent must offer `wl_compositor` v6 and `xdg_wm_base` v6.
   sway 1.12 (xdg_wm_base v5), weston 15 (wl_compositor v5), cage 0.3.1 (xdg_wm_base v5) all
   abort. **labwc 0.20.2 offers both → works.**
3. labwc headless creates **no output** unless `WLR_HEADLESS_OUTPUTS=1` (default 1280x720).
4. **Aquamarine 0.15.0 nesting bug:** `createOutput` never flushes the Wayland connection, so the
   initial toplevel commit sits in the client buffer until some event arrives from the parent.
   A headless parent with no input never sends one → Hyprland idles with zero monitors.
   Nudging it (a virtual keyboard on the parent → `wl_seat.capabilities`) flushes, but then
   Hyprland commits a buffer before acking configure → labwc: `xdg_surface has never been
   configured` → connection killed. Both fixed by aquamarine PR #415 (merged 2026-09-22, after
   0.15.1). Nesting in a real Hyprland hides the bug because input events keep flowing.
   Diagnosis path: `WAYLAND_DEBUG=client` on Hyprland shows requests *queued*, `WAYLAND_DEBUG=server`
   on labwc shows they never *arrived*; `ss -x` showed empty socket queues.
5. **Safety invariant:** Hyprland tries DRM first (libseat → seatd, logind). It fails only because
   the box exposes no `/dev/dri/card*`, no `/dev/input`, no `/run/seatd.sock`, no system bus.
   **Never bind those into the box.** Same reason the box's "Shutdown/Reboot" menu is inert.
6. **Teardown:** PID 1 of a pid namespace ignores SIGTERM from outside; killing the outer bwrap
   leaves the inside running. Use `bwrap --info-fd` to get `child-pid` and `kill -KILL` it.
7. Unix socket paths are limited to 108 bytes: from the host, point `XDG_RUNTIME_DIR` at a short
   symlink to the box's run dir.
8. Hyprland 0.56 is Lua-only (`hl.monitor`, `hl.config`, `hl.on`, `hl.window_rule`,
   `hyprctl eval '<lua>'`). Plain `hyprctl monitors` can say `unknown request`; use `-j`.
9. Omarchy config reuse: `dofile(bootstrap.lua)`, then pre-mark `default.hypr.autostart` as loaded
   (skips systemd env import, first-run provisioning, monitor-watch, udiskie), then
   `require("default.hypr.omarchy")`. `misc.disable_watchdog_warning = true` hides the
   start-hyprland banner.
10. **Idle must be off in the box**: without real input Omarchy's idle service fires the screensaver
    (and would lock). Was `~/.local/state/omarchy/indicators/stay-awake` (what `omarchy-toggle-idle`
    uses); since finding 51, long idle timeouts in the box's `shell.json` instead.
11. **wayvnc lets the viewer resize the output** by default: a large viewer window resized the
    box's screen and threw off pointer coordinates. Always run `wayvnc -R`.
12. Inside the box, D-Bus activation works (xdg-desktop-portal, dconf, gvfsd were auto-started), so
    portals may work. File chooser checked 2026-09-23: a FileChooser.OpenFile call over the box's
    session bus (`gdbus call`) opens xdg-desktop-portal-gtk's chooser, floating and centred, like on
    the real desktop. A Qt app's file pickers go through it since its own fix.

13. **wtype's keys arrive as the wrong keys.** wtype uploads its own keymap and numbers keys from
    keycode 9; Hyprland replaces a virtual keyboard's keymap with its configured layout
    (`ApplyConfigToKeyboard for "hl-virtual-keyboard-unknown"` in the log), so wtype's first key reads
    as Escape: `wtype -M logo -k space` opened the *System* menu (SUPER+ESCAPE), not SUPER+SPACE.
    The spike's "wtype → binds" check passed by luck. `tools/keyboard` sends real evdev keycodes looked
    up in the box's own XKB layout (`hyprctl getoption input:kb_layout`); `resolve_binds_by_sym` does not help.
    Correction (finding 55): clients do get the keymap a virtual keyboard uploads; what broke wtype was
    its keycodes against binds, not the keymap being dropped.
14. **A new virtual device's first event is swallowed.** Hyprland makes a new keyboard/pointer the active
    device on its first event and re-sends keymap + `wl_keyboard.enter` to the focused client; the event
    that caused the switch never reaches the client (WAYLAND_DEBUG on foot: `a` arrived as a release only).
    So a lone `Escape` did nothing and a click after one `move` missed. Both tools now spend that first
    event: the keyboard on an empty `modifiers`, the pointer on a 1px nudge and back (a zero-delta motion
    does not count).
15. QML buttons ignore a press that lands in the same instant as the motion that reached them: the
    pointer tool round-trips and waits 40 ms after each `move`. With 14 and 15 fixed: 5/5 clicks on the
    bar's menu button open the menu, from a fresh device each time.
16. Reaching the box from the host without symlinks (finding 7): `hyprctl` accepts a *relative*
    `XDG_RUNTIME_DIR`, so the CLI runs it from inside `<box>/run` with `XDG_RUNTIME_DIR=.`; libwayland
    rejects a relative runtime dir but takes an absolute `WAYLAND_DISPLAY` as the socket path itself.
17. `nsenter -t <child-pid> -U -m -p -u -i --preserve-credentials` enters a box unprivileged (we own its
    user namespace). That is `omabox run`: stdout, stdin and exit code pass through. `--wd` resolves on
    the host side, so the CLI `cd`s inside instead. Commands get the env Hyprland's children see,
    dumped by the box's Hyprland at start (`$XDG_RUNTIME_DIR/omabox.env`, also the "Hyprland is up" marker).
18. **Keyring:** `gnome-keyring-daemon --unlock` with an empty password creates no login keyring; the
    first `secret-tool store` then waits on a gcr-prompter inside the box. Seeding a plain-text
    `login.keyring` (empty password = unencrypted, never locked) + `default` fixes it: store/lookup work
    without prompts, the real keyring is untouched (host `secret-tool lookup` finds nothing).
19. **Xwayland:** the spike ran two (labwc's lazy `:0`, woken because Hyprland inherited `DISPLAY=:0`,
    and Hyprland's `:1`), ~250 MB RSS. Now Hyprland gets `DISPLAY` unset and `xwayland.enabled = false`
    unless `omabox up --xwayland`: no Xwayland in the box.
20. **Throwaway overlays:** bwrap 0.12 `--overlay-src DIR --tmp-overlay DIR` gives a writable view of a
    host dir whose writes land on a tmpfs and vanish. `omabox run` with no box up mounts the current repo
    that way, so `ctest` can write `build/Testing/` while the host checkout stays byte-identical
    (`LastTest.log` mtime unchanged). Everything else of the projects dir stays read-only.

21. **Plugins in a box:** the shell enables a plugin when `shell.json` references it (`bar.layout.*`,
    `plugins[]` or `bar.id`; `services/PluginRegistry.qml`). The seeded `shell.json` keeps the user's bar
    settings but only built-in (`omarchy.*`) widgets plus the mounted plugins, each added where its
    manifest says (`barWidget.defaultSection`, else `plugins[]`). Mounting every host plugin by default
    would be wrong: a project polls the user's real local server.
22. "Ready" = Hyprland env dumped + `omarchy-bar` layer mapped + `org.kde.StatusNotifierWatcher` on the
    box bus. The bar still paints its widgets ~150 ms later and animates for ~500 ms: `up` also waits
    for the bar's pixels to hold still (3 identical grim captures), so a first screenshot is final.
    `up` to settled bar with two plugins: 4.1 s.

23. **Interactive mode nests Hyprland directly in the host Hyprland**, not in labwc's Wayland backend
    as planned: the host offers `wl_compositor`/`xdg_wm_base` v6 (finding 2) and keeps input flowing
    (finding 4), and the box's screen (`WAYLAND-1`) follows the window's size on its own; a labwc in
    the middle would need a fullscreen rule and a second resize hop for nothing. The box gets **one
    connected fd** to the host (`tools/wlfd` → `WAYLAND_SOCKET`), never a socket path, so nothing inside
    can open a second host connection; the box's own `/run/user/1000` has only its `wayland-1`.
    Aquamarine calls every nested window `aquamarine` (app_id and title), so the window is placed by the
    host's exec rules on the launch process (`hl.exec_cmd(cmd, { workspace = "9 silent",
    no_initial_focus = true })`), not by a class rule that would catch any other nested Hyprland.
24. **The host does not render a hidden window**: with the box on workspace 9 out of sight, screencopy in
    the box never completes and `grim` blocks forever (it hung `up`'s bar-settle wait). Interactive boxes
    skip that wait; every grim call has a timeout and `shot` says why it failed. Screenshots are for
    headless boxes; interactive ones are for the user's eyes. (Replaced by finding 90: a hidden
    interactive box is drawn now, at the host's `misc.render_unfocused_fps`.)
25. **Closing an interactive box's window** only removes its output; Hyprland idled screenless. It now
    exits when its last monitor goes, and `session.sh` then `kill -KILL -1`s the namespace: bwrap's PID 1
    only exits once it has no children, and the shell's helpers (inotifywait, wl-paste) outlive
    Hyprland. Verified: box `dead` 1.5 s after the window closes.
26. **Passthrough without touching the desktop:** a headless box plays the host and runs
    `omabox up --interactive` inside itself (bwrap nests fine). Driven with the virtual keyboard:
    SUPER+SPACE opens the fake host's menu; after SUPER+ALT+ESCAPE (submap `omabox`) it opens the
    box's menu inside the window, Escape reaches the box, the toggle returns to `default`, and moving
    focus off the box resets the submap by itself (so nobody is stuck with SUPER swallowed). The host
    side is runtime-only (`hyprctl eval`), gone at the next config reload.

27. **No app launched from a box's binds or menus** (found by the maintainer in interactive mode): Omarchy starts
    every app with `uwsm-app -- CMD` (binds, menu, app launcher → `uwsm-app -- gtk-launch ID.desktop`),
    which asks the systemd user manager for a unit: `Failed to connect to user scope bus`, and nothing
    shows up. `share/bin/uwsm-app` (first on the box's PATH) starts the app directly, detached, and
    handles `-T`, desktop entry IDs and uwsm's unit options; `share/bin/uwsm` maps `app` to it and `stop`
    (logout) to ending the box. Launch log: `<box>/home/apps.log`. Apps that need the **system bus**
    still cannot work, by design (finding 5): GNOME Disks aborts on "Error getting udisks client".
28. **Resizing an interactive box's window did not re-lay it out** until something inside it committed
    (the maintainer had to click its bar). Hyprland 0.56 takes the parent's new size as the output's mode but
    fires no event and never re-arranges layers: bar and wallpaper keep the old width (`hyprctl layers`),
    the window shows a crop of a stale frame. Re-applying the monitor rule (even with the explicit new
    mode), `force_renderer_reload` and pointer motion do not help; `hyprctl reload` does. The box's
    Hyprland polls its output size every 250 ms and reloads its config when it changes. Verified with
    three resizes in the nested setup (finding 26): layers follow within a second. The reload costs a
    brief black frame. **It is Hyprland's, not our aquamarine patch:** a bare nested Hyprland 0.56.2
    (minimal config, a 10-line quickshell PanelWindow as the layer, no omabox logic) inside a headless
    box, resized from 1896x1019 to 1000x600 to 1500x900, reports the new monitor size but keeps the layer
    at 1896 wide and a tiled foot at 958x528, with the system aquamarine 0.15.0 and the patched build
    alike. No matching upstream issue found (2026-09-23). Cause (reading v0.56.2): the output `state`
    listener in `src/output/Monitor.cpp` calls `applyMonitorRule` with the new size, and nothing on that
    path runs `arrangeLayersForMonitor`/`recalculateMonitor` (only `onConnect` and the reload path do).
    Brief for a fix on a fork: `docs/hyprland-nested-resize-bug.md`. If fixed, drop the timer.
29. **Passthrough did not end when clicking the host bar** (the maintainer): a layer click leaves the box the
    focused window, so the `window.active` hook never fires, and returning true from an
    `input.keyboard.key` handler does not swallow the key (tested). Now the host also leaves passthrough
    on the first key pressed while the pointer is off the box window: after clicking the bar, SUPER+1's
    SUPER press (harmless, it reaches the box) ends passthrough and the 1 hits the host bind. Verified
    in the nested setup: keys with the pointer over the box keep passthrough; the same keys off it end
    it; bar click then SUPER+1 switches the host workspace. The Lua is versioned, so a running host
    Hyprland with the old hooks upgrades on the next `omabox up --interactive`.
30. `omabox keys shift` (a modifier on its own) failed: modifier names are not keysyms. Aliased to
    Shift_L, Control_L, Alt_L, Super_L, ISO_Level3_Shift.

31. **D-Bus-activated apps got no display** (Files from the app launcher, SUPER+SHIFT+F; the portals'
    "cannot open display"): `dbus-run-session` starts the bus before the compositor, so services it
    activates (`DBusActivatable=true` apps, xdg-desktop-portal-gtk) had no `WAYLAND_DISPLAY`. Omarchy's
    autostart (skipped, finding 9) runs `dbus-update-activation-environment --systemd --all`;
    `share/shell.sh` now runs it without `--systemd` before the shell. Verified: Nautilus opens from
    the launcher and the bind, no "cannot open display" left in the box log. (Headless only: interactive
    boxes had a second cause, finding 33.)
32. Passthrough and focus-follows-mouse (Omarchy default `follow_mouse = 1`): moving the pointer onto
    another host window moves keyboard focus there, so passthrough ends with it (finding 29's focus
    hook). Intended: SUPER keys would reach that other window anyway. Re-enable with the toggle.

33. **The host connection leaked into the box's session bus** (interactive; found because Files still
    did not open there): `dbus-run-session` inherited the `WAYLAND_SOCKET` fd from bwrap, so the bus
    daemon held a copy of the connection to the host compositor, and every service it activated (Files,
    gvfsd, portals) inherited a stale `WAYLAND_SOCKET=4`, which libwayland prefers over
    `WAYLAND_DISPLAY`: "Failed to open display". Finding 23's "only Hyprland holds it" was not true.
    `session.sh` is now the box's first process: it takes the fd out of the env, starts the private bus
    (`dbus-daemon --session --fork`) and the keyring with the fd closed for them, hands it to Hyprland
    alone, then closes its own copy. Verified in the nested setup: no process but Hyprland (and bwrap's
    init, which forked it) has it; Files opens; closing the window still ends the box. Headless
    regression: tray watcher, keyring, Files, `run -- ctest` all pass.

34. **VNC removed** (the maintainer's call, 2026-09-23). `omabox watch` showed a black screen: wayvnc only sends
    damaged regions and an idle box redraws nothing, so the first full frame never comes (only the
    area around a moved pointer appeared). `force_renderer_reload`, `debug.damage_tracking = 0`,
    `debug.vfr = false` and a config reload did not produce a full frame. It was also view-only by
    design and cost two packages (wayvnc, gtk-vnc) and a port per box. Headless (agents) plus
    interactive (the user) cover the need; `omabox shot -b NAME` looks at any running box. Findings
    11 and the VNC parts of the spike are history.

35. **Machine-independent** (to share it): the render node is detected (first usable
    `/dev/dri/renderD*`, `OMABOX_RENDER_NODE` overrides, only ever a `renderD*`: a `card` path is
    refused), the box's runtime dir is `/run/user/$UID`, and host code is no longer the whole projects dir by
    default: the repo `up` runs from, plus `~/.config/omabox/ro-bind` (e.g. `~/code`)
    and `--ro-bind`; HOME or anything above it is refused. `install.sh` checks Hyprland >= 0.56 and any
    render node. Verified: a repo in the scratch dir runs its own script in a box and sees neither
    the projects dir nor HOME; with the config, the projects dir is there read-only;
    `OMABOX_RENDER_NODE=/dev/dri/card1` refused; `run -- ctest` in the Qt app passes. The skill installs
    wherever Omarchy installs its own agent skills (`omarchy-provision-user`).

36. **`omabox peek`** (`tools/peek`): a live, view-only window of a headless box. A headless box has no
    window and its output type is fixed at start, so looking at it live means copying frames out:
    wlr-screencopy `copy` (not `copy_with_damage`, which is what left VNC black) from the box socket,
    drawn letterboxed and bilinear-scaled into an xdg-shell window on the host, 10 fps. Sends nothing
    into the box. Opened through the host's exec rules (workspace 9 silent), closed by `down`, exits on
    its own when the box dies. Verified in the nested setup (first frame immediate on an idle box, live
    update when the box's menu opens, ~6% of a core at 1896x1019, exits with the box) and on the real
    desktop (workspace 9, host workspace and focus unchanged, gone after `down`).

37. **Projects need no omabox edits.** A project's own CLAUDE.md/AGENTS.md ("run ./build/app",
    "hyprctl -j clients", "grim", "ctest", "omarchy-theme-set") is loaded every session and beat the
    skill: a Qt app's instructions sent agents to the real desktop. Rewriting projects' instruction files would taint
    them with one user's tooling, so the skill now says those instructions still hold and maps each to
    its box equivalent (table in the skill), and not to edit project files unless asked. Also: a box
    starts with a fresh HOME, so apps show first-run screens; never pick a real local service there
    (the Qt app's first run defaults to the user's real server on 127.0.0.1, reachable over the shared
    network). Verified with one Claude Code run in plan mode in a scratch repo holding the Qt app's old
    CLAUDE.md: it listed the omabox equivalents of every step, cited the reason, and avoided the real server.
    Other agents (Codex, OpenCode) not yet tried.

38. **The skill decides when a box cannot test something**, so projects need no "use the real desktop"
    notes either. It lists what a box lacks (real monitor modes/HDR/VRR/scale, the system bus, audio,
    DDC/i2c, backlight, input devices, systemd user units, the user's data) and the signals in a
    project's code (`hl.monitor`, refresh/HDR, `ddcutil`, `wpctl`, `nmcli`, `systemctl --user`, ...).
    Such steps are split out: the box shows what it can (layout, UI, logic), and for the rest the
    agent says exactly what will change on the real desktop and asks once per task. Verified with
    Claude Code in plan mode on unmodified copies: a monitor-settings project ("check 120 Hz works") split into
    offline tests, box (layout only, noting the virtual screen cannot show 120 Hz) and a real-desktop
    part behind a question describing the change and its 15 s revert; the Qt app ("see the main window")
    went straight to a box with no question and pointed first run at :8081.

39. **A stopped `run` launcher hung `omabox down` (fixed).** `box_exec` uses
    `nsenter -p`, which forks and stays on the host waiting with `WUNTRACED`. If the child stops, nsenter
    SIGSTOPs itself and only SIGCONTs the child once it is resumed (`continue_as_child()`, util-linux
    2.42.3 `sys-utils/nsenter.c`). A `kill -CONT` sent from inside the box resumes only the child, so the
    host nsenter stays in `T` for the rest of the box's life. When the child dies (a `pkill`, or down's
    SIGKILL) it is a zombie in the box's pid namespace, and nothing can reap it. The box's PID 1 then
    blocks in `zap_pid_ns_processes` (the kernel waits until every task in the namespace is reaped) and
    `down` waits on it forever. One STOP/CONT at any time is enough; no `pkill` is needed. This is not
    caused by TTYs: `-d` children are `setsid` with no controlling terminal (`/dev/tty` gives ENXIO).
    Verified in a box: `run -d -- sleep 1000`, then from inside STOP, CONT, `pkill` → launcher `T`, sleep
    `Z`, `timeout 8 omabox down` → 124, PID 1 wchan `zap_pid_ns_processes`. A `kill -CONT` on the
    launcher lets down finish in ~100 ms. Fixes: `run -d` now launches with `setsid -f`, so nsenter exits
    at once and the command is reparented to the box's PID 1 (no host launcher left behind). `down`
    also SIGCONTs `nsenter -t <pid1>` after the SIGKILL, for foreground runs stopped from elsewhere.
    Verified in a box: the repro above now gives down rc 0; a foreground run stopped and resumed from
    inside (launcher `T`) goes down in 96 ms; `-d` logs, and a missing command's error, still reach the
    run log. A foreground `run` whose command stops still blocks until it is resumed, like Ctrl-Z in a
    terminal.

40. **Bar widget** (`plugin/`, Omarchy plugin `chaves.omabox`; the id was free in the official
    registry, omacom/omarchy-plugin-marketplace `registry.json`, 4027 sources, 2026-09-23). A plugin
    rather than a tray icon: omabox needs Omarchy anyway, and a panel shows more than a tray menu. The
    icon is only in the bar while a box exists. The panel is display-only, like `omarchy.agents`: it
    polls `omabox ls --json` (new; 5 s, 2 s while open) and runs `omabox peek -b N --focus`,
    `shot` + `xdg-open`, `down` (two presses within 3 s). `peek --focus` is the one place omabox
    focuses a host window, and only on that click: an existing peek window is focused instead of
    opening a second; a new one is waited for, then focused. An interactive box's window is found by
    pid, which is the outer bwrap's, since omabox-wlfd connects and then execs bwrap. Failures show in an alert strip
    under the header (urgent colour, wraps, dismissable), kept until the next action succeeds, and
    also go out as a notification, because peek and shot close the panel. Verified in a
    stand-in (finding 26: a headless box whose bar ran the plugin, with the real omabox run nested
    inside, `command` set to the repo's `bin/omabox`): hidden with no boxes, shown after a nested
    `up`, rows for headless/interactive/dead boxes, the count, Peek (workspace 1 → 9, peek window
    focused, the row shows "peeking"), a second Peek focusing the same window, Show focusing the
    interactive box, Shot opening imv, a forced shot failure in the alert, Down disarming after 3 s
    and going through on d d and on two clicks (the dead box cleaned up, the peek window closing with
    its box, the icon hiding). Then on the real bar (enabled
    2026-09-23): Down, and the notification, checked by the maintainer with a `command` wrapper whose `shot`
    fails. A box has no notification daemon, so notifications can only be checked on the real desktop
    (no longer true: every box has one since finding 42).

41. **Input quirks in `omabox keys` / `omabox click`** (found while testing finding 40; fixed).
    (a) With a shell panel open, only the first `keys` call reached it: later calls were lost
    (the built-in audio panel: Tab in one call, Escape in the next did not close it). Keys sent in one
    call all arrived. (b) A `click` at the pointer's current position was lost; after a `pointer move`
    elsewhere it worked. Cause: a headless box has no input devices except the helpers' own, which
    each call creates and destroys. Between calls the seat has none, and Hyprland then drops focus
    changes (its log: `setKeyboardFocus without a valid keyboard set`, `setPointerFocus without a
    valid mouse set`), so the next device found the panel without keyboard focus and the surface
    under the cursor without pointer focus. Fix: `omabox-keyboard --hold` and `omabox-pointer --hold`
    keep an idle keyboard and pointer on the seat for the box's life, started by the box's Hyprland
    (headless only; an interactive box has the host's devices). Verified in a box: Tab, Down, Escape
    as three calls close the audio panel (twice); five clicks on the same spot toggle it five times;
    no more of those log lines; typing into foot unchanged; the helpers die with the box. Also: plugin
    edits in a box did not always hot-reload (one edit was, the next was not); `omabox restart-shell`
    always picks them up. The same on the real bar with the symlinked plugin: neither the edit, a
    `touch`, nor `omarchy-shell shell rescanPlugins` reloaded it; `omarchy-restart-shell` did.

42. **No notifications in a box** (reported from the Qt app: `notify-send` → ServiceUnknown
    `org.freedesktop.Notifications`). Not a missing daemon: the box copies the user's `shell.json`,
    whose `disabledPlugins` had `omarchy.notifications` because the real desktop's notifications come
    from a third-party plugin (njpatel.omapager, which has a `NotificationServer`), and the box drops
    third-party plugins. `seed_home` now takes `omarchy.notifications` out of `disabledPlugins` unless
    a mounted plugin's QML has a `NotificationServer`. Verified: a plain box shows a `notify-send` in
    the built-in style (top right); `up --plugin njpatel.omapager` keeps the built-in off and shows
    omapager's.

43. **`--ro-bind DIR:DEST`** (path mapping: a server says `/data/...`, the
    desktop sees `/mnt/nas/...`). Split on the first `:/`, so plain paths work as before; the same form
    works in `~/.config/omabox/ro-bind`. DIR keeps the "never all of HOME" check; DEST may not be `/`,
    contain `.`/`..`, or sit under `/usr /etc /proc /dev /sys /run /bin /lib /lib64 /opt/omabox`, or be
    the box HOME itself (subdirs of it are fine). All mounts are now checked before the box dir is
    created, so a refused `up` leaves nothing behind (before, the HOME refusal left an empty box dir).
    Verified: a dir mounted at `/mnt/nas` and at its own path in one box, read-only (`touch` fails);
    a `DIR:/data` line in the config file; refusals for a missing DIR, `/usr/...`, `/run/user/1000`,
    `/home/sbx`, `/mnt/../etc`, `/` and HOME, none leaving a box dir.

44. **`up --net isolated [--allow PORTS]`** (headless only at first, interactive too since 45; first built on Arch's pasta 2026_07_28;
    finding 45 replaced the pid workaround and found the real cause of the window escape). Launch
    is `pasta -q --splice-only -t none -u none -U none -T PORTS -P pasta.pid -- sh -c 'exec 3>info.json;
    exec "$@"' bwrap --unshare-user --uid $UID --gid $GID ...`. `--splice-only`: the box has only `lo`,
    no interface and no route (no internet, no LAN); `-T` splices just the listed ports to the host's
    loopback; every other direction is `none` because pasta's default is `auto` (forward whatever is
    bound). pasta's namespace maps us to root, hence bwrap's uid/gid map back to ours. pasta closes
    inherited fds, hence the info fd opened inside it. This pasta always adds a pid namespace (no
    `--no-pidns` yet), so bwrap's `child-pid` is pasta's numbering: `isolated_pid` walks pasta → bwrap
    → child and matches `NSpid` to record the host pid in `<box>/pid`, which `box_pid` reads for these
    boxes. `omabox run` adds `nsenter -n` for isolated boxes only (joining the host's own netns from
    the box's userns is refused). Verified with dummy servers on :18081 (allowed) and :18080: `run`,
    `run -d` and a Hyprland `exec_cmd` inside all get 200 / refused, uid 1000, only `lo`; a throwaway
    `run --net isolated`; screenshots; `down` leaves no pasta or bwrap. `ss` shows the real and test servers bound
    to 127.0.0.1 only, so the LAN route is not a way to them (the all-interfaces case could not be
    tested: this agent sandbox cannot listen on 0.0.0.0).
    Dead ends on the way: attaching pasta to bwrap's netns (`pasta PID`, `--userns/--netns`) fails
    with "Couldn't switch to pasta namespaces: Operation not permitted" (the `setns` in `ns_check`;
    the cause, found in the review of PR #8: bwrap's `--dev` nests a second user namespace, and pasta
    joins the inner one first. `nsenter --user-parent -U --preserve-credentials -- pasta … --netns …`
    attaches; unused, since launching bwrap inside pasta works);
    wrapping without the uid map gives uid 0 in the box; the first build trusted `child-pid` and lost
    two boxes (killed by hand). Two mistakes touched the real desktop during this spike: a probe of
    pasta's default gateway mapping sent one unauthenticated API request to the real local
    server (403; use dummy ports for reachability tests), and an `--interactive --net
    isolated` box opened its window on the user's workspace with focus, because the window's client
    runs in pasta's pid namespace and the host's exec rule (workspace 9 silent) did not match it. That
    combination is refused until the rule can match (needs `--no-pidns` or another way to place the
    window). (No longer: finding 45 found the cause, the exec-rule token, and lifted the refusal.)

45. **Pinned pasta, and why the isolated interactive window escaped.** `install.sh` builds passt at
    588b545 ("pasta: Add --no-pidns", 2026-09-06, in no release yet; Arch has 2026_07_28) into
    `build/prefix/bin` (drop once Arch ships a release with it). With `--no-pidns` bwrap's `child-pid`
    is ours again, so finding 44's `isolated_pid` walk is gone. It did NOT fix the window: in a
    stand-in, `foot` launched through the old pasta (own pid ns) under a `workspace N silent` exec rule
    still landed on N. Bisecting the real chain (bash → `env -i` → setsid → pasta → sh → omabox-wlfd →
    client) in a stand-in: Hyprland ties a new window to its exec rule by the launched pid, or by
    `HL_EXEC_RULE_TOKEN` (and `HL_INITIAL_WORKSPACE_TOKEN`) in the client's environment. A plain
    interactive box matches by pid (omabox-wlfd connects as the launched process, then execs bwrap).
    With pasta the connection comes from a forked child, and `launch.sh`'s `env -i` had dropped the
    token: neither matched, so the window opened on the user's workspace. `env -i` + wlfd → rule held;
    `env -i` + pasta + wlfd → active workspace; the same with the token kept → rule held. `launch.sh`
    now keeps `HL_EXEC_RULE_TOKEN` through `env -i`, and bwrap `--unsetenv`s it for the box (the host
    reads it from the outer bwrap's environment, which keeps it). Verified in a stand-in: a nested
    plain interactive box still lands on workspace 9 without focus, the token does not reach the box's
    Hyprland; headless isolated boxes unchanged. Not verified: an interactive isolated box end to end,
    because a box cannot host one (bwrap inside a box cannot write pasta's uid map: "setting up uid
    map: Operation not permitted"; this blamed the outer bwrap's bounding set, but the cause is
    no_new_privs on everything a box's session starts, finding 89). Checked once on the
    real desktop instead (the user's go-ahead): `up isoi --interactive --net isolated --allow 18081`
    opened on workspace 9, the active workspace and window unchanged; inside, uid 1000, only `lo`,
    :18081 200, :18080 refused; `down` left no pasta. The refusal is gone. (A stray `peek --focus` in
    the same test command then moved the user to workspace 9; focus was restored to the same window at
    once. Keep real-desktop test commands to exactly the checks announced.)
    Afterwards the pinned pasta was dropped again: in a stand-in, Arch's pasta (own pid ns) with the
    token kept also placed the window by its rule, so `--no-pidns` only saves the pid lookup, not
    worth a second source build. omabox is back on Arch's `passt` with finding 44's `isolated_pid`;
    `UPSTREAM.md` says what to simplify once a passt release has `--no-pidns`.
    Rechecked on the real desktop with Arch's pasta (the user's go-ahead): interactive isolated box on
    workspace 9, active workspace and window unchanged, box pid found through pasta's pid namespace
    (NSpid three levels), uid 1000, only `lo`, :18081 200, :18080 refused, no pasta after `down`.

46. **`omabox run` no longer falls back to a throwaway box for a named or dead box.** Asked for a
    box by name (`-b NAME`, `$OMABOX`) that is not up, or with a default-named box that was up and
    died, `run` started a throwaway box instead: host network, none of the box's mounts. It hid a
    dead isolated box twice in testing. Now it fails, and `need_box` says when a box is dead (its
    logs, `omabox down NAME` to clear it). A throwaway box is still started when no name was given and
    no box by the default name ever existed. Verified: `-b nosuch`, `OMABOX=nosuch`, a killed box by
    `-b` and by default name all fail; a plain `run` with no box still runs (exit code passed through).

47. **The logo** (`assets/`): offset arms around a window, the window standing in for the box. It sits
    on a 15-unit grid with a 1-unit stroke, the proportions of the Omarchy mark, without its shapes
    (Omarchy is a pending trademark). The bar glyph is a separate 8-unit drawing, because the bar's
    icon canvas is 16 px (`Style.bar.iconCanvas`): 2 px a unit, no blur. `plugin/Mark.qml` draws
    both as whole-pixel rects in the bar's foreground; it replaces the md-cube_outline font icon in
    the bar and in the panel header. The README lockup is JetBrains Mono ExtraBold as outlines, in
    a dark and a light version. Quirk: `Mark` as the root of `PanelHero`'s `iconComponent`, with its
    own width and height, kept the panel from opening (no log line); wrapped in a sized `Item` it
    works. Verified in a box standing in for the host (finding 26: `command` set to a stand-in
    that lists one or two boxes): the glyph with and without the count, the panel's header.

47. **Toolchains installed in HOME were not in a box** (found in testing, fixed). `node` existed
    only under mise (`~/.local/share/mise/installs/node/…`), so every `node --test` suite failed in a
    box; omakade's ctest was configured with mise's python by absolute path. Now `up` mounts mise's
    `installs` dir read-only at its own path (`$MISE_DATA_DIR` respected; interpreters and compilers,
    no secrets), and `box_exec` builds PATH as omabox's stand-ins, then the caller's PATH in its order
    keeping only dirs the box can see, then the rest of the box's. So `omabox run` finds the host's
    node/python/uv. Left out: mise shims (need mise's config), `~/.local/bin`, rustup's `~/.cargo`
    (can hold a registry token); add a dir to `~/.config/omabox/ro-bind` to have it on PATH. Apps
    launched by the box's Hyprland keep the box PATH. Verified: a service-status widget's 40/40 and omakade's
    tv_game_launcher pass with a plain `omabox run`; node in an isolated named box; the mount is
    read-only; no `~/.cargo` or `~/.local/bin` in the box.

48. **No git identity in a box** (found in testing, fixed). The fake HOME had no `.gitconfig`, so
    the Qt app's `release` test (a dry-run commit in a throwaway clone) failed with "Committer identity
    unknown". `seed_home` now writes `user.name` and `user.email` from the user's global git config
    and nothing else (credential helpers and URLs with tokens stay out). Verified: the box's
    `~/.gitconfig` has just `[user]`; the test passes in a box.

49. **No `USER`/`LOGNAME` in a box** (found in testing, fixed). The launcher starts bwrap under
    `env -i` with only PATH, TERM and LANG, so everything in the box saw them empty: a VM plugin's
    settings face read "Allows, for  only". launch.sh now sets both to `id -un`. Verified: `omabox
    run -- sh -c 'echo $USER'` prints the user; the face reads "for USER only".

50. **A box's HOME lived in the user's runtime dir** (found in testing, fixed). `$D` is
    `$XDG_RUNTIME_DIR/omabox/NAME`, so the box HOME (`$D/home`) was on the host's `/run/user/$UID`
    tmpfs (RAM), the same filesystem as the real session's sockets: a box writing a lot
    to HOME (a build, a download) could fill it and break the real desktop. A VM plugin's Tune face showed
    it ("5 GB free"). Now the HOME is `${XDG_CACHE_HOME:-~/.cache}/omabox/NAME/home` on disk and
    `$D/home` is a symlink to it, so `omabox path`, logs and the widget are unchanged; `down` removes
    both, and `up` sweeps HOMEs whose box dir is gone (the runtime dir is cleared at reboot, the
    cache is not; the box dir is created first, so a box being set up is never swept). The box's
    /tmp and overlays are its own tmpfs (RAM, not the runtime dir). Verified: `df ~` in a box is the
    root disk; 1 GB written there leaves `/run/user/1000` at 56 MB; `down` empties
    `~/.cache/omabox`; an orphan HOME is swept by the next `up`; throwaway runs clean up.

51. **Every box showed Stay Awake as on** (found in testing, fixed). Finding 10 turned idle off by
    touching the `stay-awake` flag, so a Stay Awake indicator (an indicators plugin) was always lit
    in a box: a state the user never set, in screenshots meant to show their bar. The box's
    `shell.json` now sets `idle.screensaver` and `idle.lock` to 1000000 s (~11.5 days; 1e9 ms still
    fits a 32-bit int), which Omarchy's idle service reads, and the flag is gone. Verified: in a
    fresh box the idle monitor is active, `stay-awake: disabled`, no idle cycle after 3 min (longer than
    a usual timeout) and the indicator is off; a control box set to 20/40 s launched the
    screensaver at 20 s and locked at 40 s, so the setting is what holds idle off.

52. **No DNS in a host-network box** (found in testing, fixed). `/etc` is the host's but `/run` is
    the box's, and Arch's `/etc/resolv.conf` links to `/run/systemd/resolve/stub-resolv.conf`: the
    link dangled, nss-resolve's socket was absent too, so glibc asked 127.0.0.1:53 and nothing
    answered. `curl` could not resolve anything, and a weather widget sat on "Fetching forecast…".
    A host-network box now gets the resolved-to file bound at its own path when it is under `/run`
    (the resolved stub at 127.0.0.53 is reachable in the host netns; nsswitch falls from `resolve` to
    `dns`). Isolated boxes get nothing: they have no network to resolve for. Verified: `getent hosts
    archlinux.org` and wttr.in (200) in a host box, the weather panel filled; an isolated box still
    resolves nothing. Untested: NetworkManager's `/run/NetworkManager/resolv.conf` (same code path).

53. **A plugin center anchor survived the shell.json filter** (found in testing, fixed). In
    `($keep | index(.))` the `.` is `$keep` itself, so the test was always true (index 0) and a
    `bar.centerAnchor` naming an unmounted plugin (here a workspaces plugin) stayed. Harmless on 0.4 (the
    bar treats a missing anchor as none) but not what seed_home meant; the anchor is now bound to a
    variable first. Checked with jq against mounted, unmounted and built-in anchors.

54. **An isolated box was named `pasta-HOST`** (found in testing, fixed). pasta runs the box in its
    own UTS namespace and names it `pasta-<host>`, so a terminal prompt in an isolated box read
    `user@pasta-omarchy` while a host-network box read `user@omarchy`: screenshots differed by
    network mode. bwrap already unshares UTS; `--hostname "$(uname -n)"` now names every box after
    the host. Verified: `hostname`, `uname -n` and `/proc/sys/kernel/hostname` read `omarchy` in a
    host-network and an isolated box.

55. **`omabox keys` could not type characters outside the layout's first two levels** (found in
    a project's docs during testing, fixed). It looked a character up on levels 1-2
    of the box's layout (plain, Shift) only, so Ü on `us`, or `@` on `de` (AltGr+Q), failed with
    "cannot type U+00DC". Now (a) the modifiers for a level come from the key type
    (`xkb_keymap_key_get_mods_for_level`), so AltGr levels work, and (b) a character the layout lacks
    is bound to a spare keycode (one the layout leaves empty) in an extra `key` line of the keymap the
    virtual keyboard uploads; clients decode it as that character. Only `-t TEXT` adds spares; a call
    without them uploads the plain layout as before. Verified in a box with foot running `cat >
    file`: on `us`, "Hello Ünïcödé ß € 😀 ~/|?" arrived byte for byte; on `de`, "Grüße @ € {x} | ~ \ zy
    ñ 😀" (AltGr, QWERTZ, ñ through a spare); SUPER+SPACE still opened the menu in a call that also
    uploaded spares. In the Qt app (Qt 6) the filter field took "Ünïc" and matched its one item.
56. **Rendering cost: refresh rate, host-matched mode, `omabox gpu`** (from the VM plugin session,
    2026-09-23; built 2026-09-24). Tuning a VM plugin's popup showed a box measures GPU cost cleanly (box
    Hyprland/quickshell hold their own `amdgpu` DRM clients) but host-wide tools mislead: they sum
    fdinfo `drm-engine-*` by process *name*, so a box's `Hyprland`/`quickshell` add to the real ones.
    And a box at 60 Hz understates a 144 Hz user for anything that repaints per frame. Now:
    `--size WxH@HZ` (plain `WxH` stays @60) goes into `hl.monitor` so it holds from the first frame;
    `--size host` copies the focused host monitor (read-only `hyprctl -j monitors`, scale stays 1);
    `omabox mode [WxH@HZ|host]` shows or changes it live through `hyprctl eval 'hl.monitor(...)'`
    (`hyprctl keyword monitor` fails: "keyword can't work with non-legacy parsers. Use eval.") and
    writes `omabox.mode` in the box runtime dir, which `share/hyprland.lua` prefers, so a config
    reload keeps it (Hyprland's Lua has `io`). `omabox gpu [SECONDS] [--json]` sums `drm-engine-*`
    deltas from `/proc/PID/fdinfo/*` of the processes in the box's pid namespace (`/proc/PID/ns/pid`,
    so two boxes never mix; `find -lname` needs the `[`/`]` of `pid:[N]` escaped), each DRM client
    once (`drm-pdev` + `drm-client-id`), as % of wall time; its first line is the mode. Verified:
    `up --size 3440x1440@144` → `hyprctl monitors` says `3440x1440@144.00000`; `--size host`
    reproduced the host monitor's mode (the screenshot matched it); `mode 1920x1080@120` survived `hyprctl
    reload`; an idle box read 0.0% for Hyprland, quickshell, labwc. The VM plugin, two boxes
    measured at the same time with `-b`, card open in "Starting" (screenshot), `gpu 10`:
    | VM plugin | 3440x1440@144 | 3440x1440@60 |
    |---|---|---|
    | before the fix (per-frame pulse) | Hyprland 48.8%, quickshell 5.9% | 21.7%, 3.6% |
    | after the fix (25 fps bar) | Hyprland 10.3%, quickshell 1.4% | 11.4%, 1.8% |
    Hyprland matches what was measured earlier (~47%, ~10%); quickshell reads lower than it
    did there (9.3%, 5.7%; not chased). The per-frame version costs 2.25x more at 144 Hz than at 60
    (144/60 = 2.4); the fixed-rate one does not change. Rule: measure rendering cost in a box whose
    mode matches the user's monitor, with `omabox gpu`, never with host-wide tools while a box is up.
57. **The box's bar had a narrower PATH than `omabox run`** (external test pass, group D, fixed). Finding
    47 gave `omabox run` the caller's PATH; the session itself (Hyprland, quickshell, apps from binds)
    still had `/opt/omabox/share/bin:/usr/bin`, so a plugin calling claude/gh from mise said "not
    installed" in a box and worked on the real bar. `up` now passes its PATH (`OMABOX_CALLER_PATH`) and
    `share/session.sh` builds the session PATH: stand-ins, the box HOME's `~/.local/bin` (a stub CLI
    dropped there wins), the caller's dirs the box can see in order, the box's own. Verified: the box
    quickshell's PATH has mise's installs; `command -v claude` from a Hyprland exec finds mise's, and a
    stub `gh` in the box `~/.local/bin` wins over mise's. Also new: `up`/`run --env KEY=VAL` for a
    variable the whole session sees (a plugin's API base pointed at a stub).
58. **`SHELL` unset and `LC_*` dropped in a box** (external test pass, group B, fixed). `launch.sh`'s
    `env -i` passed only `LANG`: a terminal multiplexer's new pane opened `sh`, and `date` printed English 12-hour
    times where the host sets a different `LC_TIME`. Now `SHELL` (the user's login shell from `getent
    passwd`), `LANGUAGE` and every `LC_*` set on the host go in. Verified from a Hyprland exec in a box:
    `SHELL=/usr/bin/bash`, `LC_TIME` as on the host, `date +%X` equal to the host's.
59. **Idle expiry for headless boxes** (decided 2026-09-24). Agents
    forget `down`, and a box holds ~500 MB and a GPU client. A headless box started with `up` goes
    down after `$OMABOX_IDLE` (default 2h) idle; `--idle 30m|2h|90s|0` per box (0: never); interactive
    boxes and `run`'s throwaway boxes never. Idle means no omabox command naming the box (`need_box`
    touches `$D/used`; `up` on a box already up counts), no peek window on it and no `omabox run` still
    running in it (a host `nsenter -t PID`). `up` starts a host reaper (`omabox _reap NAME CREATED`,
    `setsid`, log `$D/reap.log`) that checks every idle/4 (5-60 s), exits when the box goes or a new box
    took the name, and on expiry runs `down` and leaves `$BOXES/.expired-NAME`, so the next command
    says "box 'X' went down after 2h idle" instead of "no box". Verified with `--idle 20s`: an idle box
    went down and `shot` gave that message; a box running a 45 s `omabox run` stayed up through it and
    went down 20 s after; `--idle 0` stayed; a manual `down`, and a new box on the same name, ended the
    reaper; `down` clears the note. Not verified: the peek window keeping a box (it would open a window
    on the real workspace 9).
60. **`--stock-bar`** (external test pass idea, 2026-09-24). A box copies the user's bar (their layout
    and `shell.toml`, filtered to built-ins plus mounted plugins); the maintainer's has no workspaces widget or
    clock, so a plugin was never seen next to what most people have. `up`/`run --stock-bar` seeds from
    Omarchy's default `/usr/share/omarchy/config/omarchy/shell.json` and skips `shell.toml`; plugins
    are still placed by their manifests, idle and notifications handled as before; `box.json` records
    `bar: stock|user`. Verified: with a workspace plugin mounted, the stock box's bar showed menu,
    workspaces, the plugin (left), "Thursday 01:19" (center), tray/network/audio/power (right); a box
    without the flag kept the user's filtered layout (menu, update, tray, network, audio, power).
61. **A real `systemd --user` in a box: `up --systemd`** (spike and build, 2026-09-24; the spike is
    `spike/systemd-user.sh`). Seven of the projects tried use `systemctl --user`/`systemd-run`
    (a service-status widget, a VM plugin, omakade, a download-dashboard widget, the Qt app, an alarm app, a photo-sync plugin), and a box had no user manager.
    Opt-in, not the default yet. How: the whole box (pasta too) is launched in a transient scope of the
    user's manager, `systemd-run --user --scope -p Delegate=yes --expand-environment=no` (without that
    last flag systemd-run expands `$VARS` in the command, like `ExecStart=`); inside the scope a
    `bash -c` puts the scope's own cgroup path where bwrap's args say `@CGROUP@`, and bwrap binds it
    writable at `/sys/fs/cgroup` under `--unshare-cgroup` (the box sees `0::/`), plus `--dir
    /run/systemd/system` (else "the system has not been booted with systemd"). `share/session.sh`
    starts `systemd --user` first and lets it run the session bus: a manager only joins the bus when
    its own `dbus.service` is up (masking it kept the manager off the bus: no environment import, GUI
    units without WAYLAND_DISPLAY), so the box HOME gets a `dbus.service` that is dbus-daemon (as in
    every box; the host's is dbus-broker), socket-activated at `$XDG_RUNTIME_DIR/bus`. Masked there:
    the keyring socket (the box's keyring daemon starts after, as before) and PipeWire's sockets (no
    audio). Not masked: xdg-document-portal, which fails in every box (no `/dev/fuse`), so the manager
    reads `degraded`; masked, xdg-desktop-portal refuses to start at all. `share/shell.sh` imports the
    session environment into the manager (`dbus-update-activation-environment --systemd --all`) and
    `reset-failed`s: in an interactive box the portal was bus-activated before that, failed three
    times and hit its start limit. The manager leaves mode-000 entries in the runtime dir
    (`systemd/inaccessible`), so `up`/`down` make it writable before `rm -rf` (`rm_box`). Teardown is
    unchanged: killing PID 1 empties the scope and the host collects it with its nested cgroups.
    Verified in boxes: headless, `--net isolated` and interactive all start (interactive: window on
    workspace 9, the host's focus and workspace unchanged), the portal answers, keyring and
    notifications work, a `systemd-run --user` timer fires, a GUI unit (`systemd-run --user foot`)
    opens a window, no `omabox-*` scope is left after `down`; a box without the flag is as before.
    With projects: **an alarm app** set an alarm for 1 min (`systemd-run --user --on-calendar`) and it fired
    (notification and the bar's red "now"), which the external test pass could not show; **a service-status widget** with a
    stand-in `status.service` in the box HOME (`sleep infinity`) followed the real unit: stopped, STARTING
    ("Unit active, /health not answering yet", Stop and Logs live) after `systemctl --user start`,
    FAILED after `kill -s KILL`. Its FAILED card showed no body and no buttons (lib/State.js says the
    stopped state's buttons): not chased; possibly the missing journal (no journald in a box, so
    `journalctl --user` is empty). Still out of reach: logind (a screen-time app), journald, Docker (a VM plugin's VM).
62. **`up` stopped waiting for the shell** (regression from finding 58, fixed the same day). The
    `SHELL` fix added `local shell=<login shell>` in `cmd_up`, which clobbered `up`'s own `shell` flag
    (wait for the Omarchy shell): `wait_ready /usr/bin/bash` returned before the bar, tray watcher and
    stable-pixel checks, so `up` returned in ~2 s instead of ~4 and a `notify-send` right after it
    failed with ServiceUnknown. Found testing `--systemd`; renamed to `login_shell`. `wait_ready` now
    also waits (5 s, then a warning) for a notification server to own
    `org.freedesktop.Notifications`, checked with `NameHasOwner` (`busctl status` exits 0 for a name
    nobody owns); with the bar wait back this was not seen to be needed, kept as a guard. Verified:
    `up` ~4 s again, `notify-send` right after `up` works with and without `--systemd`. Boxes started
    between `77e8bef` and this fix (part of the external test pass) had the short wait.
63. **Review pass 1: safety and lifecycle** (2026-09-24; six reviewers, reports not kept in the repo;
    `test/run.sh` added, see below). Fixed, each with a regression test:
    - *Mounts* (`refuse_src`/`refuse_dest`): only HOME itself was refused, so `--ro-bind
      /run/user/$UID:/mnt/rt` reached the **host session bus** from an isolated box (sockets ignore the
      network namespace), `--overlay ~` and a throwaway `run` from `~` put all of HOME (api-keys.env,
      ~/.ssh) in the box, and `--plugin` paths were never checked. Now every source (ro-binds, the
      ro-bind file, overlays, plugins, the auto-mounted repo) is refused when it contains HOME,
      `~/.config/omarchy` or `/tmp`, or is inside or contains the secret stores, the runtime dir,
      `/run`, `/dev`, `/proc`, `/sys` or /tmp's socket dirs. DEST is normalised (`//usr`) and may not
      hide `/home/sbx`, `/opt/omabox` or `/tmp`. A throwaway `run` overlays only a git repo that passes.
    - *The box aiming host tools elsewhere*: `on_box` ran grim, hyprctl, the keyboard and pointer on the
      host against paths in the box's own writable runtime dir, so a box could swap its socket for a
      symlink and have `shot`/`keys`/`click` act on another compositor (verified box to box). They
      now run inside the box's mount namespace (`nsenter -U -m`, `env -i`), where only the box is;
      values read from `omabox.env` must be tame names; `peek` (a host window) checks the socket is a
      socket and not a symlink, and nothing box-controlled reaches the host's Lua unchecked.
    - *Stale pids*: a recorded pid is only the box's while it is in the box's pid namespace (`pidns`
      in box.json), so `down` never SIGKILLs, and `run` never enters, a process that reused it.
    - *Lifecycle*: `up`/`down` take a per-name lock (two `up`s orphaned a box); a failed `up` kills the
      box it started (dir kept for the logs, `ls` says dead); an isolated box with no pid file is still
      found; `down` kills the box's reaper and waits at most 10 s; a headless box ends with its
      Hyprland (`labwc -S`), an interactive `--systemd` box with its window (`wait` on Hyprland only);
      a throwaway `run` starts `up` as its own process (errors shown, `set -e` intact), keeps its
      `-run<pid>` suffix, and has a reaper that downs it when the `run` is killed outright; `run` counts
      as use for idle expiry and, after an expiry, reports it instead of starting a throwaway.
    - *In the box*: `restart-shell` kills only the Omarchy shell (pid file), not a project's quickshell;
      Hyprland, labwc, quickshell and hyprctl by absolute path (a stub in `~/.local/bin` replaced the
      bar); relative PATH entries dropped; `run` and the session agree on PATH order; `--no-shell`
      really starts no shell; `systemd --user` failing to start ends the box with a message; the
      `uwsm stop` stand-in refuses outside a box (on the host, `kill -KILL -1` is the whole session).
    - *CLI*: `--idle 010` was octal; `mode @60.0` failed and was saved anyway; `run` took unknown options
      as the command; `path`/`env NAME` ignored NAME; `shot` into a missing dir exited silently; `up`
      checks the aquamarine soname Hyprland actually links; `ls --json` has idle/allow/bar/systemd.
64. **Review pass 2** (2026-09-24, continuing 63). Fixed, each with a check in `test/run.sh`:
    - *A repo in /tmp*: 63's `refuse_dest` refused every DEST under `/tmp`, so `up` and `run` failed
      for any repo there (`t_run_idle` failed since). The box's /tmp is its own tmpfs: inside it is
      fine now; `/tmp` itself and Xwayland's socket dirs are still refused.
    - *keyboard*: every token is checked before connecting (a bad one used to type the ones before
      it); `-s`/`--delay` take only 0..600000 / 0..10000 (`-s -1` slept 49 days, `-s abc` was 0); a lost
      compositor exits 1 (was 0 after `down` mid-run); a single character is a key (`+`, `ctrl++`,
      `super+/`: not keysym names); with modifiers a letter is the key, as in binds (`SUPER+W` sent
      SUPER+SHIFT+W); a modifier with no key in the layout fails the token (released an uninitialised
      keycode); `mod` alone is Super; strict UTF-8; `--model`/`--options` from the box's `kb_model`/
      `kb_options` and no `XKB_DEFAULT_*` from the caller; memfd closed, allocations checked.
      *X11 apps*: keycodes above 255 do not exist for them, and the spares were taken from the top
      (709 down), so an X11 app got `abc` for `aÜb€c`. Keys up to 255 are now looked up and used as
      spares first (the us layout has 14 free), with a note when a character only fits above.
      Verified with `GDK_BACKEND=x11 zenity` under `--xwayland`: Ü, α, 😀 arrive. **Open:** € still does
      not reach the X11 app, on any keycode, while Greek letters on the same spare keycodes do; binding it
      as `U20AC` changed nothing. Wayland apps get it.
    - *peek* (a host process reading frames a box sends): each frame's format, size and stride are
      checked before its buffer is allocated or read (a stride shorter than the width made `draw()` read
      past the buffer). Checked in a box against a fake compositor that sends 4096x4096 with stride 4:
      refused. The old peek failed there only because the fake's stock shm code rejected the buffer; a
      server that skipped that check reached the read. `draw()` uses the completed frame's size and
      format, not the one being captured; `WAYLAND_SOCKET` unset; allocations and roundtrips checked.
      It now captures only when the host has shown the last frame (frame callbacks): hidden on another
      workspace, 0 CPU ticks in 2 s where the old one used 60 in 3 s (30 fps, scaling nobody saw).
      `t_peek` runs peek inside a box on that box's own screen.
    - *wlfd*: `socket()` failing said "connect"; the fd was leaked on a failed connect. *Makefiles*:
      `CFLAGS ?=`, CPPFLAGS/LDFLAGS honoured, `.PHONY: clean`.
    - *A dead throwaway left in `ls`*: a full suite run left `default-run<pid>` dead. `run`'s EXIT-trap
      `down` gave up on it (most likely PID 1 over 10 s to exit under the suite's load; the trap sent
      the reason to /dev/null) and the reaper exits when the box is dead. The reaper now clears a
      throwaway it finds dead once its run is gone, and a failed teardown says why. `t_throwaway_dead`.
    - *install.sh*: `make ... && echo` and `hv=$(...|grep)` under `set -e` carried on past a failed
      build, or exited with no message on an unreadable Hyprland version; an empty soname passed the
      check; `ln -sfn` onto a real dir nested the link inside it; it fetched from GitHub even with the
      commit local; it created `~/.codex`, `~/.pi/agent`, `~/.hermes` for agents not installed (now only
      linked when they exist). PKGS names what the box runs (quickshell, gtk3, xdg-terminal-exec, dbus).
      `t_unit_install` runs it with a temporary HOME and stubs (a failing `make`, a `sudo` that refuses).
    - *The box session* gets Omarchy's `TERMINAL`/`EDITOR` (its uwsm env.d default; they were unset).
      A dev link (`/etc/omarchy.conf`) is not followed: a box always runs the packaged Omarchy
      (until finding 135 this held for the session only, not for a terminal's bash).
      *uwsm-app*: options after `-T` go to `xdg-terminal-exec` as uwsm passes them (it maps
      `--app-id`/`--title` to the terminal's flags when the terminal's entry declares them; foot's does
      not, with uwsm too); a `.desktop` path launches with `gio launch`; a missing command fails.
      `t_uwsm_app`. NOTES "Reproduce" by-hand steps rewritten from install.sh (they still had wayvnc).
    - *Bar widget*: Screenshot ran `omabox shot && xdg-open`, and xdg-open waits for the viewer on
      Hyprland, so every later action did nothing while an image was open; now only `shot` runs, and
      the viewer starts detached. The selection was an index, so a box coming up above it moved the
      cursor to another box (`s` shot the wrong one); it follows the name. Opened (IPC, a keybinding)
      with no boxes, the panel stayed logically open and popped up, taking the keyboard, when an agent
      started a box; closing it from `onOpenedChanged` is a binding loop that leaves `opened` true, so
      it closes on the next tick. A failing `omabox ls` was silent and kept the old list; it shows in
      the alert strip, and after three failures the list goes, with one notification. A command that
      cannot start (not on PATH) emits no `exited` in Quickshell: both processes watch `running` for
      that. A second action while one runs says "still busy" instead of vanishing. Settings: NaN or
      empty values fall back to the defaults (a NaN interval was a 0 ms timer); counts say "N up · M
      dead"; Enter on a dead row arms Down; the mark rounds its unit down and the hero reserves whole
      units (it drew 30 px in a 24 px item). `t_widget` runs it in a box's bar with a stand-in omabox.
      The real bar picks it up at its next shell restart (symlinked plugins do not hot-reload).
    - *CLI*: `ls` has an IDLE column (minutes since last use / the limit, or `never`); inside a box
      `OMABOX=1` (with `OMABOX_NAME`) is no longer read as a box named "1"; usage documents that a
      `run -d` job is not idle activity, `run`'s up options (for a throwaway), `--net isolated` for
      tests, `--no-shell`, `OMABOX_READY_TIMEOUT`, `shot FILE`, `env`/`path NAME`, and the read-only
      repo when `run` goes into a box that is up.
    - *Docs* (the docs review): the skill no longer says a box has no user manager or lists `systemctl
      --user` as a reason not to use one (`--systemd`); README's three "no systemd" spots; what of HOME
      goes in (mise installs, plugins, git identity) and what is refused (finding 63's rules);
      same-basename repos share a box; the uwsm-app stand-in is used with `--systemd` too; the file
      chooser portal was checked; the process tree in README and NOTES (session.sh, pasta, the systemd
      scope, box dir files); CLAUDE.md's tools list, the suite, and the rule about the input tools;
      stale notes in findings 40, 44 and the dead ends.
    - **Open, flaky:** `t_failed_up` failed once in ~8 full runs (a failed `up` left its box running:
      state up, bwrap alive; `down` cleared it). Not reproduced in 17 targeted tries, alone or next to
      other tests. The likeliest path is `kill_box` giving up after 10 s while PID 1 is still exiting,
      as with the dead throwaway above. The test now prints `up`'s own message when it fails.

65. **The agent guard: `omabox guard`** (2026-09-24, after a leak in another project). A worktree
    subagent ran a Qt test binary directly (`./build/tests/tst_qml`, not through ctest, which sets
    `QT_QPA_PLATFORM=offscreen`) and its windows flashed on the maintainer's desktop; another checked a
    keyring test with `secret-tool search --all` on the real keyring (it prints the secrets; only the
    test's fake one was there). Neither had loaded the skill: to them it was "running tests". omabox
    had not failed, it was never called, and nothing failed closed: the agent's shell holds the real
    display. The skill cannot list every binary that opens a window.
    - *The guard*: opt-in, one `SessionStart` hook in Claude Code's user settings that appends
      `export WAYLAND_DISPLAY=omabox-guard HYPRLAND_INSTANCE_SIGNATURE=omabox-guard DISPLAY=` to
      `$CLAUDE_ENV_FILE` (and, since finding 66, an empty `QT_QPA_PLATFORMTHEME`) and prints one line telling the agent to use omabox and never set those back.
      A fake socket name, not an empty one: libwayland falls back to `wayland-0` when unset, and hyprctl
      with an empty signature says only "not set"; with `omabox-guard` both errors name it. `DISPLAY`
      empty: Xwayland's `:0` is live. Checked in headless `claude -p` runs: the main agent's and a
      subagent's commands both get it, `hyprctl` fails with `…/hypr/omabox-guard/.socket.sock`, the
      line reaches the context. Not the settings' `env` block: it reaches subagents too, but it also
      changes Claude Code's own process (its hooks saw it), which would break clipboard image paste.
    - *omabox under it* (`host_session`): the host's session is the environment's when that names a
      live one, else the only one `hyprctl instances` lists (it works under any signature), its Wayland
      socket from the same list; several and none named: refused. `up --interactive`, `peek`, `--size
      host` and every host `hyprctl eval` go through it. Box commands never used the host's display.
      `t_guard` runs all of it under the guard inside a box standing in for the host (finding 26):
      interactive window on workspace 9 without focus, peek's window, `--size host` = the stand-in's mode.
    - *`omabox guard [on|off]`*: merged with jq (other hooks and keys kept, `off` gives back the same
      file), a `.bak-<ts>` per change and none for a no-op, mode kept, a linked settings.json stays a
      link (its target changes), `CLAUDE_CONFIG_DIR` honoured, a file that is not a JSON object refused
      untouched; an older hook of ours reads `outdated` and `on` replaces it. `install.sh` asks, only in
      a terminal.
    - *Qt aborts with no display* (qFatal), so a Qt binary run under the guard dumps core, and
      `omarchy-crash-watch` announces every core dump of the user's as a critical notification
      ("Process crashed", through Do Not Disturb, deduplicated per program for 60 s): no window, but a
      toast. Found when the suite's first Qt check did exactly that (two toasts); it checks with
      `wl-paste` now, which exits cleanly and names `omabox-guard` in its error. GTK apps exit cleanly.
    - *What it does not stop*: an agent that sets the variables back on purpose (the skill forbids
      it); the session bus: notifications, the keyring, D-Bus-activated apps, a running browser opening
      a URL, `systemd-run --user`; processes the agent starts itself rather than through its shell
      (MCP servers, a headed browser MCP among them: they have the real display). A fence needs the
      agent itself sandboxed. Checked 2026-09-24: Claude
      Code's `sandbox` (`allowUnsandboxedCommands: false`) blocks every Unix socket (Wayland, X11's
      abstract one, the session bus, hyprctl) and all network by default; too wide to be omabox's
      default (ssh-agent, docker, gh's keyring, omabox's own box sockets need exceptions). ai-jail
      (bwrap + seccomp around the agent) hides the display and the bus together, and blocks
      `unshare`/`setns`, so omabox cannot run inside it.
    - *Also fixed*: `up` exited 1 with no message when `/etc/resolv.conf` links into a directory that
      does not exist (`readlink -f` fails under `set -e`): inside an isolated box, or a host with its
      resolver stopped. Found running `up` nested in `t_guard`.
66. **Feedback from the first project to use the guard** (2026-09-24, the same day as 65). Fixed:
    - *An edit to bin/omabox broke commands already running* ("line 1207: q: command not found", then
      "unknown command: strict-walk"). `~/.local/bin/omabox` links to the working copy, and bash reads a
      script as it runs it: an edit written in place (same inode) changes what a running command reads
      next. The dispatch is now `main()`, called on the file's last line (`main "$@"; exit`), so bash
      has read everything before any of it runs; `share/session.sh`, which runs for a box's whole life
      from the repo, is one `{ … }` block. `t_unit_live_edit` rewrites a pausing copy mid-run: the old
      layout fails with exactly that error (127), the new one finishes. Kept the link (a pull or an edit
      applies at once); edits by agents here are also written to a new file and renamed over.
    - *`omabox run` drops the caller's variables* (`env -i` with the box's environment, by design), so a
      project's strict live tests skipped silently for want of a password, and `--env KEY=VAL` would put
      it in the process list. `run --pass NAME` (repeatable) takes the value from the caller's
      environment and sends it through a pipe (bash's printf builtin, no argv) that the inner bash reads
      and closes before the command starts. An unset or malformed name fails the run. Checked: the value
      arrives as it was (spaces, `=`, a newline) and no `/proc/*/cmdline` contains it.
    - *The guard broke `ctest`*: Omarchy exports `QT_QPA_PLATFORMTHEME=gtk3`, which makes even an
      offscreen Qt program initialise GTK, and GTK exits with "cannot open display" when the display is
      gone. The guard blanks it too. Checked in a box under the guard: offscreen Qt with `gtk3` exits 1
      with that message, with it blank it runs. The check runs Qt under an `omarchy-crash-*` name with
      core files off, which omarchy-crash-watch never announces, in case it ever aborts.

67. **The guard, made livable: `omabox host`, crash notifications, Codex** (2026-09-24, continuing 65
    and 66, decided with the maintainer).
    - *It blocked work the user asks for on the real desktop* ("switch my theme", `hyprctl reload`
      after a config edit): the hook's note forbids setting the variables back, so the only way was the
      user's own `!` command. `omabox host -- CMD` runs one command with the session as omabox finds it
      (`host_session`), `DISPLAY` and `QT_QPA_PLATFORMTHEME` from the user manager (uwsm exports the
      session there), cores as usual, and names it on stderr. The skill allows it only when the user
      asked for their real desktop in the task. Nothing stops an agent using it unasked: any way through
      the agent can reach it can use; this one is never taken by accident and is on the record. A pause
      the user toggles was weighed and dropped: its flag file is just as writable by the agent.
    - *Qt aborts under the guard, and every abort was a crash notification.* A core limit of 0 does
      not help: systemd-coredump still journals the crash (`COREDUMP_RLIMIT=0`, no file) and
      omarchy-crash-watch announces journal entries. A limit of exactly 1 byte does: the kernel skips
      the core helper altogether ("RLIMIT_CORE is set to 1, aborting core" in the kernel log) and
      nothing is journaled. The Claude Code hook sets it (`prlimit --pid $$ --core=1:`, hard limit
      kept); so do boxes, for everything in them and what `run` starts (`nocore`; the launch line of
      launch.sh), since a crash in a box was a notification on the desktop too. Without a terminal Qt
      sent its reason to the journal, so the agent saw only "Aborted"; `QT_FORCE_STDERR_LOGGING=1` in the
      guard prints "could not connect to display". `QT_QPA_PLATFORM=offscreen` in the guard was
      considered and refused: it would hide the failure (an app running invisibly on the real bus).
      Checked in a headless `claude -p` under the hook: the variables, core limit 1, Qt's message, no
      crash in the journal, `omabox host` reaching the real display. All Qt checks run under an
      `omarchy-crash-*` name, which the notifier never announces, in case a limit does not hold.
    - *Codex*: `shell_environment_policy.set` in a marked block of `config.toml`, edited as text and
      checked with python's `tomllib` (a clashing `set` table of the user's is refused with the lines to
      add by hand; an existing `[shell_environment_policy]` is kept). Checked through `codex sandbox`
      (it applies the policy: `WAYLAND_DISPLAY` came out `omabox-guard`); no Codex login here, so not
      in a session. No core limit or note for Codex. Any other agent: `omabox guard exec -- AGENT`.
      omarchy-in-omarchy (the VM) was read for its agent integration: a skill linked by hand, invoked
      when asked ("/vm"); nothing enforced.
    - `omabox guard` reports each agent even when one's config cannot be read; a backup per change
      is named to the nanosecond (two changes in one second overwrote the first backup).
    - Left open: the session bus and the user manager (`uwsm-app`, `systemd-run --user` start things in
      the session's environment, display included); OpenCode, pi and Hermes have no guard of their own.
68. **A second machine: the suite on a laptop (Intel iGPU)** (2026-09-25, a second
    Omarchy laptop, a clean clone and `./install.sh --check`). First run 278/281. Two failures were the
    suite's: install.sh ends with `omabox up && omabox shot`, which from the checkout starts a box
    named `omabox`, and the throwaway tests ran `run` from `$ROOT`, whose default name is that box, so
    `run` used it (`--net bogus ignored`, a read-only repo instead of an overlay). They run from a fresh
    repo named `$P-*` now (`tmp_repo`), and the `~` one refuses clearly while a box named `default` is
    up. Reproduced here with a box `omabox` up: the old suite fails the same two, the new one passes.
    The third failure, the host's focused window, was most likely Omarchy's screensaver (a
    fullscreen window, idle during the ~4 min run): turn on Stay Awake for a full run. Rerun with the
    box down and Stay Awake on (before this fix), the laptop passed 281/281. Also: `t_unit_guard_exec_host` read `$HYPRLAND_INSTANCE_SIGNATURE`,
    which is the guard's when an agent runs the suite; it uses `host_session` now.
    By hand on the laptop: `--size host` (1920x1080@60), interactive (window, passthrough, resize,
    close), `peek` (the view scales, the box keeps its size, as intended) and the bar widget all
    worked. Omarchy's About looked different in the box: finding 69.
69. **The box HOME lacked Omarchy's terminal setup** (2026-09-25, seen on the laptop, reproduced here).
    Omarchy's About opened tiled instead of floating and centred, in foot's default colours.
    Omarchy's install puts its own `foot.desktop` in `~/.local/share/applications`, with
    `X-TerminalArgAppId=--app-id=`; without it `xdg-terminal-exec` drops `--app-id`, the window's
    class is `foot`, and no `org.omarchy.*` rule matches (About, and every TUI launched through
    `omarchy-launch-tui`). `~/.config/foot/foot.ini` is what includes the theme's colours. `seed_home`
    now copies Omarchy's desktop entries, the user's foot/alacritty/ghostty/kitty configs and
    `xdg-terminals.list`, and `~/.config/omarchy/branding` (About sizes its window from `about.txt`).
    Checked in a box: About is `org.omarchy.about`, floating, fitted and centred, themed. A check in
    `t_main`.
    Then the audit of what else the box HOME lacked. Packaged Omarchy seeds a new user from
    `/etc/skel` (owned by omarchy-settings): configs for the apps it ships, the desktop entries, the
    menu extension, `~/.local/state/omarchy/toggles/hypr`, the migrations marked done. The box HOME
    starts from it now (minus nvim's 85 MB of plugins and the XDG autostart entries), then the user's
    look on top: terminal configs, branding, `extensions`, custom `themes`, the Hyprland toggles. Never
    `~/.config/omarchy` wholesale (`api-keys.env`, `hooks`). The skel's `~/.config/hypr/*.lua` are
    comments only, so the box's own `hyprland.lua` still stands in for them; but stock Omarchy ends
    with `require("default.hypr.toggles")` and the box's did not, so `omarchy-hyprland-*-toggle` did
    nothing in a box (gaps stayed at 10 after the gaps toggle; now 0). Checks in `t_main`. Left out on
    purpose: the user's `~/.config/hypr` (monitors, binds), `~/.XCompose` (may carry name/email),
    Omarchy hooks. A box HOME is ~13 MB.
    Verified on the laptop too (after a pull): About in an interactive box floats, centred and themed.
70. **Settings: where windows open, confirm before closing** (2026-09-25, asked for by the maintainer).
    `~/.config/omabox/config` (KEY=VALUE), changed with `omabox config`, and by the bar widget's
    panel through the same command, so the two never disagree. The installed Omarchy (4.0.4) has no
    screen that renders a widget's settings schema (`settingsForm`/`schema` are metadata only;
    values are hand-edited in `shell.json`), so the panel has a Settings section of its own
    (Dropdown, Toggle from `qs.Ui`); it shows while boxes exist, like the panel.
    - `workspace`: where interactive and peek windows open, still `silent` and without focus: 1-99,
      `special` = `special:scratchpad` (Omarchy binds SUPER+S to it) or `special:NAME`. It goes into
      the host's Lua, so only those shapes pass; a bad value in the file is ignored with a warning.
      `up --interactive --workspace`, `peek --workspace` per window. An interactive box on the
      scratchpad starts fine while hidden (the host does not render it; `shot` needs it shown; see
      finding 90: it is drawn while hidden now).
    - `confirm-close`: closing an interactive box's window asked nothing and ended the box. With it
      on, the box's Hyprland (hyprland.lua, `monitor.removed` with no monitor left) runs
      `share/confirm-close.sh`: `hyprctl output create wayland` opens a new window (it lands on the
      host's active workspace with focus, as any new window: the user just closed one there), a
      Hyprland notification, and Omarchy's menu (`omarchy-menu-select`) with "Shut down" / "Keep it
      running". A close while it asks is a yes; with `--no-shell` there is no menu, only the
      notification and the second close. The box reads a flag in its runtime dir, so `omabox config
      confirm-close` reaches running boxes that took it from the config (`--confirm-close` /
      `--no-confirm-close` on `up` pin a box's own); written with O_EXCL, as the box can write there.
    - Two things learned: a reload starts a fresh Lua state (globals do not survive; "asking" is a
      file), and the new window's output is WAYLAND-2, so the monitor rule is now for any output
      and the resize watch follows whichever exists.
    Checked in a stand-in host (a box, finding 26): windows on 3 and on the scratchpad, no focus;
    keep, a second close, the menu's "Shut down", `--no-shell`, confirm off, the live change,
    resize after the new window; the widget's section in a box (toggle and dropdown write the
    file). Checks in `t_unit_config`, `t_guard`, `t_widget`. Not yet on the real desktop.
71. **An interactive box closed by its user no longer stays "dead"** (2026-09-25, seen by the maintainer
    after confirming a close: the widget showed "dead · Down cleans it up"). Interactive boxes had
    no reaper (it only ran for an idle limit or a `run` owner), so nothing cleared the dir. Now
    they get one (every 2 s), and it clears the box only when it ended on purpose: hyprland.lua
    (window closed) and confirm-close.sh ("Shut down") write `omabox.closed` in the box's runtime
    dir before exiting. Any other death (a crash) stays dead with its logs until `down`, as before.
    The marker is box-writable, but all it can do is get that box's own dir cleared. Checked in a
    stand-in host: closes (confirm on and off) leave no box; `pkill -KILL Hyprland` leaves it dead.
72. **The widget can stay in the bar: `bar-icon`** (2026-09-25, asked for by the maintainer once the panel
    held settings). `omabox config bar-icon auto|always`; `always` keeps the icon, dimmed, with no
    box up, and the panel then opens to "No boxes up" and its settings. The widget needs the value
    while its panel is closed, so it reads `config --json` at start and when the file changes (a
    `FileView` watch; `omabox config` renames a new file into place and the watch still fires),
    not on every poll. A watch sees the file appear only if `~/.config/omabox` exists: install.sh
    and box HOMEs create it. Turning it off from the panel with no box up hides the icon at once;
    it comes back with a box, or with the CLI (the toggle says so). Checked in a box: the icon
    follows repeated CLI changes, the file's first creation, and the panel's toggle.
    Default `always`; a missing or unreadable
    value counts as `always` in the widget too.
73. **The widget's Settings face, and "New interactive box"** (2026-09-25, asked for by the maintainer).
    Settings moved out of the list into a face of its own, as omawin does it: a gear at the top
    right of the hero, the hero turning into "Settings / omabox VERSION" with a bordered ‹ Back in
    the gear's place, Esc going back before it closes; rows of a bold line, a dim caption and the
    control (ToggleSwitch, Dropdown), then the file and command as label/value pairs.
    New box: `omabox up --new` picks `box-N`, the first N with no box dir, under a lock held only
    until the new box's dir exists (the box's long-lived processes must not inherit it, or every
    later `--new` would wait), and prints the name. Started from the shell the default name would be
    "default" for every box (no repo there). The widget runs `up --interactive --new`, then
    `peek -b NAME --focus`: the user pressed the button, so the window comes forward.
    Checked: two `--new` at once got box-1 and box-2; the panel in a box (gear, switch, Esc back,
    `n` → up then focus); a stand-in host (the new box's window focused on its workspace).
71. **The README says what `down` does and does not undo** (2026-09-25, the maintainer asked whether
    trying plugins in an interactive box keeps them off the system, e.g. Omawin's polkit rules). Docs
    only, read from `up`'s bwrap arguments: read-only `/usr`, `/etc`, `/sys` (no sudo, polkit or
    system bus, so a `setup` writing `/etc/polkit-1/rules.d` cannot run inside), the pid namespace,
    the interactive window's single connection and the keys it receives, host network by default
    (internet, LAN, `localhost`, abstract sockets: no network namespace), effects outside the
    machine, host commands, and a shared kernel and GPU: not a security boundary.

74. **Review pass 3** (2026-09-25, before the first release: three reviewers over everything since
    pass 2, the guard, settings, confirm-close, the closed-box watcher, seeding and the widget's new
    faces). No injection found (workspace values and box names reach
    the host's Lua only checked). Fixed, each with a check in `test/run.sh`:
    - *Codex's MCP servers lost on `guard off`* (high): Codex keeps its file's last comment, our end
      marker, last, so a table it adds (`codex mcp add`, a trusted project) lands inside our block,
      which `off` removed whole; the state read `outdated`, so every install.sh asked again and `on`
      deleted it too. Now only our own lines go (our header, our keys with any value); a table inside
      stays; a stray key inside is refused, untouched. Checked with a real `codex mcp add` against a
      scratch `CODEX_HOME`.
    - *The suite toggled real settings* when `CLAUDE_CONFIG_DIR` or `CODEX_HOME` was exported: it
      only set HOME. run.sh unsets both.
    - *A reaper could take down a box just started under its name*: it decided on the old box, then
      waited in `down` for the lock the new `up` held. `down` from a reaper now checks, once it has the
      lock, that the box is still the one it watched (its creation time); the `.expired` note only
      when the box went.
    - *Host writes into a box's runtime dir followed what was there*: `omabox mode` wrote
      `omabox.mode` through a link the box could plant (to any file of the user's), and the
      confirm-close flag's `set -C` still followed a link to a FIFO (a hang of `omabox config` and the
      widget's switch). Both now write a new file in the box dir, which the box cannot see, and rename
      it over the name (`box_file`).
    - *A headless box that wrote `omabox.closed` and died was cleared*, logs and all: that path is for
      interactive boxes only.
    - *`seed_home` followed links* (`cp -rL`) in the terminal, branding, extensions, themes and
      toggles dirs: a theme linked from its repo brought `.env` and `.git/config`, a link to
      `api-keys.env` the keys. `seed_copy`: the dir itself may be a link (each theme followed that far),
      links inside stay links, no hidden files, `.git` or `node_modules`, and `refuse_src` applies.
    - *The widget's workspace dropdown stopped following the setting* after a pick (Omarchy's Dropdown
      sets its own value, which ends the binding): set again on every read. A pick made while the last
      was being written is queued, not dropped; a setting that succeeds clears the alert of one that
      failed. A single `n` no longer starts a box (twice, like Down), and a new box is brought forward
      only if the panel is still open (else a notification), so a workspace switch never lands later.
    - Smaller: `run --pass ROOT` (or `D`, `NAME`) sent omabox's own variable, now read from the
      caller's environment in /proc; a stale `HYPRLAND_INSTANCE_SIGNATURE` (a shell that outlived a
      crashed session) made `omabox host`, peek and interactive fail instead of using the live
      session; the Claude Code hook said "no display" even with no `CLAUDE_ENV_FILE` to write to;
      a `confirm-close` change during an interactive `up` was lost for that box; `omabox config`
      replaced a config linked from dotfiles with a file (now written through, under a lock);
      install.sh asked about the guard on every re-run, yes by default (a "no" is remembered in
      `~/.config/omabox/guard-declined`), and a failing `guard on` stopped it.
    - Documented, not fixed: MCP servers and anything else an agent starts itself are outside the
      guard (finding 65's list).
75. **`omarchy-theme-set` ended with "pkexec must be setuid root" in a box** (seen while making the
    README's demo). Its last step writes the theme colour into `/etc`'s browser policies through sudo
    or pkexec; a box has neither. A silent stand-in for `omarchy-theme-set-browser-policy` in
    `share/bin` (first on the box's PATH): the host's policies are not a box's to change.
76. **The README's screenshots and video are made in a box** (`docs/demo.sh`): a box plays the
    desktop at 1280x720 (Omarchy's stock bar and a stock theme, the widget, a terminal), and a second
    box runs inside it (bwrap nests, finding 26), driven from the terminal as an agent would. Peek and
    interactive windows open on workspace 1 there (the stage's own `omabox config`). The video is
    `wf-recorder` inside the stage (it works on a box's headless output; `gpu-screen-recorder` needs
    the real screen or a portal). wf-recorder writes frames on damage only, and with `-r` it waited
    for one forever on a still screen, so the script records on damage and ffmpeg makes it 30 fps.
    Keyboard focus does not go back to the terminal when the widget's panel closes: the script
    clicks it, as a person would.
    The README links the video on GitHub Pages (`diogochaves.github.io/omabox/docs/media/demo.mp4`,
    as omaroll does), since a repo file does not play inline; Pages must be on (main, root) for it.
77. **NVIDIA needs its userspace nodes and a Wayland screen** (2026-09-25, RTX 4070 SUPER, driver
    615.71.09). With only `/dev/dri/renderD128`, nested Hyprland aborted in `CBackend::create()`.
    Bind `/dev/nvidiactl` and the `/dev/nvidiaN` matching the render node's PCI slot and device
    minor; never bind `/dev/dri/card*`, input or the host display. A GBM probe then allocated XR24
    buffers, and aquamarine's `WAYLAND-1` swapchain worked, but its synthetic `HEADLESS-2` still
    rejected buffers because NVIDIA cannot render to the linear layout that path requests. Keep the
    private labwc Wayland output as the box screen on NVIDIA. Set the parent's mode with wlr-randr,
    then reapply Hyprland's mode after its initial 1280x720 xdg configure; `omabox mode` changes both.
    A custom bar without `omarchy.tray` does not start `org.kde.StatusNotifierWatcher`: readiness
    waits for it only when that widget is present. This machine's driver exposes no `drm-engine-*`
    counters in `/proc/*/fdinfo`, so `omabox gpu` reports no per-process figures. Default and
    `--net isolated` boxes, 1920x1080 screenshots, input, live resize and host focus were checked;
    broader NVIDIA hardware remains untested.
78. **Check the NVIDIA resize helper before starting a box** (2026-09-25). The NVIDIA headless path
    calls `/usr/bin/wlr-randr` to size labwc's private output. `install.sh` installs it, but an
    incomplete or older installation could fail only after `up` created the box directory. `up`
    now checks for the helper alongside its other installed files before creating the box, when the
    render node's driver is nvidia: AMD and Intel boxes never run it, so an install from before this
    change keeps working without it. The agent skill also states the missing per-process GPU counters observed with driver 615.71.09.
79. **Make the failed-start cleanup test deterministic** (2026-09-25). The old test relied on a
    one-second startup timeout, but a warm box completed within that second on this machine and
    left the test's expected dead-box assertions failing. The test now suppresses the box's shell
    through its environment while `up` still waits for that shell, so readiness must fail and the
    cleanup path is exercised even when startup is fast. In a nested box, bwrap may take a fraction
    of a second to exit after `up` returns, so the cleanup checks poll for completion.
Findings 80-86 started from reading Cua (github.com/trycua/cua, MIT), a computer-use agent platform;
the designs here were measured in boxes and built for a contained desktop, and no code was taken from it.
80. **The suite, hardened** (2026-09-25). A suite that passes must have checked:
    - *No silent passes*: checks that depended on the machine (`[ -f … ] && check`, `command -v
      zenity`) passed unseen when skipped; they are `skip`s now, counted and listed at the end, and
      fail under `--strict` / `OMABOX_TEST_STRICT=1`. A test that runs no check fails, a PATTERN that
      matches no test exits 2 (`test/run.sh zzznomatch` passed with 0 checks), `t_unit_registry`
      fails for a `t_*` function missing from UNIT/BOX, and an unfiltered run needs 340 checks (175
      of them unit; 360 ran). The runner is one function (an edit mid-run cannot change it).
    - *Provenance*: the first line names the checkout (sha, dirty), the Hyprland boxes start and the
      host's running one, aquamarine (the box's and the system's), quickshell, labwc, bwrap, the
      render node and driver, and the kernel.
    - *Evidence*: a failure's output was cut at 300 characters, `until_ok` threw its command's output
      away, and nothing outlived the run. Each run has a folder,
      `~/.local/state/omabox/test/<date>-t<pid>/` (0700; the last 5 runs, never one still going):
      the provenance, every failure's whole output, and at a test's first failure, while its boxes
      are up, each box's screen, clients, layers, focus, cursor, devices and log tails, and the
      last wait that timed out. `until_ok` says what it last saw when it times out, and notes a wait
      that took over half its deadline (the widget's "cannot run" notification took 13 of 20 s, the
      killed throwaway's teardown 9 of 15: the next slower machine is where those fail).
    - *Waits instead of sleeps*: 20 fixed sleeps became waits for what they stood for (the reaper
      process gone once it has decided, the widget stub's log of each `ls` poll and settings read,
      the panel's layer, the scope, the servers), and "stays so" checks use `holds T CMD` (true now
      and all of T) instead of a sleep and one look. A full run: 237 s to 214 s on this machine.
      The sleeps left are the scenario itself (`t_run_idle`'s use, a key sent mid-`down`), a reload
      whose end has no signal, and an X11 window that takes keys a moment after it maps.
    - *A leak detector, and its positive control*. The only host checks were two compares at the
      end (focused workspace and window): a leak that was undone before the end passed, and you
      switching windows during a run failed it (a run ended 359/1 on that, cause unknown). Hyprland's
      event socket (`.socket2.sock`) reports `openwindow`, `activewindowv2`, `workspacev2` and
      `activelayout>>hl-virtual-keyboard-…,…` (what `omabox keys` causes) live, reverted ones
      included; the host log is no use (debug is off: no virtual keyboard lines). The suite listens
      to it for the whole run (read-only, as a bar does; nothing is written to it), a python watcher
      logging to the run's folder with a marker per test. On each focus change it asks for the
      focused window (`hyprctl -j activewindow`, the read the end checks already did) and reads that
      process's `/proc` environ for `OMABOX_SUITE` (exported by the suite) and `OMABOX_NAME` (what
      `run` gives a box's processes); for an interactive box's window (class `aquamarine`, a title
      naming no box) its bwrap's command line names the box dir. After each test, the test fails on
      a window, or focus, that is this run's, an interactive or peek window (by title, `omabox peek:
      NAME`), any box's process, or a virtual keyboard's layout event (`OMABOX_TEST_HOST_KEYBOARDS`
      names your own: wayvnc, an input method); everything else is one "not the suite's" note. The
      end compares stay, but a change the log shows was not the suite's passes, named.
      `t_leak_control` runs first among the box tests: the same watcher, inside a box standing in for
      the host, sees nothing while quiet, then a box's window taking focus, a workspace switch and
      back, and a key from `omabox keys` are leaked into it on purpose, and each must be reported;
      if it fails, the host verdict fails ("a clean host log proves nothing"). Proven by breaking
      it: with the watcher no longer logging layout events or reading `OMABOX_NAME`, the control
      failed both checks and the host verdict failed; restored, it passes. `t_unit_leak_scan` checks
      the reading of each event, on lines as Hyprland 0.56 sends them.
      What it cannot see: pointer motion and clicks have no event (seen only when they move focus),
      a workspace switch has no owner (a switch alone that ends where the log shows passes), an
      interactive box's `openwindow` has no pid (while an interactive box of yours started during
      the run exists, such a window is taken as yours, so one of the suite's opening then would be
      missed unless it also took focus), and a window of the suite's that closed before its focus
      was looked up is unattributed. A run
      under the guard or from a guarded shell finds the host session the CLI's way.
      A full run of 80-86 together (while the maintainer worked) failed five tests and the
      end check on `activelayout>>hl-virtual-keyboard-fcitx5`: Omarchy's input method re-sends its
      layout on every focus change of the user's. `omabox keys` is anonymous there
      (`hl-virtual-keyboard-unknown`, seen in a stand-in box), so fcitx5 is always the user's now.
      Merged with main on 2026-09-28, where PR #13 had meanwhile added a watch of its own: `hyprctl`
      polled every 0.5 s for a window of the suite's boxes on the desktop and for omabox's workspace
      coming up, one verdict at the end. This watcher stays (it sees reverted leaks and keys, and
      names the test); #13's rule is taken into `leak_scan`: omabox's workspace (`omabox config
      workspace`) coming up is a leak, not a note, unless the run started on it. `t_leak_control`
      switches its stand-in to 9 and back and requires that leak. The suite already needed python3
      (its throwaway servers), so the watcher adds no dependency.
81. **Windows as targets: `omabox windows`, `shot/click/keys --window`, `shot --fit`, `click --in`**
    (2026-09-25). No new tool or protocol: the box's grim 1.5 has `-T ID` (ext-image-copy-capture of a
    foreign toplevel), and Hyprland's id for it is the `stableId` `hyprctl -j clients` reports (hex).
    Seen in boxes: the capture is the window's own pixels, its size exactly the client's `size`, with
    no border, nothing that covers it and no trace of the cover; a window on another workspace, on
    `special:scratch`, an inactive group tab and an XWayland window capture too (the last three in
    the analysis boxes only), 21-36 ms each; a bad id exits 1 ("cannot find toplevel") and an idle
    window does not hang it. **The first capture can be stale**: a window off screen that keeps
    redrawing (a clock in foot) gets no frame callbacks, stops drawing, and its first capture was the
    frame from 15 s earlier; one ~100 ms later was current (the export asks it to draw). So a window
    shot is always primed: captured, 100 ms, captured again (`t_window` fails without it). A window
    that draws once after being idle renders at once and was never stale. Other facts it rests on:
    `at` excludes the border (border 2 + gaps 10 → at 12), the order of `hyprctl clients` is the
    stacking order (later on top) except that floating windows stay above tiled ones even after
    `alter_zorder top` on the tiled one, fullscreen above the rest and a shown special workspace
    above its workspace; `visible` is true for windows on hidden workspaces (useless: on screen =
    the workspace is shown and `hidden` is false). From the analysis boxes only: the shell's layers
    (bar, menus, panels) are all full-screen surfaces, so they are not counted as covering anything
    and there is no `--layer`; a popup or menu past its window's edge is clipped in a window shot
    (whole in a full one).
    Focusing a window (`hl.dsp.focus({ window = 'address:…' })`) shows its workspace, opens a special
    one, brings an inactive tab to the front; a float also needs `hl.dsp.window.alter_zorder`. A
    click right after that workspace switch (0.18 s, animations on) landed in the window (foot's
    mouse reporting read the right cell).
    - SEL (`bin/omabox`'s usage) picks exactly one mapped window: none or several is exit 2 with the
      candidates, never the first match (the resolver is jq, `WIN_JQ`/`WIN_SEL_JQ`, unit-tested on
      made-up JSON). `click --window SEL X Y` takes window coordinates, refuses a point outside the
      window, focuses the window when it is off screen or something covers that point, checks again
      and refuses naming the window still on top (a tiled window under a float stays under it);
      `--no-raise` refuses at once. `keys --window` focuses, waits for it, then types.
    - Screenshot scale (an agent loop could downscale every shot and rescale its clicks). Measured
      with Opus 5.5 reading shots through Claude Code: 1920x1080 is seen 1:1 (max error 0.7 px);
      2560x1440 and 3440x1440 are downscaled by Claude Code to 2000 px wide with a "multiply by
      1.28/1.72" note: with the factor max 1.5 px off, without it 544/1364 px. So full resolution
      stays the default; `--fit N` caps the longest side (grim `-s`, rounded up since grim truncates).
      The likelier miss was cropped shots (`--active`, `-g`): the image starts at the crop's origin
      but `click` takes screen pixels.
    - So every shot is recorded in `$D/shots.tsv` (the box cannot see it): path, size and mtime, the
      box's mode, the window's address or the region's origin, the size covered and the image's.
      A cropped or scaled shot says so in one stderr line (stdout stays the path). `click --in SHOT X
      Y` and `pointer --in` take that image's pixels: the screen pixel under the pixel's centre, from
      the window's *current* position for a window shot (it may have moved; resized is refused), from
      the fixed origin for `-g`. A file that is not a shot of this box, changed since, or a mode
      change is refused. `--active` is now `--window active` (the window's own pixels instead of a
      crop of the screen); install.sh checks that grim has `-T`.
    - Not done: occlusion ignores layers that take input (an open panel), and pinned or
      override-redirect windows only approximately follow the stacking rule; an app that stops
      rendering when hidden and ignores the export's frame callback would still come out stale (only
      foot checked). An interactive box whose window is hidden is drawn since finding 90, so `-T`
      gets frames too (assumed, as for a plain `shot`; not checked for `--window`).
    - Review of PR #17 (2026-09-28): `click --window` and `pointer --in` with a selector matching
      none or several exited 1 with "outside window  ()": `win_point` runs in its caller's `$(...)`,
      where errexit is off (no `inherit_errexit`), so `win_select`'s exit 2 fell through. Now `||
      exit $?` there; `t_window` checks exit 2 for both cases.
82. **`omabox wait` and `--wait` on keys, click and run -d** (2026-09-25). Agents slept
    between actions and guessed how long. Measured in the analysis boxes (1920x1080@60, shell): a
    menu settles ~130 ms after its key, a notification ~240, typing in foot ~210, a terminal ~660;
    first to last frame of an app launch 300-850 ms (zenity) to 650-1200 (chromium); the largest gap
    between two changed frames inside any animation 51 ms, so 300 ms of quiet is a 6x margin. An idle
    box changes no pixel in 65 s.
    - **How**: `tools/still` (`omabox-still`, bound at `/opt/omabox/bin` like the keyboard and pointer,
      refusing to run outside a box) asks for frames with wlr-screencopy's `copy_with_damage`, which
      Hyprland only completes when the output is drawn again: waiting on an idle screen costs
      nothing (0 CPU ticks in 10 s; a 20 Hz repaint of a 909x1020 window: 26 ticks in 10 s, 2.6% of a
      core). Its damage rectangles are always the whole output and some frames come with damage but
      the same pixels, so every frame is compared with the one before, pixel by pixel. Before it
      answers "nothing changed" or "still" it takes one plain copy (never waits) and compares that
      too, so a change drawn between two requests cannot be missed.
    - **Settle** (`--wait`) = the screen before the action (the tool prints `ready` on its first
      frame, then the CLI acts), a change within `--start` (2 s; 5 s for `run -d`), then `--quiet`
      (300 ms) without one, so it cannot pass before a slow reaction has begun, as a check for a few
      stable samples in a row could. No change at all is `unsatisfied: nothing changed in 2.00s` (exit 124) and stderr says the input
      was sent anyway: the agent should look, not resend. `wait still` is quiet from now, `wait
      change` the first change; `-g` or `--window SEL` watch a region (a window's place on the screen:
      one off screen is refused).
    - **Not changes** (said in the line, `--strict` counts them): a change 4 px or thinner in either
      direction (a caret: foot's beam blinks as 2x20, reported `caret?`) and the software cursor, which
      is in every frame (headless, `no_hardware_cursors`) and hides on a key press: seen as 16x27 from
      1 px up and left of its hotspot, and right after `up` a cursor change reaching ~38 px below it,
      so the ignored rectangle is 64x64 from 16 px up and left of where the cursor is before the
      action, and where a click sends it. (It was 56x56 from 8 px until the first NVIDIA run,
      2026-09-28: over a terminal the cursor is foot's I-beam, centred on its hotspot and reaching
      ~10 px above it; when a click moved it, the I-beam's top row at its old place fell one row
      outside, and `click --wait` on a terminal read "settled" instead of 124. Deterministic there,
      not seen on AMD; why the old place was redrawn only on NVIDIA is not known.)
    - `wait window SEL [--gone|--focused]` (the resolver of finding 81; several matches were exit 2
      until finding 105, now any of them answers it; `--gone` counts any), `wait layer NAMESPACE
      [--gone]` and `wait cmd -- CMD` (exit 0 inside the box) poll every 100 ms; a window or layer
      must hold on 2 polls in a row. Absence can be asserted (`--gone`).
    - **Exit 0 satisfied, 124 unsatisfied at `--timeout` (10 s; at most 10 min), 1 unknown**, never 0
      for what could not be seen: the box went down mid-wait (`unknown: box 'x' went down after
      1.50s`), or an interactive box whose window is hidden, which renders nothing (finding 24).
      Found in a stand-in host: right after `up --interactive` the tool did get a first frame (drawn
      before the window was hidden) and then nothing, and waited out the whole timeout; so the
      plain copy taken before an answer must come within `--first` (2 s) too. Now `unknown: box
      'inner' not rendered (its window is hidden on workspace 9)` in 2.05-2.6 s, every time.
      Since finding 90 (merged here 2026-09-28) a hidden interactive box is drawn, so `wait` settles
      on it (`t_guard` requires an answer there, never `unknown`: with animated gears in the box it is
      124, still changing). `unknown ... not rendered` is left for a box that
      is not drawn while hidden, and says why as `shot` does: started by an older omabox, or its
      window replaced after a confirm-close keep; either way, ask the user, never show the window.
    - One line on stdout (`satisfied: settled after 0.40s (last change 0.08s at 0,12 1920x1068;
      ignored 64x64 at 944,524: cursor)`) or `--json`. Waiting is use (idle expiry, finding 59):
      `need_box` touches `used`, and a long wait again every 30 s. The tool's stdin is a pipe from
      the CLI (`--tied`): when the CLI goes (Ctrl-C, an action that failed) the tool ends with it
      instead of holding the box's socket until its timeout.
    - Seen in boxes: `wait still` on an idle box 0.31-0.36 s; `keys --wait -t hello` into foot settled
      in 0.34-0.42 s; SUPER+SPACE (the menu) 0.40 s, Escape 0.37 s, then `wait layer omarchy-menu
      [--gone]` 0.12 s; `run -d --wait -- foot` 0.85-1.23 s; `shift` alone: 124, nothing changed.
    - Not done: late content passes `still` (a list filled from the network after a pause of more
      than `--quiet`: wait for a title or a `cmd`); a thin progress bar or a spinner 4 px wide is
      ignored like a caret (said, `--strict`); Qt's and Chromium's carets, GTK4's (1x18 in the
      analysis) and the busy cursor's exact size are not measured here; the cursor rectangle assumes
      Hyprland's default cursor size (24).
    - Review of PR #17 (2026-09-28): `wait still -g` on a region off the screen answered satisfied,
      having watched nothing: `omabox-still` now answers `unknown off-screen` (exit 1) before
      `ready`. `--start`/`--quiet` over 10 min passed the CLI, the tool refused them, and keys, click
      and `run -d` acted all the same, then said the screen was lost: the CLI caps all three now, and
      `settle_ready` sends nothing when the tool gives no `ready` (unknown, exit 1). Checked in
      `t_unit_wait` and `t_wait`; the no-`ready` path by reading only (nothing left in the CLI that
      reaches it on purpose). `click --wait` on a small toggle still reads 124: its change sits under
      the cursor's ignored rectangle (the skill's symptom table says so).
83. **`shot` compresses less: PNG level 1** (2026-09-25, seen while measuring 82). Most of a shot's
    time was grim's PNG compression, for a file an agent reads once. Measured in a shell box with
    a terminal full of text (1920x1080, 5 shots each): grim's default level 124 ms and 739 KB a
    shot, `grim -l 1` 46 ms and 915 KB (+24%); `omabox shot` end to end 160 → 82 ms. The analysis
    box (another screen) had 621 → 100 ms and +13%. Pixels are the same (PNG is lossless).
84. **`omabox keys --pass VAR`** (2026-09-25). An agent typing a
    password into a login form had only `keys -t "$PW"`, which puts it in `omabox`'s, `nsenter`'s and
    the keyboard tool's argv: any user's `ps` shows it. `--pass VAR` (repeatable, in order with the
    other tokens) takes the value from the caller's environment as `run --pass` does (finding 66,
    `caller_env`) and hands it to `tools/keyboard` on stdin, NUL-terminated (bash's printf builtin, no
    argv); the tool's new `-T` token reads one text per `-T` before it checks or types anything and
    types it as `-t` would, from an argv copy in memory. Checked in a box (`t_keys`): the value (spaces,
    `=`, Ü) lands in a terminal exactly, in order with `-t`, and no `/proc/*/cmdline` has it while the
    tool is typing; `-t -T` is still text; an unset or malformed name fails before anything is sent.
85. **Marks in peek: what the agent does, over the view** (2026-09-25).
    A box's cursor is already in every frame: headless Hyprland draws it in software
    (`no_hardware_cursors`), so it is in every `omabox shot` and peek too (`grim -c` changes nothing),
    it starts at the screen's centre, and it hides on any key press (`cursor:hide_on_key_press`). Tiny
    at peek's scale, gone after keys, and it says nothing about clicks or keys. (Every agent screenshot
    has that arrow in it: noise for anything that compares shots, and it can cover UI.)
    Now `click`, `pointer` and `keys` append a line to `<box dir>/marks` (`ptr WxH <pointer commands>`,
    `combo KEY`, `text TEXT` (≤200 characters, control characters as spaces), `secret N`: a `--pass`
    value only as its length) once the tool has sent them, and only while a peek window of the box
    is open; otherwise the file is removed. The box dir is out of the box's reach, so nothing in a box
    can forge or read marks. Mode 0600, emptied past 64 KB, fresh for each `peek`, gone with `down`.
    `tools/peek --marks FILE` reads it from its end on inotify in the same poll loop, validates each
    line whole (anything else, or a line over 1 KB, is ignored) and draws, in a desynchronized
    `wl_subsurface` over its window (empty input region): a ring that glides (120 ms) to where the
    pointer went, a ripple per click (`right`/`middle` labelled), and key captions at the bottom
    (keys within 1.5 s share one, at most 3, the last 40 characters; ASCII only, from the public-domain
    font8x8, `tools/peek/font8x8.h`), all faded out 3 s after the last one. The subsurface has frame
    callbacks of its own and is only drawn while something is on show, then unmapped: an idle or
    hidden peek costs what it did. Only in the peek window: the box's own frames and every shot stay
    clean. No new dependency (wayland-client; no libm). Checked in a box whose peek shows its own
    screen, with a process named like a host peek relaying `<box dir>/marks` into it (`t_peek`): the
    ring's centre within 8 px of the clicked point, a caption, a secret mark without its value, all gone
    after 3.5 s, junk lines drawing nothing and not stopping a good line after them, the 64 KB reset,
    no file without a peek window, CPU after the marks no higher than before them, still idle hidden.
    And end to end in a box standing in for the host (finding 26): `omabox up` and `omabox peek` inside
    it, `click`/`keys`/`keys --pass` against the inner box, the marks on the stand-in's screen, and
    nothing left after 3.5 s. Not done (the design's trimmings): a pointer trail, scroll marks, a
    setting to turn marks off.
86. **The skill says how to drive an app, and what to do when it goes wrong** (2026-09-25, written
    once 81-85 existed so it names real commands). Every
    rule traces to something agents did here: input sent twice (every `keys` call is delivered,
    finding 41), typing into a field that had lost focus (76), clicks where a direct route was there
    (the shell's IPC, a seeded HOME), `pkill -f` killing the agent's own shell (a stress-test
    report), `alacritty` missing in a box, two agents on one name. New: a "Driving an app" section
    (look, act once, look again; never resend what you have not seen land; type only into a field
    seen focused; set state directly; stop when a shot shows the goal), a symptom → next step table
    ending in real commands (`--wait`, `click --in`, `shot --window`, `keys --window`, `up --new`),
    `B=$(omabox up --new)` in the loop (its stdout is only the name: `t_new`). Instead of states such
    as partial or unverifiable input, one fact: every call is delivered. The direct
    route was read from Omarchy's source (`/usr/share/omarchy/bin/omarchy-shell`, `shell/shell.qml`),
    not guessed: `omarchy-shell shell summon|toggle|hide ID` for any plugin, third-party ones included,
    `shell call ID METHOD ARG` for a loaded panel's function; checked in a box (`shell summon
    chaves.omabox` opened the widget's panel, `hide` closed it, an unknown method answered
    `unknown`). What is not safety (screen size and GPU cost, waiting options, mounts, stubs,
    `--systemd`, Xwayland, cores, logs, the IPC list, peek and interactive detail) moved to
    `skill/reference.md`, one file (agents do not reliably follow a tree of links); the skill is linked
    as a directory for every agent, so it ships with it (`t_unit_install`). SKILL.md: 2,458 words on
    main, 2,818 with the lines 81-85 added, 2,487 now; the safety sections (the project table, the
    guard, tests, what a box cannot test, `/sys`, network and HOME) are all still in it.
87. **Apps installed in the box HOME are D-Bus-activated** (2026-09-26, issue #4; numbered after the
    `agent-driving` branch's 80-86). A per-user install (a `.desktop` with `DBusActivatable=true` in
    `~/.local/share/applications`, its service in `~/.local/share/dbus-1/services`) did not start from
    the launcher in a box: `ServiceUnknown ... not provided by any .service files`. The box's bus is
    dbus-daemon 1.16.2, which reads `$XDG_DATA_HOME/dbus-1/services`, and without that variable falls back
    to the passwd entry's home (`/home/USER`, not mounted in a box), not `$HOME`. A box had none of the
    XDG base dirs; the host has them from the
    systemd user environment (and runs dbus-broker). bwrap now sets `XDG_DATA_HOME`, `XDG_CONFIG_HOME`,
    `XDG_CACHE_HOME` and `XDG_STATE_HOME` to the box HOME's defaults, so the bus, the `--systemd`
    manager (it inherits them) and `omabox run` all have them; `--env` still overrides. A command that
    sets another HOME in a box now keeps the XDG dirs under /home/sbx, as on the host. Second part:
    dbus-daemon watches only the service dirs that exist when it starts. A fresh box HOME had no
    `~/.local/share/dbus-1/services`, so a service installed there after `up` launched (a miss in an
    activation rescans every configured dir) but was not listed by `ListActivatableNames` / `busctl
    --user list --activatable` until its first activation or a `ReloadConfig`. `seed_home` now creates
    the dir. Checked in plain and `--systemd` boxes before and after; `t_dbus_user_app` and `t_systemd`
    install an app after `up` and launch it with `gtk-launch` (all five checks fail without the fix).
88. **A box per agent session** (2026-09-25). The default name was the repo's, so two agents in one
    checkout (two Claude Code windows, a Claude Code and a Codex) shared a box: one's `up` got the
    other's box with its options ignored, its `run` saw the repo read-only, and its `down` ended the
    other's work. Both agents already tell their shell commands who they are: Claude Code exports
    `CLAUDE_CODE_SESSION_ID` (a UUIDv4), Codex `CODEX_THREAD_ID` (a UUIDv7, checked in its rollout
    logs). The default name is now `<repo>-<last 8 of the id>`; the tail, because a UUIDv7 starts with
    a timestamp two sessions opened in the same minute share. `guard exec` sets `OMABOX_SESSION` for
    any other agent; set to empty, it turns the suffix off. `-b` and `OMABOX` are unchanged.
    Where one box used to be reused, each session now starts its own (~500 MB), so a session's
    default box goes down after 30 min idle instead of 2 h (`--idle` and `OMABOX_IDLE` still set it);
    agents are told to `omabox down` when done, and this catches the ones that forget. Taking the box
    down when its agent exits is left for a follow-up (done in finding 93, which puts the limit back
    to 2 h). What follows from the id: Claude Code's
    subagents run in its process with the same `CLAUDE_CODE_SESSION_ID`, so parallel subagents still
    share one box (each needs `-b NAME` for its own); `/clear` gives the session a new id, and
    `/resume` the resumed one's (`/compact` keeps it), so after `/clear` the agent starts a new box
    and the old one waits out its idle limit; and a box the user started with `omabox up` in the repo
    (named `myrepo`) is no longer an agent's default: the agent passes `-b myrepo` to use it, and the
    user passes `-b` to `peek` or `shot` an agent's box.
89. **A headless box captured host X11 apps (Steam)** (2026-09-26, PR #8). In the host's network
    namespace, a box's labwc (lazy Xwayland) bound the abstract `@/tmp/.X11-unix/X0` about 2 s into
    `up`, for the box's whole life (the next free X<n> when :0 was taken). The host's Xwayland
    (Hyprland) has only the filesystem sockets, and libxcb tries the abstract one first, so a host
    X11 app opened in the box: Steam logged the box's 1600x900 screen, the real Hyprland listed no
    Steam window, and stopping the box gave host apps the real 4480x1440 display again. (1600x900
    needs an NVIDIA box: only there is labwc's output sized to `--size`, finding 78; on AMD/Intel it
    stays 1280x720.) An X server run in such a box (Xvfb, xvfb-run) did the same, box apps could
    reach any host abstract socket, and host apps could squat the box's. Every top-level box now
    runs behind pasta in a network namespace of its own, which closes that whole class;
    `--net isolated` keeps its arguments. It replaces 44's isolated-only `nsenter -n` in `run`
    (`run` joins a box's network namespace whenever it differs from its own), and 52's stub resolver
    is now reached through pasta's `-T`, which mirrors the host's port 53 into the box (the splice
    keeps the address, 127.0.0.53). `--net connected` (the default; `host` is its old name) runs
    `pasta --config-net --no-map-gw -D none -S none --host-lo-to-ns-lo -t 127.0.0.1/1-65535,auto
    -u 127.0.0.1/1-65535,auto -T 1-65535,auto -U 1-65535,auto`.
    - *Flags*: a box's ports are bound on the host's 127.0.0.1 only; plain `-t auto` bound `*:P`,
      which opened them to the LAN, and a host `localhost` client's ::1 attempt was accepted, then
      reset. `1-65535,auto` includes the ephemeral range (32768-60999), which `auto` alone skips.
      `--no-map-gw` makes the box's gateway the router again, not all of the host's loopback.
      `-D none`: with `--no-map-gw`, pasta cannot hand the box the host's loopback nameserver, and
      said "Couldn't get any nameserver address" on every start. `-S none`: no search domains either
      (the box reads the host's resolv.conf).
    - *Across the boundary* (curl and python in real boxes): use 127.0.0.1, with a server listening
      on IPv4 (127.0.0.1, 0.0.0.0 or ::). From the host, `localhost` works too (::1 is refused, then
      127.0.0.1). From a box, `localhost` to an IPv4-only host server is accepted and then reset
      (`-T` listens on both families and takes no address), and from box to box it resets even for a
      dual-stack server (the hop is the host's 127.0.0.1). A box server on ::1 only cannot be
      reached from outside (reset); a host server on ::1 only is reached from a box as [::1] or
      `localhost`, not 127.0.0.1. Inside a box the host's LAN address is the box itself. No
      multicast (mDNS, SSDP). Forwards appear 0.2-1.0 s after a server starts listening (33 timed
      runs; one untimed try took over 5 s and did not recur). pasta's UDP auto mode also forwards
      the ports bound for TCP: while a box has a TCP server on P, a host program cannot bind
      127.0.0.1:P, TCP or UDP (it can still bind [::1]:P, which host `localhost` clients try first).
    - *Cost*: pasta takes about 28 MB and 0.4% of a core per box (its 1 s rescan), and every host
      listener is mirrored into every box.
    - *Requirements*: a connected box's tap device needs /dev/net/tun; `up` checks for it and for
      pasta up front, before the built helpers (`check_install`, which a fresh checkout's first unit
      run otherwise hit first). `--net isolated` needs no tun. A nested connected box (`net: none`)
      gets bwrap's `--unshare-net` and no route out, because a box's /dev has no /dev/net/tun. A
      nested `--net isolated` box still runs pasta `--splice-only`, but only a headless one, from
      `omabox run`: everything a box's session starts has no_new_privs (bwrap sets it; `run` enters
      from the host and has none). pasta runs under it too, but its command, uid 0 in pasta's
      namespace, then keeps only the capabilities pasta held (CapPrm 0x201400: net_bind_service,
      net_admin, sys_admin), not all of root's, and without CAP_SETFCAP the bwrap it starts cannot
      map its uid onto pasta's root ("setting up uid map: Operation not permitted"). Checked in a
      box: from a script the box's Hyprland started, `up --net isolated` and pasta + bwrap failed
      so, while pasta alone and bwrap alone ran; through `omabox run` all four worked, and pasta +
      bwrap failed again with CAP_SETFCAP alone dropped before bwrap. The bounding set is full in
      both kinds of process (finding 45 blamed it). The same goes for any no_new_privs process, a
      top-level one too (an agent's sandbox, a systemd unit with `NoNewPrivileges=`): under
      `setpriv --no-new-privs`, `up` failed 10 s in with only "bwrap did not start" (box.log had the
      uid-map line), where a box in the host's network, before this change, came up. So `up` reads
      NoNewPrivs from /proc/self/status up front (an interactive box is started by the host's
      Hyprland, so it is not affected). A connected headless box falls back to `none`, as a nested
      one does, and `up` says it has no network and why: plain bwrap with `--unshare-net` works
      under no_new_privs, and on main a sandboxed agent could start a box there (Tyler's point on
      PR #8; such sandboxes mostly block the network anyway). It still gets a namespace of its own,
      so no host X11 capture (`t_no_new_privs`, which also checks `run` from such a process). An
      isolated box is refused with the reason, since its `--allow` ports need pasta
      (`t_unit_refusals`). A box that is up already still reads "already up" there, without the
      per-name lock, so such a process can use a box the user started (`t_connected`).
    - *Pids*: pasta's pid namespace makes bwrap's child-pid 2. `pasta_pid` finds the host pid only
      through the box's own pasta (`box_pasta`, which `kill_box` uses too): a process whose comm is
      pasta's (passt.avx2 on this CPU; readable although pasta is non-dumpable) and whose command
      line names the box's pasta.pid. pasta never removes that file: a stale one naming another
      box's pasta led `down` to that box's PID 1 (reproduced with a fabricated box dir), and, with
      only the command line checked, a process whose argv held the path was killed with its child.
      bwrap gets `--die-with-parent` (its parent is pasta), so a box whose pasta dies (OOM, a
      killall) ends with it instead of staying up with no network; isolated boxes too.
    - *Older boxes and CLIs*: boxes started before this change keep the host's namespace until
      `down` (`ls` shows them as `host`); entering the host's namespace from the box's user
      namespace is EPERM ("reassociate to namespaces failed"; checked against a box started by
      0.1.2). An older CLI reads a connected box's child-pid (2), so it lists the box dead, and its
      `down` deletes the dir but, where pasta runs as passt.avx2, leaves the box and its pasta
      running (its fallback checks `comm = pasta`): take boxes down before downgrading.
    - *Tests*: `t_connected` diffs the host's abstract X11 sockets around `up` (a new one counts
      unless `ss -xlp` places its owner in another pid namespace: another checkout's box may start
      meanwhile), and checks the netns, DNS, `localhost` from host to box (bound on 127.0.0.1 only),
      127.0.0.1 from box to host, a kernel-chosen port each way, that the box's gateway is not the
      host's loopback (one connection to the router), and that box.log is empty. Reverting
      `1-65535`, the 127.0.0.1 scoping, `--host-lo-to-ns-lo`, `--no-map-gw` or `-D none` fails one
      of them. `t_pasta_dies` covers `--die-with-parent`; `t_stale_pid` a stale pasta.pid naming a
      copy of sleep called pasta, or a process whose argv names the file; `t_race` counts one pasta
      per box. `t_throwaway_dead` killed info.json's child-pid, 2, which on the host is kthreadd
      (EPERM), so the box stayed up; it now kills the PID 1 `box_pid` finds.
    - *Interactive*: pasta forks, so every top-level interactive box's window now reaches its
      workspace through `HL_EXEC_RULE_TOKEN` only, as isolated ones have since 45. The suite cannot
      cover it: its stand-in interactive boxes (`t_guard`) are nested connected ones (`net: none`,
      no pasta), and a nested `--interactive --net isolated` box is started by the stand-in's
      Hyprland (`hl.exec_cmd`), so it has no_new_privs and fails at bwrap's uid map (Requirements,
      above). Still to check on the real desktop, with the user's go-ahead: in the default mode,
      `up --interactive` lands on workspace 9 with the active workspace and window unchanged,
      `peek --focus` focuses it, and `down` leaves no pasta.
90. **A hidden interactive box is drawn, so no agent switches the user's workspace for a shot**
    (2026-09-26, seen by a user: a Codex agent testing an app in an interactive box on
    workspace 9 kept flipping their second monitor to 9 and taking focus). Finding 24's `shot`
    failure said the host only renders the window "while its window is visible (workspace 9)", and
    the agent took that as the fix: `omabox host -- hyprctl eval 'hl.dispatch(hl.dsp.focus({
    workspace = "9" }))'` before every click and shot, then focus back to workspace 1, dozens of
    times. The exec rule now adds `render_unfocused = true`: the host sends the hidden window frame
    callbacks (at `misc.render_unfocused_fps`, 15 by default), the box's Hyprland keeps drawing, and
    `shot`, `click` and `keys` all work with the window out of sight. Hyprland keeps the exec
    rule's `render_unfocused` with the window through every rule re-check (focus, a move to another
    workspace, a reload). The renderer only starts drawing a hidden window when rules are
    (re)applied (`window.updateRules`: at map and on each re-check), so `set_prop` on an existing
    window reports true (getprop says so) but does nothing until the next re-check, and a re-check
    never overwrites it. A box started before this has an exec rule without `render_unfocused`, so
    no re-check makes it draw: it needs a restart, or `set_prop` followed by a re-check (a dispatch
    on the user's session). Hyprland 0.56.2's source says so, and es2gears in a box agreed: 0 fps
    after `set_prop` alone, 15 after a tag toggle; an old-style rule stayed at 0 through a tag
    toggle, a workspace move and a reload. A restart ends the user's session in the box, so
    `shot`'s message for such a box says to ask the user rather than to restart it. `up` records
    `drawn_hidden` in box.json to tell those apart; no message suggests showing the window, each
    says not to. The skill says the same under `omabox host`, and that a box the user started has
    its own name (the repo's, finding 88, or box-N from the bar widget): an agent passes `-b NAME`
    to reach it. The cost: a hidden interactive box running something animated now keeps drawing at
    15 fps where it used to stop, and interactive boxes never idle out. The exception is a box kept
    running after a close with confirm-close on (finding 70): the exec rule went with the first
    window, so the new one is not drawn while hidden (Open). `click` and `keys` still reach it, as
    input never waits for a frame, but `shot` gets no frame, so an agent cannot see what they did.
    In a stand-in (`--no-shell`, a terminal in the box), with the new window hidden and `shot`
    failing, `keys super+1` switched the box to the terminal's workspace, and a click and typed
    keys then reached the terminal: it got the mouse report and ran the command. Right after the
    keep, though, the box's new output showed a new, empty workspace (3; the terminal stayed on 1
    and nothing had focus), hidden or shown, so a blind click or keys reached no app until the
    box's workspace was switched (fixed since: finding 114). confirm-close.sh leaves
    `omabox.reopened` in the box's runtime dir, and `shot`'s message then says the box needs a
    restart and to ask the user; for any other interactive box with no frame it says to ask the
    user too.
    Checked in a stand-in host (finding 26): an old-style box timed out after 10 s; a new one gave
    a frame at once, drew a terminal opened while hidden, took a click on its bar and typed text,
    with the stand-in's workspace and focus unchanged throughout. `t_guard` shoots hidden boxes on
    workspaces 9 and 3 and the scratchpad, and the one on 9 again once a window has opened in it,
    which that second shot must show (the first shot right after `up` gets a frame even from a box
    that is not drawn while hidden); the stand-in's workspace and focused window stay unchanged.
    `t_unit_shot_hidden` checks the messages, for a box with and without `drawn_hidden` and with
    `omabox.reopened`, and `t_guard` that a close with confirm-close on leaves that file. Not yet
    checked on the real desktop.
91. **`up` ended without a word when git had no identity** (2026-09-26). `seed_home` copies the
    user's git `user.name` and `user.email` (finding 48) in a loop whose last command was
    `val=$(git config --global user.email) && git config --file ...`. With no global `user.email`
    (a fresh machine) that lookup fails, `seed_home` returns 1, and `set -e` ends `up` right after
    the box dir is made: exit 1, no message. Found running the suite from a git worktree: in the
    stand-in host the worktree's `.git` file points at a gitdir that is not mounted, so every git
    command there fails with 128, `--global` lookups included, and `t_guard`'s inner `up` died.
    A value that is not set is now skipped, and the lookups run from `/`. `t_no_git_identity`
    starts a box with `GIT_CONFIG_GLOBAL=/dev/null` (exit 1 before the fix).
92. **The guard let agents open links on the desktop** (2026-09-26). The guard takes the display away,
    but a browser already running there takes a URL over its own socket. Checked in a box standing
    in for the host (Chromium running in it, then from a shell with the guard's variables):
    `xdg-open https://example.com` printed "Opening in existing browser session", exit 0, and the
    running Chromium opened the tab, focused. xdg-open sees a non-empty `WAYLAND_DISPLAY`
    (`omabox-guard`), so it uses the `x-scheme-handler` desktop entry (`chromium %U`), and Chromium's
    process singleton hands the URL over before it needs a display. On a host with
    `misc:focus_on_activate` that can also take focus. `gh ... --web` goes the same way through
    `$GH_BROWSER`, `$BROWSER` or xdg-open. `share/guard/xdg-open` refuses
    with a note (give the user the link; look at it in a box; `omabox host` when asked) and exit 4,
    xdg-open's code for a failed action. `BROWSER` and
    `GH_BROWSER` name it for every guarded agent; Claude Code's hook and `guard exec` also put
    `share/guard` first on PATH. Codex's `shell_environment_policy.set` only sets values (no PATH
    prefix), so a plain `xdg-open` from Codex is not covered (real xdg-open uses the desktop's URL
    handler before `$BROWSER`). `omabox host` drops the PATH entry, and a `BROWSER`/`GH_BROWSER` that
    is the stand-in gives way to the user manager's value or none (none too when `systemctl --user
    show-environment` prints the value quoted, as `$'...'`, for spaces or shell characters); any
    other value stays (Omarchy sets `BROWSER` in the shell, never in the user manager). Both match
    any checkout's `*/share/guard` (a PATH entry also with a trailing slash), since a hook written
    from another clone or worktree names its own. The caller's PATH that `up` gives a box session
    and `run` gives a command leaves the entry out, so links open in a box as before (this repo is
    mounted in boxes the suite starts, so the entry would be visible there).
    The guard's settings now hold the checkout's path, so `guard on` refuses a path with characters
    that would need quoting in a settings file (`guard exec` exports it, which takes any path). An
    update that changes the guard makes it read "outdated", and `install.sh` then asks "Update it?"
    for the agents that have it, whatever the others' state, and "Turn it on?" only for those where
    it is off (a "no" to turning it on does not hold an update back, and a "no" to an update is not
    remembered). If the checkout a hook names is gone (a removed worktree), its PATH entry quietly
    stops working and `xdg-open` reaches the desktop again: `omabox guard` says so for that agent.
    Python's `webbrowser` stops at the stand-in: `BROWSER` names an `xdg-open`, so Python runs it
    in the background as it runs xdg-open and counts it as opened once it has started. Checked in a
    box with only fake browsers after the guard's PATH entry (Python 3.14.7): 20 of 20 opens
    returned True and reached none, and as many with Codex's variables (a fake `xdg-open` on PATH
    instead of the guard's). Only when the stand-in's path has a space (`guard exec` from
    such a checkout; `guard on` refuses one) does Python wait for it, and its exit 4 then sends
    Python on to the next browser, the desktop's default first (with a fake `xdg-settings` naming
    `chromium.desktop`, 3 of 3 reached the fake `chromium`). Not covered, and no variable reaches
    them: `gio open`, which starts the desktop's URL handler itself (in a box under the guard's
    variables and PATH, `xdg-open` refused while `gio open` started the `x-scheme-handler/https`
    entry, exit 0); npm's `open` runs the copy of xdg-open it ships; a browser started directly
    with a URL (Arch's `chromium` launcher takes flags from a file only); and
    `omarchy-launch-browser` / `omarchy-launch-webapp`, which go through `uwsm-app` and the user
    manager, as the guard's other gaps do. Per-browser wrappers were considered and left out: a fake
    `chromium` would break `chromium --headless`, which agents use on purpose, the list of names
    never ends, and Codex would not get them.
93. **A session's box goes when its agent does** (2026-09-27, the follow-up finding 88 left). `up`
    records the agent's process for a session's default box, as pid and start time in box.json, and
    the reaper takes the box down once that pid no longer has that start time (a reused pid is not
    the agent) or is a zombie (an agent that exited under a parent that never waits kept its start
    time, and its box, until that parent went; found in review). A box.json that records no agent,
    or no start time, has none alive: an empty pid read `/proc//stat`, the system-wide `/proc/stat`,
    and an empty start time matched the empty one read for it (latent: every caller checked for an
    agent first; found in review). `agent_proc` gives no agent when it cannot read the start time.
    The agent: Claude Code exports its own pid as `CLAUDE_PID` to every command; `guard exec`
    exports `OMABOX_AGENT_PID`, the pid it execs the agent as; Codex sets `CODEX_THREAD_ID` for its
    children only, so it is the nearest ancestor whose /proc/PID/environ lacks `VAR=value` (compared
    by value: a `codex` started from another session's shell carries the outer id). It reads each
    environ with `grep -qz`: through `tr | grep -q` under pipefail, grep quitting at the match left
    `tr` to die of SIGPIPE when much of the environment followed (~1.5 MB: every time, in review),
    which read as no match, and the walk stopped at `omabox` itself. The pid must be one `up`
    descends from. A first version walked /proc for every agent and, for `OMABOX_SESSION`, took the
    farthest ancestor with it, assuming `guard exec` exported it: set by hand for one command
    (`OMABOX_SESSION=x omabox up`), that recorded `omabox up` itself, and the reaper took the box
    down about a minute later. Now `OMABOX_SESSION` without `guard exec` names no agent, and such a
    box only idles out. A `guard exec` inside a guarded agent keeps its session, so it shares the
    outer agent's box, but it exported its own pid: a box the inner agent started went when that one
    exited, under the outer one (found in review). It now keeps an `OMABOX_AGENT_PID` it runs under
    along with the session, and only then (a new session, or a pid it does not run under, gets its
    own). A box in use when its agent goes (a peek window, an `omabox run` still running) is not
    taken down: the agent check comes after the activity check, so the box stays while in use and
    goes at the next check (within a minute) once nobody uses it, without waiting out its idle
    limit. A `run -d` job is not use (its `nsenter` exits at once, finding 39), so it does not keep
    the box. A box found dead keeps its logs until `down`, like any other dead box (findings 71,
    74). A session's box has the 2 h idle limit again, whether or not its agent is found (finding 88
    had cut it to 30 min). An agent in a pid namespace of its own (a sandbox) is not found; its box
    only idles out. Interactive boxes, other names given with `-b`, and every box while `OMABOX` is
    set are never tied to an agent (`-b` with the session's own name is that box, and is tied). The
    reaper polls every idle/4 capped at 60 s, so a box can outlive its agent by up to a minute. With
    `--idle 0` a session's box still goes with its agent (`ls` says never): the reaper runs for the
    agent alone, every 60 s (it polled every 5 s, the floor for short limits, until review). Codex
    is untested with a real Codex: only fake processes that set `CODEX_THREAD_ID` for their children
    were checked. A session resumed in a new process (`claude --continue` or `--resume`, which keep
    the id) found its box still up but recording the agent that had exited, and the reaper took it
    down under the new one at its next check (11 s after the first quit, in review). Now a command
    that reaches a session's box that is up (`up`, `run`, `path` and every one through `need_box`)
    records its own agent there when the one recorded is gone; only then, not in a box that records
    none or whose agent still runs. The write holds the box's lock, and the reaper's `down`, which
    decided on the old agent, checks it again under that lock, so a takeover made while it waited
    keeps the box. The takeover too checks again under that lock that the box records an agent and
    that it is gone: an `up` of the name that found no agent may have come in between that `down`
    and it, and the box that `up` started is not taken over (found in review). An agent whose first
    command comes after that check finds the box gone, and so does one whose command waited for the
    lock while that `down` had it: `run`, `path` and every command through `need_box` say no box is
    up (`run -d` went on and failed with a bash error, its log's dir gone; found in review), and
    `up` starts a new box, as it would for one that had died a moment earlier. `mode` writes
    box.json's size under the box's lock too: its read and write around a takeover would put back
    the agent that exited (found in review; `t_mode_lock` holds the lock and checks that `mode`
    waits for it). `t_unit_agent_session` checks each way of finding the agent and the one-command
    case; `t_agent_session` runs fake Claude Code sessions (a shell exporting its own pid as
    `CLAUDE_PID`): the box knows its agent, keeps 2 h, stays while an `omabox run` is going after
    the agent is killed, goes once that ends, and a box that died stays dead; a new agent of the
    session whose first command is `run` or `up` keeps the box past two checks and takes it down
    when it exits, a new agent's first `hyprctl` (through `need_box`) or `path` records it too, and
    another session's command naming the box with `-b` does not (the reapers stopped across the
    handover; each fails without its part of the takeover). A takeover made while the reaper's
    `down` waits for the lock keeps the box, and the reaper then watches the new agent: the lock is
    held by hand while both queue, and the reaper's `flock` stopped until the new agent's command is
    done, so the order is fixed (it fails without `down`'s check under the lock, or with a reaper
    that stops watching after it); with the new command's `flock` stopped instead, that command's
    `run -d` says no box is up, and with an `up` of the name that finds no agent also let in between
    the two, the command does not take over the box it started.

94. **From another user namespace, a live box read dead and `up` orphaned it** (2026-09-28, found in
    the review of PR #8). Under `unshare -Ur`, or in a sandbox that makes its own user namespace,
    `readlink /proc/PID/ns/pid` of a box's PID 1 fails, and `box_pid` took that as a dead box:
    `ls` listed it dead, and `up` cleared its dir and started another, leaving the first one
    running where `down` could no longer find it (reproduced by `t_other_userns` on the old code:
    its pasta and bwrap stayed up, the dir gone). `box_pid` now returns 3 when the pid is a process
    of ours whose namespace it cannot read, and `box_alive` stops there ("cannot tell whether box
    ... is up from this user namespace"); a process that is not ours (its pid reused) still reads
    dead. Such a process could not enter or signal the box anyway. `t_other_userns`: `up` from
    `unshare -Ur` fails with that message, and the box is the same one, still up.
95. **An interactive box renders on the desktop's GPU, not the one omabox picked** (2026-09-29,
    Diogo's desktop with the RTX 5070 Ti on nvidia beside the AMD iGPU, monitors on both). With
    `OMABOX_RENDER_NODE=/dev/dri/renderD129` (the RTX), `up --interactive` failed 10 s in with
    "bwrap did not start"; the box's Hyprland had aborted with `CBackend::create() failed!` right
    after the host's `zwp_linux_dmabuf_v1` format table (AMD modifiers only). Aquamarine's Wayland
    backend opens the render node of the parent's dmabuf main device, here the host's AMD
    `renderD128`, which the box did not have; the "Failed to open node" line never reached the log
    (the abort). Not new: 0.1.0 picked the node the same way, and the default (the first usable
    node) matches the desktop's GPU on this machine, so it only shows when the desktop renders on
    another GPU than the first node or the variable points elsewhere. An interactive box now gets
    every usable render node, plus `/dev/nvidiactl` and `/dev/nvidiaN` for each NVIDIA one whose
    nodes are there (render nodes only: finding 5 holds), and its Hyprland opens the one the desktop
    names; `OMABOX_RENDER_NODE` now chooses a headless box's GPU only. And a box that started and
    ended before `up` saw it now says so ("died while starting", as `wait_ready` did already, with
    Hyprland's last log line when there is one) instead of "bwrap did not start", which is kept for
    a bwrap that wrote nothing to `--info-fd`. `t_guard`: the interactive box nested in the stand-in
    sees the stand-in's GPU nodes (on AMD, and on the RTX with the stand-in on it). The stand-in has
    one GPU, so the suite cannot reproduce the mismatch itself: verified on the real desktop.
96. **`OMABOX=NAME` was ignored under `omabox run`** (2026-09-29, issue #19, seen recording the 0.2.0
    clip). `run` marks its commands with `OMABOX=1 OMABOX_NAME=NAME`, and every check read any
    `OMABOX` there as that marker, so a script under `run` that exported `OMABOX=inner` got the
    default box. `OMABOX` is now the marker only when it is `1` and `OMABOX_NAME` is set
    (`env_names_box`), in `default_name`, `session_box` and `run`'s throwaway check. So a `run`
    command with only the marker now gets a throwaway box from `omabox run` like a host shell,
    instead of "box not up". Checked in a box with the issue's repro (`OMABOX=inner omabox path` is
    `inner`, `OMABOX=1` still the default); `t_unit_cli`.
97. **A bare `--window` word matches a reverse-DNS class's last part** (2026-09-29, issue #20).
    `nautilus` found nothing for `org.gnome.Nautilus`, and the app's name is what people and agents
    type. The word now also matches the class's last dot-separated part, whole and in any case
    (`naut` does not); none or several is still exit 2. Checked with Files in a box;
    `t_unit_window_select`.
98. **A Qt app's log reaches `run -d`'s log file** (2026-09-29, issue #26, found by an agent
    debugging omaseed). Without a terminal Qt sends its logging to the journal, and a box has none, so
    `qWarning`, QML errors and an abort's reason went nowhere: `run-*.log` stayed empty (a foreground
    `run` printed nothing either). Boxes now set `QT_FORCE_STDERR_LOGGING=1` for the session, so
    everything in them, `run` included, logs to stderr, as the guard already does on the host (finding
    67); `--env QT_FORCE_STDERR_LOGGING=0` turns it off. Checked in a box with a QML `console.warn`:
    missing before, in the log after, for `run -d` and a foreground `run`; `t_main`.
99. **An agent inside ai-jail drives boxes of its own, through a broker** (2026-09-29, issue #16;
    ai-jail 2.2.1). ai-jail's seccomp filter refuses `setns` and `unshare` (EPERM), and the jail has
    its own pid, user, ipc and uts namespaces, a tmpfs `/run` and `/tmp` and, by default, a tmpfs
    HOME and no network: omabox inside could neither enter a box nor start one (`ls` read boxes as
    dead). No change to ai-jail: its `ro_maps` (or `--map`) already pass a socket's folder in, and a
    Unix socket connect through a read-only bind works. `omabox broker on` writes a systemd user
    socket (`$XDG_RUNTIME_DIR/omabox/.broker/sock`, 0600) and service (`tools/relay/omabox-relay
    listen -- bin/omabox`, KillMode=process so boxes outlive it) and prints the `~/.ai-jail` lines
    (omabox and the relay at `~/.local/bin`, `skill/` so the agent's skill link resolves, the socket's
    folder; all read-only); it never edits `~/.ai-jail`. In a jail, `omabox` sees the socket and that
    `unshare -U` fails, and hands the command to `omabox-relay call`: cwd, arguments, the default box
    name it computed there (repo and session as the jail sees them), its PATH and `--pass` values as
    `--env` entries, and fds 0-2 (plus a shot's file) by SCM_RIGHTS. The relay's `listen` checks the
    peer's uid, takes an SO_PEERPIDFD, and runs `bin/omabox` in a session of its own with those fds,
    a fixed PATH and the caller's entries renamed `OMABOX_RELAY_*` (a `BASH_ENV` or `LD_PRELOAD` from
    the jail never reaches the broker under its name) and SHLVL=1 (bash sources ~/.bashrc when its
    stdin is a socket and SHLVL is below 2, as under rsh/ssh: seen, mise's PATH in the broker, when
    the caller's stdin was one); the caller going kills the command's session.
    The broker (`broker_init`) walks up from the peer to the bwrap whose parent is ai-jail: that
    bwrap's command line is the jail's whole policy (every mount, `--unshare-net`), which nothing in
    the jail can change (other pid namespace, no ptrace); the pidfd is checked before and after, so a
    reused pid is not read. `jail_policy` parses it with a table of bwrap's options (an unknown one,
    `--args` included, fails the read) into `net` and `roots` (folders bound at their own path; masked
    when a later mount lands inside, dropped when one lands at or above). Driving a box is running
    code in it (keys into a terminal), so a jailed agent's boxes get no more than its jail: `--net
    isolated` without `--allow` when the jail has no network; the repo mounted only when it is a jail
    folder, whole and unmasked, found without git (its `.git/config` is the jail's to write, and
    `core.fsmonitor` and friends would run on the host); `--plugin`, `--ro-bind`, `--overlay` only
    for such folders (one below could be swapped for a link between the check and the mount); no
    `~/.config/omabox/ro-bind`, mise's toolchains only if the jail sees them; no interactive box,
    `host`, `peek`, `guard`, `broker` or `config` changes. Boxes carry `jail` ("PID START" of that
    bwrap) in box.json: a jailed caller sees and drives only its jail's (`select_box`, `list_names`),
    and its box's `agent` is the bwrap, so the box goes when the jail does (seen: within a reaper
    poll). Files never go by path: `shot` is made in the jail and written by the broker through the
    fd the jail passed (`--to-fd`, never reopened by name, so a read-only fd stays read-only: seen,
    grim's write failed and the file kept its content), `--in` shots are passed open (`IN_AS` names
    them for the shot record); the broker refuses `-o PATH` and `--in PATH` from a jail. Every
    `omabox` the broker starts (run's throwaway `up`, a reaper) inherits `OMABOX_JAIL`, so the limits
    hold there too; a box starts with `env -i`, so none of it reaches a box. Checked in a real jail:
    up, run in the project, shot into the jail's /tmp and project, windows, keys into foot (seen),
    click --in, run --pass, down; refused: -o/--in paths, --overlay/--ro-bind/--plugin outside,
    --interactive, --allow, --net connected, host, guard, broker, peek, _reap, config changes, the
    user's box, env injection (BASH_ENV did not run). `t_unit_jail_policy`, `t_unit_relay`,
    `t_unit_broker_units` (systemctl stubbed), `t_jail` (a real ai-jail, skipped without one). Then
    on the real session: `omabox broker on`, its line in `~/.ai-jail`, and a real Claude Code in
    `ai-jail --network --agent-state claude -p` (plus `--map ~/.config/mise --map
    ~/.local/share/mise`, which ai-jail's README asks for when mise installed the agent) loaded the
    omabox skill by itself and ran up, run -d --wait foot, keys --window foot, shot --window foot
    and down, each exit 0, reading the shot right; its box took the session's name and was
    connected (the jail had --network). Not checked: `--lockdown` (drops maps, so no broker:
    expected). Open: GPU (a box has a render node the jail may not), the
    seeded box HOME (the user's non-secret Omarchy look, which a private-home jail does not see),
    and how many boxes a jail may start.
100. **Saves: `omabox save`, `up --from`, `run --from`** (2026-09-29, from omavm's idea; the stress
    test's solari vault PIN and omaplex sign-in were never reached, the setup each needed was redone
    or skipped in every box). `save SAVE [-b NAME]` copies a running box's HOME to
    `~/.local/share/omabox/saves/SAVE/home` (with `save.json`: box, date, version), not under
    `~/.cache/omabox`, which `up` sweeps (finding 50). Left out: `.cache` and the logs at the HOME's
    top. Kept: the keyring (being signed in is most of what a save is for; a box's keyring only holds
    what was stored in it there), and `saves` says which saves have secrets. The saves dir is 0700.
    The box is paused for the copy (SIGSTOP to every process in its pid namespace, `box_pids`, which
    `gpu` uses too; SIGCONT after, and on any exit): an app's database is copied as a power cut
    would leave it, whole to a crash-safe app, never torn by a write during the copy. Seen: a save of
    30 000 files caught the box's Hyprland in state T, S again after. A killed `save` can leave the
    box stopped (a headless one still idles out). `up --from SAVE` copies the save into the new
    HOME (`cp --reflink=auto`: free on btrfs), then `seed_home` as for any box, without `/etc/skel`
    (the save's app configs stay) and with the theme dir replaced, not copied into: the Omarchy look,
    the bar (`shell.json` with this box's plugins), terminals and git identity are today's, the
    apps' data the save's. `box.json` records `from`. `--force` replaces a save (renamed aside, then
    removed), one save per name at a time (a lock in the runtime dir). No saving a box that is down:
    `down` is SIGKILL, nothing flushed. Verified in boxes: data, a `secret-tool` secret and a
    changed btop config come back, `.cache` and logs do not, `shell.json` is regenerated, `run
    --from` in a throwaway, a `--systemd` box's user unit starts from its save. `t_saves`,
    `t_unit_saves` (saves in the suite's own data dir, never the user's). Not for an agent inside
    ai-jail (finding 99): `save` and `saves` are not on the broker's list, and `up --from` is refused
    there, since a save holds the user's keyring secrets and apps signed in as them, which the jail
    never had (`t_unit_jail_policy`).
101. **Pointer: `drag`, `pointer --window`, a default button** (2026-09-29, issue #25, from agents
    driving omaseed). `omabox drag X1 Y1 X2 Y2` presses, moves in `--steps` (10), holds `--hold`,
    releases; `--window`/`--in` map both points as for `click`. `--shot FILE` shoots with the button
    still down: the pointer tool's new `pause` step prints "paused" and waits for a line on stdin, and
    `drag` runs it as a coproc and takes the shot there (a shot that fails still releases). Not with
    `--wait`, and not from a jail (the shot's file is a path of the broker's there). `pointer --window
    SEL` maps each `move` from the window's coordinates, raw like `--in` (on screen and uncovered, or
    refused). `down`/`up` default to left. The tool's `--hold` (finding 41, an idle device that never
    returns: two agents called it and hung) is out of its usage line and refused by `omabox pointer`.
    Seen: a button pressed by `down` stays down after the tool exits, until an `up` from a later call
    (the selection in foot completed only then), so a `down` without `up` turns the next clicks into
    drags; reference.md says so. A drag right after a window was moved went where the window was still
    being drawn (the move is animated): `wait still` first. peek parses a bare `down`/`up` and `pause`;
    `drag` marks only its two ends (its steps would pass peek's 64 tokens). Checked in a box: foot
    selects what is dragged across (primary selection), `--shot` shows the selection held;
    `t_window`.
102. **Shot: `-g` inside `--window`, `-o` makes its folder, looser `-g`** (2026-09-29, issue #27).
    grim crops a toplevel capture (`-T`) with `-g` in the window's own coordinates (compared with a
    crop of the whole window shot: identical); the shot records the crop's origin and the window's
    size, so `--in` maps from it. `-g` takes `X,Y,W,H` and `X,Y W,H` too (`geom_parse`, also for
    `wait -g`). `-o DIR/F` makes DIR. The pointer "drawn in some shots, missing in others": screen
    shots (whole or `-g`) always have the box's software cursor, window shots never do; said in the
    help and the skill rather than a `--pointer` flag. `t_window`, `t_main`, `t_unit_wait`.
103. **`run --env-file FILE`** (2026-09-29, issue #30; BOX-5 in omaseed's review of 2026-09-24).
    KEY=VAL lines (`export `, `#` lines, matching quotes) read as data, nothing expanded, handed to the
    command through `--pass`'s pipe, never a command line; `--pass` of the same name wins. A bad line
    fails with its number, not its text. From a jail the caller's omabox reads the file and sends its
    entries as `--pass` values. Checked in a box (and that no value is in any command line while it
    runs); `t_main`.
104. **`-b NAME` before the command** (2026-09-30, issue #36). An agent's helper `ob() { omabox -b
    "$BOX" "$@"; }` got "unknown command: -b" and the whole help (97 lines) on every call. `main`
    now moves a leading `-b NAME`/`--box NAME` to right after the command (`global_box`), where every
    command that takes `-b` parses it, before anything reads the command: the relay in a jail and the
    broker's `broker_check` see `windows -b NAME` as if typed so, and a jailed `-b` still names only
    the jail's boxes (`select_box`). For `up`, `down`, `env` and `path`, which also take a positional
    NAME, it is the same as `-b` after the command (`omabox -b a up b` is "one box name, got a and b";
    `down` takes both). Commands that are not about one box (`ls`, `saves`, `config`, `guard`,
    `host`, `broker`) say "ls takes no -b" in one line (exit 2); `help` and `--version` ignore it;
    given twice, the last wins. An unknown command is now one line pointing at `omabox help`, not
    the help (exit 2 still). `t_unit_cli`, `t_jail`.
105. **`wait window SEL` is satisfied by any of several matches** (2026-09-30, issue #37). It used
    the one-window resolver (finding 81) and failed with exit 2 when a second window matched: an
    app that opens one window per vault (Obsidian) broke `wait window class:obsidian` as soon as the
    second came up, while `--gone` already counted them. "Until a window like this exists" is
    answered by any: `satisfied: window foot after 0.20s: 2 windows: 0x... foot "A" at ...; 0x...`,
    every match named (`--json`'s `detail`). `--focused` is satisfied when the focused window is one
    of them, named with "(1 of N matching)"; none focused says "no N matching, none focused". Only a
    bad regex is still exit 2. What acts on one window keeps refusing several (`--window` on `shot`,
    `click`, `keys`, `pointer`, `drag`, and `wait still/change --window`, which watches one window's
    place). `win_answer` is the probe without the box; `t_unit_window_select`, `t_wait`.
106. **`omabox lua`: Lua in the box's Hyprland, and its value** (2026-09-30, issue #40, from an agent
    that wrote files from Lua to read them back). `hyprctl eval` answers `ok` or `error: MESSAGE`,
    with the message whole (100 000 bytes came through; a NUL ends it: a C string) and no overlay or
    log line in the box. So `share/lua.lua`, sent as the body of a function with the source in a Lua
    long string whose brackets the source does not contain (`[==[`: nothing is quoting), compiles
    `return SRC` or else `SRC` (an expression or statements, as eval itself does), runs it under
    `pcall` and raises its answer as an error on purpose: `omabox-lua-ok:` and the values as a JSON
    array, or `omabox-lua-error:` and the Lua error. No file in the box, so calls at once never meet
    (six in parallel checked). JSON escapes control characters, NUL included; floats print as Lua
    does (`960.0`), inf and nan as strings. Hyprland's objects are userdata with an `__index`
    function, whose fields Lua cannot list: they come from Hyprland's own stubs
    (`/usr/share/hypr/stubs/hl.meta.lua`, `---@class`/`---@field`, read once per Hyprland into a
    global), and an object inside one prints by name (`HL.Workspace(1:1)`), as `hyprctl -j clients`
    names them; without the stubs an object is its tostring. Seen in a box (Hyprland 0.56.2, Lua
    5.5): an error in an `hl.on` callback that a `hyprctl dispatch` sets off comes back in that
    dispatch's answer; one in a timer, or in a callback an app's event sets off later, is logged
    nowhere (not the Hyprland log, not `configerrors`, nothing on screen). reference.md says to
    `pcall` inside callbacks. Allowed to a jailed agent, as `hyprctl` is. `t_unit_inspect`,
    `t_inspect`.
    Later the same day: with no EXPR at all it read the source from stdin, undocumented (the usage
    says `EXPR | -`), and a suite check that ran `omabox -b NAME lua` with an open stdin hung on it.
    Now no EXPR is refused ("nothing to evaluate") and only `-` reads stdin; `t_unit_inspect`.
107. **`omabox log`: a box's logs by name** (2026-09-30, issue #41, from an agent that grepped the
    Hyprland log through `run -- bash -c` twice in one session). `log [LOG...|all]` with `hyprland`
    (the default: `run/hypr/SIG/hyprland.log`), `shell`, `apps` (the uwsm-app stand-in's), `run`
    (the latest `run -d` log), `keyring`, `labwc`, `systemd`, and the box dir's `box` (bwrap's)
    and `reap`; `-n N` (100, or `all`), `--grep RE` (grep -E, `-i`), `-f`. `path --logs` lists the
    files. The box HOME and runtime dir are the box's to write, so a log swapped for a link to a host
    file would have `log` print that file into an agent's context: a live box's logs are read inside
    its mount namespace (`on_box`, where the path resolves as the box sees it), a dead box's from the
    host only as regular files that resolve inside its own dirs (checked both ways with a link to a
    file in the suite's host /tmp). A dead box's logs are read too, since they say why it died
    (`need_box` now points at `omabox log -b NAME all`). `-f` runs `tail -F --pid=<box PID 1>`, in
    the box's mount namespace but the host's pid namespace, so it ends within a second of the box
    going down (seen: 1.1 s after its Hyprland was killed), says so and exits 0; the box dir's logs
    are followed by a second tail into the same stream, tail's headers renamed to the logs' names
    and, with `--grep`, printed only before a match. A follower is not use for idle expiry (its
    command line is tail's, not nsenter's). Seen: Hyprland writes its log in pieces (the file often
    ends mid-line, and a burst of lines landed only as Hyprland exited), so a line about an action
    can come late; `log` ends every line it prints. `t_unit_inspect`, `t_inspect`.
108. **`omabox events`: the box's Hyprland events, recorded from its start** (2026-09-30, issue #39,
    from an agent that hand-rolled `socat` on `.socket2.sock` and twice truncated the log under it
    (`: > ev.log`): socat's fd without O_APPEND went on writing at its old offset, the file came back
    NUL-padded, grep skipped it as binary, and "no events" was reported when there were some; this
    finding's own test did it once as it was written: two followers on one file).
    `tools/events` (`omabox-events FILE`, bound at `/opt/omabox/bin`, started by the box's
    Hyprland at `hyprland.start`, before the shell) connects to the socket from its directory (the
    path can pass 108 bytes), and writes each event as `SECONDS.MILLIS EVENT>>DATA`, one `write()` to
    a file opened `O_APPEND`, to the box HOME's `events.log` (on disk, like the HOME: finding 50;
    among the logs a save leaves out). Its own lines are `omabox>>listening` and `omabox>>stopped:
    ...`; it stops writing at 256 MB (an app retitling its window every frame) and refuses outside a
    box (it would record the real session). A blocked read the rest of the time: no cost to speak
    of; it dies with the box. Nothing is ever truncated, so "from here" is a byte offset:
    `events --mark [NAME]` prints the log's size (read in the box) and keeps NAME in
    `<box dir>/events.marks`, which the box cannot see; `--since` takes a name, an offset or a time
    ago (`30s`), and reads whole lines from there (a mark taken mid-burst is checked to fall at a line
    end: eight during forty workspace switches). `--grep` and `--until` match `EVENT>>DATA`
    (ERE); text shows local `HH:MM:SS.mmm`, `--json` `{time, event, data}`. `--until RE` waits like
    `wait` (0 with the event, 124 at `--timeout`, 1 when the box goes down; use for idle expiry),
    from the mark when one is given, so an event that came between the action and the call counts.
    `-f` and `--until` read `tail -F --pid=<box PID 1>` in the box's mount namespace (as `log -f`
    does), in the background, and end it on any exit of omabox (a trap; the reader is waited for
    with `wait`, which a signal interrupts where a foreground pipeline would not): seen, a TERM
    left no tail behind, where the first version (`pkill -P` failing under `set -e` in the EXIT trap
    before its `kill`) left one per call until the box went down. `log events` shows the file as it
    is. A box started before this has no listener: `events` says to start it again. The leak
    detector's watcher (finding 80) keeps its own `run/events.log` in its stand-in box: it asks who
    has focus on every change, which this does not. `t_unit_inspect`, `t_inspect`.
109. **`run -d -q` and `--print-log`** (2026-09-30, issue #42, from an agent session that started a
    dozen windows and filtered `grep -v '^omabox: started'` in almost every command; omaseed's
    `scripts/dev/app-box.sh` parsed `(log: PATH)` out of stderr). `-q` drops `run -d`'s own lines
    (started, `--replace`'s); errors and `--wait`'s answer stay. `--print-log` prints the log's path
    on stdout, the first line (before `--wait`'s), so a script takes it without parsing a message;
    the stderr line is worded as before. The long `--quiet` stays `--wait`'s quiet period (a
    duration): `run --quiet` without one says to use `-q`. No `OMABOX_QUIET`: a variable set once in
    a profile would also hide the log path from an agent that needs it, and a script can pass `-q`.
    No `--log FILE` either: from ai-jail the broker writes no path of the caller's (finding 99), and
    `--print-log` covers the script. All three go with `-d` only (refused otherwise). `t_unit_cli`,
    `t_replace`, `t_jail` (through the broker).
110. **`run -d --replace`: jobs are recorded** (2026-09-30, issue #29; agents restarting omaseed
    after a rebuild did kill, `run -d`, wait ~10 times a session, and once the old window was still
    starting, so the single-instance app only raised it and the agent looked at the old build).
    `run -d` no longer launches with `setsid -f` (it forks and never says the pid): a shell in the
    box starts `setsid -- CMD` in the background (`trap - INT QUIT` first: a background job of a
    shell without job control ignores them, and the job would too), prints its pid on fd 3 and
    exits, so nsenter still exits at once (finding 39; checked: no host `nsenter` left, the job's
    parent is the box's PID 1). `setsid` runs in a process that leads no group, so it does not fork:
    the pid printed is the job's, and a missing command still logs `setsid: failed to execute`.
    The record is `$D/jobs/PID` (its pid in the box, its leader's start time, the log, argv), in the
    box dir, which only the host writes (a record the box could forge would have the host signal
    any pid). A job is its session: a launcher that forks the app and exits leaves the app in the
    session (`NSsid` in `/proc/PID/status`, read over `box_pids`), and the app is the job's. A pid is
    not reused while any process of its session is left; after that, a leader with that pid and
    another start time is not the job. `--replace` takes the records whose argv is the same, word for
    word, and finds their windows (Hyprland's pids are the box's), sends SIGTERM to every process of
    the sessions, SIGKILL to what is left after 5 s, waits until the processes and those windows are
    gone, drops the records, and only then watches the screen for `--wait` and launches. It fails,
    starting nothing, when a job survives SIGKILL or a window stays. Nothing else is signalled: an
    app started another way, or one that `setsid`s itself out of the session (a terminal's shell goes
    with its terminal's pty all the same), or a single-instance app's first instance that the job
    only handed its arguments to. `--replace` needs a box that is up, as any `run -d` does: with no
    box, nothing to replace. Two `--replace` of one command at the same moment can both start it
    (no lock). Checked in a box: foot replaced (one window, a new pid), replaced while still
    starting (one window), a launcher's app replaced, a `sleep` started by a foreground `run` left
    alone, a job that exited (nothing to replace, its record dropped), one ignoring SIGTERM killed
    after 5.2 s, the job's SigIgn without INT/QUIT; `t_replace`, `t_unit_cli` (`job_procs`),
    `t_jail` (through the broker).
111. **A pointer that travels: `--steps N` on `click` and `pointer`** (2026-09-30, issue #38, found by
    an agent whose box test passed while the desktop failed: a scrolling layout's centred column lost
    its place because the pointer, on its way to the bar, crossed the next column and Omarchy's
    `input:follow_mouse = 1` focused it). `click` and `pointer move` put the pointer at the target in
    one motion event, so a test never saw what a hand on a mouse passes over. The stepping is the
    CLI's (`steps_to`, which `drag` now uses too): N moves in a straight line from where the pointer is
    (`hyprctl cursorpos`), the last on the target; the tool round-trips and waits 40 ms after each
    (finding 15), so Hyprland has run its focus-follows-mouse on it and the client has had the motion
    and a frame before the next. `pointer --steps N -- move A move B` goes through A (a waypoint: a way
    around something), `move X Y --steps N` sets one move's. `click --steps` travels in a pointer run
    of its own, then clicks: `--wait` watches the click, not the cursor crossing the screen (the still
    tool ignores 8 rectangles at most), and `--mod` holds its keys for the click alone. The default
    stays a jump: a click that now focused every window between would change what existing tests and
    agents rely on. Checked in a box (`t_pointer`): two floating foots, the pointer resting on the
    left one; a jump past the right one to the empty desktop leaves the focus on the left and the
    right one gets no motion (foot's mouse mode 1003 reports every motion); `--steps 20` to the same
    point focuses the right one and it reports motions; a path around it (a waypoint below) does not;
    `click --steps` does as `pointer --steps`. Also from inside ai-jail through the broker (`t_jail`).
112. **Modifier clicks: `--mod MODS` on `click`, `drag` and `pointer`** (2026-09-30, issue #25's
    last item). The keyboard tool holds the modifiers while the pointer tool clicks: `-m MODS` presses
    them and keeps them down to the end of the run (under any keys after it), `-p MS` prints "paused"
    and waits for a line or the end of stdin, MS at most. `with_mods` starts it on a fifo, runs the
    pointer once "paused" came, then closes the fifo, the tool's "let go". The fifo is held by that
    omabox alone, so however omabox ends (an error, Ctrl-C, SIGKILL) the tool reads its end and
    releases; it also releases on SIGTERM, SIGINT and SIGHUP, ignores SIGPIPE (a closed stdout must not
    end it with keys down), and after the pause's 3 min cap. `-p` and `-T` both read stdin: refused
    together; `keys` refuses `-m`/`-p`. Seen: a tool killed outright (SIGKILL, which nothing catches)
    leaves the modifier down for the seat; the next event of any new keyboard sets the modifiers to
    none again (a keyboard's first event, finding 14), so after a hold that did not end well
    `with_mods` runs an empty keyboard (`-s 0`), and until then a click still carries it. Modifiers
    pressed while another window has focus reach the one the pointer then moves onto and clicks
    (ctrl down with a foot focused; the jump onto the GTK window focused it; its click had ctrl).
    SUPER with the left button is Omarchy's move window and with the right its resize (`hyprctl
    binds`: `mouse:272`/`mouse:273`, "Move window"/"Resize window"), with virtual devices too: the app
    gets no click, and `drag --mod super` moved a tiled window into the other's place. A click
    without motion moves nothing. Checked in a box: a GTK 4 window logging `GestureClick` state saw
    ctrl, shift+alt, ctrl+shift on the right button, and nothing held on the next plain click and key;
    in `t_pointer`, foot's SGR mouse reports give 0, 16 (ctrl), 24 (alt+ctrl), 16 for `pointer` and
    `drag --mod ctrl`, nothing for `--mod super`, then 0; a pause with nothing on stdin lets go after its
    time; the tool SIGKILLed mid-run: that click had ctrl, omabox exits 1 saying so, the next click has
    none. Not seen: an Xwayland app's view of the modifiers (they are the seat's, as for keys).
113. **The pointer's position is test state** (2026-09-30, issue #43). Under focus-follows-mouse the
    window the pointer rests on, or last passed over, takes focus, and gets it back when a menu or
    panel closes. A box's pointer starts at the screen's centre (finding 85) and stays where the last
    command left it; a test whose result depends on focus is only as good as where it put the pointer.
    The skill's "Driving an app" says so: move it deliberately first, travel (finding 111) when the
    way matters, `omabox hyprctl cursorpos` for where it is. `omabox help` says `click`/`move` jump.
114. **After a confirm-close keep the box came back on another workspace** (2026-09-30, issue #24,
    seen by the maintainer: WAYLAND-3 became WAYLAND-4 and showed workspace 3, empty, the windows
    still on 1; finding 90 had seen the same). Reproduced in a stand-in host (finding 26): windows on
    1 and 2 of an interactive box, a close, and the new window showed 4. The box's Hyprland log of
    events (a debug handler) says why: when the last output goes, no workspace is active any more
    (not even on the removed monitor's object), Hyprland adds its FALLBACK monitor and gives it the
    first free workspace (3), the new output gets the next free one (4), and only then are 1 and 2
    moved over from FALLBACK and FALLBACK removed. hyprland.lua now follows the workspace shown
    (`workspace.active`, and the active one at each load, never FALLBACK's or a special one), writes
    it to `omabox.workspace` in the box's runtime dir when the last output goes (a file: the resize
    watch reloads the config when the output's name changes to FALLBACK and again to the new one, and
    a reload starts a fresh Lua state), and focuses it once a monitor is there and FALLBACK is gone
    (checked on `monitor.added` and `monitor.removed`: in practice FALLBACK's removal). The empty
    workspace the new output got goes by itself. Hiding the window (moving it to another host
    workspace and back) and resizing it do not recreate the output: the box's workspace stayed, before
    and after the fix. Checked in the stand-in: active 2 with windows on 1 and 2 (back on 2, its
    window focused, `SUPER+1` shows 1's window and takes typed text), active 1 after two resizes
    (reloads), a box that never switched workspace, and the second close still ends the box.
    `t_guard` checks the workspace, the focused window and the windows per workspace after a keep
    (they fail without the fix). A shown special workspace (the scratchpad) is not restored: the one
    under it is. Not yet checked on the real desktop.

115. **No workspace numbers in a box when the user's come from a plugin** (2026-09-30, issue #21, seen
    by the maintainer after finding 114's bug: the box came up on an empty workspace and nothing said
    so). The filter of finding 21 keeps only `omarchy.*` widgets and the mounted plugins, so a bar
    whose workspaces widget is a plugin's has none in a box. The issue named `njpatel.omapager` (in
    the centre), but that is a notification daemon; the maintainer's workspace switcher is
    `chaves.solari` ("named, coloured workspaces"), the centre anchor. There is no manifest kind for a
    workspaces widget, so omabox recognises one by its manifest: a bar widget whose name or
    description, or its bar widget's display name, description or aliases, say "workspace" (as
    `omarchy.workspaces`' do), read from `~/.config/omarchy/plugins/*/manifest.json` and the mounted plugins (`workspace_widgets`); a
    broken manifest counts as none. When the user's layout has no `omarchy.workspaces` and no mounted
    plugin is such a widget, `omarchy.workspaces` goes where the first one left out was, else after
    `omarchy.menu` at the start of `left` (as Omarchy's default bar has it; `left` made if missing),
    so a box always shows its workspaces. A user's own `omarchy.workspaces` stays where it is, and a
    mounted workspace widget stays instead (added where its manifest says when the user's bar lacks
    it). A layout that is not there (the shell's default has workspaces) is left alone. The jq moved
    into `shell_json_filter`. Checked in a box with this machine's bar: `omarchy.workspaces` in the
    centre where Solari was, drawn (1-5); with `--plugin chaves.solari`, Solari and no
    `omarchy.workspaces`. `t_unit_bar_filter` covers the placements and the manifest reading.
116. **`up --hyprland PATH`: a Hyprland build of the user's in a box** (2026-09-30, issue #44: an agent
    confirmed a community fix for a scrolling-layout bug by hacking a copy of `share/start-hyprland.sh`
    to exec its build, `--ro-bind`ing the build dir; it worked first time). Now a flag, on `up` and a
    throwaway `run`. `hypr_bin` resolves PATH and refuses a missing, non-executable or non-ELF file (a
    wrapper script: the box must exec the ELF itself). The binary's folder is mounted read-only at its
    own path, checked by `refuse_src`/`refuse_dest` as `--ro-bind`'s are (finding 43): a binary right
    in HOME or `/tmp` is refused, not its whole folder mounted; one under `/usr` needs no mount.
    `session.sh` → `start-hyprland.sh` execs `$OMABOX_HYPRLAND` (bwrap's `--setenv`, unset for
    Hyprland's children) instead of `/usr/bin/Hyprland`; everything else stays the box's: the patched
    aquamarine by `LD_LIBRARY_PATH`, `/usr/bin/hyprctl` in `hyprland.lua` and the tools, the shell.
    `check_install PATH` checks the build's aquamarine: its `DT_NEEDED` libaquamarine soname, read with
    `readelf -d` (never run or loaded), must be `build/prefix`'s `SONAME`, or `up` stops before the
    box dir is made, naming both (a build against the system's newer aquamarine would otherwise load
    the unpatched library, or none, and die inside the box, where only labwc.log says why). Then `ldd`
    with the box's `LD_LIBRARY_PATH`: any other library it needs and the box lacks (a newer hyprutils)
    is refused the same way. Not `ldd` for a jailed agent's file: the broker loads nothing the jail
    hands it. The stock check keeps `ldd` on `command -v Hyprland`. `box.json` has `hyprland` (the
    resolved path) and, once it answers, `hyprland_version` (`hyprctl version`'s first line from inside
    the box, `hypr_version` right after Hyprland is up, before the shell wait). `ls` prints it under
    the box's line, `ls --json` has `hyprland` (null for the installed one) and `hyprland_version`,
    `windows` starts with a `box NAME runs Hyprland PATH (...)` line. `session.sh` writes the same
    line into `box.log` itself (a background wait for `omabox.env`, then `hyprctl version`): box.log is
    bwrap's stdout without `O_APPEND`, so a line appended from the host could be overwritten. hyprctl
    and hyprpm stay the installed ones (the issue's run: IPC matched across the same version); `up`
    warns when the box's `.version` differs from `/usr/bin/Hyprland --version-json`'s, and a throwaway
    `run` now passes `up`'s `warning:`/`note:` lines through (it printed nothing of `up`'s on success).
    `restart-shell` restarts the shell only, so nothing changes there; `save` records `hyprland` in
    `save.json` and `up --from` notes when the new box runs another one. ai-jail (finding 99): the
    folder must be one of the jail's, whole (`jail_root`, as `--ro-bind`), or the binary inside the
    jail's project, which the box mounts anyway: then nothing extra is mounted and the path resolves
    in the box's view, so swapping a link in there reaches nothing the box did not have;
    `relay_call` sends the path absolute. What a box proves: compositor logic on a virtual output with
    virtual input devices (layouts, focus, input routing, the Lua config, IPC, protocols); never the
    DRM/KMS backend (modesetting, real monitors, HDR/VRR, multi-GPU), libinput with real devices, or
    the session/suspend/lock paths: those stay the real desktop's, or a VM's with passthrough.
    Verified in a box with a copy of `/usr/bin/Hyprland` in a scratch dir: the box's Hyprland process
    runs it (`readlink /proc/PID/exe`, its pid namespace the box's), shell and bar up (shot),
    `ls`/`ls --json`/`windows` name it with the version line, box.log has the line, the folder is
    read-only in the box, `restart-shell`, `save` then `run --from` (the note) and a throwaway `run
    --hyprland`. Refusals checked with a stub `libaquamarine.so.99` and a program linking it, a stub
    of the right soname plus a missing library, a script, `/usr/bin/true`, a non-executable file.
    Not checked: a real patched build (the issue's run was one), and the version warning against a
    real other version (a stubbed `on_box` in `t_unit_hyprland`). `t_unit_hyprland`, `t_hyprland`.
117. **keys-to-box: SUPER keys that follow focus, and where keys go shown** (2026-09-30, issue #22,
    from the maintainer: with focus-follows-mouse, moving the pointer across the desktop dropped
    passthrough, and a SUPER+W meant for a box hit the host and started #24). `omabox keys-to-box [-b
    NAME] [on|off]` (no argument prints on or off; interactive boxes only, a headless one is refused;
    `-b` in `BOX_CMDS`; refused to a jailed agent by `broker_check`, like every interactive thing):
    while on, the host enters the `omabox` submap whenever that box's window takes focus and leaves it
    when focus goes anywhere else, with no key to press. Off by default, for the box's lifetime: the
    state is `$D/keys-to-box`, in the box dir (the box cannot see or write it), gone at `down`. `ls`
    prints `keys-to-box: on` under the box, `ls --json` has `keys_to_box`. The widget has a keyboard
    button on interactive rows (lit when on) and `f` (h/j/k/l and x are the key catcher's), which run
    the CLI. The host side moved from an inline string to `share/passthrough.lua` (version 3; it
    replaces version 2's hooks and unbinds its toggle key, which binding again would have doubled),
    sent as a function body with the boxes dir, the key and the theme's `colors.toml`. **Which box a
    window is:** every box window has class and title `aquamarine`; its client pid is the box's outer
    bwrap (omabox-wlfd connects, then execs it), whose command line binds `<boxes>/NAME/run`, so the
    Lua reads `/proc/PID/cmdline` on a focus change to an `aquamarine` window and checks
    `<boxes>/NAME/keys-to-box`. No registry to keep: the pid stays when confirm-close recreates the
    output (the same connection), and state lives in files, so a reload loses nothing. **Rules:**
    focus on a keys-to-box box enters (`how = sticky`); focus off every box leaves (both modes);
    focus from a sticky box to another box leaves, a one-shot one stays box to box as before. In
    sticky mode finding 29's "a key with the pointer off the box ends it" does not apply: focus
    decides (so after clicking the host bar, SUPER+1 still goes to the box: the documented catch).
    SUPER+ALT+ESCAPE in the submap now remembers the focused window: sticky mode stays out until
    focus leaves that window and comes back; in the default submap it clears that and enters.
    `keys-to-box on|off` applies at once to the focused window (`omabox_keys_changed()`). **The
    indicator, both modes:** on `keybinds.submap` (its argument is the submap's name, "" for reset)
    and each check, the focused box window gets the tag `omabox-keys`, which a window rule of ours
    colours (`border_color`): a tag change re-applies rules at once, and `getprop active_border_color`
    shows it. The colour is the theme's `red` from `colors.toml` (the colour Omarchy's `shell.toml`
    gives the bar's `active` modules, which the widget's `active` icon uses too; the theme's accent is
    already the normal active border, so it would not stand out), `rgb(ff5555)` when the theme names
    none. And `<boxes>/.keys` (the runtime dir's omabox folder, not a box's) is renamed into place
    with the box's name, or an empty line: the widget watches it with a FileView and lights its icon
    (WidgetButton's `active`, the bar's urgent colour) and says "SUPER keys here" on that row, only for
    a box `ls` has up (a file left by a Hyprland that went away lights nothing). The file is also read
    at each list poll: a FileView does not watch a file whose folder was missing when the shell
    started. Hyprland quirk found on the way: while the focused window closes, `window.active` passes
    nil but `hl.get_active_window()` still returns the closing window, so the submap stayed on after a
    sticky box went down; the hook now tells "no window" (false) from "ask Hyprland" (nil). Checked in
    a stand-in host (finding 26) with two interactive boxes and a foot: focus on the sticky box
    enters, border red (the stand-in theme's), file names it; foot leaves, border back; SUPER+2
    switched the box's workspace and not the stand-in's; SUPER+ALT+ESCAPE got the keys back with the
    box still focused (SUPER+1 then switched the stand-in), and focus away and back entered again; the
    other box did not enter; one-shot on it lit the indicator and ended on a key with the pointer off
    it; sticky ignored that key; `off` with the box focused left at once; after a confirm-close keep
    the new window was still the sticky box; `down` with it focused left no submap on. The widget in a
    box with a stub CLI: `f` ran `keys-to-box -b NAME on`/`off`, the icon lit from the file (first at
    a poll, then within a watch). `t_keys_to_box`, `t_unit_keys_to_box`, `t_widget`. Not checked on
    the real desktop: the border on a real theme, focus-follows-mouse with a real pointer, the widget
    in the user's bar.

    **2026-09-30, issue #55: the pointer decides too** (the maintainer, on the real desktop with 0.3.0:
    the box alone on its workspace, pointer on the bar, SUPER+1 went to the box: the bar is a layer,
    so focus never left it; "focus alone decides" was not what a user expects). Sticky mode now needs
    the box's window focused AND the pointer over it (inside `at`/`size`: the border and gaps are
    off it); the pointer anywhere else (the bar, an empty part of the workspace, another monitor)
    leaves the submap with keys-to-box still on, and back over the box enters it again. The catch
    above is gone. `passthrough.lua` version 4 (`PASS_VERSION` with it, so a running host's version 3
    is replaced at the next `up --interactive`, `keys-to-box`, or reaper tick of a box this version
    started). **How:** Hyprland 0.56.2 gives Lua no pointer event (`hl.meta.lua`'s event list has no motion, enter or hover; only
    `input.keyboard.key`), so two things look at `hl.get_cursor_pos()` against the focused box
    window: an `hl.timer` every 100 ms, enabled only while a keys-to-box box is focused and not
    paused by the toggle key (focus on anything else, or the toggle, disables it: no cost otherwise;
    a tick is the focused window, the cursor and the submap looked up, a dispatch only on a change),
    which moves the indicator (border, `.keys`) with the pointer; and the key hook, which does the same
    on every key press. **Ordering, found in a stand-in:** the `input.keyboard.key` hook runs before
    Hyprland looks the key up in the binds, and a submap change made in it applies to that very key
    (a hook that reset a submap on the `1` press let SUPER+1 hit the default submap's workspace bind).
    So the key pressed right after the pointer moved goes where the pointer says, even before the
    timer has looked: SUPER+1 with the pointer off the box switches the host's workspace on the first
    press, never eaten by the box. (Finding 29's "that first key still reaches the box" was about
    modifiers: an unbound key goes to the focused window in any submap, and SUPER alone is unbound.)
    Timers, like hooks, are gone at a config reload (checked: one stopped firing); a newer version
    disables an older one's (`omabox_pass_timer`). The toggle key in sticky mode: over the box it
    gives the keys back until focus leaves and returns, as before, and the pointer leaving and coming
    back does not undo that; with the pointer off the box it only clears that pause (the pointer
    decides). Focus moved by keyboard onto the box: Omarchy warps the cursor to the focused window
    (`cursor.no_warps` off; `hl.dsp.focus` warps it to the window's middle in the stand-in), so the
    keys are the box's there too. One-shot mode unchanged. Checked in the stand-in (`t_keys_to_box`):
    the box alone on a stand-in workspace, keys-to-box on, pointer over it: submap, border, file; the
    pointer on an empty corner (where the bar would be; the stand-in runs `--no-shell`): all three off,
    the box still focused, keys-to-box still on, SUPER+1 switched the stand-in's workspace on the
    first press; the pointer back: all on, SUPER+3 went to the box; with the timer disabled by hand,
    both directions still routed on the first press (the key hook alone); focus starts the timer and
    focus elsewhere stops it; every older check (one-shot, reload re-install, confirm-close keep, down
    while focused) as before. Not checked on the real desktop: a real pointer on the real bar and
    across monitors, and whether 100 ms of indicator lag is noticed.
118. **A host config reload left passthrough on with nothing bound** (2026-09-30, found while doing
    117). `hyprctl reload` on the stand-in drops every Lua global, hook, `hl.bind` and window rule
    that `hyprctl eval` added, but keeps the current submap (and tags on windows): with the `omabox`
    submap on at a reload, it stayed on with nothing defined in it, so no SUPER bind of the host
    worked and the toggle key did nothing, until something reset it by hand. It was so since finding
    26 (the host side was "gone at the next config reload"), and Omarchy reloads on every theme change.
    An interactive box's reaper (every 2 s already) now asks the host whether version 3 is there (one
    `hyprctl eval` of a comparison that errors on purpose) and sends `passthrough.lua` again when it
    is not (`keys_ensure`, its host session found once, in a subshell so a host that does not answer
    never ends the reaper). The install strips the tag from every box window and starts over from
    what focus says: a submap left on stays on only for a focused box that should have it. The theme's
    colour is read at each install, so a theme change (a reload) brings the new one. Checked in the
    stand-in: reload with the sticky box focused, the hooks back within 2 s, one toggle bind in each
    submap, border and file as before, focus still drives it (`t_keys_to_box`). A box started by an
    older omabox has an older reaper: its hooks come back at the next `up --interactive` or
    `keys-to-box`. The brief gap (up to 2 s after a reload) is still there.
119. **`omabox clip`: the user's clipboard into an interactive box, and back** (2026-09-30, issue
    #23: driving an interactive box, a password or URL had to go in by hand, `wl-paste -n | omabox
    run -b BOX -- wl-copy`). `clip [-b NAME]` reads the host's clipboard item (`wl-paste` on the
    session `host_session` finds, whatever this shell's display is) and puts it on the box's;
    `--from-box` the mirror. One shot, decided with the maintainer: no watcher, no live sync, no
    toggle (the issue's "share clipboard" is left out). Nothing stays running but what a Wayland
    copy always leaves: `wl-copy` forks to serve the item until the next copy replaces it, in the
    box (dies with it) or on the host. Both run in a session of their own (`setsid -w`, so a
    closing terminal or the widget's process group does not take the item with it), with no fd of
    omabox's: stdout /dev/null, stderr a file (the box's own `mktemp` for the box's, an unlinked
    one of omabox's for the host's; a pipe held by the fork kept `$(...)` and the widget's
    collector waiting). The item passes through an unlinked file in `$XDG_RUNTIME_DIR` (tmpfs,
    0600, gone however omabox ends; `/dev/fd/N` reopens it), capped at 64 MiB, each read under 10 s
    (`timeout`); the fds are closed for every child, or the box's wl-copy fork would hold the host
    file read-write. The box side runs by absolute paths (`/usr/bin/wl-copy`: the box's
    `~/.local/bin` comes first on its PATH and is the box's to write), and what a box says back
    (its type list, an error) is printed tame and short. *Types*: text first
    (`text/plain;charset=utf-8`, `text/plain;charset=UTF-8`, `UTF8_STRING`, `text/plain`, `STRING`,
    `TEXT`), handed over as `text/plain;charset=utf-8` (wl-copy offers the other names along with
    it); else an image by its own type (`image/png` first, any `image/NAME`), bytes unchanged, which
    was as simple as text; anything else (a file list, `text/html` alone) is refused naming the
    types. Empty clipboard, empty item, over the cap: refused, nothing handed over. It says what it
    handed over (text or an image, the type, the size), never the content. A password manager's
    `x-kde-passwordManagerHint` goes along (`wl-copy --sensitive`): Omarchy's clipboard history
    (the shell's `wl-paste --watch` into `~/.local/state/omarchy`) skips such items, in the box and,
    for `--from-box`, on the host; anything else from `--from-box` lands in the user's history like
    any copy (both seen on a stand-in with the shell). *Which box*: `-b NAME`, else the interactive box whose window has the host's focus
    (the window is the outer bwrap's, as for `peek --focus`), else the only one; several and none
    focused: refused, naming them. So a key binding of the user's (none installed; README has
    `o.bind("SUPER + ALT + V", ..., "omabox clip")`) pastes into the box being worked in, except
    while SUPER+ALT+ESCAPE sends SUPER keys to the box. Headless boxes are refused (agents'). The
    widget: Paste in / Copy out buttons (`v`, `c`) on an interactive box's row only, closing the
    panel; the CLI's line comes back as a notification. `clip` is in `BOX_CMDS` (`-b` first works).
    Verified with a box standing in for the host (finding 26) and interactive boxes nested in it,
    the clipboards the stand-in's and theirs (`t_clip`): text with a non-ASCII word and a trailing
    newline byte for byte, the text names offered in the box, no `-b` with one box, a PNG of the
    screen both ways (same md5, `image/png`), a JPEG as `image/jpeg`, `--sensitive` carried, the
    refusals (another type, an empty clipboard or item, both sides, a headless box, two boxes and
    none focused), the focused one of two picked, after it all one wl-copy on the stand-in, no
    wl-paste or clip process, no process holding the item's file, the stand-in's windows and focus
    unchanged (data-control: no window of wl-paste's), and the widget's `v` and `c` running the real
    CLI there (a stub CLI in `t_widget` for the rows: none on a headless box, the notification).
    Not checked: the real desktop (the user's clipboard is never read by an agent, not to test):
    real apps' offers (a browser's copied image, KeePassXC's secret), the bind, the widget in the
    user's bar. Same Hyprland and wl-clipboard as the stand-in, so expected the same. Not handled:
    the primary selection; an item's other types (one type goes across); a box that stops its own
    `wl-paste` on purpose (ptrace) can hang `--from-box` past its `timeout` (Ctrl-C; the widget
    stays busy). `t_unit_clip`, `t_clip`, `t_widget`.
120. **`clip` is never an agent's** (2026-09-30, issue #23). The guard keeps agents' shells off the
    user's display, and so off their clipboard (finding 65); an omabox command that read it for them
    would undo that. `clip` refuses before anything else when `agent_caller` finds an agent: in
    ai-jail (the broker's `OMABOX_JAIL`; `broker_check` also refuses `clip` by name, so it never
    joins the allowed list), or a mark in the environment of this process or any it runs under, up
    to PID 1 or its pid namespace's edge (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PID`,
    `CODEX_THREAD_ID`, `CODEX_SANDBOX`, `OPENCODE`, `AI_AGENT`, `guard exec`'s `OMABOX_AGENT_PID` and
    `OMABOX_SESSION`, the guard's display, signature and PATH entry), or such a process named
    `claude`, `codex*`, `opencode`, `pi`, `hermes*` (or node, bun, deno, python running one).
    `/proc/PID/environ` is a process's environment at its start: `env -u`/`env -i` in front of
    omabox leaves the shell it came from marked, and `exec env -i` leaves the agent's own process
    above it (`t_unit_clip`, `t_clip`: each mark, an ancestor's, a program named claude, node with
    Claude Code's path, guard exec, `omabox host`). One line says why and that the user runs it
    (a terminal of theirs, a key binding, the widget); the skill tells agents not to try. *What
    still gets past* (a seatbelt, not a fence, like the guard): a process that does not descend
    from the agent (`setsid -f` or a double fork, reparented to the user manager; `systemd-run
    --user`; `hyprctl dispatch exec` with the real signature), a new pid namespace (`unshare -p`
    cuts the walk as a box's does), an agent run under a program name and without variables of the
    ones above, and above all a bare `wl-paste` on the real socket, which the guard never stopped.
    Only ai-jail fences (display hidden, broker refusing). Also refused, rightly: `!omabox clip` in
    Claude Code's prompt (the agent's shell) and a terminal an agent opened (`omabox host -- foot`).
    What the user hands to a box is the box's: anything running there (an agent driving that box
    with `run -- wl-paste`) can read it, and a box with the shell keeps an item not marked sensitive
    in its clipboard history (`~/.local/state/omarchy/clipboard-history.json` in its HOME, images
    beside it) until the box goes; a `save` of the box keeps it.
121. **A peek the user opens at the suite's boxes is theirs, not a leak** (2026-09-30, issue #45: during
    the 0.3.0 merge runs Diogo peeked at two `t<pid>-ag-*` boxes from the bar widget and the run
    failed three checks: the leak detector saw a peek window of the run's box opened, focused, and
    workspace 9 come up; `t_agent_session` waited for a box that a peek holds in use, by design
    (`_reap`'s `peek_pid`); and the end's focus check found focus on the peek). Nothing said why: it
    took the host event log. Now `omabox peek` says who asked, in the peek process's environment,
    through the host Hyprland's exec (`env MARKER omabox-peek ...`; env execs peek, so `peek_pid`'s
    command-line match holds): `OMABOX_SUITE=t<pid>` when the caller has it (every omabox the suite
    runs), else `OMABOX_PEEK_BY=you` when the command run was `peek` (the widget's click, a
    terminal), else nothing (`peek_marker`). The watcher already read focused windows' environ; it now
    reads `OMABOX_PEEK_BY` too, and after a peek's `openwindow` looks the window up in `clients` and
    logs a `+` line with its tags (a failed lookup logs `+ 0xADDR ? ERROR`). `leak_scan` decides a
    this-run peek on that line: `OMABOX_SUITE` of this run: a leak "by this run's commands";
    `OMABOX_PEEK_BY=you`: a note, "watched by you"; anything else, or no `+` line: a leak whose
    message says a command other than `omabox peek` opened it, or the user did with an omabox older
    than the suite (then run again without peeking). Focus on a peek marked yours is a note, and
    omabox's workspace coming up is decided by the focus right after it (a stand-in showed `peek
    --focus` gives activewindow, workspace 9, activewindow again): on a peek of yours, a note, else a
    leak as before. So `host_same` at the end, which needs a clean scan, passes too. Why a mark of
    the user's and not just the suite's, and why in the process rather than a record in the box dir:
    an unmarked peek must stay a leak, since that is what a regression looks like (`up` or another
    command opening one through its own exec: no `peek_marker`), and the suite's own `omabox peek`
    carries its `OMABOX_SUITE`, so neither can read as the user's. Only `omabox peek`, run as the
    command, marks a window yours: a suite process that lost `OMABOX_SUITE` (none does: the reaper
    inherits it through `setsid`) and reached `cmd_peek` some other way would give no mark, a leak.
    The environment is set by the exec and read by the detector from `/proc`, where the box cannot
    write; a box-dir record would be one more file to keep in step with a window that the user can
    close any time. What is still excused wrongly: a real leak that brings up workspace 9 while a peek
    of yours sits there and takes the focus; and, below, the checks a peek holds up. `test/run.sh`:
    `your_peek BOX` (a peek process at that box with `OMABOX_PEEK_BY=you`), `held BOX CHECK...`, and
    `no` skipping ("held by your peek at BOX") a held check that fails while such a peek is open:
    `t_agent_session`'s reaping checks, `t_idle`, `t_reap_race`, `t_run_idle`, and `t_peek`'s marks
    (the CLI writes marks for any peek of the box). Verified: `t_unit_leak_scan` (the widget's event
    sequence with each marker, a missing or failed `+` line, workspace 9 with other focus, `host_same`,
    `your_peek`/`held` with a stand-in process, `peek_marker`), and `t_leak_control` live on a stand-in
    host with a box inside it: `omabox peek --focus` run there (the box's environment has no
    `OMABOX_SUITE`, as the widget's) is noted, the same with `OMABOX_SUITE` is reported, and a peek
    its Hyprland starts with no marker is reported. Not verified: a real bar-widget click on the
    host during a run (never on the real desktop; the widget runs the same `omabox peek --focus`).
    **2026-09-30, issue #56: the focus comes before the workspace.** A full run failed `t_unit_inspect`
    on "omabox's workspace 9 came up" while Diogo went to workspace 9 for his own interactive box
    `box-1`: focus on `box=box-1` was already a note, but only a peek of his excused the workspace.
    Looking at the events in a box showed the rule above read the wrong line: in Hyprland 0.56 a
    workspace switch (a `focus` dispatch or SUPER+9 alike) sends `activewindowv2` (and the watcher's
    `~` line) *before* `workspacev2`; `peek --focus` only passed because it focuses a second time
    after. Now `leak_scan` judges omabox's workspace by the focus line right before it, or, when
    there is none (another line in between, or focus on nothing), the one right after: focus on a
    window that is not the suite's (the user's box, their peek, any app with no omabox marks) is a
    note, "workspace 9 for WHO"; the suite's, another box's process, an unmarked interactive box or
    peek, a window the watcher could not ask about (`~ ? ERROR`), or no focus at all is a leak, as
    before. What this excuses wrongly: a real leak that shows workspace 9 while a window of the
    user's sits there and takes the focus (the suite's boxes open nothing on the host, so a leak there
    would be a workspace dispatch reaching it). A **special** workspace configured (`omabox config
    workspace special[:NAME]`, `HWS=special:NAME`) was never seen: it shows with
    `activespecial>>special:NAME,MONITOR` (hidden: `activespecial>>,MONITOR`; seen in a box, the
    focus line again first), which the watcher did not keep. The watcher keeps `activespecial>>` now
    and `leak_scan` treats it as a workspace line (another special one is a note, a closing one
    nothing); the run's start leaves it unwatched when the focused monitor already shows it, as for a
    numbered one. Verified: `t_unit_leak_scan` (#56's own lines, focus before and after, each kind of
    leak, stale focus, special workspaces) and `t_leak_control` live on the stand-in: workspace 9
    brought up with focus on an unmarked `foot` (an app of the user's) is noted "workspace 9 for
    foot", and `special:omabox` shown empty is reported.
122. **An NVIDIA GPU switched to its driver at runtime has no `/dev/nvidiaN` yet** (2026-09-30, this
    machine: the RTX 5070 Ti moved from `vfio-pci` to `nvidia` while the session ran). The driver
    listed it (`/proc/driver/nvidia/gpus/0000:01:00.0/information`, Device Minor 0) and
    `/dev/nvidiactl` existed, but not `/dev/nvidia0`. The nodes are made by `nvidia-modprobe`, which
    nvidia-utils' udev rule (`60-nvidia.rules`) runs on bind only while `/dev/nvidia-uvm` does not
    exist; here the uvm nodes were already there, so no `/dev/nvidia0`. `up` with `OMABOX_RENDER_NODE` on its render
    node stopped: "NVIDIA GPU 0000:01:00.0 has no usable /dev/nvidia0 or /dev/nvidiactl".
    `nvidia-modprobe -c 0` (nvidia-utils' setuid helper, made for unprivileged users) created it and
    the box then started and passed. `nvidia_device` now does that itself: when the minor's node or
    `nvidiactl` is missing it runs `nvidia-modprobe -c MINOR` once (if installed) and looks again;
    still missing, the error names the command to run. Not for an interactive box's other GPUs
    (`nvidia_device N 0`): a missing node there means the desktop does not render on it, and it is
    left out as before. The body is `nvidia_node SLOT CREATE PROC DEV`, so `t_unit_nvidia` runs it
    on a fake `/proc/driver/nvidia` and `/dev` with a stub helper. Not reproduced live since: the
    node now exists, and making it disappear needs root. `t_main`'s NVIDIA branch said "the render
    node's driver is nvidia, not nvidia" when `up` had failed there: it now fails saying the box did
    not get the private Wayland screen.

123. **The guard outlived omabox** (2026-09-30, issue #51, for packaging, #53). The Claude Code hook
    is a standalone snippet that never calls omabox, so after omabox was deleted without `guard off`
    every session still got the guard's display and `PATH` entry, with no `guard off` or `omabox
    host` left to get out. The hook now starts with `[ ! -x '$ROOT/bin/omabox' ]`: omabox gone, it
    writes nothing to `$CLAUDE_ENV_FILE` and prints one line (omabox is gone, the guard is not
    applied, the hook naming `omabox-guard` can go from settings.json). Codex's guard is fixed values
    in config.toml, which nothing there can make conditional, so Codex also gets a `SessionStart` hook
    in `~/.codex/hooks.json` (Codex 0.159: hooks are a stable feature, the file has Claude Code's
    format, so `guard_hooks` now does both with the same jq): with omabox there it prints the guard's
    note, which Codex's agents never had; gone, it says the guard still applies and which lines to
    delete from config.toml. Codex runs a new or changed hook only after the user trusts it (`/hooks`;
    it keeps a `trusted_hash`), so `guard on` says so when it adds one; omabox never writes that
    trust itself. Codex's state is on only with both the block and the hook current; either alone
    reads outdated, so an install from before this is updated by `install.sh`, which already offers
    `guard on` for an outdated guard. `guard on` reads hooks.json before writing config.toml, so a
    broken hooks.json leaves both untouched. README's Remove section now says to run `guard off`
    before deleting omabox. Verified: `t_unit_guard_settings` (both hooks from a copy of the CLI:
    applied, then with the copy deleted, one line and no env file; Codex's hook added next to
    another, trust asked once, a block without it outdated, a broken hooks.json refused before
    anything is written, `off` takes it out). Not verified: the hook in a logged-in Codex session
    (no login here), so that Codex adds its stdout to the context, as Claude Code does, is from the
    shape of its hooks, not seen.

124. **The suite runs box tests in parallel** (2026-09-30, issue #60). A full run took ~9 min (537 s):
    39 s of unit tests (30 of them a `sleep 30 |` in `t_unit_inspect`, fixed first) and 498 s of box
    tests one after another, mostly waiting (idle limits running out, reapers polling, timeouts that
    prove something does not happen) on a machine left idle. Now `test/run.sh` runs the unit tests
    and `SERIAL` (empty: no test needed it) one at a time, then the box tests `-j N` at a time (default
    half the CPUs, at most one per 2 GB available and 8; `OMABOX_TEST_JOBS`; `-j 1` is the old run).
    Each runs in a subshell (`par_test`) with its own notes and last-wait files, stops the servers it
    started, and hands its counts back in a file from an EXIT trap, so a test that exits midway (an
    unset variable) keeps its counts and fails "ran to its end"; the runner (`par_done`) prints its
    output whole when it ends, with its time. t_leak_control and the slowest start first. Leaks: the
    runner marks `== TEST` and `== /TEST` in the host's event log and scans each test's window when it
    ends; tests beside it share that window, so a leak there fails each, naming the others (`beside`);
    `slice` leaves other tests' markers out (one between an event and the line that decides it would
    read as a leak); what came while no test ran (`gap_events`, between `== parallel` markers) is
    scanned once at the end. Audit before: no two box tests share a box name, a `$TMP` path or a temp
    repo; the ones that read every box (`ls`) or the user's settings do it by name, or inside a
    stand-in box with its own HOME and runtime dir; background subshells do not run the suite's EXIT
    trap (checked). Ctrl-C: bash leaves background jobs ignoring SIGINT and the runner sat in `wait
    -n`, so a run went on; INT and TERM are trapped now, and `cleanup` kills everything under the run
    (`descendants`, deepest first) before taking its boxes down. A test's flock holder left looping on
    a file in the deleted `$TMP` kept a box "busy" once: those loops now also end when `$TMP` is gone.
    Found on the way: `omabox ls` stopped partway when a box went down while it listed them (`meta`
    failed on the gone box.json, under errexit; the list ended there): a box going down is now left
    out, the rest shown (`t_unit_cli`, which failed on the old code). Verified on this machine: AMD
    iGPU, -j 4 three times (1221/0/1, 141 s each), -j 6 (101 s), -j 8 (88 s), -j 1 (509 s); the same
    three slow waits (half their limit) in every run, and no test more than 2 s slower beside 7 others
    than alone. RTX 5070 Ti (`rtx nvidia`, renderD129, parked with `rtx vfio` after): default (8)
    twice, 76 and 75 s (one run hit the `ls` bug, 1222/1; the other 1223/0/0), -j 1 411 s 1223/0/0.
    With the `ls` fix, at the default: NVIDIA twice 1224/0/0 in 75 s, AMD twice 1222/0/1 in 94 and
    90 s. Ctrl-C checked on both -j 4 and -j 1 runs: gone within 2 s, no box, process or
    runtime file left. `t_unit_parallel` covers the windows, `beside`, the gaps and what a test hands
    back (made-up log and tests).

125. **Only headless NVIDIA boxes and confirm-close need aquamarine's fix; the private build is
    optional** (2026-09-30, issues #47, #53). Every box ran a private libaquamarine (`build/prefix`,
    0.15.1@7bb8bdf4, finding 4) since the spike; a package cannot ship that. Checked on the system's
    0.15.0 in real boxes (the box Hyprland's maps and fds read each time), on the AMD iGPU and on the
    RTX 5070 Ti, the desktop itself on each in turn: headless AMD/Intel boxes work (their screen is a
    headless output, `HEADLESS-2`; the Wayland output stock never flushes is disabled there);
    interactive boxes start, take input, follow their window's resizes (a second box tiling beside it
    on workspace 9 and going again, on the real NVIDIA desktop). Two things fail: a headless box on an
    NVIDIA render node (its screen is the private labwc's Wayland output, finding 77: `Output WAYLAND-1:
    initialized`, then only `FALLBACK`, `Hyprland not up after 30s`), and confirm-close (finding 70),
    whose new window is `hyprctl output create wayland`, a Wayland output made after the backend
    started: stock never sends its first commit, so `WAYLAND-2` never comes and the box ran on with no
    window at all and nothing left to close (the suite on stock: the 7 confirm-close checks of
    `t_guard` and `t_keys_to_box`; reproduced on the real desktop). An interactive box's first window
    works because the backend's start flushes it. A headless box with no `OMABOX_RENDER_NODE` takes the
    first render node, so on a hybrid machine it renders on the iGPU and needs nothing, whichever GPU
    the desktop is on.
    So: `aq_pick` chooses the box's aquamarine: a checkout's `build/prefix`, then the user's build
    (`~/.local/share/omabox/aquamarine`), each only while it has the soname the box's Hyprland links
    (an older one is stale: skipped, said so), else the system's (nothing mounted at `/opt/omabox/lib`,
    no `LD_LIBRARY_PATH`: `share/start-hyprland.sh` sets it only when the dir is there). A private build
    has the fix; the system's has it after 0.15.1 (`AQ_FIXED_AFTER`; the release after that one comes
    from a main that has #415). Without it `up` refuses a headless box on an NVIDIA render node (naming
    other GPUs for `OMABOX_RENDER_NODE`) and `--confirm-close`, before anything is made, saying to run
    `omabox setup --aquamarine`; confirm-close from the settings is turned off for that box with a
    note, and `config confirm-close on` notes it, and `confirm_live` leaves such boxes alone (box.json
    `aquamarine.fixed`). `confirm-close.sh` ends the box when no window comes within 5 s, whatever the
    library: its wait had counted Hyprland's `FALLBACK` as a monitor, so it never waited. `omabox setup
    --aquamarine` builds `AQ_COMMIT` (into `build/prefix` in a checkout, which `install.sh` now calls,
    else into the user's data dir, its source in `~/.cache/omabox-aquamarine`, not under the box HOMEs
    `sweep_homes` clears), installed beside the old one and swapped in; a no-op while the box's
    aquamarine has the fix. Its tools (git, cmake, ninja, hyprwayland-scanner, base-devel) are what a
    package lists as optional. `--version` says which aquamarine new boxes use, `ls --json` each
    box's. `OMABOX_AQUAMARINE=system` skips the private builds, for this box and the boxes it starts
    (the suite's stand-in hosts): `OMABOX_AQUAMARINE=system test/run.sh` is the suite on stock.
    Verified: from a copy of the tree without `.git` (as a package), `setup --aquamarine` cloned and
    built into a scratch data dir in 12 s, and a headless box on the RTX ran on it (`WAYLAND-1`); a
    second run built nothing. On stock: the NVIDIA refusal, `--confirm-close` refused, the settings'
    confirm-close turned off (one close ended the box, cleared), a box with the flag forced on ended
    6 s after its close. `t_unit_aquamarine` covers the choice and the refusals.
    The bar widget's "Confirm before closing" switch ran `config confirm-close on`, which succeeds (the
    note is on stderr, which the widget shows only for a failure), so it read on while no box asked.
    `config --json` now has a read-only `confirm-close-available`; the widget, which reads it each
    time the panel or Settings opens, shows the switch off and greyed, not clickable, with "Needs
    aquamarine's fix ...: run omabox setup --aquamarine" as its caption. Checked by hand in a box (stub
    `omabox`, as `t_widget` has it): unavailable, a click on the switch sent nothing; available after
    reopening Settings, on, and a click sent `config confirm-close off`. The suite checks the CLI's
    field only: no key opens Settings, and the gear's place depends on the bar layout a box copies.
126. **`omabox setup` is what each user runs; install.sh is the system part** (2026-10-01, issue #49).
    A package installs files for everyone; it cannot link into each user's HOME. So the per-user
    steps of install.sh moved, unchanged and with the same messages, into `omabox setup`: the
    `~/.local/bin/omabox` link (not for a system install under `/usr`, whose command is
    `/usr/bin/omabox`), the agent skill in each agent's dir, the bar widget, `~/.config/omabox`, the
    agent guard (asked in a terminal, a "no" remembered). install.sh keeps the packages, the tools and
    `setup --aquamarine` (finding 125), then calls `omabox setup`. The widget must stay a per-user
    link: Omarchy's shell reads third-party plugins only from `~/.config/omarchy/plugins` (its
    `PluginRegistry`; its own dir holds first-party plugins, and `omarchy.*` ids are reserved).
    `omabox setup --remove` undoes it: `guard off` when this HOME's guard is on or outdated, `broker
    off` when this HOME has the broker's unit, `omarchy plugin disable chaves.omabox` when this HOME's
    `shell.json` has the widget (that asks the running shell, as the command does), then the links,
    each only while it still points at this omabox (never a real dir or a link moved elsewhere), and
    the per-user aquamarine build and its source. Settings and saves stay, said so; a checkout's
    `build/` goes with the checkout. `t_unit_setup` checks both in a temp HOME, with the XDG dirs
    pointed there too: the session sets them, so HOME alone would have aimed `--remove` at the user's
    own aquamarine build (seen in a first run of the test: nothing was there to delete).
127. **`shot -o` to a path that cannot be written says so, before capturing** (2026-10-01, issue #63).
    The capture was written to `OUT.part` through a redirect; when that failed (a read-only mount, a
    root-owned dir) the shot took it for a failed capture: an interactive box said "no frame ... in
    10 s: ask the user" (after 0.05 s, sending an agent to the user for a bad path), a headless one
    "grim failed" after the shell's own error. `shot_path` now makes the directory and writes and
    removes `OUT.part` before the box is asked for anything: `shot: cannot write OUT: Permission
    denied` (or the directory that cannot be made), exit 1, nothing left behind. The default path in
    `$TMPDIR` goes through it too, once the box's name is known. Checked in a real box and by
    `t_unit_shot_hidden` (both modes, the box never asked).
128. **omabox runs from a read-only system install** (2026-10-01, issue #50). A package would put the
    tree at `/usr/lib/omabox` (not `/opt/omabox`: the tools' path inside a box) with `/usr/bin/omabox`
    linking to it. `bin/omabox` finds `ROOT` through `readlink -f`, and nothing writes under it at run
    time: the aquamarine build goes to the user's data dir outside a checkout (finding 125), `setup`
    makes no `~/.local/bin` link under `/usr` (finding 126), and `broker on` builds the relay only when
    it is missing (a package ships it built). The guard's `GUARD_PATH`, the broker's units and
    `~/.ai-jail` lines, the skill links and the box mounts (`/opt/omabox/share`, the tools) all take
    `/usr/lib/omabox` as they took a checkout. `test/run.sh --installed` checks it: the checkout's
    tracked files and built tools, read-only at `/usr/lib/omabox` with `/usr/bin/omabox`, in a
    throwaway mount namespace (overlays on `/usr/lib` and `/usr/bin` made as root of a user namespace,
    then the user's own uid in one below it, with no `no_new_privs`, which boxes behind pasta need,
    finding 89), running the install's own copy of the suite. Boxes there run on the system's
    aquamarine (no private build in reach), as a package's would. What the suite assumed of a checkout
    now tells the two apart: `t_unit_install` skips (install.sh builds the tools into ROOT; a package
    builds them itself) and has its own XDG dirs in a checkout too (from the installed tree it had
    built aquamarine into the user's real data dir, removed at once); the default box name and
    setup's link are checked against what ROOT gives. Two checks cannot run there: `t_jail` (ai-jail
    runs only a root-owned bwrap, and the user namespace shows root's files as nobody's) and
    `t_widget`'s missing command (a box sees the host's `/usr/bin/omabox`). The files are the user's,
    read-only, not root's. Run on this machine: checkout 1268/0/1, installed 1221/0/6.
129. **The package, tested on a fresh Omarchy in a VM; the suite knows an install by itself**
    (2026-10-01). A PKGBUILD for omarchy-pkgs (tree at `/usr/lib/omabox`, tools built in `build()`,
    `xdg-terminal-exec` from `[omarchy]`), on a fresh Omarchy 4.0.4 in QEMU/KVM (omarchy-in-omarchy,
    run headless: `egl-headless` on a host render node, the guest a virgl one): built with
    `makechrootpkg -c` from Omarchy's `pacman.conf`, namcap clean but for what it cannot see (the
    commands omabox runs, the shell's own QML modules); `pacman -U`, `omabox setup`, a box up in ~4 s
    on virgl, keys, shots, the widget listing it; `setup --remove` and `pacman -R` left nothing
    under `/usr` and no dangling link. The suite run from that install failed 14 checks, none the
    package's: it knew an install only by `OMABOX_TEST_INSTALLED`, which `--installed` sets and a
    package does not (13: `t_unit_install` ran install.sh, `t_widget` expected no
    `/usr/bin/omabox`), and `gpu --json lists Hyprland` wants DRM fdinfo, which virtio_gpu keeps
    none of. Now an install is a read-only ROOT (a checkout never is), the widget check skips
    whenever the host has `/usr/bin/omabox`, and the gpu check skips when a client of the render
    node gets no `drm-driver` fdinfo line (amdgpu writes one on a fresh fd, before any engine
    time). `OMABOX_TEST_INSTALLED` stays for `--installed`'s user namespace (`t_jail`).
130. **What more package testing found** (2026-10-01, for the omarchy-pkgs PR). Round two, the
    package from omarchy-pkgs' own builder (their CI's `bin/build`, in Docker in the VM), plus
    `setup --aquamarine` from the package, an interactive box, an upgrade (`-1` to `-2`), a second
    user, and the installed layout with boxes on the RTX. Fixed:
    - **A user Omarchy has set no theme for** (an account that never logged in: Omarchy makes
      `~/.local/state/omarchy/current/theme` at the first login) made `up` die on a bare `cp: cannot
      stat`, leaving the box's dirs behind. `up` now says so before anything is made, and what to do.
    - **No box can start** (on NVIDIA without aquamarine's fix, from an install that has no private
      build): the suite failed 166 checks, one per `up`. It now starts a probe box first; when that is
      refused it fails once, with `up`'s message, and runs the unit tests only.
    - **A box started inside a box** (the suite's stand-in hosts) looked for a private aquamarine only
      in the checkout's `build/prefix` and the data dir, out of reach from a system install there. In a
      box, `aq_resolve` now also takes the box's own, which `up` binds at `/opt/omabox/lib`. The save
      tests use a data dir of their own: they link the user's build into it.
    - `setup --remove` left `~/.cache/omabox` (where boxes' homes go) behind, empty: it now removes it
      and the data dir's `omabox/` once empty (never a box's home still in it, nor saves).
    - The tools' Makefiles put `$(LDFLAGS)` after the libraries, so a distro's `-Wl,--as-needed` came
      too late: `libm` (from wayland-client's pkg-config) stayed linked unused in keyboard, peek and
      still (namcap said so). `$(LDFLAGS)` now comes first.
    Kept on purpose: an empty `.lock-NAME` per box name in `$XDG_RUNTIME_DIR/omabox` outlives the box.
    Removing a lock file another `up` may be waiting on is the classic flock race (two holders of
    "the same" lock); they are empty and on a tmpfs gone at logout. The recipe gained `base-devel` in
    its optdepends (omabox's own `setup --aquamarine` hint names it).
Findings 131-136 started from reading omadev (github.com/llstrk/omadev, MIT), nested Omarchy
sessions as windows; each was checked in boxes first, and no code was taken from it.
131. **`systemd-run` and `systemd-cat` stand-ins** (2026-10-01). Both checked in a box before the fix.
    Omarchy 4 starts the browser (SUPER+SHIFT+B, `omarchy-launch-browser`), LocalSend (menu share) and
    the Hermes theme with `systemd-run --user`; a box without `--systemd` has no user manager, so it
    said "Failed to connect to user scope bus" and the launcher still exited 0: the bind opened
    nothing. `omarchy restart shell` (Omarchy's own, not `omabox restart-shell`) killed the bar and
    asked `omarchy-launch-shell` for a new one, which runs it under `systemd-cat -t omarchy-shell`;
    with no journald that failed, its supervisor retried, and the box was left with no bar.
    `share/bin/systemd-run` (first on the box's PATH) runs a `--user` command directly: detached,
    output in `apps.log`, or in the foreground with `--wait`, `--pipe` or `--scope` (exit code passed
    on); `-E`, `--working-directory`, `--same-dir` are kept, the unit options dropped. Timers
    (`--on-*`) are refused, naming `--systemd`: reminders also list and stop their timers with
    `systemctl`. With `--systemd` (or without `--user`) it is the real one, except for omabox's
    `uwsm-app`: that detaches the app and returns, so the transient unit ended at once and took the app
    with its cgroup: in `--systemd` boxes the browser bind had never worked either.
    `share/bin/systemd-cat` appends to `~/IDENTIFIER.log`; the shell's tag goes to `~/shell.log` and
    records the shell's pid where `omabox restart-shell` looks. `share/shell.sh` now asks the old shell
    to quit (`quickshell kill --pid`) before a signal: a shell Omarchy's launcher started is started
    again when it dies of a signal, not when it quits. Either restart after the other leaves one shell
    (`t_omarchy_restart`).

132. **Keys held when an interactive box loses focus stayed down in it** (2026-10-01). aquamarine's
    Wayland backend listens only to `wl_keyboard.key` and `modifiers`, not `leave`, so a key down when
    the host moves focus off the box window (SUPER+1 on the host, focus-follows-mouse) is released
    where focus is by then, and the box never hears it. Seen in a stand-in host (finding 26): SUPER
    held in the box while focus went to the stand-in's foot, released there; back in the box, `w`
    closed the box's foot (SUPER+W) instead of typing. A control with no focus change typed `w`. A held
    letter would repeat in the box's app the same way. `patches/aquamarine/` carries the fix, which
    `setup --aquamarine` applies to `AQ_COMMIT`: the keys reported pressed are tracked and released on
    `leave`, with a `modifiers` event that keeps only the locks; keys down on `enter` are not pressed
    (they were pressed for something else). It keeps the public header as it is, so the build stays a
    drop-in for the system's soname. A build records `AQ_BUILD` (`7bb8bdf4+keys`); `setup` says when
    the private build is older, and `setup --aquamarine` rebuilds it. Boxes on the system's aquamarine
    keep the bug until a release has the fix (UPSTREAM.md; the PR text is prepared). Test:
    `t_held_keys` (fails on the old build).
133. **setup and the widget across installs and upgrades** (2026-10-01). Three gaps a package makes
    likely. (1) A `~/.local/bin/omabox` linked by a checkout's setup stays when the user moves to the
    package; setup under `/usr` never looked at it. Omarchy's `env-bootstrap` appends `~/.local/bin`
    after `/usr/bin`, so the package's runs, but a PATH with it first (the user's own rc) runs the
    checkout, the widget's `omabox` too. A system install's setup now says what the file is (a link to
    where, gone, or a file of its own) and whether PATH finds it first, and offers to remove a link
    (Y/n, in a terminal; "not asked" otherwise). (2) The widget was linked but left off; setup now asks
    whether to put it in the bar, as `omarchy plugin add` does: `omarchy-shell shell rescanPlugins`,
    then `omarchy plugin enable` until the shell knows it (the rescan returns before it is done). A
    "no" is remembered in `~/.config/omabox/widget-declined`, as the guard's is. (3) A shell does not
    reload a plugin when its files change (finding 41), so after an upgrade the bar runs the old widget
    against the new CLI. `config --json` now names the CLI's version; the widget compares it with its
    own `pluginVersion` and says, in its alert strip, to restart the shell (dismissable, once per
    version). Only widgets from this version on can say it. Tests: `t_setup_prompts` answers through
    `script(1)` in a box (the widget in the box's bar, a "no" remembered; under `--installed`, the old
    link removed), `t_widget` (the note), and `omarchy-plugin-validate` on the widget in
    `t_unit_version`.
134. **`up` on a running box refuses options it lacks; `down` frees the host's submap** (2026-10-01).
    (1) `up NAME --plugin X` on a box without X said "options ignored" and exited 0, so an agent went
    on without what it asked for. Now the options given are compared with `box.json` (mode, size, net,
    allow, systemd, stock bar, Xwayland, no-shell, Hyprland build, save, plugins mounted) and any it
    lacks are refused, exit 1, naming each and what to do; a bare `up`, or one asking for what the box
    has, is fine. Mounts, `--env` and `--idle` are not compared. `box.json` records `xwayland` and
    `shell` for that. `up --json` prints the box as `ls --json` lists it. (2) A host config reload
    drops passthrough's hooks and the submap's binds but keeps the submap (seen in a stand-in host:
    "omabox" with `omabox_pass_version` nil, SUPER+1 dead); a box's reaper puts the hooks back within 2
    s, but with the last interactive box going at that moment nothing would, and every bind of the
    user's would stay dead. `down` of an interactive box now resets the host's submap when it is
    "omabox" and no interactive box is left (`t_submap_release`: reaper killed, reload, down, SUPER+1
    works again).
135. **`up --omarchy DIR`, and the host's dev link kept out of boxes** (2026-10-01). Omarchy's
    `env-bootstrap` sources `/etc/omarchy.conf` (written by `omarchy dev link`) in every bash: the box
    HOME's `.bashrc` (from `/etc/skel`) included. Boxes bind the host's `/etc`, so on a dev-linked host
    a box terminal set `OMARCHY_PATH` to the host's checkout and put its `bin` first, while the session
    ran the packaged Omarchy: seen in a box with a nested overlay `/etc` standing in for a dev link
    (`omarchy-version` came from the fake checkout). Not seen on a real dev-linked host (none here;
    `omarchy dev link` needs sudo). Every box now has its own `omarchy.conf` (`$D/omarchy.conf`): bound
    over the host's when there is one, naming the packaged Omarchy or `--omarchy`'s tree. `up --omarchy
    DIR` runs a tree (it must have `bin/`, `default/hypr/bootstrap.lua`, `shell/shell.qml`;
    `/usr/share/omarchy` itself is the default): mounted read-only at its own path (refused as
    `--ro-bind`'s are; a jailed agent's, as `--hyprland`'s); `OMABOX_OMARCHY` makes `start-hyprland.sh`
    set `OMARCHY_PATH` to it (Hyprland's config, then the shell and everything Hyprland starts),
    `session.sh` put its `bin` after omabox's stand-ins and source its uwsm defaults; `--stock-bar` and
    the desktop-entry fallback read its `config`/`applications`. With no host `omarchy.conf` to bind
    over (bwrap cannot add a file to a read-only bind), `/etc` is a read-only overlay with the box's
    file on top (`--overlay-src /etc --overlay-src $D/etc --ro-overlay /etc`; the last source is the
    top layer, checked). `box.json`, `ls --json` and `up_already` know it; `run` passes it to a
    throwaway box. Left as on the host's dev link: files Omarchy installs outside its tree (`/etc`,
    units, `/etc/skel`). `t_omarchy_tree` (a copy of the installed tree with markers; a box inside it
    without `--omarchy` runs the installed one, through the bind over the outer box's file).
136. **An interactive box's window under another tool's host rule** (2026-10-01). A tool that nests
    Hyprland can add a runtime rule to the host for every `aquamarine` window (float, no focus, a
    workspace of its own), and aquamarine names every nested window so: it catches ours too.
    Tried in a stand-in host: our exec rule's workspace (9) and no-focus won, but the window
    floated. The exec rule now says `float = false` too: tiled on its workspace whatever such a rule
    says (`t_submap_release` checks it under such a rule).
Findings 137 on started from reading Tom Ballard's Omarchy plugin and app projects
(github.com/tcballard, MIT and Apache-2.0); each was checked in boxes first, and no code was taken
from them.
137. **`omarchy-version` in a box** (2026-10-01). Omarchy's asks pacman (`pacman -Q omarchy-dev`, then
    `omarchy`), and a box binds no `/var`, so it printed nothing and exited 1 (checked in a box: what an
    agent quoting the tested version hits first). A stand-in in `share/bin` (first on every box PATH,
    a terminal's bash included) prints `OMABOX_OMARCHY_VERSION`, which `up` reads on the host the same
    way (inside a box, the box's own); with `--omarchy`, the tree's own script (its git commit). The
    pacman database stays out of boxes. `t_uwsm_app` (run and a terminal's bash).
## Dead ends (kept so we don't retry them; probes in `spike/dead-ends/`)

- Headless output inside the real Hyprland: shares seat/focus with the user; black-output bugs.
- Docker: user not in `docker` group (= root); image drift vs host packages.
- Qt offscreen: Qt-only, no shell/tray, clicks don't deliver.
- sway / weston / cage as the parent compositor: protocol versions (finding 2).
- Keeping labwc's lazy Xwayland off the host's abstract X11 sockets without a network namespace
  (finding 89): `WLR_XWAYLAND=/bin/false` or a missing path still binds the sockets, and host X11
  clients then hang or fail; copying the host's `/tmp/.X*-lock` files into the box does not hold
  them, since their pids look stale from the box's pid namespace.

The spike installed sway, weston and cage to try them as the parent compositor; omabox needs none
of them, nor wayvnc (finding 34). It needs labwc.

## Open

Bugs and ideas live in the GitHub issues. Known gaps:

- `test/run.sh` ends with a bare `main "$@"`: after main returns, bash reads the file on from where
  it was, so an edit that grew the file during a run could run what now sits there (the call again).
  `bin/omabox` ends `main "$@"; exit`; here any `exit` at the end makes shellcheck 0.11 take every
  test function for dead code (SC2329, and the SC2086 it then no longer rules out), so it is left
  as it was (finding 124). Until then, do not edit the suite while it runs.

- The aquamarine build step goes once Arch ships a release with #415 (`UPSTREAM.md`). A system
  aquamarine patched downstream (#48) cannot be told from its version: it would still be refused for
  headless NVIDIA boxes and confirm-close until omabox learns how to recognise it (finding 125).
- More of the box's stack from a local build, per box, as `--hyprland` does (finding 116, issue #44):
  `--quickshell PATH` (a shell or Quickshell change); `--omarchy DIR` is done (finding 135); `--lib DIR` (library dirs ahead of the system's, for
  hyprutils/aquamarine/hyprlang work; `/opt/omabox/lib` comes first now). A `--hyprland` build whose
  RUNPATH points outside its own folder finds those libraries on the host (`ldd` passes) but not in
  the box.
- Portals (finding 12): the file chooser (xdg-desktop-portal-gtk) is checked; other portals, and
  `QT_QPA_PLATFORM` apps with file choosers, are untested in a box.
- AMD and Intel iGPUs and one NVIDIA RTX 4070 SUPER tested; other NVIDIA cards, multi-GPU and other
  user setups remain open.
- Marks (finding 85) are for peek only: an interactive box is not marked (no window of ours to draw
  in; a host overlay would touch the real desktop). A peek that starts while a `click` is deciding
  whether to write can miss marks until the next peek (the file removed under it); not seen.
- Findings 80-86 never ran on NVIDIA (the NVIDIA card here is bound to vfio). They should hold on the
  NVIDIA screen (`WAYLAND-1`, finding 77): they read the screen from `hyprctl monitors` (the first one
  that is enabled), not by name, so shots, `--in`, marks and `mode` do not care what it is called.
  `tools/still` binds the one `wl_output` a box offers, and screencopy already works there for
  `shot`. Assumed, not seen: that `grim -T` (ext-image-copy-capture) and screencopy with damage
  both work with NVIDIA's renderer, and that labwc's headless parent sends the frame callbacks that
  let `wait` see a change. `t_main` skips its NVIDIA checks here, saying why.
- The window confirm-close opens for a box kept running (finding 70) has no `render_unfocused`, so
  `shot` gets no frame from it while it is hidden (finding 90). The host could give it one: a Lua
  `window.open` hook matching the box's client, then `set_prop` and a re-check. Untried.
