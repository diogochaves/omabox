# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## 0.4.4 — 2026-10-01

An agent inside ai-jail 2.6.2 drives boxes again, and each command has its own help.

### Added

- **`omabox help CMD` and `omabox CMD --help`** print one command's options and the notes about it,
  not the whole help (`omabox help shot` is about a twelfth of `omabox help`). The agent skill sends
  agents there, and tells them to check with text (`windows`, `wait`, `events`, `log --grep`) before
  taking a screenshot, and to take the smallest one that shows what they need.

### Fixed

- **The ai-jail broker works with ai-jail 2.6.2.** ai-jail 2.6.2 passes bwrap's options through a
  memfd (`bwrap --args`) to keep `--env` values off the process list, and the broker, which reads a
  jail's limits from bwrap's command line, refused every command from such a jail ("cannot read the
  jail's policy: unknown bwrap option --args"). It now reads them from ai-jail's memfd. Older
  ai-jail versions work as before.

## 0.4.3 — 2026-10-01

Plugin and app work in a box: `up --plugin` and `restart-shell` say why a plugin is not in the bar,
`omabox ls --json` says what a box tested (Omarchy version, theme, each plugin's commit),
`omarchy-version` works in a box, and the agent skill maps plugin and app steps written for the real
desktop to boxes, with recipes.

### Added

- **The agent skill knows plugin and app work**: what to do in a box instead of linking a plugin into
  your plugins dir, editing your `shell.json` or running `omarchy plugin add` on your desktop (writes
  the guard does not stop: your bar would change at once), and recipes for testing a shell plugin
  (settings, placement, a vertical bar, data states, leftover processes, the install path), an app
  from the launcher, a theme switch, a demo video, and reviewing an Omarchy pull request. It also
  says how to report what a box showed, and to use `--net isolated` for anything that starts local
  servers (a connected box's server holds that port on your machine too).
- **`omabox up --plugin` and `restart-shell` say why a plugin is not in the bar**: Omarchy's plugin
  validator's message, or the shell's own (a refused manifest, a QML error), as a warning; the box
  still comes up. `omabox ls --json` and `up --json` have each plugin's state (`plugin_status`).
- **`omabox ls --json` says what a box tested**: the Omarchy version (`omarchy_version`), the box's
  current theme (`theme`) and each mounted plugin's git commit, `+dirty` with uncommitted changes.

### Fixed

- **`omarchy-version` works in a box**: it says the installed Omarchy's version (or, with `up
  --omarchy`, the tree's commit), where it printed nothing and exited 1.

## 0.4.2 — 2026-10-01

Omarchy's browser bind and its own shell restart work in a box, a box can run an Omarchy checkout
(`up --omarchy`), keys held when an interactive box loses focus are released (with omabox's aquamarine
build), `setup` offers the bar widget and tidies up after a checkout, and `up` refuses options a
running box lacks.

### Added

- **`omabox setup` offers to put the bar widget in your bar** (yes by default, a "no" remembered).
- **The widget says when it is older than the omabox installed** (after an upgrade, until the shell
  restarts), and `omabox config --json` names the version.
- **`omabox up --omarchy DIR`** runs a box on an Omarchy checkout instead of the installed Omarchy
  (its Hyprland config, shell, `bin/` and `OMARCHY_PATH`), as `omarchy dev link` would, without
  touching your system.
- **`omabox up --json`** prints the box as `omabox ls --json` lists it.

### Changed

- **`omabox up` on a box that is already up refuses options the box lacks** (exit 1, naming each),
  where it ignored them: an agent asking for a plugin, isolation or a size no longer goes on without.
  A bare `omabox up`, or one asking for what the box has, is fine as before.

### Fixed

- **Omarchy's browser bind (SUPER+SHIFT+B) opens the browser in a box**, and so do the other things
  Omarchy starts with `systemd-run --user` (LocalSend from the share menu). Without `--systemd`
  nothing opened; with it, the browser closed again at once.
- **A key held when an interactive box loses focus no longer stays down in it** (SUPER held while
  SUPER+1 switched your workspace made a later W in the box SUPER+W), with omabox's aquamarine build:
  `omabox setup --aquamarine` builds it, or rebuilds an older one (`omabox setup` says when).
- **`omabox setup` from a package notices an `omabox` link left in `~/.local/bin` by a checkout**,
  says whether it takes over, and offers to remove it.
- **Your Hyprland binds are never left dead** by an interactive box going down right after a config
  reload (the host could stay in omabox's key-passing mode with no way out).
- **A box no longer follows your own `omarchy dev link`** in its terminals (they took the host's
  checkout while the rest of the box ran the installed Omarchy).
- **An interactive box's window stays tiled on its workspace** when another tool's rule floats
  every nested Hyprland window.
- **`omarchy restart shell` in a box brings the bar back.** It left the box with no bar (no journald
  for `systemd-cat`); it and `omabox restart-shell` now take turns cleanly.

## 0.4.1 — 2026-10-01

Fixes from more testing of the package for Omarchy's repository: a clear message instead of a bare
error for an account Omarchy has not set a theme for, no empty directories left by
`setup --remove`, and a test suite that copes with an installed omabox on NVIDIA.

### Fixed

- **`omabox up` for someone Omarchy has not set a theme for yet** (an account that never logged in)
  now says so and what to do, before making anything. It died on a bare `cp: cannot stat` and left
  the box's directories behind.
- **`omabox setup --remove` leaves no empty directories** of omabox's behind.
- **The tools link only what they use** when built with a distribution's flags (`libm` stayed
  linked, unused, in three of them).
- **The test suite says once when no box can start here** (a headless box on NVIDIA without
  aquamarine's fix), instead of failing every box test, and finds your aquamarine build from an
  installed omabox in the tests that start a box inside a box or from a save.

## 0.4.0 — 2026-10-01

omabox is ready to be packaged: it runs read-only from `/usr/lib/omabox`, `omabox setup` does what
each user needs (and `--remove` undoes it), and boxes run on your system's aquamarine unless a box
needs the fix.

### Added

- **`omabox setup`** makes what each user needs: the links (omabox, the agent skill, the bar
  widget), the settings dir, and the agent guard question. `install.sh` runs it; a package's users
  will run it themselves. **`omabox setup --remove`** undoes it before you delete omabox: guard and
  broker off, widget disabled, links removed; your settings and saves stay
  ([#49](https://github.com/diogochaves/omabox/issues/49)).
- **omabox runs from a system install**, read-only at `/usr/lib/omabox` with `/usr/bin/omabox`, as a
  package installs it ([#50](https://github.com/diogochaves/omabox/issues/50)). Checked in a fresh
  Omarchy VM: built in a clean chroot, installed, a box driven, removed with nothing left behind.
  `test/run.sh --installed` checks it on a checkout, and the suite run from an install skips by
  itself what only a checkout has.

### Changed

- **omabox runs on your system's aquamarine unless a box needs the fix** (Hyprland's backend
  library; [#47](https://github.com/diogochaves/omabox/issues/47)). Only headless boxes on an NVIDIA
  GPU and confirm-close need [aquamarine PR #415](https://github.com/hyprwm/aquamarine/pull/415)
  until a release ships it. `omabox setup --aquamarine` builds it (`install.sh` runs it, into the
  checkout's `build/prefix` as before); without it, `up` refuses those two and says what to run, and
  confirm-close from your settings stays off for the box. A private build whose soname Hyprland no
  longer links (after an aquamarine upgrade) is skipped for the system's instead of stopping `up`.
  `omabox --version` says which aquamarine new boxes use, `omabox ls --json` each box's.

### Fixed

- **`omabox shot -o` to a path it cannot write says so**: it reported a failed capture instead
  ("no frame ... ask the user" on an interactive box, "grim failed" on a headless one)
  ([#63](https://github.com/diogochaves/omabox/issues/63)).
- **Closing an interactive box whose window could not come back now ends it**: with confirm-close
  on, a box that could not open its new window ran on with no window at all. It now ends after 5 s.
- **The bar widget's "Confirm before closing" switch says when it cannot work**: on an aquamarine
  without the fix, it read on while no box asked. It is now off and greyed, its caption saying to run
  `omabox setup --aquamarine`.

## 0.3.2 — 2026-09-30

The agent guard steps aside once omabox is gone, and Codex's agents now get its note too.
`omabox ls` shows every box even while one goes down, and the test suite runs in about 1.5 minutes.

### Fixed

- **The agent guard no longer outlives omabox**: deleting omabox without `omabox guard off` left
  every Claude Code session without a display, and nothing to turn that off with. The guard's hook
  now applies nothing when omabox is gone, and says so. Codex's guard gains a hook of its own
  (Codex asks you to trust it once, `/hooks`): it gives Codex's agents the guard's note, and once
  omabox is gone it tells them which lines of `~/.codex/config.toml` to delete. `install.sh` offers
  the update ([#51](https://github.com/diogochaves/omabox/issues/51)).
- `omabox ls` no longer stops partway through the list when a box goes down while it lists them.
- **Going to your own box during a test run no longer fails it**: `test/run.sh` failed with
  "omabox's workspace 9 came up" when you switched to workspace 9 for your interactive box (or an
  app of yours there). omabox's workspace coming up is now a note when the focus it brings is on a
  window that is not the suite's, and still a failure otherwise. With a special workspace in
  `omabox config workspace`, the suite now watches that one too
  ([#56](https://github.com/diogochaves/omabox/issues/56)).

### Changed

- **The test suite takes about 1.5 minutes instead of 9**: `test/run.sh` runs box tests side by
  side, as many as half your CPUs (at most 8); `-j N` picks the number and `-j 1` runs them one at a
  time, as before ([#60](https://github.com/diogochaves/omabox/issues/60)).

## 0.3.1 — 2026-09-30

With keys-to-box on, pointing away from the box (your bar, another monitor) gives SUPER keys back to
your desktop.

### Changed

- **keys-to-box: the pointer decides too** ([#55](https://github.com/diogochaves/omabox/issues/55)).
  SUPER keys go to the box while its window has focus and the pointer is over it. Move the pointer
  off it (onto your bar, an empty part of the workspace, another monitor) and they are your
  desktop's again, keys-to-box still on; back over the box, they are the box's. The first key you
  press with the pointer off the box is already yours: SUPER+1 switches your workspace at once. The
  red border and the widget's icon follow. A running desktop picks the new rule up at the next
  `omabox up --interactive` or `omabox keys-to-box`.

## 0.3.0 — 2026-09-30

Agents see inside a box (`omabox lua`, `log`, `events`), restart an app in one step
(`run -d --replace`), move the pointer the way a hand does (`--steps`) and click with modifiers held
(`--mod`). Interactive boxes get SUPER keys that follow focus and your clipboard on demand, and
`--hyprland` runs a Hyprland build of yours in a box. Releases now carry a source tarball and its
checksum.

### Added

- **SUPER keys that follow focus into an interactive box**: `omabox keys-to-box -b NAME on` (or the
  keyboard button, or `f`, in the widget) sends SUPER keys to the box whenever its window has focus,
  and gives them back to your desktop when focus goes elsewhere, with no key to press. Per box, off
  by default, until the box goes down; SUPER+ALT+ESCAPE still passes them once, and in this mode is
  the way out until the box loses focus. While keys go to a box, its window's border turns the
  theme's red and the widget's icon lights up. `ls` shows the mode. A config reload of your Hyprland
  no longer leaves passthrough stuck on with nothing bound: the box puts it back within 2 s
  ([#22](https://github.com/diogochaves/omabox/issues/22)).
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
- **`run -d --replace -- CMD`** restarts an app after a rebuild in one step: it stops what `run -d`
  started in the box with the same command (SIGKILL if it ignores SIGTERM for 5 s), waits until its
  windows are gone, then starts it again. Nothing else in the box is touched
  ([#29](https://github.com/diogochaves/omabox/issues/29)).
- **`run -d -q`** drops the "started in box" line, and **`run -d --print-log`** prints only the
  log's path, on stdout, for scripts ([#42](https://github.com/diogochaves/omabox/issues/42)).
- **A pointer that travels**: `click --steps N` and `pointer --steps N -- move X Y` (or `move X Y
  --steps N`) move there in N steps from where the pointer is, so what lies on the way is hovered
  and, under Omarchy's focus-follows-mouse, takes focus, as with a real mouse. `click` and `move`
  still jump by default ([#38](https://github.com/diogochaves/omabox/issues/38)).
- **Modifier clicks**: `click --mod ctrl` (shift, alt, super, altgr; `ctrl+shift`), and `--mod` on
  `drag` and `pointer`, hold modifiers down across the click, then let go, however omabox ends
  ([#25](https://github.com/diogochaves/omabox/issues/25)).
- The skill says the pointer's position is part of what a test sets up: where it starts, that
  `click` jumps, how to travel ([#43](https://github.com/diogochaves/omabox/issues/43)).
- **`omabox lua EXPR`** evaluates Lua in the box's Hyprland and prints what it returns (tables and
  Hyprland's objects as JSON), where `hyprctl eval` says only `ok`
  ([#40](https://github.com/diogochaves/omabox/issues/40)).
- **`omabox log`** prints or follows (`-f`) a box's logs: Hyprland's by default, the shell's, the
  apps', the latest `run -d`'s and more, `--grep RE`, `-n N`, a box that died too. `omabox path
  --logs` says where each one is ([#41](https://github.com/diogochaves/omabox/issues/41)).
- **`omabox events`**: every box records its Hyprland events (`activewindow`, `urgent`,
  `openlayer`, ...) from its start, timestamped. `--mark` says "from here" without clearing anything,
  `--since MARK` (or `30s`) reads from there, `--grep`, `--json`, `-f`, and `--until RE` waits for an
  event like `omabox wait` does ([#39](https://github.com/diogochaves/omabox/issues/39)).
- **`omabox clip`** hands your clipboard's item (text, or an image by its type) to an interactive
  box, once, and `clip --from-box` hands the box's back: for a password or a URL while you drive a
  box. With no `-b` it takes the box whose window has focus, so a key binding of yours pastes into
  the box you are in; the widget has **Paste your clipboard into the box** and **Copy the box's
  clipboard out** on an interactive box's row (`v`, `c`). Nothing keeps watching either clipboard; a
  password manager's "sensitive" mark goes along. Never for agents: refused in their sessions, under
  the guard, in ai-jail, and for headless boxes ([#23](https://github.com/diogochaves/omabox/issues/23)).
- **Release files**: each GitHub release carries `omabox-X.Y.Z.tar.gz` (the tag, without the demo
  media) and `SHA256SUMS`, made by `release.sh`, so a package can track omabox by them
  ([#52](https://github.com/diogochaves/omabox/issues/52)).

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
- An interactive box kept running after a close (confirm-close) comes back on the workspace it
  showed, with its windows, instead of a new, empty one
  ([#24](https://github.com/diogochaves/omabox/issues/24)).
- A box's bar shows workspace numbers when yours come from a plugin left out of the box: Omarchy's
  go where that plugin was, or after the menu when your bar has none
  ([#21](https://github.com/diogochaves/omabox/issues/21)).
- **Peeking at the test suite's boxes no longer fails it**: a peek you open from the bar widget (or
  `omabox peek`) while `test/run.sh` runs is noted as watched by you, and the checks it holds up
  (a box you watch is not reaped) are skipped, saying why. A peek the suite's own commands open, or
  one opened any other way, still fails it, and the message now names a peek of yours as a possible
  cause ([#45](https://github.com/diogochaves/omabox/issues/45)).
- **A box starts on an NVIDIA GPU just switched to its driver** (from `vfio-pci`, say): its
  `/dev/nvidiaN` did not exist yet and `up` stopped. omabox now creates it with NVIDIA's
  `nvidia-modprobe -c N` (no root needed), and says to run that when it cannot.

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
