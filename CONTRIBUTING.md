# Contributing

omabox is young, and it has only run on a couple of machines. You can help without writing any code:
try it on a setup it has not seen (NVIDIA, another GPU, several monitors, a fresh Omarchy install,
a distribution with Hyprland 0.56+ other than Arch, Debian sid and openSUSE Tumbleweed) and say what
happened. Bug reports, fixes, tests, docs and ideas are all welcome.

## Reporting a problem

Open an issue with:

- `omabox --version`, `Hyprland --version` and your Omarchy version (`omarchy version`), or without
  Omarchy your distribution (`/etc/os-release`'s `PRETTY_NAME`);
- your GPU (`ls -l /sys/class/drm/renderD*/device/driver`);
- the command you ran and what it printed;
- for a box that failed or died, its logs: `omabox ls` names it, and `omabox path -b NAME` is where
  they are (`box.log`, `home/*.log`, `run/hypr/*/hyprland.log`). Look them over before you attach
  them: they can hold paths and names from your machine.

If you use omabox with an agent, it can do the reporting. `omabox setup --feedback-skill` adds the
`omabox-feedback` skill next to the `omabox` one (Claude Code, Codex, OpenCode, pi, Hermes); it is off
unless you add it. It runs only when you ask (`/omabox-feedback`, or "anything for omabox?"): the
agent looks back through the session for what omabox got wrong, got in the way of or lacked, checks
the existing issues, and drafts each one with your paths, names, projects and the session's content
taken out. It shows you a short list, prints any draft you want to see, takes your changes, and
posts nothing until you say yes. `omabox setup --no-feedback-skill` takes it out again.

A problem with safety (a box reaching your real desktop, your files or your session) matters most:
report it privately, as [SECURITY.md](SECURITY.md) says, not in an issue.

## Changing it

```bash
./install.sh           # builds the tools, omabox itself (cargo, into build/omabox) and aquamarine's fix
test/run.sh unit       # the fast tier, no box
test/run.sh            # the whole suite in real, headless boxes, side by side (~1.5 min on 16 CPUs); before a PR
test/run.sh -j 1       # the same, one test at a time (~8.5 min): what a leak is pinned to, exactly
OMABOX_AQUAMARINE=system test/run.sh   # on the system's aquamarine, as a package without the fix runs
test/run.sh --installed   # against omabox installed read-only at /usr/lib/omabox, as a package would (a throwaway namespace)
shellcheck install.sh release.sh docs/demo.sh test/run.sh share/*.sh share/bin/* $(grep -l "^#!" share/guard/*)
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test   # omabox itself (crates/omabox)
```

CI runs that shellcheck (version 0.11.0), the cargo checks and the release build on every pull request. It cannot run the suite: that needs
a Hyprland session, so run it yourself and say in the pull request what passed.

Read [AGENTS.md](AGENTS.md) first, whether you are a person or an agent: it has the rules the code
keeps. The ones that matter most:

- **Never disturb the real desktop.** Test input and UI changes in a box, or in a box standing in
  for the desktop (how the suite checks interactive mode, peek and the guard). Never run
  `tools/keyboard` or `tools/pointer` from a host shell.
- **The box safety invariant**: a box never gets `/dev/dri/card*`, `/dev/input`, seatd, the system
  bus or your real `$XDG_RUNTIME_DIR`. Those are what keep its Hyprland off your seat.
- A box's HOME is seeded without secrets, and host code is mounted read-only.

Then:

- Keep pull requests small and focused, one change each, with a commit message that says why.
- A fix comes with a check in `test/run.sh` that fails on the old code, when a box can show it.
- Every change of behaviour gets a line in [NOTES.md](NOTES.md) (a finding, or an item under Open), and
  user-visible ones a line under Unreleased in [CHANGELOG.md](CHANGELOG.md).
- Say what you verified in a real box and what you only reasoned about.

## Releasing

For the maintainer. After the last pull request of the release is merged and the full suite has passed
on `main`:

1. One commit, `omabox X.Y.Z`: `VERSION`, `plugin/manifest.json`'s `version` and
   `plugin/Panel.qml`'s `pluginVersion` all to X.Y.Z; in `CHANGELOG.md`, `## Unreleased` becomes
   `## X.Y.Z — DATE` with a short summary under it.
2. `git tag -a vX.Y.Z -m "omabox X.Y.Z"`, then push `main` and the tag.
3. `gh release create vX.Y.Z --title "omabox X.Y.Z"` with that CHANGELOG section as the notes, each
   paragraph and list item joined onto one line (GitHub shows every newline in release notes).
4. `./release.sh --upload`: attaches `omabox-X.Y.Z.tar.gz` and `SHA256SUMS` to the release.

If you want to work on something bigger, open an issue first so we can talk it over. NOTES.md's
"Open" list and [UPSTREAM.md](UPSTREAM.md) are good places to start.
