# Installing omabox

What a machine needs, what `install.sh` does, updating and removing. Back to the
[README](../README.md).

## What you need

- **Omarchy 4.5** on Arch, with Hyprland 0.56+ (the Lua config). Omarchy 4.0 still works but is
  deprecated (`up` and `setup` say so on a `deprecated:` line): omabox 1.3 drops it, not before
  2026-12-02, four weeks after 4.5 reached Omarchy's stable channel
  ([#312](https://github.com/diogochaves/omabox/issues/312)). `omarchy update` brings 4.5.
- **Or another Linux with Hyprland 0.56+** ([#247](https://github.com/diogochaves/omabox/issues/247)):
  Arch without Omarchy, Debian sid, openSUSE Tumbleweed (aimed at: checked in containers before a
  release), Fedora (its packages named, untested: Hyprland only from COPR). There `install.sh`
  checks your distribution's packages under their own names and, if some are missing, prints the
  command that installs them (`sudo apt install …`, `sudo zypper install …`) and stops: run it, then
  `./install.sh` again. Without Omarchy, boxes are plain Hyprland (no Omarchy bar, theme or plugins),
  `setup` links no bar widget, and `omabox doctor` says so as information, not failures. NixOS is
  not supported yet: a box looks for its programs in `/usr/bin`.
- **A GPU render node** (`/dev/dri/renderD*`). Tested on AMD and Intel iGPUs, an NVIDIA RTX 4070
  SUPER (driver 615.71.09) and an RTX 5070 Ti (610.57.04, open kernel module). A headless box takes
  the first usable node, or the GPU `omabox config gpu` names (`OMABOX_RENDER_NODE` overrides); an
  interactive box renders on the GPU your desktop renders on.
- For headless boxes on NVIDIA and for confirm-close: aquamarine's fix (Hyprland's backend library),
  until a release ships [PR #415](https://github.com/hyprwm/aquamarine/pull/415) (a system copy
  patched with it counts: omabox looks for the fix, not the version). Everything else
  runs on your system's aquamarine. `install.sh` builds it privately into `build/prefix`
  (`omabox setup --aquamarine`); your system's copy is not touched. That build also carries fixes
  of omabox's for interactive boxes: a key held when the box's window loses focus is released, in a
  monitor's window your pointer lands on that monitor and your cursor stays shown, and each
  monitor's window has a class of its own, which keeps it off your workspace (since 1.0; monitors
  on an interactive box need it). No system aquamarine has those, so `omabox setup --aquamarine`
  builds them even when yours has PR #415 (enough on its own for headless NVIDIA boxes and
  confirm-close, not for those). It builds the version your Hyprland links: aquamarine 0.15.1 with
  the fix for Hyprland 0.56, 0.16.0 for Hyprland 0.57 (Arch, openSUSE). For a Hyprland newer than
  this omabox knows, or when the build fails, `install.sh` goes on: boxes run on your system's
  aquamarine, and it says what interactive boxes go without.

## Install and update

```bash
git clone https://github.com/diogochaves/omabox && cd omabox
./install.sh                            # packages (on Arch, sudo only if some are missing; elsewhere the command to run), tools, omabox, aquamarine's fix, links
./install.sh --check                    # the same, then start a box, screenshot it, tear it down
```

`install.sh` then runs `omabox setup`, which links `~/.local/bin/omabox`, the agent skill (wherever
Omarchy puts its own skills: `~/.agents`, `~/.claude`, `~/.codex`, `~/.pi/agent`, `~/.hermes`) and
the bar widget (on Omarchy), and asks whether to put the widget in your bar and to turn on the
[agent guard](guard.md) (or: `omarchy plugin enable chaves.omabox`, `omabox guard on`). omabox is built
from Rust: `install.sh` installs Arch's `rust` (elsewhere it names your distribution's `cargo`) when there is no `cargo` (a rustup needs a default
toolchain), and the first build fetches its crates. **Update** with `git pull && ./install.sh` in one go
(until it has run, `bin/omabox` links to a binary not built yet);
run it after a Hyprland upgrade too: a private aquamarine whose soname Hyprland no longer links is
skipped for the system's (`omabox --version` says which one boxes use). **From a checkout to the
package** (once Omarchy's repository has it): `omabox setup --remove` in the checkout first; if you
forget, the package's `omabox setup` finds the checkout's `~/.local/bin/omabox` and offers to remove
it. After an upgrade the bar runs the old widget until the shell restarts; its panel says so. Boxes
already up keep the Hyprland config they started with: `omabox reload` gives one
the new one, or `down` and `up`.

Upgrading from 0.x, step by step: [omabox.app/install](https://omabox.app/install).

## Remove

```bash
omabox down --all                          # every box
omabox setup --remove                      # guard and broker off, widget disabled, links removed
rm -rf ~/.config/omabox ~/.cache/omabox    # settings, and box HOMEs a crash left behind
rm -rf ~/.local/share/omabox               # saves, if you made any
```

`omabox setup --remove` turns the guard off (if on), the ai-jail broker off (its `~/.ai-jail` lines
are yours to delete), disables the bar widget if it is on, and removes the links `setup` made (only
those that still point at this omabox) and a per-user aquamarine build; it keeps your settings and
saves. Run it before you delete omabox: once it is gone there is no `guard off` left to
run. A guard left behind applies nothing in Claude Code and says so; in Codex its values stay, and
its hook says which lines of `~/.codex/config.toml` to delete.

Then delete the clone (its `build/` holds the aquamarine build). The guard leaves one backup of each
file it changed, its newest, next to it (`~/.claude/settings.json.bak-<time>`, and the same for
`~/.codex/config.toml` and `~/.codex/hooks.json`); `setup --remove` names them. The packages `install.sh` added stay; it printed them as `missing:` if there were any
(`labwc`, `passt` and the build tools are the likely ones): `sudo pacman -Rns` those you do not use
(or your distribution's own remove command).
