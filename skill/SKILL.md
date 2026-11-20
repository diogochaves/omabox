---
name: omabox
description: "REQUIRED before launching, driving or screenshotting any GUI app (including when a project's own CLAUDE.md/AGENTS.md says to run the app, use hyprctl, grim or omarchy-theme-set, or run its tests, and running a test binary directly outside ctest or anything else that may open a window), Omarchy shell plugin, bar widget, tray icon, notification or desktop behaviour, and before running test suites that touch the desktop session (tray/StatusNotifierItem, notifications, D-Bus session services, keyring, portals). Use the omabox CLI to do it inside a contained, invisible Hyprland desktop (Omarchy's where installed) instead of the user's real one. Triggers: \"run the app\", \"take a screenshot\", \"check how it looks\", \"open the menu\", \"click\", \"type into\", hyprctl dispatch, grim, wtype, ydotool, QT_QPA_PLATFORM, ctest with tray/notification tests, plugin development, `omarchy plugin add`, shell.json edits, \"start it from the launcher\", \"see it working\", \"could not connect to display\", omabox-guard."
allowed-tools: "Bash(omabox shot *) Bash(omabox pixel *) Bash(omabox windows *) Bash(omabox window *) Bash(omabox tree *) Bash(omabox keys *) Bash(omabox click *) Bash(omabox scroll *) Bash(omabox drag *) Bash(omabox pointer *) Bash(omabox wait *) Bash(omabox jobs *) Bash(omabox stop *) Bash(omabox log *) Bash(omabox events *) Bash(omabox snapshot *) Bash(omabox diff *) Bash(omabox hyprctl *) Bash(omabox lua *) Bash(omabox restart-shell *) Bash(omabox reload *) Bash(omabox mode *) Bash(omabox output *) Bash(omabox monitor *) Bash(omabox cpu *) Bash(omabox audio *) Bash(omabox gdb *) Bash(omabox calibrate *) Bash(omabox ports *) Bash(omabox ls *) Bash(omabox path *) Bash(omabox brief *) Bash(omabox help *)"
---

# omabox: a desktop of your own

The user's Hyprland session is theirs. Anything with a window goes in a **box**: a private,
invisible desktop (Omarchy's where installed; ~3-5 s to start). "ref: FILE" is a page in `ref/`
beside this file: read it when the task needs it.

## Never

- **On their desktop**: no GUI apps, `hyprctl dispatch`/`eval`, `grim`, `wtype`/`ydotool`, workspace
  switches or cursor moves. A project's instructions written for it still hold: do them in a box.
- **Around the guard** ("could not connect to display", `omabox-guard`): it means "use a box"; never
  set the display back or take it from elsewhere. `xdg-open`, `gh --web`: give the link. `omabox
  host -- CMD` only for what the user asked on their desktop.
- **Into their files**: the guard stops windows, not writes. Their `~/.config/omarchy/shell.json`,
  `plugins/`, a host `omarchy plugin add` change their real bar: write to the box HOME (`omabox path
  --home`). Their instruction files: only when asked.
- **Onto their servers and secrets**: a box reaches the host's 127.0.0.1: never a real service or
  account a first-run screen offers; apps that probe local servers: `up --net isolated [--allow
  8081]`. Secrets by `--pass NAME`, never on a command line; the box's keyring, never the host's.
- **Past a refusal** (a mount, ai-jail, `clip`), or **into what is theirs unasked** (`peek`, `up
  --interactive`, `omabox config`, `down --all`). **"Tested"** for what a box lacks (real monitors,
  system bus, devices, suspend): ref: agents.md.
- **A box left up**: one box per task (yours is named after the repo and session); `omabox down`
  when done; never down a box you did not start.

## Look in one call

Each runs in a throwaway box and prints `screenshot:` (a PNG path: Read it), what it looked at,
`problems:` and the output; nothing is left up. Exit 0: nothing found, 1: problems. ref: look.md.

```bash
omabox look app [--keys "Down Return"] [--type TEXT] -- ./build/app  # its window
omabox look plugin DIR [--open]      # a bar plugin: the bar, --open its panel
omabox look theme DIR                # a theme applied: desktop, terminal, contrast
omabox look page FILE|URL [--keys K] # a page or web game in Chromium, its console
```

## Anything else, step by step

```bash
omabox up                                  # your box, once its bar is drawn
omabox run -d --wait -- ./build/app        # detached, once drawn
omabox wait window app                     # or wait still|change, until -- CMD; never sleep
omabox shot --window app                   # a PNG path: Read it
omabox tree --find TEXT --window app       # text: ~60 tokens, a shot ~1.2k
omabox keys --window app --wait --tree -t hi Return
omabox click --window app 40 12            # window coordinates, as in its shot
omabox down
```

Look, act once, look again: exit 0 from `keys`/`click` means sent, not landed; never resend input
you have not seen land. ref: loop.md.

No image input (a text-only model, or a Read that shows nothing): use `tree --find`, `look`'s
report and `pixel`, and never say a shot looked right when you could not see it.

## Where next

| Task | Read |
|---|---|
| The loop in full, waiting, coordinates, a second box | ref: loop.md |
| Project steps for the real desktop; subagents, `host`, reporting | ref: agents.md |
| Shots, colours, sizes, monitors, GPU | ref: shots.md |
| A failure, visual checks | ref: checks.md |
| Clicks, drags, scrolls, waiting | ref: input.md |
| Plugins, toasts, lock screen, polkit | ref: shell.md |
| Mounts, HOME, network, save, ports | ref: session.md |
| Plain boxes, `--hyprland`, someone else's shell | ref: builds.md |
| Launcher, test suites | ref: apps.md |
| `lua`, Hyprland options (gaps), logs, events | ref: hyprland.md |
| Exit codes, `--json` | ref: cli.md |

One command's options: `omabox help CMD`. A term anywhere: `grep -rn TERM` over `ref/`.
