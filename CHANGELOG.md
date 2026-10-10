# Changelog

What changed in each version of omabox, newest first. The CLI, the agent skill and the bar widget
share one version (`omabox --version`). Update with `git pull && ./install.sh`.

## Unreleased

### Added

- **`omabox config shot-fit N`: a default size for every shot** (#188). Some models read click
  points in the smaller image their API scales a big shot down to (Haiku 4.5: hundreds of pixels
  off on a 1920x1080 shot); whoever runs one sets `shot-fit 1456` once, and every `shot` given no
  `--fit` comes out as with `--fit 1456`, `click --in` mapping it as usual. `OMABOX_SHOT_FIT`
  overrides the setting (an agent inside ai-jail's too), `--fit N` on a shot wins, and the new
  `--fit 0` takes one full size. It reaches what `--fit` reaches (window shots, `-g`, `--monitor`,
  `--burst`, `drag --shot`), not `--changed` or `--zoom`.

### Fixed

- **Security: a jailed agent's `up` says nothing of host paths outside its jail** (#198). `up`
  looked at each path it was given before checking the jail, and its refusal named where the path
  led: from ai-jail, `up --seed /etc/localtime:x` answered with the zoneinfo file it links to, and
  "no such path" for one not there, so an agent could learn whether any host file exists, or where a
  host link leads (nothing was copied or mounted). `--seed`, `--theme-dir`, `--overlay`, `--ro-bind`,
  `--plugin`, `--hyprland` and `--omarchy` now follow a jailed agent's path only inside the jail's
  folders, through links in its project too, and refuse one that leaves them with the same words
  whether anything is there or not, naming the path as written. Outside a jail nothing changes.
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
