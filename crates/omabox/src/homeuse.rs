//! Whose a box HOME is (finding 584). HOMEs live on disk (HOMES, under the cache), box dirs in the
//! runtime dir: an `up` with another runtime dir (a test's stand-in, another login) sees none of this
//! one's boxes. So a HOME is taken as gone only when its box is gone in the runtime dir it was made in,
//! which its record names, and no process uses it; one with no record (an older omabox's) only when
//! nothing uses it either.

use crate::cat;
use crate::shell;
use std::collections::HashSet;

/// The file beside a box's HOME (in HOMES/NAME, which the box never sees: it has HOMES/NAME/home)
/// that names the box dir it was made for.
pub const RECORD: &str = "box-dir";

/// record H: the box dir H's record names (H: HOMES/NAME), None when there is none (an older
/// omabox's HOME) or it is not a plain file.
pub fn record(h: &[u8]) -> Option<Vec<u8>> {
    let f = cat!(h, "/", RECORD);
    let m = std::fs::symlink_metadata(shell::path(&f)).ok()?;
    if !m.is_file() || m.len() > 4096 {
        return None;
    }
    let r = shell::strip_nl(&std::fs::read(shell::path(&f)).ok()?);
    (r.starts_with(b"/") && !r.contains(&b'\n')).then_some(r)
}

/// write_record H D: H's record names box dir D. Whether it was written.
pub fn write_record(h: &[u8], d: &[u8]) -> bool {
    let f = cat!(h, "/", RECORD);
    let new = cat!(f, ".new");
    std::fs::write(shell::path(&new), cat!(d, "\n")).is_ok()
        && std::fs::rename(shell::path(&new), shell::path(&f)).is_ok()
}

/// The name a sweep moves a gone box's HOME to before it removes it, then PID-NAME (#433).
pub const SWEPT: &str = ".swept-";
/// The name a box's HOME is made under, with its record, before it is moved to HOMES/NAME in one
/// rename, then PID-NAME (#433).
pub const MAKING: &str = ".new-";

/// make_home HOMES NAME D: HOMES/NAME made for box dir D, its record and its `home` in it from the
/// first moment it is there: made under a name of its own and renamed into place, so no sweep, of
/// any runtime dir, ever sees it without its record (taken then for an older omabox's HOME and
/// judged by the sweeper's own runtime dir: #433). False when it could not be made, or HOMES/NAME
/// is there and not empty (another box's, made meanwhile).
pub fn make_home(homes: &[u8], name: &[u8], d: &[u8]) -> bool {
    let new = cat!(
        homes,
        "/",
        MAKING,
        std::process::id().to_string(),
        "-",
        name
    );
    let ok = std::fs::create_dir_all(shell::path(&cat!(new, "/home"))).is_ok()
        && write_record(&new, d)
        && std::fs::rename(shell::path(&new), shell::path(&cat!(homes, "/", name))).is_ok();
    if !ok {
        let _ = std::fs::remove_dir_all(shell::path(&new));
    }
    ok
}

/// The log beside the HOMEs (HOMES/.removed.log; hidden, so never taken for a box's) of every HOME
/// omabox removes (#433): when, which process (its pid, command line, runtime dir and pid namespace)
/// and why it judged the HOME free to go. A HOME that vanishes under a box names its remover there.
pub const REMOVED_LOG: &str = ".removed.log";
/// Past this size the log is moved to `.removed.log.old` (the one before dropped) and begun anew.
const REMOVED_LOG_MAX: u64 = 256 * 1024;

/// note_removed HOMES NAME WHY: a line in HOMES' removal log for box NAME's HOME, which this process
/// is about to remove because WHY. Best effort: a log that cannot be written stops no removal.
pub fn note_removed(homes: &[u8], name: &[u8], why: &[u8]) {
    use std::io::Write;
    let f = cat!(homes, "/", REMOVED_LOG);
    if std::fs::metadata(shell::path(&f)).is_ok_and(|m| m.len() > REMOVED_LOG_MAX) {
        let _ = std::fs::rename(shell::path(&f), shell::path(&cat!(f, ".old")));
    }
    let cmd: Vec<u8> = std::fs::read("/proc/self/cmdline")
        .unwrap_or_default()
        .strip_suffix(b"\0")
        .unwrap_or_default()
        .iter()
        .map(|&c| if c == 0 { b' ' } else { c })
        .collect();
    let ns = crate::procfs::pidns(b"self").unwrap_or_else(|| b"-".to_vec());
    let line = cat!(
        shell::date_iso(),
        " ",
        now_ns().to_string(),
        " pid ",
        std::process::id().to_string(),
        " (",
        shell::cut_line(&cmd, 300),
        ") runtime ",
        shell::env_or_empty("XDG_RUNTIME_DIR"),
        " ",
        ns,
        ": box '",
        name,
        "''s HOME removed: ",
        why,
        "\n"
    );
    // (one write with O_APPEND: lines of processes writing at once do not mix)
    if let Ok(mut fh) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(shell::path(&f))
    {
        let _ = fh.write_all(&line);
    }
}

/// Nanoseconds since the epoch, as the removal log's second field has them.
pub fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// removed_note HOMES NAME SINCE_NS: the last line of HOMES' removal log for box NAME's HOME
/// written at or after SINCE_NS (epoch_ns) by another process, when there is one: who removed it
/// while an up made it.
pub fn removed_note(homes: &[u8], name: &[u8], since_ns: u128) -> Option<Vec<u8>> {
    let log = std::fs::read(shell::path(&cat!(homes, "/", REMOVED_LOG))).ok()?;
    let tag = cat!(": box '", name, "''s HOME removed: ");
    let me = cat!(" pid ", std::process::id().to_string(), " (");
    log.split(|&c| c == b'\n')
        .rfind(|l| {
            crate::exit::contains(l, &tag)
                && !crate::exit::contains(l, &me)
                && l.split(|&c| c == b' ')
                    .nth(1)
                    .and_then(|t| std::str::from_utf8(t).ok()?.parse::<u128>().ok())
                    .is_some_and(|t| t >= since_ns)
        })
        .map(<[u8]>::to_vec)
}

/// there P: P exists, or is a link.
pub fn there(p: &[u8]) -> bool {
    let p = shell::path(p);
    p.exists() || p.is_symlink()
}

/// mountinfo LINE: its device (major:minor) and its root (the path in that file system it shows),
/// and its mount point, unescaped.
fn mount_line(l: &[u8]) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let mut w = l.split(|&c| c == b' ');
    let (_, _, dev, root, at) = (w.next()?, w.next()?, w.next()?, w.next()?, w.next()?);
    Some((
        dev.to_vec(),
        crate::broker_side::unescape(root),
        crate::broker_side::unescape(at),
    ))
}

/// What the processes seen use: the mounts of each mount namespace (device, root) and the paths
/// they have open (cwd, root, fds). Read once, when a HOME is first asked about.
pub struct Users {
    /// None: /proc could not be read, so every HOME counts as used.
    seen: Option<Seen>,
}

struct Seen {
    host: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    mounts: HashSet<(Vec<u8>, Vec<u8>)>,
    open: Vec<Vec<u8>>,
}

impl Users {
    pub fn scan() -> Users {
        Users { seen: Seen::read() }
    }

    /// uses P: a process seen has P (or what is in it) mounted, as a box has its HOME, or open, as
    /// a cwd or an fd. A P that cannot be resolved is not used (it is gone); /proc unread: used.
    pub fn uses(&self, p: &[u8]) -> bool {
        let Some(s) = &self.seen else {
            return true;
        };
        let Some(p) = crate::repo::readlink_e(p) else {
            return false;
        };
        let under = |q: &[u8]| q == p.as_slice() || q.starts_with(&cat!(p, "/"));
        if s.open.iter().any(|o| under(o)) {
            return true;
        }
        // P in its own file system: the host mount it is under (the longest mount point; the last
        // one of those, mounted over the rest), its root there and the rest of P.
        let inside = |at: &[u8]| at == b"/" || p.as_slice() == at || p.starts_with(&cat!(at, "/"));
        let Some((dev, root, at)) = s
            .host
            .iter()
            .filter(|(_, _, at)| inside(at))
            .max_by_key(|(_, _, at)| at.len())
            .map(|m| {
                let n = m.2.len();
                s.host
                    .iter()
                    .rev()
                    .find(|(_, _, at)| at.len() == n && inside(at))
                    .unwrap_or(m)
            })
        else {
            return true;
        };
        let rest = if at.as_slice() == b"/" {
            &p[..]
        } else {
            &p[at.len()..]
        };
        let f = if root.as_slice() == b"/" {
            rest.to_vec()
        } else {
            cat!(root, rest)
        };
        s.mounts
            .iter()
            .any(|(d, r)| d == dev && (r == &f || r.starts_with(&cat!(f, "/"))))
    }
}

impl Seen {
    fn read() -> Option<Seen> {
        let host: Vec<_> = std::fs::read("/proc/self/mountinfo")
            .ok()?
            .split(|&c| c == b'\n')
            .filter_map(mount_line)
            .collect();
        if host.is_empty() {
            return None;
        }
        let mut mounts = HashSet::new();
        let mut spaces = HashSet::new();
        let mut open = Vec::new();
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            let n = e.file_name();
            if !n.as_encoded_bytes().iter().all(u8::is_ascii_digit) {
                continue;
            }
            let pd = e.path();
            // (a mount namespace's mounts once, from the first of its processes read)
            let ns = std::fs::read_link(pd.join("ns/mnt")).ok();
            if ns.as_ref().is_none_or(|ns| spaces.insert(ns.clone()))
                && let Ok(mi) = std::fs::read(pd.join("mountinfo"))
            {
                for (dev, root, _) in mi.split(|&c| c == b'\n').filter_map(mount_line) {
                    mounts.insert((dev, root));
                }
            }
            for l in ["cwd", "root"] {
                if let Ok(t) = std::fs::read_link(pd.join(l)) {
                    open.push(shell::bytes(&t).to_vec());
                }
            }
            if let Ok(fds) = std::fs::read_dir(pd.join("fd")) {
                for fd in fds.flatten() {
                    if let Ok(t) = std::fs::read_link(fd.path()) {
                        let t = shell::bytes(&t);
                        if t.starts_with(b"/") {
                            open.push(t.to_vec());
                        }
                    }
                }
            }
        }
        // (the root every process has is no use of anything under it)
        open.retain(|o| o.as_slice() != b"/");
        Some(Seen { host, mounts, open })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A HOME bound into another mount namespace, as a box's is: used while that namespace lives,
    /// not once it is gone; a dir nothing holds is not used, one a process is in is.
    #[test]
    fn a_bound_home_is_used() {
        let r = std::env::temp_dir().join(format!("omabox-hu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        let h = r.join("cache/omabox/b/home");
        std::fs::create_dir_all(&h).unwrap();
        std::fs::create_dir_all(r.join("free/home")).unwrap();
        std::fs::create_dir_all(r.join("cwd")).unwrap();
        let hb = shell::bytes(&r.join("cache/omabox/b")).to_vec();
        let free = shell::bytes(&r.join("free")).to_vec();
        let cwd = shell::bytes(&r.join("cwd")).to_vec();
        // (a namespace of its own with the HOME bound in, as bwrap makes; skipped where unshare is
        // missing, or refused and gone at once: a container without user namespaces, as
        // omarchy-pkgs' bin/build)
        let mut c = std::process::Command::new("unshare");
        c.args([
            "-Urm",
            "sh",
            "-c",
            "mount --bind \"$1\" /mnt && exec sleep 30",
            "_",
        ])
        .arg(&h)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        let mut held = c.spawn().ok();
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(r.join("cwd"))
            .spawn()
            .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut bound = false;
        while held.is_some() && std::time::Instant::now() < end {
            if Users::scan().uses(&hb) {
                bound = true;
                break;
            }
            if held
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_some())
            {
                held = None;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let u = Users::scan();
        assert!(!u.uses(&free), "a dir nothing holds is not used");
        assert!(u.uses(&cwd), "...one a process is in is");
        assert!(
            !u.uses(b"/nonexistent/omabox-hu"),
            "...a path that is gone is not"
        );
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        if let Some(c) = held.as_mut() {
            assert!(bound, "a HOME bound in another mount namespace is used");
            let _ = c.kill();
            let _ = c.wait();
            assert!(
                !Users::scan().uses(&hb),
                "...not once that namespace is gone"
            );
        }
        let _ = std::fs::remove_dir_all(&r);
    }

    /// #433: each removal a line (when, who, why); up's note names another process's removal of
    /// its HOME made since it began, never its own, nor one before.
    #[test]
    fn removal_log() {
        let r = std::env::temp_dir().join(format!("omabox-rl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        let h = shell::bytes(&r).to_vec();
        let before = now_ns();
        note_removed(&h, b"x", b"because");
        let log = std::fs::read_to_string(r.join(REMOVED_LOG)).unwrap();
        let me = format!(" pid {} (", std::process::id());
        assert!(
            log.ends_with(": box 'x''s HOME removed: because\n")
                && log.contains(&me)
                && log.lines().count() == 1,
            "a line, with this process's pid and why: {log}"
        );
        assert_eq!(removed_note(&h, b"x", before), None, "...its own: no note");
        let other = log.replace(&me, " pid 1 (");
        std::fs::write(r.join(REMOVED_LOG), format!("{log}{other}")).unwrap();
        assert_eq!(
            removed_note(&h, b"x", before),
            Some(other.trim_end().as_bytes().to_vec()),
            "...another process's since up began: the note"
        );
        assert_eq!(
            removed_note(&h, b"x", now_ns()),
            None,
            "...one before up began: none"
        );
        assert_eq!(
            removed_note(&h, b"y", before),
            None,
            "...another box's: none"
        );
        let _ = std::fs::remove_dir_all(&r);
    }

    /// #433: a HOME made whole, its record and `home` in it, under its own name for nothing but
    /// the rename; never over another box's HOME made meanwhile.
    #[test]
    fn make_home_whole() {
        let r = std::env::temp_dir().join(format!("omabox-mh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        let homes = shell::bytes(&r.join("cache/omabox")).to_vec();
        assert!(make_home(&homes, b"x", b"/run/user/1/omabox/x"));
        assert_eq!(
            (
                record(&cat!(homes, "/x")),
                r.join("cache/omabox/x/home").is_dir()
            ),
            (Some(b"/run/user/1/omabox/x".to_vec()), true),
            "made with its record and its home"
        );
        assert!(
            !make_home(&homes, b"x", b"/run/user/2/omabox/x"),
            "...not over one there"
        );
        let names: Vec<_> = std::fs::read_dir(r.join("cache/omabox"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            (names, record(&cat!(homes, "/x"))),
            (vec!["x".into()], Some(b"/run/user/1/omabox/x".to_vec())),
            "...which keeps its record, nothing left beside it"
        );
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn record_round_trip() {
        let r = std::env::temp_dir().join(format!("omabox-hr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        let h = shell::bytes(&r).to_vec();
        assert_eq!(record(&h), None, "no record: an older omabox's HOME");
        assert!(write_record(&h, b"/run/user/1/omabox/x"));
        assert_eq!(record(&h), Some(b"/run/user/1/omabox/x".to_vec()));
        std::fs::remove_file(r.join(RECORD)).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", r.join(RECORD)).unwrap();
        assert_eq!(record(&h), None, "...a record that is a link is none");
        let _ = std::fs::remove_dir_all(&r);
    }
}
