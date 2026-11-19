# omabox: plain boxes and builds of your own

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for a box without Omarchy, the user's own desktop in a box, or your own Hyprland or Quickshell build.

Sections:

- Plain boxes
- A Hyprland build of your own
- A Quickshell build of your own

## Plain boxes

A box is Omarchy's desktop where Omarchy is installed, else **plain**: `up` says `desktop: plain, as
Omarchy is not installed here`, and `omabox ls` says `desktop: plain` under its line. Never tell the
user about a bar, a theme or Omarchy's menus in a plain box: it has none.

- `--desktop plain` is a box with nothing of Omarchy's: Hyprland on omabox's own small config
  (Hyprland's defaults, the machine's keyboard layout, SUPER+RETURN a terminal, SUPER+W closes), an
  empty `/usr/share/omarchy`, no shell, a HOME with git's name and email only. For an app or a
  compositor bug that must not depend on Omarchy. What needs Omarchy (`--plugin`, `--theme`,
  `--stock`, `--dnd`...) is refused. Without `--desktop` a box is plain where Omarchy is not
  installed (`up` says so) or when `omabox config desktop plain` is set. `--clean` works there: its
  sentinel is the Hyprland background colour (`#ff00ff`), not a wallpaper, and `check run --desktop
  plain` checks on such boxes.
- `--desktop desk` (#321) is the user's own desktop in a box: their Hyprland config, their shell,
  their look, at their HOME's path. `$HOME` in the box is the same string as yours, so what writes
  your path into files or settings finds the box's. It is still the fake HOME (`omabox path --home`,
  as any box's), never your files: only the repo, `--ro-bind`, `--seed` and the copy of your desktop
  bring any in. A path under your HOME in a command is the box HOME's there, not yours (`run` says
  so). A mount (`--ro-bind`, `--overlay`) goes inside that HOME, never at it or above it, over
  omabox's own files in it (`.config/omabox`, its logs), nor through a link in it, nor inside
  another mount there (a font folder, a plugin, the repo).
- **Its files from the host**: Read, Write and Edit them at their paths under `omabox path --home`
  (or through `omabox run`), never by following a link there: on the host, an absolute link in that
  folder leads to your real HOME's file, not the box's (`path --home` warns when it holds one;
  omabox's own links there are relative).
- **The copy**, made at `up` before `--seed`'s (which win): your `~/.config/hypr` (links followed,
  stow and home-manager included, as copies), the config and state of each desk shell found
  (`--shell CMD`, what your `hyprland.lua` starts, or its unit linked under
  `~/.config/systemd/user/{hyprland-session,graphical-session}.target.wants/`), and the wallpapers
  they show: where a shell's profile says it keeps them (DMS's `wallpaperPath`, Caelestia's
  `wallpaper/path.txt`), and the pictures your config's `swww`, `swaybg` or hyprpaper names; nothing
  else a copied file names (a `~/Downloads` in a bind stays out). Never a secret store or what looks
  like a key; capped (a part stops at its cap, said in one line; the whole copy at 30 s); `up` says what
  it left out and box.json's `desk_copy` lists it, `shell_guess`/`shell_from` (and the box's
  `OMABOX_DESK_SHELL`) the shell found first. A shell's own idle timeouts are turned off in its copy
  where its profile says how (Caelestia). A write to the copy never reaches your files. A config the
  copy would refuse (a link into a secret store, a `hyprland.lua` or a module it requires holding a
  key's shape or over 1 MiB) fails `up` before any box is made.
- **Its base** is your desk's (box.json's `desk_base`): where Omarchy is installed and your
  `hypr/hyprland.lua` loads it, an Omarchy box (Omarchy's shell, with your plugins and `shell.json`
  as `--desk-shell` copies them, and your wallpaper: a `backgrounds/THEME` that is a link is
  followed and its pictures copied, no intro video; `--plugin`, `--theme`, `--theme-dir`, `--dnd`, `--no-dock` and
  `--omarchy` work, `--stock`, `--stock-bar` and `--keep` are refused: they are not your desk);
  otherwise plain. It needs a Lua config (`hypr/hyprland.lua`; a `hyprland.conf` alone is refused:
  `hyprctl eval`, which keys, click, pointer, mode, monitor and lua use, refuses it). Refused with
  `--desk-shell` (part of it), `--clean` (the path carries your login name) and inside ai-jail (not
  yet); `omabox config desktop` does not take `desk` yet, and `check run --desktop desk` is refused
  (a check needs the same box every run).
- **Its Hyprland** runs the box HOME's `~/.config/hypr/hyprland.lua`, after omabox's autostart
  policy: what the config starts (at load, in its `hyprland.start` handlers, through `hl.exec_cmd`,
  `os.execute` or `io.popen`) runs unless a box must not or cannot run it: idle and lock daemons,
  your systemd session and uwsm (unless `--systemd`), system-bus applets (a polkit agent with
  `--polkit`), what the box runs itself (keyring, PipeWire, portals), Omarchy's own session work.
  Binds run as on your desk. `omabox log autostart` is one line a decision: `PHASE VERDICT CLASS VIA
  CMD  (WHY)`, PHASE `load`, `start`, `reload` or `later` (a bind's Lua), VERDICT `run`, `drop` or
  `take` (the shell slot's). omabox's settings win over the config's: the screen and monitors, the
  red error bar, the cursor's buffer, autoreload, Xwayland; your look and input stay. An error in the
  config is said in `log autostart` and `log hyprland`, and the box still starts. Its idle is held
  off (an idle inhibitor, `tools/idlehold`), so your shell's own idle timeouts never lock or blank it
  between your steps; a shell's idle client that ignores inhibitors still goes idle.
- **Its shell**, on a plain base, is the one your config starts (`dms run`, `qs -c noctalia-shell`,
  `caelestia shell -d`, `waybar`, `qs -c NAME`, `qs -p DIR`; `env`, `setsid`, `uwsm app --`, `sh -c`
  around it are seen through), taken into the same slot as `--shell CMD`'s (`take` in `log
  autostart`): `up` waits for its layer and says `its shell: CMD (PROFILE, from your config)`, and
  `ls` (`shell: CMD (from your config)`; `ls --json`'s `shell_cmd` and `shell_from`), `pid shell`,
  `log shell`, `restart-shell` and the crash notes work on it. A second shell the config starts is
  dropped (one bar). When the config starts none but `up` found one (its systemd unit, say), the
  slot starts that (`guessed`). None at all: `up` fails at once, saying so; `--shell CMD` names one
  (it runs instead of the config's), `--no-shell` runs none. A shell found whose program the box
  does not have (a `dms.service` link left after dms went) is never started: `up` says `not the
  NAME shell (...): its program X is not installed`, box.json's `desk_copy.skipped` has `(shell
  NAME)`, and the box comes up with no shell, as with `--no-shell`.
- `--desktop plain --shell CMD` runs a shell of yours there (`--shell 'dms run'`, `--shell 'qs -p
  DIR'`; DIR outside the repo needs `--ro-bind DIR`). CMD may be a launcher that starts the shell
  and exits (`caelestia shell -d`, `qs -d`): the shell is all CMD starts, gone once nothing of it is
  left. `up` waits until one of its processes draws a layer, `omabox log shell` is its output,
  `restart-shell` stops all of it and starts CMD again, a crash is said as the shell's. An app the
  shell launches itself counts as the shell. `--quickshell PATH` makes the build CMD's `qs`. `dnd`,
  `dock` and `plugin` stay refused (they ask Omarchy's shell).
- A plain box has no bar, launcher, tray host, notification server or wallpaper but what its
  `--shell` runs: a tray or notification test needs a shell that has them (`omabox tray` says when
  the box has none; `restart-shell` and `log shell` need `--shell`). Omarchy's binds and menus are
  not there (nor its `omarchy-*` commands where Omarchy is not installed): start apps with `omabox
  run -d`, not a launcher. Everything else (run, shot, keys, click, tree, wait, record, monitors,
  `--net`, `--hyprland`) works as in an Omarchy box.

## A Hyprland build of your own

- `omabox up [NAME] --hyprland PATH` (and `run --hyprland PATH -- CMD` for a throwaway) starts PATH
  instead of `/usr/bin/Hyprland`. PATH's folder is mounted read-only at its own path, refused as
  `--ro-bind`'s are (a binary right in HOME or `/tmp`: build into a folder of its own). It runs in the
  box, never on the host. Build, never install (e.g. `cmake -B build && cmake --build build`, then `--hyprland build/Hyprland`: wherever
  the build put the binary; a wrapper script is refused, it must be the ELF itself).
- The box loads a private aquamarine from `/opt/omabox/lib` when omabox has one with the build's
  soname (`omabox --version` names it), else the system's; so the build must link a
  `libaquamarine.so.N` soname one of them has. `up` reads it (`readelf -d`) and refuses another one
  before the box starts, and refuses a build that needs any other library the box lacks (`ldd`).
  Build against the installed aquamarine, or `PKG_CONFIG_PATH=<private prefix>/lib/pkgconfig`.
- `hyprctl`, `hyprpm` and the rest stay the installed ones; `up` warns when the box's Hyprland
  version (as it says over IPC) differs from theirs: the IPC may not match then. A patched build of
  the installed version is fine.
- **Another version whole: `--hyprland DIR`.** DIR is an unpacked package (`usr/bin/Hyprland`, e.g.
  Arch's `hyprland` and the `aquamarine`, `hyprutils`, `hyprgraphics` it needs, each `bsdtar -xf` into
  DIR), an install prefix (`bin/Hyprland`, `lib/`) or a build folder (`Hyprland`, `hyprctl/hyprctl`).
  It is mounted whole; its libraries load first (its aquamarine is the box's: on NVIDIA it needs the
  #415 fix, aquamarine 0.16.0 or later), and its `hyprctl` is the box's `/usr/bin/hyprctl`, so no
  IPC warning. `OMABOX_HYPRLAND=DIR` makes it every `up`'s default (a whole suite on another
  Hyprland); `--hyprland` on the line wins.
- `omabox ls` shows the build under the box's line with `hyprctl version`'s first line (commit,
  dirty or clean), `ls --json` has `hyprland` (null for the installed one) and `hyprland_version`
  (every box's, the installed one's too), `omabox windows` starts with it, and `<box dir>/box.log`
  has that line as the box's Hyprland said it. A save records it, and `up --from` notes a different one. Check which binary runs:
  `omabox run -- sh -c 'readlink /proc/$(pgrep -x Hyprland)/exe'`.
- What a box proves: compositor logic runs for real on a virtual output with virtual input devices
  (layouts, focus, input routing, the Lua config, IPC, protocols). What it never runs: the DRM/KMS
  backend (modesetting, real monitors, HDR/VRR, multi-GPU), libinput with real devices, and the
  session and suspend paths, or a real password at the lock screen (the lock itself runs: "Testing
  the lock screen" in shell.md). Those stay the real desktop's (or a VM's with passthrough): say so.
- Inside ai-jail the build must be in the jail's project (or a folder the jail was given whole), one
  the jail hides no file in (`--mask`, its `.ai-jail`): the project itself still goes in then, those hidden.
- **A hang or a crash**: `omabox gdb` prints every thread's backtrace of the box's Hyprland, stopped
  or deadlocked too, and leaves it as it was (`-- -ex 'info threads'`: gdb's own commands). A crash:
  `omabox gdb --watch` before the step that crashes it; the box then goes down as it would have, and
  `omabox log gdb` has the fatal signal, the backtrace of the thread it hit, then every thread's
  (`log gdb --crash`: that thread alone; the box writes no core and no Hyprland crash report). `--shell` for the shell, `--pid PID` for any process of the box. A gdb run
  with `omabox run` is refused (`ptrace: Operation not permitted`). Symbols: what the binaries carry
  (a debug build has them); Arch's are on debuginfod, offline from the addresses.

## A Quickshell build of your own

- `omabox up [NAME] --quickshell PATH` (and `run --quickshell PATH -- CMD` for a throwaway) runs the
  box's shell on PATH instead of `/usr/bin/quickshell`: a Quickshell fix not in the installed
  package (e.g. `cmake -GNinja -B build && cmake --build build`, then `--quickshell
  build/src/quickshell`, the ELF itself, not a wrapper). Its folder is mounted read-only at its own
  path, refused as `--ro-bind`'s are; inside ai-jail, as `--hyprland`'s. Not with `--no-shell`.
- Quickshell uses Qt's private API, so the build must be one against the installed Qt: `up` refuses
  a build against another Qt minor (it reads the build's `qt_version_tag`) or needing a library the
  box lacks, before the box starts; another patch release ends the box as it starts ("died while
  starting (--quickshell: built against Qt X, the box has Qt Y ...)"). Rebuild against the
  installed Qt then. A shell that dies anyway is said with the build named: `omabox log shell`.
- In the box the build is `quickshell` and `qs` by name (Omarchy's `omarchy-shell` and `omarchy
  restart shell`, `omabox run -- qs ipc ...`), and `restart-shell` keeps it. Check which binary
  runs: `omabox run -- sh -c 'readlink /proc/$(cat $XDG_RUNTIME_DIR/omabox-shell.pid)/exe'`.
- `omabox ls` shows the build under the box's line, `ls --json` has `quickshell` (null for the
  installed one) and `quickshell_version` (`quickshell --version` in the box, every box's), `omabox
  windows` starts with it; a save records it and `up --from` notes a different one.
- **The version is not proof of a fix**: a build of an unreleased commit says the last release's
  number (a build with 0.3.2's fix says "Quickshell 0.3.1 (revision fae96f1..., ...)"). The revision
  in that line, when there is one, is the commit: check it contains the fix (`git merge-base
  --is-ancestor FIX REVISION` in the Quickshell checkout).
- Testing a shell plugin's accessibility tree: `omabox tree --layer NAMESPACE` (Shots, in shots.md), on
  the installed Quickshell from 0.3.2 or on a build.
