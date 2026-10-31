# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

### Added

- **A click, scroll or drag at the screen's X Y says a layer over the window there**, as `--window`
  does, when the layer certainly covers the point: a panel or menu over it is named on stderr before
  the input is sent (never refused). A layer spanning the screen (Omarchy 4.5's notifications
  column, its dock) is not said there, since it takes input on its items only
  ([#361](https://github.com/diogochaves/omabox/issues/361)).
- **`click --find TEXT`** clicks what the accessibility tree names TEXT, with the real pointer at the
  middle of what of it is in view, read fresh on every call: no `tree --find`, then working out the
  middle by hand. An exact name wins over a part of one, and a control over a label of its name;
  several in view is exit 2 with their lines, numbered for `--index N`. A hidden node, one with no
  bounds, one scrolled out of its container and a disabled one are refused, saying which, and nothing
  is clicked. `--window SEL` (none: the active window) or `--layer SEL` for the shell's layers
  ([#318](https://github.com/diogochaves/omabox/issues/318)).
- **`tree` names an unnamed control after its label** (LABELLED_BY), so `--find Email` finds the
  entry its label names, and reads a node once when it sits under two parents (Qt WebEngine)
  ([#318](https://github.com/diogochaves/omabox/issues/318)).

### Fixed

- **A click at the top of a fullscreen window was reported as on the bar**: `--wait`'s answer and
  `--json`'s `target` named a top layer (Omarchy's bar) over the point, `certain`, where a window in
  fullscreen covers it and takes the click. Overlays still count
  ([#409](https://github.com/diogochaves/omabox/issues/409)).
- **`tree --layer` could not read an Omarchy 4.5 bar popup**: the popup is a full-screen layer, the
  size of the wallpaper (and of 4.5's bar), so it refused with "which one is the layer's cannot be
  told". The popup is the one of them that takes the keyboard (on demand, not exclusively, which was
  all `tree` looked for), so its tree is read now, and the wallpaper's and the bar's say they publish
  nothing ([#315](https://github.com/diogochaves/omabox/issues/315)).
- **No "plugin X disabled: listed but not enabled" for a plugin that is running.** A service with a
  bar widget, loaded from its own `plugins[]` entry with its widget off the bar, now reads `loaded`
  with no warning. A widget in no layout of the box's shell.json while another mounted plugin runs
  (one that may place it through its own config and IPC) gets a note naming that plugin, not a
  warning (`plugin_status`: `off the bar`); with none, the warning says "not in the bar's layout"
  and gives the `putBarWidget` command that puts it there
  ([#326](https://github.com/diogochaves/omabox/issues/326)).
- **An `up` with another `XDG_RUNTIME_DIR` deleted the HOMEs of boxes still running** (a test's
  stand-in runtime dir, another login, with the same cache). A box HOME now records the box it was
  made for, and goes only once that box is gone where it was made and nothing uses the HOME; an
  older omabox's HOME only when nothing uses it. An `up` of a name whose HOME is another box's is
  refused instead of taking it (finding 584).
- **`tree` placed a Qt dock widget's contents relative to the dock**: Designer's filter field said
  `@4,4`, drawn at 4,88, so a click at its middle missed. They now have where they are drawn, as a
  Qt Quick dialog's contents do (finding 244, [#318](https://github.com/diogochaves/omabox/issues/318)).
- **`tree` printed no text for an editable combo box** (an input with suggestions in Chromium or Qt
  WebEngine), so a typed value could not be read, found by `--find` or seen in `--json`. It is now
  its `'text'`; a closed select still says its choice as `=value`
  ([#341](https://github.com/diogochaves/omabox/issues/341)).
- **`run`'s note "WORD is relative to DIR" fired on words that only share a folder's name**: from a
  checkout with a `stash` folder, `run -- omarchy-shell stash modules` was told to restart the box
  with `--overlay`. A word now counts only when it reads as a path (a `/` in it, the command itself,
  `cd`'s folder) ([#325](https://github.com/diogochaves/omabox/issues/325)).
- **Ctrl-D at one of `omabox setup`'s questions ended it silently** (exit 1). It is now a no, said
  so, and setup goes on; a Ctrl-D at the widget's question is not remembered as a no
  ([#373](https://github.com/diogochaves/omabox/issues/373)).
- **`config protect-ports` said nothing about boxes up already**: they keep reaching the ports they
  could reach at their start. Changing it now names the connected boxes that need a restart, and
  warns when `nft`, which the rule needs, is missing
  ([#374](https://github.com/diogochaves/omabox/issues/374)).
- **A `run -d` job stopped with `omabox run -- pkill` was still warned about when it exits with a
  code of its own** on SIGTERM (foot exits 230). A job that ends while a `run` that kills runs (or
  just after) is no longer said, whatever its status; the warning's hint now reads `(log: omabox
  log -b NAME run)` ([#329](https://github.com/diogochaves/omabox/issues/329)).
- **A foreground `run` and a plain `path` skipped the session's box**: run from another directory,
  `run` quietly started a throwaway box and `path` printed the folder of a box that was not there.
  Both now name the session's boxes, as `run -d` does; `omabox help` says how the box is picked
  without -b ([#327](https://github.com/diogochaves/omabox/issues/327)).
- **The agent guard refused `omarchy plugin list` naming `omarchy-shell`**, a command the agent never
  ran. When one of Omarchy's commands calls it, the guard now names that command, and for `omarchy
  plugin list` says where the installed plugins are
  ([#332](https://github.com/diogochaves/omabox/issues/332)).
- **A bound folder deleted and made again on the host showed empty in the box**, with every command
  succeeding. `run` now says so once, with what to do
  ([#391](https://github.com/diogochaves/omabox/issues/391)).
- **A box whose Hyprland crashed was sometimes reported as hung**: `omabox lua` (or any call to
  its Hyprland) said "did not answer (hung?)" when the box ended in the moment between two of its
  looks. It now says the box went down and how
  ([#351](https://github.com/diogochaves/omabox/issues/351)).
- **Inside ai-jail, a project folder without `.git` silently gave a box with no project**: `up` (and
  a throwaway `run`) started fine, the box had none of the folder's files, and nothing said why. It
  now says the folder is not a git repo and that `--ro-bind DIR` (or `--overlay`) brings it in
  ([#394](https://github.com/diogochaves/omabox/issues/394)).
- **A Hyprland notification looked missing from shots**: `hyprctl notify` (and Hyprland's own
  warnings, such as the monitor-layout one) draws at the top right of the focused monitor, so a shot
  of the screen's left never had it. `omabox hyprctl notify` now says where, with the `shot -g` of
  that corner, and the reference says it
  ([#359](https://github.com/diogochaves/omabox/issues/359)).
- **`omabox log` missed Hyprland's latest lines**: Hyprland writes `hyprland.log` 8 KiB at a time,
  so what a reload or a plugin just logged was often not in the file yet, and `log --grep` found
  nothing as if it never happened. `log hyprland` of a live box now adds the lines past the file's
  end from `hyprctl rollinglog`; when the file is further behind than rollinglog's last 4 KiB reach,
  it says so on stderr ([#328](https://github.com/diogochaves/omabox/issues/328)).
- **`up` on a GPU a box cannot render on said only "died while starting"**: on a VM's virtio-gpu
  without virgl the reason was only in `labwc.log`. It now says a box needs GPU rendering and that
  the node renders in software only (or has no EGL), how to give a VM 3D, and another GPU when the
  machine has one ([#396](https://github.com/diogochaves/omabox/issues/396)).
- **`down` signals the box's processes through pidfds**: the continue it sends the commands that
  entered the box, and the kill of its PID 1 and pasta, went by pid number after the check that
  they were the box's, so a pid reused in between (a stopped job of yours) could be continued or
  killed. Now each goes to the process that was checked, or to none
  ([#377](https://github.com/diogochaves/omabox/issues/377)).
- **A `down` by another omabox left the box's reaper running** (a package's `down` of a box a
  checkout started, or another worktree's) until its next look. The reaper now records its pid in
  the box's folder, and `down` ends it whichever omabox started it
  ([#383](https://github.com/diogochaves/omabox/issues/383)).

## 1.0.2 — 2026-10-30

Omarchy 4.5.0-rc2 support for `click`, `windows` and `tree --layer` (its bar now spans the screen),
and a batch of smaller fixes: `wait until` with a command the box cannot run, `tree` names past 80
characters, `check point` regions, back-to-back `run`s. Update if you run the 4.5 release candidates.

### Fixed

- **`check point -g` kept the region as typed**: `-g "010,010 400x300"` went into the checkpoint as
  `010,010`, so the same region typed another way (`10,10,400,300`) read as a different one. It is
  now kept as read, `10,10 400x300`, as `shot` does
  ([#384](https://github.com/diogochaves/omabox/issues/384)).
- **`shot -o x.ppm` wrote a PNG without a word**: shot writes PNG whatever the file is called. It
  now says so on stderr when the name is another image format's (`.ppm`, `.jpg`, ...), and `help
  shot` says how to convert one ([#346](https://github.com/diogochaves/omabox/issues/346)).
- **`check point --ignore` partly outside the shot was cut down without a word**: a region given in
  the wrong coordinates was kept as a sliver. It now says what was kept, and `check run --approve`'s
  "with regions left out" counts the checkpoints with `--ignore` regions too
  ([#342](https://github.com/diogochaves/omabox/issues/342)).
- **`tree --json` and `--find` saw names and text cut at 80 characters**: a status line whose end
  mattered could not be found or read. They now see them whole (up to 4000 characters); the text
  view still cuts them at 80, as `help tree` now says
  ([#314](https://github.com/diogochaves/omabox/issues/314)).
- **A box whose `used` file was removed never went down for idleness**, and a folder in the runtime
  dir that no box can be named made `ls` exit 2 with nothing listed. The reaper now counts the idle
  time from when it finds the file gone, and `ls` leaves such a folder out, saying so
  ([#371](https://github.com/diogochaves/omabox/issues/371)).
- **Back-to-back `run`s got slower**: each kept a marker file for 10 s, and every run read them
  all. Runs close together now share one, so thousands in a row cost the same each
  ([#392](https://github.com/diogochaves/omabox/issues/392)).
- **`wait until` with a command the box cannot run retried it to the timeout**, and never showed
  why. A command not found (127) or that cannot run (126) in the box now ends the wait at once
  ("unknown", exit 1); a wait that did not succeed shows the last lines of the command's stderr; and
  a command naming a host path the box cannot see (this box's folder, your `$TMPDIR`) is said before
  waiting ([#400](https://github.com/diogochaves/omabox/issues/400)).
- **On Omarchy 4.5.0-rc2 every `click` said it "may go to omarchy-bar"**: the bar's surface now
  spans the screen, taking input on its strip only. omabox now takes the bar as that strip, along
  the edge it reserves ([#407](https://github.com/diogochaves/omabox/issues/407)). And `tree
  --layer omarchy-bar` refused as ambiguous (the bar and the wallpaper, one size): two frames with
  nothing in either now read as a layer that publishes no accessible items, as one does.

## 1.0.1 — 2026-10-27

Fixes for what an upgrade from 0.x runs into, and a security fix: a box could take a port on your
`protect-ports` list while your own service on it was down (S10). Update.

**Upgrading from 0.x:** while `./install.sh` builds the new omabox, the bar widget still running from
the old one can notify "omabox: cannot list boxes". That goes away by itself once the install is done.

### Fixed

- **`omabox guard on` and `off` left one more `settings.json.bak-*` each time**: they now keep only
  the newest backup of each file and say how many older ones they removed, also when the guard is
  already on or off, so the upgrade (whose `install.sh` turns the guard on) clears the ones earlier
  versions left. Backups of your own (`settings.json.bak`, `.bak-mine`) stay. `setup --remove` names
  the backups it leaves. Thanks to [@btsouth](https://github.com/btsouth)
  ([#403](https://github.com/diogochaves/omabox/pull/403), [#398](https://github.com/diogochaves/omabox/issues/398)).
- **A box could take a protected port while your service on it was down**: pasta still forwarded
  the box's servers on `protect-ports` to your `127.0.0.1`, so a box listening there took the port
  and your browser's `localhost:PORT` reached the box. Protected ports are now left out both ways.
  And a missing or failing `nft` no longer shows as "bwrap did not start" after 10 s: `up` looks for
  it in `/usr/bin`, where the box's start runs it, and says at once when setting the rule failed,
  with nft's error ([#364](https://github.com/diogochaves/omabox/issues/364)).
- **`run --pass` from an agent in ai-jail started before the upgrade exited 2**, as if its command
  had: the jail still had 0.x's `omabox-relay`, which does not know how 1.0 hands it the values. It
  now exits 125, `run`'s own, saying to restart the jail; the relay's own usage errors are `run`'s
  125 too ([#380](https://github.com/diogochaves/omabox/issues/380)).
- **`omabox broker off` left the broker's socket file behind** in `$XDG_RUNTIME_DIR/omabox/.broker/`:
  the socket unit now removes it whenever it stops, and `off` removes it (and `.broker/`, when
  nothing else is in it) itself, so the first `off` after upgrading also clears what an earlier
  version's unit left ([#393](https://github.com/diogochaves/omabox/issues/393)).
- **`omabox setup`'s `omarchy plugin enable chaves.omabox` line failed when run right after it**
  ("plugin 'chaves.omabox' is not known"): the running shell had not yet seen the widget setup just
  linked. setup now has the shell rescan its plugins, as `omarchy plugin add` does, and waits until
  it lists the widget, so the line works at once; a shell that still does not know it after 4 s gets
  a line that names the rescan too ([#397](https://github.com/diogochaves/omabox/issues/397)).
- **`./install.sh` without a terminal**: `--check`'s failure named box logs that its own cleanup had
  just removed; they are now kept in `~/.local/state/omabox/install-check/`. With missing packages
  and no terminal for sudo, it prints the exact `sudo pacman -S --needed …` line to run, then
  `./install.sh` again, instead of sudo's own error. A machine whose only render node renders in
  software (a VM's virtio-gpu without virgl) is told up front that a box needs GPU rendering,
  instead of the box dying "while starting"
  ([#395](https://github.com/diogochaves/omabox/issues/395)).

## 1.0.0 — 2026-10-27

omabox is one Rust binary now, with the same commands, options, output and exit codes as before
except where this section says otherwise ("Changes from the bash omabox" lists each). What scripts
and agents branch on is kept from this release on: exit codes and `--json` fields stay as they are
within 1.x, or change only by addition, and a command, option or variable that has to change warns
for one minor release first (`omabox: deprecated: …`, naming the new form) before it is refused.
README, "Stability", says what that covers and what it does not (message wording, help text,
defaults and heuristics, image pixels, what other programs report through omabox). The forms 0.9.0
deprecated are refused now, and a region given beside `--window` is the window's everywhere. Also:
protected ports no box reaches, boxes at home on Omarchy 4.5 (1.0.x keeps working on 4.0; 1.1 will
need 4.5), and the fixes from a review of every rule in SECURITY.md.

**Upgrading from 0.x:** take your boxes down (`omabox down --all`), then `git pull && ./install.sh`
in one go: it now builds omabox, and needs Rust (it installs Arch's `rust` when there is no `cargo`).
Restart any agent running in ai-jail. Every path in your HOME stays where it was.

### Added

- **Protected ports**: `protect-ports=8080,5432` in `~/.config/omabox/config` names host ports no
  connected box reaches, on the host's loopback or through any of its other addresses (a docker
  bridge, a VPN), TCP and UDP; an isolated box's `--allow` refuses them. None by default. Needs `nft`
  (nftables) when set. A value that is not a list of ports refuses every `up` (exit 2) rather than
  being ignored. Set it with
  `omabox config protect-ports 8080,5432` (checked as `up` reads it, kept sorted; `default` removes
  it); `omabox config` lists it, `--json` as `protect_ports`
  ([#355](https://github.com/diogochaves/omabox/issues/355)).

- **`OMABOX_STRICT=1`** refuses a deprecated form at once (exit 2) instead of warning, for CI.
  Every deprecation warning from 1.0 on starts `omabox: deprecated:` and names what to write; 1.0
  itself has none ([#287](https://github.com/diogochaves/omabox/issues/287)).

### Changed

- **omabox is one binary, built by `./install.sh`, which now needs Rust**: `bin/omabox` is a link to
  `build/omabox`, built from `crates/omabox` (install.sh installs Arch's `rust` when there is no
  `cargo`; the first build fetches its crates). Pull and run `./install.sh` in one go: until it has
  run, `omabox` is missing, new agent sessions start without the guard, and ai-jail with omabox's
  maps does not start. Take boxes down before the upgrade, and restart agents running in ai-jail.
  Every path in your HOME (`~/.local/bin/omabox`, ai-jail's maps, the broker's unit, the guard's hook)
  stays as it was; what changed beside it is under "Changes from the bash omabox" below
  ([#324](https://github.com/diogochaves/omabox/issues/324)).
- **An interactive box's monitor windows have a class of their own**: `aquamarine-omabox-<token>`
  where it was `aquamarine` (the box's first window keeps `aquamarine`), each with a host rule of its
  own that keeps it on the box's workspace, silent, and stays until your Hyprland next reloads its
  config, so those windows stay drawn while hidden. A rule of yours matching `^aquamarine$` no longer
  applies to them; `^aquamarine` matches both. `monitor add` on an interactive box and `up
  --interactive --monitor` need omabox's own aquamarine build for this: on the system's aquamarine,
  or in a box started before you rebuilt it or by an older omabox, they are refused, saying so
  (`omabox setup --aquamarine`, then a new box). Two `monitor add`s on one interactive box now take turns (the
  box's lock: the second waits up to 60 s, then says the box is busy)
  ([#349](https://github.com/diogochaves/omabox/issues/349)).

- **Beside `--window`, `-g` and `--ignore` are the window's own coordinates everywhere** (0,0 its top
  left corner), as `click`'s X Y, `shot -g` and `check point` already were: `wait still|change
  --window SEL --ignore`, and `--wait`'s `-g` and `--ignore` beside `--window` on `keys`, `click`,
  `scroll` and `drag`. 0.9 took them in the screen's, with a warning. A 124's `--ignore "X,Y WxH" if
  that is an animation` gives the window's coordinates there too. To watch a part of the screen
  whatever the window, `-g` without `--window`.

- The skill's reference says a box turns PipeWire's realtime module off for its clients, which hides
  a realtime-limit bug, and how to run one client without that
  ([#305](https://github.com/diogochaves/omabox/issues/305)).
- The reference says plainly that wgpu apps (eframe's default renderer, iced, Bevy) do not start in
  a box on NVIDIA, and what to do: a GL renderer, or a box on the AMD/Mesa GPU
  ([#306](https://github.com/diogochaves/omabox/issues/306)).
- Parallel subagents of one session share its default box name: the reference tells each to take a
  box of its own (`up --new` or `-b` a name of its task), and `omabox brief` to take down no box it
  did not start ([#295](https://github.com/diogochaves/omabox/issues/295)).

- **Where box windows open**: the widget's Settings offers workspaces 1-10 and **Its own
  (special:omabox)**, a special workspace of omabox's: full size, opened by the widget's Show, hidden
  by a workspace switch. It no longer offers the scratchpad, which Omarchy 4.5 made its Quake console
  (a half-height panel that starts the default agent when it opens empty). `special` still works
  (`--workspace`, `omabox config workspace`), and a config that names it is kept and shown as it is;
  on 4.5 `omabox` says what the scratchpad is now. The README gives a key binding for `special:omabox`
  ([#292](https://github.com/diogochaves/omabox/issues/292)).

### Removed

- The forms deprecated in 0.9.0 are refused (exit 2; `run`'s 125) with one line naming what to
  write, never read another way ([#287](https://github.com/diogochaves/omabox/issues/287)):
  - `wait cmd`: write `wait until -- CMD`.
  - `up --idle 45` and `OMABOX_IDLE=45`, a bare number (minutes up to 0.9): write `45m`.
  - `keys -s 300` and `pointer -- sleep 300`, a bare number (milliseconds up to 0.9): write `300ms`.
  - `up -i`: write `--interactive`. `save -f`: write `--force`.
  - `shot` and `calibrate` given two files: name one.
  - A box name given with characters a box name cannot have, or past 40 of them (`-b "my box"`):
    the refusal names the one that fits (`my-box`).
  - `keys --delay MS`: for a pause, `-s DURATION`.
- The JSON fields 0.9 kept beside their successors: `config --json`'s kebab-case keys (`bar-icon`,
  `confirm-close-available`, `gpu-now`...: read `bar_icon`...), `ls --json`'s `allow` (read
  `allowed`), `ports --json`'s `address` (read `addresses`).

### Fixed

- **Plugin settings that held what looks like a key, token or password were copied into every box**:
  `~/.config/omarchy/extensions` went into the box HOME without the check your desk's other files
  get under `--desk-shell`. Such a file now stays on your desk, and `up` names it ("not seeded:
  extensions/NAME ..."); a setting that only mentions one (`maxTokens`) still goes in.
- **`up --clean --from SAVE` could write its wallpaper outside the box**, onto the file a link the
  save left at `~/.config/omabox/clean-wallpaper.png` led to. It is written in the box HOME.
- **A monitor window of an interactive box could open on your workspace and take focus**: when the
  box's Hyprland took more than 5 s to make it, when `monitor add` was interrupted (Ctrl-C, TERM,
  HUP) while the box made it, or when two adds ran at once; and an add cut short could leave its host
  rule on, sending every later aquamarine window (a nested Hyprland's too) to the box's workspace.
  The window now has its own class and rule from the start, so it opens on the box's workspace
  without focus whatever happens to the add, unless your Hyprland reloads its config between the
  rule and the window's first map (a theme change, a saved config: the reload drops the rule). An
  add cut short removes its half-made monitor, and a second Ctrl-C ends that at once
  ([#349](https://github.com/diogochaves/omabox/issues/349)).
- **An `up` interrupted after its box started (Ctrl-C, TERM, HUP) left the box running**, unseen
  and, for an interactive one, with no idle limit to take it down: during `up --interactive
  --monitor`'s add, or while it waited for the box's shell. It now ends the box, as any other `up`
  that fails, with the signal's exit code (130, 143, 129); a Ctrl-C to `up` alone no longer lets it
  go on.
- **`pixel` left a temporary file in $TMPDIR** when grim's frame was cut short, its size unreadable,
  the point past its end, or a signal ended it; a failed update of a box's `box.json` left
  `box.json.tmp` in the box's folder, and `output` its `output.dropped.tmp`. Scripts: nothing to
  change ([#357](https://github.com/diogochaves/omabox/issues/357)).
- **A command in a box had your terminal as its controlling terminal** when it had no terminal of
  its own (its output piped or in a file, from a terminal): it, and what it left running, could open
  `/dev/tty` and read what you typed there, write to it or signal its session. It now runs in a
  session of its own in the box, with no controlling terminal; a Ctrl-C, Ctrl-\ or Ctrl-Z at your
  terminal, or a signal to `run`'s process group, still reaches it and everything it started, and
  nothing of it is left once `run` is stopped or killed. A command run at your terminal keeps the
  pseudo-terminal of its own it had.
- **`omabox env` printed a path inside the box's runtime folder**, which the box can write: a link
  put there in its socket's place could lead a host tool to another display, your desktop's included.
  It now prints a hard link to the box's own socket in the box's folder, and refuses a socket that is
  not one.
- **The tools omabox runs against a box (grim, hyprctl, its input tools, gdb) could make a crash
  notification on your desktop**: unlike a box's own commands, they kept your core limit. They get
  the box's 1-byte limit.
- **`monitor remove` on an interactive box returned before its screens had their new scales**: right
  after it, the main screen could still be at the scale it had beside the monitor (0.825 for 1). It
  now waits for them, 5 s at most ([#382](https://github.com/diogochaves/omabox/issues/382)).
- **`click --window` (and `--in` a window's shot) could fail with "did not take focus in 3 s"** on a
  window moved while its workspace was hidden: the window it had been over took the focus back. A
  window that had the focus and lost it at once is focused again.
- **`keys up:KEY` of the last key held returned while the key holder was still letting go**: its
  virtual keyboard stayed on the box's seat, and the holder in the box's process list, for some
  30 ms more. It now returns once the holder has ended.
- **A monitor could come up at scale 2 where `--monitor` or `monitor add` asked for another**: one
  at the size an output starts at (1280x720 on NVIDIA) was now and then kept at Hyprland's default
  scale, which a reload kept too. Its scale is set before it exists, and the add waits until the box
  has it (rounded to whole pixels at most, as `mode`'s); one Hyprland makes at another is removed,
  saying what it made, exit 1 ([#379](https://github.com/diogochaves/omabox/issues/379)).
- **`check run` failed a checkpoint on holes its baseline already had**: every hole was read as
  new, so an approved see-through design failed each run. Only holes the baseline has none
  overlapping count as new, as before the Rust omabox.
- **A call right after a box's Hyprland crashed could say it "did not answer (hung?)"**: the call
  that hit the crash said the box went down while the box was still ending. It now says so once
  the box is down (within a second), and the next call says the box is dead and why.

- **`tree --window` printed another window's tree for a window its app does not publish**
  ([#302](https://github.com/diogochaves/omabox/issues/302)). An app that puts only one of its
  windows on the accessibility bus (egui's root viewport, not its child ones) had that window's tree
  printed for every other one. Now the app's only window stands for one of another title only when
  it is that window's size and no other window of the app is; else `tree` says the window has no
  tree, or that which one it is cannot be told, exit 1.

- **`run -d` in a repo no longer says to start a box when your session has one up elsewhere**
  ([#296](https://github.com/diogochaves/omabox/issues/296)). After `omabox up` in a scratch folder,
  `run -d` back in the repo said `omabox up <repo>-…`, and agents started a second box. It now names
  the session's box to pass with `-b`, as the other commands do there.

- **`up --desk-shell` leaves omabox's own bar widget out**
  ([#304](https://github.com/diogochaves/omabox/issues/304)). It lists your boxes with `omabox ls`,
  which a box does not have, so the box's shell logged a warning for it (and would have notified).
  `up` says it left it out; `--plugin` still mounts it.

- **`up`'s line names every monitor** ([#308](https://github.com/diogochaves/omabox/issues/308)):
  `box 'NAME' up (1920x1080@60 + 1080x1920@60)` for a box started with `--monitor 1080x1920`, not
  the main screen alone.

- **`run -d --wait` said `satisfied` for a command that had already died**
  ([#297](https://github.com/diogochaves/omabox/issues/297)), when something else on the screen
  settled meanwhile (the last app's window closing, another agent's window), and its end only came
  as a warning on the next command. Now a command killed or ended with a status other than 0 by the
  time the screen settles makes the answer `unsatisfied`, says its end and its log's last lines, and
  exits with its status. One that ends with 0 having changed the screen stays satisfied.

- **A `run -d` job's warning said "its words were not kept" for ordinary jobs**
  ([#298](https://github.com/diogochaves/omabox/issues/298)). The next command's warning about a
  job that ended lost what the job was when it died at once, when the box was slow to make it a
  session of its own, when `--replace` or another command had looked at it first, or when another
  command touched the job records at the same moment. A job's record is now always kept until its
  end has been said, so the warning names the command and its pid.

- **The warning about a `run -d` job that ended fired for jobs you stopped yourself**
  ([#300](https://github.com/diogochaves/omabox/issues/300)): after `omabox run -- pkill APP`, the
  next command said the job "was killed by SIGTERM". A job killed (SIGTERM, SIGINT or SIGKILL) while
  an `omabox run` ran in its box, or within 2 s of one ending, is no longer said; exits and crashes
  still are.

- **A box going down could show on your desktop as "Process crashed: Xwayland"** (`up --xwayland`).
  Hyprland starts Xwayland with `-core`, which raised its core limit back past the 1 byte everything
  in a box gets, so its abort when the box's compositor went was recorded as a crash. A box's Xwayland
  now keeps the 1-byte limit (unless you raised it yourself).
- **The shell-crash warning fired for any Quickshell in the box, and missed a killed shell.** A
  Quickshell of your own crashing (to test its crash handling) was reported as the shell crashing,
  and omabox closed its crash dialog; now only the box shell's crashes count and only its dialog is
  closed. A shell killed outright (`kill -9`) and started again by Omarchy's launcher is now said by
  the next command: `the shell was killed by SIGKILL (137) since the last command and was started
  again`.
- **`run` printed bash's `line N: PID Killed prlimit …` for a command killed by a signal**; it now
  says `the command was killed by SIGKILL (137)`, with the same exit status.

- **`wait` and `--wait` could print a garbled answer under load**
  ([#310](https://github.com/diogochaves/omabox/issues/310)), such as `unsatisfied: changingt=2002`,
  when the screen watcher's answer arrived in two pieces split at a space. The space is kept now.

- **An interactive `monitor add` could open its window on your workspace, focused**, when the box
  took more than 5 s to make it: `hyprctl` stops waiting after 5 s, and omabox took that for a
  refusal and turned off the rule that sends the window to the box's workspace. Now an answer that
  says nothing either way counts as "maybe made": the add removes the monitor again, waiting for the
  box to answer (about 20 s at most), and only then turns the rule off, also when the add is
  interrupted or its terminal hangs up; `monitor add` returns only once the rule is off, whatever the
  signal. The add still fails, saying the box did not answer
  ([#349](https://github.com/diogochaves/omabox/issues/349)). While the rule is on (about 35 s at
  worst), a nested Hyprland of your own opening a window is caught by it too, as before.
- **A shell a test starts itself, under `omabox run`, no longer loses plugin hot-reload.** The box's
  stand-in that keeps its own shell from reloading a mounted plugin on every save also caught other
  shells, so Omarchy's own `runtime-smoke-test.sh` failed in a box. Now it applies to the box's
  session only ([#317](https://github.com/diogochaves/omabox/issues/317)).
- **The first shot after `up` could be a black screen**: 4.5's shell covers every screen for a new
  login until the theme's intro has played, which in a box (no OWE daemon) took up to 5 s past `up`.
  A box's start is now marked as had, and `up` also waits out the cover
  ([#291](https://github.com/diogochaves/omabox/issues/291)).
- **A box's screen came up on workspace 11**: 4.5 gives each monitor ten workspaces of its own, and
  Hyprland's stand-in output took the first ten. The main screen is on 1-10 again, monitors added
  after it on 11-20 and on; an interactive box closed with confirm-close and opened again keeps its
  workspaces ([#292](https://github.com/diogochaves/omabox/issues/292)).
- **`omarchy-theme-set` reloaded Hyprland about a second after it returned** in a box, on themes with
  intros: now before it returns, so `wait still` and a shot see the new theme.
- **A desk's `centerAnchor` and a plugin's `center` section** are placed as 4.5 places them
  (`centerBegin`, `center`, `centerEnd`), also for a desk on an older Omarchy in an `--omarchy` box.
- **`up --clean --from SAVE` showed the save's notifications**: 4.5 keeps them in the HOME; a clean box
  starts without them.
- **`up --omarchy` of a 4.5 tree on an older host**: the tree's xkb files go into the box (its keyboard
  option), and `up` says which of owe, omasnap and the titlebars plugin the host lacks
  ([#293](https://github.com/diogochaves/omabox/issues/293)).
- **`click --window --no-raise` was refused over 4.5's notifications column and dock with nothing
  there**: both stay mapped while idle and take input only on their items. A layer spanning the
  screen's height or width (and not reserving its edge, as a bar does) is now said as one the input
  may go to, like a full-screen one, and never refused; the message says "a layer spanning the screen"
  where it said "a full-screen layer" ([#311](https://github.com/diogochaves/omabox/issues/311)).

### Changes from the bash omabox

- **`omabox up B` right after `omabox down B` starts the box**: the bash omabox could cancel it as
  if the down had come while it was starting, when both began within the same 10 ms. A down that
  comes while its up starts (`omabox up B & omabox down B`) still cancels it, told by the order the
  two processes were made in. Scripts: nothing to change.
- **A host command omabox runs gets your `SHLVL` and `_` as they came** (`guard exec`, `host`, a
  `check run` scenario); the bash omabox set its own. Neither is part of omabox's contract. Scripts:
  nothing to change.
- **`omabox --version` says the version omabox was built from**: a build older than its checkout
  (pulled without `./install.sh`) says its own, where the bash omabox read the checkout's file.
- **A jailed agent's `--pass` and `--env-file` values reach omabox-relay on a file descriptor, not its
  command line**, as values everywhere else in omabox do. `omabox-relay call` takes `--env-fd N`. A
  jail started before the upgrade keeps its old relay: restart it.
- **A jailed agent's `config` leaves `protect-ports` out**, and `config protect-ports` from inside
  ai-jail is refused saying why ("protect-ports is not for a jailed agent"), not as a change of
  settings: which host ports are protected is the user's to know. Scripts: nothing to change.
- **Without `XDG_RUNTIME_DIR`, omabox says so in its own words**: "omabox: XDG_RUNTIME_DIR is not
  set" (exit 1, as before), where the bash omabox printed "bin/omabox: line 13: XDG_RUNTIME_DIR:
  XDG_RUNTIME_DIR is not set". Scripts matching that line: match "XDG_RUNTIME_DIR is not set".
- **`help WORD` takes WORD as written**: `omabox help 'u\160'` is an unknown command (exit 2). The
  bash omabox handed the word to awk, which read escapes in it, so it printed `up`'s help. Scripts:
  nothing to change.
- **`config` and `guard` refuse a settings link they cannot follow** (into a folder not there yet,
  or a loop), saying so and naming it; the bash omabox exited 1 there without a word. The link is
  kept. A config that cannot be read stops every `up` (exit 2) and `config` says so; the bash
  omabox read it as empty, protecting no port. Scripts: nothing to change.
- **Entering a box is omabox's own**, not `nsenter`'s (`omabox run -b B -- true` takes about 3 ms
  on a box already up). What a failed entry says is in omabox's words, "omabox: cannot enter the box: REASON",
  where nsenter's own line was printed (and "dropped N bytes of other output on the box entry's
  stderr" where it said "nsenter's stderr"). A TERM or KILL sent to omabox alone, or to the process
  that waits for a box command (`pkill -f 'omabox run'` reaches it), ends the command and what it
  started, where nsenter (or omabox) died and left them running; that process is named
  `omabox-enter`, and `pgrep -f 'omabox run ...'` finds it besides omabox; in the box, `ps` shows the
  command's session led by `omabox-session`, its command line that name alone (nothing in the box has
  the words of your call, so `run -- pgrep -f WORD` finds only what is in the box). Scripts that
  matched `nsenter` lines or processes: match these.
- **A command in a box has no controlling terminal** unless it runs at yours (then it has a
  pseudo-terminal of its own, as before): `/dev/tty` there is "No such device or address", and it
  runs in a session of its own, in a process group of its own. A command that needs a terminal while its output is piped: run it as
  `script -qc CMD /dev/null` in the box.
- **`omabox env` exports `WAYLAND_DISPLAY=$(omabox path)/wayland`**, a hard link to the box's
  socket, where it exported `$(omabox path --run)/wayland-1`; it exits 1 when the box's socket is not
  a socket (a link in its place). Scripts that built the path from `path --run`: use `omabox env`.
- **`down` of a box it cannot see from this user namespace** (a sandbox's) **refuses**, as `up` does:
  "cannot tell whether box 'B' is up from this user namespace ...", exit 1, the box left running and
  listed. The bash omabox said "box 'B' down", exited 0 and removed the box's dir while it ran on.
  Scripts: run `down` outside the sandbox.
- **`omabox config` has a sixth key, `protect-ports`**: listed after `shot-fit` (`protect-ports=`
  when none is set), in `--json` as `protect_ports`, and named among the keys when one is unknown.
  A bad value of it in the file is said as `up` says it and shown, not ignored. Scripts that read
  `omabox config` line by line or its `--json` keys: expect the new one.
- **`host` and `guard exec` say a command they cannot run in omabox's words**: "omabox: host: cannot
  run CMD: not found in PATH" (or "no such file", "permission denied"), where the bash omabox printed
  "bin/omabox: line N: exec: CMD: not found". Exit codes unchanged (127, 126).
- **A jailed agent's `down` and `up` check whose the box is again once they hold its name's lock**,
  and a jailed `down` that finds no box cancels only its own jail's `up` of that name, not yours.
  Scripts: nothing to change.
- **The idle reaper says how often it looks**, once, in the box's `reap.log` (`omabox log reap`):
  "box 'B': watching it every 60 s". The bash omabox said nothing of it. Scripts: nothing to change.
- **`save` and `saves rm` say what they could not remove in omabox's words** ("could not remove all
  of save 'S': ..."), where the bash omabox printed `rm`'s own line. Exit codes unchanged.
- **`up` stopped by INT, TERM or HUP ends with the signal's code at every step** (130, 143, 129),
  its box ended; the bash omabox could exit 1, its box left running, when the signal landed during
  `up --interactive --monitor`'s add. Scripts: nothing to change.
- **`up --no-seed` and `--keep` take their words as written**: `*`, `?` or `[` in them are refused.
  The bash omabox matched them against the current folder's file names (`--no-seed 'font?'` was
  `fonts` in a folder holding a file of that name). Name the words instead.
- **`up` says its own words**: `up --audio` names the programs it lacks as "pipewire, pactl" (the
  bash omabox wrote "pipewire,pactl"); a desk with no `shell.json` no longer prints bash's "No such
  file or directory" line; a failure to make the box HOME is said as "up: could not ... in the box
  HOME". Exit codes unchanged.
- **An INT sent to omabox by another process (`kill -INT PID`) stops the command like TERM**, once
  the step in progress is over, with what a TERM would leave (a temporary file removed, a monitor
  half made removed, a `record --for` stopped), exit 130. The bash omabox went on or stopped
  depending on the step it was in (`cpu`, `gpu`, `peek --focus`, `gdb`, `record --for` and `check
  run` went on to their end). A Ctrl-C at the terminal is unchanged where a program takes it (the
  scenario of `check run`): if that program carries on, so does omabox; any other step it lands on,
  `gdb`'s among them, now stops the command (gdb's runs in a process group of its own, which a
  Ctrl-C never reached). Scripts that sent INT to omabox expecting it to go on: send nothing, or send
  it to the child.
- **A TERM or HUP lets the step in progress end before omabox undoes its work** (at most ~20 s,
  then that step, with its process group, is stopped; after a second omabox says "stopping once the
  step in progress ends (at most 20 s)"): `check run` stopped during a scenario takes down its boxes
  once the scenario's step is over, where the bash omabox took them down under it and left the
  scenario running. Exit codes unchanged.
- **`record --for` and `record -- ACTION` stopped by INT, TERM or HUP stop their recording**: the
  video so far is made and said, the pointer shown again, then omabox exits 130, 143 or 129. The
  bash omabox left the recording running in the box, the pointer hidden, and said nothing (`record
  stop` ended it later). Scripts that ran `record stop` after such a signal: it now says the box is
  not recording; the video is already where `-o` said.
- **The first INT, TERM or HUP decides how omabox ends**, and what it was undoing finishes: a
  second signal no longer cuts its cleanup short, and the exit code is the first signal's (TERM then
  INT: 143; the bash omabox gave the lowest-numbered one's, 130). A signal during `record stop`
  stops the step in progress: during `--sheet`'s frames, no sheet is made (the bash omabox made it,
  then exited 130 or 143 all the same); during its encode, an INT sent to omabox alone stops it with
  130 (the bash omabox went on). Scripts: nothing to change unless they relied on the second
  signal's code.
- **An interactive `monitor add` ends at any Ctrl-C or INT**, its half-made monitor removed, exit 130.
  The bash omabox did so for a Ctrl-C to its terminal, but an INT sent to omabox alone (`kill -INT
  PID`) ended it with 130 while the add went on and kept the monitor. A script that sent omabox an
  INT and counted on the monitor being made should let the add finish instead.
- **A box name such as `-n` is named in its refusal** (`OMABOX=-n`, a repo of that name): "bad box
  name -n: ...". The bash omabox read the name through `echo`, which took `-n`, `-e` or `-E` for its
  own options, so its message showed an empty name. Exit code unchanged. Scripts: nothing to change.
- **`--mod` takes its words as written** (click, drag, pointer, scroll): `*`, `?` or `[` are refused
  as modifiers. The bash omabox matched them against the current folder's file names, so `--mod '*'`
  in a folder holding a file named `ctrl` held Ctrl. Name the modifiers instead.
- **`run`'s command killed by a signal is said**: "omabox: the command was killed by SIGTERM (143)"
  (any signal but INT and PIPE). The bash omabox counted its own "Terminated" line as nsenter's, said
  "dropped 50 bytes of other output on nsenter's stderr" for a TERM, and waited 2 s before
  answering; now only a SIGKILL (what a box's end sends) waits up to 2 s, to say whether the box
  went down. That wait and that word now come from every box step killed so (a `record` or `audio`
  step that had to be killed after its time limit), where some said nothing. Exit codes unchanged. A script that matched "dropped N bytes" after a TERM:
  match "was killed by", or the exit code.
- **Errors omabox meets on its way are said in its own words**, no longer in bash's or a tool's: a
  `check run` scenario that cannot run says so in its run's log as "omabox: check: cannot run
  scenario X: its #! interpreter '/bin/sh^M' is not there (CRLF line ends?)" (was "bash: X:
  /bin/sh^M: bad interpreter: ..."), a file omabox could not write as "omabox: CMD: cannot write
  PATH: why" (was a bare "PATH: why"), a missing tool as "omabox: cannot run jq: not found in PATH"
  (was "jq: command not found"), `pixel` past its image as "omabox: pixel: the point is past the end
  of grim's image" (was od's words), and `clip`'s refusal names the process "omabox", not "bash". Exit
  codes unchanged (127 for a scenario not found, 126 for one that cannot run). Scripts that matched
  those texts: match the exit code, or the new words.
- **Where omabox ended without a word, it says why**, with its usual exit codes: `pixel` on a frame
  it cannot read (1), `gdb` on a box this user namespace cannot read (1, was 3) or a gdb that did
  not start (1, was gdb's status), `check` on a folder or file it cannot make, write or copy (2,
  check's code for its own failures; was 1, check's code for "changed"), `shot`, `record` and
  `calibrate` on a path that cannot be resolved (1). A box's list of monitors that cannot be written
  to its `box.json` is said and the command carries on (some commands ended with 1 there, unsaid, and
  others went on). Scripts that tested for exit 3 from gdb, or exit 1 from `check` for those: test
  for non-zero, or for 2.
- **Box names, and every number and name omabox checks, are ASCII** in every locale: `é` or `²`
  are refused with the usual message. The bash omabox checked them in the caller's locale, where
  en_US.UTF-8 and most others take `é` for a letter and `²` for a digit: a process with
  OMABOX_BOX=é was named box é by `which`, and `pixel 1 ²` ended in bash's own words ("invalid
  integer constant", exit 1). `events --grep` and `--until` still read your pattern in your locale.
  Scripts: nothing to change.
- **Lists are in byte order, the same in every locale**: a session's boxes in a message, run logs,
  `check run`'s checkpoints, `ports` and `gpu`'s rows, and `ls` and `saves` (table and `--json`).
  The bash omabox followed the locale's collation, so under en_US.UTF-8 `a` came before `B`, under C
  after it. Scripts that sorted the output themselves: nothing to change.
- **Text is read as UTF-8 whatever the locale**: the default box name of a repo with non-ASCII
  letters, `keys`' notes (`secret N`) and `record`'s `--as` and mark lengths come out the same under
  LANG=C as under a UTF-8 locale (the bash omabox counted bytes under C), and option words are
  lowered the same way in every locale (a Turkish locale refused `--mod SHIFT`). A monitor's scale
  and rate are written with a dot whatever LC_NUMERIC says, and a system error in omabox's message
  in English. `events --grep` and `--until` still read your pattern in your locale. Scripts:
  nothing to change.
- **Numbers with zeros in front are decimal**: `--fps 010` is 10 fps, `-g 010,0,…` starts at
  x=10, `--frames 08` is 8 frames. The bash omabox read them as octal (8), and `08` or `09` ended
  the command with bash's "value too great for base", exit 1. A number too big for 64 bits gets the
  option's usual refusal (it wrapped). Scripts that padded numbers with zeros get what they wrote.
- **JSON numbers are written in standard form**: `1000`, not jq's `1E+3`; `1e+300`, not `1E+300`
  (`lua --json`, a box's `box.json`). A number written without an exponent stays as it was. Scripts
  that read the JSON with a JSON reader: nothing to change.
- **Waits and timeouts run on a clock that a time change does not move**: `wait --timeout`,
  `events --until`'s timeout, `run -d --replace`'s grace before SIGKILL, `shot --burst`'s spacing,
  `cpu` and `gpu`'s sampling window, and the built-in waits (a window to appear, a mode to apply,
  peek's focus, gdb's attach, `wait`'s 30 s notes), which also last their full time now. The bash
  omabox measured them on the wall clock, and the built-in ones in whole seconds, so they could end
  up to a second early. Scripts: nothing to change.
- **`check run SCENARIO` always runs the program found in PATH**: a scenario named like a shell
  builtin or one of omabox's functions (`test`, `true`, `say`) is no longer run inside omabox. One
  that names a program in PATH (`test`, `true`) runs that program; one that names none (`say`,
  `shopt`) fails as a scenario not there does: "omabox: check: cannot run scenario say: not found
  in PATH" in its run's log, its status 127, `check run` exit 3. The bash omabox ran its own
  builtin or function there. Scripts: nothing to change (give a path, as before, for a scenario of
  your own).
- **A Ctrl-C (or a TERM or HUP) during `omabox up` clears the box**: the box was ended either way,
  but the bash omabox kept its folder once it was written, so `omabox ls` listed it `dead` and its
  idle watcher ran until `omabox down`. Now it goes as a `down` would. An `up` that fails still keeps
  the box's folder and logs, as before (#388). Scripts: nothing to change.
- **A signal no longer leaves temporary files behind**: `omabox shot` stopped during its capture
  left an empty `OUT.part` and grim's error file in `$TMPDIR` (#386; with `--no-pointer` the
  `OUT.part` still can), and a signal in the instant
  between making and registering `cpu`'s, `gpu`'s or `record stop`'s working files left those
  (#360). The bash omabox did the same.

## 0.9.2 — 2026-10-20

A fix to the agent guard for hosts on Omarchy 4.5 (its release candidates and the edge channel):
without it, a guarded agent could reach your desktop's shell through `omarchy-shell`. Run `omabox
guard on` once after updating. Also: a box's shell is no longer killed now and then as it starts
under load.

### Fixed

- **The agent guard let Omarchy 4.5's `omarchy-shell` reach your desktop's shell** (#290). On
  Omarchy 4.5 (its release candidates and the edge channel today), `omarchy-shell` finds your
  session's display when the one it is given does not exist, which the guard's never does, and then
  talks to your bar's shell directly, so a guarded agent's `omarchy shell …`, menus or theme hooks
  could act on your real desktop. The guard now leaves a file in your runtime directory that keeps
  `omarchy-shell` from looking further, and its own `omarchy-shell` refuses with a reason. Run
  `omabox guard on` once to renew the hooks (sessions say theirs is outdated until you do).
- **A box's shell was now and then killed as it started, under load** (`the shell exited after
  starting`; its log: `exited with status 137`). PipeWire, asking a box's portal for realtime limits
  it cannot give (a box has no RealtimeKit), set the shell's realtime time limit to zero, and the
  kernel kills such a process the moment a thread of it is boosted to a realtime priority, which
  happens under load. PipeWire's clients in a box no longer load that module, so the limit stays
  unlimited.

## 0.9.1 — 2026-10-20

The release candidate for 1.0 (planned for 2026-10-24). The skill's core is about half its size,
the rest in `reference.md`. An action's `--wait` says what changed on the desktop, and where the
input went when it timed out. A `run -d` job that dies in its first 30 s is said on the box's next
command. A full-screen layer such as Omarchy's notifications is only "possibly" over a point, so
`--no-raise` goes on. And `up --clean`'s frozen clock no longer slips under load.

### Added

- **`--wait` says what changed on the desktop** (#274). After `keys`, `click`, `scroll`, `drag` and
  `run -d` with `--wait`, the line (still starting `satisfied:` or `unsatisfied:`) adds what changed
  since just before the input: `window opened, focused: zenity "Save as"`, `window opened on
  workspace 2: ...`, `window closed: ...`, `layer omarchy-menu opened`, `focus: none`, `foot title
  now "..."`, `workspace now 2`; three at most, then `+N more (omabox events --since OFFSET)`. A 124
  says where the input went (the focused window, a layer holding the keyboard, what was under the
  click, in the window's own coordinates) and `no window, layer or focus changed` when nothing did,
  so it needs no shot to learn that nothing happened. `--json` adds `changes`, `target` and
  `events_since`. Exit codes are unchanged; it costs ~25 ms a call (~40 ms on a 124).

- **A `run -d` job that dies a few seconds in is said by the next command**
  ([#286](https://github.com/diogochaves/omabox/issues/286)). `run -d` catches a job over in its
  first 0.3 s; a server that died a second later was said by nothing. Now a job that ends within its
  first 30 s with a status other than 0 is one warning on the box's next command, whatever it is
  (`omabox: warning: run -d job ./server (pid 115) exited with status 1 after 2.1s, since the last
  command (omabox log -b NAME run)`), once. A job `--replace` stopped, one `run -d` already
  reported, one that ran longer, a one-shot that exited 0 and a launcher whose app still runs are
  not said. No cost to a command while no job has ended.

### Changed

- **A full-screen layer is said as possibly over a `--window` point, not refused**
  ([#263](https://github.com/diogochaves/omabox/issues/263)). Omarchy's notifications sit in a
  layer covering the whole screen that takes input on its toasts only, so while a toast showed every
  click counted as under it: `--no-raise` refused clicks far from the toast. Such a layer is now
  said as one the input *may* go to (`a full-screen layer "omarchy-notifications" … is up …: input
  at X,Y may go to it if one of its items (a toast) is there`), `--no-raise` goes on, `windows`
  lists it as possibly over the windows, and an action's 124 names the window under it, `or
  possibly on layer omarchy-notifications`. A layer smaller than its screen (a panel, a menu) is
  still over the point for certain and still refused, and wins where both are. `--json` adds
  `certain` to `windows`' `layers` and to `--wait`'s `target`.

- **The agent skill's SKILL.md is the core every task needs**, about half its size (8 kB where it
  was 14 kB, ~2k tokens fewer each time an agent loads it): what never to do, the project
  instructions table, the loop, driving an app, waiting, coordinates, and a line for each section of
  `reference.md` beside it. The rest (the guard and `omabox host`, ai-jail, mounts, showing the
  user, what a box cannot test, reporting) moved to `reference.md` sections the core names; nothing
  was dropped. A text question (a label, a checked box, a field's text) now goes to `tree --find`
  before a shot ([#270](https://github.com/diogochaves/omabox/issues/270)).

### Fixed

- **`up --clean`'s bar clock showed the real time in about one start in six under load**, so
  `omabox check` read the bar as changed with nothing changed. Omarchy's shell reads its defaults
  and the user's shell.json each on its own, and when the defaults came first the frozen "Friday
  12:00" was lost while the bar was being built. In a clean box the shell's defaults are now the
  box's own shell.json, so either order draws the same bar.

## 0.9.0 — 2026-10-20

The last release before 1.0. `omabox help CMD` is short, the command's lines and options
(`--long` for the prose), and `omabox brief` prints what a subagent needs in place of the whole
skill. New: `wait until` (the new name of `wait cmd`), `path --home`, `tree --layer` for the
shell's bar and panels, `record --for` and `record hold`, and the time on each `log shell` line.
Every duration option takes one grammar, every option takes `--opt=value`, `--json` is there where
it was missing, and exit codes mean one thing in every command (`omabox help exit-codes`): called
wrong is exit 2 with the nearest option or command named, and `run`'s own failures are 125. The
forms under Deprecated still work, with a warning; 1.0 refuses them.

### Added

- **`omabox brief -b NAME`** prints ~15 lines a parent agent pastes into a subagent's prompt
  instead of the subagent loading the whole skill (a quarter of its size or less): the box's name,
  size and windows, each window with a `--window` word that finds it alone, then the rules (`-b NAME`
  on every call, one literal command per call, look, act once, look again, never resend unseen
  input, no sleep, never the real desktop, the box HOME's host path for file tools, scripts there
  for several shell steps, stay in your worktree) and when to load the skill after all. The skill
  tells a parent that delegates box work to use it
  ([#271](https://github.com/diogochaves/omabox/issues/271)).
- **`--opt=value`** wherever an option takes a value, in every command (`shot --window=firefox
  --fit=1456`, `omabox --box=x windows`, `pointer move X Y --steps=5`): as `--opt value`. An option
  that takes none refuses one (`--json=1`, exit 2); a value given as a separate word stays as it is
  (`log --grep --x=y` looks for `--x=y`), and so does everything after `--`.
- **`omabox wait until -- CMD`**: the new name of `wait cmd`, waiting until a command run in the
  box exits 0 (the same flags, exit codes and `--json`, whose `condition` is `until`). Claude Code
  refuses any call holding `wait cmd` from a subagent working in its own git worktree, since it
  reads the word as a shell; `wait until` passes
  ([#272](https://github.com/diogochaves/omabox/issues/272)).
- **`omabox path --home`** prints the box's HOME (`/home/sbx` inside) as the host sees it, a plain
  path for an agent's own file tools, in place of `$(omabox path)/home` inside a command.
- The skill's reference has a section for subagents in an isolated worktree: five rules that keep
  every omabox call one the worktree check lets through.
- **`omabox record --for 5s`** records the screen for that long with no action: something that
  moves by itself (a panel cycling on a timer) no longer needs a dummy action that moves the
  pointer. The video is that long, a frame or two more
  ([#259](https://github.com/diogochaves/omabox/issues/259)).
- **`omabox record hold 2s`**, between `record start` and `record stop`, is a pause in a scripted
  take: it returns once the video has 2 s more from when it was called, measured in the video's own
  clock (its frames' times), so a slow box or a slow script never shortens the pause the viewer
  sees; it prints the span ("held 4.210s to 6.210s"). With nothing recording it exits 2, naming
  `record start`. `record mark TEXT --hold 2s` marks, then holds from the mark. The skill names
  both as the way to let time pass in a recording
  ([#280](https://github.com/diogochaves/omabox/issues/280)).
- **`omabox tree --layer SEL`** reads the accessibility tree of a layer surface: the Omarchy
  shell's bar, panels and menus, a plugin's panel. SEL is a namespace (`omarchy-bar`),
  `address:0x...` or `pid:PID`; `--find`, `--json` and `--all` work as for windows, and the bounds
  are the screen's, so a plain `click X Y` at a node's middle lands on it. It needs Quickshell 0.3.2
  or later (or `up --quickshell` a build), and shows only items that set an `Accessible.role`: a
  layer without any says it "publishes no accessible items", as Omarchy's own bar does today
  ([#254](https://github.com/diogochaves/omabox/issues/254)).
- **`--json` on what lacked it** ([#273](https://github.com/diogochaves/omabox/issues/273)):
  `check run` and `check diff` (each checkpoint's result, regions, holes and tree changes, the
  report; for CI), `shot` (the path, what it covers, in which coordinates, the image's size and
  scale, and what `--changed` found), `record status`, `mode`, `path --logs`, `calibrate --answer`,
  `which`, `--version` and `events --mark`. `ls --json` adds `scale`, `dir` and `allowed` (the
  ports as numbers), `ports --json` `addresses`, `saves --json` `size_kb`, `windows --json`
  `x, y, w, h` beside Hyprland's `at` and `size`. The skill's reference lists each command's
  fields under "JSON": from 1.0 they only grow.
- **`omabox ports` lists an isolated box's `--allow` ports** as `allowed` (pasta's, in the box):
  pasta listens on each inside the box, leading to the host's, so a box server on one fails at
  once with "address in use", and before, it just never appeared. `help up` and the skill's
  reference say so: an `--allow` port is for reaching that host port from the box; serve on another
  ([#286](https://github.com/diogochaves/omabox/issues/286)).

### Changed

- **`omabox help CMD` is short**: the command's lines and its options, one line each (30 lines at
  most; `help up` 2 kB where it was 19 kB), ending with `more: omabox help CMD --long`, which
  prints the whole text as before: what each option does, defaults, limits. `omabox CMD --help` is
  the short form too. `omabox help` lists every command's lines (11 kB); `omabox help --long` is
  all of it, as `omabox help` was (60 kB)
  ([#269](https://github.com/diogochaves/omabox/issues/269)).
- `config --json` gives its keys in snake_case (`bar_icon`, `shot_fit`, `gpu_now`), as every other
  `--json` does; the bar widget reads them. `windows --json` is one line, as the others are.
- `omabox help CMD` names every option the command takes, aliases too: `--box`, `--output`,
  `--follow`, `--detach`, `-l`, `-w` and `--active` where they go, `scroll --no-raise`.
- **Exit codes mean one thing in every command**, in one table: `omabox help exit-codes` and the
  skill's reference ([#273](https://github.com/diogochaves/omabox/issues/273)). Called wrong (an
  unknown option or command, a value missing or not of its form, a word too many or too few) is
  exit **2** everywhere, where most commands gave 1; 1 stays "it ran and failed", 124 "not in
  time". Each such refusal names the nearest option, subcommand or command when one is close
  (`shot: unknown option --widnow (did you mean --window?)`), the form a missing value takes
  (`-g needs a value: -g "X,Y WxH"`), and `omabox help CMD`. A script that tested for 1 after a
  bad call should test for non-zero, or 2.
- **`omabox run` exits 125 for its own failures** (no box, a bad option, a throwaway that did not
  start, the box going down while the command ran, the broker out of reach from ai-jail), so they
  are never read as the command's status, which passes through as before (126 and 127: the command
  could not run, or was not found, in the box).
- Refusals that did not say what to do now do: a bad box name says what one is; a box's name where
  `windows`, `wait`, `record` and others take no word says it goes with `-b`; `click`, `drag` and
  `scroll` list what goes where a word was not understood; `keys` with nothing to send gives
  examples; `config`, `monitor remove`, `guard` and `events` say what goes there; a window that
  will not take focus is named, with the way round; `mode`, `gdb`, `peek --focus` and
  `restart-shell` say what to look at next; messages that lacked their command's name have it.
- `keys` refuses an unknown `--option` (exit 2) where it handed it to the keyboard tool (`--hold`
  there never returned).
- **One duration grammar for every option** ([#273](https://github.com/diogochaves/omabox/issues/273)):
  `300ms`, `2s`, `1.5m` and now `1h`; a bare number is seconds. `gpu`, `cpu` and `audio` take
  their length as one (`omabox gpu 500ms`, `cpu 1m --settle 2s`), where they took plain numbers
  only, and so does `$OMABOX_READY_TIMEOUT`.
- `--active` (`--window active`) goes with `keys`, `click`, `scroll`, `pointer`, `drag` and
  `wait` too; `check run`, `check point` and `check diff` take `--output` beside `-o`;
  `events --grep -i RE` ignores case, as `log --grep -i RE` does; `check point` refuses a bad
  `--timeout` before the scenario gets there.
- **`omabox log shell` lines carry their time**, to the millisecond (`2026-10-20 14:03:07.512
  WARN qml: ...`): the box's shell runs with Quickshell's `--log-times`, and the launcher's
  `logger -t omarchy-shell` lines have the same stamp. Timing what a shell plugin does (a retry's
  backoff) no longer takes `quickshell log --log-times` by hand; `--grep` and `-f` work as before
  ([#276](https://github.com/diogochaves/omabox/issues/276)).
- `record start`, `stop`, `status` and `sheet` refuse `--hold` (exit 2), which they ignored: it
  goes with `-- ACTION` or `mark`.
- `omabox help tree --long` and the skill's reference say what an egui/winit app needs for a tree
  on at-spi2-core 2.61 and later (Arch's now): accesskit_unix 0.22+ (eframe 0.37+). An older one
  waits only for a property those versions dropped, so it never joins the accessibility bus, and
  nothing `tree` sets helps; they no longer say `tree` turns it on
  ([#282](https://github.com/diogochaves/omabox/issues/282)).
- `test/run.sh` prints a long failure's first 100 and last 200 characters, where it printed its
  first 300 and lost the reason at the end, and keeps the newest 10 runs with a failure besides the
  newest 5 that passed: runs that pass no longer prune a failed run's evidence
  ([#285](https://github.com/diogochaves/omabox/issues/285)).
- **`omabox run -d` says a command that ended at once** (in its first 0.3 s, leaving nothing
  running) instead of "started": its exit status and its log's last lines, and it exits with that
  status (0 stays 0, said; with `-q`, not said). A server on a port already taken was "started" and
  never seen again. A job still running after 0.3 s is "started" as before; `--wait` keeps its own
  answer ([#286](https://github.com/diogochaves/omabox/issues/286)).

### Fixed

- A jailed agent's `shot --monitor NAME` took NAME for the output file, and the broker refused it
  ([#284](https://github.com/diogochaves/omabox/issues/284)).
- A jailed agent's one-action `record -- ACTION` (and `--for`) gives `--show-input`, `--captions`
  and `--cut-idle` to its stop, where they belong; its start refused them.

### Deprecated

- `omabox wait cmd` still works, with a warning naming `wait until`; 1.0 refuses it.
- Each of these still works, with a warning naming what to write instead, and 1.0 refuses it
  ([#273](https://github.com/diogochaves/omabox/issues/273)):
  - `up --idle 45` and `OMABOX_IDLE=45`, a bare number read as minutes: write `45m`.
  - `keys -s 300` and `pointer -- sleep 300`, a bare number read as milliseconds: write `300ms`.
  - `up -i`: write `--interactive` (`-i` is ignore-case in `log` and `events`).
  - `save -f`: write `--force` (`-f` is follow in `log` and `events`).
  - `shot` and `calibrate` given two files: the last was taken without a word; name one.
  - A box name given with characters a box name cannot have, or past 40 of them (`-b "my box"`):
    it was written to fit without a word (`my-box`), `up` aside; write the name that fits.
  - `keys --delay MS`, the keyboard tool's own time between keys, read only as the first word and
    never documented: for a pause, `-s DURATION`.
- With `--window`, every coordinate is the window's own (`click`'s X Y, `shot -g`, `check point`),
  except the regions `wait still|change --window` and `--wait` take beside it (`--ignore`, `-g`):
  those are still the screen's, now with a warning, and 1.0 takes them in the window's own. A script
  that wants the screen's watches with `-g` (the window's place) instead of `--window`.
- Fields that 1.0 drops, each with its successor beside it now: `config --json`'s kebab-case keys
  (`bar-icon`, `confirm-close-available`, `gpu-now`...), `ls --json`'s `allow` (a comma string; use
  `allowed`), `ports --json`'s `address` (use `addresses`).

## 0.8.0 — 2026-10-20

Visual checks: `omabox check` compares an app's or a plugin's look with a baseline, in an `up
--clean` box where only what is tested moves. `up --quickshell PATH` runs the box's shell on a
Quickshell build of yours, your fonts are in every box, and `click --window` says when a toast or
another layer covers the point it aims at. A config error no longer hangs a box, `reload` says when
Hyprland stopped answering, `record stop` no longer waits forever on a stuck ffmpeg, `tree` puts
what is inside a Qt Quick dialog where it is drawn, and `down` names your session's box when the
directory's name has none.

### Added

- **Visual checks: `omabox check`.** An app's or a plugin's look compared with a baseline, so
  nobody has to look at every screen. A scenario (a script of yours that drives the box and calls
  `omabox check point ID` where a shot should match) runs in a fresh clean box; `check run
  --approve -- SCENARIO` runs it three times and keeps what most runs agree on as the baseline,
  saying what was not the same from one run to the next (and leaving that out from then on); `check
  run -- SCENARIO` then says, for each checkpoint, same or changed: each region that changed, marked
  on a copy of the shot, and, for an app with an accessibility tree, which element moved, resized,
  changed its text or state, appeared or vanished (`moved -6,+0 button "Save"`), and one report
  image of every change, each cropped from the baseline beside the new shot and labelled, a fifth
  of the image tokens of looking at each changed shot or less. Shots are compared exactly, as users
  see them, leaving out only specks: stray parts smaller than what that checkpoint's approve runs
  differed by (twice their largest, 16 px at least; Qt Quick's text varies by a few pixels a glyph
  from one start of an app to the next), and under 8 px where the runs agreed to the pixel (the bar,
  most apps), so a 2 px spacing change in a bar widget or a vanished sort arrow is caught, and a
  colour a few levels off too; `same` says what it left out. `--approve` also writes one image of
  the whole baseline, what is left out marked, to look at before keeping it (what most runs showed
  is kept, a wrong screen too). `check point --window SEL -g "X,Y WxH"` checks a part of a window,
  in its coordinates, with the tree of that part. The last line counts same, changed, new and
  missing checkpoints, each run says how long its box took to start, and a check with no baseline
  exits 4; one called wrongly or that cannot run (an unknown option, a missing file, no box, no
  magick) exits 2, never 1, its "changed". Checkpoints are shot full size whatever your `shot-fit`,
  so a baseline is the same image on every machine, and `--ignore "X,Y WxH"` is in `-g`'s
  coordinates: the window's with `--window`, else the screen's. Each checkpoint is searched for holes too: the clean wallpaper showing through inside a
  window, the bar or a popup (a transparent part, a gap, a surface that did not draw), not around
  them or at rounded corners; `--approve` says each, and a check fails on any the baseline lacks.
  `check diff A.png B.png` compares two shots of yours the same way (`--report` too).
- **`up --clean`: a desktop where only what is tested moves.** Omarchy's own bar on tokyo-night, a
  solid `#ff00ff` wallpaper (any of it inside a window or the bar is a hole), the bar's clock frozen
  at "Friday 12:00" by its format (in its place, at its width; nothing is preloaded into the box's
  processes, which keep the real clock: seed an app's dates relative to now, `--ignore` a time it
  shows), English (en_US.UTF-8, or C.UTF-8) in UTC for the whole session whatever your locale and
  timezone (a Portuguese LC_TIME made an app's dates "Há 9 meses"), no pointer, no network, none of
  your fonts and no animations (over reloads too). `--keep
  animations,bar,clock,fonts,locale,pointer,wallpaper` takes any of them back, for a test of Hyprland or the shell that needs them; `--theme` and `--net connected`
  choose their own. `run` takes it for a throwaway box too.
- **`up --quickshell PATH`: the box's shell on a Quickshell build of yours** (and `run --quickshell
  PATH` for a throwaway), for a Quickshell fix your installed package does not have yet, such as
  0.3.2's accessibility tree. The box's shell, `restart-shell`, Omarchy's own restart and anything
  calling `quickshell`/`qs` in the box run the build; its folder is mounted read-only. Quickshell
  uses Qt's private API, so a build against another Qt than the installed one is refused (naming
  both versions), as is one needing a library the box lacks. `ls`, `ls --json`, `windows` and
  `save` name it. Every box's `ls --json` now has `quickshell_version`; it is the last release's
  number even for a build of a newer commit, so it is no proof that a fix is in.
- **Your fonts are in every box.** `~/.local/share/fonts` and `~/.fonts` are mounted read-only at
  the same place in the box HOME, so a theme's or a plugin's own faces draw as on your desktop:
  without them the box drew them in a fallback family, silently, and shots looked plausible but
  wrong. `up` says it; `ls --json` has `desk_fonts`. `--no-seed fonts` leaves them out; an `up
  --clean` box has none (Omarchy's look), `--keep fonts` brings them. A box HOME with its own folder
  there (a `--from` save's, a `--seed` into it) keeps it, said, and a link in them that leads out of
  them is said too: it leads nowhere in the box.

### Fixed

- **`click --window` over a notification toast says so.** A layer surface (a notification toast, a
  panel, the bar over a float) over the point a `click`, `scroll`, `drag` or `pointer` `--window`
  aims at takes the input there, and nothing said it: the click went to a toast and the app seemed
  flaky. Now it is said on stderr, naming the layer, and `--no-raise` refuses (raising the window
  would not help); `windows` lists the layers over windows under its table, and `--json` has each
  window's `layers`. Only a layer's rectangle is known, not where it takes input: while the Omarchy
  shell shows a toast, its full-screen layer is named over every point. `omabox wait layer
  omarchy-notifications --gone` waits toasts out.

- **A config error no longer hangs a box.** Under load, `hyprctl reload` in a box whose config had
  an error could freeze its Hyprland for good while it drew the red "Your config has errors" bar (a
  Hyprland deadlock in that bar's text). A box now draws no such bar, even when your
  `looknfeel.lua` asks for it; `up`, `reload` and `hyprctl reload` still print the errors, and
  `omabox hyprctl configerrors` lists them.
- **`reload` and `hyprctl reload` say when the box's Hyprland stopped answering after the reload.**
  They printed nothing more when Hyprland answered the reload and then hung, which read as a clean
  reload. Now they say it did not answer (with `omabox gdb` and `omabox down` to go on) and exit 1;
  so does `up`, which leaves the box up.
- **`logger` reaches the box's journal by name.** 0.7.0 said `logger --journald` lands in `omabox
  log journal`, but in a box `logger` is omabox's own stand-in, which wrote `~/logger.log`, so the
  journal stayed empty unless you typed `/usr/bin/logger`. Now `logger --journald` and a plain
  `logger [-t TAG] MESSAGE` (or its stdin's lines) go to the journal, as on your desktop; `logger -t
  omarchy-shell`, the shell launcher's, still goes to `omabox log shell`. `systemd-cat` by its bare
  name still writes `~/IDENTIFIER.log` (the box's shell relies on it): use `/usr/bin/systemd-cat`
  for the journal, as the help now says.
- **`up` says when the box's bar is your desk's, not Omarchy's.** A box copies your bar's layout
  and `shell.toml` (its sizes and fonts), and `--theme` leaves both as they are, so a shot or a
  measured icon size taken "on a stock theme" came out of your bar without a word. Now `up` says
  "seeded the bar from your desk: its shell.json layout and shell.toml [font], [bar] (--stock-bar
  starts from Omarchy's default)" when they differ from Omarchy's (names only), and the skill's row
  for theme-dependent results says `--theme NAME --stock-bar`. `--stock-bar` from a save now drops
  the save's `shell.toml` too.
- **`run` says when the command runs in the box HOME because the box lacks your current dir.**
  Outside a repo a throwaway `run` mounts nothing of the current dir, so `omabox run -- python3
  tests/x.py` from a scratch copy failed with "can't open file '/home/sbx/tests/x.py'", which read as
  a wrong path. When a word of the command (or of an `sh -c` script) is a path in your current dir
  and the box does not have that dir, `run` now says so first, with the mount that brings it in
  (`run --overlay DIR`, or `--ro-bind DIR`; for a box that is up, `down`, then `up` with it). A plain
  `run -- app` says nothing.
- **`record stop` no longer waits forever on a stuck ffmpeg.** ffmpeg 9.0.1 now and then deadlocks
  (about 1 run in 100 of the same encode), and `record stop` (or `record sheet`) then hung. Each
  ffmpeg and ffprobe step a recording runs now has a time limit, 60 s and 5 s more for each second
  of 1080p at 30 fps it recorded (so a long recording still has the time to encode); one that runs
  out is killed and run once more, and a second time `stop` says so and fails, keeping the raw
  recording in the box as its other failures do. `omabox audio`'s ffmpeg is bounded too.
- **`tree` puts what is inside a Qt Quick dialog where it is drawn.** A Qt Quick popup with the
  dialog role (a settings or add sheet) read `@0,0`, and everything in it relative to the sheet, not
  the window (Qt measures from it), so a click at a row's middle landed on what was behind the sheet,
  and `check point --window SEL -g` on a sheet compared the elements behind it instead of its own.
- **`check`'s `--ignore` holds for the tree too.** A region left out of a checkpoint (a time the app
  shows) still changed its tree, so `--approve` saw every run differ and named the wrong cause for an
  app whose layout moved in one start of three, and a check a minute later read changed. An element
  wholly inside an `--ignore` region is now left out of the tree's comparison and the report; an
  `unstable` tree line says the element's box, so the region can be made to hold it.
- **`baseline.png` stays readable with many checkpoints.** It fitted every checkpoint into one image,
  so 28 of them came out 40-70 px wide each. Each shot now keeps a size you can read a screen at
  (about 400x225 for a window), and what does not fit one look goes on more pages, `baseline-2.png`
  and on, all named when `--approve` ends.
- **One change seen in many checkpoints is said once.** A margin that moved the same rows in 17
  checkpoints of one sheet listed its regions and tree lines 17 times, and the report showed the same
  card until it ran out of room. A checkpoint whose change an earlier one had (the same tree changes
  and no other pixels; without a tree, the same regions) is now one line, `changed  ID: as FIRST`,
  still counted as changed (`18 changed (16 as another)`), and the report names it on the first one's
  card.
- **`tree` prints no warning on every call with at-spi2-core 2.61 or later.** That version dropped
  `org.a11y.Status.ScreenReaderEnabled`, and each `tree` said it could not set it, a line that broke
  `--find`'s one-line answer and `--json` read with its errors. `tree` now leaves it alone where it
  is gone and says why only when an app is not on the accessibility bus: an egui/winit app on
  AccessKit before accesskit_unix 0.22 waits for that property, so it has no tree there. libatspi's
  warning about such an app's missing cache, which `tree` drops, is dropped under libatspi's new log
  name too.
- **A plugin that loaded is no longer said to have a warning on Quickshell 0.3.2.** That version
  warns about every plugin folder outside the shell's config folder, as all of Omarchy's are, and
  `up` and `restart-shell` printed it as each plugin's own warning ("plugin ID loaded, with a
  warning in the shell log: Module path ... is outside of the config folder"). That message is left
  out now; any other line naming the plugin still shows.
- **Omarchy's idle-spawn test passes in a box.** The box's stand-in for the plugin registry's
  `inotifywait` started a `sleep 5` every 5 s for as long as the shell ran, so Omarchy's own "an
  idle shell starts no processes" test failed in every box (`6 sleep 5`). It now waits on its
  stdin, which the shell holds and never writes, and starts nothing; it still ends with the shell.
- **`down` removes a box's HOME even when an app in the box left a read-only folder in it.** The
  HOME (`~/.cache/omabox/BOX`) stayed behind, `down` printed only rm's "Permission denied", and the
  next box under that name started with its leftovers. A HOME that still cannot be removed is now
  said on stderr, by `down` and by the clean-up at each `up`.
- **`run -d --replace` with other arguments names the job it leaves running.** `--replace` stops
  only the job started with the same command and arguments, so relaunching `./build/app --demo
  table` after `./build/app --demo details` left both running, while the note said "no earlier job
  of this command". It now says which job of that program still runs and how to deal with it:
  `--replace: no earlier job with these arguments; ./build/app (pid 115, --demo details) still runs:
  stop it, or --replace with its arguments`. The skill's line says "the same command and arguments".
- **`omabox down` names your session's box when the directory's name has none.** A box is named
  after the directory a command runs in, so a box started in a scratch folder and a `down` run later
  in a worktree said only "no box '…'" and exited 0, leaving the box up. When your session has boxes
  up under other names, `down` now names each, where it was started and the command that takes it
  down (`this session has 'default-…' (started in ~/…): omabox down -b default-…`), and exits 1.

## 0.7.0 — 2026-10-16

Hold keys across commands, a journal in every box, CPU per thread, and an opt-in skill for
reporting what omabox got wrong. Print no longer kills a box on NVIDIA, `tree` reads egui apps, a
box loads your Hyprland bindings and input settings, and a crash says it crashed, with the signal.
A box refuses the coding agents' and the browsers' own stores, as it does other secrets. Includes
0.6.2's security fix. The guard's hooks changed and now say when an update changes them: run
`omabox guard on` (or `omabox setup`) once.

### Added

- **`keys down:KEY` and `keys up:KEY` hold a key down across other commands.** `keys` only tapped,
  so a bind on a key's release, or anything that happens while a key is held, could not be tested:
  now `omabox keys down:XF86Tools`, then `omabox pointer move …`, then `omabox keys up:XF86Tools`
  fires the press bind, then the release bind, as a held key does. A held key joins the others as on
  a real keyboard (`down:super`, then `keys a` is SUPER+A; `down:shift` types capitals), and a held
  letter repeats in the app until let go. Pressing a held key again, or letting go of one that is not
  held, is an error, before any key of that call is sent; anything still held goes with the box, and
  a key whose holder was killed outright is let go, once, by the next `keys`.

- **A box has a journal: `omabox log journal`.** A program that logs only to the journal failed in a
  box: a test harness that starts its own accessibility bus from a cleaned environment got
  "at-spi-bus-launcher exited with 1", because the dbus-broker it launches exits when it cannot
  reach `/run/systemd/journal/socket`. Every box now has its own journal sockets there (the native
  one and the stdout stream; never your host's), and what comes in goes to `omabox log journal`, one
  line per message with the sender's name and pid: dbus-broker, `logger --journald`, a `--systemd`
  box's units' output. The log keeps at most 4 MB, then starts again (the one before as
  `journal.1.log`). Still no journald: `journalctl` reads nothing in a box.

- **`omabox-feedback`, an opt-in skill for reporting what omabox got wrong.** Off unless you run
  `omabox setup --feedback-skill` (`--no-feedback-skill` takes it out; a plain `omabox setup` never
  adds it). Then `/omabox-feedback` has your agent look back through the session (in Claude Code,
  every subagent's transcript too) for omabox failures, refusals, workarounds, missing commands and
  misleading docs, check them against the issues, and file them on your yes, after a short list you
  can ask to see in full or change. Your paths, names, hosts, tokens, projects and the session's
  content are taken out of every draft first; a safety problem is never filed, only pointed to
  SECURITY.md. It runs only when you ask; `/omabox-feedback auto` files without the question.
- **`--window` takes a hyphen-separated part of the class.** `--window portal` (or `gtk`) finds a
  window of class `xdg-desktop-portal-gtk` when nothing matches the word as a whole class, a last
  dotted part or part of a title; those still win, and a word matching several windows is still
  refused with each one listed.
- **`up` says when a plugin under test gets your desktop's settings, and `--no-seed extensions`
  starts without them.** A box gets a copy of your `~/.config/omarchy/extensions`, where shell
  plugins keep their settings (an on/off switch for their key binds, say). A `--plugin` whose switch
  was off on the desktop bound nothing in the box, and the test failed with only a timeout. Now `up`
  prints "seeded extensions/NAME.json from your desk: plugin ID reads it" for each such file, and
  `--no-seed extensions` gives the box a new Omarchy user's `extensions` instead. The skill now lists
  everything `up` copies from your desktop.
- **`up` and `run` say when a path in the repo links out of it, to what the box does not have.** A
  build cache that turns `target/` into a link to `~/.cache/…` left it dangling in the box (your HOME
  is not mounted), and `run -- target/debug/app` said only "No such file or directory". Now `up` names
  each such link at the top of the repo, and `run` each argument that goes through one, with the
  `--ro-bind` that mounts its target when that is a cache, outside your HOME or in no hidden folder of
  it (a repo's link may lead anywhere: elsewhere no mount is offered).
- **A box refuses the coding agents' and the browsers' own stores**, as it does the other secret
  stores: `~/.claude`, `~/.codex`, OpenCode's, pi's, Hermes' and Gemini's, the browser profiles
  (`~/.mozilla`, Chromium, Chrome, Brave, Vivaldi, Edge) and `~/.pki`, whether by `--ro-bind`,
  `--seed`, `--overlay` or a plugin.
- **`cpu --threads`: CPU per thread.** With `--app NAME` or `--pid PID`, each of the app's threads
  is listed under it by name, over the same window: a Qt Quick app's main thread apart from its
  `QSGRenderThread`, each thread of a pool on its own row. `--json` gives them as
  `processes[].threads[]` (`tid` as the box numbers it, `host_tid`, `name`, `cpu`). Without `--app`
  or `--pid` it is refused: every process's threads would bury the app's.

- **The guard's hooks say when an omabox update changed them.** What `omabox guard on` writes into
  Claude Code's and Codex's settings changes now and then, and the old hook kept working, so nobody
  was told: only `omabox guard` read it as outdated. Now each hook carries a stamp, and at session
  start, when its omabox ships another, it adds one line to its note: "this hook is outdated (omabox
  changed it): tell the user to run omabox guard on to renew it". This version changes both hooks
  itself, so run `omabox guard on` (or `omabox setup`) once after updating: the last time a change to
  them goes unannounced.

### Fixed

- **`omabox tree` reads egui and other winit (AccessKit) apps.** An egui app on eframe 0.36 or older
  never joined the box's accessibility bus: its AccessKit waits for a screen reader
  (`ScreenReaderEnabled`), and `tree` only turned on what Qt waits for. `tree` now sets both, and
  libatspi's "Unknown object '/org/a11y/atspi/cache'" warning, printed after every such app's tree,
  is gone.
- **Print (Omarchy's screenshot) no longer kills a box on NVIDIA.** The screenshot turns hardware
  cursors on while it runs, and Hyprland then wrote the cursor into a buffer NVIDIA cannot map: the
  box's Hyprland crashed at the next cursor change (the selection going away, a pointer move). A box
  now draws hardware cursors on the GPU; nothing changes on other GPUs.
- **A box loads `~/.config/hypr/bindings.lua`, `input.lua` and `looknfeel.lua` from its HOME**, as
  Omarchy does. An edit there, then `omabox hyprctl reload`, did nothing: a box never loaded them, so
  a plugin's "add this line to `bindings.lua`" (or its own Set up button) bound nothing, and the
  reload still answered `ok`. A new box's are Omarchy's comment-only files, so nothing changes until
  you write to them. `monitors.lua` and `autostart.lua` stay unloaded: the box's screen is the one
  `--size`, `--monitor` and `mode` give it, and nothing autostarts. A file with an error does not
  stop the rest of the config loading, and `up`, `reload` and `hyprctl reload` now print the
  config's errors (what the red bar at the top of the box's screen says) instead of answering only
  `ok`. A box started before this update loads them after `omabox reload`.

- **`run -- wl-copy TEXT` returns.** A command that left a process running (`wl-copy`'s server, a
  `daemon &`) kept `run`'s output open, so `$(omabox run -- wl-copy TEXT)`, or a run whose output went
  to a pipe, waited until that process ended, and every `run` writing to a file paid a second for it.
  Now `run` returns as soon as its command exits, with its status and all of its output; what the
  process it left writes after that is dropped (that process keeps running). For a process you want
  to keep the output of, use `run -d` and its log.
- **A call that crashes the box's Hyprland says it crashed, with the signal.** `omabox lua` (and
  `hyprctl`, `windows`, `shot` and the rest) said "its Hyprland did not answer (hung?)" when Hyprland
  had just segfaulted. Now: "went down: its Hyprland crashed (SIGSEGV)", with how to get a backtrace
  (`omabox gdb --watch` before the call, in a new box: boxes keep no core dumps), or where it is
  when one was watching (`omabox log gdb --crash`). A later call to that box says it too.
- **`omabox log gdb` puts the thread a crash hit first.** After `gdb --watch` caught a fatal
  signal, every thread's backtrace followed it with Hyprland's main thread last, after its worker
  threads. The thread the signal hit now comes right under the signal line, then every thread; `omabox
  log gdb --crash` prints only that thread's backtrace for each crash.
- **`shot -g` past the window or the screen is cut to it.** `shot --window SEL -g` a few pixels past
  the window was refused ("not inside window"), and `-g` past the screen gave an image padded with
  transparent pixels. Both now take the part inside and say so on stderr, by edge (`-g clipped to
  1380,40 474x330 of window …: 6 px past the right edge`), exit 0; only a region with nothing
  inside is refused.

- **`omabox ls --json` has the Hyprland version of every box.** `hyprland_version` was null for a
  box on the installed Hyprland, the usual one; it was recorded only with `up --hyprland`. Now it is
  the first line of the box's own `hyprctl version` for each box `up` starts. The skill names the
  keys to report from (`omarchy_version`, `hyprland_version`).
- **`omabox ls` refuses arguments it does not take.** `ls -b NAME --json` printed the table and
  exited 0, as did `ls` with any unknown flag. Now `-b` after `ls` says "ls takes no -b", as before
  it, and anything else but `--json` is an unknown argument; both exit 2. `omabox saves` too:
  `saves --json --bogus` printed the JSON and exited 0. And `omabox --version` (`version`, `-V`):
  `version foo` printed the version and exited 0.
- **`omabox tree` no longer prints libatspi's warnings after a tree it read.** A warning about
  another app on the box's accessibility bus came out as a stray `omabox: tree:` line; now they show
  only when `tree` fails.
- **`omabox ls` no longer calls a box whose Hyprland just crashed `hung`.** In the moment before the
  box goes down it says `crashed` (or `going`), in `ls --json`'s `state` too. An interactive box's
  crash now names its signal at once, as a headless box's does.
- **The skill shows `wait change` again**, for an app that redraws later by itself (a sync, a
  timer): `omabox wait change --window APP`, then `wait still`. Without it agents slept with
  `wait cmd -- false`. `omabox help wait` says what `change` waits for.
- **The guard no longer piles its line into Claude Code's session environment.** Its hook added the
  line again on every resume and compaction, so a long session sourced it many times before each
  command. It is written once now, an older omabox's replaced. `omabox guard` reads the hook as
  outdated: `omabox guard on` renews it.

## 0.6.2 — 2026-10-16

A security fix over 0.6.1, and nothing else. A process in a box could write into what omabox printed
of nsenter's own messages on a call into the box (any amount, control characters included), and
reach files the caller of `omabox gdb` or `restart-shell` had open above fd 2 through omabox's own
steps in the box. Update.

### Fixed

- **nsenter's own messages are read through a bounded pipe and printed only when they look like
  nsenter's.** What nsenter (and the shell it starts in the box) said before a command's own stderr
  took over was held in a temporary file and printed whole; it is now held in a pipe, read up to
  4 KiB, and only nsenter's own lines, without control characters, are printed; anything else is
  dropped and counted. `gdb`'s and `restart-shell`'s own steps in a box now get only the files they
  are meant to have, as `run`'s commands do, not every one omabox had open.

## 0.6.1 — 2026-10-14

Security fixes, and a fix pass over 0.6.0. A process in a box could reach the host files you gave
`omabox run` as stdin, stdout or stderr, your terminal, and other files the caller had open,
through the box's own view of `/proc`; omabox's own helpers in a box ran from the box's PATH; and
a jailed agent's `shot --to-fd` took its own `/proc` files. Update. Also `omabox record` for agents
inside ai-jail, and four fixes from a review of 0.6.0's new features.

### Added

- **Videos people can follow: `record stop --show-input`, `--captions`, `--cut-idle`, `--glide`.**
  `--show-input` draws what omabox's clicks, pointer moves, drags and keys did into the video, as
  `peek` shows them, at the times they happened: a ring over the pointer, a ripple from each click,
  key captions at the bottom (a `--pass` as `*`). `--captions` puts the `record mark` texts at the
  top. `--cut-idle [1s]` cuts every still stretch (no change on screen, no input, no mark, no sound)
  down to that, and moves the marks and captions with it. `record start --glide 600ms` (or `--glide`
  on `click`, `pointer`, `drag`) makes the pointer travel there in that time instead of jumping, while
  agents' clicks stay instant otherwise (#176).

- **`omabox record` inside ai-jail.** A jailed agent's `record start -o demo.mp4`, `mark`, `stop`,
  `record -- ACTION` and `record sheet` work as outside: the video, its marks, captions and contact
  sheet land in the jail's project, written there by the agent's own `omabox`; the broker writes
  and reads them only through the files it was handed, never at a path the jail names (#224). A
  video is at most 16 GiB (an hour of 4K): `record stop` says so and keeps a larger one in the box.

### Fixed

- `record --no-pointer -- ACTION` ended by Ctrl-C while the action ran showed the pointer again
  although the recording went on; it stays hidden until `record stop`, as a plain `start`'s does.
- `--show-input` drew the pointer's ring 40 ms early for each scroll or drag pause in a line.
- `up --desk-shell` kept a plugin's config out of the box when a setting's name only contained a
  word like token or password (`"maxTokens": 4`); only a key of that name with a value is a secret.
- `omabox tree --find` paths lost an ancestor past 64 printed levels.
- **What omabox runs in a box for itself is the system's.** Its own steps there (recording, the
  clipboard, `up`'s checks) run the box's `/usr/bin` tools by their paths, not what the box's own
  PATH would pick, and what omabox reads back from a box is bounded on its side. A command in a box
  (`omabox run`) gets stdin, stdout and stderr, and no other file the caller has open.
- **A box command's stdin, stdout and stderr are never your files.** They reach the box as pipes
  held on the host (a terminal of its own when you run a command from one), so no process in the
  box can open them again through `/proc` and read or write a host file you redirected `omabox run`
  to, or your terminal. Since 0.1.0 it could, while the command ran. A command run from a terminal
  with its output elsewhere (`omabox run -- cmd > log`, `| less`) no longer reads the terminal.

## 0.6.0 — 2026-10-14

Agents can read a window as text (`omabox tree`), check how their model reads click points
(`omabox calibrate`), and make videos with sound, marks and every monitor (`up --audio`, `record
mark`, `record --monitor all`, `--no-pointer`). A box can mirror your whole shell (`up
--desk-shell`), `ls` names boxes nobody will take down (`down --orphans`), and fast `shot --burst`es
no longer stall what they record. Fixes for scaled and turned screens, video colours, and the Codex
guard. Packages: two new tools, `omabox-burst` (libpng) and `omabox-tree` (at-spi2-core, glib2).

### Added

- **Text instead of a shot: `omabox tree`.** A window's accessibility tree as compact text: each
  control's role, name, text, value and state (checked, focused, selected...) with its bounds in the
  coordinates `click --window` takes. Ask first: `omabox tree --find 'dark mode'` prints the one
  line that answers "is Dark mode checked?" (~17 tokens against a shot's ~1.5k), and the middle of
  its bounds is where to click. For GTK and Qt apps (`tree` turns Qt's tree on), and Chromium or
  Electron started with `--force-renderer-accessibility`; a terminal or the Omarchy shell says "no
  accessibility tree" (#178).
- **`omabox calibrate`: does this model need `--fit` to click right?** It draws a test card the size
  of a full shot of the box, six lettered crosses at random points; the agent reads it as it reads a
  shot and answers where it sees them (`omabox calibrate --answer "A X,Y B X,Y ..."`). omabox fits the
  readings and says whether full-size shots read 1:1, else the `--fit N` whose shots, with `click
  --in`, land within a few pixels, and the `omabox config shot-fit N` to suggest to you (it never sets
  it), or that no one scale fits (read again). For any model and harness, without knowing either
  (#188).
- **Your desk's whole shell in a box: `omabox up --desk-shell`.** Every plugin in
  `~/.config/omarchy/plugins` (read-only, as `--plugin` mounts one; a `--plugin` of the same id wins,
  for your working copy), your `shell.json` as it is (only idle held off), and the other files in
  `~/.config/omarchy` your plugins read, such as a plugin's JSON or a folder of profiles, copied into
  the box. Never `api-keys.env` nor anything named or holding a key, token, secret or password,
  your hooks, a git dir or a backup; regular files only, with a size cap, and a link only to a file
  inside the folder. `up` lists what went in and what did not, and why (`ls --json`: `desk_shell`),
  so a widget can be checked against the bar you really have (#206).
- **Sound in a box, and in its videos.** `omabox up --audio` gives a box its own PipeWire (Pulse
  apps too) with a speaker nobody hears as its only output and a silent microphone: an app's sounds
  play, and never reach your speakers or your audio server. `omabox audio 3` listens for three
  seconds and says whether the box made a sound, when and how loud (`--json` for every 100 ms), so
  an agent can check a notification sound or a player without hearing it. `omabox record start
  --audio` puts the sound in the video (AAC), in step with the picture to about 15 ms, with
  `--monitor all` too (#226).
- **A video of every monitor at once.** `omabox record start --monitor all` (and `record --monitor
  all -- ACTION`) records a box's whole layout as one video, laid out as `omabox shot` takes the
  screen: each monitor at its place, a lower-scale or turned one as it is shown, the gaps black, the
  video the size of the shot. The monitors are kept in step by their own frame times, so something
  drawn on two of them at once shows in the same frame; marks and contact sheets work as for one
  monitor. A layout too big for H.264 is scaled down to fit, and said (#225).
- **Marks and contact sheets for `omabox record`.** `omabox record mark "scene 2"`, while a
  recording runs, notes the text at that moment in the video's own clock (milliseconds from its
  first frame, exact to a few ms), and `record stop` writes them beside the video: `demo.marks.tsv`
  and `demo.vtt`, captions any player loads or `ffmpeg -vf subtitles=demo.vtt` burns in. They also
  show where a script of omabox calls and sleeps drifts from its plan. `record stop --sheet` (or
  `omabox record sheet demo.mp4`) makes `demo.sheet.png`, a contact sheet of the video's frames with
  their times and the marks under them (`--frames N`, `--every 2s`, `--fit N`), so an agent can check
  a take without watching it (#176).
- **Shots and videos without the pointer.** `shot --no-pointer` and `record --no-pointer` leave the
  pointer out, for stills and videos meant for people: no arrow on the app, and none parked on
  something with a hover state. It is shown again afterwards, as it was, also when the shot or
  recording fails or is stopped (#228). `omabox pointer hide` (or `up --no-pointer`) hides it in a
  box until `omabox pointer show`, and it stays hidden over config reloads and theme switches, so a
  series of stills across themes needs it once. Hidden, the pointer still moves, clicks and gives
  focus as before.
- **`ls` tells a box nobody will take down.** A headless box at `--idle 0` with no agent session to
  go with has nothing to end it, and when the script or agent that started it was gone it stayed up
  for good, listed like any box in use (#220). `up` now records what started a box; `ls` says
  `orphan` under IDLE for such a box once that is gone (`ls --json`: `orphan`, and `starter`), the bar
  widget says it on the box's row, and `omabox down --orphans` takes those down, naming each. Nothing
  takes one down by itself, and a box you start at a terminal is never one. `up --idle 0` says, to a
  caller that is not a terminal, that nothing will take the box down but `omabox down`.
- **Addresses of its own for an isolated box.** `up --net isolated --addr 203.0.113.10 --addr
  203.0.113.11` puts those addresses on the box's loopback, for apps that judge their network by
  where a peer comes from: a BitTorrent client reads "firewalled" until a peer connects from a
  non-local address, and in an isolated box every peer was on 127.0.0.1. A server on one sees a
  client bound to another come from there. Nothing in the box could add one itself. Documentation
  ranges only (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32), so they never stand for
  a real host; `--allow` forwards are unchanged. `ls` lists them under the box, `ls --json` as
  `addrs` (#229).

### Fixed

- **`omabox shot --burst` no longer holds up what it records.** Each frame was a grim of its own, and
  each grim four Hyprland events: a shell or widget that works on every event (as widgets do to refresh) was
  held up by a fast burst, so its timers and animations stalled in the very frames meant to show them.
  A burst is now one capture client in the box for all its frames, four events in all, so `--every 0`
  (back to back, ~15 ms a crop) shows an animation as it runs. Frames are the same images `omabox shot`
  takes. A box started before this update still takes a frame at a time until it is started again
  (#196).
- **`omabox record`'s colours.** Every video came out washed out: pure green as #0cea0d, white as
  #e8e8e8, black as #101010. wf-recorder wrote its frames' values in TV range (16-235) but marked them
  full range, and every player believed the mark. Videos now come out in the standard form players
  and browsers all read the same (H.264 4:2:0, limited range, BT.709, and marked so), a pure colour
  within 2 levels of the screen's; `--monitor all` and contact sheets alike. Videos made before keep
  their muted colours.
- **Turned (portrait) monitors.** A monitor turned by its transform (`hl.monitor({ ..., transform =
  1 })`) was taken at its unturned size: a 1280x720 one turned upright counted as 1280 wide and 720
  high. The screen's size was wrong (`shot` said 2560x1080 of a 2000x1280 layout), so `click`,
  `pointer move` and `pixel` were refused or landed elsewhere below its old height, and `shot
  --monitor` of it took the wrong part, partly black. Every monitor's size in the layout now follows
  its transform: `shot`, `shot --monitor`, `--fit`, `click --in`, `pointer`, `pixel` and an
  interactive box's windows on a turned host monitor.
- **A `run` killed while its throwaway box starts takes the box with it at once.** Killed outright
  (SIGKILL, a harness's timeout) before its box was ready, the box's `up` went on to the end, waiting
  for the bar, and the box went only once its reaper got the box's lock after that: seconds later
  under load. The `up` now notices its run is gone and ends the box itself (#222).
- **`omabox gpu` exits 0 when no process has GPU figures.** On a GPU whose driver keeps no
  per-process counters (a VM's virtio GPU) it printed its header and ended with 1, before saying
  "(no GPU time)". It says so now, and exits 0.
- **`shot --fit N` on a scaled screen.** It compared N with the layout's size, not the image's: at
  `--size 2560x1440,scale=2`, `--fit 2000` gave the full 2560x1440, and a window's shot came out at
  twice N. The longest side is N now on any screen, window, region or mix of scales.
- **`shot --window -g` and `pixel --window` on a scaled monitor.** They read the window's buffer
  pixels as its own coordinates: at scale 2, `-g "10,10 300x200"` took the window's 150x100 at 5,5,
  `click --in` that shot missed by the scale, and `pixel --window X Y` read X/2,Y/2. Both take the
  window's own coordinates now, as the help says.
- **The bar widget's mark keeps its size beside the box count.** On Omarchy with fitted bar icons
  (after 4.0.4) the count was scaled down with the mark, and the mark shrank from 12 px to about
  7 px as soon as two boxes were up. The count is drawn beside the mark now; on 4.0.4 the widget
  looks as it did, except that the mark no longer moves up a pixel when the count shows.
- **A bare "shot: grim failed" says how grim ended** when grim gives no reason of its own: an empty
  image, nothing in 10 s, or the status it ended with. A shot grim gives an empty image for, which
  happens now and then under load, is taken once more.
- **The Codex guard works with your own `shell_environment_policy.set`.** `guard on` refused a
  `~/.codex/config.toml` that already had one, and told you to add its keys by hand, after which
  `omabox guard` said `outdated` forever. It now adds its keys to your table between its markers, and
  `guard off` takes out only those. Keys you merged by hand read as on, and `guard on` marks them.
  Your own value for one of the guard's keys is never overwritten: `guard on` says which one.

## 0.5.5 — 2026-10-14

Sharp, high-resolution screenshots and videos of a box: a scale for the main screen (`--size
3840x2160,scale=2`) and a new `omabox record`. On NVIDIA, apps in a box now draw on the GPU instead of
in software, and `omabox gpu` gives per-process figures there.

### Added

- **A scale for the main screen**: `up --size 3840x2160,scale=2` and `omabox mode 3840x2160,scale=2`.
  The box lays out as 1920x1080, with the bar and apps at their usual size drawn in twice the pixels,
  so a `shot` is 3840x2160 and stays sharp when you zoom or crop it. Scale 1 to 4, as `--monitor`'s;
  coordinates stay the layout's, and `click --in` maps a shot's. Headless boxes only.
- **`omabox record`: a video of a box's screen.** `record start [-o FILE] [--monitor NAME] [--fps 30]`,
  then `record stop`, which prints the file: an H.264 MP4 at a steady frame rate, at the screen's
  own pixels (4K from a box at `--size 3840x2160,scale=2`). `record -o FILE -- ACTION` records one
  action (`keys super+space`, `click X Y`, ...). Needs wf-recorder and ffmpeg. Not yet for agents
  inside ai-jail (#224).
- **`omabox gpu` on NVIDIA.** NVIDIA's driver reports no per-process DRM counters, so `gpu` listed
  nothing there. It now takes the driver's own per-process sampling through `nvidia-smi pmon`, for
  the box's processes only: whole seconds, each process's share of the GPU as sampled once a second
  (`--json` has `source`).

### Fixed

- **Apps in a box on an NVIDIA GPU draw on the GPU** (#204). They fell back to Mesa's software
  renderer (llvmpipe): NVIDIA's Wayland EGL looks for the GPU's card node, which a box never has,
  and gave up without trying the render node the box does have. An NVIDIA box now has an empty
  read-only file at that path (never the device), and its apps and bar render on the GPU. Vulkan in
  an NVIDIA box is unchanged (`vulkaninfo` still crashes).

## 0.5.4 — 2026-10-13

A security fix for agents inside ai-jail driving boxes through the broker: update if you use
`omabox broker on`. A jailed agent's box now has its project even when the jail hides files in it.
Also new: back and forward mouse buttons, and `up`, `ls`, `gpu` and `cpu` say when a box's apps
render in software.

### Added

- **Back and forward mouse buttons** (#216). `click X Y back`, `forward` (also `side`, `extra`), in
  `drag` and `pointer -- click|down|up BTN` too: a mouse's thumb buttons, which apps take as back
  and forward.
- **A 0x0 output on NVIDIA is said** (#213). In a box on NVIDIA an output the app makes itself
  (`hyprctl output create headless`) comes up 0x0 with no error. `monitor list` and `shot` now say
  so, why, and which box to test it in instead (one on your AMD/Intel GPU, `OMABOX_RENDER_NODE`).
- **Apps rendering in software are said** (#204). In a box on NVIDIA, apps get Mesa's software
  renderer (llvmpipe), so their CPU and frame cost there is not the GPU's. `up` now says so once,
  with a box on your other GPU to use instead; `ls`, `gpu` and `cpu` show an `apps:` line, and
  `ls --json` a `client_render` record (from `eglinfo`, when mesa-utils is installed).

### Fixed

- **A jailed agent's box has its project even when the jail hides files in it** (#210). An agent in
  ai-jail whose project had a path the jail hides (an `ai-jail --mask`, or the project's own
  `.ai-jail` file, which ai-jail hides by default) got a box without its project: `run` started in
  the box HOME. Now the project is mounted with those paths hidden in the box as in the jail (empty),
  checked before the box starts. So are `--ro-bind` of such a folder and a git worktree's main `.git`
  with a hidden `config`. Plugins, themes, `--omarchy`, `--hyprland` and `--overlay` inside such a
  project are still refused, and so is `--seed` from one whose hidden paths sit below its folders.
  The check needs `/usr/bin/python3` (package `python`): without it the project stays out, as
  before, and `up` says why. If you replace a hidden file on the host while the jail runs, ai-jail
  stops hiding it, and `up` from that jail is refused until the jail restarts.
- **A frozen shell reads as `hung`, not `running`** (#209). A shell whose pid lived but which
  answered nothing (a crash whose handler hung, its bar left frozen with no report and no relaunch)
  was taken as healthy by `ls` and every command. Now `ls` (and `ls --json`'s `shell_state`) says
  `hung`, `shot`, `windows`, `keys`, `click`, `scroll`, `pixel` and `wait` warn, and `restart-shell` kills it at once
  and says so (it waited 10 s on it before).
- **No hidden X server in a box without `--xwayland`** (#211). The box's parent compositor kept a
  lazy Xwayland on `:0`: an X client that set `DISPLAY=:0` itself ran, but its window was on a screen
  nobody sees (`windows` never listed it, `wait window` timed out). Now there is no X server unless
  `up --xwayland`, so such a client fails with "unable to open display", and `--xwayland`'s
  `DISPLAY` is `:0` (it was `:1`).
- **The bar widget keeps the theme's colours on Qt 6.12.** Qt 6.12's QtQuick brings its own `Color`
  type, which shadows Omarchy's palette singleton, so the widget's `Color.foreground` and
  `Color.urgent` fallbacks no longer reached the theme. It reads `Commons.Color` now, as Omarchy's
  shell does since its b83d3df.
- **CUDA works in a box on an NVIDIA GPU** (#149). The box had the GPU's node and `/dev/nvidiactl`
  but not `/dev/nvidia-uvm`, which CUDA allocates through, so `cuInit` failed (999). It is bound now
  when the driver has made it. Wayland EGL and Vulkan in an NVIDIA box are unchanged (#204).
- **`run -d --wait` says when the command already exited** (#215). A command that ended at once
  (say, on a file under `/tmp`, which a box does not mount) was reported as "started all the same"
  with 124, pointing at the screen. Now it says the command exited with status N, names any argument
  that is a host path the box does not have, shows its log's last lines, and exits with that status
  (124 still when it was 0).
- **`scroll --source finger` and `tilt` arrive as a touchpad and a tilt** (#214). Hyprland dropped
  the source omabox sent first in each frame, so a finger scroll reached apps as a wheel's small
  notches (no kinetic scrolling, Qt with no scroll phases) and fired a wheel bind per frame, and a
  tilt's first notch came as a wheel. It is sent last now. `help scroll` also says why a Qt Quick
  `WheelHandler` at its defaults ignores every wheel, a real mouse's too (a Qt bug), and what to set.

## 0.5.3 — 2026-10-10

Security fixes for agents inside ai-jail driving boxes through the broker: update if you use
`omabox broker on`. Also new: `omabox cpu`, `omabox path --run` and `omabox config shot-fit`.

### Added

- **`omabox path --run`: where a box's /run/user/$UID is on the host** (#208): `<dir>/run`, the
  box's `XDG_RUNTIME_DIR`, where an app's runtime state (a `status.json`, a socket) is read from the
  host. The host's own `/run/user/$UID` is your real session's: `omabox help path` and the agent skill
  now say so, after an agent read a desktop app's state there instead of the box's.
- **`omabox cpu`: CPU and memory of a box's processes** (#205), `gpu`'s twin. `omabox cpu
  [SECONDS] [--settle S] [--pid PID | --app NAME] [--json]` gives each process of the box (never the
  desktop's, which `top` and `pgrep` mix in), or one, its CPU over the window as % of one core, RSS
  and PSS, and a total. A process that starts or exits during the window is counted for what was
  seen of it, and says so. No more hand-written samplers of `/proc/PID/stat`, or a `sleep` before them.
- **`omabox config shot-fit N`: a default size for every shot** (#188). Some models read click
  points in the smaller image their API scales a big shot down to (Haiku 4.5: hundreds of pixels
  off on a 1920x1080 shot); whoever runs one sets `shot-fit 1456` once, and every `shot` given no
  `--fit` comes out as with `--fit 1456`, `click --in` mapping it as usual. `OMABOX_SHOT_FIT`
  overrides the setting (an agent inside ai-jail's too), `--fit N` on a shot wins, and the new
  `--fit 0` takes one full size. It reaches what `--fit` reaches (window shots, `-g`, `--monitor`,
  `--burst`, `drag --shot`), not `--changed` or `--zoom`.

### Fixed

- **`output back` on an NVIDIA box brings the main screen back at its mode under load** (#200). When
  the box's labwc was slow to size the new window, the screen came back at 1280x720, `output back`
  failed, and the box listed it twice (on screen, and as dropped): `mode` and the next `drop` then
  refused it as away. Its mode is now set again until it holds, and a screen made again is never
  kept as dropped.
- **A peek window survives its monitor dropping** (#200). `omabox peek --monitor NAME` of a monitor
  that `omabox output drop` took away kept asking the box for the dead screen, and its window died
  (or froze on the last frame); after `output back` it never showed the monitor again. It now lets the
  screen go: its window turns dark, says `NAME is gone`, and its title ends ` (gone)`; when a monitor
  of that name comes back it shows it again.
- **`keys --window` no longer types into a launcher that has the keyboard** (#201). With Omarchy's
  launcher (or another layer that holds the keyboard against any window) open, `keys --window SEL`
  exited 0 while the text went into the launcher's search field. It now refuses before sending or
  focusing anything, exit 1, naming the layer: `a layer (omarchy-menu) has the keyboard`. It also
  focuses the window when it is already the active one, the pointer left where it is: a window can
  be active without the keyboard (after such a layer closed with the pointer over no window, or with
  a panel that takes the keyboard on demand clicked), and the keys went nowhere or to the panel.
- **Git works in a jailed agent's box from a git worktree** (#193). Inside ai-jail, a box started
  from a linked worktree still had no git (`fatal: not a git repository`): the main checkout's `.git`
  went in only outside a jail, where git itself is asked. omabox now reads the worktree's `.git` file
  and `commondir` instead, never running git on the jail's files, and mounts the main checkout's
  `.git` read-only when the jail has it as a folder of its own (`ai-jail --worktree` maps it so).
- **A box starts on the wallpaper the desk has, one of your own too** (#184). A wallpaper added with
  Omarchy's background install (`~/.config/omarchy/backgrounds/THEME/`) gave a box with no wallpaper:
  its background link led to a path the box does not have, and the shell started with
  `OMARCHY_STARTUP_BACKGROUND` empty. The box now gets a copy of your wallpapers for its theme
  (images only, none followed through a link, up to 64 MiB each and 256 MiB in all) and its link
  leads to the copy; `up --theme NAME` brings NAME's, and the box picks from them as Omarchy does.
  A background set from elsewhere gives the box's first, never a link to a host path.
- **A theme installed from git stays one in a box** (#184). The box's copy left out the theme's
  `.git`, so the box's `omarchy-theme-set` took its Lua and app configs, which the desk ignores. The
  copy now has an empty `.git` (nothing of the real one), and the box treats the theme as the desk
  does.
- **Security: a jailed agent's `up` says nothing of host paths outside its jail** (#198). `up`
  looked at each path it was given before checking the jail, and its refusal named where the path
  led: from ai-jail, `up --seed /etc/localtime:x` answered with the zoneinfo file it links to, and
  "no such path" for one not there, so an agent could learn whether any host file exists, or where a
  host link leads (nothing was copied or mounted). `--seed`, `--theme-dir`, `--overlay`, `--ro-bind`,
  `--plugin`, `--hyprland` and `--omarchy` now follow a jailed agent's path only inside the jail's
  folders, through links in its project too, and refuse one that leaves them with the same words
  whether anything is there or not, naming the path as written. Outside a jail nothing changes.
- **Security: a jailed agent's `up` runs no host program or git on paths the jail can write** (#198).
  A jailed `up --plugin` or `up --omarchy` had the broker run `git status` on the agent's own plugin
  dir and `--omarchy` tree to read their commit, and `core.fsmonitor` in a `.git/config` the jail
  writes runs a program on `git status`; `up --omarchy` also ran that tree's own
  `bin/omarchy-plugin-validate`, both on the host, outside the jail. The commit is now left unknown
  for a jailed caller (no git is run), and a jailed caller's plugins are checked with the host's
  installed Omarchy validator, never the tree's.
- **Security: a jailed agent's `up --seed` refuses a file ai-jail masked** (#198). ai-jail's
  `--mask`/`--deny-path` put an empty file over a path while the real one stays on the host, and the
  box seeds from the host, so `up --seed SECRET:x` could copy the masked host file into the box. A
  jailed `up` now refuses a `--seed` at, under or holding a path the jail masks, where the jail's
  mounts have it now (a folder the jail renamed included), and refuses every seed if it cannot tell
  which paths are masked. A jailed agent's box from a git worktree no longer gets the main
  checkout's `.git` when the jail masks a path inside it (its `config`, say). Outside a jail nothing
  changes.
- **A jailed `up --omarchy` with a relative path works.** `./tree` went to the broker as written, so
  it was refused as outside the jail.
- **A git worktree of omabox runs headless NVIDIA boxes on its main checkout's aquamarine build**
  (#182). A fresh `git worktree add` has no `build/prefix`, so its boxes on NVIDIA were refused for
  lacking aquamarine's fix the main checkout had built. A worktree with none of its own now uses the
  main checkout's (`omabox --version` and `omabox gpu` say so), and `up` from a worktree whose tools
  are not built says to build them with make, not to run `install.sh` (which would link your omabox
  to the worktree).
- **`up` refuses a `--seed` into a `--plugin`'s or `--theme-dir`'s folder** (#199). The plugin's or
  theme's place in the box HOME became a folder holding only the seeded file: the plugin silently not
  in the box (while `ls --json` still said `via: linked`), or the theme gone; under a mounted plugin
  the file was hidden. `up` now says so before the box starts, naming the plugin or theme: write the
  file in its folder instead, or seed outside it.
- **`wait still` (and every `--wait`) sees a thin change that is no caret** (#180). Any change 4 px
  thin was taken for a blinking caret and ignored, however long: a tab's underline appearing or a
  2 px progress bar could leave it "still" while the screen changed. A caret is now 40 px long at
  most, as for `shot --changed`, and blinks in place: a short thin segment that moves (a "working"
  sweep) is a change too.
- **A box has an accessibility (AT-SPI) bus** (#177). It never started (its launcher's
  dbus-broker needs a journal a box lacks), so GTK apps showed no accessibility tree and any AT-SPI
  client, dogtail or a toolkit's a11y tests aborted. `up` now has it run on dbus-daemon, in the
  box's own runtime dir; it adds ~18 MB to a box. `up --env ATSPI_DBUS_IMPLEMENTATION=dbus-daemon`
  is no longer needed.
- **Inside ai-jail, the guard's hook no longer says omabox is gone** and that it can be removed
  from settings.json (#203). It looked for omabox only in its checkout, which a jail sees only as
  the `~/.local/bin/omabox` the broker maps in. A jailed agent got the guard's usual note instead.
  Run `omabox guard on` once to update the hook.
- **omabox's bar widget stays quiet inside a box** (#207). A box that mounts every plugin of
  your desk brought the widget along, and with no `omabox` in the box it notified "cannot list
  boxes: cannot run omabox" and kept the error in the box's bar. Inside a box it now draws nothing
  until an `omabox` runs there. Restart the shell to load the new widget.

## 0.5.2 — 2026-10-09

`omabox shot` works again from a directory you cannot read, as `up` does since 0.5.1.

### Fixed

- **`omabox shot` works from a directory you cannot read** too (an `su` or `runuser` that kept
  root's): it wrote the image, then exited 1 with only `find: Failed to restore initial working
  directory`, as `up` did in 0.5.0.

## 0.5.1 — 2026-10-09

Fixes from a review of 0.5.0. Security: a jailed agent's `up --seed` could have copied a file it
swapped in after the check, and a box could hang `omabox ls --json` (so the bar widget) with a FIFO
in its runtime dir. `up` works again from a directory you cannot read. Update.

### Fixed

- **`omabox up` works again from a directory you cannot read** (an `su` or `runuser` that kept
  root's), where 0.5.0's ended with only `find: Failed to restore initial working directory`.
- **Security: a jailed agent's `up --seed` copies what was checked.** The source was resolved and
  checked once and copied by path later; a path swapped for a link out in between (one the agent
  can make in its own project) would have been followed. The copy now reads from inside the folder
  checked and refuses one that is not what it was.
- **Security: a box cannot hang the host's reads of its runtime dir.** The shell's pid, the monitor
  list, the box environment and the parent display are read only as regular files: a FIFO a box
  planted there would have hung `omabox ls --json` (so the bar widget) and `restart-shell`; a link
  would have read a host file.
- **`omabox down` outside any repo takes the session's box**, as the help said and the other commands
  do; it said `no box 'default-…'` and left the box up.
- **`mode` and `monitor remove` wait for Hyprland to move the monitors** before saying which moved;
  the line was empty when read too soon.
- **`pixel` names a point in a gap with one monitor left**, instead of "no frame from box";
  `monitor add --name` refuses the main screen's names and `FALLBACK`; `up --owner` must be a pid;
  `shot --burst --after run -d -- CMD -b …` passes `-b` to CMD; `output drop NAME` names the ones
  dropped before it; a jailed agent may run `omabox monitor` (it could `up --monitor` already).
- **A jailed agent's call turned away by a busy broker says so** (exit 1) instead of dying of
  SIGPIPE (#197).
- **Interactive multi-monitor boxes keep their layout file** when a monitor lands on a fractional
  position; **`wait -g` ignores a monitor moved out of the region**; **`omabox peek` on a stopped box
  gives up after 10 s** instead of hanging with no window.

## 0.5.0 — 2026-10-09

Several monitors in one box, a box on the theme you name or the one you are editing, and new ways
to see what a box did (`shot --changed`, `shot --burst`, `pixel`, `gdb`, `which`). Security fixes:
files from `~/.config/omarchy`, such as `api-keys.env`, could reach a box through links in your
current theme or a `--ro-bind` inside that folder, and a box started without pasta inherited its
caller's open files. Update.

### Added

- **A box on a theme of your choosing**: `omabox up --theme NAME` starts the box on that theme
  (Omarchy's, one of yours or a `--theme-dir` one) instead of your desktop's current one, so a test
  gets the same start whatever your desktop is on; with `--from SAVE` it wins over the save's look.
  An unknown name is refused before anything is made (#173).
- **A theme you are editing, live in a box**: `omabox up --theme-dir DIR` links the box's
  `~/.config/omarchy/themes/NAME` to DIR, so edits after `up` reach the box and `omarchy-theme-set
  NAME` there applies the theme as it is then. A DIR the box already has at its own path (in the
  repo, a same-path `--ro-bind` or an `--overlay`) is linked to there; any other is mounted
  read-only at its own path. Your other themes are still copies made at `up`, and `up` now says so
  when one is a link into the repo you run it from, or into another checkout of it (a worktree's main
  one) (#170).
- **More monitors in a headless box**: `omabox up --monitor 1080x1920 --monitor 2560x1440,scale=1.6,below`,
  and `omabox monitor add SPEC | remove NAME | list` on a running one: any size, scale and place, a bar
  on each, kept over a config reload, `remove` as an unplug; one placed right or below moves with the
  one before it (`omabox mode`, an unplug). `shot --monitor NAME`; `peek --monitor NAME`, a peek
  window per monitor, each marking the clicks and pointer on its own monitor, `down` closing them all
  (#164); `drag --shot` takes `--shot-fit`, `--shot-g`, `--shot-monitor`; `pixel` refuses a point in a
  gap between monitors. On a box rendering on NVIDIA too, where they are named WAYLAND-1, WAYLAND-2,
  ... (#122, #165). In an interactive box each monitor is a window on your desktop, on the box's
  workspace without focus; together they are the box's layout scaled as a whole, as display settings
  draw it, laid out again at each `monitor add` and `remove`, and each window is a view of its
  monitor, which keeps the size asked for (the main screen `--size`'s) at the window's scale: resizing
  a window scales its view. Closing one unplugs it. Your pointer in a monitor's window lands on that
  monitor, and your cursor stays shown as it goes from one window to another, with omabox's
  aquamarine build (`omabox setup --aquamarine`, then a new box); `monitor add` says when a box's
  build lacks either (#123, #161, #174, #175).
- **`omabox output drop [NAME] [--for DURATION] [--cycles N]` and `output back [NAME]`**: a headless
  box's screen goes and comes back under its own name, mode and position, as a monitor that drops
  off on wake does; cycles stop at the first shell crash. It found a real shell plugin crash (#146).
  With `--monitor`s, the main screen by default or a monitor by its name, each kept apart until it
  is back (`back` alone brings back all of them); while the main screen is away `mode` says so and a
  second `drop` is refused (#163). On NVIDIA boxes too (#165).
- **`omabox up --seed SRC:DEST`** (and `run --seed`): a file or folder copied into the box HOME before
  its session starts, for a plugin that reads its config once at start, where a box needed a second
  shell start after writing it in (#80). It gets `--ro-bind`'s refusals (#159).
- **`omabox up --autoreload`**: the box's Hyprland reloads its config when a file it loaded
  changes, as a desktop's does, so a project can see what a file change does (a helper that
  rewrites a file the config loads, on a Hyprland event, reloads a desktop for ever). Boxes keep
  autoreload off otherwise (see `omabox reload` under Changed).
- **`--ignore "X,Y WxH"` on `wait still`, `wait change` and every `--wait`, and `-g` on `--wait`**:
  an animation that never stops (a spinner, a glow) no longer keeps them from their answer. A 124
  "still changing" names the region that kept changing as an `--ignore` to add (#131).
- **`omabox pixel X Y`** prints the colour of a screen or window pixel (`#rrggbb`, several points
  in one call, `--json`, `--in SHOT`), and **`shot -g … --zoom N`** shows a small crop with each
  pixel N x N, unblended: colours and 1 px details without an image tool of your own (#133).
- **`shot --burst N [--every DURATION] [--diff] [--sheet] [--after -- ACTION]`**: frames of a
  transition into a folder, each with its time and (`--diff`) what changed from the one before, in
  the screen's coordinates (the window's with `--window`) whatever the crop or `--fit`, the action
  (`keys`, `click`, `scroll`, `monitor add` ...) sent after the first frame, and one contact sheet of
  them all. A folder of its own each time, or `-o DIR`, cleared of an earlier burst's frames
  (#132, #167). Frames are 100 ms apart unless `--every` says otherwise (`--every 0`: back to back):
  each grab is four Hyprland events, and faster grabs froze the animation of a shell or widget that
  works on each event, so frames identical to the one before are now counted on stderr (#191).
- **`shot --changed [--window SEL]`**: only what changed since the last whole shot of that window or
  screen, cropped with a margin and `click --in`-able: a menu opening is ~65 image tokens instead of
  ~1.5k (a median 6.7x less over 17 measured actions). Nothing changed: no image, said so. A caret
  blinking alone and where the pointer was are not changes; `--ignore "X,Y WxH"` leaves out an
  animation; `--since SHOT` compares with an earlier shot (#145).
- **`omabox scroll X Y DY [DX] [--source wheel|finger|continuous|tilt] [--mod MODS]`** and `pointer --
  hscroll DX`: horizontal scrolling, a mouse wheel's notches (a DY that is not whole notches said) or a
  touchpad's smooth scroll with its stop, and Omarchy's SUPER+wheel binds with `--mod super`, where
  `pointer -- scroll DY` was one vertical event only (#134, #167).
- **`omabox gdb [--shell | --pid PID] [--watch]`**: every thread's backtrace of the box's Hyprland
  (or shell, or any process of the box), hung or stopped too; `--watch` catches a crash into `omabox
  log gdb`. A gdb started in the box was refused by the kernel's ptrace rules (#135).
- **`omabox config gpu auto|nvidia|amd|intel|SLOT`**: the GPU headless boxes render on, kept in the
  settings (a jailed agent's `up` and every terminal get it), falling back to the first GPU when
  that one is gone, where `OMABOX_RENDER_NODE` named a node that could vanish or change number;
  `omabox config gpu` lists the GPUs, their slots and the one in use (#156). The widget's Settings
  has it as a picker, `ls` and `omabox gpu` show each box's GPU (`render` in `ls --json`), and
  **`omabox gpu release GPU`** takes down the headless boxes on a GPU before it goes to a VM, once any
  `up` in progress is done (#118). A driver's name (`config gpu amdgpu`) is taken; a box that fell
  back says why and on which GPU, in `up`, `ls`, `gpu` and the widget (#168).
- **`omabox ls` has a SHELL column**: `running`, `restarting`, `gone` (a bar that crashed for good
  mid-test), `+N` for the crashes since `up` (`shell_state` and `shell_crashes` in `ls --json`), and
  `shot`, `wait`, `windows`, `keys` and `click` say when the shell is gone, and once when it crashed
  since the last command, where a box with no bar read as up (#147).
- **`omabox which PID`: the box a host process is in**, or `not in a box` (exit 1). A box's bar has
  your desktop's command line, so a host `pgrep quickshell` showed what looked like two desktop
  shells. Every process a box's session starts also carries `OMABOX_BOX=<name>` in its environment
  (`--env` cannot set it) (#172).
- **The bar widget says whether a box is up, for a rail that hosts bar widgets.** It exposes
  `consoleAwake` and `consoleState` (`running` while a box is up), so the rail no longer reads the
  widget's own box count.
- The README says how to open a box's server from another machine on your tailnet (`tailscale serve`
  on your host), and what that shares.

### Changed

- **A `--plugin` inside the repo you run `omabox up` from (or a same-path `--ro-bind`, or an
  `--overlay`) is a link in the box, as on your desk.** It was mounted at
  `~/.config/omarchy/plugins/<id>`, so a helper that finds the rest of its repo through `readlink -f`
  failed in a box and worked on the desk. Any other plugin is still mounted; `ls --json`'s
  `plugin_status` says which (`via`: `linked` or `mounted`). From ai-jail, a plugin inside the jail's
  project no longer has to be a folder the jail was given whole (#171).
- **A running box keeps the Hyprland config it started with.** Every box loaded omabox's config
  live, so a pull, a branch switch or an upgrade of omabox reloaded every running box at once and
  wiped the binds, rules and Lua state an agent had added; an Omarchy upgrade could do the same.
  A box now runs its own copy, with autoreload off (`up --autoreload` keeps it on), and **`omabox
  reload`** gives it this omabox's config and reloads it once. omabox's other scripts in a box are
  still the new ones from their next start (#140).
- **A box's shell runs under `omarchy-launch-shell`, as Omarchy starts it since 4.0.4**: a shell that
  crashes is started again, as on your desktop, where a box was left with no bar (#151).
- **A command run outside any repo uses your session's box**, said once, when it has exactly one,
  `omabox down` included; agents no longer need `-b NAME` on every call from a scratch directory, and
  "no box" names the session's box first (#136).
- **`up --env`, `run --pass` and `--env-file` refuse the variables the session sets itself**
  (`PATH`, `HOME`, `XDG_RUNTIME_DIR`, the `XDG_*_HOME` dirs, the display variables, `LD_PRELOAD`...),
  which only broke the box 30 s later ("Hyprland not up after 30s"). A plugin's manifest id must be
  letters, digits, `.`, `_` and `-`, and `--allow` ports 1-65535, kept sorted (`--allow 8082,8081`
  matches a box up with `8081,8082`) (#106).
- **`keys -t` and `--pass` refuse control characters other than newline and tab** (exit 2, nothing
  typed): a backspace or escape in the text was pressed as that key, and a password with a trailing
  `\r` submitted the form. A `--pass` value's one trailing `\r` (a Windows line ending) is dropped
  (#104).
- **`omabox host` shortens only long arguments in the line it prints** before it runs a command
  (`env PATH=$PATH` printed ~900 characters ahead of what mattered); the record stays (#150).
- **A system aquamarine patched with the fix for nested Wayland outputs is used as fixed.** omabox
  told the fix (hyprwm/aquamarine#415) by the version, so a package that patched it into 0.15.0 would
  still have refused headless NVIDIA boxes and turned confirm-close off. It now looks for what the fix
  added to the library; `omabox --version` says "patched with the fix" for such a one. `omabox setup
  --aquamarine` still builds with such a one, for omabox's own fixes for interactive boxes, which no system
  aquamarine has (held keys, the pointer and cursor in monitor windows); it builds nothing only when
  boxes already use this omabox's build, and `omabox setup` says what yours lacks (#48, #186).
- **The bar widget's list holds still while the pointer is over it**: a box that goes stays in its
  row, greyed and marked gone, and a new one is counted in the header until the pointer leaves, so a
  click never lands on a box that slid under it (#117). **A list taller than the screen scrolls**,
  the New button and the keys staying in view, and stays where the wheel left it (#121, #168).
  **Every row has its buttons in the same places**, so a click from habit on an interactive box's row
  no longer pastes your clipboard into it, and **a double-click on Down no longer shuts a box down**
  (nor `d d`, `n n`) (#120).
- **The agent skill is a third of its size** (SKILL.md 29 KB → 12 KB, ~4k tokens a load): what every
  task needs stays, every safety rule included; the rest is in `reference.md`, named by section (#143).
- **The agent skill says what agents worked out by trial** (waits sent to `/dev/null`, screens that
  never stop moving, compound commands refused in worktree subagents, the plugin registry's reloads,
  Lua state lost on a reload) (#138), that `up --new` starts the box and takes `up`'s options on that
  call (#148), and that Omarchy's lock screen runs in a box, with a recipe for unlocking it with a
  test password (a real one cannot work there).
- The test suite checks the box safety rules on every kind of box it starts, not one, covers
  fourteen flags no check used, and no longer passes checks that could not fail or fails ones that
  ran late under load (#91, #110, #111, #116).
- **The agent skill no longer says a full-size screen shot is read 1:1.** That held for Opus and
  Sonnet; Haiku 4.5 reads points in the smaller image the model API shows it (0.76x of a 1920x1080
  shot), so its clicks landed hundreds of pixels off, and other providers' models scale images with
  limits of their own. To click off a whole screen or a big window the skill now says `shot --fit
  1456` (16:9 on Claude) and `click --in` it, which worked for every model measured (#188). It also
  names `omabox gpu` for a box's rendering cost, which agents had been summing by hand (#190).

### Fixed

- **An interactive box's window resized just after a config reload is followed.** The box's bar and
  wallpaper kept the old size when the resize came in the quarter second after a reload (#174).
- **A link inside your current theme no longer brings the file it names into a box.** omabox copied
  your theme into each box HOME following its links, and `omarchy-theme-set` keeps the links of a
  theme you made: a link to `api-keys.env` put your keys in every box (and in its saves). Links in
  the theme now stay links, as in the rest of the box HOME. If a theme of yours has links to files
  outside it, update (#97).
- **Nothing inside `~/.config/omarchy` goes into a box but `plugins/` and `themes/`**, nor a link
  there or in a secret store to a file elsewhere: `--ro-bind` mounted a file or folder inside it
  (`api-keys.env`, `hooks/`), refusing only the folder itself. If you or an agent mount anything from
  `~/.config/omarchy`, update. The new `--seed` gets the same refusal, never writes through a link
  already in its DEST (an earlier seed's, or a save's: it overwrote the host file the link named),
  and takes a relative SRC from inside ai-jail (#159).
- **A box started without pasta gets none of its caller's open files.** A nested box, or one started
  from a process that cannot gain privileges, inherited every file descriptor omabox's caller had
  open (a harness pipe, a lock, a log), readable or writable from inside it (#112).
- **omabox's input tools tell a box from your desktop by what only a box has.** The keyboard,
  pointer, still and events tools refused outside a box by checking that `/opt/omabox/share` exists,
  so on a machine with that folder they would have driven your real desktop. They also give up after
  10 s on a compositor that stopped answering, where `keys` and `click` waited for ever (#108).
- **A box can no longer make `gdb --watch` write to your files**: it appended its header to whatever
  the box's `gdb.log` linked to; the header is now written from inside the box, as the rest of the
  log is, and `run -d`'s log is opened in the box too. `gdb` runs in the box's network namespace, and
  prints a process name the box set with escapes or newlines as `?` (#160).
- **Under the agent guard, `quickshell`/`qs` `kill` and `ipc` are refused**, with bundled options
  too (`qs -np PATH kill --any-display`): `omarchy restart shell` from an agent stopped your
  desktop's shell and could not start it again, leaving you with no bar (#141, #166). The guard's
  note and the skill say that your own `!` commands in Claude Code are guarded too, and send desktop
  commands to your own terminal (#142). The note changed, so `omabox guard` calls an installed hook
  outdated: `omabox guard on claude` renews it.
- **`wait still` and every `--wait` end on a box whose Hyprland stopped answering** (stopped, or
  deadlocked by a plugin under test): "its Hyprland did not answer (hung? …)", exit 1, where they
  waited forever whatever `--timeout` said (#124).
- **A box whose Hyprland stopped answering is said so in a few seconds**, with the same words, by
  `windows`, every `--window`, `shot -g`, `wait window` and `wait layer`, where they printed jq's
  errors or "grim failed" after 10-15 s. `omabox ls` shows such a box as `hung`, and `ls --json`
  has `"hung": true` (#125). **`omabox hyprctl` and `omabox lua`** end there in 10 s with the same
  words, exit 1, where they could wait for ever or print hyprctl's "IPC didn't respond in time"
  (#139, #166).
- **A box that goes down while a command reaches into it is said in words** by `keys`, `click` and
  every other command, and by `run` in place of nsenter's "cannot open /proc/…" or a bash "Killed"
  line (#166). **`omabox run -- pkill -x labwc` no longer ends the box** (its own labwc is
  `omabox-labwc`), and `run` says when the box went down while its command ran (#128).
- **`restart-shell` and `up` say when the shell crashes as it starts**, naming Quickshell's crash
  report, and exit 1; `restart-shell` said "shell restarted". Quickshell's crash dialog no longer
  appears in a box, where it took the keys meant for the app under test (Return on it opened a
  browser); the report is kept (#126).
- **`up` gives up at once on a box that dies while it waits for the bar**, where it retried until its
  timeout, holding the box's lock (#157).
- **A box whose `up` is killed still goes down at its idle limit**: an `up` killed after the box
  started (a harness's timeout, a suite run ended with its process group) left a box that nothing
  would take down. The reaper now starts before the box (#137, #168).
- **`wait` and every `--wait` no longer exit 1 "lost the box's screen"** for an answer that came at a
  whole second of the wait, as whole-second `--timeout` values make it do (in 0.4.8 too).
- **`wait still`, `wait change` and every `--wait` watch every monitor of a box**, in the layout's
  coordinates as `click` and `shot -g` take them: a change on another monitor (a spinner, typing into
  a window there, an app opening at scale 1.6) went unseen, `-g` or `--window` there was "not on the
  screen", and after `output drop`/`back` the regions named were another monitor's own pixels. An
  `--ignore` outside the region watched is said, and `help scroll`/`help drag` show `--wait` (#162).
- **`drag --wait` ignores the cursor along the whole path**, not only at its ends: a drag across an
  empty screen reported "settled" on its own arrow (#102).
- **Omarchy's plugin registry no longer reloads a mounted plugin in a box** on every save in your
  checkout (about four times a save), so a shot or `wait still` never catches it half-reloaded (#129).
- **A bar widget kept in another mounted plugin's layout is `hosted`**, its host named in
  `plugin_status`, where `up` and `restart-shell` warned it was disabled every time (#127).
- **A box's session bus is at `$XDG_RUNTIME_DIR/bus`, and its runtime dir is 0700**, as in a
  session: helpers that check their bus refused the old one (a socket in `/tmp`, a 0755 dir) (#153).
- **A jailed agent gets `omabox ports` and `omabox config KEY`**: the ai-jail broker refused both
  (`config KEY`, a read, as a change). The host side of `ports` is a state only, with no process of
  yours named, and a port a box outside the jail serves on reads as taken (`host`), without naming
  that box. A caller outside any jail that reaches the broker is told why (#93, #114, #168).
- Three edges agents tripped on: `log --grep -i RE` took `-i` as the expression (both orders work
  now); `lua` returning a dispatcher printed `HL.Dispatcher`, exit 0, and nothing ran (it now says
  on stderr that it was returned, not run, and how to run it); `run --env K=V` on a box that is up
  was dropped (#130).
- `omabox windows` keeps its columns for a window with no class, and shows a quoted title once
  (`"hi"`, not `\\"hi\\"`) (#105).
- `down` no longer fails without a word, leaving the dead box's dir and HOME, when the box's reaper
  exits at the same moment (#115).
- **An `omabox down` that comes while an `up` of the same box is starting always wins**: two narrow
  gaps let the `up` start the box anyway (#168).
- **The runtime dir no longer keeps a lock and a marker for every box name ever used** (4443 lock
  files two days after a boot, for one box up): they go with the box, a `down`'s marker after ten
  minutes, an idle box's note after a day (#158, #168).
- Small edges said in words: `events --mark v1.0` no longer removes the mark `v1x0`; `click`,
  `pointer` and `drag` past the screen name the point and the screen; `keys ü` suggests `-t`; `shot`
  on an interactive box gives grim's reason; `up 'bad name!'` says the name as written (#107).
- `peek` gives up after 10 s on a box that never draws ("the box sent no frame in 10 s"), where it
  left a peek with no window; `peek` and `wait` name a frame format they cannot read, where peek's
  view froze; the ai-jail relay serves at most 64 connections at once (#109).
- In a box, a test's stub `hyprctl` in `~/.local/bin` can no longer end an interactive box without
  asking (confirm-close); `lua` prints bytes that are not UTF-8 as `\u00XX`, where they came out as
  U+FFFD; the box's `systemd-cat`, `systemd-run` and `uwsm-app` take `--level-prefix VALUE`, `--shell`
  and a desktop file's path with an action as the real ones do; the guard's hook no longer adds its
  folder to PATH again at each session start (#113).
- The guard's Codex check runs the system's Python, not a mise shim first on PATH that could exit
  instead; `install.sh` lists `python` (#152).
- **The bar widget no longer logs `TypeError`s while a bar rebuilds** (a `bar.layout` edit with the
  widget inside another plugin): it falls back to the theme's colours while its bar is gone (#155).
- **git works in a box started from a git worktree.** The box mounted the worktree but not its git
  dir, inside the main checkout's `.git`, so `git status`, `git describe` and an installer that runs
  git there failed with "not a git repository". That `.git` is now mounted too, read-only at its own
  path (not the rest of the main checkout): git reads work, writes (a commit, a tag) fail. The same
  for a submodule (#183).
- **A GPU moved to the nvidia driver while the machine runs (from vfio-pci, after a VM) gets both its
  device nodes.** omabox asked NVIDIA's helper for the GPU's node only, so `/dev/nvidiactl` stayed
  missing, every headless box on that GPU was refused, and the error suggested a command that could
  not make it. A GPU on nvidia from boot is not affected (its udev rule makes both). Each missing
  node is now made, and the error names the right command for each (`nvidia-modprobe -c 255` for
  `/dev/nvidiactl`).
- **`restart-shell` no longer says the shell exited when it is running.** When the shell had just
  died (a plugin edit crashing it, a kill), Omarchy's launcher started it again a second later next
  to the new one, which exited with "An instance of this configuration is already running":
  `restart-shell` reported a failed start (exit 1), and every restart after failed the same way.
  `restart-shell` now stops every running copy of the shell and its launcher before starting one. If
  something else starts the shell at the same moment, it reports that shell as the running one (exit 0),
  and a crash of an old copy it stopped is not taken for the new shell's (#192).

## 0.4.8 — 2026-10-03

A security fix for boxes started from a save, and a bar widget that no longer freezes on a command
that hangs.

### Fixed

- **`up --from` no longer writes through links in a saved HOME.** A box can put links in its HOME,
  and a save keeps them; starting a box from that save then made omabox, on your machine, write the
  box's settings wherever those links pointed, onto your own files. omabox now removes such links
  before it seeds the box. If you start boxes from saves of boxes that ran code you do not trust,
  update.
- **The bar widget stops an `omabox ls --json` that does not answer** within 15 s, with what it
  started, and says so; one that hung used to freeze the widget's list until it was killed by hand.
- **The bar widget reads its own settings.** Its `command` and `refreshIntervalSec` in shell.json
  were ignored since 0.1.0 (the CLI's settings hid them); `listTimeoutSec` sets the new limit.

## 0.4.7 — 2026-10-03

Fixes from a review of the whole tool, first among them `ls --json` reading a file the box can
replace, and `omabox ports`, which says which box holds a port on your machine.

### Added

- **`omabox ports`** lists every box's TCP servers and what holds each port on your `127.0.0.1`:
  that box, one of several boxes serving on it, one of your own processes, or nothing yet
  (`--json` too; `omabox help ports` lists the states) (#88).
- **SECURITY.md**: each box rule, where it is enforced and the test that proves it, and how to
  report a way around one privately (#83).

### Fixed

- **`ls --json` reads only plain files from a box.** A box could make its theme name a pipe, which
  hung `ls --json` and the bar widget, or a link to one of your files, whose first line then showed
  as the box's theme. The same check now covers every file omabox reads from a box's HOME (#95).
- **Mounting omabox's own saves and box HOMEs is refused**, with the token dirs of gh, gcloud,
  azure, 1Password's `op` and git's credential files, as `~/.ssh` already was (#98).
- **Six rare states no longer end omabox without a word**: a peek window closing during `down`, a
  shell log the host cannot read in `restart-shell`, a failed bar capture while `up` waits, a box
  dying at two points of `up`, and `run -d` on a box that is gone (#101).
- **A box's idle limit holds when a `down` fails.** The reaper that takes idle and orphaned boxes
  down stopped when a `down` timed out under load, leaving the box running; it tries again now. A
  throwaway's owner is checked by its start time too, so a reused pid does not keep its box (#99).
- **A failed `up` leaves nothing behind**, and `up --from` reads its save once: a save removed while
  `up` waited could have made it copy your home into the box (#96).
- **`ports` no longer exits silently** when a box goes down while it reads it (#92).
- **`ports` names a box whose server listens on `::1` only** as holding its port: its pasta takes
  the port on your `127.0.0.1` and connections there are reset; it said "not forwarded" (#90).
- **The docs say a box server on a port your host or another box has fails to start** (address in
  use); they said it still answered inside its box (#89).
- **`omabox help ports` lists what its states mean**, and leftover text from `ports` is fixed (#94).
- **`drag --hold` is documented as a duration** (`300ms`, `2s`; a bare number is seconds), and its
  refusal says so; the docs said milliseconds (#103).
- **A failed throwaway `run` keeps its logs** where its message points (#100).
- **A box keeps forwarding its servers when one of its ports is taken on your machine.** pasta
  stopped forwarding every port above one it could not bind, for as long as it was taken (#88).
- **A failed `up` takes its box down completely**: its cleanup stopped after the first step and
  could leave the network helper running.
- **`ls` lists a box it cannot read as `unknown`** instead of refusing the whole list (from a
  sandbox, for instance) (#87).
- **An `up` and a `down` of one box started together** are ordered by when each was started, so
  the `down` wins as it should under load.

## 0.4.6 — 2026-10-02

Fixes found on a dev-linked Omarchy and with a bar widget kept in a sidebar plugin.

### Fixed

- **A box started from a dev-linked Omarchy checkout runs one Omarchy.** `omarchy dev link` puts the
  checkout's `bin` first on your PATH, and a box started from inside that checkout had the
  checkout's `omarchy-*` commands while the rest of it was the installed Omarchy. A box's PATH now
  leaves an Omarchy checkout's `bin` out (`--omarchy DIR` still runs a checkout on purpose);
  `omabox host` keeps it, since there the link is your real desktop.
- **Binds, the bar and terminals in a box get omabox's stand-ins.** Omarchy's config puts its own
  `bin` first for everything Hyprland starts, so there `omarchy-version` printed nothing and the
  browser policy stand-in was skipped. They come first again.
- **`up --plugin` puts a bar widget in the bar when you keep it in a plugin of yours.** A widget you
  placed inside a sidebar plugin's layout was in no bar in a box (the sidebar is not there): it now
  goes where its manifest says, its settings kept. Mounting the sidebar too keeps it there.

## 0.4.5 — 2026-10-01

Fixes found by testing what had shipped untested: a box that outlived its `down`, `wait layer` on
newer Omarchy, pi not loading the skill, and light dialogs under a dark theme.

### Fixed

- **A `down` while its box is still starting takes it down.** A `down` in the first moments of an
  `up` said "no box" and exited 0, and the box came up behind it and ran until its idle limit. Now
  it waits for that `up` and takes its box down, or the `up` stops itself ("up: cancelled: omabox
  down NAME came while it was starting").
- **`wait layer` on newer Omarchy shells.** Omarchy's development branch keeps its menus mapped as
  1x1 layers while hidden, so `wait layer omarchy-menu` was satisfied with the launcher closed and
  `--gone` never was. A layer counts only when it is drawn (bigger than 1x1), here and in `up`'s wait
  for the bar.
- **pi loads the agent skill.** The skill's description was not valid YAML to a strict parser, and pi
  skipped the skill without a word. Claude Code, OpenCode, Codex and Hermes read it either way.
- **Dialogs follow the theme's light or dark mode.** A box's settings said "no preference", so file
  choosers and other portal dialogs, GTK, libadwaita and Qt apps were light under a dark theme. A box
  sets the mode from its theme at start, as `omarchy-theme-set` does on your desktop.
- **`restart-shell` records an `--omarchy` tree's commit again** (`+dirty` once edited), as it does
  for plugins, so `ls --json` says what the box tested.
- `omabox gpu` names the aquamarine the box runs (a line under the first; `aquamarine` in `--json`).

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
