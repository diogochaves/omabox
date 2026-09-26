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
                                  box.json (options, pidns), info.json (bwrap child-pid), pid + pasta.pid
                                  (--net isolated), used (idle clock), launch.sh, box.log, reap.log
[systemd-run --user --scope]      --systemd only: a delegated cgroup the box's user manager owns (61)
[pasta --splice-only]             --net isolated only: own netns, loopback, the --allow ports (44, 45)
bwrap sandbox            fake HOME=/home/sbx, private /run/user/$UID and /tmp, pid/ipc/uts namespaces
│                        binds: /usr /etc /sys ro, the repo + ro-bind file + --ro-bind ro, mise installs ro,
│                        one render node (plus its NVIDIA render-side nodes on NVIDIA),
│                        share/ → /opt/omabox/share, patched aquamarine →
│                        /opt/omabox/lib, keyboard/pointer → /opt/omabox/bin, --plugin dirs ro →
│                        ~/.config/omarchy/plugins/<id>, --overlay dirs (discarded writes); every source
│                        and DEST checked (refuse_src/refuse_dest, finding 63)
└ share/session.sh       the session (bwrap's init is PID 1): private session bus (dbus-daemon, or
  │                      systemd's dbus.service with --systemd), gnome-keyring, PATH, env, then:
  ├ [systemd --user]     --systemd only
  └ labwc -S (headless)  invisible parent compositor (WLR_BACKENDS=headless, 1 output); ends with Hyprland
    └ Hyprland (nested)  real Omarchy config minus autostart; LD_LIBRARY_PATH → patched aquamarine
      ├ HEADLESS-2       screen on AMD/Intel; WAYLAND-1 bootstrap disabled
      │ WAYLAND-1        screen on NVIDIA; labwc's private headless output is resized to --size
      ├ quickshell       the Omarchy shell (bar, menu, tray host, notifications): share/shell.sh;
      │                  not with --no-shell
      ├ keyboard, pointer --hold: idle devices so focus changes work (41)
      └ apps             `omabox run [-d]` (nsenter into the namespaces, Hyprland's env)
interactive: no labwc; Hyprland nests in the host Hyprland through one fd (tools/wlfd, WAYLAND_SOCKET),
             launched by the host's hl.exec_cmd with rules → workspace 9 silent; WAYLAND-1 is the screen
```

Driving it from the host is `omabox` (`omabox help`): `hyprctl`, `grim` (shot), the keyboard and the
pointer all run inside the box's mount namespace (`nsenter -U -m`), so they only ever see the box's
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
2. Patched aquamarine into a private prefix (drop once Arch ships a release containing #415):
   ```
   git clone https://github.com/hyprwm/aquamarine build/aquamarine
   git -C build/aquamarine checkout 7bb8bdf4      # "wayland: fix configure not applying sometimes (#415)"
   cmake -S build/aquamarine -B build/aquamarine/out -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=$PWD/build/prefix
   cmake --build build/aquamarine/out && cmake --install build/aquamarine/out
   ```
   It must provide the soname the installed Hyprland links (`ldd $(command -v Hyprland)`); only the
   nested Hyprland loads it. `omabox up` checks this too.
3. Tools: `make -C tools/pointer`, `keyboard`, `wlfd`, `peek` (need `wayland-scanner`; protocol XML is
   vendored).
4. Links: `~/.local/bin/omabox` → `bin/omabox`; `skill/` as `skills/omabox` in `~/.agents` and
   `~/.claude` (and `~/.codex`, `~/.pi/agent`, `~/.hermes` when those exist); `plugin/` as
   `~/.config/omarchy/plugins/chaves.omabox`. A real directory where a link goes stops the install.
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
    headless boxes; interactive ones are for the user's eyes.
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
    with "Couldn't switch to pasta namespaces: Operation not permitted" (the `setns` in `ns_check`);
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
    map: Operation not permitted", the outer bwrap's bounding set lacks what it needs). Checked once on the
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
      A dev link (`/etc/omarchy.conf`) is not followed: a box always runs the packaged Omarchy.
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
      scratchpad starts fine while hidden (the host does not render it; `shot` needs it shown).
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
80. **A box per agent session** (2026-09-25). The default name was the repo's, so two agents in one
    checkout (two Claude Code windows, a Claude Code and a Codex) shared a box: one's `up` got the
    other's box with its options ignored, its `run` saw the repo read-only, and its `down` ended the
    other's work. Both agents already tell their shell commands who they are: Claude Code exports
    `CLAUDE_CODE_SESSION_ID` (a UUIDv4), Codex `CODEX_THREAD_ID` (a UUIDv7, checked in its rollout
    logs). The default name is now `<repo>-<last 8 of the id>`; the tail, because a UUIDv7 starts with
    a timestamp two sessions opened in the same minute share. `guard exec` sets `OMABOX_SESSION` for
    any other agent; set to empty, it turns the suffix off. `-b` and `OMABOX` are unchanged.
    Per-session boxes would pile up (~500 MB each) until their 2h idle limit, so `up` also records the
    agent's process, found without asking the agent: /proc/PID/environ is a process's environment as
    it was exec'd, and Claude Code and Codex set the variable for their children only, so the agent is
    the nearest ancestor whose environ lacks `VAR=value` (compared by value: a `claude` started from
    another session's shell carries the outer id). Under `guard exec` the variable is exported and the
    agent exec'd, so there it is the farthest ancestor with it. Its start time is recorded too, and
    the reaper takes the box down once that pid no longer has it (a reused pid is not the agent). An
    agent in a pid namespace of its own is not found and its box only expires when idle, as before.
    Interactive boxes and names given with `-b`/`OMABOX` are never tied to an agent. The reaper polls
    every idle/4 capped at 60 s, so a box can outlive its agent by up to a minute.

## Dead ends (kept so we don't retry them; probes in `spike/dead-ends/`)

- Headless output inside the real Hyprland: shares seat/focus with the user; black-output bugs.
- Docker: user not in `docker` group (= root); image drift vs host packages.
- Qt offscreen: Qt-only, no shell/tray, clicks don't deliver.
- sway / weston / cage as the parent compositor: protocol versions (finding 2).

The spike installed sway, weston and cage to try them as the parent compositor; omabox needs none
of them, nor wayvnc (finding 34). It needs labwc.

## Open

Bugs and ideas live in the GitHub issues. Known gaps:

- The aquamarine build step goes once Arch ships a release with #415 (`UPSTREAM.md`).
- Portals (finding 12): the file chooser (xdg-desktop-portal-gtk) is checked; other portals, and
  `QT_QPA_PLATFORM` apps with file choosers, are untested in a box.
- `omabox shot` of an interactive box while its window is visible is untested (it only fails fast
  when hidden).
- AMD and Intel iGPUs and one NVIDIA RTX 4070 SUPER tested; other NVIDIA cards, multi-GPU and other
  user setups remain open.
