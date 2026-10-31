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
        // (a namespace of its own with the HOME bound in, as bwrap makes; skipped where unshare can't)
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
