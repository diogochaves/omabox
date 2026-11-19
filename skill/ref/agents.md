# omabox: agents, the guard and reporting

The safety rules are in `SKILL.md` ("Never"), some spelled out here; nothing here relaxes them.

Read it for project instructions, subagents and worktrees, the guard, ai-jail, prompts, what a box cannot test, and showing or reporting to the user.

Sections:

- Project instructions, in detail
- Handing box work to a subagent
- For subagents in an isolated worktree
- The guard, in detail
- Inside ai-jail
- Commands that run without a prompt
- When a box cannot test it, in detail
- Showing the user
- Reporting what a box showed

## Project instructions, in detail

A project's CLAUDE.md, AGENTS.md or README written for the real desktop **still holds: carry it out
inside a box.**

| The project says | Do |
|---|---|
| `./build/app`, `app &` | `omabox up`, then `omabox run -d -- ./build/app` |
| `hyprctl …`, `hyprctl eval EXPR` | `omabox hyprctl …`, `omabox lua EXPR` (a dispatch: `hl.dispatch(hl.dsp…)`) |
| `grim`, `wtype`, `ydotool`, `hyprctl -j clients` | `omabox shot`, `keys`, `click X Y`, `windows` |
| `ctest`, a test binary (tray, D-Bus, keyring…) | `omabox run -- ctest …` (or the binary) |
| `omarchy-theme-set`, `omarchy plugin …`, `omarchy-shell …` | `omabox run -- …` (`omarchy plugin validate` only reads: fine on the host) |
| `omarchy restart shell` | `omabox restart-shell` |
| A plugin in `~/.config/omarchy/plugins`, `shell.json` edits | `omabox up --plugin PATH`; `shell.json`: `omabox path --home` |
| A theme in the works; a theme or bar result | `omabox up --theme-dir PATH`; `--theme NAME --stock-bar` |
| What a new Omarchy user sees (a showcase, shots for others) | `omabox up --stock` |
| Shots compared across runs | `omabox up --clean` (ref: checks.md) |
| Install or remove the package | Not a box (read-only `/usr`): a VM, or the user |

- More of the project's steps, in a box: `grim -T ID` → `omabox shot --window SEL`; a test binary run
  directly (`./build/tests/tst_x`) has none of ctest's environment (a Qt test opens real windows):
  `omabox run -- ./build/tests/tst_x`; starting the app from the launcher, its `.desktop` and icon:
  its files in the box HOME, then `omabox keys --wait super+alt+space` ("Testing an app as a desktop
  app" in apps.md); `omarchy-theme-set NAME` → `omabox run -- omarchy-theme-set NAME`; `omarchy plugin
  add/enable/disable/remove` → `omabox run -- omarchy plugin …`; a back-up and restore of the bar
  around a test ("put the desktop back afterwards"): nothing to restore, the box's bar is its own
  (`omabox down`). `hyprctl eval EXPR` to read a value → `omabox lua EXPR`, which prints the value.
- Themes: a theme in `~/.config/omarchy/themes` linked to your working copy → `omabox up --theme-dir
  PATH` (live; else a copy made at `up`). A result that depends on the theme or bar (colours, sizes,
  a theme-switch flow) on an otherwise normal box → `omabox up --theme NAME --stock-bar`; else they
  are the desktop's.
- How it looks to a new Omarchy user (a showcase, a screenshot for someone else, "with a default
  look") → `omabox up --stock`: Omarchy's bar on tokyo-night (or `--theme NAME`) with the theme's
  own wallpaper, English in UTC, and nothing of the desktop's look (terminal configs, branding, the
  user's themes and wallpapers, the plugins' settings, Hyprland toggles, fonts); the clock live,
  animations on, the network as usual; `--plugin`, `--theme-dir` and the rest on top. `--theme NAME
  --stock-bar` alone still has the rest of the desktop's look. Shots compared across runs →
  `omabox up --clean` (`--stock` plus the freezes: ref: Visual checks in checks.md); never for a showcase (a
  `#ff00ff` wallpaper, the clock frozen at "Friday 12:00", no network, animations off).
- A step that needs real hardware is not moved into a box: it goes to the user ("When a box cannot
  test it, in detail").
- A box starts with a fresh HOME: apps start as on first run. A server in a default box also holds
  its port on the user's 127.0.0.1 (their own dev server on it then fails to start), and a probe there
  can reach theirs or another box's (only one box gets a port on the host; `omabox ports` says which
  holds it): check your server from inside the box (`omabox wait until -- curl -fsS
  http://127.0.0.1:PORT/`). A server that fails with "address in use" on a port nothing in the box
  uses: the host or another box holds it (`omabox ports` names it); use another port. An isolated
  box's ports are its own; two of them can use the same one.
- Names: the repo's directory plus your session's id (`myrepo-5cc72cdc` in a Claude Code or Codex
  session); in a git worktree, the worktree's folder, so a worktree agent has its own box with no
  `-b`. A box goes down when your agent exits, not on `/clear` or `/resume`. `-b NAME` also goes
  before the command (`omabox -b box-3 shot`). In a worktree-isolated Claude Code subagent: "For
  subagents in an isolated worktree" below.
- `omabox help CMD` (or `omabox CMD --help`): one command's flags, one line each; `omabox help CMD
  --long` adds its notes (what each flag does, defaults, limits): read it before guessing.
  `omabox help`: every command, a line each; `help --all`: every command's flags.

## Handing box work to a subagent

`omabox brief -b NAME` prints what a subagent needs to drive that box, ~15 lines (~450 tokens,
against the skill core's ~2 000): the box's name, size and scale, its windows one a line (each by a
`--window` word that finds it alone, else its address; six at most), then the rules: `-b NAME` on
every call, one literal command per call, look, act once, look again, never resend input not seen
landing, no sleep, never the real desktop, the box HOME's host path, and the five below. Paste it
whole into the subagent's prompt: the subagent needs no skill, and the brief's last line tells it to
load the skill when the brief is not enough. Run it right before delegating (the windows are as
they were then). With no box up it prints the same rules for a box the subagent starts itself (`up
--new`, down when done), and says so on stderr; exit 0 either way.

**Parallel subagents share your session's default box name** (it comes from the session's id, which
they all have), so two that run `omabox up` get one box, and one's `down` ends the other's test. Give
each its own: `omabox up --new` (then `-b box-N`, as printed) or `-b` a name of its task; it never
takes down a box it did not start (a box with your session's suffix in `omabox ls` may be a
sibling's: `ls` says where each was started, and `down` warns of one another caller used just now).

## For subagents in an isolated worktree

Claude Code checks each Bash call of a worktree-isolated subagent and refuses what it cannot follow:
a word it reads as a shell (`wait cmd`), shell text it cannot see through (`$(…)` with more after it,
a pipe into another program, a heredoc with `cd`), anything leaving the worktree. Each refusal costs
a turn. Five rules keep every omabox call plain:
- **A state: `omabox wait until -- CMD`** (never `wait cmd`, refused even alone); the screen: `wait
  still`, `wait change` or the action's `--wait`, never a sleep.
- **Several shell steps in the box: a script in the box HOME.** Write it with your file tool at the
  path `omabox path --home` printed, then `omabox -b NAME run -- sh /home/sbx/F.sh`. A host script:
  `omabox run -- python3 - < FILE`, alone in its call.
- **Box files from the host: `omabox path --home` once**, then Read, Write and Edit on that literal
  path (`/home/sbx/X` in the box is `<that path>/X`); never `$(omabox path)` inside a command.
- **Every argument literal**: no `$VAR`, no `$(…)`; look the value up first (`omabox run -- pgrep -x
  APP`), then type it (`--pid 1809`).
- **Stay in your worktree**: no `cd` out of it, no git on another checkout (absolute paths instead);
  scroll with `omabox scroll … --source wheel`, not `pointer -- source`.

## The guard, in detail

- Under the user's agent guard your commands get `WAYLAND_DISPLAY=omabox-guard`, an empty `DISPLAY`
  and `HYPRLAND_INSTANCE_SIGNATURE=omabox-guard`, so what would reach the real desktop fails: do it in
  a box. Never set those back to the real session or take the display from elsewhere
  (`/proc/*/environ`, `hyprctl instances`, `$XDG_RUNTIME_DIR/wayland-*`). `xdg-open`, `gh --web` and
  `$BROWSER` fail too: give the user the link.
- Only when the user asked for their **real** desktop in this task ("switch my theme", reload my
  config), run that one command as `omabox host -- CMD`. Never hand them a `! CMD` that touches their
  desktop (their `!` is guarded too): their own terminal is for that. Never use `host` to switch
  their workspace or focus to see a box.
- Before touching the real desktop for a check a box cannot make, ask once: "When a box cannot test
  it, in detail".
- `QT_QPA_PLATFORM=offscreen` still works under the guard. To look at a web page yourself, open it in
  your box (`omabox run -d -- xdg-open URL`, then `omabox shot`). `quickshell kill`, `qs ipc` and
  `omarchy-shell` are refused ("not running quickshell kill"): they would reach the user's desktop
  shell; a box's shell is `omabox run -- qs ipc …` (or `omarchy-shell …`) or `omabox restart-shell`.
- `omabox host -- CMD` examples, once the user asked for their real desktop: `omabox host -- hyprctl
  reload`, `omabox host -- omarchy-theme-set NAME`. Testing, screenshots and anything the user did not
  ask to see on their desktop stay in a box: `shot` works on a hidden interactive box.
- "this hook is outdated (omabox changed it)" at session start: tell the user to run `omabox guard on`
  (it rewrites your own settings, so leave it to them).

## Inside ai-jail

omabox works through the user's broker (no `host`, `peek` or `--interactive`). "this sandbox refuses
new namespaces": ask the user to run `omabox broker on` outside the jail and add what it prints to
ai-jail's config (`~/.config/ai-jail/config.toml`; `~/.ai-jail` before ai-jail 3.0). Never try to get around the jail. The jail's project goes into the box only when it is a
git repo; `up` says so of one without a `.git`: `up --ro-bind DIR` (or `--overlay`) brings it in.

## Commands that run without a prompt

The skill's `allowed-tools` pre-approves the verbs that act only inside a box and read none of the
user's files: `shot`, `pixel`, `windows`, `window`, `tree`, `keys`, `click`, `scroll`, `drag`,
`pointer`, `wait`, `jobs`, `stop`, `log`, `events`, `hyprctl`, `lua`, `restart-shell`, `reload`,
`mode`, `output`, `monitor`, `cpu`, `audio`, `gdb`, `calibrate`, `ports`, `ls`, `path`, `brief`,
`help`. Each is a rule `Bash(omabox VERB *)`, so:

- **Claude Code** applies it during the turn that invoked the skill (a subagent's whole run; in a
  session, until the user's next message), only to `omabox VERB ...` as typed: `omabox -b NAME shot`,
  `bin/omabox shot` or `FOO=1 omabox shot` still ask. Put `-b NAME` after the verb. A `&&` chain is
  pre-approved only when every part is.
- **Still asked:** `host`, `peek`, `keys-to-box`, `clip` (the real desktop and clipboard); `config`,
  `guard`, `setup`, `broker` (the user's settings); `up` and `run` (a rule cannot tell `up` from `up
  --interactive`, or a box's `run` from a throwaway's `--ro-bind`, `--seed`, `--plugin`,
  `--desk-shell` or `run --env-file FILE`); `down` (`--all`), `save`, `saves`, `gpu` (`gpu release`
  ends other boxes); `record` (`record sheet VIDEO` reads any video), `check` (`check run` runs a
  scenario on the host); `env`, `which`, `--version` (harmless, rare).
- **What the listed ones write on the host:** only the images named with `-o` (`shot`, `drag
  --shot`, `calibrate CARD`; a burst's `-o DIR` frames), at a new path or over an earlier image or
  an empty file: a link, a folder or any other file there is refused (`is there and is not a PNG`),
  untouched. `keys --pass VAR` types a variable of
  the agent's own environment, as the user exported it. `shot --after`'s action takes no
  `--env-file`. Keys, clicks and `hyprctl dispatch exec` run anything inside the box, which reaches
  the network as the box does (ref: Network in session.md): a box the user started is still theirs.
- **Codex** reads no grants from a skill: the user adds `prefix_rule(pattern = ["omabox", "shot"],
  decision = "allow")` lines to `~/.codex/rules/default.rules`. **OpenCode**: `"permission": {"bash":
  {"omabox shot *": "allow"}}` in `opencode.json`. **pi** asks for nothing; **Hermes** follows its
  own approval settings. Only the user changes these files.

## When a box cannot test it, in detail

A box has virtual screens only (`--monitor` for more, any size and scale; `output drop` makes one
vanish and return), no system bus (NetworkManager, bluetooth, UPower, logind), no devices (audio,
DDC/CI, backlight, real input, USB), no suspend, a lock screen with no real password, and a fresh
HOME. A change touching `hl.monitor`, HDR/VRR/`cm`, `ddcutil`, backlight, `wpctl`, `nmcli`,
`bluetoothctl`, `powerprofilesctl`, `journalctl`, `loginctl`, `/dev/…` or the user's real accounts
and data cannot be verified in a box: do not report it as tested. (`systemctl --user`: `omabox up
--systemd`.)

- **Split the work**: the box for what it can show; name the part that needs the real machine.
- **Ask once before touching the real desktop**: say exactly what will change ("your monitor at 120
  Hz for 15 s, then back"), wait for a yes (it covers this task, not the next), use `omabox host --
  CMD` under the guard, follow the project's own safety rules and put everything back.
- **A host `pgrep` sees boxes too** (a box's shell has the desktop's command line): before reading
  two `quickshell`s as two desktop shells, `omabox which PID` names a pid's box (or `not in a box`).
- The screens: any size, refresh rate, number (`--monitor`) and scale for the extra ones; no real
  modes, HDR, VRR, 10-bit, colour management or DPMS. The screen going and coming back (a monitor
  dropping off on wake, a KVM, a dock) can be tested: `omabox output drop [NAME] --for 300ms
  --cycles 20`, which stops at the first shell crash; an unplug for good: `omabox monitor remove NAME`.
  On NVIDIA an output the app creates itself (`hyprctl output create headless`) comes up 0x0: a box
  on the other GPU (`OMABOX_RENDER_NODE`, "Screen size and rendering cost" in shots.md).
- **Nothing in a box is root's**: one user, so `/`, `/run`, `/tmp` are yours and the host's root
  files read as `nobody` (65534). Code that wants a root-owned parent chain or refuses a path the
  user owns (Omarchy's update inhibitor, sudo and Windows VM helpers; their update-lock,
  update-stay-awake-security and windows-vm-compose tests) refuses in a box: not testable there.
- Omarchy's own suite: `omabox run --stock-bar -- ./test/all` from the checkout (its background tests
  read a pixel the desk's bar may cover). Besides the three above, a test that sets `HOME` but not
  the `XDG_*_HOME` vars, or times the shell against a fixed budget (faster on the iGPU's render node,
  `OMABOX_RENDER_NODE`), fails as it would on a desk like yours.
- No system bus: no NetworkManager, bluetooth, UPower/power profiles, udisks, logind, polkit (an
  agent's dialog: `up --polkit`, "Testing a polkit agent" in shell.md; `timedatectl list-timezones` and `show -p
  Timezone` answer from tzdata; setting it fails). No
  devices: no sound card (`up --audio` gives a box its own PipeWire, with a null sink, omabox-speaker,
  as its only output and a silent mic, omabox-mic: apps play, nobody hears, `omabox audio` and
  `record --audio` read it; real outputs, Bluetooth audio and `wpctl` on the user's devices stay
  untestable), no `/dev/i2c` (DDC/CI brightness), no backlight, no real keyboards,
  mice, touchpads, tablets, cameras, USB or printers. No systemd user manager unless `up --systemd`
  (never journald or logind), no installed `.desktop` files or URL handlers, no idle or suspend; the
  lock screen locks but takes no real password ("Testing the lock screen" in shell.md).
- **No PipeWire realtime in a box**: every box writes `~/.config/pipewire/client.conf.d/90-omabox.conf`
  (`module.rt = false`: a box has no RealtimeKit, and the limit of 0 its portal gave got processes
  SIGKILLed), so a client's `RLIMIT_RTTIME` stays unlimited where module-rt would set one, and a bug
  in that (a `0/0` limit) does not show. One client without the file: `omabox run -- env
  PIPEWIRE_CONFIG_DIR=/usr/share/pipewire APP` (PipeWire's own config only, no drop-ins); in a box
  module-rt then sets `0/0`, as on a desk without RealtimeKit (one with it gets 200000).
- Reporting: each plugin's `commit` in `ls --json`'s `plugin_status` ends in `+dirty` for uncommitted
  edits. Virtual monitors are not the user's: say which layout you tested.

## Showing the user

- Only when asked: `omabox peek` or `omabox up --interactive` (a box they drive; never bring its
  window forward yourself). `omabox config` is the user's: change it only when asked. `omabox clip`
  refuses agents (`host` too): never work around it; type text with `keys -t`.
- `omabox peek` opens a live, view-only window of your headless box on the user's workspace 9 (or the
  one they set with `omabox config workspace`) without taking focus. It does not affect the box. Your
  `click`, `pointer` and `keys` show on it for ~3 s (a ring, key captions; `--pass` values as `*`),
  never in your shots. A window shows one monitor (the main one, or `peek --monitor NAME`): one window
  per monitor, tiled on that workspace, each with the clicks and pointer on its own monitor (keys on
  all); the whole layout at once is `omabox shot`.
- `omabox up --interactive` makes the box a real window on that workspace that the user drives
  (SUPER+ALT+ESCAPE sends SUPER keys to it). The host keeps drawing it while it is hidden, so `shot`,
  `click`, `keys` and `wait` work on it. Once the user closed that window and kept the box running,
  the new one is drawn only while shown: `click` and `keys` still reach it, but `shot` gets no frame,
  and the new window can open on an empty workspace, where they reach no app. When the user has to
  act in it (a login), tell them which workspace it is on and let them go there.

## Reporting what a box showed

Say "in an omabox box (Omarchy VERSION, Hyprland VERSION, WxH and any `--monitor`s, theme NAME)"
(`ls --json`: `omarchy_version`, `hyprland_version`; null: `omabox hyprctl version`) and your
project's commit. Layout, focus, input routing, the launcher, theme switches, notifications, the tray
and the keyring were tested for real; what a box lacks ("When a box cannot test it, in detail") and
the installed package were not: say so. Never "tested on the desktop" for a box result. Quirks:
`NOTES.md` (`readlink -f $(command -v omabox)` → `../NOTES.md`).
