# omabox: shots and screen size

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for taking and reading shots, colours, transitions, sizes, monitors and GPU cost.

Sections:

- Shots
- Screen size and rendering cost

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
- No tree (the bar, the shell's panels, a plugin, a terminal, a game): find it by its picture.
  Shoot a crop of it once, `omabox shot -g "X,Y WxH" -o icon.png` (its comment records the box's
  scale), then `omabox find --image icon.png` prints each match as `X,Y WxH SCORE` in the layout's
  coordinates (a text line, not a shot), exit 1 none with the best score seen and where.
  `click --image icon.png` clicks its middle (`--offset 40,0`: beside a label, in its field), and
  `wait image icon.png [--gone]` waits for it to be drawn or to go. Several matches are all listed in
  reading order and said; click refuses them, exit 2, numbered for `--index N`. The score
  (`--similar`, 0.95 by default; 1 is the same pixels) is correlation of the colours, lower for
  another contrast or tone, so a disabled grey copy or the off state of a switch does not pass for
  the on one; a crop of one colour is matched by difference (it matches every place of that colour),
  and transparent pixels are not compared. The pointer's place is left out (a click leaves it on the
  match). A crop shot at another scale is resized to the box's and softened, said, and needs 0.8
  unless `--similar` says: shoot crops in the box they are used in when you can. `-g` or `--window
  SEL` (its own pixels, covered or not) narrow the search; each search is one frame, 50-100 ms on a
  1080p screen. A scenario for `check run` can click by image instead of by fixed coordinates.
- The shell's bar, panels, menus and plugins are layer surfaces: `omabox tree --layer SEL` (a
  namespace from `omabox hyprctl layers`, `address:0x...` or `pid:PID`), with `--find`, `--json` and
  `--all` as for windows. Its bounds are the **screen's** coordinates: plain `click X Y` at a node's
  middle lands on it, and `click --layer SEL --find TEXT` finds and clicks it. Quickshell publishes them from 0.3.2 (older: "no accessibility tree", or
  `--quickshell` a build), and only items that set an `Accessible.role`; the box's own shell also gives
  the text it draws, `text "WORDS"` lines (#467: a bar widget's count, a panel's labels; ref:
  shell.md). A layer with neither says "publishes no accessible items", exit 1. Its `frame`s have no
  names, so the layer's is the one of its pid that is its size: two layers of one pid and size are
  told apart only when one holds the keyboard (the launcher, or an Omarchy 4.5 bar popup, over the
  wallpaper); else `tree` says so (the shell's text is read by namespace).
- `click --window SEL X Y` takes the window's own coordinates (0,0 its corner, as in its `shot
  --window`).
- `--window SEL`: `myapp` is a class, its last part (`nautilus` for `org.gnome.Nautilus`) or part of a
  title, failing those a hyphen-separated part of the class (`portal` for `xdg-desktop-portal-gtk`);
  `title:RE`, `class:RE`, `pid:N` or an address (`0x…`) narrow it; `wait window SEL` is satisfied by
  any window it matches. `shot --window SEL -g "X,Y WxH"` crops the window in its own
  coordinates; a `-g` past the window (or the screen) is cut to it, said on stderr. A menu or popup
  reaching past the window's edge is cut off in its window shot: take the screen around it (`shot
  -g` in the screen's coordinates, or a whole shot; Symptoms in checks.md).
- Coordinates are screenshot pixels. A cropped, scaled or zoomed shot says so on stderr: then `click
  --in SHOT X Y` (and `drag`, `pointer`, `scroll`, `pixel --in`), X Y read from that image: the
  point is the layout pixel under that image pixel, also in a crop at a fractional scale. Screen
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
- A small target with no tree (a bar icon, a caret), the first look: `shot --grid 8` (2-26) draws
  lettered columns and numbered rows over the shot, labels in a 28 px margin, stderr naming each
  column's and row's span; `shot --grid 8 --cell C3` is that cell cropped, enlarged and gridded
  again (stderr gives the `-g` for a closer look still): two or three small images, not a misclick
  loop of whole shots. A target a grid line cuts: both cells, `--cell C3:D3`. `click --in GRID.png
  --cell C3` clicks a cell's middle; `click --in GRID.png X Y` reads the image as drawn, margin
  included. Once found, a plain `shot -g` of it is what later runs compare with: the grid is for
  aiming, never a baseline.
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
  as a desktop app" in apps.md), one contact sheet, never a loop of shots. 100 ms apart by default (`--every`; `--every 0`:
  back to back, ~15 ms a crop, ~25 a 1920x1080 screen, over in a fraction of a second, before an
  `--after` action may have landed). The whole burst is one capture client, so Hyprland's `screencast`
  events come once and a shell or widget that works on each event is not held up at any pace (a grim
  a frame stalled its timers). `N of M frames identical to the one before` on stderr: nothing new
  drawn between them.

- A look compared with a baseline over time, no person looking (an app's or a plugin's screens after
  a change): `omabox check`, next section.

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
  a fullscreen or maximized window is refused. `wait still` before a shot, while the app redraws. Not
  `hyprctl dispatch togglefloating`: the old dispatcher syntax does nothing under the Lua config
  ("Lua dispatchers" under Hyprland's Lua, logs and events in hyprland.md).
- **The app alone, fullscreen** (a take, a shot with no bar or wallpaper): `omabox window --window SEL
  --fullscreen`; `--maximize` fills the work area and keeps the bar. Each **sets** the state, never
  toggles it: run it again and nothing changes, so it is safe when you do not know the window's
  state (the old `dispatch fullscreen` toggled). `--fullscreen off` / `--maximize off` take it out
  of that state only (a maximized window stays so after `--fullscreen off`). It prints
  `0x... CLASS "TITLE" tiled, fullscreen at 0,0 1920x1080`; `windows` shows `,fullscreen` or
  `,maximized` in STATE, `windows --json` has `fullscreenMode` (`none`, `maximized`,
  `fullscreen`). A Hyprland whose `hl.dsp.window.fullscreen` takes other arguments is exit 1,
  naming that dispatcher. `wait still` before the shot: the app redraws at the new size.
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
