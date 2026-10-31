# omabox: reference

Detail that `SKILL.md` points to ("ref: SECTION" there is a section here). The safety rules are in
`SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

## Project instructions, in detail

- More of the project's steps, in a box: `grim -T ID` → `omabox shot --window SEL`; a test binary run
  directly (`./build/tests/tst_x`) has none of ctest's environment (a Qt test opens real windows):
  `omabox run -- ./build/tests/tst_x`; starting the app from the launcher, its `.desktop` and icon:
  its files in the box HOME, then `omabox keys --wait super+alt+space` ("Testing an app as a desktop
  app"); `omarchy-theme-set NAME` → `omabox run -- omarchy-theme-set NAME`; `omarchy plugin
  add/enable/disable/remove` → `omabox run -- omarchy plugin …`; a back-up and restore of the bar
  around a test ("put the desktop back afterwards"): nothing to restore, the box's bar is its own
  (`omabox down`). `hyprctl eval EXPR` to read a value → `omabox lua EXPR`, which prints the value.
- Themes: a theme in `~/.config/omarchy/themes` linked to your working copy → `omabox up --theme-dir
  PATH` (live; else a copy made at `up`). A result that depends on the theme or bar (colours, sizes,
  shots compared across runs, a theme-switch flow) → `omabox up --theme NAME --stock-bar`; else they
  are the desktop's.
- A step that needs real hardware is not moved into a box: it goes to the user ("When a box cannot
  test it, in detail").
- A box starts with a fresh HOME: apps start as on first run. A server in a default box also holds
  its port on the user's 127.0.0.1 (their own dev server on it then fails to start), and a probe there
  can reach theirs or another box's (only one box gets a port on the host; `omabox ports` says which
  holds it): check your server from inside the box (`omabox wait until -- curl -fsS
  http://127.0.0.1:PORT/`). A server that fails with "address in use" on a port nothing in the box
  uses: the host or another box holds it (`omabox ports` names it); use another port. An isolated
  box's ports are its own; two of them can use the same one.
- Names: the repo's directory plus your session's id (`myrepo-5cc72cdc` in a Claude Code or Codex
  session); in a git worktree, the worktree's folder, so a worktree agent has its own box with no
  `-b`. A box goes down when your agent exits, not on `/clear` or `/resume`. `-b NAME` also goes
  before the command (`omabox -b box-3 shot`). In a worktree-isolated Claude Code subagent: "For
  subagents in an isolated worktree" below.
- `omabox help CMD` (or `omabox CMD --help`): one command's flags, one line each; `omabox help CMD
  --long` adds its notes (what each flag does, defaults, limits): read it before guessing.
  `omabox help`: every command's flags.

## Exit codes

The same in every command (`omabox help exit-codes`):

| Code | Means |
|---|---|
| 0 | Done, or the condition held (`wait`, `events --until`; `check run`: nothing changed). |
| 1 | It ran and failed, or the answer is no: no such box up, a refusal, `wait`'s `unknown` (the box went down), `tree` finding nothing, `which`'s `not in a box`; `check`: a checkpoint changed. |
| 2 | Called wrong: an unknown option or command (the nearest one is named: "did you mean --window?"), a value missing or not of its form, a word too many or too few. The message says which, and `omabox help CMD`: fix the call, do not retry it. Also `--window SEL` matching no window or several (they are listed), `which PID` for a process gone, every error of `check`'s. |
| 3, 4 | `check run`: the scenario itself failed / no baseline to compare with. |
| 124 | Not in time: `wait`, `--wait` (keys, click, scroll, drag, `run -d`), `events --until`. |
| 125 | `run`: omabox's own failure (no box, a bad option, a throwaway that did not start, the box going down while the command ran), never the command's. |
| 126, 127 | `run`: the command could not be run / was not found in the box. |
| N | `run`, `hyprctl`: the command's own status, passed through; `run -d`: the job's, when it ended at once (`--wait`: before the screen settled). |
| 128+N | Ended by signal N (130: Ctrl-C). |

An option that takes a value takes it as `--opt VALUE` or `--opt=VALUE`, in every command.
A form on its way out still works for one minor release, with a warning on stderr (`omabox:
deprecated: OLD: write NEW`): write NEW from then on. It is refused (2) after that; `OMABOX_STRICT=1`
refuses it at once (2), for CI.

## JSON

What `--json` gives, by its top-level fields. These are kept from 1.0: a field may be added, never
renamed, dropped or given another type; read the ones you need by name. Field order, whitespace and
the words inside `message` are not part of it. Snake_case everywhere; rectangles are `x`, `y`, `w`,
`h`; durations seconds; sizes in kB (`_kb`). One object or array on stdout unless said "a line each".

| Command | Shape |
|---|---|
| `up --json`, `ls --json` | a box (an array of them for `ls`): `name, state, mode, size, scale, created, dir, net, allowed, addrs, idle, plugins, plugin_status, bar, theme, render, monitors, hyprland_version, quickshell_version, omarchy_version, aquamarine, clean, starter, hung, peeking, keys_to_box, shell_state, shell_crashes, orphan` and more |
| `jobs --json` | an array, oldest first: `pid, age` (seconds), `processes, log, argv` |
| `windows --json` | an array: `address, stableId, class, title, initialClass, initialTitle, pid, workspace, x, y, w, h, at, size, floating, fullscreen, xwayland, active, onscreen, cover, layers` (Hyprland's own names kept: selectors use them); each of `layers`: `namespace, pid, layer, x, y, w, h, at, size, monitor, certain` (false: a layer spanning the screen, which may take input only where its items are, so it may not cover the window at all) |
| `tree --json` | an array of nodes: `depth, role, name, states`, and `text, value, x, y, w, h, path` when known |
| `tray --json` | an array of items: `id, title, status, category, service, path, menu` (null: none), `item_is_menu, rows` (each `id, label, role, enabled, visible, toggle, checked, rows`); `tray menu --json`: one item |
| `pixel --json` | a line each point: `x, y, hex, r, g, b` |
| `lua --json` | a line each value Lua returned (any JSON value) |
| `wait --json`, `--wait --json` | `result, condition, message, elapsed`; still/change/settle add `first_change, last_change, change, late_changes, ignored, frames`; window/layer/job/log/tree/until add `detail`; `log LOG` also `log, offset` (where the line found ends: the next wait's `--since`; null for hyprland); `for` has `detail` null; `--expect` adds `expect` (`tree, met, detail`: `met` null when the screen did not settle); `--tree` adds `tree` (`window, changes, why`: each change `kind` changed, appeared or vanished, `role, name, path, line`, and `name_was, text, value` (`from, to`), `states_added, states_removed, hidden, inside` when they apply; `changes` null and `why` when there was no tree to compare); an action's `--wait` adds `changes` (an array, `[]` none, null not read: each `kind`, more kinds may come, ignore unknown ones), `target` (`kind` window, layer or none, `window, namespace, at, certain`: false when a layer spanning the screen over the point may have taken it), `events_since` (an `events --since` offset) |
| `events --json` | a line each event: `time, event, data`; `--mark --json`: `mark, offset` |
| `shot --json` | `path, kind` (screen, region, monitor, window), `region` (the part taken: layout coordinates, a window's own for a window), `image` (`w, h`), `scale` (image px per layout px), `window, monitor, fit, zoom, changed` (`--changed`: `result` changed, nothing or whole, `since, why`) |
| `check run --json` | `result` (same, changed), `base, shots, report, report_scaled, setup, only, counts` (`skipped`: not named by `--only`), `checkpoints` (each `id, result, as, size, regions, holes, tree, specks, marked, approved_holes, still`); exit 3 or 4: `result` failed or no_baseline |
| `check diff --json` | `result, size, regions, specks, marked, report, report_scaled` |
| `record status --json` | `recording, stopped, monitors, fps, file, audio, started` |
| `mode --json` | `name, mode, scale` |
| `window --json` | the window, as an element of `windows --json` |
| `monitor list --json` | an array: `name, mode, scale, x, y, workspace` (`dropped` for one `output drop` took) |
| `gpu --json` | `box, seconds, source, render, percent, engines` and more |
| `cpu --json` | `box, seconds, settle, cores, percent, processes, total` and more |
| `audio --json` | `box, seconds, sound, sound_s, peak_db, rms_db, spans, levels` and more |
| `polkit ask --json` | `result` (authenticated, not-authenticated, cancelled, timeout, agent-gone, no-agent), `tries, wrong, agent, path, action, why`; `polkit agents --json`: an array, `subject, name, path, fallback` |
| `ports --json` | an array: `box, port, addresses, process, host` |
| `path --logs --json` | `{NAME: PATH}`, each log there is |
| `saves --json` | an array: `name, box, saved, size, size_kb, keyring` |
| `config --json` | `workspace, confirm_close, bar_icon, gpu, shot_fit, confirm_close_available, version, gpus, gpu_auto, gpu_now` |
| `calibrate --answer --json` | `result` (one, fit, up, none), `card, marks, scale, fit, fit_size, error_px, worst, off, shot_fit` |
| `which --json` | `pid, box, by` (pidns or env; `box` null: not in one) |
| `pid --json` | `box, process` (hyprland or shell), `pid` (the host's), `box_pid` (the box's own) |
| `--version --json` | `version, aquamarine` (`kind, version, fixed, message`) |

## Handing box work to a subagent

`omabox brief -b NAME` prints what a subagent needs to drive that box, ~15 lines (~450 tokens,
against the skill core's ~2 000): the box's name, size and scale, its windows one a line (each by a
`--window` word that finds it alone, else its address; six at most), then the rules: `-b NAME` on
every call, one literal command per call, look, act once, look again, never resend input not seen
landing, no sleep, never the real desktop, the box HOME's host path, and the five below. Paste it
whole into the subagent's prompt: the subagent needs no skill, and the brief's last line tells it to
load the skill when the brief is not enough. Run it right before delegating (the windows are as
they were then). With no box up it prints the same rules for a box the subagent starts itself (`up
--new`, down when done), and says so on stderr; exit 0 either way.

**Parallel subagents share your session's default box name** (it comes from the session's id, which
they all have), so two that run `omabox up` get one box, and one's `down` ends the other's test. Give
each its own: `omabox up --new` (then `-b box-N`, as printed) or `-b` a name of its task; it never
takes down a box it did not start (a box with your session's suffix in `omabox ls` may be a
sibling's: `ls` says where each was started, and `down` warns of one another caller used just now).

## For subagents in an isolated worktree

Claude Code checks each Bash call of a worktree-isolated subagent and refuses what it cannot follow:
a word it reads as a shell (`wait cmd`), shell text it cannot see through (`$(…)` with more after it,
a pipe into another program, a heredoc with `cd`), anything leaving the worktree. Each refusal costs
a turn. Five rules keep every omabox call plain:
- **A state: `omabox wait until -- CMD`** (never `wait cmd`, refused even alone); the screen: `wait
  still`, `wait change` or the action's `--wait`, never a sleep.
- **Several shell steps in the box: a script in the box HOME.** Write it with your file tool at the
  path `omabox path --home` printed, then `omabox -b NAME run -- sh /home/sbx/F.sh`. A host script:
  `omabox run -- python3 - < FILE`, alone in its call.
- **Box files from the host: `omabox path --home` once**, then Read, Write and Edit on that literal
  path (`/home/sbx/X` in the box is `<that path>/X`); never `$(omabox path)` inside a command.
- **Every argument literal**: no `$VAR`, no `$(…)`; look the value up first (`omabox run -- pgrep -x
  APP`), then type it (`--pid 1809`).
- **Stay in your worktree**: no `cd` out of it, no git on another checkout (absolute paths instead);
  scroll with `omabox scroll … --source wheel`, not `pointer -- source`.

## Shots

- Text before a shot, when the question is a label, an entry's text or a state: `omabox tree --find
  TEXT [--window SEL]` prints the lines of the window's accessibility tree that hold TEXT (any case),
  each after its ancestors: `check box "Dark mode" [checked] @12,148 1872x26` (~17 tokens against a
  shot's ~1.5k). A line is role, "name", 'text', =value (slider, spin button, combo choice), [states]
  (checked/unchecked, focused, selected, editable, disabled...) and @X,Y WxH in the window's own
  coordinates: `click --window SEL` at the middle of those bounds lands on it. `click --window SEL
  --find TEXT` does both: reads the tree again, clicks the one node named TEXT at the middle of what
  of it is in view (exact name over a part of one, a control over its label), and says its line;
  several in view is exit 2 with them numbered (`--index N`), and a hidden, scrolled-out or disabled
  one is refused, exit 1, saying which (scroll first). `tree` alone prints the whole window
  (no SEL: the active one); a dense one (an editor, a long page) costs more than its shot, so ask
  with `--find`. `--json` for a script; exit 1 when nothing holds TEXT. GTK, Qt and egui/winit
  (AccessKit) apps have a tree (`tree` turns Qt's and AccessKit's on). AccessKit needs
  accesskit_unix 0.22+ (eframe 0.37+) on at-spi2-core 2.61+ (Arch has it): an older one never joins
  the bus, `tree` says so, and only updating the app's AccessKit helps; Chromium and Electron only
  when started with `--force-renderer-accessibility` (start them so when you will ask); terminals
  (foot) have none and say "no accessibility tree": shots for those. An unnamed control takes its
  label's name where the app links them; one that does not (a switch beside its label) matches by
  its role (`--find switch`), or read the whole tree.
- The shell's bar, panels, menus and plugins are layer surfaces: `omabox tree --layer SEL` (a
  namespace from `omabox hyprctl layers`, `address:0x...` or `pid:PID`), with `--find`, `--json` and
  `--all` as for windows. Its bounds are the **screen's** coordinates: plain `click X Y` at a node's
  middle lands on it, and `click --layer SEL --find TEXT` finds and clicks it. Quickshell publishes them from 0.3.2 (older: "no accessibility tree", or
  `--quickshell` a build), and only items that set an `Accessible.role`: a layer without any says
  "publishes no accessible items" (Omarchy's own bar today), exit 1. Its `frame`s have no names, so
  the layer's is the one of its pid that is its size: two layers of one pid and size are told apart
  only when one holds the keyboard (the launcher, or an Omarchy 4.5 bar popup, over the wallpaper);
  else `tree` says so.
- `click --window SEL X Y` takes the window's own coordinates (0,0 its corner, as in its `shot
  --window`).
- `--window SEL`: `myapp` is a class, its last part (`nautilus` for `org.gnome.Nautilus`) or part of a
  title, failing those a hyphen-separated part of the class (`portal` for `xdg-desktop-portal-gtk`);
  `title:RE`, `class:RE`, `pid:N` or an address (`0x…`) narrow it; `wait window SEL` is satisfied by
  any window it matches. `shot --window SEL -g "X,Y WxH"` crops the window in its own
  coordinates; a `-g` past the window (or the screen) is cut to it, said on stderr.
- Coordinates are screenshot pixels. A cropped, scaled or zoomed shot says so on stderr: then `click
  --in SHOT X Y` (and `drag`, `pointer`, `scroll`, `pixel --in`), X Y read from that image. Screen
  shots show the pointer (hover evidence: a `-g` crop of the screen; `--no-pointer` leaves it out of
  one), except after `keys`: Omarchy hides it on a key press until it moves; window shots never do.
  A shot right after a `click` or `keys` without `--wait` can show the frame before the redraw: `--wait`, or
  `omabox wait still`, first.
- An action `--in` a screen shot is refused (exit 1, nothing sent) when what is at its point changed
  since the shot: "the screen changed at X,Y since SHOT: then on ..., now on layer omarchy-menu ...;
  take a new shot". Shoot again and read the new image; a change away from the point (a toast, the
  clock) is not refused. A window's shot follows its window.
- How a model reads points off a shot depends on the model. An image bigger than the model sees is
  scaled down before it looks (Claude: 1568 px a side and about 1.2 MP, so 1920x1080 is seen at
  ~1456x819; other providers have their own limits). Some models map what they read back to the
  file's pixels (Opus 5.5, Sonnet 5.5: full size read 1:1); others answer in the smaller image they
  saw (Haiku 4.5: every point 0.76x the real one, a median 347 px off, no click on target), and
  nothing says the click went elsewhere. A shot no bigger than what the model sees, plus `click
  --in`, worked for every model measured: **to click off a whole screen or a big window, `shot --fit
  1456`** (16:9 on Claude; a squarer screen a little less, the limit is an area) **and `click --in`
  it**, whatever the screen size (it also stays under the 2000 px past which Claude Code downscales
  with a "multiply by" note). A `-g` crop, a `--changed` crop or a window already that small needs no
  `--fit`. Full size reads small text; do not click off it. A small model still missed about a third
  of small targets at 1456: a `-g` crop around one is the next step (not measured). NOTES finding 251.
  Whoever runs a small model sets it once: `omabox config shot-fit 1456` (or `OMABOX_SHOT_FIT=1456`)
  makes every shot `--fit 1456` unless one is given (stderr says so, `click --in` maps it); `--fit 0`
  is one full-size shot (small text, or a baseline for `--changed`). The setting is the user's.
  For the model at hand, measure it: `omabox calibrate` draws a test card the size of a full shot
  (no box needed) with six lettered crosses; Read it as you read a shot and answer where each cross's
  centre is, `omabox calibrate --answer "A X,Y B X,Y …"`. It says whether full size reads 1:1, else
  the `--fit N` that maps your readings back within a few pixels and the `config shot-fit N` to
  suggest to the user (never set it), or that no one scale fits (read a new card again).
- One image outweighs anything omabox prints as text. Take the smallest that shows it: `shot --window
  SEL` for one app, `-g "X,Y WxH"` for the part under test (a menu, a field, a bar widget), `--fit
  1280` for a whole screen's layout (full size to read small text). `--wait`'s `at X,Y WxH` is the
  last change only: where to look, not everything that changed.
- After an action, to see what it did: `shot --changed --window SEL` (or the screen's) is a crop of
  what changed since the last whole shot of it, 16 px around (a menu opening: ~65 image tokens
  against ~1.5k), `click --in` it as any crop. Nothing changed: no image and nothing on stdout (exit
  0; stderr says so): do not look for one. The first time, or after a resize, it is a whole shot (said
  why). Changes far apart make one big crop: stderr names each place, for a `-g` of one. A spinner or
  a caret blinking beside the change: `--ignore "X,Y WxH"` (the window's coordinates with --window).
  `--since SHOT` compares with an earlier whole shot instead of the last.
- A colour: `omabox pixel X Y` (several points in one call; `--window SEL` reads the app's own pixels,
  before Omarchy's window opacity blends it and with no pointer over it). `shot -g "X,Y 40x30" --zoom
  8` shows a 1 px border or a glyph's edge unblended. Never read a colour off a scaled shot.
- Frames over time (a transition): `shot --burst N --sheet --diff --after -- ACTION` ("Testing an app
  as a desktop app"), one contact sheet, never a loop of shots. 100 ms apart by default (`--every`; `--every 0`:
  back to back, ~15 ms a crop, ~25 a 1920x1080 screen, over in a fraction of a second, before an
  `--after` action may have landed). The whole burst is one capture client, so Hyprland's `screencast`
  events come once and a shell or widget that works on each event is not held up at any pace (a grim
  a frame stalled its timers). `N of M frames identical to the one before` on stderr: nothing new
  drawn between them.

- A look compared with a baseline over time, no person looking (an app's or a plugin's screens after
  a change): `omabox check`, next section.

## Visual checks

For "does it still look right" without a person looking: `omabox check`. Write the scenario once, a
script that sets the box up and drives it with omabox commands, with `omabox check point ID` where
a shot should match (`--window SEL` for an app's window, which brings its tree; `-g "X,Y WxH"` for
a region, a bar widget's; both, `--window SEL -g "X,Y WxH"`, for a part of the window in its own
coordinates, its tree then only the elements touching that part; nothing: the screen):

```bash
#!/usr/bin/env bash
set -euo pipefail
H=$(omabox path --home)                               # the box HOME, /home/sbx inside
cp -r tests/fixtures/library "$H/library"             # data the app reads, before it starts
omabox run -- touch -d "9 months ago" /home/sbx/library/old.txt   # relative to now: "9 months ago" each run
omabox check point bar -g "1700,0 220x26"            # the widget, before anything runs
omabox run -d --wait -- ./build/myapp
omabox wait --timeout 20s window myapp
omabox check point main --window myapp
omabox check point toolbar --window myapp -g "0,0 800x48"   # a part of it, in its coordinates
omabox keys --window myapp --wait ctrl+comma
omabox check point settings --window myapp --ignore "10,400 120x20"   # its own clock: left out (window coordinates)
```

The scenario runs on the host with `OMABOX` naming its box, so a bare `omabox` command goes there
(and is the omabox running the check: its bin comes first on PATH). SCENARIO is a program: a path,
or a name looked up in PATH (`test` or `true` is `/usr/bin/test`, never a shell builtin). Files go in from the host under `$(omabox
path)/home`, or through stdin: `omabox run -- bash -s < setup.sh` runs a script of yours in the box.
Seed data before the app starts: a redraw it does on its own can finish before a `wait change` after
it starts, and that wait then times out; else `omabox wait still`. `check point` waits until the
shot is still (15 s, `--timeout`; then takes it anyway, noted), full size whatever `shot-fit`.
`--ignore "X,Y WxH"` is in `-g`'s coordinates: the window's with `--window` (a crop's too), else
the screen's. A popup (a bar widget's, a menu)
goes over an empty workspace for its checkpoint: holes (below) show only where the wallpaper is
behind.

Times: only the bar's clock is frozen ("Friday 12:00", by its format); the box's processes run on
the real clock, in UTC. So make every time the app shows relative to when the scenario runs:

- Seed data relative to now, in the scenario: `omabox run -- touch -d "9 months ago" FILE` for a
  "9 months ago" (mtimes), a reset time of `$(date -d "+2 hours 10 minutes" +%s)` for "resets in
  2h 10m", never a fixed date.
- A time a daemon in the box stamps as it runs ("added 16:37", a log line's time): `--ignore` its
  region at that checkpoint.
- An app with test hooks of its own (a fixed "now", a fake clock): use them.
- Absolute dates and "today" displays (a date column, a calendar's month, "added today") drift from
  run to run whatever you seed: `--ignore` them.

An `--ignore` region leaves out the tree's elements wholly inside it too, so it must hold the
element's whole box, often wider than its text: an `unstable` tree line says the box (`(label "added
today 02:13" @1512,91 368x15)`); `tree --find TEXT` before.

`omabox check run --approve [--plugin PATH]... -- ./visual.sh` runs it three times (`--runs N`), each
in a fresh `up --clean` box (Omarchy's bar on tokyo-night, a `#ff00ff` wallpaper, the bar's clock
frozen, English (en_US.UTF-8, else C.UTF-8) in UTC whatever your locale and zone, no pointer, no
network, no animations; `--keep` takes
any back, `--keep locale` your locale and timezone, `--keep fonts` your fonts (none by default); `--theme`/`--size`/`--net connected` as `up`),
says how long each box took to start, and writes the baseline to `./omabox-check` (`--base DIR`;
commit it with the project). It also writes `baseline.png` (path printed): every checkpoint as kept,
in one image (more than about ten: pages, `baseline-2.png`..., one look each, all named), unstable
regions in amber, holes in red. Read it before you keep the baseline: the
baseline is what most runs showed, so a screen that was wrong in most of them (an error dialog, a
page that had not loaded, an empty list) is kept as right. "unstable ID"
names what was not the same from one run to the next, left out from then on: one run of three
differing is the app (a layout that changes from start to start: worth a look) or a minute that
ticked; every run differing is data or a time the scenario does not pin (a random port, a time the
app shows): pin it in the scenario's data or `--ignore` it (Times, above). Then `omabox check run -- ./visual.sh`
(same UP OPTIONS) after a change: `same ID` (with what it left out, `(12 specks left out, the
largest 9 px)`), or `changed ID` with each region (`at X,Y WxH`) and the tree's
`moved/resized/name/text/state/appeared/vanished` lines (an element with no name says where it is:
`appeared  button  (in frame "W" > panel#1 > button#3)`); a checkpoint whose change is an earlier
one's (the same tree lines and no other pixels; without a tree, the same regions) is one line,
`changed  ID: as FIRST (...)`, counted with the others (`18 changed (16 as another)`), and named on
FIRST's card in the report; `new`/`missing` for checkpoints the
baseline lacks or this run did not take; the last line counts each kind (`check: 2 of 3 same, 1
missing`). When anything changed, Read the `report:` image first: one
image, ~150-800 image tokens, each change cropped from the baseline beside this run (red: pixels,
blue: a move only the tree has), with its checkpoint, region and tree line; what did not fit is
named. `ID.diff.png` is the whole shot marked (~1.5k tokens each): for the context a crop lacks. Exit
0 all same, 1 not, 2 called wrongly or could not run (an option, a file, no box, no magick), 3 the
scenario failed (its log named), 4 no baseline. A change you meant:
`--approve` again. Shots are compared exactly, leaving out specks: parts of the difference smaller
than each checkpoint's own speck, measured at `--approve` as twice the largest stray part between
its runs (Qt Quick's text varies by a few pixels a glyph from one start to the next: 22 px for an
app on NVIDIA; 16 at least, 32 at most; every checkpoint of one window takes the window's largest),
and 8 where the runs agreed to the pixel (the bar, a GTK app: a few runs can agree on a window with
little text, and the next show a pixel of it), so a 2 px spacing change in a bar widget (16 px) is
caught. `--speck N` sets one for all
(a baseline from before this: 32). A change smaller than the speck shows only in the tree.
Baselines are per GPU: another GPU, omabox, Hyprland, Omarchy, theme, size, locale or timezone is
said in the report. A check's tree is a window's: the shell's and its plugins' checks are pixels only. `omabox check
diff A.png B.png [--ignore G] [--speck N] [--report R.png]` compares two shots of yours the same way
(32 px specks unless `--speck`; exit 0 same, 1 not, 2 an error). Not inside ai-jail yet.

While you work on one screen, check only its checkpoints: `omabox check run --only settings,settings-*
-- ./visual.sh` (IDs or globs, comma-separated). A plain run drives the whole scenario every time
(37 checkpoints: 2.5 min, 7-9 with `--approve`); `--only` skips the others, counted as skipped, not
missing (`check: 2 of 2 same, 35 skipped (--only)`), and refuses a name that matches none of the
baseline's before a box starts. `check point` takes no checkpoint it does not name (it says so, exit
0), but the scenario still drives the app to those screens, where the time is: guard each section
with `omabox check wants ID...` (exit 0 when the run takes any of them, always without `--only`; 1
when none), the list is also in `OMABOX_CHECK_ONLY`:

```bash
if omabox check wants settings settings-dark; then
  omabox keys --window myapp --wait ctrl+comma
  omabox check point settings --window myapp
  # ...
fi
```

`--approve --only ID,...` replaces those checkpoints in the baseline (adds a new one) and keeps every
other one byte for byte; `baseline.png` shows the ones approved. A part approved on another setup
(GPU, Omarchy...) keeps the rest's `check.json`, said. Run the whole check (no `--only`) before the
work lands: a change to one screen can move another.

For people (a README, a site, a release video), `--report-scale N` (2 to 4; `check run`, and with
`check diff --report R.png`) also draws the report N times its size, `report@Nx.png` beside
`report.png` (`baseline@Nx.png` at `--approve`): the same layout, its text, marks and crops N times,
a crop at its shot's own pixels on a box of scale N (`--size 3840x2160,scale=2` with 2). Agents read
`report.png`, kept one look whatever the box's scale.

Each checkpoint is also searched for holes: the `#ff00ff` wallpaper showing through inside a window,
the bar or a popup (a transparent part, a gap, a card that forgot its background), and in a
`--window` shot fully transparent pixels. The wallpaper around and between surfaces and rounded
corners are not holes. A hole shows only where the wallpaper is behind it: take a popup's checkpoint
over an empty workspace, not over your app. `--approve` says each (`holes ID: the wallpaper shows
through at X,Y WxH`) and ends `check: HOLES in N of M checkpoints`: look before keeping that
baseline; it keeps them, and a check then fails only on holes it lacks (`holes ID: ... the baseline
does not have`, marked on `ID.diff.png`), even ones too small to change the shot. None are looked for
with `--keep wallpaper` or after a theme set in the box.

## Symptoms

| Symptom | Next step |
|---|---|
| "could not connect to display", `omabox-guard` | The agent guard: do it in a box ("The guard, in detail"). |
| Your own shell died after `pkill -f PATTERN` | It matched its own command line: kill by PID, or `omabox run -- pkill -x NAME`. |
| After a rebuild the app still shows the old build (a single-instance app raised the old window) | Restart it with `omabox run -d --replace --wait -- CMD`, not a kill and a new `run -d`. |
| `unsatisfied: nothing changed` (124) after `--wait` | Shot; right window focused (`omabox windows`)? Do not resend. |
| `run -d` (`--wait` or not): "the command exited with status N … left nothing running" | It ended at once, or before drawing: read the log lines it printed (a file under `/tmp` or HOME the box does not have is named), fix that, run it again. No shot. |
| "warning: run -d job CMD (pid N) exited with status N after 2.1s, since the last command" | A `run -d` job ended within its first 30 s, after `run -d` had said "started" (a server dying a second in): read its log (`omabox log run`, or the path given), fix it, start it again. Said once, by whatever command came next; not for a job you killed with `omabox run -- pkill`/`kill`. |
| "its Wayland connection was closed for a protocol error: …" after a job's end | The compositor dropped the app for a request it refused; the line is from its log (libwayland's names the object and the error; GTK's and Qt's own do not). "…was lost" or "…failed": the connection broke otherwise (the box going down, the app's own fault). A bug in the app or its toolkit (or the compositor), not the box: rerun it with `--env WAYLAND_DEBUG=1` to see the request that preceded it. |
| `click --wait` 124 on a checkbox or small toggle | A change under the cursor (from ~16 px above and left of the click to ~48 px below and right) is ignored as the cursor: shot, do not click again. |
| `unsatisfied: still changing` (124) | Something never stops moving (a spinner, a glow): the line names the region; `--ignore "X,Y WxH"` it, or `-g` the part under test. |
| Context filling up with screenshots | Text checks first (`windows`, `wait`, `events`, `log --grep`); `shot --window`, `-g`, `--fit 1280`. |
| Text went to the wrong window | `keys --window SEL`, or click the field and see it focused. `keys --window` refusing with "a layer (NAME) has the keyboard": a launcher or menu is open and would take the keys: Escape it (twice if its field has text) or close it, then again. |
| A click missed a cropped or scaled shot | `click --in THAT.png X Y`. |
| Clicks land up and to the left of what you read, more so further from 0,0 | You read points in the scaled-down image you were shown: `shot --fit 1456` and `click --in` it; `omabox calibrate` measures the size for you (Shots). |
| The window is not in the shot (covered, other workspace) | `shot --window SEL`; `click --window` raises it. |
| `click --window` says "a layer surface "NS" … is over X,Y" (or `--no-raise` refuses it); a click at the screen's X Y, "… is over the screen's X,Y, on window …" | A panel or menu is over that point and takes the click where it takes input (only its rectangle is known). `omabox wait layer NS --gone`, then click again. |
| `click --window` says "a layer spanning the screen, "NS" … is up … may go to it" | A layer covering the screen, or its whole height or width (Omarchy's notifications and, on 4.5, its dock), takes input only on its items: a click on a toast goes to the toast, elsewhere to the window. Not refused under `--no-raise`. Clicking where a toast is: `omabox run -- omarchy-shell notifications dismissAll` first (Omarchy 4.5 keeps the layer up with no toast, so `wait layer NS --gone` would not return). |
| `unknown: … not rendered` (exit 1) | An interactive box started by an older omabox, or whose window confirm-close replaced, is not drawn while hidden: ask the user; never show its window yourself. |
| `box 'x' is already up, without what you asked for: …` (exit 1) | It lacks those options. Yours: `omabox down` it, then `up` again. Not yours: `omabox up --new` with those options (it starts the box), then `-b box-N` as it printed. |
| `setsid: failed to execute APP` | The box has the host's programs only (`foot`, not `alacritty`). |
| Tray items that stay after their process exits; no tray at all | Quickshell bug: tray tests in a throwaway box (`omabox run`, no box up); `--stock-bar` if the user's bar has no tray. |
| "went down while this command ran", or "box is dead" after `omabox run -- pkill -x Hyprland` (or quickshell, omabox-labwc) | Those are the box itself (`pkill -x labwc` no longer matches its own): kill your own process by PID; `omabox down` then `up` to recover. |
| "box … has no shell", or `ls` says `gone` under SHELL (bar gone mid-test) | It crashed past what Omarchy's launcher restarts: `omabox log shell`, then `restart-shell`. "the shell crashed since the last command" (or "was killed by SIGKILL"): it came back, but what you see changed (a shot may show it starting). A Quickshell of your own crashing is not the shell: omabox leaves its crash dialog to it. |
| "the shell of box … does not answer (hung …)", or `ls` says `hung` under SHELL | Its pid lives but it answers nothing over its IPC: a frozen bar (a crash whose handler hung, or a stopped shell). `omabox restart-shell` kills it and starts a new one; `omabox gdb --shell` first for its stack. |
| "the shell crashed … (report: PATH)" from `up` or `restart-shell` (exit 1) | Read PATH and `omabox log shell`. From `up` before the bar came, the box is gone: `up` again once fixed; after it (the box up), `restart-shell` once fixed. For its stack: `omabox gdb --shell --watch`, the crash again, `omabox log gdb --crash`. As on a desktop, Omarchy's launcher starts it again (up to 5 times a minute), so the bar may be back: the crash still happened. |
| `omabox lua 'hl.dsp…'` printed `HL.Dispatcher` or `function: 0x…` | A dispatcher, returned and not run: `omabox lua 'hl.dispatch(EXPR)'` or `omabox hyprctl dispatch 'EXPR'`. |
| "its Hyprland did not answer (hung? …)", `ls` says `hung` | The box's Hyprland is stuck (a plugin under test?): `omabox gdb` for where, `omabox log`, then `omabox down`. |
| "went down: its Hyprland crashed (SIGSEGV)", `ls` says `crashed` or `going` (it goes `dead` in a moment) | What you sent crashed it (a Hyprland bug, or a plugin's): `omabox down`, `up`, `omabox gdb --watch`, the same step again, `omabox log gdb --crash` for the backtrace. |
| "no box 'default-…' is up" | Run from outside any repo, where your session's box is used only when it has exactly one up: run it from the repo, or pass `-b NAME` (`omabox ls`). |
| A box went down by itself ("idle") | 2h with no omabox command against it (`up --idle 0` keeps one, `--idle 30m`); your session's box still goes when your agent exits; a `run -d` job is not use: a server you only poll over HTTP needs `--idle 0`, and such a box outlives you unless it is your session's: `omabox down` it when done (`ls` says `orphan` for one whose starter is gone). `down --all` takes other agents' and the user's boxes too. |

## Tests that touch the desktop

- With no box up, `omabox run -- ctest …` starts a throwaway box with the current repo as a
  **discarded overlay** (writes succeed, the checkout never changes), runs, tears down: tray and
  notification tests register with the box's bar, not the user's. Use it for any test that talks to
  the session bus, the tray, notifications, the keyring or a compositor; tests that talk to 127.0.0.1:
  `omabox run --net isolated --allow PORTS -- ctest ...` (`up`'s options work here). With a box
  already up, `run` uses it and the repo is **read-only** there: a test that writes into the tree
  fails; `omabox down` first. Outside a repo (a scratch copy of a project) nothing of the current dir
  is in the box and the command runs in its HOME: `omabox run --overlay "$PWD" -- python3 tests/x.py`.
- `run` gives the command the box's environment, not your shell's: a variable a test needs (a test
  server's password from `dev.env`; a test that skips is the sign) goes with `--pass NAME`, off the
  command line, or a whole file with `--env-file`: `omabox run --env-file ./dev.env -- ctest …`
  (KEY=VAL lines, read as data); a plain value: `run --env KEY=VAL` (never a secret: that is in the
  process list). A `run -d` command's output, a Qt app's warnings and QML errors too, is in the log
  file it prints (`-q`: no line; `--print-log`: only the path, on stdout). A plain `run` returns when
  its command exits, even one that leaves a process running (`run -- wl-copy TEXT` puts TEXT on the
  box's clipboard); what that process writes afterwards is dropped: `run -d` for its output.
- Check what a keyring or D-Bus test left behind inside the box (`omabox run -- secret-tool …`), never
  with `secret-tool` on the host: that is the user's real keyring, and `search --all` prints the
  secrets themselves.
- Accessibility (AT-SPI) tests (dogtail, a toolkit's a11y tests, pyatspi) run in a box against its
  own a11y bus, in its runtime dir: GTK apps register on it; Qt ones once `org.a11y.Status.IsEnabled`
  is true (`omabox run -- busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true`),
  AccessKit (egui, winit) from accesskit_unix 0.22 (eframe 0.37) on IsEnabled too; before 0.22 only
  once `ScreenReaderEnabled` is (the same call), a property at-spi2-core 2.61 and later no longer
  have: such an app never registers with them (IsEnabled, or an AT's registered events, do not
  help: it watches that one property).

## The guard, in detail

- Under the user's agent guard your commands get `WAYLAND_DISPLAY=omabox-guard`, an empty `DISPLAY`
  and `HYPRLAND_INSTANCE_SIGNATURE=omabox-guard`, so what would reach the real desktop fails: do it in
  a box. Never set those back to the real session or take the display from elsewhere
  (`/proc/*/environ`, `hyprctl instances`, `$XDG_RUNTIME_DIR/wayland-*`). `xdg-open`, `gh --web` and
  `$BROWSER` fail too: give the user the link.
- Only when the user asked for their **real** desktop in this task ("switch my theme", reload my
  config), run that one command as `omabox host -- CMD`. Never hand them a `! CMD` that touches their
  desktop (their `!` is guarded too): their own terminal is for that. Never use `host` to switch
  their workspace or focus to see a box.
- Before touching the real desktop for a check a box cannot make, ask once: "When a box cannot test
  it, in detail".
- `QT_QPA_PLATFORM=offscreen` still works under the guard. To look at a web page yourself, open it in
  your box (`omabox run -d -- xdg-open URL`, then `omabox shot`). `quickshell kill`, `qs ipc` and
  `omarchy-shell` are refused ("not running quickshell kill"): they would reach the user's desktop
  shell; a box's shell is `omabox run -- qs ipc …` (or `omarchy-shell …`) or `omabox restart-shell`.
- `omabox host -- CMD` examples, once the user asked for their real desktop: `omabox host -- hyprctl
  reload`, `omabox host -- omarchy-theme-set NAME`. Testing, screenshots and anything the user did not
  ask to see on their desktop stay in a box: `shot` works on a hidden interactive box.
- "this hook is outdated (omabox changed it)" at session start: tell the user to run `omabox guard on`
  (it rewrites your own settings, so leave it to them).

## Inside ai-jail

omabox works through the user's broker (no `host`, `peek` or `--interactive`). "this sandbox refuses
new namespaces": ask the user to run `omabox broker on` outside the jail and add what it prints to
`~/.ai-jail`. Never try to get around the jail. The jail's project goes into the box only when it is a
git repo; `up` says so of one without a `.git`: `up --ro-bind DIR` (or `--overlay`) brings it in.

## Commands that run without a prompt

The skill's `allowed-tools` pre-approves the verbs that act only inside a box and read none of the
user's files: `shot`, `pixel`, `windows`, `window`, `tree`, `keys`, `click`, `scroll`, `drag`,
`pointer`, `wait`, `jobs`, `stop`, `log`, `events`, `hyprctl`, `lua`, `restart-shell`, `reload`,
`mode`, `output`, `monitor`, `cpu`, `audio`, `gdb`, `calibrate`, `ports`, `ls`, `path`, `brief`,
`help`. Each is a rule `Bash(omabox VERB *)`, so:

- **Claude Code** applies it during the turn that invoked the skill (a subagent's whole run; in a
  session, until the user's next message), only to `omabox VERB ...` as typed: `omabox -b NAME shot`,
  `bin/omabox shot` or `FOO=1 omabox shot` still ask. Put `-b NAME` after the verb. A `&&` chain is
  pre-approved only when every part is.
- **Still asked:** `host`, `peek`, `keys-to-box`, `clip` (the real desktop and clipboard); `config`,
  `guard`, `setup`, `broker` (the user's settings); `up` and `run` (a rule cannot tell `up` from `up
  --interactive`, or a box's `run` from a throwaway's `--ro-bind`, `--seed`, `--plugin`,
  `--desk-shell` or `run --env-file FILE`); `down` (`--all`), `save`, `saves`, `gpu` (`gpu release`
  ends other boxes); `record` (`record sheet VIDEO` reads any video), `check` (`check run` runs a
  scenario on the host); `env`, `which`, `--version` (harmless, rare).
- **What the listed ones write on the host:** only the images named with `-o` (`shot`, `drag
  --shot`, `calibrate CARD`; a burst's `-o DIR` frames), at a new path or over an earlier image or
  an empty file: a link, a folder or any other file there is refused (`is there and is not a PNG`),
  untouched. `keys --pass VAR` types a variable of
  the agent's own environment, as the user exported it. `shot --after`'s action takes no
  `--env-file`. Keys, clicks and `hyprctl dispatch exec` run anything inside the box, which reaches
  the network as the box does (ref: Network): a box the user started is still theirs.
- **Codex** reads no grants from a skill: the user adds `prefix_rule(pattern = ["omabox", "shot"],
  decision = "allow")` lines to `~/.codex/rules/default.rules`. **OpenCode**: `"permission": {"bash":
  {"omabox shot *": "allow"}}` in `opencode.json`. **pi** asks for nothing; **Hermes** follows its
  own approval settings. Only the user changes these files.

## Screen size and rendering cost

- `omabox up --size 3440x1440` for another screen size (@60), `--size 3440x1440@144` for a refresh
  rate, `--size host` for the user's own monitor; `omabox mode` shows or changes it on a running box.
- **Crisp, high-resolution screenshots** (docs, a README, a zoomed-in detail): `omabox up --size
  3840x2160,scale=2` (or `omabox mode 3840x2160,scale=2`), a 4K screen laid out as 1920x1080: the bar
  and apps at their usual size in twice the pixels, so `shot` is 3840x2160 and stays sharp when
  zoomed or cropped. Coordinates are still the layout's (1920x1080); `click --in SHOT` maps a shot's.
  Scale 1 to 4 (`7680x4320,scale=4` for 8K), headless boxes only; `mode WxH` without one is scale 1.
- **One window at a width** (a narrow layout, a sheet that wraps, a breakpoint): `omabox window
  --window SEL --size 760x600`, then `--size 540x600` for the next, not `mode` (the whole screen).
  It floats the window, sizes it in layout pixels (as `windows` gives sizes) and centres it on its
  monitor's free area, in the box's own Hyprland; it prints `0x... CLASS "TITLE" floating at X,Y
  WxH`. A size the app does not take (its smallest or largest) is exit 1, naming the size it took:
  check at that one or say so. `--tile` puts it back in the layout, `--float` floats it as it is;
  a fullscreen window is refused. `wait still` before a shot, while the app redraws. Not
  `hyprctl dispatch togglefloating`: under the Lua config it prints a note and does nothing.
- **More monitors** (a bar on each, windows between them, portrait, ultrawide, mixed scale, an
  unplug): `omabox up --monitor 1080x1920 --monitor 2560x1440,scale=1.6,below` (SPEC
  `WxH[@HZ][,scale=S][,right|below|X,Y]`, right of the last made by default, moving with it), or
  `omabox monitor add SPEC` / `remove NAME` (an unplug) / `list` on a running box. Coordinates are the
  layout's (`pixel` refuses one in a gap between monitors);
  `shot --monitor NAME` (`--fit` for a mixed-scale layout, which comes out at the highest scale),
  `drag --shot F --shot-monitor NAME`; `peek --monitor NAME` for the user, one window per monitor
  (`shot` is the whole layout). Names: HEADLESS-2 (the first), HEADLESS-3, ...; on NVIDIA WAYLAND-1,
  WAYLAND-2, ... (`monitor list`). On Omarchy 4.5 each monitor has its own ten workspaces, as on a
  desk: the main screen 1-10, the first added 11-20, the next 21-30 (the bar labels each 1-10). An app that makes its own output (`hyprctl output create
  headless`, a streaming helper's virtual screen) gets one listed **0x0 in a box on NVIDIA**, with no
  error (`monitor list` and `shot` say so): test it in a box on the AMD/Intel GPU,
  `OMABOX_RENDER_NODE=/dev/dri/renderDN omabox up --new` (`omabox config gpu` lists them); with no
  other GPU it cannot be tested here. In an interactive box (the user's) each monitor is a window on their
  desktop, the box's layout scaled as a whole (each a view: the size asked for, at a scale below 1):
  only when they ask for it. Those windows have class `aquamarine-omabox-<token>` (the main one
  `aquamarine`), and need omabox's aquamarine build: a refusal says `omabox setup --aquamarine`, then a
  new box, which is the user's call.
- **Measuring rendering cost** (GPU time of an animation, a repaint loop): `omabox up --size host`,
  put the UI in the state to measure, then `omabox gpu 10` (% of wall time per process, this box
  only; `--json`). Never read host-wide tools (nvtop, radeontop, scripts summing `/proc/*/fdinfo` by
  name) while a box is up: they add the box's Hyprland and quickshell to the user's. Match the mode:
  anything that repaints per frame costs ~2.4x more at 144 Hz than at the default 60. On NVIDIA,
  whose driver has no per-process DRM counters, `gpu` reads the driver's own sampling (`nvidia-smi
  pmon`, `source` in `--json`): whole seconds, % of the GPU each process was seen using once a second;
  a light process can read 0, and one saturating the GPU leaves the others at 0. Compare runs, not
  absolute figures, there.
- **wgpu apps (eframe's default renderer, iced, Bevy) do not start in a box on NVIDIA**:
  `RequestAdapterError(NotFound { active_backends: VULKAN | GL })`; wgpu does not fall back to GL
  there. Build the app with a GL renderer (eframe's `glow`), or run it in a box on the AMD/Mesa render
  node, whose Vulkan (RADV) a box has: `OMABOX_RENDER_NODE=/dev/dri/renderDN omabox up --new`
  (`omabox config gpu` lists them).
- **Apps in a box that render in software** (llvmpipe; on NVIDIA they draw on the GPU, but Vulkan
  there still fails). Their CPU and frame cost measured there is software rendering's, and `gpu`
  counts none of their drawing. `up` says so once, `ls` and `gpu`/`cpu` show an `apps: llvmpipe, in software` line,
  `ls --json` has each box's `client_render` (`renderer`, `software`; from `eglinfo`, absent without
  mesa-utils). For GPU rendering use a box on another GPU (`OMABOX_RENDER_NODE=/dev/dri/renderD128
  omabox up --new`, the node `up` names); with no other GPU, compare against a baseline measured in
  the same box and report only the difference.
- **Measuring CPU and memory** (an app's idle cost, a busy loop, a leak): `omabox cpu 30 --settle 5
  --app NAME` (or `--pid PID`, as `omabox run -- ps -e` numbers it, or the host's; neither: every
  process of the box), CPU as % of one core over the window, RSS and PSS as last seen (`--json`:
  `processes[]`, `total`). `--settle` stands for the `sleep` you would put before measuring.
  `--threads` (with `--app`/`--pid`): each thread's CPU by name, an app's main thread apart from its
  render thread (`QSGRenderThread`); `--json`: `processes[].threads[]` (`tid`, `name`, `cpu`). Not
  `top`/`pgrep` on the host: they mix the box's processes with the user's (`omabox which PID`).
- Which GPU: headless boxes render on the one the user's `omabox config gpu` names (`auto`: the first;
  `up` says when it fell back because that GPU is gone, and why; `omabox gpu` names the box's; `ls
  --json` has each box's `render`: node, pci, driver, fallback, and for one the setting's `wanted` and
  `why`). It is their setting: ask before changing it. `omabox gpu release GPU` takes every
  box on a GPU down, others' too: the user's to run, never yours.

## Pointer, in detail

- **The pointer is test state.** Under Omarchy's focus-follows-mouse the window the pointer rests on,
  or last passed over, takes focus, and gets it back when a menu or panel closes. A box's pointer
  starts at the screen's centre and stays wherever the last command left it (`omabox hyprctl
  cursorpos`). `click` and `pointer -- move` jump: they cross nothing on the way. Before a test whose
  result depends on focus, put the pointer where a user's would be (`omabox pointer -- move X Y`), and
  when the way there matters (to the bar, across other windows) travel: `click --steps 20 X Y`,
  `pointer --steps 20 -- move X Y`. `--mod ctrl` (shift, alt; `ctrl+shift`) holds modifiers across a
  click or drag; SUPER with a button is Hyprland's own (move, resize), never the app's.
- `omabox drag [--window SEL | --in SHOT] X1 Y1 X2 Y2 [BTN] [--steps 10] [--hold DURATION]
  [--mod MODS] [--shot FILE | --wait]`: press at the first point, move to the second in steps, hold
  there `--hold` (a drop target reacting to the hover), release. `--hold` is a duration, `300ms` or
  `2s` (a bare number is seconds, up to 60s), as every duration is: `300ms`, `1.5s`, `2m`, `1h`.
  `keys -s 300`, `pointer sleep 300` and `up --idle 45` are refused (milliseconds and minutes up to
  0.9): write the unit.
  `--shot FILE` takes a shot while the button is still down (a drag's own feedback); `--wait`
  ignores the cursor along the path, so a change wholly under it may not count. Both points are
  mapped as for `click`. It jumps to the first point: to get there on foot, `pointer --steps N --
  move X1 Y1` first.
- `omabox pointer [--window SEL | --in SHOT] [--steps N] [--mod MODS] -- move X Y [--steps N], click
  [BTN], down [BTN], up [BTN], scroll DY, hscroll DX, source wheel|finger|continuous|tilt, sleep DURATION` in
  one run: raw, it raises nothing (a `--window` must be on screen and uncovered). BTN is left (the default), right, middle, back or forward (a mouse's thumb buttons, also `side` and `extra`; `click X Y back` too). A button pressed with `down` stays
  down after the call, until an `up` (in a later call too): end every `down` with an `up`, or the
  box's next clicks are drags.
- `omabox pointer hide` / `show`: the pointer out of every shot and recording of the box until `show`,
  over reloads and theme switches. It still moves, clicks and gives focus (focus follows it).
- `omabox scroll [--window SEL | --in SHOT] [--wait] [--mod MODS] X Y DY [DX] [--source wheel|finger]`:
  the pointer there, then a scroll; DY down, DX right (negative: up, left), 15 a wheel notch. `--source
  wheel` is a mouse's notches (one per frame: apps counting them see each; a DY that is not whole
  notches is rounded to the nearest, at least one, and said: 20 scrolls 15), `finger` a touchpad's
  smooth scroll ending in a stop (kinetic scrolling); none is one plain event. Hyprland binds see the
  wheel as `mouse_down`/`mouse_up`/`mouse_left`/`mouse_right`, at most one per
  `binds.scroll_event_delay` (300 ms) as from a real wheel; `--mod super` reaches Omarchy's
  SUPER+wheel binds (workspaces, groups). `--source tilt` (a wheel pushed sideways) and `finger` reach
  apps but no Hyprland bind (it binds the wheel source only): test binds with `wheel`.
  Qt apps get every wheel, a real mouse's too, from a device of type TouchPad (Qt's Wayland plugin
  registers no other, QTBUG-112432): a Qt Quick `WheelHandler` at its default `acceptedDevices`
  (Mouse) never fires, in a box or on the desktop. Not omabox's to fix; for the app:
  `acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad`, or a `MouseArea`'s `onWheel`.
- `keys --pass VAR` types the value of a variable from your environment, shown nowhere: a secret
  never goes in `-t TEXT`, which the process list shows.
- `keys -t TEXT` and `--pass VAR` type a newline as Return and a tab as Tab; any other control
  character (backspace, escape, a carriage return) is refused, exit 2, before anything is typed. A
  `--pass` value's one trailing `\r` (a Windows line ending) is dropped.
- **A held key** (`keys down:KEY` … `keys up:KEY`): KEY stays down across the commands between
  (`pointer move`, `click`, other `keys`), for a bind on a key's release (`hl.bind(KEY, fn, {release =
  true})`) or a gesture made while a key is held. One key each (`down:XF86Tools`, `down:super`,
  `down:a`). It joins the other keys as on a real keyboard: `down:super` then `keys a` fires SUPER+A,
  `down:shift` types capitals; a held letter repeats in the focused app until another key is pressed.
  A second `down:` of the same key, or an `up:` of one not held, is exit 1, before any key of that call
  is sent (`keys a up:super` with nothing held types nothing). A holder killed outright leaves its keys
  down: the next `keys` lets go of them, once, and says so (an `up:` of one is that release). A key still held goes with
  the box (`down`, or your agent exiting, for a box named after your session): a box you keep (`up
  --new`) keeps it down until `up:KEY`.
- **Travel** (`--steps N` on `click` and `pointer`; 1-999, 1 is a jump): N moves in a straight line
  from where the pointer is, each seen by Hyprland and drawn before the next (~40 ms apart), so
  focus-follows-mouse and hover happen on the way. Pick N so a step (the distance over N) is narrower
  than the narrowest window to cross. `pointer --steps N -- move A move B` goes through A (a path
  around something: a waypoint beside it); `move X Y --steps N` sets one move's. `click --steps` travels
  first, then clicks: `--wait` watches the click, not the way.
- **Modifiers** (`--mod MODS` on `click`, `drag` and `pointer`: ctrl, shift, alt, super, altgr, or
  several as `ctrl+shift` or `--mod` again): held down on the box's keyboard from just before the
  press (for `pointer`, the whole run) to just after the release, then let go, however omabox ends.
  The app sees them as a real keyboard's (a table's ctrl-click and shift-click selection). SUPER with
  the left button is Hyprland's move window and with the right its resize (Omarchy's binds): the app
  gets no click, and `drag --mod super` moves the window. Shift-click in foot is its own selection,
  not reported to the program in it.

## Waiting, in detail

- `omabox wait [--timeout 10s] [--json] COND`, one condition per call (chain with `&&`): `still
  [--quiet 300ms | --like SHOT] [-g GEOM | --window SEL] [--ignore GEOM]... [--strict]`, `change
  [--since SHOT] [-g GEOM | --window SEL] [--ignore GEOM]...`, `window SEL [--gone | --focused]`,
  `layer NAMESPACE [--gone] [--monitor NAME]` (`omarchy-menu`, `omarchy-notifications`, ...), `job
  [PID] [--gone]` (a `run -d` job's end), `log [PID] RE [-i]` (a line in its log), `log LOG RE [-i]
  [--since OFFSET]` (a new line in a box log: shell, hyprland, ...), `tree TEXT [--window SEL |
  --layer SEL] [--gone] [--shot FILE]`, `until -- CMD` (exit 0 inside the box); `wait for DURATION`
  is a fixed time.
  Options go before or after COND. `window SEL` is satisfied by any window SEL matches, and the line
  names them all (`--focused`: when one of them has focus); only commands acting on one window
  refuse a SEL that matches several. `layer`'s line names the monitor of each one drawn (`at
  0,0 1920x1080 on HEADLESS-2`; a bar on every monitor is each named): which screen a popup opened
  on, without parsing `hyprctl layers`. `--monitor NAME` waits for one on that monitor (`--gone`:
  none left there, after `monitor remove` or `output drop`); `--json` has it in `detail`.
- A label, a state, a sheet: `omabox wait tree TEXT [--window SEL]` (none: the active window) until
  the accessibility tree holds TEXT, matched as `tree --find` matches it (any case; `[STATE ...]` at
  its end: each among the line's states, any order: `'"Dark mode" [checked]'` finds `[focused
  checked]`, never `[unchecked]`), read about every 100 ms (each read given the time left: a hung
  app keeps `--timeout`): satisfied at the first read
  holding it (a toast or a sheet that lasts a second is caught), the line giving that line. `--gone`:
  two reads in a row without it, or its window gone. A 124 says what the tree had instead (the lines
  holding TEXT's name, else why). `--shot FILE` takes a shot the moment it holds. No hand-rolled
  `tree --find` loops.
- `keys`, `click`, `scroll` and `drag` `--wait` take `--expect tree TEXT`: once the screen settled, the
  tree must hold TEXT (in the window the input went to: `--window`'s, `click --find`'s; none: the
  active one) by `--timeout`, else 124 with what it had instead. `omabox click --find Save --wait
  --expect tree 'label "Saved"' && omabox keys ...` stops when the effect is not the one wanted; the
  input was sent either way.
- Did it land, without a shot: add `--tree` to an action's `--wait`. The window's tree (`--window`'s,
  `click --find`'s, else the one under the point, else the active one) is read before the input and
  once the screen settled; the line ends `; no change in the app's tree` or `; the app's tree: N
  changes`, then a line each, two spaces in, 20 at most (`and N more`): `entry "Name": '' → 'hello'`,
  `check box "Beta": +checked -unchecked`, `list item "Pear": +selected`, `label "Idle" → "Saving"`,
  `+ dialog "Confirm" (and 3 inside)`, `- menu "File"`. A password field's text (and any text after
  `keys --pass`) is never shown: "its text changed (not shown)". No tree (a terminal): `; no tree to
  compare (why)`. It costs a read before the input (the box's first: ~3 s), so only when the answer
  replaces a look.
- An app that redraws later, by itself (after a server sync, a timer): `omabox wait --timeout 30s
  change --window APP` (satisfied at the first change there; the window must be on screen), then
  `omabox wait still` for it to finish. Never `wait until -- false` (or `-- true`) as a sleep: the first
  is a fixed wait ending in 124, the second no wait at all.
- A change you cause without an omabox input (a file the shell applies live, an IPC call, a command
  in the box) can be drawn before a `wait change` starts, which then times out: shot first, act,
  then `omabox wait change --since BEFORE.png [-g GEOM]` (satisfied at once if it already differs,
  else at the first change). `omabox wait still --like BEFORE.png` waits until it looks as before
  (a panel closed, a theme back). SHOT: a full-size shot of the screen, a part of it, or a whole
  window (that window is watched); the pointer and a caret alone do not count, a clock does
  (`--ignore` it). Not for an agent inside ai-jail yet.
- A line in a box log: `omabox wait log shell 'RE'` (a plugin's line, a QML warning), `wait log
  hyprland '\[Lua\] ready'`, any of `log`'s logs by name (a `run -d` job's: `wait log [PID] RE`,
  from its first line); `-i` for case, RE as `log --grep` takes it (grep -E; `--grep RE` works too).
  Only a line written after the wait began counts, printed on the `satisfied:` line; a 124 says when
  one was there before (`--since 0` counts the whole log: after `restart-shell`, which begins the
  shell's anew). `--json` has `offset`, where the line found ends: a next `wait log shell RE --since
  OFFSET` takes the lines after it, so two lines in a row are never missed. A log begun anew during
  the wait is read from its start (said). Hyprland's lines come from its rolling log (its file is
  written 8 KiB late): no `--since` there.
- A time that really is fixed (a timer under test, a debounce of 2 s): `omabox wait for 2s`, where a
  foreground `sleep` is blocked. Its line says it was a fixed time; not for letting an app settle.
- Time that is part of a video is the only time you let pass, and never with a sleep: `omabox record
  -o cycle.mp4 --for 5s` records the screen for 5 s with no action (a panel cycling on a timer: no
  dummy action, the pointer left where it is); in a take, `omabox record hold 2s` between `record
  start` and `record stop` gives the viewer 2 s to read the screen, measured in the video's own clock
  (its frames' times), so a slow box or a slow script never shortens it. `record mark "scene 2" --hold
  2s` marks, then holds from the mark. `wait still` is no hold: it returns at once on a still screen
  and runs to its timeout under a clock or a spinner.
- `keys`, `click`, `scroll`, `drag` and `run -d` take `--wait [--start 2s] [--quiet 300ms] [--timeout 10s]
  [-g GEOM] [--ignore GEOM]... [--json]`: the screen before the action, a change within `--start` (5 s
  for `run -d`), then `--quiet` with none. `-g` watches only that part of the screen.
- The line then says what changed on the desktop since just before the input, from one Hyprland read
  before and one after (~25 ms): `window opened, focused: zenity "Save as"`, `window opened on
  workspace 2: ...` (where a shot shows nothing), `window closed: ...`, `layer omarchy-menu
  opened|closed` (a menu kept mapped at 1x1 is closed), `focus: ...` or `focus: none`, `foot title now
  "..."`, `workspace now 2`, `... moved to workspace 3`, `... now fullscreen` (or floating, tiled, `out of fullscreen`); 3 at most and
  ~160 characters, then `+N more (omabox events --since OFFSET)`. Since the input, not caused by it:
  a toast or a clock's retitle at the same moment is said too. Typing into a field changes none of
  these: the line is as before. A window that opened and closed within the wait is not seen.
- A 124 adds where the input went and whether anything moved: `the keys went to foot "K" (focused)`
  or `to layer omarchy-menu (it has the keyboard)`, `the click at 960,540 was on zenity "Info" (its
  129,113)` (its own coordinates, as `click --window` takes them) or `on layer omarchy-bar`, with
  `, or possibly on layer omarchy-notifications` while a layer spanning the screen is up (it takes
  input only on its toasts), `no window from pid 1240 yet` (run -d); then `no window, layer or focus changed`, or what did (a window
  opened off screen). When it says both, look (`windows`, `tree`, a shot) before sending again; the
  input was sent. `unknown` (1) adds nothing. The exit code is the screen's, never the desktop's.
- `--json` adds `changes` (every change, up to 50: `window_opened` `{window, focused}`,
  `window_closed`, `layer_opened` `{namespace, at}`, `layer_closed`, `focus` `{from, to}`, `title`
  `{window, from, to}`, `workspace` `{from, to}`, `window_moved` `{window, from, to}`, `window_state`
  `{window, floating, fullscreen}`; a window is `address, class, title, workspace`; more kinds may
  come: ignore unknown ones), `[]` for none, `null` when the desktop could not be read; `target`
  (`kind` window, layer or none, `window`, `namespace`, `at`: what got the input; `certain` false
  when a layer spanning the screen may have taken it instead; on a 124 or with --json); `events_since` (`omabox events --since N`: everything from just before the input). `wait
  still|change` has no action and none of these.
- The screen watched is every monitor of the box. `-g` and `--ignore` are the layout's coordinates,
  as for a plain `click` and `shot -g`; beside `--window`, the window's own (0,0 its top left corner),
  as `click --window` takes X Y (0.9 took the layout's, with a warning). The regions the line names
  are the layout's; a 124's suggested `--ignore` is the window's when `--window` was given. One
  `--ignore` outside the region watched is said. To watch a part of the screen whatever the window,
  `-g` without `--window`.
- `--ignore "X,Y WxH"` (up to 16; `--strict` keeps them) leaves out an animation that never stops. A 124
  "still changing" whose late changes were all in one small region ends with `--ignore "X,Y WxH" if
  that is an animation`: add it if the shot shows a spinner or a glow there, not the app under test.
- A caret blinking in place (a change 4 px thin and 40 long at most, where the last one was) and the
  software cursor (in every frame; it hides on a key press) are not changes; the line says what was
  ignored. A longer thin change (a tab's underline, a progress bar) or a short one that moves is one.
  `--strict` counts them all.
- Exit 0 satisfied, 124 not in time, 1 unknown (the box went down; an interactive box that is not
  drawn while hidden: an older one, or one whose window confirm-close replaced). Waiting counts as use
  for the idle timeout.

## Hyprland's Lua, logs and events

- `omabox lua EXPR` evaluates Lua in the box's Hyprland and prints what it returns, where `omabox
  hyprctl eval` prints only `ok`: `omabox lua 'hl.get_cursor_pos()'`, `omabox lua
  'hl.get_active_window().class'`, `omabox lua 'local w = hl.get_active_window(); return w.title,
  w.pid'` (statements need `return`). One line per value: strings and numbers as they are, `nil`,
  tables and Hyprland's objects (a window, a monitor, a layer) as JSON, objects inside them by name
  (`HL.Workspace(1:1)`); `--json` quotes strings too. A long script: `omabox lua - < script.lua`. A Lua
  error is exit 1 with its message. Globals you set stay for the next call (until a config reload).
- A config reload (`hyprctl reload`, `omabox reload`, a theme switch, `omabox mode`, an interactive
  box's window resize) starts a fresh Lua state: binds, rules, `hl.config` values and plugins added
  with `lua`, `eval` or `plugin load` are gone. Re-apply them on `configreloaded`. Nothing reloads
  because a file changed: a box runs its own copy of omabox's config, made at `up`, with autoreload
  off, so an update of omabox or Omarchy leaves it as it is; `omabox reload` takes the newer one.
  A test that must see what a file change does, as a desktop would (a helper that rewrites a file
  the config loads, on a Hyprland event, reloads for ever there): `omabox up --autoreload`.
- The box HOME's `~/.config/hypr/bindings.lua`, `input.lua` and `looknfeel.lua` (`$(omabox
  path)/home/.config/hypr/`) load as on a desktop: append a bind or a setting, `omabox hyprctl
  reload`, and it applies (a plugin's "add this to bindings.lua", its Set up button). A new box's are
  Omarchy's comment-only files, not your desktop's. Never loaded: `monitors.lua` (the screen is
  `--size`/`--monitor`/`mode`'s), `autostart.lua`, and `hyprland.lua` itself. A file with an error
  does not stop the rest loading; `up`, `reload` and `hyprctl reload` print the config's errors
  (`omabox hyprctl configerrors`). A box draws no red "Your config has errors" bar
  (`debug:suppress_errors` is on, whatever `looknfeel.lua` says: drawing it can hang Hyprland 0.56):
  a shot never shows the errors, those commands do. A Hyprland that answers the reload and then
  stops answering is said there ("did not answer hyprctl configerrors"), exit 1: `omabox gdb`.
- Hyprland's own notifications (`omabox hyprctl notify`, and the ones it raises itself, such as "Your
  monitor layout is set up incorrectly") are drawn at the top right of the focused monitor, below
  the bar, sliding in over 0.6 s: a shot of the screen's left (`-g 0,0,...`) never has them.
  `hyprctl notify` says the `shot -g` of that corner. The layout warning is in the Hyprland log as
  "Monitor NAME: detected overlap with layout" (`omabox log --grep overlap`); a notification's own
  text is not logged.
- An error inside a callback (`hl.on`, `hl.timer`) is not in `lua`'s answer, and when the callback
  runs later (a timer, an app's event) it is logged nowhere, not even in the Hyprland log: seen
  nothing, check with `pcall` inside the callback and keep the error in a global to read with
  `omabox lua`. (An `hl.on` callback that a `hyprctl dispatch` sets off errors in that dispatch's
  answer.)
- `omabox log [LOG...|all] [-n 100|all] [--grep RE [-i]] [-f]`: the box's logs, the last 100 lines
  of each. `hyprland` (the default), `shell` (the bar, plugins, QML errors; each line with its
  time to the millisecond, `YYYY-MM-DD HH:MM:SS.mmm`, for timing what the shell does), `apps` (what the
  launcher and binds started), `run` (the latest `run -d`), `keyring`, `labwc`, `systemd` (with
  `--systemd`), `audio` (PipeWire's, with `--audio`), `polkit` (the stand-in authority's, with `--polkit`), `events` (Hyprland's events, which `omabox events` reads), `journal` (what programs
  sent the box's journal sockets: dbus-broker, `logger` and `logger --journald` (not `-t omarchy-shell`: `shell`), `/usr/bin/systemd-cat` (the bare name writes `~/ID.log`), Qt with `QT_FORCE_STDERR_LOGGING=0`, units' output; at most 4 MB, then
  `journal.1.log`), `box` (bwrap), `reap` (the
  reaper's: idle and agent checks, the down it did). A box that died keeps its logs until `down`: read them to see why.
  `-f` follows until the box goes down (exit 0). `-i` (ignore case) goes anywhere, `--grep -i RE`
  too. Hyprland writes its log file 8 KiB at a time: `log hyprland` of a live box adds the lines
  not in the file yet from `omabox hyprctl rollinglog` (its last 4 KiB), and says on stderr when the
  file is further behind than that (the lines between come later). `-f` follows the file, so a line
  about what just happened can come many lines later; `omabox hyprctl rollinglog` has it at once.
- `omabox events`: Hyprland's event stream (`activewindow>>`, `urgent>>`, `openlayer>>`,
  `workspace>>`, ...) as the box recorded it from its start, one stamped line each (`--json`:
  `{time, event, data}`). Never hand-roll a socat on `.socket2.sock`, and never truncate a log
  something is writing (NUL-padded files, "no events" when there were some). To look at what one
  step caused: `omabox events --mark m1` (a byte offset, kept by name; nothing is cleared), act, then
  `omabox events --since m1 [--grep '^urgent>>']`. `--since 30s` also works. `--until RE [--timeout
  10s]` waits for the first matching event from the mark (one that came already counts) or from
  now: 0 with the event printed, 124 none in time, 1 the box went down. `-f` follows. E.g. "did the
  app ask for activation when relaunched?": mark, relaunch, `events --since m1 --until '^urgent>>'`.

## Mounts, HOME and the session

- In short: a box is a full Omarchy desktop (its Hyprland config, shell, theme, tray, notifications,
  keyring). Read-only in it are the repo (same path), `--ro-bind`, `--plugin` and `--theme-dir`
  dirs, mise's toolchains and the fonts; nothing else of the user's HOME. Mounting HOME,
  `~/.config/omarchy`, `/tmp`, secret stores or the runtime dir is refused: do not work around it.
  The box HOME is fake (`/home/sbx`; `omabox path --home` on the host). The box's /run/user/$UID is
  `omabox path --run` on the host; the host's own is the real session's: never read app state there.
  Private session bus and keyring; audio: `up --audio`; a polkit agent: `up --polkit`; `/sys`: the host's.
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
  `desk_fonts`). Not in an `up --clean` box (`--keep fonts` brings them), not with `up --no-seed
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
  can see.
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
- A crash in a box leaves no core file and no crash notification on the user's desktop (the core
  limit is 1 byte). To get a core: `omabox run -- bash -c 'ulimit -c unlimited; exec ./app'`.
- Logs: `omabox log` prints them (below); `omabox path --logs` says where each file is.
- `run -d` jobs: each is its own session, with a log (`run-*.log`) named on stderr, as the host has it
  and as the box does (`/home/sbx/run-*.log`: the one to give a command in the box, `wait until --
  grep -q X /home/sbx/run-….log`); `-q` drops that
  line, `--print-log` prints only the host path on stdout (first, before a `--wait` line). A job that
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
  desktop): `run -d` says the job's pid ("started in box 'B' (pid 115, log: …)"). `omabox jobs
  [--json]` lists the box's running jobs (PID, AGE, PROCS, LOG, COMMAND). `omabox stop PID...`
  (or `--all`) stops one as `--replace` does (SIGTERM to its session, SIGKILL after 5 s, its windows
  gone), through a pidfd of each of its processes; PID may be any pid of its session, a window's PID
  from `windows` too. A stopped job is not said as an early end. Another process of the box: `omabox
  run -- kill PID`. `omabox wait job [PID]` (none: the latest job) until its command exits,
  satisfied with how ("exited with status 3"), at once when it already had; `--gone` until nothing
  of its session runs either (a launcher's app). `omabox wait log [PID] RE [-i]` until a line of
  its log matches RE (grep -E; a line there already counts), satisfied with the line; a job that
  ended without one is `unknown` (exit 1). Not `wait until -- sh -c '! pgrep …'` (it matches
  itself) nor a loop over `run-*.log`.

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
- An app that judges its network by where a peer comes from (a BitTorrent client reads
  "firewalled" while every peer is on 127.0.0.1): `omabox up --net isolated --addr 203.0.113.10
  --addr 203.0.113.11` puts those on the box's loopback; bind the server to one and the peer
  (client) to the other, and the server sees it come from there. Documentation ranges only
  (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32); `ip addr add` in the box is refused,
  so give them at `up`. The `--allow` ports still arrive on 127.0.0.1, not on these.

## Box lifetime

- Your box is named after the repo (or worktree) and your session, so no other session shares it;
  subagents share it (parallel ones each need their own: "Handing box work to a subagent").
  `omabox down` before `/clear`. A box of a subagent's own: `omabox up --new
  [--plugin …]`, all its options on that call, then `-b box-3` (as printed) on every later call, typed
  out (your shell keeps no variables); it outlives you: `omabox down -b box-3`. A box the user
  started: `-b NAME` (`omabox ls`). `omabox down` never with `--all`: it takes others' and the user's
  boxes. A box idle for 2h goes down by itself (`up --idle 0` keeps one).
- Your session's box goes down when your agent (Claude Code, Codex) exits. A peek window or an
  `omabox run` still going then keeps it until they end; a `run -d` job does not. After `/clear` the
  agent's process lives on and the old box only idles out.

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
- `omabox ls` shows the build under the box's line with `hyprctl version`'s first line (commit,
  dirty or clean), `ls --json` has `hyprland` (null for the installed one) and `hyprland_version`
  (every box's, the installed one's too), `omabox windows` starts with it, and `<box dir>/box.log`
  has that line as the box's Hyprland said it. A save records it, and `up --from` notes a different one. Check which binary runs:
  `omabox run -- sh -c 'readlink /proc/$(pgrep -x Hyprland)/exe'`.
- What a box proves: compositor logic runs for real on a virtual output with virtual input devices
  (layouts, focus, input routing, the Lua config, IPC, protocols). What it never runs: the DRM/KMS
  backend (modesetting, real monitors, HDR/VRR, multi-GPU), libinput with real devices, and the
  session and suspend paths, or a real password at the lock screen (the lock itself runs: "Testing
  the lock screen"). Those stay the real desktop's (or a VM's with passthrough): say so.
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
- Testing a shell plugin's accessibility tree: `omabox tree --layer NAMESPACE` (Shots, above), on
  the installed Quickshell from 0.3.2 or on a build.

## The Omarchy shell's IPC (direct routes)

`omabox run -- omarchy-shell TARGET METHOD [ARGS]` talks to the box's shell (never the user's):

- `shell summon ID`, `shell toggle ID`, `shell hide ID`: open or close a plugin's panel, third-party
  plugins included (`shell summon chaves.omabox` opens that bar widget's panel).
- `shell call ID METHOD ARG`: a function of a loaded panel/overlay/menu plugin (`unknown` when there
  is none).
- `shell listPlugins` (JSON: id, kinds, enabled), `shell listShellConfig`, `shell ping`.
- Bar widgets: `shell moveBarWidget ID PLACEMENT`, `shell putBarWidget ID PLACEMENT` (enable and place,
  unless it is on the bar already), `shell setBarWidget ID KEY VALUE_JSON SELECTOR`, where PLACEMENT is
  `'{"section":"left","after":"omarchy.clock"}'` (`before`, `index`) and SELECTOR `'{}'` or
  `'{"section":"right","index":2}'`. Plugins: `shell enablePlugin ID PLACEMENT`, `shell
  setPluginEnabled ID true|false`, `shell rescanPlugins`. Also `shell reloadConfig`, `shell
  debugBarGeometry`, `shell togglePanelAt SECTION INDEX`.
- A plugin's own `IpcHandler` answers to its `target` (a panel's `ipcTarget`, e.g. `omarchy.audio
  toggle`: open, close, show, hide, toggle). An unknown target or method prints `Target not found.` / `Function not found.`.

## Testing a shell plugin

The checks a plugin's own checklist asks for "on a live desktop", in a box. `omabox up --plugin
PATH` (add `--stock-bar` for Omarchy's default bar, `--net isolated` when it starts or probes local
servers; do not mount plugins you were not asked to test), then `omabox restart-shell` after each edit (the plugin is live in the box, read-only). The box's
`shell.json` has only built-in widgets plus the plugins you mount, each enabled where its manifest
says, in a copy of the user's bar layout (Omarchy's workspace numbers in place of a plugin's left out);
`--stock-bar` uses Omarchy's default bar (workspaces, clock, the stock right side), to see a plugin as
most people will. Then:

- **Loaded?** `up` and `restart-shell` print a warning for a plugin the shell did not load (its
  validator's message, or the shell's QML error); `omabox ls --json` has its `plugin_status`. A panel,
  menu or overlay loads its QML only when summoned: its errors show then, in `omabox log shell`. A
  widget placed in another mounted plugin's layout is `hosted` (`plugin_status`'s `host` names that
  plugin), with no warning: check it with that plugin's IPC. A service with a widget, in its own
  `plugins[]` entry, is `loaded` even with its widget off the bar. `plugin X is not in the bar's
  layout: if Y places it itself, ignore this`: a widget in no layout of the box's shell.json while
  another mounted plugin runs (`off the bar`); if Y puts it in its own lanes through its config or
  IPC, that is fine. `warning: plugin X disabled: not in the bar's layout`: no mounted plugin could
  place it; the line gives the `putBarWidget` command that puts it there.
- **Its settings** are the desk's (`~/.config/omarchy/extensions/NAME.json`, copied at `up`, which
  says so): an on/off switch off on the desk is off in the box. `up --no-seed extensions` starts
  without them; `--seed` a fixture for a known state (Mounts, HOME and the session above).
- **Linked or mounted**: a plugin inside the repo `up` runs from (or a same-path `--ro-bind`, or an
  `--overlay`) is a link in the box HOME to its own path, as a plugin linked from its repo is on a
  desk, so a helper that finds the rest of its repo through `readlink -f "$0"` works; any other PATH
  is mounted there.
  `plugin_status`'s `via` says which (`linked`, `mounted`).
- **Into a box that is up**: `omabox plugin add PATH|ID [-b NAME]` puts one in as `--plugin` would
  have (linked or mounted, read-only), has the shell rescan until it lists it, enables it and says
  its state; `omabox plugin remove ID` takes one out (an `up --plugin` one too). Not a link made by
  hand in the box HOME: it is seen only for a folder the box has at its own path, and `omarchy plugin
  enable` right after `rescanPlugins` says "is not known" until the rescan is done. Not for a box a
  jail started: `up --plugin` there.
- **Edits**: `omabox restart-shell` (linked or mounted, the plugin is live). Nothing reloads by itself in a box (neither
  Quickshell's watcher nor Omarchy's plugin registry's), so a shot never catches a half-reloaded
  plugin: `restart-shell` after edits, then `wait still`, before a shot. A `keepLoaded` plugin or a service needs a restart on any
  desktop: Omarchy keeps the loaded instance across its hot reload.
- **Open and close**: `omabox run -- omarchy-shell shell summon ID ['{"payload":1}']` (a bar widget
  takes no payload), `shell hide ID`, `shell toggle ID`; the user's ways to close it are keys and
  clicks: `keys --wait Escape`, a click outside, a click on the widget again. `omabox wait layer NS
  --gone`, then which window has focus: `omabox lua 'hl.get_active_window()'`.
- **Settings and placement**, through the box's shell (the IPC list above): `omarchy-shell shell
  setBarWidget ID KEY VALUE_JSON '{}'` (kept in the widget's entry in the bar layout),
  `moveBarWidget ID '{"section":"left","after":"omarchy.clock"}'`, `setPluginEnabled ID false`. Or
  edit `.config/omarchy/shell.json` under `omabox path --home`: the box's shell applies it at once.
- **Against the user's real desk** (a bar plugin hosting other widgets, plugins reading their own
  files beside `shell.json`, a bug that shows only with everything loaded): `omabox up --desk-shell
  [--plugin PATH]` mounts every plugin in `~/.config/omarchy/plugins` (read-only; the `--plugin` of
  the same id wins: the working copy), puts in the desk's `shell.json` unfiltered (only idle held
  off) and copies the other files there the plugins read. Secrets never go in (`api-keys.env`, any
  name or content that is a key, token, secret or password), nor hooks; `up` prints each file left
  out and why, `ls --json` has `desk_shell`. A plugin the desk has off stays off, unwarned. Only when
  asked to test against the desk: otherwise mount just the plugin under test.
- **A config the plugin reads once at start**: `omabox up --plugin PATH --seed ./fixtures/config.json:.config/myplugin/config.json`
  copies it into the box HOME before the shell starts (no second shell start, no wrong state in between).
- **A vertical or bottom bar**: `"bar": {"position": "left"}` (`right`, `bottom`) in that file.
- **Data states** (missing tool, signed out, empty, malformed output, slow, failing): a stub CLI
  in `.local/bin` under `omabox path --home` (ahead of yours on the box's PATH, the bar's included; not for
  `omarchy-*` commands, whose bin comes first for the bar), offline with
  `--net isolated`.
- **Theme**: `omabox run -- omarchy-theme-set NAME` (names: `omabox run -- omarchy-theme-list`),
  `omabox wait still`, `omabox shot`.
- **Under a replacement bar**: Omarchy gives a widget hosted by a third-party bar no service; check it
  under the built-in bar too.
- **Processes it starts** (servers, helpers): look before `omabox down`, which kills everything in
  the box and so hides a leak. After stopping them, after `restart-shell` and after
  `omabox run -- omarchy plugin disable ID`: `omabox run -- ps -eo pid,pgid,stat,args --forest`.
  A server it starts is up when `omabox wait until -- curl -fsS http://127.0.0.1:PORT/` exits 0.
- **The install path** (how users get it): from the checkout (mounted: the repo you ran `up` from,
  or `--ro-bind DIR`), `omabox run -- omarchy plugin add /abs/checkout --yes --enable`. It clones the
  committed HEAD (not your uncommitted edits) into the box HOME, validates it and places the widget
  in the box's bar. Not in a box that mounts it with `--plugin`: there its id is taken. Then
  `omabox run -- omarchy plugin remove ID --yes`, and nothing of it should be left in the box's
  `shell.json` or plugins dir.
- **Not in a box**: real monitors (virtual ones, several and at any scale, are: `--monitor`), a
  real password at the lock screen (the lock itself runs: "Testing the lock screen"), real devices.

## Testing an app as a desktop app

- **From the launcher** (desktop entry, icon, app id): put `myapp.desktop` in
  `.local/share/applications/` under `omabox path --home`, its icon in
  `.local/share/icons/hicolor/SIZE/apps/`, and, when `Exec` names a command, a wrapper of
  that name in `.local/bin` there that runs your build. The launcher sees new entries without a
  restart. `omabox keys --wait super+alt+space`, `omabox keys --wait -t myapp`, a shot, `omabox keys
  Return`, `omabox wait window myapp`; `omabox windows --json` has the class to compare with the
  entry's name (`StartupWMClass`), and `omabox log apps` what the launcher ran. A launcher start goes
  through omabox's `uwsm-app` stand-in: a plain process, without the user's uwsm environment.
- **The monitor dropping off and back** (wake from standby, a KVM, a dock): `omabox output drop --for
  300ms --cycles 20`. Per-screen windows (a bar, a panel, an app's) are torn down and rebuilt; the
  cycles stop at the first shell crash, naming its report. `output drop` and `output back` by hand
  to look at the box in between (`omabox log shell`: "There are no outputs"). With `--monitor`s,
  `output drop` takes the main screen (the others stay on) and `output drop HEADLESS-3` (on NVIDIA
  `WAYLAND-2`) that monitor (`--for`/`--cycles` too); each comes back under its name, mode, place and
  scale, and `output back` alone brings back every one dropped.
- **Theme switch while it is open**: `omabox run -- omarchy-theme-set NAME`, `omabox wait still`,
  `omabox shot --window myapp`, `omabox pixel --window myapp X Y` for a colour. The colours are in the box's
  `~/.local/state/omarchy/current/theme/colors.toml`; `omarchy-theme-set` replaces that whole
  directory (`rm -rf`, then `mv`), so an app watching a file or the directory itself loses track:
  watch `current/`. A malformed theme: edit the box's copy of that file.
- **Its tray icon and menu** (a row toggled, its checked state, a submenu): `omabox tray` lists the
  box's items, `ID "TITLE" [status]`, and each one's menu in `tree`'s words (`check menu item
  "Sync" [checked]`, `menu "Stations"` with its rows indented under it), read over the box's own
  session bus; `--json` for a script. `omabox tray click ID LABEL` clicks a row as the bar's menu
  does (any case, exact over a part; several alike: exit 2, `--index N`) and prints it as the app
  then has it: `check menu item "Sync" [unchecked]`. `tray menu ID` opens the menu as a host does
  (AboutToShow; an app may fill it then), `tray activate ID` is a left click on the icon,
  `--secondary` a middle one. Not pixels, and not the bar's menu card: Omarchy's bar publishes no
  accessible items nor where an icon is (`tree --layer omarchy-bar` has none), so for the look of
  the icon or the card, a shot (`--stock-bar` if the user's bar has no tray). An item listed
  `(no menu)` shows a menu of its own: `tray menu ID` asks it (ContextMenu), then `windows`, `tree`.
  What a row opens, as it opens: `shot --burst 12 --sheet --after -- tray click ID LABEL` (or
  `record -- tray click ...`); `menu` and `activate` are actions too, a bare `tray` is not.
- **Fresh profile, first run**: every new box. A state you reach once and test from often: `omabox
  save NAME`, then `up --from NAME`.
- **Focus loss**: open another window (`omabox run -d -- foot`) or the Omarchy menu (`keys
  super+space`) while the app holds a drag (`pointer -- down`, ..., `up`), then come back.
- **A transition or an animation** (a panel sliding in, a hover fading): `omabox shot -g "X,Y WxH"
  --burst 12 --sheet --diff --after -- keys super+space`: the first frame, then the keys, the rest
  100 ms apart (`--every`), each frame's time and what
  changed from the one before (`changed X,Y WxH` in the screen's coordinates, as `wait` says them and
  `click` takes them, whatever the crop or `--fit`; with `--window` the window's own), and one contact
  sheet to read instead of twelve shots. `-o DIR` again clears the frames and sheet an earlier burst
  left there (nothing else); without `-o` each burst gets a folder of its own.
- **A demo or README capture**: shots of a box are clean (a fresh HOME, no notifications of the
  user's). Without the pointer: `shot --no-pointer` and `record --no-pointer` (it comes back after),
  or `omabox pointer hide` once for a series (kept over reloads and theme switches; it still moves
  and gives focus) and `pointer show` after. Video: `omabox record start -o demo.mp4`, act, `omabox record stop` (prints the file: an
  H.264 MP4 at a steady `--fps`, 30 by default), or `omabox record -o demo.mp4 -- keys super+space`
  for one action (and `--hold 1s` after it). It records one output at its pixels (`--monitor NAME`;
  the main screen by default): in a box at `--size 3840x2160,scale=2`, a sharp 4K video of a
  1920x1080 layout. `--monitor all`: every monitor in one video, laid out as `shot` takes the screen
  (the highest scale, gaps black), the monitors in step to a frame. Needs wf-recorder and ffmpeg (they run in the box; a stuck ffmpeg step is killed at its time limit and run once more, then `stop` fails keeping the raw files). You cannot watch it: `record
  stop --sheet` (or `record sheet demo.mp4 [--frames N | --every 2s] [--fit N]`) prints a contact
  sheet, demo.sheet.png, frames labelled with their times: Read it, fix the script, record again.
  `omabox record mark "scene 2"` just before each scene notes it in the video's own clock (ms from
  the first frame, exact to a few ms); `record hold 2s` holds the picture for the viewer, 2 s of the
  video whatever the script's own time (`record mark "scene 2" --hold 2s`: both; `record -o
  cycle.mp4 --for 5s`: 5 s of the screen, no action); `stop` writes demo.marks.tsv and demo.vtt (captions: `ffmpeg
  -i demo.mp4 -vf subtitles=demo.vtt out.mp4`; a cut: `ffmpeg -ss A -to B` with two marks' times), and
  the sheet shows each mark under the frame after it. A script's omabox calls take time of their own:
  the marks show how far the scenes drift; holds, not sleeps, make its pauses. For people watching it: `record start
  --glide 600ms` (click, pointer and drag glide there while it records), then `record stop
  --show-input --captions --cut-idle` (clicks, pointer and keys drawn in as peek shows them, --pass as
  `*`; the marks as captions; every still stretch cut to 1 s, marks moved with it). Sound (an `up --audio` box): `record start
  --audio` muxes the box's speaker into the MP4 (AAC), in step with the picture (a sound within ~15 ms
  of a change drawn at the same moment; a player's own start adds its latency: pw-play ~60 ms);
  `omabox audio 3 [--json]` listens for 3 s from now and says silence, or when and how loud (peak, RMS
  in dBFS, the spans): start it, then make the sound. Inside ai-jail `record` works the same (its
  files are written into the jail by your omabox). Frames to compare instead (an animation's steps):
  `omabox shot --burst N --sheet` (above), never a loop of `grim` in the box.
- **The package itself** (install, upgrade, removal, pacman hooks): not in a box (a read-only `/usr`,
  no pacman). A VM, or ask the user.

## Reviewing an Omarchy change

- `omabox up --omarchy ~/src/omarchy` runs that checkout in the box instead of `/usr/share/omarchy`
  (as `omarchy dev link` does on a host, without touching the user's): its Hyprland config, shell,
  `bin/` and `OMARCHY_PATH`. Never `omarchy dev link` on the host to test a change. Edits show after
  `omabox restart-shell` or `omabox hyprctl reload`.
- Two boxes side by side: the change and the release it changes. Check the change out in a worktree
  (`git fetch origin pull/123/head:pr-123`, `git worktree add ../omarchy-pr-123 pr-123`), never by
  switching the user's checkout. Then `omabox up pr --omarchy ../omarchy-pr-123` and `omabox up
  base`, and run every step in both (`-b pr`, `-b base`).
- A change you did not write runs as the user in the box: the box keeps it off the desktop and away
  from the user's HOME, but it is not a sandbox. `--net isolated` keeps it off the network too.
- Hardware-driven UI (volume, brightness, battery) has no hardware in a box: drive the shell part
  through its own command or IPC, e.g. the OSD with `omabox run -- omarchy-osd -i volume-high -p 40`,
  then `omabox shot`. When the changed script itself calls `wpctl` or `brightnessctl`, a stub of that
  name in `.local/bin` under `omabox path --home` answers it.
- Edits to the tree: `omabox restart-shell` (the shell; `keepLoaded` plugins, such as the OSD,
  notifications, the menu and the lock, keep their old code until then) or `omabox hyprctl reload`
  (the config). Omarchy 4.5's shell keeps its toasts across a restart (and in a save; `up --clean`
  starts without them).

## Testing the lock screen

The shell's lock runs in a box: the system menu's Lock (or `omabox run -- omarchy-system-lock`)
takes a real session lock (`omabox wait until -- omarchy-hyprland-session-locked`), keys go to the lock
and not the window under it, a wrong password is refused. What a box cannot do is accept a real
password: it cannot read `/etc/shadow`, so PAM fails every password ("cannot retrieve authentication
info"). Never type the user's password into a box. To test unlocking, give the lock a PAM config of
your own and a test password, through an Omarchy tree (`up --omarchy`; a box refuses mounts on its
`/etc`):

- A copy of `/usr/share/omarchy` (or the checkout under review) where
  `shell/plugins/lock/Service.qml`'s password `PamContext` gets `configDirectory: "/opt/lockpam"`
  next to its `config: "omarchy-lock-password"`.
- In a folder of yours: `omarchy-lock-password` with `auth required pam_exec.so expose_authtok quiet
  /opt/lockpam/check` and `account required pam_permit.so`; `check`, executable, compares stdin up
  to its NUL with the test password (`IFS= read -r -d "" pw; [ "$pw" = test-pw ]`).
- `omabox up --omarchy COPY --ro-bind FOLDER:/opt/lockpam`, then `omabox keys --pass VAR Return` on
  the lock (a variable holding the test password); unlocked once `omarchy-hyprland-session-locked`
  fails. `omabox log shell` shows each PAM attempt and the folder it used.

`t_lock` in the suite does exactly this. The real PAM stack (pam_unix, faillock, fingerprint), the
login screen and the disk passphrase stay a VM's to test.

## Testing a polkit agent

A box has no system bus and no logind session, so a polkit agent (the shell's `omarchy.polkit`, a
plugin that replaces it, a GTK or Qt agent) never registers in a plain box ("No session for pid N").
`omabox up --polkit` gives the box a system bus of its own with a stand-in authority on it (never
your bus or polkitd) and a session, "omabox", for its processes. The agent registers as on a desk;
`omabox polkit agents` lists it (subject, bus name, object path). Then make a request and drive the
dialog, never faking its flow:

```bash
omabox up --polkit --stock-bar          # or --plugin PATH for yours (your bar: only if its shell.json loads it)
omabox polkit ask --action org.example.restart --message "Authentication is required to restart." \
  --accept test-pw --json > /tmp/ask.json &   # waits for the request's end
omabox wait layer omarchy-polkit        # the dialog (omabox hyprctl -j layers names yours)
omabox keys -t wrong Return             # FAILURE: the agent's wrong-password state
omabox keys -t test-pw Return           # SUCCESS
wait; cat /tmp/ask.json                 # {"result":"authenticated","tries":2,"wrong":1,...}
```

The end is the agent's own, through polkit's real agent library and helper protocol, so the order of
its signals is the real one (Quickshell drops a finished request before it emits
`authenticationSucceeded`: code that waits for that signal on the flow never hears it). Results:
authenticated, not-authenticated, cancelled (its user dismissed it: Escape), timeout (no end in
`--timeout`, 60s: the authority cancels the request, as polkitd does when its caller gives up; exit
124), agent-gone, no-agent. `--delay 1s` holds each answer, to see a checking state; `--accept` is a
test password (never the user's): without it every password is wrong. A second agent for the session
is refused, as polkitd refuses it. `omabox log polkit` has each registration and request. Not
testable here: a real polkitd's decisions (rules, `auth_admin_keep`), pkexec (the stand-in decides
nothing), the real PAM stack (fingerprint), an admin dialog's list of other users.

## When a box cannot test it, in detail

A box has virtual screens only (`--monitor` for more, any size and scale; `output drop` makes one
vanish and return), no system bus (NetworkManager, bluetooth, UPower, logind), no devices (audio,
DDC/CI, backlight, real input, USB), no suspend, a lock screen with no real password, and a fresh
HOME. A change touching `hl.monitor`, HDR/VRR/`cm`, `ddcutil`, backlight, `wpctl`, `nmcli`,
`bluetoothctl`, `powerprofilesctl`, `journalctl`, `loginctl`, `/dev/…` or the user's real accounts
and data cannot be verified in a box: do not report it as tested. (`systemctl --user`: `omabox up
--systemd`.)

- **Split the work**: the box for what it can show; name the part that needs the real machine.
- **Ask once before touching the real desktop**: say exactly what will change ("your monitor at 120
  Hz for 15 s, then back"), wait for a yes (it covers this task, not the next), use `omabox host --
  CMD` under the guard, follow the project's own safety rules and put everything back.
- **A host `pgrep` sees boxes too** (a box's shell has the desktop's command line): before reading
  two `quickshell`s as two desktop shells, `omabox which PID` names a pid's box (or `not in a box`).
- The screens: any size, refresh rate, number (`--monitor`) and scale for the extra ones; no real
  modes, HDR, VRR, 10-bit, colour management or DPMS. The screen going and coming back (a monitor
  dropping off on wake, a KVM, a dock) can be tested: `omabox output drop [NAME] --for 300ms
  --cycles 20`, which stops at the first shell crash; an unplug for good: `omabox monitor remove NAME`.
  On NVIDIA an output the app creates itself (`hyprctl output create headless`) comes up 0x0: a box
  on the other GPU (`OMABOX_RENDER_NODE`, "Screen size and rendering cost").
- No system bus: no NetworkManager, bluetooth, UPower/power profiles, udisks, logind, polkit (an
  agent's dialog: `up --polkit`, "Testing a polkit agent"). No
  devices: no sound card (`up --audio` gives a box its own PipeWire, with a null sink, omabox-speaker,
  as its only output and a silent mic, omabox-mic: apps play, nobody hears, `omabox audio` and
  `record --audio` read it; real outputs, Bluetooth audio and `wpctl` on the user's devices stay
  untestable), no `/dev/i2c` (DDC/CI brightness), no backlight, no real keyboards,
  mice, touchpads, tablets, cameras, USB or printers. No systemd user manager unless `up --systemd`
  (never journald or logind), no installed `.desktop` files or URL handlers, no idle or suspend; the
  lock screen locks but takes no real password ("Testing the lock screen").
- **No PipeWire realtime in a box**: every box writes `~/.config/pipewire/client.conf.d/90-omabox.conf`
  (`module.rt = false`: a box has no RealtimeKit, and the limit of 0 its portal gave got processes
  SIGKILLed), so a client's `RLIMIT_RTTIME` stays unlimited where module-rt would set one, and a bug
  in that (a `0/0` limit) does not show. One client without the file: `omabox run -- env
  PIPEWIRE_CONFIG_DIR=/usr/share/pipewire APP` (PipeWire's own config only, no drop-ins); in a box
  module-rt then sets `0/0`, as on a desk without RealtimeKit (one with it gets 200000).
- Reporting: each plugin's `commit` in `ls --json`'s `plugin_status` ends in `+dirty` for uncommitted
  edits. Virtual monitors are not the user's: say which layout you tested.

## Showing the user

- Only when asked: `omabox peek` or `omabox up --interactive` (a box they drive; never bring its
  window forward yourself). `omabox config` is the user's: change it only when asked. `omabox clip`
  refuses agents (`host` too): never work around it; type text with `keys -t`.
- `omabox peek` opens a live, view-only window of your headless box on the user's workspace 9 (or the
  one they set with `omabox config workspace`) without taking focus. It does not affect the box. Your
  `click`, `pointer` and `keys` show on it for ~3 s (a ring, key captions; `--pass` values as `*`),
  never in your shots. A window shows one monitor (the main one, or `peek --monitor NAME`): one window
  per monitor, tiled on that workspace, each with the clicks and pointer on its own monitor (keys on
  all); the whole layout at once is `omabox shot`.
- `omabox up --interactive` makes the box a real window on that workspace that the user drives
  (SUPER+ALT+ESCAPE sends SUPER keys to it). The host keeps drawing it while it is hidden, so `shot`,
  `click`, `keys` and `wait` work on it. Once the user closed that window and kept the box running,
  the new one is drawn only while shown: `click` and `keys` still reach it, but `shot` gets no frame,
  and the new window can open on an empty workspace, where they reach no app. When the user has to
  act in it (a login), tell them which workspace it is on and let them go there.

## Reporting what a box showed

Say "in an omabox box (Omarchy VERSION, Hyprland VERSION, WxH and any `--monitor`s, theme NAME)"
(`ls --json`: `omarchy_version`, `hyprland_version`; null: `omabox hyprctl version`) and your
project's commit. Layout, focus, input routing, the launcher, theme switches, notifications, the tray
and the keyring were tested for real; what a box lacks ("When a box cannot test it, in detail") and
the installed package were not: say so. Never "tested on the desktop" for a box result. Quirks:
`NOTES.md` (`readlink -f $(command -v omabox)` → `../NOTES.md`).
