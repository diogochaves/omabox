# omabox: looking in one call

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it before checking a program, plugin, theme or page in one call (`omabox look`).

Sections:

- Looking in one call

## Looking in one call

`omabox look app [--keys "K..."] [--type TEXT] [--after DURATION] [--timeout 10s] [-o FILE]
[--keep-box] [UP OPTIONS] -- CMD [ARG...]` (#461) is `up`, `run -d`, waiting for its window, `keys`,
`wait still`, `shot --window` and reading its output, in a throwaway box of its own (`look-PID`),
taken down however it ends. The report:

```
screenshot: /tmp/omabox-look-123-456.png
window: tally "Tally" 960x680 (floating)
problems:
  qrc:/Main.qml:42: TypeError: Cannot read property 'name' of undefined
output:
  ...
```

- **Its window**: the program's own (by pid), else the first; none within `--timeout` (10 s) is a
  problem, and so is a program that ends before one, said with how it ended ("exited with status 3").
- **Keys**: `--keys` (combos, space separated) and `--type` go in the order given, sent once the
  window has drawn (`wait still`), with `--wait`. `--after 2s`: a fixed wait before the shot.
- **Problems**: omabox's own warnings, and each line of the program's output (stdout and stderr, its
  `run -d` log) naming an error, a crash or a QML file and line (`error`, `critical`, `fatal`,
  `traceback`, `panicked`, `exception`, `segmentation fault`, `core dumped`, `aborted`, `Foo.qml:42`).
  A warning is not one. What a terminal runs inside it goes to the terminal, not here.
- **Output**: the first 40 lines, each cut at 200 characters.
- **Exit**: 0 nothing found, 1 problems, an error of omabox's as its other commands give.
- `--keep-box`: the box stays up (its name printed) for a follow-up; take it down after.
- UP OPTIONS as a throwaway `run` takes them (`--size`, `--plugin`, `--net`...). Your repo is not
  overlaid as `run`'s is: the program is the command you give, by its path.

`omabox look plugin DIR [--open] [-o FILE] [--keep-box] [UP OPTIONS]`: `up --plugin DIR` in a
throwaway box, then

```
screenshot: /tmp/omabox-look-123-456.png
panel: /tmp/omabox-look-123-789.png
plugin: me.tally loaded (linked)
text: tally 42
panel text: Today | 42 cups | Reset
problems:
  qs:///home/sbx/.config/omarchy/plugins/me.tally/Panel.qml[15:-1]: ReferenceError: x is not defined
```

- **The shot** is the bar's strip, the edge Hyprland reserves for it (the top, or a side for a
  vertical bar), in your bar's layout as `up` copies it (`--stock-bar` for Omarchy's default one);
  the whole screen when no edge is reserved.
- **`text:`**: what the widget shows, as text: the shell's text items in its slot on the bar (`tree
  --layer omarchy-bar` reads them, ref: shell.md), joined by ` | `; icon glyphs and pictures are not
  text (`none` for a widget of icons only). `none` and a note for one hidden in its slot; `unknown`
  and a note for one with no slot (placed by another plugin); a note when part of it is cut off (a
  label wider than a vertical bar: the shot shows only its middle).
- **`--open`**: `omarchy-shell shell summon ID`, then the whole screen (`panel:`; `-o a.png` names it
  `a-panel.png`), and `panel text:`, the text of the layers the summon mapped (60 items at most). A
  widget with no panel shows nothing more (`panel text: none`).
- **Problems**: its state when it is not `loaded` or `hosted` (`plugin_status`, as `ls --json` has
  it), `up`'s warnings for it, and the shell's warnings and errors that name the plugin (its QML
  files by path). The shell's own about the box (no portal, no system bus) are left out.
- DIR is the plugin's folder, with its `manifest.json`; the plugin id is read from it.

`omabox look theme DIR|NAME [-o FILE] [--keep-box] [UP OPTIONS]`: `up --theme-dir DIR --theme NAME`
(the folder's name), or `up --theme NAME` for an installed theme, in a throwaway box, then

```
screenshot: /tmp/omabox-look-123-456.png        # the desktop on it: wallpaper, bar
terminal: /tmp/omabox-look-123-789.png          # foot printing its 16 colours, bold, dim, reverse
theme: tally
problems:
  colors.toml: no accent (Omarchy's templates use it, and no fallback resolves it)
  foreground #2a2a3a on background #1e1e2e: 1.2:1, under 4.5:1 (stock themes: 6.7 to 21)
  background 1-x.png: 640x480, small for a 1920x1080 screen (stock themes: 1536x1024 at least)
```

The checks (#462), each a line, are read as Omarchy reads the theme: its `omarchy-theme-color`
(aliases, fallbacks) and the keys its templates use, at run time. A key no fallback resolves; a
value that is no `#rrggbb` (Hyprland's `hyprland_*_border` take their own syntax); contrast under
4.5:1 for `foreground` and `bright_foreground` on `background` and selection text, under 3:1 for
`light_foreground` and `accent` (`dark_foreground` and `muted` are dim on purpose: not checked); a
background under three quarters of the screen on a side, nearly one colour, too bright for a dark
theme or too dark for a light one. No stock theme of Omarchy 4.5 trips any of them, and each line
says what stock themes score. `up --theme-dir DIR` prints the same lines as notes; they never fail
`up`. An installed theme is checked by its name too: `look theme NAME` (a word that is no folder
here) and `up --theme NAME` read it as `omarchy-theme-set` builds it, Omarchy's folder of that name
with yours of the same name copied over it.

`omabox look page FILE|URL [--keys "K..."] [--hold KEY DURATION] [--shots N] [--after 2s] [-o FILE]
[--keep-box] [UP OPTIONS]` (#463): Chromium in the box on it, an app window with its own profile
in the box HOME, its sandbox kept (a FILE's folder is bound read-only; an `http://` URL is reached
as the box's network allows: `--net isolated --allow PORT` for a local server only).

- **Real frames.** Headless Chrome under `--virtual-time-budget` runs `requestAnimationFrame` two or
  three times a run; a box's Chromium draws every frame on a real (headless) output, so a loop that
  moves something does.
- **Held keys**: `--hold Right 1500ms` is `down:Right -s 1500ms up:Right`: the page sees the key held
  over that many frames. `--keys` takes `keys`' words as they are (combos, `down:K`, `-s 1s`, `up:K`).
- **`--shots N`** (1 to 24): the keys sent while N shots of the window are taken over `--after` (2 s
  by default), then put side by side as `sheet:` (ImageMagick's montage; `-o a.png` names them
  `a-1.png`... and `a-sheet.png`). Motion shows as the object's place changing from shot to shot.
- **Its console**, from Chromium's log (`--enable-logging=stderr`): `output:` is each console line as
  `message (file:line)`, Chromium's own lines about the box (no system bus, protocol versions)
  left out. `console.log` and `console.error` come out alike, so the problems are uncaught exceptions
  (`Uncaught TypeError: ...`) and messages that name an error, a load that failed or a `net::ERR_`.
- No Chromium in the box (`chromium`, `google-chrome-stable`): said as the program ending at once.
