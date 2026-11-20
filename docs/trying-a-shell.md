# Trying someone else's shell in a box

A Quickshell shell or a rice someone published can run in a box without being installed: its QML
runs from its own folder, on a desktop that is not yours, and you look at it with `omabox shot` or
`omabox peek`. This page is about that one use. For developing apps and plugins, see
[usage.md](usage.md).

omabox is still **not a sandbox** (the [README](../README.md) says why). What follows is what a box
happens to block for this use, and, just as plainly, what it does not.

## How

A plain box (nothing of Omarchy's), no network unless you open it, the shell started from its
folder:

```sh
omabox up --desktop plain --net isolated --shell 'qs -p /path/to/their/shell'
omabox shot
```

Bring in what it needs read-only with `--ro-bind DIR` (its builds, a helper it calls) and its fonts
or starting files with `--seed SRC:DEST` (copied into the box's fake HOME). Never run its install
script: those write to your real `~/.config` and use `sudo`.

A trial with Caelestia (caelestia-dots/shell `f80a9807`) ran this way. The examples below are from
it.

## What a box blocks, and why

| What the shell or its helpers tried | What stops it in a box |
|---|---|
| Suspend, hibernate, logout and power-off over logind; its idle chain ends in `SuspendThenHibernate` | No system bus in a box (`SECURITY.md`, S1) |
| Escape codes written to every `/dev/pts/*`; `ddcutil` on `/dev/i2c-*` (the real monitor's brightness) | A fresh `/dev`: none of your terminals or devices |
| `killall -USR2 btop htop cava` | The box's own pid namespace (S5): it sees only its own processes |
| `sudo -n` into `/etc` (browser policies) | `no_new_privs`: nothing in a box can gain privileges |
| Theming that rewrites about 15 other apps' configs (GTK, htop, fuzzel, cava...) | The fake HOME (S2): those configs are the box's, gone with it |

## What a box does not block

- **The network**, unless you pass `--net isolated`. That shell geolocates by IP over plain HTTP at
  every start.
- **The shared kernel and GPU.** A box is processes on your machine, not a VM.
- **Code you did not read.** A shell's QML can start any process. Read it first, or run the agent
  and its boxes under [ai-jail](../README.md#with-ai-jail). For code you do not trust at all, use a
  VM.
- **Its install script.** Never run it, in a box or out of one.

## Two things the trial showed

- **A box is always a first run.** Each box starts with a fresh HOME, so it meets the bugs a real
  user meets only once. Caelestia's colour scheme changes did not apply because its state folder did
  not exist when the shell started; `--seed`ing that empty folder fixed it. When something "never
  updates", look for a folder or file the shell expects at start.
- **Edge gestures.** Caelestia's sidebar opens on a drag that starts on the last pixels of the right
  edge. `drag --glide 700ms` from there sometimes missed it; `drag --steps 40 --hold 300ms` from the
  same edge opened it. A plain box does not load the shell's own Hyprland config, so its keybinds are
  not there either: find where its panels open in its code.
