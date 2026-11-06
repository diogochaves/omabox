//! Omarchy's crash watcher and a box's crashes (#441, finding 672). A box runs its programs with a
//! core limit of 1 byte, which the kernel's coredump pipe takes as "no core, no report" (finding 67).
//! systemd's coredump socket (262's preset, on a fresh install) journals the crash anyway, with
//! COREDUMP_RLIMIT=1, and omarchy-crash-watch, which follows the journal's crash records, would
//! announce it on the user's desktop. omabox cannot keep the record out (systemd's forwarding to a
//! container fails for one the user owns: upstream finding), so `omabox setup` offers a filter on the
//! reading side: a drop-in for the watcher's unit that puts omabox's journalctl first in its PATH,
//! which passes every record on but those with COREDUMP_RLIMIT=1. Omarchy's script is not touched;
//! the desktop's own crashes are announced as before. `setup --remove` takes it away.

use crate::cat;
use crate::shell;

/// The watcher's unit.
pub const UNIT: &str = "omarchy-crash-watch.service";

/// Where a vendor unit may be, under ROOT, the first found winning (as systemd's search path has
/// them, the admin's first).
const UNIT_DIRS: [&str; 3] = [
    "/etc/systemd/user/",
    "/usr/local/lib/systemd/user/",
    "/usr/lib/systemd/user/",
];

/// The drop-in's line that records the unit's own ExecStart, so a changed unit is noticed.
const WAS: &str = "# omabox: the unit's own ExecStart: ";

/// The journalctl the watcher finds first: every line as journalctl writes it, but a JSON record
/// whose COREDUMP_RLIMIT is "1" (a box's crash) left out. Not JSON (`-o json` not asked, or no jq
/// here): journalctl itself, untouched.
pub const SHIM: &str = r#"#!/bin/bash
# omabox setup made this (#441): the journalctl Omarchy's crash watcher (omarchy-crash-watch) finds
# first, through omabox's drop-in for its unit. It runs the real journalctl and passes every line on,
# but a crash record of a process inside an omabox box: its core limit of exactly 1 byte
# (COREDUMP_RLIMIT=1), which the kernel's coredump pipe takes as "no core, no report" and systemd's
# coredump socket journals anyway. omabox setup --remove takes it away.
set -f
here=$(cd "${BASH_SOURCE[0]%/*}" && pwd -P)
p='' IFS=:
for d in $PATH; do
  [ "$(cd "$d" 2>/dev/null && pwd -P)" = "$here" ] || p=${p:+$p:}$d
done
unset IFS
PATH=$p
json=0 prev=''
for a in "$@"; do
  case $a in
    -ojson | --output=json) json=1 ;;
    json) [ "$prev" = -o ] || [ "$prev" = --output ] && json=1 ;;
  esac
  prev=$a
done
if [ "$json" = 0 ] || ! command -v jq >/dev/null; then exec journalctl "$@"; fi
journalctl "$@" | exec jq -R -r --unbuffered '. as $l | try (fromjson | if .COREDUMP_RLIMIT == "1" then empty else $l end) catch $l'
"#;

/// The user's config and data folders, where the drop-in and the wrapper go.
pub struct Dirs {
    pub config: Vec<u8>,
    pub data: Vec<u8>,
}

impl Dirs {
    /// `${XDG_CONFIG_HOME:-HOME/.config}` and `${XDG_DATA_HOME:-HOME/.local/share}`.
    pub fn of(home: &[u8]) -> Dirs {
        let xdg = |name: &str, default: &str| {
            shell::env(name)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| cat!(home, "/", default))
        };
        Dirs {
            config: xdg("XDG_CONFIG_HOME", ".config"),
            data: xdg("XDG_DATA_HOME", ".local/share"),
        }
    }

    /// The wrapper's folder: what the drop-in puts first in the watcher's PATH.
    pub fn shim_dir(&self) -> Vec<u8> {
        cat!(self.data, "/omabox/crash-watch")
    }

    /// The drop-in.
    pub fn dropin(&self) -> Vec<u8> {
        cat!(self.config, "/systemd/user/", UNIT, ".d/omabox.conf")
    }
}

/// V without blanks (space, tab, CR) at either end.
fn trim(v: &[u8]) -> &[u8] {
    let blank = |c: &u8| matches!(c, b' ' | b'\t' | b'\r');
    let a = v.iter().position(|c| !blank(c)).unwrap_or(v.len());
    let b = v.iter().rposition(|c| !blank(c)).map_or(a, |i| i + 1);
    &v[a..b]
}

/// The vendor unit's ExecStart (its last non-empty one, as a unit's own reset takes it), under ROOT;
/// None without the unit (no Omarchy, or one without the watcher).
pub fn vendor_exec(root: &[u8]) -> Option<Vec<u8>> {
    let root = root.strip_suffix(b"/").unwrap_or(root);
    let text = UNIT_DIRS
        .iter()
        .find_map(|d| std::fs::read(shell::path(&cat!(root, d, UNIT))).ok())?;
    let mut exec: Option<Vec<u8>> = None;
    for l in text.split(|&c| c == b'\n') {
        if let Some(v) = l.strip_prefix(b"ExecStart=") {
            let v = trim(v);
            exec = (!v.is_empty()).then(|| v.to_vec());
        }
    }
    exec
}

/// A unit file's text: `%` and `$` as systemd reads them literally.
fn unit_literal(v: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(v.len());
    for &c in v {
        match c {
            b'%' => o.extend_from_slice(b"%%"),
            b'$' => o.extend_from_slice(b"$$"),
            b'"' | b'\\' => {
                o.push(b'\\');
                o.push(c);
            }
            _ => o.push(c),
        }
    }
    o
}

/// The drop-in for ExecStart EXEC (the unit's own): the same command, its PATH with DIR first. Its
/// prefixes (`-`, `@`, `:`, `+`, `!`) stay first, as systemd reads them.
pub fn dropin_text(dir: &[u8], exec: &[u8]) -> Vec<u8> {
    let n = exec
        .iter()
        .take_while(|c| matches!(c, b'-' | b'@' | b':' | b'+' | b'!'))
        .count();
    let (pre, cmd) = exec.split_at(n);
    cat!(
        "# omabox setup made this (#441): Omarchy's crash watcher reads the journal through omabox's\n",
        "# journalctl, which leaves out the crashes inside omabox's boxes (their 1-byte core limit,\n",
        "# COREDUMP_RLIMIT=1). Your desktop's own crashes are announced as before. omabox setup --remove\n",
        "# takes it away.\n",
        WAS,
        exec,
        "\n[Service]\nExecStart=\nExecStart=",
        pre,
        "/usr/bin/env \"PATH=",
        unit_literal(dir),
        ":${PATH}\" ",
        cmd,
        "\n"
    )
}

/// The ExecStart a drop-in's text says the unit had when it was written.
fn recorded(text: &[u8]) -> Option<Vec<u8>> {
    text.split(|&c| c == b'\n')
        .find_map(|l| l.strip_prefix(WAS.as_bytes()))
        .map(<[u8]>::to_vec)
}

/// Whether the kernel hands crashes to a socket (`kernel.core_pattern` `@...`, systemd's coredump
/// socket), where a box's 1-byte core limit no longer keeps them out of the journal.
pub fn socket_mode(root: &[u8]) -> bool {
    let root = root.strip_suffix(b"/").unwrap_or(root);
    std::fs::read(shell::path(&cat!(root, "/proc/sys/kernel/core_pattern")))
        .is_ok_and(|p| p.first() == Some(&b'@'))
}

/// What the watcher makes of a box's crash.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    /// No watcher of Omarchy's here.
    NoWatcher,
    /// The coredump pipe: a box's crash never reaches the journal.
    Pipe,
    /// omabox's filter in place, for the unit as it is.
    Filtered,
    /// omabox's filter, written for another ExecStart (the unit changed), or its journalctl gone.
    Stale(Vec<u8>),
    /// The socket and no filter: a box's crash is announced.
    Unfiltered,
}

/// The state under ROOT for the user's DIRS.
pub fn state(root: &[u8], dirs: &Dirs) -> State {
    let Some(exec) = vendor_exec(root) else {
        return State::NoWatcher;
    };
    if let Ok(t) = std::fs::read(shell::path(&dirs.dropin())) {
        let was = recorded(&t).unwrap_or_default();
        let shim = cat!(dirs.shim_dir(), "/journalctl");
        let shim = shell::path(&shim);
        let current = std::fs::read(&shim).is_ok_and(|s| s == SHIM.as_bytes());
        return if was == exec && current {
            State::Filtered
        } else {
            State::Stale(was)
        };
    }
    if socket_mode(root) {
        State::Unfiltered
    } else {
        State::Pipe
    }
}

/// Write the wrapper and the drop-in for ROOT's unit; whether anything changed (false: they were
/// so already). Err: what could not be written.
pub fn install(root: &[u8], dirs: &Dirs) -> Result<bool, String> {
    let exec = vendor_exec(root).ok_or_else(|| format!("no {UNIT} here"))?;
    let dir = dirs.shim_dir();
    let shim = cat!(dir, "/journalctl");
    let d = dirs.dropin();
    let text = dropin_text(&dir, &exec);
    let same = |p: &[u8], want: &[u8]| std::fs::read(shell::path(p)).is_ok_and(|t| t == want);
    if same(&shim, SHIM.as_bytes()) && same(&d, &text) {
        return Ok(false);
    }
    let fail = |what: &[u8], e: std::io::Error| format!("{}: {e}", String::from_utf8_lossy(what));
    std::fs::create_dir_all(shell::path(&dir)).map_err(|e| fail(&dir, e))?;
    write_atomic(&shim, SHIM.as_bytes(), 0o755).map_err(|e| fail(&shim, e))?;
    if let Some(i) = d.iter().rposition(|&c| c == b'/') {
        std::fs::create_dir_all(shell::path(&d[..i])).map_err(|e| fail(&d[..i], e))?;
    }
    write_atomic(&d, &text, 0o644).map_err(|e| fail(&d, e))?;
    Ok(true)
}

/// Remove the drop-in and the wrapper; whether there was any.
pub fn remove(dirs: &Dirs) -> bool {
    let d = dirs.dropin();
    let dir = dirs.shim_dir();
    let had = std::fs::remove_file(shell::path(&d)).is_ok();
    let had = std::fs::remove_file(shell::path(&cat!(dir, "/journalctl"))).is_ok() || had;
    for p in [&dir, &d[..d.iter().rposition(|&c| c == b'/').unwrap_or(0)]] {
        let _ = std::fs::remove_dir(shell::path(p));
    }
    had
}

/// TEXT to PATH by a rename, mode MODE.
fn write_atomic(p: &[u8], text: &[u8], mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = cat!(p, ".new");
    std::fs::write(shell::path(&tmp), text)?;
    std::fs::set_permissions(shell::path(&tmp), std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(shell::path(&tmp), shell::path(p))
}

/// The user manager reloaded and the watcher restarted (if it runs), so it reads through the
/// drop-in as it is now; whether both said so.
pub fn reload_watcher() -> bool {
    let run = |args: &[&str]| {
        let mut c = crate::sig::command("systemctl");
        c.arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null());
        crate::sig::run_status(c).unwrap_or(127) == 0
    };
    run(&["daemon-reload"]) && run(&["try-restart", UNIT])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> Vec<u8> {
        let d = std::env::temp_dir().join(format!("omabox-cw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        shell::bytes(&d).to_vec()
    }

    fn put(root: &[u8], rel: &str, text: &str) {
        let p = shell::path(&cat!(root, rel)).to_path_buf();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn the_vendor_units_exec_start_the_admins_first() {
        let r = tmp("vendor");
        assert_eq!(vendor_exec(&r), None, "no unit: no watcher");
        put(
            &r,
            "/usr/lib/systemd/user/omarchy-crash-watch.service",
            "[Unit]\nDescription=x\n[Service]\nType=simple\nExecStart=/usr/bin/omarchy-crash-watch\nRestart=always\n",
        );
        assert_eq!(
            vendor_exec(&r).as_deref(),
            Some(&b"/usr/bin/omarchy-crash-watch"[..])
        );
        put(
            &r,
            "/etc/systemd/user/omarchy-crash-watch.service",
            "[Service]\nExecStart=/bin/false\nExecStart=\nExecStart= -/opt/w --x \n",
        );
        assert_eq!(
            vendor_exec(&r).as_deref(),
            Some(&b"-/opt/w --x"[..]),
            "the admin's, its last"
        );
    }

    #[test]
    fn the_dropin_keeps_the_command_and_its_prefix_and_puts_the_dir_first() {
        let t = dropin_text(
            b"/h/a b%/.local/share/omabox/crash-watch",
            b"-/usr/bin/omarchy-crash-watch",
        );
        let s = String::from_utf8(t.clone()).unwrap();
        assert!(s.contains("\n[Service]\nExecStart=\nExecStart=-/usr/bin/env \"PATH=/h/a b%%/.local/share/omabox/crash-watch:${PATH}\" /usr/bin/omarchy-crash-watch\n"), "{s}");
        assert_eq!(
            recorded(&t).as_deref(),
            Some(&b"-/usr/bin/omarchy-crash-watch"[..])
        );
        assert_eq!(unit_literal(b"a$b\"c\\"), b"a$$b\\\"c\\\\".to_vec());
    }

    #[test]
    fn state_follows_the_unit_the_mode_and_the_filter() {
        let r = tmp("state");
        // (the folders given, never the environment's: a test writes nothing of the user's)
        let h = Dirs {
            config: cat!(r, "/home/.config"),
            data: cat!(r, "/home/.local/share"),
        };
        assert_eq!(state(&r, &h), State::NoWatcher);
        put(
            &r,
            "/usr/lib/systemd/user/omarchy-crash-watch.service",
            "[Service]\nExecStart=/usr/bin/omarchy-crash-watch\n",
        );
        put(
            &r,
            "/proc/sys/kernel/core_pattern",
            "|/usr/lib/systemd/systemd-coredump %P\n",
        );
        assert_eq!(state(&r, &h), State::Pipe);
        put(
            &r,
            "/proc/sys/kernel/core_pattern",
            "@@/run/systemd/coredumpd/kernel\n",
        );
        assert_eq!(state(&r, &h), State::Unfiltered);
        assert_eq!(install(&r, &h), Ok(true));
        assert_eq!(install(&r, &h), Ok(false), "written once");
        assert_eq!(state(&r, &h), State::Filtered);
        put(
            &r,
            "/usr/lib/systemd/user/omarchy-crash-watch.service",
            "[Service]\nExecStart=/usr/bin/omarchy-crash-watch --new\n",
        );
        assert_eq!(
            state(&r, &h),
            State::Stale(b"/usr/bin/omarchy-crash-watch".to_vec()),
            "the unit changed"
        );
        assert_eq!(install(&r, &h), Ok(true));
        assert_eq!(state(&r, &h), State::Filtered);
        assert!(remove(&h));
        assert!(!remove(&h), "nothing left the second time");
        assert_eq!(state(&r, &h), State::Unfiltered);
    }

    #[test]
    fn the_shim_leaves_out_a_boxs_crash_and_passes_the_rest() {
        let r = tmp("shim");
        let real = cat!(r, "/real");
        put(
            &r,
            "/real/journalctl",
            "#!/bin/sh\nprintf '%s\\n' '{\"MESSAGE\":\"desk\",\"COREDUMP_RLIMIT\":\"0\"}' '{\"MESSAGE\":\"box\",\"COREDUMP_RLIMIT\":\"1\"}' 'not json' '{\"MESSAGE\":\"none\"}'\necho \"args: $*\"\n",
        );
        put(&r, "/shim/journalctl", SHIM);
        use std::os::unix::fs::PermissionsExt;
        for p in ["/real/journalctl", "/shim/journalctl"] {
            std::fs::set_permissions(
                shell::path(&cat!(r, p)),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let path = format!(
            "{}:{}:{}",
            String::from_utf8_lossy(&cat!(r, "/shim")),
            String::from_utf8_lossy(&real),
            std::env::var("PATH").unwrap_or_default()
        );
        let run = |args: &[&str]| {
            let o = std::process::Command::new("journalctl")
                .args(args)
                .env("PATH", &path)
                .output()
                .unwrap();
            String::from_utf8(o.stdout).unwrap()
        };
        let j = run(&["-f", "-n", "0", "-o", "json", "MESSAGE_ID=x"]);
        assert_eq!(
            j,
            "{\"MESSAGE\":\"desk\",\"COREDUMP_RLIMIT\":\"0\"}\nnot json\n{\"MESSAGE\":\"none\"}\nargs: -f -n 0 -o json MESSAGE_ID=x\n",
            "{j}"
        );
        let plain = run(&["-u", "x"]);
        assert!(
            plain.contains("\"box\""),
            "not JSON asked: journalctl untouched: {plain}"
        );
        assert!(
            run(&["--output=json"])
                .lines()
                .all(|l| !l.contains("\"box\""))
        );
    }
}
