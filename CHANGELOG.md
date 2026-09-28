# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

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
  before; `OMABOX_SESSION=` (empty) turns this off.

### Changed

- `shot --active` is `shot --window active`: the window's own pixels, not a crop of the screen.
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
  port of the same number. A connected box started inside another box has no network.
- Every box needs `passt` (`./install.sh` installs it) and, unless it is `--net isolated`,
  `/dev/net/tun`. A headless box can no longer start from a process with no_new_privs (some agent
  sandboxes, a systemd unit with `NoNewPrivileges=`). `omabox up` says so at once in each case; a
  box that is already up can still be used from such a process.
- Boxes that were up before you updated still share your network, and can still catch host X11
  apps, until `omabox down` (`omabox ls` shows them as `host`).
- Take your boxes down before going back to an older omabox: it lists a box started by this one as
  dead (unless it is `--net isolated`), and its `down` can leave that box running.

### Fixed

- `omabox up` no longer exits silently when git has no global `user.email`, as on a fresh machine.
- The agent guard now refuses to open links and files on your desktop. `xdg-open URL` or
  `gh pr view --web` from a guarded agent handed the URL to a browser already running there, which
  opened a tab and could take focus. `BROWSER` and `GH_BROWSER` point at a stand-in that fails with a
  note, and Claude Code and `guard exec` put it first on PATH as `xdg-open`. Under Codex only `gh`
  and what reads `$BROWSER` are covered: a plain `xdg-open` there still uses the desktop's URL
  handler. After updating, `install.sh` offers to update the guard for the agents that have it
  (`omabox guard on` does it too).
- `omabox shot`, `click` and `keys` work on an interactive box while its window is hidden on its
  workspace. The host now keeps drawing the window (at `misc.render_unfocused_fps`), and the error
  for a box started by an older version no longer suggests showing the window, which led an agent
  to switch the user's workspace before every screenshot. After you closed a box's window and kept
  the box running (`confirm-close`), its new window is only drawn while on screen: `click` and
  `keys` still reach it, but `shot` gets no frame while it is hidden.
- A headless box no longer captures X11 apps started on the host. Its parent compositor's Xwayland
  claimed the host's abstract `:0` X11 socket (or the next free one), and a host app (Steam) then
  opened in the box. An X server run inside a box (Xvfb) could do the same.

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
