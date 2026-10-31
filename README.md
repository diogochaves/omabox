<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/omabox-lockup-dark.svg">
    <img alt="omabox" src="assets/omabox-lockup-light.svg" height="72">
  </picture>
</p>

> Your agents get desktops of their own. Yours stays untouched. omabox gives every coding agent a
> whole Hyprland desktop, with the Omarchy shell, to launch, click, type and screenshot its work in:
> invisible, one per agent, gone when the agent is.

**Website: [omabox.app](https://omabox.app)**

[![Website](https://img.shields.io/badge/website-omabox.app-blue)](https://omabox.app)
[![Release](https://img.shields.io/github/v/release/diogochaves/omabox)](https://github.com/diogochaves/omabox/releases/latest)
[![Rust](https://img.shields.io/badge/rust-1.85+-blue)](Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

https://github.com/user-attachments/assets/8dca4735-140e-472a-9bb8-b05412a4649a

## What omabox is (and isn't)

A box is the real thing, not a mock: Omarchy's own Hyprland config, the Omarchy shell (bar, menu,
tray, notifications) and your theme, bar layout and terminal settings, on a private screen and a
private session bus. An agent drives it the way you drive your desktop: it starts apps, clicks,
types, takes screenshots, reads what is on screen, records video. Nothing of it reaches your
screen, your cursor, your focus or your notifications.

omabox is **not** a sandbox. It keeps an agent's windows off your desktop; it does not fence the
agent in. A box shares your kernel, your GPU and, by default, your network, and commands the agent
runs on your host are not boxed. To limit what the agent itself can read, write and reach, pair
omabox with [ai-jail](#with-ai-jail): a jailed agent then tests in boxes of its own. For code you
do not trust at all, use a VM.

## Why omabox

- **Nothing lands on your desktop.** An agent testing a desktop app without omabox tests it on
  yours: windows pop up while you type, the cursor jumps, focus and workspaces switch, password and
  keyring prompts appear, notifications and tray icons pile up. In a box all of that happens on a
  screen nobody sees.

- **Every agent gets its own.** Each agent session gets a box named after its repo (or worktree)
  and session, the way you give each agent a worktree. It is up in 3-5 s, uses 400-700 MB
  depending on the GPU, and goes down when its agent exits. Four agents test four apps at once
  while you keep working.

  <p><img src="docs/media/boxes.webp" alt="Your desktop: your notes in an editor, the agent's terminal listing four boxes with omabox ls, and the bar widget's panel listing the same four boxes with peek, screenshot and down buttons" width="800"></p>

- **Your projects don't change.** A skill your agent already loads (Claude Code, Codex, OpenCode,
  pi, Hermes) tells it when a task needs a desktop. When a project's own instructions say "run the
  app", "screenshot with grim" or "run ctest", the agent does exactly that, in a box. An opt-in
  [guard](#the-agent-guard) makes a slip fail instead of landing on your screen.

- **Agents check their own work.** An agent scripts a visual pass once; every later build runs it
  in a clean box and gets "same", or the regions that changed and the button that moved, by how
  many pixels. `omabox tree` reads an app's controls as text, about 17 tokens where a screenshot
  costs about 1.5k.

  <p><img src="docs/media/check-report.webp" alt="A check report: 1 of 1 changed, tree: moved +0,+6 button Save, the baseline beside this run with the Save button 6 pixels lower" width="480"></p>

- **Tests that touch the desktop run boxed.** `omabox run -- ctest` starts a throwaway box,
  mounts your repo as a discarded overlay, runs the tests (tray, notifications, D-Bus, keyring)
  and tears the box down.

- **A desktop to spare, for more than tests.** Agents film demos (with sound nobody hears), test
  on monitors you don't have, take 4K screenshots, check a plugin in every theme, try a Hyprland
  build or an Omarchy checkout, all without touching yours.

  <p><img src="docs/media/monitors.webp" alt="One box with three monitors: the Omarchy menu on a 1920x1080, an app on a 2560x1440 at scale 1.6 below it, and hyprctl monitors on a portrait 1080x1920 beside them" width="800"></p>

- **You can look in, or take over.** `omabox peek` opens a live, view-only window of any box; the
  agent never notices. `omabox up --interactive` gives you a box as a window you drive yourself,
  while the agent keeps working in it.

  <p><img src="docs/media/interactive.webp" alt="An interactive box as a window on your desktop, the app running in it, next to the agent's terminal rebuilding and replacing the app" width="800"></p>

- **It is a contract.** Exit codes and `--json` fields stay, or only grow, within 1.x; anything
  else changes only after a warning. See [Stability](#stability).

## Install

You need **Omarchy 4** (Hyprland 0.56+, the Lua config) and a GPU render node: AMD and Intel iGPUs
and NVIDIA RTX cards are tested.

```bash
git clone https://github.com/diogochaves/omabox && cd omabox
./install.sh                            # packages (sudo only if some are missing), omabox, links
./install.sh --check                    # the same, then start a box, screenshot it, tear it down
```

`install.sh` builds omabox (Rust) and a private copy of aquamarine with the fixes boxes need (your
system's is not touched), then runs `omabox setup`: it links `~/.local/bin/omabox`, the agent skill
for Claude Code, Codex, OpenCode, pi and Hermes, and the bar widget, and asks whether to put the
widget in your bar and to turn on the [agent guard](#the-agent-guard). **Update** with
`git pull && ./install.sh`, after a Hyprland upgrade too. **Remove** with `omabox down --all &&
omabox setup --remove`, then delete the clone. Every step, upgrading from 0.x and what is left
behind: [docs/install.md](docs/install.md). A box that will not start: `omabox doctor` says what
this machine lacks and the line that installs it, changing nothing (`--smoke` starts a box too).

## Quick start

```bash
omabox up                                 # a hidden desktop for this repo; returns when its bar is drawn
omabox run -d --wait -- ./build/myapp     # launch your app in it, detached, once its window is up
omabox shot                               # a screenshot of the box; prints the PNG's path
omabox peek                               # watch it live, in a view-only window on workspace 9
omabox down                               # end it
```

That is the loop an agent runs, through the skill, without being told. Tests that touch the desktop
are one line: `omabox run -- ctest --test-dir build --output-on-failure`.

Every command, testing a shell plugin, a Hyprland build or an Omarchy checkout, several monitors:
[docs/usage.md](docs/usage.md). `omabox help` lists every command and flag.

## Seeing a box yourself

- **`omabox peek`**: a live, view-only window of any box, opened on workspace 9 without focus. The
  agent never notices; a ring shows where it clicks and the keys it types.
- **`omabox up --interactive`**: a box as a window you drive with your own keyboard and mouse.
  SUPER+ALT+ESCAPE sends SUPER keys to the box instead of your desktop.
- **The bar widget** lists every box, with Peek, Screenshot and Down, and starts a new interactive
  box.

<p><img src="docs/media/peeks.webp" alt="Four peek windows in a grid, each a live view of an agent's box: the same app in three themes, one with its Add torrent dialog open" width="800"></p>

The clipboard, keys-to-box, the widget's settings and keys: [docs/seeing-a-box.md](docs/seeing-a-box.md).

## Agents

`install.sh` installs the `omabox` skill wherever Omarchy installs its own agent skills, so Claude
Code, Codex, OpenCode, pi and Hermes load it when a task involves a GUI app, a screenshot, a shell
plugin or desktop-touching tests. Projects need no changes: when a project's own instructions say
"run the app", "screenshot with grim", "use hyprctl" or "run ctest", the skill has the agent do
exactly that in a box.

To make it a rule rather than the agent's judgement, add a line to your agent's global instructions
(`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`, `~/.config/opencode/AGENTS.md`, ...):

> Never launch, screenshot or drive GUI apps on my desktop, and never run tests that touch the desktop
> session there. Use omabox (see the omabox skill).
### The agent guard

The skill is advice: an agent that does not think of `./build/tests/tst_x` as GUI work still opens
its windows on your desktop, because its shell holds your real display. The guard (opt-in; Claude
Code and Codex, or any agent through `guard exec`) makes that fail instead: agents' shells get a
display that does not exist, so a stray window, `hyprctl`, `grim` or `omarchy-theme-set` fails with
an error the skill explains, and a link an agent opens is refused instead of landing in your
browser. `install.sh` asks (yes by default); it never turns the guard on by itself.

```bash
omabox guard on               # every installed agent (or: on claude | on codex)
omabox guard off              # take it out again
omabox host -- CMD            # from a guarded shell: one command on your real desktop, when you asked for it
omabox guard exec -- AGENT    # any other agent: start it under the guard
```

It stops accidents, not an agent that sets the variables back on purpose, processes the agent starts
outside its shell (a headed browser MCP opens on your desktop), writes to the files your shell
watches, or anything over the session bus. What it sets for each agent, and the full list:
[docs/guard.md](docs/guard.md).

## With ai-jail

[ai-jail](https://github.com/akitaonrails/ai-jail) ([aijail.io](https://aijail.io)), by Fabio
Akita, fences the agent in: bubblewrap, Landlock and seccomp on Linux, `sandbox-exec` on macOS, so
your home, keys and cloud credentials are out of reach. ai-jail answers what the agent can touch,
omabox where it draws, and they stack: an agent inside ai-jail drives boxes of its own through
omabox's broker, with no change to ai-jail.

```bash
omabox broker on   # a systemd user socket; prints the lines to add to ~/.ai-jail
ai-jail claude     # then, as usual: omabox up, shot, keys, click work inside the jail
```

The broker keeps a jail's boxes to what the jail has: no network when the jail has none, only the
jail's own project mounted, and only the boxes that jail started, which go down when it exits. A
jailed agent cannot use `omabox host`, `peek`, interactive boxes or `guard`. Running a build you do
not trust with a box's screen as its only display, and the details: [docs/ai-jail.md](docs/ai-jail.md).

Tested with ai-jail 2.10.0 and 2.12.0 and omabox 1.0 on Linux, and before that with ai-jail 2.2.1,
2.6.2 and 2.8.1.

## What a box can and cannot touch

- **Your files**: the repo you run `omabox up` from, **read-only** at the same path (from a git
  worktree or submodule, its git dir in the main checkout's `.git` too, so git works there; inside
  ai-jail, a worktree's when the jail has that `.git` as a folder of its own, as `ai-jail --worktree`
  maps it), plus the dirs
  in `~/.config/omabox/ro-bind` (one per line) or `--ro-bind`; mise's toolchains (so `omabox run`
  finds node, python, uv as on the host), the `--plugin` and `--theme-dir` dirs, your fonts
  (`~/.local/share/fonts`, `~/.fonts`, at the same place in the box HOME; not in a `--clean` box,
  `--no-seed fonts` leaves them out) and your git `user.name` and `user.email`. Nothing else of your
  HOME. Refused whatever you pass: anything that
  is or contains HOME, `~/.config/omarchy` or `/tmp`, anything inside `~/.config/omarchy` but
  `plugins/` and `themes/` (`api-keys.env`, your hooks), the secret stores (`~/.ssh`, `~/.gnupg`, keyrings,
  `~/.password-store`, `~/.aws`, `~/.kube`, `~/.docker`, `~/.netrc`, `~/.git-credentials`, gh's,
  gcloud's, azure's and 1Password's `op` config; the coding agents' own, `~/.claude`, `~/.codex`, OpenCode's,
  pi's, Hermes' and Gemini's; the browsers' profiles, `~/.mozilla`, Chromium's, Chrome's, Brave's, Vivaldi's,
  Edge's, and `~/.pki`), omabox's own saves and box HOMEs
  (`~/.local/share/omabox`, `~/.cache/omabox`), your runtime dir, `/run`, `/dev`, `/proc`, `/sys`.
- **Its HOME** is fake, seeded with your theme and shell settings and nothing secret (no API keys,
  keyrings or tokens), and `omabox down` deletes it with whatever a plugin or app changed there.
- **Its session**: a private session bus, its own accessibility (AT-SPI) bus and a throwaway keyring
  (secrets stored and read without prompts), no system bus, no real input devices, no audio unless `--audio` (then its own PipeWire, a
  speaker nobody hears and a silent mic; never your audio server or `/dev/snd`), no Xwayland unless `--xwayland`.
  `/usr`, `/etc` and `/sys` are read-only, with no `sudo`, pacman or polkit, so nothing in a box can
  install packages, system config or rules. Your host's processes are not visible from it.
- **Your seat**: a box never gets `/dev/dri/card*`, `/dev/input`, seatd, the system bus or your real
  `$XDG_RUNTIME_DIR`: that is what keeps its Hyprland off your real screens. An interactive box
  holds one already-open connection to your compositor, never its socket, so nothing inside can open
  windows on your desktop. Keys go the other way: what you type into its window, the box reads.
- **The network**: every box has one of its own. By default (`--net connected`) it reaches the
  internet, your LAN and your host's servers, and you reach its servers; with `--net isolated` it has
  only a loopback and the host ports you `--allow` (`omabox up --net isolated --allow 8081,8082`),
  and any `--addr` you give it on that loopback ([docs/network.md](docs/network.md)).
  What reaches outside (API calls, uploads, changes to a server) is real, and `down` does not undo it.
- **Not a security boundary**: a box shares your kernel and GPU (a runaway app can load or hang it)
  and, by default, your network. Commands you run on the host, a project's `sudo ./setup` or
  `omabox host -- CMD`, are not boxed. For code you do not trust, use `--net isolated` at least,
  [ai-jail](#with-ai-jail), or a VM. [SECURITY.md](SECURITY.md) lists each rule that keeps a box
  off your desktop, where it is enforced and the test that proves it.

Some things still need your real machine: your real data and services, desktop integration outside a
session (.desktop files, URL handlers, autostart; systemd user units only in a `--systemd` box, and
never with journald or logind), real monitors (their modes, HDR, VRR, a real hotplug), idle and
suspend, a real password at the lock screen, and final release acceptance.

Ports, `localhost` across the boundary, more addresses on a loopback, Tailscale, `--ro-bind`,
`--seed` and the other mounts: [docs/network.md](docs/network.md). What is untested or works
differently in a box: [docs/limitations.md](docs/limitations.md).

## Stability

omabox is used mostly by agents, which read its warnings; a script or CI reads `--json` and exit
codes, and watches stderr for `deprecated:`. From 1.0, within 1.x:

- **Kept, or changed only by addition:** exit codes (0 done or yes, 1 failed or no, 2 called wrongly,
  124 not in time; `check run`'s 3 and 4; `run`'s 125 for its own failures, its command's status
  otherwise: `omabox help exit-codes`), and `--json`'s field names, types and meaning (fields may be
  added: read the ones you need by name; `skill/reference.md`, "JSON", lists them).
- **Changed only after a warning:** commands (`brief` and the aliases help documents among them),
  options, arguments, `config` keys and `OMABOX_*` variables. The new form comes first; the old one
  keeps working for one minor release, with `omabox: deprecated: OLD: write NEW` on stderr and a
  CHANGELOG line under "Deprecated"; then it is refused with a line naming the new one, never read
  another way. `OMABOX_STRICT=1` refuses it at once (exit 2), for CI.
- A save made by a 1.x starts with any later 1.x (`up --from`). A `check` baseline approved by a 1.x
  compares with any later 1.x, or is refused ("approve again"), never compared wrongly. [What a box
  cannot touch](#what-a-box-can-and-cannot-touch) only gets stricter.

Not covered: the wording of messages and tables; help and skill text; defaults and heuristics (each
change gets a CHANGELOG line, and no timeout gets shorter in 1.x); the pixels of the images omabox
draws; what other programs report through omabox, as they report it (`hyprctl`, `lua`, Hyprland's
own fields in `windows --json`, `events`, `tree`'s roles); a box's inside (its HOME, config and
logs); and internal or undocumented forms, such as a box's name given to `peek`, `reload` or
`restart-shell` without `-b`.

## Docs

- [omabox.app](https://omabox.app): [install](https://omabox.app/install),
  [what agents use boxes for](https://omabox.app/uses), [testing your app](https://omabox.app/develop),
  [Omarchy in a box](https://omabox.app/omarchy), [how it works](https://omabox.app/how-it-works),
  [every command](https://omabox.app/commands).
- In this repo: [install](docs/install.md), [using omabox](docs/usage.md),
  [seeing a box yourself](docs/seeing-a-box.md), [the agent guard](docs/guard.md),
  [with ai-jail](docs/ai-jail.md), [network and mounts](docs/network.md),
  [how it works](docs/architecture.md), [known limitations](docs/limitations.md).
- `omabox help` lists every command and flag; `omabox help CMD --long` one command in full;
  `omabox help exit-codes` the exit codes.
- [SECURITY.md](SECURITY.md): the rules that keep a box off your desktop, where each is enforced
  and the test that proves it; how to report a hole.
- [CHANGELOG.md](CHANGELOG.md): what changed in each version.
- [CONTRIBUTING.md](CONTRIBUTING.md): how to send a fix, and what a bug report needs.
  Contributions are welcome, and so are people trying it on other NVIDIA models, other GPUs,
  multi-monitor desks, or a fresh Omarchy install.
- [NOTES.md](NOTES.md): design notes, findings (cited in the code as "finding N") and dead ends.
- [UPSTREAM.md](UPSTREAM.md): what omabox carries until upstream releases land, and what to drop
  then.

## License

MIT: see [LICENSE](LICENSE).

<sub>omabox is an independent community project, not affiliated with or endorsed by Omarchy.</sub>
