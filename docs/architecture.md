# How omabox works

The process tree of a box, the repo's layout and where a box keeps its files. Back to the
[README](../README.md).

```
[pasta]                        boxes behind pasta: connected (default), or isolated (--allow ports)
└ bwrap (pid/ipc/uts namespaces, fake HOME, private /run/user/$UID and /tmp)
  └ share/session.sh           the session: private bus (dbus-daemon), keyring, PATH, env
    ├ [systemd --user]         --systemd only (in its own delegated cgroup scope)
    ├ [pipewire, wireplumber]  --audio only (and pipewire-pulse): a null sink, a silent mic
    └ labwc, headless          invisible parent compositor
      └ Hyprland, nested       Omarchy's config, or a plain box's own; screen = a headless output of any size
        ├ quickshell           the Omarchy shell (not with --no-shell), or a plain box's --shell CMD
        └ your app             via `omabox run` (nsenter into the box)
```

In interactive mode, labwc is left out: Hyprland runs inside your Hyprland through a single
Wayland connection.

A box is Omarchy's or plain (`--desktop`; plain where Omarchy is not installed). The same
`share/hyprland.lua` runs in both: Omarchy's bootstrap, the box HOME's Hyprland modules and toggles
on Omarchy, `share/plain.lua` (Hyprland's defaults, your keyboard layout, a terminal and a close
bind) on plain, then what every box needs. A plain box sees an empty `/usr/share/omarchy`, and its
shell, if any, is the command `--shell` names. `--hyprland DIR` runs another Hyprland version from a
folder: its libraries first, its `hyprctl` bound over the box's.

| Path | What |
|---|---|
| `bin/omabox` | the CLI: a link to `build/omabox`, the binary `install.sh` builds from `crates/omabox` (Rust) |
| `crates/omabox` | omabox's source (a package builds it with cargo and installs the binary at `bin/omabox`) |
| `share/` | runs inside the box: session, Hyprland config (`hyprland.lua`, a plain box's `plain.lua`), shell launcher |
| `tools/pointer`, `tools/keyboard` | virtual pointer and keyboard; `wtype` sends the wrong keys under Hyprland |
| `tools/wlfd` | hands an interactive box its one connection to your compositor |
| `tools/peek` | the live view-only window (`omabox peek`) |
| `tools/still` | waits in a box until its screen holds still or changes (`omabox wait`, `--wait`) |
| `tools/burst` | takes a `shot --burst`'s frames through one capture client (Hyprland's screencast events once a burst) |
| `tools/tree` | reads a window's accessibility (AT-SPI) tree in a box, as text (`omabox tree`) |
| `tools/events` | records a box's Hyprland events, stamped, from its start (`omabox events`) |
| `tools/notices` | records what reaches a box's notification server, from its start (`omabox log notifications`, `wait notification`) |
| `tools/relay` | carries an omabox command from inside ai-jail to the broker outside (`omabox broker`) |
| `skill/` | the agent skill (Claude Code, Codex, OpenCode, pi, Hermes) that sends agents here |
| `skill-feedback/` | opt-in: the `omabox-feedback` skill, to report what omabox got wrong in your sessions ([CONTRIBUTING](../CONTRIBUTING.md#reporting-a-problem)) |
| `plugin/` | the bar widget |
| `test/run.sh` | the regression suite: real boxes, never the real desktop (`test/run.sh unit` is fast) |
| `NOTES.md` | design notes: architecture, findings (cited in the code as "finding N"), dead ends |
| `spike/` | the original proof of concept |

Boxes live in `$XDG_RUNTIME_DIR/omabox/<name>/` (`omabox path`): `box.json` (its options),
`info.json` (bwrap's pids; `pid` and `pasta.pid` for a box behind pasta), `run/` (the box's runtime
dir, its `/run/user/$UID`: `omabox path --run`; the host's own is your real session's), `used` (idle clock), `events.marks` (`omabox events --mark`), `reap.log`, `box.log`. The box's HOME (`home/`, `omabox path --home`, on disk in
`~/.cache/omabox/<name>/home`, removed on `down`) and logs are readable from the host.
