# omabox: the Omarchy shell

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for shell plugins, toasts and the dock, the shell's IPC, an Omarchy change, the lock screen or a polkit agent.

Sections:

- Toasts and dock
- The Omarchy shell's IPC (direct routes)
- Testing a shell plugin
- Reviewing an Omarchy change
- Testing the lock screen
- Testing a polkit agent

## Toasts and dock

Both are the box's own shell's (never your desktop's), on Omarchy 4.0 and 4.5 alike for Do Not
Disturb; the dock is 4.5's.

- **Toasts in the way** (a notification over a window's corner takes the clicks meant for the app):
  `omabox up --dnd` (or `run --dnd`) starts the box with Do Not Disturb on and no toast up; `omabox dnd
  on|off` sets it later, bare `omabox dnd` prints on or off. Under it Omarchy still shows a critical
  `notify-send`, a verification code and its own action toasts (a layout change's), so between steps
  `omabox dnd --clear` dismisses whatever is up (as a click on each would). A notification you are
  testing: leave it off, or `dnd off` for that step.
- **The notifications' layer** (4.5): a column the screen's height at the right, mapped while idle
  with no input at all; `windows` and `click --window` leave it out while it shows no toast, and with
  one up `windows` says it "is full height at the right edge, possibly over" the windows under it.
- **The dock** (4.5): a strip along the bottom, shown only on a floating workspace with no fullscreen
  window; elsewhere its layer stays mapped with no input at all, and `windows` and `click --window`
  leave it out. `omabox dock` says per monitor whether it is shown (input on its icons), tucked away
  (auto-hide on, or a maximised window: input only on the strip along its edge until the pointer
  reaches it) or hidden; `--json` the same as an object. `omabox up --no-dock` (or `dock off`) disables
  its service in the box's shell.json, its layer gone; `dock on` brings it back. An app's bottom bar
  under a shown dock: `dock off`, or put the workspace back to tiling.
- A box with no shell (`--no-shell`) refuses both; `dock` and `--no-dock` refuse an Omarchy before 4.5.
- **What an app notified** (a "download finished" notice, an error, its actions): every box records
  what reaches its notification server from `up` on, whatever server that is (the shell's, a plain
  box's `--shell`, a test's own, none yet). `omabox log notifications` prints a line each: `notify
  #ID APP: "SUMMARY" "BODY"` with its urgency, category, desktop-entry and other hints, its actions
  and timeout, and the id the server answered with (`(no id) ... -- not shown: WHY` when none did: no
  server, no reply); `close #ID` (the app closed it), `closed #ID expired|dismissed|closed by
  CloseNotification` and `action #ID KEY` (the server's: a click on a toast's button). `--json`: a
  JSON object a record; `--grep RE`, `-n N` and `-f` as for the other logs. `omabox wait
  notification 'RE'` waits for one whose summary or body matches (ref: "Waiting, in detail" in input.md). Never run
  your own `dbus-monitor` in the box and parse it: it misses what came before it started and spreads
  each call over many lines.

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
  IPC, that is fine. Several such are one line (`7 plugins (X, ...) are not in the bar's layout`),
  and a later `restart-shell` says it in a few words (`still not in the bar's layout`): never filter
  omabox's notes out with grep or tail, the ones that matter go with them. `warning: plugin X disabled: not in the bar's layout`: no mounted plugin could
  place it; the line gives the `putBarWidget` command that puts it there.
- **What it shows, as text** (no shot, no OCR): `omabox tree --layer omarchy-bar --find TEXT` finds a
  widget's text on the bar, `text "cups 391" @1845,5 58x16` (the screen's coordinates: `click X Y` at
  its middle, or `click --layer omarchy-bar --find TEXT`); a panel's is its own layer's (`omabox
  hyprctl -j layers` names it, e.g. `omarchy-keyboard-panel`). The box's shell is started with
  omabox's text reader, which gives every Text, TextInput and TextEdit it draws, whatever the QML
  sets (`[cut]`: only part of it shows); icon-font glyphs are left out. `look plugin` prints the
  widget's (`text:`) and with `--open` its panel's. A Quickshell config you run yourself (`omabox run
  -- qs -p`) gives only the items that set an Accessible role.
- **Its settings** are the desk's (`~/.config/omarchy/extensions/NAME.json`, copied at `up`, which
  says so): an on/off switch off on the desk is off in the box. `up --no-seed extensions` starts
  without them; `--seed` a fixture for a known state (Mounts, HOME and the session in session.md).
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
