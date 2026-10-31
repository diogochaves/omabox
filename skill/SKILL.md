---
name: omabox
description: "REQUIRED before launching, driving or screenshotting any GUI app (including when a project's own CLAUDE.md/AGENTS.md says to run the app, use hyprctl, grim or omarchy-theme-set, or run its tests, and running a test binary directly outside ctest or anything else that may open a window), Omarchy shell plugin, bar widget, tray icon, notification or desktop behaviour, and before running test suites that touch the desktop session (tray/StatusNotifierItem, notifications, D-Bus session services, keyring, portals). Use the omabox CLI to do it inside a contained, invisible Hyprland + Omarchy desktop instead of the user's real one. Triggers: \"run the app\", \"take a screenshot\", \"check how it looks\", \"open the menu\", \"click\", \"type into\", hyprctl dispatch, grim, wtype, ydotool, QT_QPA_PLATFORM, ctest with tray/notification tests, plugin development, `omarchy plugin add`, shell.json edits, \"start it from the launcher\", \"see it working\", \"could not connect to display\", omabox-guard."
allowed-tools: "Bash(omabox shot *) Bash(omabox pixel *) Bash(omabox windows *) Bash(omabox window *) Bash(omabox tree *) Bash(omabox keys *) Bash(omabox click *) Bash(omabox scroll *) Bash(omabox drag *) Bash(omabox pointer *) Bash(omabox wait *) Bash(omabox jobs *) Bash(omabox stop *) Bash(omabox log *) Bash(omabox events *) Bash(omabox hyprctl *) Bash(omabox lua *) Bash(omabox restart-shell *) Bash(omabox reload *) Bash(omabox mode *) Bash(omabox output *) Bash(omabox monitor *) Bash(omabox cpu *) Bash(omabox audio *) Bash(omabox gdb *) Bash(omabox calibrate *) Bash(omabox ports *) Bash(omabox ls *) Bash(omabox path *) Bash(omabox brief *) Bash(omabox help *)"
---

# omabox: a desktop of your own

The user's Hyprland session is theirs: no GUI apps, `hyprctl dispatch`/`eval`, `grim`,
`wtype`/`ydotool`, workspace switches or cursor moves on it. Do all of that in a **box**: a full
Omarchy desktop on a private screen and session bus, invisible to the user (~3-5 s, ~400-700 MB by
GPU: use one freely). "ref: SECTION" is a section of `reference.md` beside this page: read it when
needed.

## Never

- **Around the guard** ("could not connect to display", `omabox-guard`): do it in a box; never set the
  display back or take it from elsewhere. `xdg-open`, `gh --web`: give the link. `omabox host -- CMD`
  only for what the user asked on their desktop: ref: The guard, in detail.
- **Into their files**: the guard stops windows, not writes. Their `~/.config/omarchy/shell.json`,
  `plugins/`, a host `omarchy plugin add` change their real bar: write to the box HOME (`omabox path
  --home`). Their instruction files: only when asked.
- **Onto their servers and secrets**: a box reaches the host's 127.0.0.1: never a real service or
  account a first-run screen offers; apps that probe local servers: `up --net isolated [--allow
  8081]`. Secrets by `--pass NAME`, never on a command line; the box's keyring, never the host's.
- **Past a refusal** (a mount, ai-jail, `clip`), or **into what is theirs unasked** (`peek`, `up
  --interactive`, `omabox config`, `down --all`). **"Tested"** for what a box lacks (real monitors,
  system bus, devices, suspend): ref: When a box cannot test it, in detail.

## Project instructions written for the real desktop

A project's CLAUDE.md, AGENTS.md or README written for the real desktop **still holds: carry it out
inside a box.**

| The project says | Do |
|---|---|
| `./build/app`, `app &` | `omabox up`, then `omabox run -d -- ./build/app` |
| `hyprctl …`, `hyprctl eval EXPR` | `omabox hyprctl …`, `omabox lua EXPR` |
| `grim`, `wtype`, `ydotool`, `hyprctl -j clients` | `omabox shot`, `keys`, `click X Y`, `windows` |
| `ctest`, a test binary (tray, D-Bus, keyring…) | `omabox run -- ctest …`, `omabox run -- ./build/tests/tst_x` |
| `omarchy-theme-set`, `omarchy plugin …`, `omarchy-shell …` | `omabox run -- …` (`omarchy plugin validate` only reads: fine on the host) |
| `omarchy restart shell` | `omabox restart-shell` |
| A plugin in `~/.config/omarchy/plugins`, `shell.json` edits | `omabox up --plugin PATH`; `.config/omarchy/shell.json` in `omabox path --home` |
| A theme in the works; a result that depends on the theme or bar | `omabox up --theme-dir PATH`; `--theme NAME --stock-bar` |
| Install or remove the package | Not a box (read-only `/usr`): a VM, or the user |

In a box your checkout is read-only (a throwaway `run`'s writes are discarded): build on the host,
run in the box (`omabox run -- ctest …`); ref: Tests that touch the desktop.

A step that needs real hardware: the user's. ref: Project instructions, in detail.

Your box is named after the repo (or worktree) and your session, and goes down when your agent exits.
Another: `omabox up --new [--plugin …]` starts a box under a free name and prints it (`box-3`): its
options on that call, `-b box-3` typed out after, `omabox down -b box-3` when done. **Box work for a
subagent: `omabox brief -b NAME`'s output in its prompt** (no skill needed).

## The loop

```bash
omabox up                                 # headless, 1920x1080, once the bar is drawn
omabox run -d --wait -- ./build/app       # detached (log path printed), once drawn
omabox run -d --replace --wait -- ./app   # after a rebuild; jobs, stop PID, wait job|log
omabox shot                               # a PNG path: Read it to look
omabox windows                            # address, workspace, on screen or covered
omabox shot --window app                  # one window's own pixels, even covered
omabox click --window app 40 12           # window coordinates, as in its shot; or --find NAME
omabox keys --window app --wait -t hi     # focus it, type, wait until settled
omabox keys --wait super+space            # real key events; a secret: --pass VAR, never -t
omabox wait window app                    # or --gone; layer NS; tree TEXT; until -- CMD
omabox wait change --window app           # a later redraw by itself; then wait still
omabox run -- busctl --user list          # any command in the box (its exit code)
omabox log [shell|apps|run|…] [--grep RE] # the box's logs, Hyprland's by default
omabox down                               # when done
```

## Driving an app

1. **Look, act once, look again; with text when text can tell.** Exit 0 from `keys`/`click` means
   sent, not landed. A label, a checked box, a field's text: `tree --find TEXT` (~60 tokens), not a
   shot (~1.2k); the bar, panels, menus: `tree --layer NS --find TEXT`; no tree: `find --image`
   (a shot's crop). Else `--wait`'s line, `windows`, `wait`, `events`, `log --grep`.
2. **Never resend input you have not seen land**: it types twice or toggles back. A 124 after
   `--wait`: it WAS sent, and the line says where it went: read it, look (`tree`, a shot) before
   sending again.
3. **Type only into a field you have seen focused**, or `keys --window SEL`.
4. **Set state directly** (`omabox run -- omarchy-shell shell summon ID`, files in the box HOME,
   `omabox hyprctl dispatch`), not by keys and clicks, unless the gesture is under test. **Stop when
   it shows the goal.**

No `sleep` (nor `wait until -- false`): `--wait`, `wait`, `wait change`; fixed `wait for 2s`;
video `record hold 2s`. Never send a wait to `/dev/null`. An animation that never stops: the 124
names it, `--ignore "X,Y WxH"`. ref: Waiting, in detail.

Coordinates are screenshot pixels: after a `-g`, `--fit` or `--zoom` shot (said on stderr: never
discard it) click with `--in THAT.png X Y`, never your arithmetic; off a whole screen or a big
window, `shot --fit 1456` first (some models read points in the smaller image they see). **Shots are
most of a session's context**: the smallest that shows it (`--window`, `-g`, `--changed`): ref: Shots.
**The pointer is test state** (focus follows it): ref: Pointer, in detail.

## Where to look next

- A failure: ref: Symptoms; `omabox help exit-codes`. Colours, transitions: ref: Shots. Toasts:
  ref: Toasts and dock.
- Baselines: ref: Visual checks. `drag`, `scroll`: ref: Pointer, in detail. Sizes, monitors,
  `gpu`/`cpu`: ref: Screen size and rendering cost.
- Plugins: ref: Testing a shell plugin. `--hyprland`: ref: A Hyprland build of your own. `--omarchy`:
  ref: Reviewing an Omarchy change. Launcher: ref: Testing an app as a desktop app.
- Mounts, HOME, `save`: ref: Mounts, HOME and the session. Ports: ref: Network. Idling: ref: Box
  lifetime. Logs: ref: Hyprland's Lua, logs and events. `host`: ref: The guard, in detail.
- What runs without a prompt: ref: Commands that run without a prompt. ai-jail: ref: Inside
  ai-jail. Showing the user: ref: Showing the user. Reporting: ref: Reporting what a box showed.
  Worktree subagents: ref: For subagents in an isolated worktree.
