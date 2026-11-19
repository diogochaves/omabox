# omabox: apps and tests

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for an app started from the launcher like a desktop app, or a test suite that touches the desktop.

Sections:

- Tests that touch the desktop
- Testing an app as a desktop app

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
  file it names on stderr (`-q`: no line); its stdout is the job's pid (`--print-log`: the log's path
  instead). A plain `run` returns when
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
  H.264 MP4 at a steady `--fps`, 30 by default, at CRF 18; footage someone will zoom or encode
  again (a video editor's): `record start --crf 8`), or `omabox record -o demo.mp4 -- keys super+space`
  for one action (and `--hold 1s` after it). It records one output at its pixels (`--monitor NAME`;
  the main screen by default): in a box at `--size 3840x2160,scale=2`, a sharp 4K video of a
  1920x1080 layout. `--monitor all`: every monitor in one video, laid out as `shot` takes the screen
  (the highest scale, gaps black), the monitors in step to a frame. Needs wf-recorder and ffmpeg (they run in the box; a stuck ffmpeg step is killed at its time limit and run once more; when the encode is stuck twice, `stop` copies the raw H.264 into the MP4 without re-encoding it and says what that video lacks (no steady fps, no sound, input, captions, cut or layout composed, a larger file), and fails keeping the raw files only when that copy fails too). An ffmpeg without libx264 (Fedora's ffmpeg-free, openSUSE's) records with openh264, said at start: still an H.264 MP4, `--crf N` its quantiser (bigger files), no `--crf 0`, 4096x2304's pixels a frame at most. You cannot watch it: `record
  stop --sheet` (or `record sheet demo.mp4 [--frames N | --every 2s] [--fit N]`, any video, no box needed) prints a contact
  sheet, demo.sheet.png, frames labelled with their times: Read it, fix the script, record again.
  A short motion (a 100 ms glitch, a text decode), frame by frame: `record sheet demo.mp4 --from
  1.2s --to 1.6s --every-frame -g "X,Y WxH"` (each frame of that stretch, cropped to the region in
  the video's pixels, a small one enlarged), or `--changed -g "X,Y WxH"` for the frames where that
  region changes (the stretch's first, then each changed one; tiles say "12: 9.433s f283"). Never
  ffmpeg's `select`/`signalstats` by hand for this.
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
