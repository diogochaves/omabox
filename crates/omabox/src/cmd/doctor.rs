//! `omabox doctor [--json] [--smoke]` (#320, finding 612): whether this machine can run boxes, and
//! what to install when it cannot, without changing anything. Each thing a box needs is looked at
//! where a box looks for it (a box's start runs with PATH=/usr/bin), with its version where a version
//! matters; each that fails or warns carries a line that fixes it for this distribution's family
//! (/etc/os-release's ID, then ID_LIKE). Read only: nothing is installed, built, written or asked
//! for, no sudo, nothing reaches the desktop; what it runs is a program's --version, `ldd`, `unshare
//! -U true` and a bwrap that runs `true` (both in namespaces of their own, as `up` does to see the
//! same), pkg-config --exists, `systemctl --user show-environment`, and pacman -Q for Omarchy's
//! version. --smoke starts a headless box, takes a shot (kept, its path said) and takes the box down.
//! The checks reuse up's and setup's own (aq_resolve, render_node, unshare_ok, aq_note_text). Exit
//! 1 when a check (or the smoke box) fails, else 0, warnings included.

use crate::cat;
use crate::exit::{Ctx, Exit, Res, muted, out};
use crate::gpu::at;
use crate::shell;
use serde_json::{Map, Value};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut as_json, mut smoke) = (false, false);
    for a in args {
        match a.as_slice() {
            b"--json" => as_json = true,
            b"--smoke" => smoke = true,
            b"-h" | b"--help" => return crate::help::usage("doctor"),
            _ => return Err(crate::args::unknown_opt(ctx, "doctor", a)),
        }
    }
    ctx.parsed();
    let os = OsRelease::read(b"/");
    let checks = gather(ctx, &os);
    let failed = checks.iter().filter(|c| c.st == St::Fail).count();
    // A box can start only when nothing failed: --smoke after a failure would only say it again.
    let sm = if as_json {
        let sm = (smoke && failed == 0).then(|| smoke_box(true));
        out(cat!(json_text(&os, &checks, smoke, sm.as_ref()), "\n"))?;
        sm
    } else {
        text(&os, &checks, smoke)?;
        (smoke && failed == 0).then(|| smoke_box(false))
    };
    if failed > 0 || sm.as_ref().is_some_and(|s| !s.ok) {
        return Err(Exit(1));
    }
    Ok(())
}

// --- distributions -------------------------------------------------------------------------------

/// A family of distributions that name packages alike, and how each installs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Arch,
    Debian,
    Fedora,
    Suse,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::Arch => "arch",
            Family::Debian => "debian",
            Family::Fedora => "fedora",
            Family::Suse => "suse",
        }
    }

    fn install(self) -> &'static str {
        match self {
            Family::Arch => "sudo pacman -S --needed",
            Family::Debian => "sudo apt install",
            Family::Fedora => "sudo dnf install",
            Family::Suse => "sudo zypper install",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// One word of ID or ID_LIKE as a family.
fn family_of(w: &str) -> Option<Family> {
    match w {
        "arch" => Some(Family::Arch),
        "debian" | "ubuntu" => Some(Family::Debian),
        "fedora" | "rhel" | "centos" => Some(Family::Fedora),
        "suse" | "sles" => Some(Family::Suse),
        _ if w.starts_with("opensuse") => Some(Family::Suse),
        _ => None,
    }
}

/// The family of a distribution: its ID, else the first of ID_LIKE's words that is one (Omarchy:
/// ID=omarchy, ID_LIKE=arch).
pub fn family(id: &str, like: &str) -> Option<Family> {
    std::iter::once(id)
        .chain(like.split_ascii_whitespace())
        .find_map(family_of)
}

/// /etc/os-release (else /usr/lib/os-release), as much as doctor reads of it.
#[derive(Debug, Default)]
pub struct OsRelease {
    pub id: String,
    pub like: String,
    pub pretty: String,
}

impl OsRelease {
    /// Read under ROOT ("/", or a unit test's tree).
    pub fn read(root: &[u8]) -> OsRelease {
        let text = std::fs::read(shell::path(&at(root, b"/etc/os-release")))
            .or_else(|_| std::fs::read(shell::path(&at(root, b"/usr/lib/os-release"))))
            .unwrap_or_default();
        OsRelease::parse(&String::from_utf8_lossy(&text))
    }

    /// KEY=VALUE lines, a value in "" or '' or bare.
    pub fn parse(text: &str) -> OsRelease {
        let mut o = OsRelease::default();
        for l in text.lines() {
            let Some((k, v)) = l.trim().split_once('=') else {
                continue;
            };
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v)
                .to_string();
            match k {
                "ID" => o.id = v,
                "ID_LIKE" => o.like = v,
                "PRETTY_NAME" => o.pretty = v,
                _ => {}
            }
        }
        o
    }

    pub fn family(&self) -> Option<Family> {
        family(&self.id, &self.like)
    }
}

/// What a fix may install, by the program (or build) it is for: its packages on Arch, Debian and
/// Ubuntu, Fedora, openSUSE, in Family's order; "" where none is known to package it. Arch's are the
/// ones install.sh installs and this machine runs; the others are written from those
/// distributions' package lists and only checked by the unit tests (#320).
const PKGS: &[(&str, [&str; 4])] = &[
    ("Hyprland", ["hyprland"; 4]),
    ("labwc", ["labwc"; 4]),
    ("quickshell", ["quickshell", "", "", ""]),
    ("bwrap", ["bubblewrap"; 4]),
    ("pasta", ["passt"; 4]),
    ("unshare", ["util-linux"; 4]),
    ("jq", ["jq"; 4]),
    ("grim", ["grim"; 4]),
    (
        "dbus-daemon",
        ["dbus", "dbus-daemon", "dbus-daemon", "dbus-1"],
    ),
    (
        "dbus-update-activation-environment",
        ["dbus", "dbus-bin", "dbus-tools", "dbus-1"],
    ),
    ("gnome-keyring-daemon", ["gnome-keyring"; 4]),
    ("python3", ["python", "python3", "python3", "python3"]),
    ("wlr-randr", ["wlr-randr"; 4]),
    ("ip", ["iproute2", "iproute2", "iproute", "iproute2"]),
    ("nft", ["nftables"; 4]),
    // tools/ (C, wayland-scanner, pkg-config's modules: the Makefiles')
    (
        "tools-build",
        [
            "base-devel pkgconf wayland libxkbcommon libpng pixman at-spi2-core glib2",
            "build-essential pkg-config libwayland-dev libxkbcommon-dev libpng-dev libpixman-1-dev libatspi2.0-dev libglib2.0-dev",
            "gcc make pkgconf-pkg-config wayland-devel libxkbcommon-devel libpng-devel pixman-devel at-spi2-core-devel glib2-devel",
            "gcc make pkg-config wayland-devel libxkbcommon-devel libpng16-devel pixman-devel at-spi2-core-devel glib2-devel",
        ],
    ),
    // setup --aquamarine (the programs it checks for)
    (
        "aquamarine-build",
        [
            "git cmake ninja hyprwayland-scanner base-devel",
            "git cmake ninja-build hyprwayland-scanner build-essential",
            "git cmake ninja-build hyprwayland-scanner gcc-c++",
            "git cmake ninja hyprwayland-scanner gcc-c++",
        ],
    ),
];

/// Where to get what no package of a family has.
fn source_of(need: &str) -> &'static str {
    match need {
        "quickshell" => "https://quickshell.org",
        _ => "its project's site",
    }
}

/// The lines that install NEEDS on FAMILY: one install command with every package, then one line
/// for each need that family has no package for. With no family known, Arch's names, said so.
pub fn install_lines(fam: Option<Family>, needs: &[&str]) -> Vec<String> {
    let mut pkgs: Vec<&str> = Vec::new();
    let mut none: Vec<&str> = Vec::new();
    for n in needs {
        let Some((_, by)) = PKGS.iter().find(|(k, _)| k == n) else {
            continue;
        };
        let p = by[fam.unwrap_or(Family::Arch).index()];
        if p.is_empty() {
            none.push(n);
        }
        for w in p.split_ascii_whitespace() {
            if !pkgs.contains(&w) {
                pkgs.push(w);
            }
        }
    }
    let mut v = Vec::new();
    if !pkgs.is_empty() {
        v.push(match fam {
            Some(f) => format!("{} {}", f.install(), pkgs.join(" ")),
            None => format!(
                "install {} (Arch's package names: this distribution's may differ)",
                pkgs.join(" ")
            ),
        });
    }
    for n in none {
        v.push(format!(
            "{n}: no {} package known to omabox: build it ({})",
            fam.map_or("", Family::name),
            source_of(n)
        ));
    }
    v
}

/// install_lines for this machine's family.
pub fn install_here(needs: &[&str]) -> Vec<String> {
    install_lines(OsRelease::read(b"/").family(), needs)
}

// --- checks --------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Ok,
    Warn,
    Fail,
}

impl St {
    fn word(self) -> &'static str {
        match self {
            St::Ok => "ok",
            St::Warn => "warn",
            St::Fail => "fail",
        }
    }
}

/// One check: what it looked at, how it went, what was found, and what fixes it.
#[derive(Debug)]
pub struct Check {
    pub id: &'static str,
    pub st: St,
    pub found: String,
    pub fix: Vec<String>,
}

fn check(id: &'static str, st: St, found: impl Into<String>, fix: Vec<String>) -> Check {
    Check {
        id,
        st,
        found: found.into(),
        fix,
    }
}

fn s(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

/// Where a box's start looks for the programs it runs (up_box's LAUNCH_PATH, #364).
fn launch_dir() -> String {
    s(crate::cmd::up_box::LAUNCH_PATH)
}

/// NAME in the launch dir under ROOT, when it is an executable file there.
fn in_launch(root: &[u8], name: &str) -> Option<Vec<u8>> {
    let p = at(root, format!("{}/{name}", launch_dir()).as_bytes());
    (crate::exec::executable(&p) && shell::path(&p).is_file()).then_some(p)
}

/// The first executable file NAME in PATH's folders (`command -v`).
fn on_path(path: &[u8], name: &[u8]) -> Option<Vec<u8>> {
    path.split(|&c| c == b':')
        .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", name))
        .find(|p| crate::exec::executable(p) && shell::path(p).is_file())
}

/// What a missing program's line says: not there, or on the caller's PATH only.
fn missing_from_launch(name: &str) -> String {
    match on_path(&shell::env_or_empty("PATH"), name.as_bytes()) {
        Some(p) => format!(
            "{name} is on your PATH ({}) but not in {}, where a box's start looks",
            s(&p),
            launch_dir()
        ),
        None => format!("no {name} in {}", launch_dir()),
    }
}

/// PROG ARGS with stdin closed: what it wrote, stdout and stderr together (64 KiB at most); None
/// when it could not start.
fn said(prog: &[u8], args: &[&str]) -> Option<Vec<u8>> {
    let mut c = crate::sig::command(OsStr::from_bytes(prog));
    c.args(args).stdin(Stdio::null());
    let (r, w) = crate::exec::pipe().ok()?;
    let w2 = w.try_clone().ok()?;
    c.stdout(Stdio::from(w)).stderr(Stdio::from(w2));
    crate::sig::run_child(c, false, |_| crate::exec::read_all(r, Some(65536)))
        .ok()
        .map(|(o, _)| o)
}

/// The first line of what PROG --version says.
fn version_line(prog: &[u8]) -> String {
    let o = said(prog, &["--version"]).unwrap_or_default();
    s(o.split(|&c| c == b'\n')
        .find(|l| !l.is_empty())
        .unwrap_or_default())
}

/// The first X.Y[.Z] in a line.
pub fn version_in(l: &str) -> Option<String> {
    shell::re_group(r"([0-9]+\.[0-9]+(\.[0-9]+)?)", l.as_bytes(), 1).map(|v| s(&v))
}

/// V is MIN or later.
fn at_least(v: &str, min: &str) -> bool {
    !crate::install::version_cmp(v.as_bytes(), min.as_bytes()).is_lt()
}

/// A program a box runs, in the launch dir: its version line, and a minimum where an older one is
/// known not to work (MIN, and what it lacks).
fn prog_check(
    root: &[u8],
    fam: Option<Family>,
    id: &'static str,
    name: &str,
    min: Option<(&str, &str)>,
) -> Check {
    let Some(p) = in_launch(root, name) else {
        return check(
            id,
            St::Fail,
            missing_from_launch(name),
            install_lines(fam, &[name]),
        );
    };
    let line = version_line(&p);
    let shown = if line.is_empty() {
        format!("{} (no version said)", s(&p))
    } else {
        line.clone()
    };
    if let Some((min, why)) = min {
        match version_in(&line) {
            Some(v) if at_least(&v, min) => {}
            v => {
                return check(
                    id,
                    St::Fail,
                    format!(
                        "{shown}: omabox needs {min} or later ({why}){}",
                        if v.is_none() { "; no version read" } else { "" }
                    ),
                    install_lines(fam, &[name]),
                );
            }
        }
    }
    check(id, St::Ok, shown, Vec::new())
}

/// Hyprland 0.56 or later: its config is Lua (finding 8), which a box's config is.
fn hyprland_check(root: &[u8], fam: Option<Family>) -> Check {
    let mut c = prog_check(
        root,
        fam,
        "hyprland",
        "Hyprland",
        Some(("0.56.0", "a Lua config; older ones read hyprlang")),
    );
    if c.st == St::Ok {
        c.found = format!(
            "{} (Lua config)",
            c.found
                .split(" built from")
                .next()
                .unwrap_or_default()
                .trim_end()
        );
    }
    c
}

/// grim, with -T (a window by its toplevel id: shot --window, finding 81; grim 1.5).
fn grim_check(root: &[u8], fam: Option<Family>) -> Check {
    let Some(p) = in_launch(root, "grim") else {
        return check(
            "grim",
            St::Fail,
            missing_from_launch("grim"),
            install_lines(fam, &["grim"]),
        );
    };
    let h = said(&p, &["-h"]).unwrap_or_default();
    let has_t = h.split(|&c| c == b'\n').any(|l| {
        let l = s(l);
        l.trim_start().starts_with("-T ")
    });
    if has_t {
        check("grim", St::Ok, format!("{} (has -T)", s(&p)), Vec::new())
    } else {
        check(
            "grim",
            St::Warn,
            format!(
                "{}: no -T, so shot --window cannot capture a window (grim 1.5 or later)",
                s(&p)
            ),
            install_lines(fam, &["grim"]),
        )
    }
}

/// Programs every box runs, without a version that matters (share/'s session and shell).
const BOX_PROGS: &[&str] = &[
    "jq",
    "dbus-daemon",
    "dbus-update-activation-environment",
    "gnome-keyring-daemon",
];

/// Programs only some boxes run, and which.
const SOME_PROGS: &[(&str, &str)] = &[
    ("wlr-randr", "NVIDIA boxes size their screen with it"),
    ("ip", "up --addr"),
    ("nft", "the protect-ports setting"),
    (
        "python3",
        "a box's journal socket, the agent guard's Codex check",
    ),
];

fn box_progs_check(root: &[u8], fam: Option<Family>) -> Check {
    let missing: Vec<&str> = BOX_PROGS
        .iter()
        .copied()
        .filter(|n| in_launch(root, n).is_none())
        .collect();
    if missing.is_empty() {
        return check(
            "box-programs",
            St::Ok,
            format!("{} in {}", BOX_PROGS.join(", "), launch_dir()),
            Vec::new(),
        );
    }
    check(
        "box-programs",
        St::Fail,
        missing
            .iter()
            .map(|n| missing_from_launch(n))
            .collect::<Vec<_>>()
            .join("; "),
        install_lines(fam, &missing),
    )
}

fn some_progs_check(root: &[u8], fam: Option<Family>) -> Check {
    let missing: Vec<(&str, &str)> = SOME_PROGS
        .iter()
        .copied()
        .filter(|(n, _)| in_launch(root, n).is_none())
        .collect();
    if missing.is_empty() {
        let names: Vec<&str> = SOME_PROGS.iter().map(|(n, _)| *n).collect();
        return check(
            "some-programs",
            St::Ok,
            format!("{} in {}", names.join(", "), launch_dir()),
            Vec::new(),
        );
    }
    let names: Vec<&str> = missing.iter().map(|(n, _)| *n).collect();
    check(
        "some-programs",
        St::Warn,
        missing
            .iter()
            .map(|(n, why)| format!("no {n} ({why})"))
            .collect::<Vec<_>>()
            .join("; "),
        install_lines(fam, &names),
    )
}

/// Omarchy, whose desktop a box runs (plain Hyprland boxes would be #247), and the look a box
/// starts from, which Omarchy makes at the first login (finding 130; up refuses without it).
fn omarchy_check(root: &[u8], version: &str) -> Check {
    let boot = at(root, b"/usr/share/omarchy/default/hypr/bootstrap.lua");
    if !shell::path(&boot).is_file() {
        return check(
            "omarchy",
            St::Fail,
            format!(
                "no Omarchy ({} is missing): a box runs Omarchy's desktop",
                s(&boot)
            ),
            vec!["install Omarchy 4 (https://omarchy.org); boxes without Omarchy are not there yet (issue #247)".into()],
        );
    }
    let v = if version.is_empty() {
        String::new()
    } else {
        format!(" {version}")
    };
    check(
        "omarchy",
        St::Ok,
        format!("Omarchy{v} (/usr/share/omarchy)"),
        Vec::new(),
    )
}

fn theme_check(home: &[u8]) -> Check {
    let t = cat!(home, "/.local/state/omarchy/current/theme");
    if shell::path(&t).is_dir() {
        return check(
            "theme",
            St::Ok,
            "your Omarchy theme, which a box starts from",
            Vec::new(),
        );
    }
    check(
        "theme",
        St::Fail,
        format!(
            "Omarchy has not set a theme for you yet ({}), and a box starts from your Omarchy look",
            s(&t)
        ),
        vec![
            "log into Omarchy once, or: omarchy-theme-set \"Tokyo Night\" (or up --theme NAME)"
                .into(),
        ],
    )
}

/// aquamarine (aq_resolve's, as `up` picks it): with or without #415, and omabox's own build.
fn aq_check(aq: Option<&crate::install::Aq>, why_none: &str) -> Check {
    let Some(aq) = aq else {
        return check(
            "aquamarine",
            St::Fail,
            format!("cannot tell which aquamarine a box would use: {why_none}"),
            vec!["omabox setup --aquamarine builds the one the installed Hyprland links (Hyprland first, if it is missing)".into()],
        );
    };
    let found = s(&aq.desc());
    match crate::cmd::setup::aq_note_text(aq) {
        None => check("aquamarine", St::Ok, found, Vec::new()),
        Some(note) => check("aquamarine", St::Warn, found, vec![s(&note)]),
    }
}

/// omabox's C tools (tools/), the ones every box needs (check_install's) and the ones only some
/// commands do.
const TOOLS_EVERY: &[&str] = &["keyboard", "pointer", "still", "events"];
const TOOLS_SOME: &[(&str, &str)] = &[
    ("wlfd", "interactive boxes"),
    ("peek", "omabox peek"),
    ("burst", "shot --burst"),
    ("tree", "omabox tree"),
    ("relay", "agents inside ai-jail"),
];

/// pkg-config modules and programs tools/ builds with.
const TOOLS_MODULES: &[&str] = &[
    "wayland-client",
    "xkbcommon",
    "pixman-1",
    "libpng",
    "atspi-2",
    "gio-2.0",
];

fn tools_check(co: &[u8], fam: Option<Family>, tool: impl Fn(&str) -> Vec<u8>) -> Check {
    let built = |n: &str| crate::exec::executable(&tool(n));
    let every: Vec<&str> = TOOLS_EVERY.iter().copied().filter(|n| !built(n)).collect();
    let some: Vec<(&str, &str)> = TOOLS_SOME
        .iter()
        .copied()
        .filter(|(n, _)| !built(n))
        .collect();
    if every.is_empty() && some.is_empty() {
        return check(
            "tools",
            St::Ok,
            format!("all built ({}/tools)", s(co)),
            Vec::new(),
        );
    }
    let mut found = Vec::new();
    if !every.is_empty() {
        found.push(format!("not built: {} (every box)", every.join(", ")));
    }
    for (n, why) in &some {
        found.push(format!("not built: {n} ({why})"));
    }
    let names: Vec<&str> = every
        .iter()
        .copied()
        .chain(some.iter().map(|(n, _)| *n))
        .collect();
    let checkout = shell::path(&cat!(co, "/.git")).exists();
    let mut fix = Vec::new();
    if checkout {
        fix.push(format!(
            "for t in {}; do make -C {}/tools/$t; done",
            names.join(" "),
            s(co)
        ));
        let mut m = crate::sig::command("pkg-config");
        m.arg("--exists")
            .args(TOOLS_MODULES)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let modules = crate::sig::run_status(m).is_ok_and(|rc| rc == 0);
        let progs = ["cc", "make", "wayland-scanner"]
            .iter()
            .all(|p| crate::exec::executable_on_path(p.as_bytes()));
        if !modules || !progs {
            fix.extend(install_lines(fam, &["tools-build"]));
        }
    } else {
        fix.push(format!(
            "this omabox ({}) is an install whose tools are missing: reinstall it",
            s(co)
        ));
    }
    check(
        "tools",
        if every.is_empty() { St::Warn } else { St::Fail },
        found.join("; "),
        fix,
    )
}

/// Why unprivileged user namespaces are off, from the sysctls under ROOT that turn them off, each
/// with its fix: Debian's switch, a zero limit, Ubuntu's AppArmor restriction.
pub fn userns_why(root: &[u8]) -> Vec<(String, String)> {
    let read = |p: &str| {
        std::fs::read(shell::path(&at(root, p.as_bytes())))
            .map(|v| s(&shell::strip_nl(&v)))
            .ok()
    };
    let mut v = Vec::new();
    if read("/proc/sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
        v.push((
            "kernel.unprivileged_userns_clone is 0".into(),
            "sudo sysctl -w kernel.unprivileged_userns_clone=1 (and the same line in /etc/sysctl.d/ to keep it)".into(),
        ));
    }
    if read("/proc/sys/user/max_user_namespaces").as_deref() == Some("0") {
        v.push((
            "user.max_user_namespaces is 0".into(),
            "sudo sysctl -w user.max_user_namespaces=28633 (and the same line in /etc/sysctl.d/ to keep it)".into(),
        ));
    }
    if read("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1") {
        v.push((
            "AppArmor restricts unprivileged user namespaces (kernel.apparmor_restrict_unprivileged_userns is 1)".into(),
            "an AppArmor profile that lets /usr/bin/bwrap make them (Ubuntu ships one: /usr/share/apparmor/extra-profiles/bwrap-userns-restrict), or sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0".into(),
        ));
    }
    v
}

/// What the bwrap probe sees: /usr and the loader's dirs, read-only; no /dev, /run or /proc (none of
/// S1's seat paths), and it runs only `true`.
const BWRAP_PROBE: &[&str] = &[
    "--unshare-user",
    "--ro-bind",
    "/usr",
    "/usr",
    "--ro-bind-try",
    "/lib",
    "/lib",
    "--ro-bind-try",
    "/lib64",
    "/lib64",
    "--ro-bind-try",
    "/bin",
    "/bin",
    "/usr/bin/true",
];

/// User namespaces as a box makes them: `unshare -U true` (up's own test), then bwrap in one with a
/// mount namespace of its own (BWRAP_PROBE), as a box's.
fn userns_check(root: &[u8]) -> Check {
    let unshare = crate::cmd::up::unshare_ok();
    let bwrap = in_launch(root, "bwrap").map(|b| run_capture(&b, BWRAP_PROBE));
    let bwrap_err = match &bwrap {
        Some(Some((0, _))) | None => None,
        Some(Some((rc, o))) => Some(match shell::strip_nl(o) {
            o if o.is_empty() => format!("bwrap exited {rc}"),
            o => s(&o),
        }),
        Some(None) => Some("bwrap did not start".into()),
    };
    if unshare && bwrap_err.is_none() {
        return check(
            "userns",
            St::Ok,
            if bwrap.is_some() {
                "unprivileged user namespaces: unshare and bwrap make them"
            } else {
                "unprivileged user namespaces: unshare makes them"
            },
            Vec::new(),
        );
    }
    let why = userns_why(root);
    let mut found = if unshare {
        format!(
            "bwrap cannot make a box's namespaces: {}",
            bwrap_err.unwrap_or_default()
        )
    } else {
        "this process cannot make a user namespace (unshare -U true failed)".into()
    };
    for (w, _) in &why {
        found.push_str(&format!("; {w}"));
    }
    let mut fix: Vec<String> = why.into_iter().map(|(_, f)| f).collect();
    if fix.is_empty() {
        fix.push("no sysctl here turns them off: a sandbox around this shell (an agent's, ai-jail) may refuse them; inside ai-jail, omabox goes through the broker (omabox broker on, outside it)".into());
    }
    check("userns", St::Fail, found, fix)
}

/// no_new_privs on this process: a box behind pasta cannot start from it (finding 89), so a
/// connected box falls back to no network and an isolated one is refused.
fn nnp_check(nnp: bool) -> Check {
    if !nnp {
        return check(
            "no-new-privs",
            St::Ok,
            "this process may start a box behind pasta",
            Vec::new(),
        );
    }
    check(
        "no-new-privs",
        St::Warn,
        "this process has no_new_privs (an agent's sandbox, a systemd unit with NoNewPrivileges=): a box started from it has no network, and --net isolated is refused",
        vec!["start boxes from a shell without it (your terminal), or take the box's network as it comes".into()],
    )
}

fn tun_check(tun: bool) -> Check {
    if tun {
        return check(
            "tun",
            St::Ok,
            "/dev/net/tun, a connected box's network",
            Vec::new(),
        );
    }
    check(
        "tun",
        St::Fail,
        "no /dev/net/tun: a box's network (pasta) needs it; --net isolated does without, and has no internet",
        vec!["sudo modprobe tun".into()],
    )
}

/// Every render node: its driver and slot, whether a box may open it, whether it renders only in
/// software; and the one a headless box would use (render_node's choice: OMABOX_RENDER_NODE, else
/// the gpu setting, else the first usable). Under ROOT; AQ_FIXED: the boxes' aquamarine has #415.
fn render_check(ctx: &Ctx, root: &[u8], forced: Vec<u8>, aq_fixed: bool) -> Check {
    use crate::render;
    let all = render::render_all_in(root);
    if all.is_empty() {
        return check(
            "render",
            St::Fail,
            format!(
                "no GPU render node ({}/renderD*): a box renders on the GPU",
                render::DRI
            ),
            vec![format!(
                "a GPU driver that gives one (amdgpu, i915/xe, nvidia with nvidia-drm); {}",
                s(crate::ready::VIRGL_HINT)
            )],
        );
    }
    let mut parts = Vec::new();
    let mut usable_hw = 0;
    for n in &all {
        let node = s(n.rsplit(|&c| c == b'/').next().unwrap_or_default());
        let drv = s(&crate::gpu::render_driver_in(n, root));
        let mut p = format!("{node} {}", if drv.is_empty() { "?" } else { &drv });
        let rw = render::render_rw_in(n, root);
        let soft = render::software_only_in(n, root);
        if !rw {
            p.push_str(" (not readable and writable by you)");
        } else if soft {
            p.push_str(" (renders in software only)");
        } else {
            usable_hw += 1;
        }
        parts.push(p);
    }
    let chosen = muted(|| render::render_node_in(ctx, forced, root)).ok();
    let Some(chosen) = chosen else {
        return check(
            "render",
            St::Fail,
            format!("{}: none a box may use", parts.join(", ")),
            vec![
                "add yourself to the render group (sudo usermod -aG render $USER), then log in again"
                    .into(),
            ],
        );
    };
    let cn = s(chosen
        .as_bytes()
        .rsplit(|&c| c == b'/')
        .next()
        .unwrap_or_default());
    // (render_node's own note, held above: the gpu setting names a GPU not usable now)
    let fell = if render::render_fallback_in(&chosen, root) {
        ", not the GPU the gpu setting names (not usable now: omabox config gpu)"
    } else {
        ""
    };
    let found = format!("{}; a headless box uses {cn}{fell}", parts.join(", "));
    if render::software_only_in(chosen.as_bytes(), root) {
        let mut fix = vec![s(crate::ready::VIRGL_HINT)];
        if usable_hw > 0 {
            fix.push(
                "or a GPU that renders: omabox config gpu (lists them), or OMABOX_RENDER_NODE"
                    .into(),
            );
        }
        return check(
            "render",
            if usable_hw > 0 { St::Warn } else { St::Fail },
            format!("{found}, which renders in software only: a box there dies while starting"),
            fix,
        );
    }
    if chosen.driver_in(root) == b"nvidia" && !aq_fixed {
        return check(
            "render",
            St::Warn,
            format!(
                "{found}, on NVIDIA, where a headless box needs aquamarine's fix (hyprwm/aquamarine#415)"
            ),
            vec![
                format!("omabox setup --aquamarine ({})", crate::install::AQ_HOWTO),
                "or render on another GPU: omabox config gpu amd, intel or a slot".into(),
            ],
        );
    }
    check("render", St::Ok, found, Vec::new())
}

/// The longest socket path the host reaches in a box by name (sun_path holds 107 bytes and its
/// NUL; finding 7): a box's Wayland socket (`omabox env`, a 40-byte name: select_box's most), and
/// the broker's.
pub const SUN_MAX: usize = 107;

pub fn socket_check(boxes: &[u8]) -> Check {
    let name = "x".repeat(40);
    let wl = cat!(boxes, "/", name, "/run/wayland-1");
    let broker = crate::front::broker_sock(boxes);
    let n = wl.len().max(broker.len());
    if n <= SUN_MAX {
        return check(
            "socket-path",
            St::Ok,
            format!(
                "{n} bytes at most of {SUN_MAX} (a box's sockets under {})",
                s(boxes)
            ),
            Vec::new(),
        );
    }
    check(
        "socket-path",
        St::Fail,
        format!(
            "{n} bytes, past the {SUN_MAX} a socket path may have: a box's sockets under {} (XDG_RUNTIME_DIR) are too deep",
            s(boxes)
        ),
        vec![format!(
            "run omabox with a shorter XDG_RUNTIME_DIR (its own, /run/user/{}, as systemd-logind makes it)",
            // SAFETY: getuid has no failure.
            unsafe { libc::getuid() }
        )],
    )
}

/// What this omabox is: a checkout, a git worktree of one (whose omabox is run as its bin/omabox:
/// setup there would point yours at it, #182), or an install under /usr (in every PATH).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Checkout,
    Worktree,
    System,
}

fn kind_of(root: &[u8]) -> Kind {
    if root.starts_with(b"/usr/") {
        Kind::System
    } else if shell::path(&cat!(root, "/.git")).is_file() {
        Kind::Worktree
    } else {
        Kind::Checkout
    }
}

/// The omabox a PATH finds: this one, another, or none. ME: this omabox, resolved.
fn path_check(id: &'static str, whose: &str, path: Option<&[u8]>, me: &[u8], kind: Kind) -> Check {
    let Some(path) = path else {
        return check(
            id,
            St::Warn,
            format!("could not read {whose} PATH"),
            vec!["systemctl --user show-environment shows it, when your session runs a systemd user manager".into()],
        );
    };
    match on_path(path, b"omabox") {
        Some(f) => {
            let real = crate::repo::readlink_f(&f).unwrap_or_else(|| f.clone());
            if real == me {
                check(
                    id,
                    St::Ok,
                    format!("{whose} PATH finds this omabox ({})", s(&f)),
                    Vec::new(),
                )
            } else {
                let fix = if kind == Kind::Worktree {
                    "this omabox is a git worktree's: run it as its bin/omabox (omabox setup here would point your omabox at the worktree)"
                } else {
                    "omabox setup, run from the omabox you want, links it as ~/.local/bin/omabox"
                };
                check(
                    id,
                    St::Warn,
                    format!(
                        "{whose} PATH finds another omabox: {} ({}), not this one ({})",
                        s(&f),
                        s(&real),
                        s(me)
                    ),
                    vec![fix.into()],
                )
            }
        }
        None => check(
            id,
            St::Warn,
            format!("{whose} PATH finds no omabox: agents started from there cannot run it"),
            if kind == Kind::System {
                vec![format!("put {} in that PATH", launch_dir())]
            } else if id == "session-path" {
                vec!["omabox setup links ~/.local/bin/omabox; then add ~/.local/bin to the PATH your desktop session gets (Omarchy: ~/.config/uwsm/env), and log in again".into()]
            } else {
                vec!["omabox setup links ~/.local/bin/omabox; add ~/.local/bin to PATH".into()]
            },
        ),
    }
}

/// The desktop session's PATH, which what it starts (a launcher's agent, a terminal) inherits:
/// the systemd user manager's environment, read.
fn session_path() -> Option<Vec<u8>> {
    let mut c = crate::sig::command("systemctl");
    c.args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    match crate::sig::run_output(c) {
        Ok((0, o)) => o
            .split(|&c| c == b'\n')
            .find_map(|l| l.strip_prefix(b"PATH="))
            .map(<[u8]>::to_vec),
        _ => None,
    }
}

/// Every check, on this machine.
fn gather(ctx: &Ctx, os: &OsRelease) -> Vec<Check> {
    let fam = os.family();
    let root: &[u8] = b"/";
    let home = shell::env_or_empty("HOME");
    let mut v = vec![omarchy_check(root, &s(&crate::install::omarchy_version()))];
    if v[0].st == St::Ok {
        v.push(theme_check(&home));
    }
    v.push(hyprland_check(root, fam));
    // (`up` picks it so: the installed Hyprland's soname, then aq_resolve; their words held)
    let (aq, why) = muted(|| match crate::install::hypr_aq_soname(ctx) {
        Ok(so) => match crate::install::aq_resolve(ctx, &so) {
            Some(a) => (Some(a), String::new()),
            None => (
                None,
                format!("Hyprland links {}, which no aquamarine here has", s(&so)),
            ),
        },
        Err(_) => (
            None,
            "ldd cannot read which libaquamarine Hyprland links".into(),
        ),
    });
    let aq_fixed = aq.as_ref().is_some_and(|a| a.fixed);
    v.push(aq_check(aq.as_ref(), &why));
    v.push(prog_check(root, fam, "labwc", "labwc", None));
    v.push(prog_check(root, fam, "quickshell", "quickshell", None));
    v.push(prog_check(
        root,
        fam,
        "bwrap",
        "bwrap",
        Some((
            "0.11.0",
            "overlay mounts: a box's /etc, run's throwaway folders",
        )),
    ));
    v.push(prog_check(root, fam, "pasta", "pasta", None));
    v.push(prog_check(root, fam, "unshare", "unshare", None));
    v.push(grim_check(root, fam));
    v.push(box_progs_check(root, fam));
    v.push(some_progs_check(root, fam));
    v.push(tools_check(&crate::paths::root(), fam, crate::paths::tool));
    v.push(userns_check(root));
    v.push(nnp_check(crate::cmd::up::no_new_privs()));
    v.push(tun_check(crate::cmd::up::is_char("/dev/net/tun")));
    v.push(render_check(
        ctx,
        root,
        shell::env_or_empty("OMABOX_RENDER_NODE"),
        aq_fixed,
    ));
    v.push(socket_check(&ctx.boxes));
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| crate::repo::readlink_f(shell::bytes(&p)))
        .unwrap_or_default();
    let kind = kind_of(&crate::paths::root());
    v.push(path_check(
        "path",
        "your shell's",
        Some(&shell::env_or_empty("PATH")),
        &me,
        kind,
    ));
    v.push(path_check(
        "session-path",
        "your desktop session's",
        session_path().as_deref(),
        &me,
        kind,
    ));
    v
}

// --- the smoke box -------------------------------------------------------------------------------

/// What --smoke saw: the box up, its shot, the box down.
pub struct Smoke {
    pub ok: bool,
    pub name: String,
    pub shot: String,
    pub error: String,
}

/// A headless box of its own name started with this omabox, a shot of it, the box taken down
/// whatever happened. Its lines said as it goes (not with --json).
fn smoke_box(quiet: bool) -> Smoke {
    let exe = std::env::current_exe()
        .map(|p| shell::bytes(&p).to_vec())
        .unwrap_or_else(|_| cat!(crate::paths::root(), "/bin/omabox"));
    let name = format!("doctor-{}", std::process::id());
    let mut sm = Smoke {
        ok: false,
        name: name.clone(),
        shot: String::new(),
        error: String::new(),
    };
    if !quiet {
        let _ = out(format!("smoke: omabox up {name}\n"));
    }
    let last = |o: &[u8]| {
        let o = shell::strip_nl(o);
        s(o.rsplit(|&c| c == b'\n').next().unwrap_or_default())
    };
    let up = run_capture(&exe, &["up", &name]);
    match up {
        Some((0, _)) => match run_capture(&exe, &["shot", "-b", &name]) {
            Some((0, o)) => {
                sm.ok = true;
                sm.shot = s(&shell::strip_nl(&o));
            }
            Some((_, o)) => sm.error = format!("shot failed: {}", last(&o)),
            None => sm.error = "shot did not start".into(),
        },
        Some((_, o)) => sm.error = format!("up failed: {}", last(&o)),
        None => sm.error = "up did not start".into(),
    }
    let _ = run_capture(&exe, &["down", &name]);
    if !quiet {
        let _ = out(if sm.ok {
            format!(
                "smoke: a box came up, its shot is {}, and it is down again\n",
                sm.shot
            )
        } else {
            format!("smoke: {} (box {name} taken down)\n", sm.error)
        });
    }
    sm
}

/// PROG ARGS, stdin closed: its status and what it wrote, stdout and stderr together.
fn run_capture(prog: &[u8], args: &[&str]) -> Option<(i32, Vec<u8>)> {
    let mut c = crate::sig::command(OsStr::from_bytes(prog));
    c.args(args).stdin(Stdio::null());
    let (r, w) = crate::exec::pipe().ok()?;
    let w2 = w.try_clone().ok()?;
    c.stdout(Stdio::from(w)).stderr(Stdio::from(w2));
    crate::sig::run_child(c, false, |_| crate::exec::read_all(r, Some(1 << 20)))
        .ok()
        .map(|(o, (rc, _))| (rc, o))
}

// --- output --------------------------------------------------------------------------------------

fn os_name(os: &OsRelease) -> String {
    let name = if os.pretty.is_empty() {
        if os.id.is_empty() {
            "an unknown distribution"
        } else {
            &os.id
        }
    } else {
        &os.pretty
    };
    match os.family() {
        Some(f) => format!("{name}, {} family", f.name()),
        None => format!("{name}, a family omabox has no package names for: Arch's are given"),
    }
}

fn text(os: &OsRelease, checks: &[Check], smoke: bool) -> Res<()> {
    let mut o = format!(
        "omabox doctor ({}): read-only, nothing changed\n",
        os_name(os)
    );
    for c in checks {
        let st = if c.st == St::Fail {
            "FAIL"
        } else {
            c.st.word()
        };
        o.push_str(&format!("{st:<5} {:<14} {}\n", c.id, c.found));
        for f in &c.fix {
            o.push_str(&format!("{:<20} fix: {f}\n", ""));
        }
    }
    let fail = checks.iter().filter(|c| c.st == St::Fail).count();
    let warn = checks.iter().filter(|c| c.st == St::Warn).count();
    let w = |n: usize| if n == 1 { "warning" } else { "warnings" };
    o.push_str(&if fail > 0 {
        format!(
            "{fail} failed, {warn} {}: boxes cannot start here until the fixes above are made{}\n",
            w(warn),
            if smoke { " (no smoke box)" } else { "" }
        )
    } else if warn > 0 {
        format!(
            "{warn} {}: boxes can start here; the warnings say what will not work{}\n",
            w(warn),
            if smoke { "" } else { " (--smoke starts one)" }
        )
    } else {
        format!(
            "all {} checks pass: this machine can run boxes{}\n",
            checks.len(),
            if smoke { "" } else { " (--smoke starts one)" }
        )
    });
    out(o)
}

pub fn json_text(os: &OsRelease, checks: &[Check], smoke: bool, sm: Option<&Smoke>) -> String {
    let st = |v: &str| Value::String(v.to_string());
    let mut m = Map::new();
    m.insert(
        "ok".into(),
        Value::Bool(!checks.iter().any(|c| c.st == St::Fail) && sm.is_none_or(|s| s.ok)),
    );
    let mut d = Map::new();
    d.insert("id".into(), st(&os.id));
    d.insert("id_like".into(), st(&os.like));
    d.insert(
        "family".into(),
        os.family().map_or(Value::Null, |f| st(f.name())),
    );
    m.insert("os".into(), Value::Object(d));
    let cs = checks
        .iter()
        .map(|c| {
            let mut e = Map::new();
            e.insert("id".into(), st(c.id));
            e.insert("status".into(), st(c.st.word()));
            e.insert("found".into(), st(&c.found));
            e.insert(
                "fix".into(),
                Value::Array(c.fix.iter().map(|f| st(f)).collect()),
            );
            Value::Object(e)
        })
        .collect();
    m.insert("checks".into(), Value::Array(cs));
    let smoke_v = match sm {
        Some(sm) => {
            let mut e = Map::new();
            e.insert("ok".into(), Value::Bool(sm.ok));
            e.insert("box".into(), st(&sm.name));
            e.insert(
                "shot".into(),
                if sm.shot.is_empty() {
                    Value::Null
                } else {
                    st(&sm.shot)
                },
            );
            e.insert(
                "error".into(),
                if sm.error.is_empty() {
                    Value::Null
                } else {
                    st(&sm.error)
                },
            );
            Value::Object(e)
        }
        // (asked for, but a check failed first: not started)
        None if smoke => {
            let mut e = Map::new();
            e.insert("ok".into(), Value::Bool(false));
            e.insert("error".into(), st("not started: a check failed"));
            Value::Object(e)
        }
        None => Value::Null,
    };
    m.insert("smoke".into(), smoke_v);
    crate::json::compact(&Value::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("omabox-doctor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(root: &std::path::Path, rel: &str, body: &str) {
        let p = root.join(rel.trim_start_matches('/'));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
    }

    /// A stand-in program in ROOT/usr/bin that prints BODY (a shell script).
    fn prog(root: &std::path::Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        put(
            root,
            &format!("/usr/bin/{name}"),
            &format!("#!/bin/sh\n{body}\n"),
        );
        let p = root.join("usr/bin").join(name);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn rb(d: &std::path::Path) -> Vec<u8> {
        shell::bytes(d).to_vec()
    }

    #[test]
    fn os_release_and_families() {
        let o =
            OsRelease::parse("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\nPRETTY_NAME='Omarchy'\n");
        assert_eq!(
            (o.id.as_str(), o.like.as_str(), o.pretty.as_str()),
            ("omarchy", "arch", "Omarchy")
        );
        assert_eq!(o.family(), Some(Family::Arch));
        for (id, like, f) in [
            ("arch", "", Some(Family::Arch)),
            ("cachyos", "arch", Some(Family::Arch)),
            ("ubuntu", "debian", Some(Family::Debian)),
            ("linuxmint", "ubuntu debian", Some(Family::Debian)),
            ("pop", "ubuntu debian", Some(Family::Debian)),
            ("debian", "", Some(Family::Debian)),
            ("fedora", "", Some(Family::Fedora)),
            ("rocky", "rhel centos fedora", Some(Family::Fedora)),
            ("opensuse-tumbleweed", "opensuse suse", Some(Family::Suse)),
            ("sles", "", Some(Family::Suse)),
            ("nixos", "", None),
            ("", "", None),
        ] {
            assert_eq!(family(id, like), f, "{id} {like}");
        }
        // ID first, then ID_LIKE in order
        assert_eq!(family("ubuntu", "arch"), Some(Family::Debian));
        let d = tree("osr");
        assert_eq!(
            OsRelease::read(&rb(&d)).family(),
            None,
            "no file: no family"
        );
        put(&d, "/usr/lib/os-release", "ID=fedora\n");
        assert_eq!(
            OsRelease::read(&rb(&d)).family(),
            Some(Family::Fedora),
            "/usr/lib's"
        );
        put(&d, "/etc/os-release", "ID=\"debian\"\n");
        assert_eq!(
            OsRelease::read(&rb(&d)).family(),
            Some(Family::Debian),
            "/etc's first"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The fix lines per family (only Arch's are run on this machine; the rest are the table's).
    #[test]
    fn install_lines_per_family() {
        let needs = ["labwc", "bwrap", "pasta", "dbus-daemon", "ip"];
        assert_eq!(
            install_lines(Some(Family::Arch), &needs),
            ["sudo pacman -S --needed labwc bubblewrap passt dbus iproute2"]
        );
        assert_eq!(
            install_lines(Some(Family::Debian), &needs),
            ["sudo apt install labwc bubblewrap passt dbus-daemon iproute2"]
        );
        assert_eq!(
            install_lines(Some(Family::Fedora), &needs),
            ["sudo dnf install labwc bubblewrap passt dbus-daemon iproute"]
        );
        assert_eq!(
            install_lines(Some(Family::Suse), &needs),
            ["sudo zypper install labwc bubblewrap passt dbus-1 iproute2"]
        );
        assert_eq!(
            install_lines(None, &["labwc"]),
            ["install labwc (Arch's package names: this distribution's may differ)"]
        );
        // a package once (dbus for both of Arch's dbus programs), several words split
        assert_eq!(
            install_lines(
                Some(Family::Arch),
                &["dbus-daemon", "dbus-update-activation-environment"]
            ),
            ["sudo pacman -S --needed dbus"]
        );
        assert_eq!(
            install_lines(Some(Family::Arch), &["aquamarine-build"]),
            ["sudo pacman -S --needed git cmake ninja hyprwayland-scanner base-devel"],
            "setup --aquamarine's line, as it was"
        );
        // none packaged there: said, with where to get it
        assert_eq!(
            install_lines(Some(Family::Debian), &["quickshell", "jq"]),
            [
                "sudo apt install jq",
                "quickshell: no debian package known to omabox: build it (https://quickshell.org)"
            ]
        );
        assert!(install_lines(Some(Family::Arch), &["no-such-need"]).is_empty());
        // every need named by a check is in the table, for every family
        for n in BOX_PROGS
            .iter()
            .chain(SOME_PROGS.iter().map(|(n, _)| n))
            .chain(&[
                "Hyprland",
                "labwc",
                "quickshell",
                "bwrap",
                "pasta",
                "unshare",
                "grim",
                "tools-build",
                "aquamarine-build",
            ])
        {
            assert!(PKGS.iter().any(|(k, _)| k == n), "{n} has packages");
        }
    }

    #[test]
    fn versions() {
        assert_eq!(
            version_in("Hyprland 0.56.2 built from branch v0.56.2 at commit e"),
            Some("0.56.2".into())
        );
        assert_eq!(version_in("bubblewrap 0.12.0"), Some("0.12.0".into()));
        assert_eq!(
            version_in("labwc 0.20.2 (+xwayland) wlroots-0.20.2"),
            Some("0.20.2".into())
        );
        assert_eq!(version_in("pasta 2026_10_02.cba3570"), None);
        assert!(
            at_least("0.56.0", "0.56.0")
                && at_least("0.56.2", "0.56.0")
                && at_least("0.100.0", "0.56.0")
        );
        assert!(!at_least("0.55.4", "0.56.0") && !at_least("0.9.0", "0.11.0"));
    }

    /// Programs looked for in ROOT/usr/bin, their versions read, a known-too-old one failed.
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn programs_where_a_box_looks() {
        let d = tree("progs");
        let r = rb(&d);
        let arch = Some(Family::Arch);
        let c = hyprland_check(&r, arch);
        assert_eq!(c.st, St::Fail);
        assert!(
            c.found.contains("not in /usr/bin") || c.found.contains("no Hyprland in /usr/bin"),
            "{}",
            c.found
        );
        assert_eq!(c.fix, ["sudo pacman -S --needed hyprland"]);
        prog(
            &d,
            "Hyprland",
            "echo 'Hyprland 0.55.1 built from branch v0.55.1 at commit x'",
        );
        let c = hyprland_check(&r, Some(Family::Fedora));
        assert_eq!(c.st, St::Fail, "{}", c.found);
        assert!(
            c.found
                .contains("omabox needs 0.56.0 or later (a Lua config"),
            "{}",
            c.found
        );
        assert_eq!(c.fix, ["sudo dnf install hyprland"]);
        prog(
            &d,
            "Hyprland",
            "echo 'Hyprland 0.56.2 built from branch v0.56.2 at commit x'",
        );
        let c = hyprland_check(&r, arch);
        assert_eq!(
            (c.st, c.found.as_str()),
            (St::Ok, "Hyprland 0.56.2 (Lua config)")
        );
        prog(&d, "bwrap", "echo 'bubblewrap 0.9.0'");
        let c = prog_check(
            &r,
            Some(Family::Debian),
            "bwrap",
            "bwrap",
            Some(("0.11.0", "overlays")),
        );
        assert_eq!(c.st, St::Fail);
        assert_eq!(c.fix, ["sudo apt install bubblewrap"]);
        prog(&d, "bwrap", "echo 'bubblewrap 0.13.1'");
        assert_eq!(
            prog_check(&r, arch, "bwrap", "bwrap", Some(("0.11.0", "overlays"))).st,
            St::Ok
        );
        prog(
            &d,
            "grim",
            "echo 'usage: grim [options...]'; echo '  -o <output>  Set the output name.'",
        );
        let c = grim_check(&r, arch);
        assert_eq!(c.st, St::Warn, "no -T: a warning");
        prog(&d, "grim", "echo '  -T <identifier> Toplevel'");
        assert_eq!(grim_check(&r, arch).st, St::Ok);
        let c = box_progs_check(&r, Some(Family::Suse));
        assert_eq!(c.st, St::Fail);
        assert_eq!(c.fix, ["sudo zypper install jq dbus-1 gnome-keyring"]);
        for p in BOX_PROGS {
            prog(&d, p, "true");
        }
        assert_eq!(box_progs_check(&r, arch).st, St::Ok);
        let c = some_progs_check(&r, arch);
        assert_eq!(c.st, St::Warn, "only some boxes need them");
        assert!(
            c.found.contains("no wlr-randr (NVIDIA boxes"),
            "{}",
            c.found
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn omarchy_and_theme() {
        let d = tree("omarchy");
        let r = rb(&d);
        let c = omarchy_check(&r, "");
        assert_eq!(c.st, St::Fail);
        assert!(c.fix[0].contains("#247"));
        put(&d, "/usr/share/omarchy/default/hypr/bootstrap.lua", "");
        assert_eq!(
            omarchy_check(&r, "4.5.0-1").found,
            "Omarchy 4.5.0-1 (/usr/share/omarchy)"
        );
        assert_eq!(theme_check(&r).st, St::Fail);
        std::fs::create_dir_all(d.join(".local/state/omarchy/current/theme")).unwrap();
        assert_eq!(theme_check(&r).st, St::Ok);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn userns_reasons() {
        let d = tree("userns");
        let r = rb(&d);
        assert!(userns_why(&r).is_empty());
        put(&d, "/proc/sys/kernel/unprivileged_userns_clone", "1\n");
        put(&d, "/proc/sys/user/max_user_namespaces", "63000\n");
        put(
            &d,
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            "0\n",
        );
        assert!(userns_why(&r).is_empty(), "all on");
        put(&d, "/proc/sys/kernel/unprivileged_userns_clone", "0\n");
        put(&d, "/proc/sys/user/max_user_namespaces", "0\n");
        put(
            &d,
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            "1\n",
        );
        let w = userns_why(&r);
        assert_eq!(w.len(), 3);
        assert!(
            w[0].1
                .starts_with("sudo sysctl -w kernel.unprivileged_userns_clone=1")
        );
        assert!(
            w[1].1
                .starts_with("sudo sysctl -w user.max_user_namespaces=")
        );
        assert!(w[2].0.contains("AppArmor") && w[2].1.contains("bwrap-userns-restrict"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn socket_paths() {
        let c = socket_check(b"/run/user/1000/omabox");
        assert_eq!(c.st, St::Ok);
        // /run/user/1000/omabox/ + 40 + /run/wayland-1
        assert!(
            c.found.starts_with("76 bytes at most of 107"),
            "{}",
            c.found
        );
        let deep = format!("/{}/omabox", "d".repeat(60));
        let c = socket_check(deep.as_bytes());
        assert_eq!(c.st, St::Fail, "{}", c.found);
        assert!(c.fix[0].starts_with("run omabox with a shorter XDG_RUNTIME_DIR"));
    }

    #[test]
    fn path_finds_which_omabox() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree("path");
        let me = d.join("me/omabox");
        let other = d.join("other/omabox");
        for p in [&me, &other] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::create_dir_all(d.join("link")).unwrap();
        std::os::unix::fs::symlink(&me, d.join("link/omabox")).unwrap();
        let mer = rb(&me);
        let p = |dirs: &[&str]| -> Vec<u8> {
            dirs.iter()
                .map(|x| s(&rb(&d.join(x))))
                .collect::<Vec<_>>()
                .join(":")
                .into_bytes()
        };
        let c = path_check(
            "path",
            "your shell's",
            Some(&p(&["none", "link", "other"])),
            &mer,
            Kind::Checkout,
        );
        assert_eq!(c.st, St::Ok, "a link to this one: {}", c.found);
        let c = path_check(
            "path",
            "your shell's",
            Some(&p(&["other", "me"])),
            &mer,
            Kind::Checkout,
        );
        assert_eq!(c.st, St::Warn);
        assert!(c.found.contains("finds another omabox"), "{}", c.found);
        let c = path_check(
            "session-path",
            "your desktop session's",
            Some(&p(&["none"])),
            &mer,
            Kind::Checkout,
        );
        assert_eq!(c.st, St::Warn);
        assert!(c.fix[0].contains("~/.config/uwsm/env"), "{:?}", c.fix);
        let c = path_check(
            "session-path",
            "your desktop session's",
            None,
            &mer,
            Kind::Checkout,
        );
        assert!(c.found.starts_with("could not read"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Render nodes under a stand-in root: one usable, a VM's software-only one (install.sh's test),
    /// none at all, none usable.
    #[test]
    fn render_nodes() {
        use std::os::unix::fs::symlink;
        let d = tree("render");
        let r = rb(&d);
        let ctx = Ctx::new(b"doctor", b"/x".to_vec());
        let c = muted(|| render_check(&ctx, &r, Vec::new(), true));
        assert_eq!(c.st, St::Fail);
        assert!(c.found.starts_with("no GPU render node"), "{}", c.found);
        // renderD128: virtio-gpu on PCI, its virtio0 child without virgl (features bit 0 and 4 off)
        put(&d, "/dev/dri/renderD128", "");
        let pci = d.join("sys/devices/pci0000:00/0000:00:02.0");
        std::fs::create_dir_all(pci.join("virtio0")).unwrap();
        std::fs::create_dir_all(d.join("sys/bus/virtio/drivers/virtio_gpu")).unwrap();
        std::fs::create_dir_all(d.join("sys/bus/pci/drivers/virtio-pci")).unwrap();
        symlink(d.join("sys/bus/pci/drivers/virtio-pci"), pci.join("driver")).unwrap();
        symlink(
            d.join("sys/bus/virtio/drivers/virtio_gpu"),
            pci.join("virtio0/driver"),
        )
        .unwrap();
        std::fs::write(pci.join("virtio0/features"), "0000000000\n").unwrap();
        std::fs::create_dir_all(d.join("sys/class/drm/renderD128")).unwrap();
        symlink(&pci, d.join("sys/class/drm/renderD128/device")).unwrap();
        let n128 = b"/dev/dri/renderD128".as_slice();
        assert!(crate::render::software_only_in(n128, &r));
        let c = muted(|| render_check(&ctx, &r, n128.to_vec(), true));
        assert_eq!(c.st, St::Fail, "only a software node: {}", c.found);
        assert!(c.found.contains("renders in software only"), "{}", c.found);
        assert!(c.fix[0].contains("virgl"));
        // with virgl (bit 0 on): it renders
        std::fs::write(pci.join("virtio0/features"), "1000100000\n").unwrap();
        assert!(!crate::render::software_only_in(n128, &r));
        let c = muted(|| render_check(&ctx, &r, n128.to_vec(), true));
        assert_eq!(c.st, St::Ok, "{}", c.found);
        assert!(
            // (and, with the gpu setting naming another GPU, that it is not that one)
            c.found.contains("; a headless box uses renderD128"),
            "{}",
            c.found
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn json_shape() {
        let os = OsRelease::parse("ID=omarchy\nID_LIKE=arch\n");
        let cs = vec![
            check("a", St::Ok, "fine", Vec::new()),
            check("b", St::Warn, "meh", vec!["do x".into()]),
        ];
        assert_eq!(
            json_text(&os, &cs, false, None),
            r#"{"ok":true,"os":{"id":"omarchy","id_like":"arch","family":"arch"},"checks":[{"id":"a","status":"ok","found":"fine","fix":[]},{"id":"b","status":"warn","found":"meh","fix":["do x"]}],"smoke":null}"#
        );
        let cs = vec![check("c", St::Fail, "no", Vec::new())];
        let j = json_text(&OsRelease::default(), &cs, true, None);
        assert!(
            j.starts_with(r#"{"ok":false,"os":{"id":"","id_like":"","family":null}"#),
            "{j}"
        );
        assert!(
            j.ends_with(r#""smoke":{"ok":false,"error":"not started: a check failed"}}"#),
            "{j}"
        );
    }
}
