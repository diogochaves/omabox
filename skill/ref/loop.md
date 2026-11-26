# omabox: driving a box step by step

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for anything `omabox look` does not cover: a box kept up, an app driven key by key and click
by click, waiting, coordinates, a second box.

Sections:

- Your box
- The loop
- Driving an app
- Waiting and coordinates
- Where to look next

## Your box

A box is a full desktop on a private screen and session bus, invisible to the user (~3-5 s,
~400-700 MB by GPU: use one freely). Omarchy's where installed; else plain Hyprland, no bar (`up`
says so), or theirs (`--desktop desk`): ref: builds.md.

In a box your checkout is read-only (a throwaway `run`'s writes are discarded): build on the host,
run in the box; ref: apps.md.

A step that needs real hardware: the user's. ref: agents.md.

Your box is named after the repo and your session, and goes down when your agent exits.
Another: `omabox up --new [--plugin …]` starts a box under a free name and prints it (`box-3`): its
options on that call, `-b box-3` typed out after, `omabox down -b box-3` when done. **Box work for a
subagent: `omabox brief -b NAME`'s output in its prompt** (no skill needed).

## The loop

```bash
omabox look app -- ./build/app            # shot, problems (ref: look.md)
omabox up                                 # headless, 1920x1080, once its bar is drawn
omabox run -d --wait -- ./build/app       # detached, its pid on stdout, once drawn
omabox run -d --replace --wait -- ./app   # after a rebuild; jobs, stop PID, wait job|log
omabox look app -- ./app                  # ...or that, its shot and its problems, in one (ref: look.md)
omabox look                               # the box now: windows, text, problems new since the last look
omabox shot                               # a PNG path: Read it to look
omabox windows                            # address, workspace, on screen or covered
omabox shot --window app                  # one window's own pixels, even covered
omabox click --window app 40 12           # window coordinates, as in its shot; or --find NAME
omabox keys --window app --wait --tree -t hi  # type, settle, what its tree changed
omabox keys --wait super+space            # real key events; a secret: --pass VAR, never -t
omabox wait window app                    # or --gone; layer NS; tree TEXT; notification RE; until -- CMD
omabox wait change --window app           # a redraw by itself; then wait still
omabox run -- busctl --user list          # any command in the box
omabox log [shell|apps|run|…] [--grep RE] # the box's logs, Hyprland's by default
omabox down                               # when done
```

## Driving an app

1. **Look, act once, look again; with text when text can tell.** `keys`/`click`'s exit 0 means
   sent, not landed: `--wait --tree` says in that call what its tree changed; `--expect tree
   TEXT`: 124 unless it shows. A label, a checked box, a field's text:
   `tree --find TEXT` (~60 tokens), not a shot (~1.2k); the bar, panels, menus: `tree --layer NS
   --find TEXT`; no tree: `find --image`. Else `windows`, `events`, `log --grep`.
2. **Never resend input you have not seen land**: it types twice or toggles back. A 124 after
   `--wait`: it WAS sent; read the line, look (`tree`, a shot) before sending again.
3. **Type only into a field you have seen focused**, or `keys --window SEL`.
4. **Set state directly** (`omabox run -- omarchy-shell shell summon ID`, files in the box HOME,
   `window --fullscreen`, a `lua` dispatch), not by keys and clicks, unless the gesture is under
   test. **Stop when it shows the goal.**

## Waiting and coordinates

No `sleep` (nor `wait until -- false`): `--wait`, `wait`, `wait change`; fixed `wait for 2s`;
video `record hold 2s`. Never send a wait to `/dev/null`. An animation that never stops: the 124
names it, `--ignore "X,Y WxH"`. ref: input.md.

Coordinates are screenshot pixels: after a `-g`, `--fit` or `--zoom` shot (said on stderr: never
discard it) click with `--in THAT.png X Y`, never your arithmetic; off a whole screen or a big
window, `shot --fit 1456` first (some models read points in the smaller image they see). **Shots are
most of a session's context**: the smallest that shows it (`--window`, `-g`, `--changed`): ref: shots.md.
**The pointer is test state** (focus follows it): ref: input.md.

## Where to look next

- A failure: ref: checks.md; `omabox help exit-codes`, ref: cli.md. Colours, transitions, sizes,
  monitors, `gpu`/`cpu`/`frames`: ref: shots.md. `drag`, `scroll`: ref: input.md.
- Plugins, toasts, `--omarchy`, the lock screen, polkit: ref: shell.md. `--hyprland`, plain boxes:
  ref: builds.md. Launcher: ref: apps.md. Logs, `lua`: ref: hyprland.md.
- Mounts, HOME, `save`, ports, idling: ref: session.md.
- `host`, grants without a prompt, ai-jail, showing the user, reporting what a box showed,
  subagents in an isolated worktree: ref: agents.md.
