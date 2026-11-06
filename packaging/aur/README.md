# omabox on the AUR (draft)

`omabox/` is the recipe for an AUR package `omabox`, for Arch Linux with or without Omarchy. It
needs only Arch's own repositories. Omarchy's repository has its own package (omacom/omarchy-pkgs),
which depends on Omarchy's packages; this one does not.

**Publishing is the maintainer's.** Nothing here is pushed to the AUR by a script, CI or an agent:
the maintainer reviews the recipe and pushes it to `ssh://aur@aur.archlinux.org/omabox.git` himself.

## What differs from Omarchy's package

- Same build: the release tarball (`omabox-X.Y.Z.tar.gz`, checked against the release's
  `SHA256SUMS`), `cargo fetch --locked` in `prepare()`, `--frozen` build and tests, the C tools with
  makepkg's flags, the tree read-only in `/usr/lib/omabox` and `/usr/bin/omabox` a link to it.
- Dependencies: what a plain box runs. `quickshell` (only a shell in a box needs it), `gtk3` and
  `xdg-terminal-exec` (Omarchy has both) are optional; `foot` is suggested as a plain box's terminal.
- `omabox.install` names the shell restart for Omarchy only.

Each user still runs `omabox setup` after installing (and `omabox setup --remove` before removing):
a package cannot link into anyone's HOME.

## Which version

Plain boxes (`up --desktop plain`, chosen by themselves where Omarchy is not installed), install and
doctor without Omarchy, and the aquamarine build for Hyprland 0.57's soname arrive in **1.3.0**.
1.2.2 builds and installs from this recipe, but on Arch without Omarchy its boxes do not start.
Publish from 1.3.0 on.

## Updating

1. `pkgver=` to the new version, `pkgrel=1`; `sha256sums=` to the tarball's line in the release's
   `SHA256SUMS` (or `updpkgsums`).
2. If the release adds a tool under `tools/`, add it to `_tools`; if it adds a dependency, add it
   where install.sh's `PKGS_arch` and doctor's list put it (needed to run a box: `depends`; one
   command or an Omarchy box: `optdepends`).
3. `makepkg --printsrcinfo > .SRCINFO`.
4. Build in a clean chroot (`extra-x86_64-build`, or `makechrootpkg -c`), run `namcap` on the
   PKGBUILD and the package, install it, and as a user run `omabox setup`, `omabox doctor` and a box
   (`test/distros.sh --boxes docker.io/library/archlinux:latest` does the same from a checkout).
5. The maintainer commits `PKGBUILD`, `omabox.install` and `.SRCINFO` to the AUR repository and
   pushes.

namcap is silent on the PKGBUILD. On the package its expected warnings are those of Omarchy's
package, dependencies on commands omabox runs that namcap cannot see ("may not be needed") and the
QML modules of the bar widget (`qs.*`, the shell's own), plus two that follow from the optional
dependencies here: the widget's `Quickshell` and `QtQuick` modules (it loads only in Omarchy's bar)
and `python` (one command each).
