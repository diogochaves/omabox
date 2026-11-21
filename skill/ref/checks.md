# omabox: visual checks and symptoms

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for shots compared across runs (`omabox check`), or when something fails or looks wrong.

Sections:

- Visual checks
- Symptoms

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
in a fresh `up --clean` box (an `up --stock` box, Omarchy's bar on tokyo-night and none of your look, with a `#ff00ff` wallpaper, the bar's clock
frozen, English (en_US.UTF-8, else C.UTF-8) in UTC whatever your locale and zone, no pointer, no
network, no animations, the user `user` on host `omabox` (no name of yours in a shot); `--keep` takes
any back, `--keep locale` your locale and timezone, `--keep fonts` your fonts (none by default), `--keep identity` your names; `--theme`/`--size`/`--net connected` as `up`),
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

A check says a screen is the same or changed, never whether it is right: `--approve` keeps what
most runs showed. Give each checkpoint proofs of what it must show, both kinds, since state and
pixels each lie alone (a lock screen whose client died: `hyprctl` says locked, the screen is black):

```bash
omabox run -- omarchy-system-lock
omabox check point lock --expect "the lock screen, its password field focused" \
  --expect-cmd -- omarchy-hyprland-session-locked
omabox check point menu --window myapp --expect "the menu open, Beta selected" \
  --expect-tree 'list item "Beta" [selected]'
```

`--expect TEXT` (repeatable) is for you, not checked: said under the checkpoint's line in `check run`
(`  expect: ...`, not under `same`) and at `--approve`, on its cards in `report.png` and
`baseline.png`, and in `--json` (`expect`). Judge a change or a baseline against it. `--expect-tree
TEXT` (repeatable, needs `--window`) fails the checkpoint when `tree --find TEXT` of its window finds
nothing; `--expect-image FILE` (repeatable; a crop of a shot) when `find --image FILE` finds no match
on the screen, or in `--window`'s own pixels: the proof for what has no tree (the bar, a panel, a
terminal); `--expect-cmd -- CMD [ARG...]` (last: the rest is the command) when CMD, run in the box after
the shot, exits non-zero or outlasts `--timeout`. A failed proof is its own result, even where the
pixels are the baseline's: `proof    lock: proof failed: command omarchy-hyprland-session-locked
(exit 1)`, then `  shot: .../run1/lock.png (the same as the baseline's: the baseline shows it too)`
(exit 1, `1 failed a proof` in the last line; `--json`: result `proof`, each in `proofs` with
`held` and `why`). `check point` itself says it on stderr and exits 0, so the scenario goes on.
`--approve` keeps no run where a checkpoint's proof failed (`proof failed in run 2 of 3 ...; run 1,
where it held, kept`); failed in every run, it approves nothing (exit 1, the baseline as it was).

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
| "could not connect to display", `omabox-guard` | The agent guard: do it in a box ("The guard, in detail" in agents.md). |
| `up` refuses or dies before the box is up (a program missing, no render node, user namespaces, "too old") | `omabox doctor` (read-only; `--json`): each failed check with the line that fixes it. Installing is the user's: tell them that line; never sudo. |
| Your own shell died after `pkill -f PATTERN` | It matched its own command line: kill by PID, or `omabox run -- pkill -x NAME`. |
| After a rebuild the app still shows the old build (a single-instance app raised the old window) | Restart it with `omabox run -d --replace --wait -- CMD`, not a kill and a new `run -d`. |
| `unsatisfied: nothing changed` (124) after `--wait` | Shot; right window focused (`omabox windows`)? Do not resend. |
| `run -d` (`--wait` or not): "the command exited with status N … left nothing running" | It ended at once, or before drawing: read the log lines it printed (a file under `/tmp` or HOME the box does not have is named) and the line after them, which names what to run instead where omabox can tell (a script's interpreter, `./FILE`, a terminal the box has, the run without `-d` for one that ends); fix that, run it again. No shot. |
| `note: CMD still runs after 10 s: run waits for it to exit` | `run` without `-d` waits for its command to end; an app, until its window closes. Stop it and start it with `omabox run -d -- CMD`. |
| "warning: run -d job CMD (pid N) exited with status N after 2.1s, since the last command" | A `run -d` job ended within its first 30 s, after `run -d` had said "started" (a server dying a second in): read its log (`omabox log run`, or the path given), fix it, start it again. Said once, by whatever command came next; not for a job you killed with `omabox run -- pkill`/`kill`. |
| "its Wayland connection was closed for a protocol error: …" after a job's end | The compositor dropped the app for a request it refused; the line is from its log (libwayland's names the object and the error; GTK's and Qt's own do not). "…was lost" or "…failed": the connection broke otherwise (the box going down, the app's own fault). A bug in the app or its toolkit (or the compositor), not the box: rerun it with `--env WAYLAND_DEBUG=1` to see the request that preceded it. |
| `click --wait` 124 on a checkbox or small toggle | A change under the cursor (from ~16 px above and left of the click to ~48 px below and right) is ignored as the cursor: shot, do not click again. |
| `unsatisfied: still changing` (124) | Something never stops moving (a spinner, a glow): the line names the region; `--ignore "X,Y WxH"` it, or `-g` the part under test. |
| Context filling up with screenshots | Text checks first (`windows`, `wait`, `events`, `log --grep`); `shot --window`, `-g`, `--fit 1280`. |
| Text went to the wrong window | `keys --window SEL`, or click the field and see it focused. `keys --window` refusing with "a layer (NAME) has the keyboard": a launcher or menu is open and would take the keys: Escape it (twice if its field has text) or close it, then again. |
| A click missed a cropped or scaled shot | `click --in THAT.png X Y`. |
| Clicks land up and to the left of what you read, more so further from 0,0 | You read points in the scaled-down image you were shown: `shot --fit 1456` and `click --in` it; `omabox calibrate` measures the size for you (Shots in shots.md). |
| The window is not in the shot (covered, other workspace) | `shot --window SEL`; `click --window` raises it. |
| A menu, dropdown or tooltip in a `shot --window` is cut off at the window's edge | A window shot is the window's box only, and Hyprland says nowhere where its popups are. Take the screen around it: `shot -g "X,Y WxH"` from the window's place (the shot's stderr line: `300x252 at 831,414`) grown toward the menu, or a whole shot; then `click --in` that shot (or `click X Y` on a whole one). |
| `click --window` says "a layer surface "NS" … is over X,Y" (or `--no-raise` refuses it); a click at the screen's X Y, "… is over the screen's X,Y, on window …" | A panel or menu is over that point and takes the click where it takes input (only its rectangle is known). `omabox wait layer NS --gone`, then click again. |
| `click --window` (or a click at the screen's X Y) says "a layer spanning the screen, "NS" … draws over …", or "a layer full height at the right edge, …" for a strip (`--no-raise` refuses it) | A layer covering the screen, or its whole height or width (Omarchy's notifications and, on 4.5, its dock), takes input only on its items, and one of them (a toast, a dock icon) is drawn at that point: the click goes to it. `omabox dnd --clear` clears the toasts (Omarchy 4.5 keeps the layer up with no toast, so `wait layer NS --gone` would not return). Far from its items nothing is said. The dock is said only where it is shown (a floating workspace): ref: Toasts and dock in shell.md. |
| `click --window` says "a layer spanning the screen (or full height at the right edge), "NS" … is up … may go to it" | As above, but the pixels could not tell where its items are (a scaled output): a click on a toast goes to the toast, elsewhere to the window. Not refused under `--no-raise`. |
| `unknown: … not rendered` (exit 1) | An interactive box started by an older omabox, or whose window confirm-close replaced, is not drawn while hidden: ask the user; never show its window yourself. |
| `box 'x' is already up, without what you asked for: …` (exit 1) | It lacks those options. Yours: `omabox down` it, then `up` again. Not yours: `omabox up --new` with those options (it starts the box), then `-b box-N` as it printed. |
| `setsid: failed to execute APP` | The box has the host's programs only (`foot`, not `alacritty`). |
| Tray items that stay after their process exits; no tray at all | Quickshell bug: tray tests in a throwaway box (`omabox run`, no box up); `--stock-bar` if the user's bar has no tray. |
| "went down while this command ran", or "box is dead" after `omabox run -- pkill -x Hyprland` (or quickshell, omabox-labwc) | Those are the box itself (`pkill -x labwc` no longer matches its own): kill your own process by PID; `omabox down` then `up` to recover. |
| "box … has no shell", or `ls` says `gone` under SHELL (bar gone mid-test) | It crashed past what Omarchy's launcher restarts: `omabox log shell`, then `restart-shell`. "the shell crashed since the last command" (or "was killed by SIGKILL"): it came back, but what you see changed (a shot may show it starting). A Quickshell of your own (an app built on it) crashing is not the shell: the next command says "a Quickshell in the box crashed … (not its shell: CONFIG; report: PATH)", and omabox leaves its crash dialog (class: its app id, title `quickshell`) to it; a click it covers names it. |
| "the shell of box … does not answer (hung …)", or `ls` says `hung` under SHELL | Its pid lives but it answers nothing over its IPC: a frozen bar (a crash whose handler hung, or a stopped shell). `omabox restart-shell` kills it and starts a new one; `omabox gdb --shell` first for its stack. |
| "the shell crashed … (report: PATH)" from `up` or `restart-shell` (exit 1) | Read PATH and `omabox log shell`. From `up` before the bar came, the box is gone: `up` again once fixed; after it (the box up), `restart-shell` once fixed. For its stack: `omabox gdb --shell --watch`, the crash again, `omabox log gdb --crash`. As on a desktop, Omarchy's launcher starts it again (up to 5 times a minute), so the bar may be back: the crash still happened. |
| `hyprctl dispatch …` printed "Note: dispatch in lua is a shorthand for hl.dispatch(...)" | The old dispatcher syntax, which the box's Lua config ran as Lua: nothing dispatched. Run the `omabox lua …` line it printed after it ("Lua dispatchers" under Hyprland's Lua, logs and events in hyprland.md). |
| `omabox lua 'hl.dsp…'` printed `HL.Dispatcher` or `function: 0x…` | A dispatcher, returned and not run: `omabox lua 'hl.dispatch(EXPR)'` or `omabox hyprctl dispatch 'EXPR'`. |
| "its Hyprland did not answer (hung? …)", `ls` says `hung` | The box's Hyprland is stuck (a plugin under test?): `omabox gdb` for where, `omabox log`, then `omabox down`. |
| "went down: its Hyprland crashed (SIGSEGV)", `ls` says `crashed` or `going` (it goes `dead` in a moment) | What you sent crashed it (a Hyprland bug, or a plugin's): `omabox down`, `up`, `omabox gdb --watch`, the same step again, `omabox log gdb --crash` for the backtrace. |
| "no box 'default-…' is up" | Run from outside any repo, where your session's box is used only when it has exactly one up: run it from the repo, or pass `-b NAME` (`omabox ls`). |
| A box went down by itself ("idle") | 2h with no omabox command against it (`up --idle 0` keeps one, `--idle 30m`); your session's box still goes when your agent exits; a `run -d` job is not use: a server you only poll over HTTP needs `--idle 0`, and such a box outlives you unless it is your session's: `omabox down` it when done (`ls` says `orphan` for one whose starter is gone). `down --all` takes other agents' and the user's boxes too. |
