# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

### Added

- **Agents inside [ai-jail](https://github.com/akitaonrails/ai-jail) drive boxes of their own**:
  `omabox broker on` (a systemd user socket) prints the lines to add to `~/.ai-jail`, and `omabox`
  in the jail then works as outside. A jail's boxes get no more than the jail: no network when it
  has none, only its project, only its own boxes, gone when it exits. No change to ai-jail
  ([#16](https://github.com/diogochaves/omabox/issues/16)).
- **Saves**: `omabox save SAVE` keeps a box's HOME (what its apps set up: signed in, a PIN, a
  library), and `up --from SAVE` / `run --from SAVE` start a box with it. Your Omarchy look is
  seeded on top as for any box. The box is paused for the copy, so a database is saved whole.
  `omabox saves` lists them (and which hold keyring secrets), `saves rm SAVE` deletes one. They live
  in `~/.local/share/omabox/saves`, private to you.
- **`omabox drag`**: press, move, hold, release, with `--window`/`--in` like `click` and `--shot` to
  see the drag while the button is down. `pointer --window` moves in a window's coordinates, and
  `down`/`up` default to the left button
  ([#25](https://github.com/diogochaves/omabox/issues/25)).
- **`run --env-file FILE`** hands a `dev.env`'s variables to the command, off the command line
  ([#30](https://github.com/diogochaves/omabox/issues/30)).
- **`shot --window SEL -g "X,Y WxH"`** crops a window in its own coordinates
  ([#27](https://github.com/diogochaves/omabox/issues/27)).
- **`up --hyprland PATH`** (and `run --hyprland PATH`) runs a Hyprland build of yours in the box
  instead of the installed one, to check a compositor change without installing it. A build linked
  against another aquamarine soname, or a file that is not an executable ELF, is refused before the
  box starts; `ls` and `windows` name the build, the box log has its version, and `up` warns when
  your `hyprctl` is another version ([#44](https://github.com/diogochaves/omabox/issues/44)).

### Changed

- `shot -o DIR/FILE` makes DIR when it is missing, and `-g` also takes `X,Y,W,H`
  ([#27](https://github.com/diogochaves/omabox/issues/27)).
- `omabox pointer --hold` is refused: it never returned, and read like "hold the button"
  ([#25](https://github.com/diogochaves/omabox/issues/25)).
- An unknown command is one line pointing at `omabox help`, not the whole help
  ([#36](https://github.com/diogochaves/omabox/issues/36)).

### Fixed

- **`-b NAME` goes before the command too**: `omabox -b NAME windows` failed with "unknown command:
  -b" and the whole help. It now means the same as after the command, for every command that takes
  `-b`; the others (`ls`, `config`, ...) say in one line that they take none
  ([#36](https://github.com/diogochaves/omabox/issues/36)).
- **`wait window SEL` is satisfied when several windows match** (an app with one window per vault or
  document): it failed with exit 2. Any match answers it and the line names them all; `--focused`
  when one of them has focus. `--window` on `shot`, `click`, `keys`, `pointer` and `drag` still
  wants exactly one ([#37](https://github.com/diogochaves/omabox/issues/37)).

## 0.2.1 — 2026-09-29

`OMABOX=NAME` works under `omabox run`, `--window` finds apps by name, and a Qt app's logging reaches
its `run -d` log.

### Changed

- **`--window nautilus` finds `org.gnome.Nautilus`**: a bare word also matches the last part of a
  reverse-DNS class, whole (`naut` does not)
  ([#20](https://github.com/diogochaves/omabox/issues/20)).

### Fixed

- **`OMABOX=NAME` works in commands `omabox run` started**: they got the default box instead
  ([#19](https://github.com/diogochaves/omabox/issues/19)).
- **A Qt app's warnings and errors reach its `run -d` log**: Qt logged to the journal, which a box
  has none of, so the log stayed empty ([#26](https://github.com/diogochaves/omabox/issues/26)).

## 0.2.0 — 2026-09-29

Agents drive apps by their windows and wait for the screen instead of sleeping; every box gets a
network of its own, and each agent session its own box.

Much of this release is Tyler South's ([@btsouth](https://github.com/btsouth)): the network of its
own, the box per agent session, hidden interactive boxes that can still be shot, and the guard
keeping links off your desktop. Thank you, Tyler
([#2](https://github.com/diogochaves/omabox/pull/2), [#7](https://github.com/diogochaves/omabox/pull/7),
[#8](https://github.com/diogochaves/omabox/pull/8), [#9](https://github.com/diogochaves/omabox/pull/9),
[#10](https://github.com/diogochaves/omabox/pull/10), [#11](https://github.com/diogochaves/omabox/pull/11),
[#12](https://github.com/diogochaves/omabox/pull/12)).

### Added

- **Windows as targets**: `omabox windows` lists the box's windows (where, on screen or covered);
  `shot --window SEL` captures one window's own pixels, covered or on another workspace too;
  `click --window SEL X Y` takes window coordinates and focuses the window first when needed;
  `keys --window SEL` focuses it, then types. A selector never guesses: none or several is exit 2.
- **`shot --fit N`** scales a shot down; `click --in SHOT X Y` and `pointer --in` take the pixels of
  a cropped or scaled shot, so nobody does the arithmetic.
- **`omabox wait`** instead of `sleep`: until the screen holds still (`still`) or changes (`change`),
  a window or a shell layer is there or gone, or a command in the box succeeds. `keys`, `click` and
  `run -d` take `--wait`: they return once what they caused has settled, and say so when nothing
  changed. Exit 0, 124 at the deadline, 1 when it cannot be seen (never 0). A blinking caret and the
  cursor are not changes. Costs nothing while the screen is idle (`tools/still`).
- `omabox keys --pass VAR` types one of your variables (a password) without putting it on a command
  line, where the process list would show it.
- The peek window shows what the agent does for a few seconds: a ring that follows its pointer and
  clicks, and captions of the keys it types (secrets as `*`). Drawn over the view only, never into the
  box's screen or its screenshots.
- Each agent session gets its own box. In Claude Code, Codex, or an agent started with
  `omabox guard exec`, the default box name ends with the session's id (`myrepo-5cc72cdc`), so two
  agents in one repo no longer share a box or take each other's down. A session's box goes down when
  its agent exits, `--idle 0` or not (unless it is in use then; a `run -d` job does not count),
  instead of waiting out the 2 hour idle limit. A session resumed in a new process
  (`claude --continue`) takes over its box if it is still up. `-b NAME` and `OMABOX=NAME` work as
  before; `OMABOX_SESSION=` (empty) turns this off. Thanks to [@btsouth](https://github.com/btsouth) ([#2](https://github.com/diogochaves/omabox/pull/2), [#11](https://github.com/diogochaves/omabox/pull/11)).

### Changed

- `shot --active` is `shot --window active`: the window's own pixels, not a crop of the screen. With
  no focused window it exits 2, as a selector that matches nothing.
- install.sh checks that grim can capture a window (`-T`, grim 1.5).
- `shot` is about twice as fast (PNG compression level 1; files ~20% larger).
- The agent skill has rules for driving an app (look, act once, look again; never resend input not
  seen to land; set state directly) and a symptom → next step table; the details that are not about
  safety moved to `skill/reference.md`.
- Every box has a network of its own (pasta). A default box (`--net connected`; `--net host` still
  works) reaches the internet, your LAN and your host's servers, and you reach its servers. Across
  the box boundary use `127.0.0.1` and a server that listens on IPv4 (`127.0.0.1`, `0.0.0.0` or
  `::`): from a box, `localhost` is reset when the server listens on IPv4 only, and from one box to
  another it never works. A box's ports appear on your host's `127.0.0.1` only (never your LAN
  address), usually within a second of its server listening; a TCP port there also takes the UDP
  port of the same number. A connected box started inside another box has no network. Thanks to
  [@btsouth](https://github.com/btsouth) ([#8](https://github.com/diogochaves/omabox/pull/8)).
- Every box needs `passt` (`./install.sh` installs it) and, unless it is `--net isolated`,
  `/dev/net/tun`; `omabox up` says so at once. Started from a process with no_new_privs (some agent
  sandboxes, a systemd unit with `NoNewPrivileges=`), a headless box has no network, only a loopback
  of its own, and `up` says so; a `--net isolated` one is refused there. A box that is already up
  can still be used from such a process.
- Boxes that were up before you updated still share your network, and can still catch host X11
  apps, until `omabox down` (`omabox ls` shows them as `host`).
- Take your boxes down before going back to an older omabox: it lists a box started by this one as
  dead (unless it is `--net isolated`), and its `down` can leave that box running.

### Fixed

- An interactive box now starts on a machine with two GPUs whatever GPU the desktop renders on. It
  failed with "bwrap did not start" when the desktop's GPU was not the first render node, or when
  `OMABOX_RENDER_NODE` named another GPU (which now applies to headless boxes only). A box that dies
  while starting says so, with Hyprland's last words, instead of "bwrap did not start".
- `omabox up` run from another user namespace (some agent sandboxes, `unshare -Ur`) no longer
  takes a box that is up for a dead one, clearing its dir and leaving it running out of reach. It
  stops and says it cannot tell.
- `omabox up` no longer exits silently when git has no global `user.email`, as on a fresh machine,
  thanks to [@btsouth](https://github.com/btsouth) ([#7](https://github.com/diogochaves/omabox/pull/7)).
- The agent guard now refuses to open links and files on your desktop. `xdg-open URL` or
  `gh pr view --web` from a guarded agent handed the URL to a browser already running there, which
  opened a tab and could take focus. `BROWSER` and `GH_BROWSER` point at a stand-in that fails with a
  note, and Claude Code and `guard exec` put it first on PATH as `xdg-open`. Under Codex only `gh`
  and what reads `$BROWSER` are covered: a plain `xdg-open` there still uses the desktop's URL
  handler. After updating, `install.sh` offers to update the guard for the agents that have it
  (`omabox guard on` does it too). Thanks to [@btsouth](https://github.com/btsouth) ([#10](https://github.com/diogochaves/omabox/pull/10)).
- `omabox shot`, `wait`, `click` and `keys` work on an interactive box while its window is hidden on its
  workspace. The host now keeps drawing the window (at `misc.render_unfocused_fps`), and the error
  for a box started by an older version no longer suggests showing the window, which led an agent
  to switch the user's workspace before every screenshot. After you closed a box's window and kept
  the box running (`confirm-close`), its new window is only drawn while on screen: `click` and
  `keys` still reach it, but `shot` and `wait` get no frame while it is hidden. Thanks to [@btsouth](https://github.com/btsouth) ([#9](https://github.com/diogochaves/omabox/pull/9)).
- A headless box no longer captures X11 apps started on the host. Its parent compositor's Xwayland
  claimed the host's abstract `:0` X11 socket (or the next free one), and a host app (Steam) then
  opened in the box. An X server run inside a box (Xvfb) could do the same. Thanks to
  [@btsouth](https://github.com/btsouth) ([#8](https://github.com/diogochaves/omabox/pull/8)).
- A check in the test suite (`t_throwaway`, "no box left") counted other boxes than its own, thanks
  to [@btsouth](https://github.com/btsouth) ([#12](https://github.com/diogochaves/omabox/pull/12)).

## 0.1.2 — 2026-09-26

Apps installed inside a box start from the launcher, and pull requests get a check.

### Fixed

- Apps installed per user inside a box (a `DBusActivatable` desktop entry and its D-Bus service in
  `~/.local/share`) now start from the launcher, as on the host. Boxes set `XDG_DATA_HOME`,
  `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` like an Omarchy session
  ([#4](https://github.com/diogochaves/omabox/issues/4)).
- A flaky check in the test suite (`t_widget`, "the viewer is started"), thanks to
  [@btsouth](https://github.com/btsouth) ([#3](https://github.com/diogochaves/omabox/pull/3)).

### Changed

- Pull requests run shellcheck in CI ([#6](https://github.com/diogochaves/omabox/pull/6)). The
  suite still needs a Hyprland session, so it stays a local step (CONTRIBUTING.md).

## 0.1.1 — 2026-09-25

NVIDIA GPUs, thanks to [@btsouth](https://github.com/btsouth) ([#1](https://github.com/diogochaves/omabox/pull/1)).

### Fixed

- Headless boxes now start and resize on NVIDIA GPUs with a render node, without exposing the host
  display or DRM card. A custom bar without a tray no longer holds startup at the tray check.
- On NVIDIA, startup checks for the screen resize helper (`wlr-randr`) before creating a box.

### Changed

- `install.sh` installs one more package, `wlr-randr` (NVIDIA boxes size their screen with it), so
  the next update may ask for sudo once. AMD and Intel boxes keep working without it.

## 0.1.0 — 2026-09-25

The first release.

### Added

- **Boxes**: `omabox up` starts a contained, invisible Hyprland + Omarchy desktop (Omarchy's
  Hyprland config and shell, your theme and bar) on a private screen and a private D-Bus session bus,
  in 3-4 s. `run`, `shot`, `keys`, `click`, `pointer`, `hyprctl`, `restart-shell`, `mode`, `gpu`,
  `env`, `path`, `down` and `ls` drive it.
- **Tests that touch the desktop**: `omabox run -- ctest ...` with no box up runs them in a throwaway
  box, the repo as a discarded overlay. `--pass VAR` hands the command one of your variables without
  putting it on a command line.
- **Box options**: `--size WxH[@HZ]|host`, `--plugin`, `--ro-bind DIR[:DEST]`, `--net isolated
  --allow PORTS` (no internet or LAN, only the listed host ports), `--env`, `--idle`, `--stock-bar`,
  `--systemd` (a real systemd user manager), `--xwayland`, `--no-shell`.
- **Seeing a box**: `omabox peek` (a live, view-only window of a headless box) and `omabox up
  --interactive` (a box as a window you use; SUPER+ALT+ESCAPE sends SUPER keys to it).
- **Settings**: `omabox config` for where windows open (`workspace`), `confirm-close` and the widget's
  `bar-icon`.
- **Bar widget** (`chaves.omabox`): the boxes on this machine, with Peek, Screenshot and Down, a New
  interactive box button and a Settings face.
- **Agent skill** for Claude Code, Codex, OpenCode, pi and Hermes, so agents do GUI work in a box.
- **Agent guard** (opt-in): `omabox guard on` gives agents' shell commands a display that does not
  exist, so a window opened by mistake fails instead of reaching your desktop; `omabox host -- CMD`
  for what you ask for on the real one.
- `install.sh` for a fresh Omarchy, with `--check`; `test/run.sh`, the regression suite.
