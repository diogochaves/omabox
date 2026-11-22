# The agent guard

What the guard does, what it sets for each agent, and what it does not stop. Back to the
[README](../README.md).

The skill is advice: an agent that does not think of `./build/tests/tst_x` as GUI work still opens
its windows on your desktop, because its shell holds your real display. The guard (opt-in; Claude
Code and Codex, or any agent through `guard exec`) makes that fail instead: agents' shells get a
display that does not exist, so a stray window, `hyprctl` or `grim` fails with an error the skill
explains, Omarchy's theme, wallpaper and font commands are refused by name, and a link an agent
opens is refused instead of landing in your browser. `install.sh` asks (yes by default); it never
turns the guard on by itself.

```bash
omabox guard on               # every installed agent (or: on claude | on codex); a backup next to each file
omabox guard off              # take it out again (each file as it was)
omabox guard                  # is it on, per agent; what it does
omabox host -- CMD            # from a guarded shell: one command on your real desktop, when you asked for it
omabox guard exec -- AGENT    # any other agent: start it under the guard
```

## What the guard sets, per agent, and what it does not stop

Agents' shell commands get `WAYLAND_DISPLAY=omabox-guard`, `HYPRLAND_INSTANCE_SIGNATURE=omabox-guard`,
an empty `DISPLAY`, an empty `QT_QPA_PLATFORMTHEME` (Omarchy's `gtk3` would make even an offscreen Qt
test start GTK, which needs a display, so `ctest` with `QT_QPA_PLATFORM=offscreen` keeps working) and
`QT_FORCE_STDERR_LOGGING=1` (a Qt program with no display aborts; this way the agent reads why).
A browser already running on your desktop takes a URL over its own socket, not the display, and with
`misc:focus_on_activate` takes focus too. So `BROWSER` and `GH_BROWSER` name omabox's
`share/guard/xdg-open`, which fails with a note to give you the link instead, and Claude Code and
`guard exec` also put it first on PATH as `xdg-open`. Inside a box, links open as usual. Quickshell's
own IPC needs no display either, so `quickshell kill` or `qs ipc` from a guarded shell would reach
your desktop's shell (`omarchy restart shell` stopped it, and could not start it again): the same
PATH puts `share/guard/quickshell` and `qs` first, which refuse `kill` and `ipc` and pass the rest on.
Omarchy 4.5's `omarchy-shell` goes further: given a display that does not exist, it finds your
session's and talks to the shell's own socket. The guard leaves an empty `omabox-guard` file in your
runtime directory, so the guard's display exists and `omarchy-shell` stays with it, and its own
`share/guard/omarchy-shell` refuses with a reason. `hyprctl` has its own way past the signature:
`hyprctl instances` lists your sessions and `-i`/`--instance SIG` talks to one. The same PATH puts
`share/guard/hyprctl` first, which refuses `-i`/`--instance` (not `instances`, which only reads) and names `omabox hyprctl` (and `omabox host --
hyprctl` for your own desktop, when you asked), and after a request that the guard's signature
cannot reach, says the same. Omarchy's `omarchy-theme-set`, `omarchy-theme-bg-set`,
`omarchy-theme-bg-next` and `omarchy-font-set` need no display at all: with the display gone only
their calls to `hyprctl` and `omarchy-shell` fail, and the rest still switches your real theme
(the files under `~/.local/state/omarchy/current`, the wallpaper link, your terminals, tmux, btop,
GTK, browser and keyboard colours, the theme-set hook) or rewrites your terminals' fonts and
fontconfig. The same PATH puts `share/guard` stand-ins for those four first, which refuse and name
`omabox up --theme NAME`, `omabox look theme NAME` or `omabox run -- CMD` for a box (and `omabox host
-- CMD` for your desktop, when you asked). They refuse whatever `WAYLAND_DISPLAY` names: a box's
socket there (`eval "$(omabox env)"`) changes nothing about where they write, your HOME. One
exception: run by Omarchy's own update (`omarchy update`, or `omarchy-migrate`, among their callers:
the program a caller runs, or the script its shell runs, by that name; not a command that only
mentions the name), they hand over to Omarchy's. Three of Omarchy 4.5's migrations re-stage the theme through
`omarchy-theme-set`, and `omarchy-migrate` stops at the first command that fails, so a refusal there
would leave an update with its packages upgraded and its later migrations pending. An agent that runs
`omarchy update` is changing your real system anyway; the guard does not stop that.

- **Claude Code**: one `SessionStart` hook in `~/.claude/settings.json` (merged with yours). Every
  shell command of the session gets the variables, subagents' too, plus a core limit of 1 byte, so a
  Qt program's abort never becomes a "Process crashed" notification on your desktop. It also tells
  the agent in one line what the error means and when `omabox host` is allowed. Claude Code's own
  process is not changed (clipboard paste, opening the browser). Your own `!` commands in Claude Code
  run in the session's shell, so they get the guard too: run desktop commands (`omarchy restart
  shell`, `hyprctl reload`, a theme switch) in your own terminal, or ask the agent to use `omabox
  host`. If omabox is gone (deleted without `guard off`), the hook applies nothing
  and says so in one line.
- **Codex**: `[shell_environment_policy.set]` in a marked block of `~/.codex/config.toml`, or, when
  the config has its own `set` table, the guard's keys marked inside it (`guard off` keeps yours; a
  value of your own for one of them, or an inline table, is refused, untouched), and a
  `SessionStart` hook in `~/.codex/hooks.json` that gives the agent the same note. Codex runs that
  hook only after you trust it once (`/hooks` in Codex). No core limit. The block's values are fixed,
  so they stay if omabox is gone: the hook then tells the agent how to take them out. Codex's `set`
  cannot put a folder first on PATH, so Codex gets **none of the stand-ins** above: `omarchy restart
  shell` from its shell stops your bar (the real `quickshell kill`; its relaunch through `hyprctl` then
  fails), `hyprctl -i`/`--instance` reaches your Hyprland, `omarchy-theme-set` and the font and
  wallpaper commands change your real look, and `xdg-open` opens on your desktop (only `BROWSER` and
  `GH_BROWSER` name the guard's). To give Codex the whole guard, start it with `omabox guard exec --
  codex` (Codex's own process then loses the display too). The guard's file in your runtime directory
  (which keeps Omarchy 4.5's `omarchy-shell` off your shell) is made by `guard on codex` and, after
  that, by the hook at each session start, or by any omabox command run from a guarded shell: until
  you trust the hook, a reboot leaves Codex's sessions without it until such a command runs. The
  variables were checked through `codex sandbox`, the hook only by what Codex's hooks.json takes, not in a
  logged-in session.
- **Anything else**: `omabox guard exec -- AGENT` starts the agent itself under the guard. Its own
  process loses the display too (clipboard, opening a browser to log in; `xdg-open` refuses). It also
  gets a session of its own (`OMABOX_SESSION`), so its default box is its own, as in Claude Code and
  Codex.

When an omabox update changes what `guard on` writes, the Claude Code and Codex hooks say so at
session start, in one more line, until `omabox guard on` renews them.

omabox keeps working under the guard: boxes have their own display, and `up --interactive`, `peek`
and `--size host` find your session by themselves. Work you ask for on your real desktop ("switch my
theme", `hyprctl reload` after editing your config) goes through `omabox host -- CMD`: your session's
display, `DISPLAY` and Qt theme for that one command, which it names on stderr, so it stays on the
record.

It stops accidents. It does not stop:

- an agent that sets the variables back, or runs `omabox host` unasked, on purpose (a
  `HYPRLAND_INSTANCE_SIGNATURE` set by hand, a hyprctl named by its path or Hyprland's socket used
  directly reach your session);
- processes the agent starts itself rather than through its shell (MCP servers: a headed browser
  MCP opens on your desktop);
- Omarchy's own `omarchy` command (`omarchy theme set`, `omarchy font set`, `omarchy theme bg
  next`): it runs each command by its path, past the stand-ins, so it changes your real theme, fonts
  or wallpaper; nor the commands that only write your files without needing a display or the shell
  (`omarchy-theme-install`, which clones into `~/.config/omarchy/themes` before its theme switch is
  refused; `omarchy-theme-remove`; `omarchy-theme-bg-intro`);
- under Codex (the variables only, no PATH entry; see above): the stand-ins, so `omarchy restart
  shell` stops your bar, `hyprctl -i` reaches your Hyprland, the theme, font and wallpaper commands
  change your look; `omabox guard exec -- codex` gives it all of them;
- file writes: your shell watches `~/.config/omarchy/shell.json` and `~/.config/omarchy/plugins/`,
  so an agent editing them (or running `omarchy plugin add` on the host) changes your real bar at
  once; the skill sends those writes to the box's HOME;
- anything over the session bus or the user manager (notifications, the keyring, apps started over
  D-Bus, `uwsm-app` and `systemd-run --user`, which run in your session's environment);
- links opened by other routes: a browser started directly with a URL (it hands the URL to the one
  already running), `gio open` (it starts your URL handler itself), npm's `open` (it runs its own
  copy of xdg-open), or, under Codex, a plain `xdg-open` (Codex gets `BROWSER` and `GH_BROWSER` but
  not the PATH entry). Python's `webbrowser` stops at omabox's `xdg-open` (it counts one that has
  started as a success), except under `guard exec` from an omabox at a path with a space: there it
  tries the next browser when that one fails.

For a fence, run the agent in a sandbox that blocks Unix sockets (Claude Code's `sandbox`: it also
blocks the network and every other socket, omabox's included), or see
[ai-jail](ai-jail.md).
