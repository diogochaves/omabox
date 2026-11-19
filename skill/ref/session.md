# omabox: mounts, HOME, network and lifetime

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for what a box mounts and seeds, its session, saves, ports and how long it lives.

Sections:

- Mounts, HOME and the session
- Network
- Box lifetime

## Mounts, HOME and the session

- In short: on Omarchy a box is a full Omarchy desktop (its Hyprland config, shell, theme, tray,
  notifications, keyring); without Omarchy it is plain (Plain boxes, in builds.md). Read-only in it are the
  repo (same path), `--ro-bind`, `--plugin` and `--theme-dir` dirs, mise's toolchains and the fonts;
  nothing else of the user's HOME. Mounting HOME, `~/.config/omarchy`, `/tmp`, secret stores or the
  runtime dir is refused: do not work around it. The box HOME is fake (`/home/sbx`, or your HOME's path in
  a desk box; `omabox path --home` on the host). The box's /run/user/$UID is `omabox path --run` on the host; the host's own
  is the real session's: never read app state there. Private session bus and keyring; audio: `up
  --audio`; a polkit agent: `up --polkit`; `/sys`: the host's.
- The repo you ran `omabox up` from is visible **read-only** at the same path (from a linked worktree
  or a submodule, the main checkout's `.git` too: git reads work there, writes fail), plus any dirs listed in
  `~/.config/omabox/ro-bind` or passed with `--ro-bind` (`DIR:DEST` for another path, e.g. testing
  path mapping with `--ro-bind ~/nas:/mnt/nas`), mise's toolchains (`omabox run` keeps your PATH, so
  node/python/uv are the host's), `--plugin` dirs and the user's fonts (below); nothing else of the
  user's HOME is mounted. A `--plugin` dir is at `~/.config/omarchy/plugins/ID` in the box, not at
  its own path, unless the box has that anyway (in the repo); `up` says so when it is elsewhere. A
  bound folder is the one that was there at `up`: deleted and made again
  on the host (a build that `rm -rf`s its output folder, another agent sharing it), the box keeps the
  old one, empty, and every command there succeeds (`run` says so once). Bind a folder you keep and
  remake the files inside it, not the folder; or `down`, then `up` again.
- A host path in `run`'s command that the box has elsewhere or not at all (a scratch file in the
  host's `/tmp`, `$(omabox path --home)/…`, a plugin's helper by its host path, a build dir in
  `LD_LIBRARY_PATH`) is said before it runs, with its path in the box or the mount that brings it in.
  The command still runs: give it the path the note names.
- **The user's fonts** (`~/.local/share/fonts`, `~/.fonts`) are mounted read-only at the same place in
  the box HOME, so a theme's or plugin's own faces draw as on the desk (`up` says it; `ls --json`:
  `desk_fonts`). Not in an `up --stock` box, nor an `up --clean` one (`--keep fonts` brings them), not with `up --no-seed
  fonts`, and not where the box HOME has its own folder (a `--from` save's, a `--seed` into it: kept,
  said). A test's own font: `--seed FILE:.local/share/fonts/NAME.ttf` (that folder is then the box's,
  without the desk's). A link in them that leads out of them leads nowhere in the box (said).
  Outside a repo a throwaway `run` mounts nothing of the current dir: the command runs in the box
  HOME, and `run` says so when a word of it is a path there ("note: tests/x.py is relative to DIR,
  which is in no repo, so not in this throwaway box"; a box up without that dir too): `run --overlay
  DIR` (writable, discarded) or `--ro-bind DIR` brings it in (for a box up: `down`, then `up` with it).
- A link in the repo that leads out of it (a build cache's `target/ -> ~/.cache/…`) dangles in the
  box: `up` names each at the top of the repo, `run` each argument through one ("outside the box,
  where the link dangles"), with the `--ro-bind TARGET` that mounts it (`down`, then `up` with it)
  when TARGET is a cache, outside HOME or in no hidden folder of it; else no mount is offered.
- **Copied from the desk into the box HOME at `up`** (copies: edits after `up` do not reach the box;
  hidden files and `node_modules` left out, links inside stay links), on top of a new Omarchy user's
  HOME (`/etc/skel`): `shell.json` (the bar's settings, with only built-in widgets and the mounted
  plugins; `--stock-bar`: Omarchy's default instead) and `shell.toml` (not with `--stock-bar`; `up`
  says when this bar is not Omarchy's: "seeded the bar from your desk: its shell.json layout and
  shell.toml [font], [bar] (--stock-bar …)", names only); the
  current theme, its name and background, and your wallpapers for it (`backgrounds/THEME/`; none of
  it with `--theme NAME`); your custom themes (`themes/*`); `branding/`; **`extensions/`**, where
  plugins keep their settings; the terminal configs (`~/.config/{foot,alacritty,ghostty,kitty}`, the
  `*xdg-terminals.list` choice); the Hyprland toggles (`~/.local/state/omarchy/toggles/hypr`); git's
  `user.name` and `user.email` only. Never `api-keys.env`, `hooks/`, keyrings, tokens or the rest of
  git's config. `--desk-shell` adds every plugin (but omabox's own widget), `shell.json` unfiltered and the other files beside
  it; `--seed SRC:DEST` adds what you name.
- **A plugin under test reads its settings from the desk's `extensions/`**
  (`~/.config/omarchy/extensions/NAME.json`), an on/off switch too: one off on the desk leaves its
  binds or widget off in the box, and the test fails with only a wait's timeout. `up` says each such
  file a `--plugin`'s code names (`seeded extensions/NAME.json from your desk: plugin ID reads it`);
  `up --no-seed extensions` starts without the desk's (a new user's, `/etc/skel`'s; `--no-seed
  fonts` without the user's fonts, which are mounted read-only), and a test
  that needs a setting seeds it (`--seed ./fixture.json:.config/omarchy/extensions/NAME.json`).
- Your custom themes (`~/.config/omarchy/themes/*`) are copied into the box HOME at `up`, a theme
  that is a link to a working copy too: edits after `up` do not reach the box, and from a git
  worktree that copy is the link's checkout (usually main), not yours; `up` notes it. `omabox up
  --theme-dir PATH` (repeatable) gives one live: the box's `themes/NAME` (PATH's name) links to PATH,
  which the box sees at its own path (inside the repo, a same-path `--ro-bind` or an `--overlay`;
  anywhere else it is mounted read-only there, refused as `--ro-bind`'s are). `omabox run -- omarchy-theme-set NAME` applies it as it is then;
  run it again after an edit. `--ro-bind` into the box HOME is refused, and `--seed` is a copy.
- A box starts on your desktop's current theme and background (a copy of your own wallpapers for
  it, from `~/.config/omarchy/backgrounds/THEME/`). `omabox up --theme NAME` starts it on NAME instead
  (Omarchy's, one of yours or a `--theme-dir` one; named as `omarchy-theme-set` takes it, `"Tokyo
  Night"` too): the session runs `omarchy-theme-set NAME` before Hyprland and the bar start, with the
  first background it finds (your own first for NAME, else the theme's). For tests whose result depends on the theme. It
  leaves your bar as it is (layout, `shell.toml`'s sizes and fonts): add `--stock-bar` for Omarchy's. An unknown NAME is refused
  before anything is made; with `--from SAVE` it wins over the save's look; `up` again with another
  is refused (box.json and `ls --json` have `theme`).
- The box HOME is `omabox path --home` (`<dir>/home`), readable and writable from the host: put outputs there
  or in `/tmp` inside, and seed a widget's data files (a usage record, a store) there while the box runs.
  Inside the box it is `/home/sbx`, which does not exist on the host: a path under it passed to a
  service running on the host (a download dir sent to a local server) fails there. Use a path both
  can see. A desk box's HOME is at your HOME's path: on the host that path is your real HOME,
  not the box's.
- The box's runtime dir (`/run/user/$UID` in it, its `XDG_RUNTIME_DIR`) is `omabox path --run` →
  `<dir>/run` on the host: an app's runtime state (a `status.json`, a socket) is read there. The host's
  own `/run/user/$UID` is the real session's: a file read there is the desktop's app state, not the box's.
- `/sys` and system-wide `/proc` files are the host's (read-only): CPU, temperatures, memory, disks,
  USB devices and DRM connectors read as the real machine's. A widget reading those shows the host's
  hardware state, not the box's.
- Every node in a box's `/dev` (`null`, `tty`, the render node, `/dev/nvidia*`) is a bind mount, so
  plain `lsof /dev/nvidia0` reads it as a file system and lists every `/dev` holder (`/dev/null`'s
  too). Who holds the GPU: `lsof -f -- /dev/nvidia0`, or `fuser -v /dev/nvidia0`. A box's own
  compositor and shell hold it; the desktop's holders are not visible from a box.
- The box has the host's programs, not more: an app the host does not have fails in the box too
  (`setsid: failed to execute alacritty`); Omarchy's terminal is `foot` or what the host has.
- The session's PATH is yours (mise's tools, as in `omabox run`) after omabox's stand-ins and the box
  HOME's `~/.local/bin`: drop a stub CLI there to fake one a plugin calls. What Hyprland starts (the
  bar, binds, terminals) has Omarchy's bin ahead of `~/.local/bin`, so a stub there answers for
  `wpctl`, `brightnessctl` or a plugin's CLI, never for an `omarchy-*` command. `--env KEY=VAL` on `up` sets a variable for
  the whole session (the bar included), e.g. a plugin's API base pointed at a stub; not one the
  session sets itself (PATH, HOME, XDG_RUNTIME_DIR, XDG_*_HOME, WAYLAND_DISPLAY, LD_PRELOAD...): refused.
- `omabox up --systemd` gives the box a real systemd user manager: `systemctl --user`, units in the box
  HOME's `~/.config/systemd/user`, `systemd-run --user` timers (for plugins that manage their own
  service or schedule alarms). No journald (`journalctl --user` is empty: what units print is in
  `omabox log journal`) and no logind either way.
- Saves: `omabox save NAME [-b BOX]` keeps the box HOME (app data, keyring; not `.cache` or logs);
  `omabox up --from NAME` / `run --from NAME -- CMD` start with it, the user's theme and bar seeded on
  top. Quit the app first for a clean save (the box is paused for the copy, so it is at least what a
  crash would leave). `omabox saves` lists them, `omabox saves rm NAME` deletes. Name saves after
  what they hold (`myapp-signed-in`); they stay until removed, so remove the ones you no longer need.
- No Xwayland unless `omabox up --xwayland` (its `DISPLAY` is `:0`): without it there is no X server
  at all, and an X client fails with "unable to open display", even one given `DISPLAY=:0`. Omarchy's `uwsm-app` launching always goes through a
  stand-in, `--systemd` or not: apps start as plain processes, not units (output in
  `<box dir>/home/apps.log`). Without `--systemd`, `systemd-run --user CMD` runs CMD the same way
  (`--wait`/`--pipe`/`--scope` in the foreground), but timers (`--on-active=...`) need `--systemd`;
  `systemd-cat -t ID` writes to `~/ID.log` (no journald in any box; `/usr/bin/systemd-cat`, by its full
  path, and other journal clients reach the box's own sockets at `/run/systemd/journal`: `omabox log
  journal`). `logger` and `logger --journald` reach them by name, but `logger -t omarchy-shell`
  (Omarchy's shell launcher) goes to `~/shell.log` (`omabox log shell`).
- The XDG base dirs are set as in a session (`XDG_DATA_HOME=/home/sbx/.local/share`, ...). An app
  installed into the box HOME the per-user way (`~/.local/share/applications`, a D-Bus service in
  `~/.local/share/dbus-1/services`) starts from the launcher and by D-Bus activation, as on the host.
  To test with another HOME (`env HOME=$(mktemp -d) app`), set the `XDG_*_HOME` vars too.
- Every process of the box's session has `OMABOX_BOX=<name>` in its environment (the bar, what
  Hyprland, `uwsm-app`, D-Bus or `--systemd` units start, `omabox run`'s commands), and `--env` cannot
  set it. On the host a box's processes are yours and look like the desktop's (its shell is
  `/usr/bin/quickshell -n -p /usr/share/omarchy/shell` too): `omabox which PID` prints the box a host
  pid is in (by its pid namespace, or an ancestor's for a sandbox inside the box; else by
  `OMABOX_BOX`, said so), or `not in a box` with exit 1 (another user's process too); exit 2 for a
  pid that is gone. `grep -z OMABOX_BOX /proc/PID/environ` says the same by hand.
- The other way: `omabox pid` prints the host PID of the box's Hyprland, `omabox pid shell` its
  shell's (for `/proc/PID/maps`: is a plugin's `.so` mapped; perf, strace), `--in-box` the PID as the
  box numbers it (`gdb --pid`, `omabox run -- kill`), `--json` both. Nothing on stdout and exit 1
  when it is not running: `P=$(omabox pid) || exit`, never a loop over `pgrep` and environ.
- What that does to the desktop's lock: Omarchy's stranded-lock recovery
  (`omarchy-hyprland-session-lock-stranded`, Omarchy 4.0.5 on) counts any `quickshell`, `qs` or
  `hyprlock` of your user as a live locker, a box's shell included. If the desktop's shell crashes
  while the screen is locked, the lock is not taken again while any box is up (any session's, an
  idle one for up to 2 h): the user is left at Hyprland's "lockscreen app died" screen until every
  box is down (`omabox ls`, `omabox down -b NAME`). It fails closed: nothing unlocks.
- `--no-shell` starts Hyprland only (no bar, tray or notifications): faster for plain app work.
- `--desktop plain` and `--shell CMD`: Plain boxes, in builds.md.
- A crash in a box leaves no core file and no crash notification on the user's desktop (the core
  limit is 1 byte). To get a core: `omabox run -- bash -c 'ulimit -c unlimited; exec ./app'`.
- Logs: `omabox log` prints them (below); `omabox path --logs` says where each file is.
- `run -d` jobs: each is its own session, with a log (`run-*.log`) named on stderr, as the host has it
  and as the box does (`/home/sbx/run-*.log`: the one to give a command in the box, `wait until --
  grep -q X /home/sbx/run-….log`); `-q` drops that
  line. Its stdout's first line is the job's pid alone, under `-q` too (`--print-log`: the host log
  path instead; `--wait --json`: the JSON's `job`), before a `--wait` line; nothing when nothing started. A job that
  ends in its first 0.3 s leaving nothing running is said instead (its status, its log's last lines),
  and `run -d` exits with its status: a server whose port is taken shows here, not as "started". One
  that ends later in its first 30 s with a status other than 0 is a warning on the box's next
  command, once (`run -d job CMD (pid N) exited with status 1 after 2.1s, since the last
  command`); not one you killed yourself with `omabox run -- pkill`/`kill`. A `run -d --wait` whose
  job died by the time the screen settled (on something else) is `unsatisfied`, said the same way,
  and exits with the job's status. `run -d --replace -- CMD` stops the jobs `run -d` started in the box with the same
  command and arguments (SIGTERM to the session, what it forked included; SIGKILL after 5 s), waits until their windows are
  gone, then starts CMD (`--wait` as usual). Only those: an app started another way (a bind, `run --
  setsid ...`, D-Bus activation) is not touched, even when the job only handed it its arguments (a
  single-instance app already running): stop that one yourself. Nor a job of the same program with
  other arguments (`--demo details`, then `--replace -- ./app --demo table`): it stays beside the new
  one, and `--replace` names it with its pid and the `omabox stop PID` that stops it. No earlier job
  (or one that exited): it just starts, and says so.
- **A job by its pid, never a host `pgrep`/`kill`** (the same program often runs on the user's
  desktop): `run -d` prints the job's pid alone on stdout, and says it on stderr ("started in box
  'B' (pid 115, log: …)"). Keep it: `J=$(omabox run -d -- ./app)`, then `omabox log run "$J"`,
  `wait log "$J" 'listening on'`, `wait job "$J"`, `stop "$J"`. `omabox jobs
  [--json]` lists the box's running jobs (PID, AGE, PROCS, LOG, COMMAND). `omabox stop PID...`
  (or `--all`) stops one as `--replace` does (SIGTERM to its session, SIGKILL after 5 s, its windows
  gone), through a pidfd of each of its processes; PID may be any pid of its session, a window's PID
  from `windows` too. A stopped job is not said as an early end. Another process of the box: `omabox
  run -- kill PID`. `omabox wait job [PID]` (none: the latest job) until its command exits,
  satisfied with how ("exited with status 3"), at once when it already had; `--gone` until nothing
  of its session runs either (a launcher's app). `omabox wait log [PID] RE [-i]` until a line of
  its log matches RE (grep -E; a line there already counts), satisfied with the line; a job that
  ended without one is `unknown` (exit 1). `omabox log run PID [--grep RE] [-n N]` reads that
  job's log however many `run -d` jobs came after it (a terminal, an `xdg-open`); a pid no job had
  is refused with the jobs there are. Not `wait until -- sh -c '! pgrep …'` (it matches
  itself) nor a loop over `run-*.log` (`ls -t … | head -1` picks the latest job's, not your app's).

## Network

- Each box has its own network namespace. By default it reaches the internet, the LAN and the user's
  servers on the host's 127.0.0.1: never pick a real local service or account a first-run screen
  offers; use a test one or ask. An app that talks to, starts or probes local servers: `omabox up
  --net isolated [--allow 8081]` reaches only those host ports.
  An `--allow` port is taken inside the box too (pasta listens on it there, leading to the host's):
  it is for reaching a host server from the box, and a box server on that port fails, "address in
  use" (`omabox ports` lists it as `allowed`). Serve on another port.
- The user may keep host ports from every connected box (`omabox config protect-ports`, theirs to
  set): a server there is refused from the box as if nothing listened, and `--allow` refuses such a
  port. Ask the user, never change the setting yourself.
- Across the box boundary use `127.0.0.1`, not `localhost`, with a server listening on IPv4
  (`127.0.0.1`, `0.0.0.0` or `::`): from a box, `localhost` is reset on an IPv4-only server, and
  between boxes always. A box's server is reachable from the host (and other boxes) on
  `127.0.0.1:PORT` within about a second of listening (poll for it); one on `::1` only is not,
  though its port there is still taken, and connections to it are reset.
  The host's servers are mirrored into each connected box, another box's forwarded ports among
  them, so a box server on a port the host or another box already has fails to start: "address in
  use" on a port nothing in the box uses means the host or another box holds it (`omabox ports`
  names the holder); use another port. Only servers started in two boxes within about a second of
  each other both run; then one reaches the host and the other does not. `omabox ports` lists every box's servers and what holds each port on
  the host (this box; this box but reset, for a server on `::1` only; several boxes, any of which
  may have it, naming those on `::1` only that would reset; a host process; nothing yet; an
  isolated box's servers are not forwarded). Check a server you started from
  inside its box (`omabox wait until -- curl -fsS http://127.0.0.1:PORT/`), never from the host.
  Inside a box the host's LAN address is the box itself. A connected box started inside a box has
  no network.
- A tool that discovers devices on the LAN (ARP, ping sweeps, mDNS) finds every address in a
  connected box (pasta answers for the whole subnet). Only when the user asks for it: `omabox up
  --net host` puts the box on the host's network itself (its own seat, session bus and HOME still).
  Its servers are on the host's real ports, colliding with the user's services and other host boxes
  (pick a free high port; `omabox ports` says `host-net`), it sees the real interfaces, and nothing
  keeps it off the user's servers. Never for untrusted code; refused with `--allow`, `--addr`,
  `--xwayland`, `--desktop desk`, inside ai-jail and while the user protects ports.
- An app that judges its network by where a peer comes from (a BitTorrent client reads
  "firewalled" while every peer is on 127.0.0.1): `omabox up --net isolated --addr 203.0.113.10
  --addr 203.0.113.11` puts those on the box's loopback; bind the server to one and the peer
  (client) to the other, and the server sees it come from there. Documentation ranges only
  (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32); `ip addr add` in the box is refused,
  so give them at `up`. The `--allow` ports still arrive on 127.0.0.1, not on these.

## Box lifetime

- Your box is named after the repo (or worktree) and your session, so no other session shares it;
  subagents share it (parallel ones each need their own: "Handing box work to a subagent" in agents.md).
  `omabox down` before `/clear`. A box of a subagent's own: `omabox up --new
  [--plugin …]`, all its options on that call, then `-b box-3` (as printed) on every later call, typed
  out (your shell keeps no variables); it outlives you: `omabox down -b box-3`. A box the user
  started: `-b NAME` (`omabox ls`). `omabox down` never with `--all`: it takes others' and the user's
  boxes. A box idle for 2h goes down by itself (`up --idle 0` keeps one).
- Your session's box goes down when your agent (Claude Code, Codex) exits. A peek window or an
  `omabox run` still going then keeps it until they end; a `run -d` job does not. After `/clear` the
  agent's process lives on and the old box only idles out.
