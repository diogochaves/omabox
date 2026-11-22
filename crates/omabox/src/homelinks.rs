//! Links in a box HOME that lead out of it (#448, finding 714, 752, 756): one rule (View::judge) for
//! up's pass over what it seeds or restores (seed::home_links_under) and for path --home's warning.
//!
//! On the host, `omabox path --home` hands the box HOME to an agent's own file tools; a link there
//! that resolves into your HOME, or into a folder that holds it (`/`, `/home`), makes an edit through
//! it an edit of your file. So a link is judged by where the host resolves it, followed from where it
//! sits (a relative one from its folder in the box HOME, an absolute one as written), each link on
//! the way followed too, and a missing part taken as written (a write through it would make it):
//! never by its target's text alone (`a -> r/home/USER/.ssh/id_rsa` beside `r -> /`).

use crate::cat;
use crate::shell;

/// How deep and how many entries in all a walk looks before it stops and says where: up's pass, once
/// per up, looks far (finding 752); path --home, which agents call often, looks less far (finding
/// 756: 50,000 entries take ~0.1 s warm, 200,000 ~0.15-0.4 s).
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub depth: usize,
    pub entries: usize,
}
pub const UP_CAPS: Caps = Caps {
    depth: 64,
    entries: 200_000,
};
pub const PATH_CAPS: Caps = Caps {
    depth: 32,
    entries: 50_000,
};

/// T is DIR or below it (everything absolute is below `/`).
fn within(t: &[u8], dir: &[u8]) -> bool {
    if dir == b"/" {
        return t.starts_with(b"/");
    }
    !dir.is_empty() && (t == dir || t.starts_with(&cat!(dir, "/")))
}

/// Where the host resolves PATH (absolute): each link on the way followed (40 at most, as the
/// kernel), `..` taken after the part before it is resolved, a part that is not there kept as written
/// with the rest (realpath -m). None for a loop.
pub fn resolve(path: &[u8]) -> Option<Vec<u8>> {
    resolve_at(path).map(|(t, _)| t)
}

/// resolve, and whether the way passed through `/proc` (finding 756). There a link is magic: what
/// `/proc/self/cwd`, `/dev/fd/N` (a link to `/proc/self/fd`) or `/proc/PID/root` lead to is that
/// process's cwd, fd or mount namespace, not omabox's now, so nothing under `/proc` is followed and the
/// rest is kept as written.
fn resolve_at(path: &[u8]) -> Option<(Vec<u8>, bool)> {
    let mut done: Vec<u8> = Vec::new();
    let mut rest: std::collections::VecDeque<Vec<u8>> =
        path.split(|&c| c == b'/').map(<[u8]>::to_vec).collect();
    let mut hops = 0;
    let mut magic = false;
    while let Some(c) = rest.pop_front() {
        match c.as_slice() {
            b"" | b"." => continue,
            b".." if !magic => {
                let cut = done.iter().rposition(|&b| b == b'/').unwrap_or(0);
                done.truncate(cut);
                continue;
            }
            _ => {}
        }
        let next = cat!(done, "/", c);
        if magic || within(&next, b"/proc") {
            magic = true;
            done = next;
            continue;
        }
        match std::fs::symlink_metadata(shell::path(&next)) {
            Ok(m) if m.file_type().is_symlink() => {
                hops += 1;
                if hops > 40 {
                    return None;
                }
                let to = std::fs::read_link(shell::path(&next)).ok()?;
                let to = shell::bytes(&to).to_vec();
                if to.starts_with(b"/") {
                    done.clear();
                }
                for part in to.split(|&b| b == b'/').rev() {
                    rest.push_front(part.to_vec());
                }
            }
            _ => done = next,
        }
    }
    if done.is_empty() {
        done.push(b'/');
    }
    Some((done, magic))
}

/// Why a link in a box HOME leads out of it (finding 752, 756): the one rule for up's pass and for
/// path --home's warning.
#[derive(Debug, PartialEq, Eq)]
pub enum Lead {
    /// into your HOME (as written or resolved)
    Home,
    /// to a folder that holds your HOME (`/`, `/home`)
    HoldsHome,
    /// to a regular file you can write, outside your HOME (S2: a copy of it, or none)
    File,
    /// to what you can write among your session's live sockets and secrets (the runtime dir, `/run`,
    /// `/tmp` itself, `/dev/shm`: refuse_path's places), or where a write would make a file there; the
    /// reason refuse_path gives
    Live(Vec<u8>),
    /// through `/proc` (resolve_at): judged as leading out
    Proc,
}

/// A link judged as leading out: where the host resolves it, and why it counts.
#[derive(Debug, PartialEq, Eq)]
pub struct Out {
    pub to: Vec<u8>,
    pub lead: Lead,
}

impl Out {
    /// What is true of this link, for a note: "it leads to T (in your HOME)".
    pub fn what(&self) -> Vec<u8> {
        let kind: &[u8] = match &self.lead {
            Lead::Home => b"in your HOME",
            Lead::HoldsHome => b"a folder that holds your HOME",
            Lead::File => b"a file you can write, outside your HOME",
            Lead::Live(w) => return cat!("it leads to ", self.to, ", which you can write: ", w),
            Lead::Proc => {
                return cat!(
                    "it leads through /proc (",
                    self.to,
                    "), where it ends depends on the process that follows it"
                );
            }
        };
        cat!("it leads to ", self.to, " (", kind, ")")
    }
}

fn writable(t: &[u8]) -> bool {
    std::ffi::CString::new(t.to_vec())
        .is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
}

/// A box HOME as the host sees it, and your HOME.
pub struct View {
    /// the box HOME, resolved
    pub own: Vec<u8>,
    /// your HOME as written, and resolved
    pub home: Vec<u8>,
    pub home_r: Vec<u8>,
    /// what the box mounts, (source, destination) each (box.json's `mounts`: the repo up runs from,
    /// --ro-bind's, --overlay's): a target the box has at its own path through them is the box's
    /// own view, not a way out (finding 241's links, 756)
    pub mounts: Vec<(Vec<u8>, Vec<u8>)>,
}

impl View {
    /// BOX_HOME on the host, USER your HOME. None when the box HOME is not there or USER is no HOME.
    pub fn new(box_home: &[u8], user: &[u8]) -> Option<View> {
        let home = crate::bwrap::lexical(user);
        if user.is_empty() || home == b"/" {
            return None;
        }
        let own = std::fs::canonicalize(shell::path(box_home)).ok()?;
        let home_r = resolve(&home).unwrap_or_else(|| home.clone());
        Some(View {
            own: shell::bytes(&own).to_vec(),
            home,
            home_r,
            mounts: Vec::new(),
        })
    }

    /// The View with the box's MOUNTS (see `mounts`).
    pub fn with_mounts(mut self, mounts: Vec<(Vec<u8>, Vec<u8>)>) -> View {
        self.mounts = mounts;
        self
    }

    /// T (a resolved path) is in the box HOME.
    pub fn inside(&self, t: &[u8]) -> bool {
        within(t, &self.own)
    }

    /// T (a resolved path) is outside the box HOME and in your HOME or a folder that holds it.
    pub fn leads_home(&self, t: &[u8]) -> bool {
        !self.inside(t)
            && (within(t, &self.home)
                || within(t, &self.home_r)
                || within(&self.home, t)
                || within(&self.home_r, t))
    }

    /// The link at REL (in the box HOME), when it leads out (finding 752, 756): through `/proc`; into
    /// your HOME or a folder that holds it; to a regular file you can write; to what you can write
    /// among refuse_path's places of secrets and live sockets (or a missing part whose nearest folder
    /// there is one you can write). A loop, a link into the box HOME or into what the box mounts at
    /// that same path (`mounts`), a device (`/dev/null`), what you
    /// cannot write, and a folder you can write elsewhere (`/mnt/data/x`) do not.
    pub fn judge(&self, rel: &[u8]) -> Option<Out> {
        let (to, magic) = resolve_at(&cat!(self.own, "/", rel))?;
        let lead = if magic {
            Lead::Proc
        } else if self.inside(&to)
            || (!self.mounts.is_empty() && crate::upspec::own_path_among(&to, &self.mounts))
        {
            // (in the box HOME, or what the box has at that same path: a --plugin linked to its own
            // path in the repo, a same-path --ro-bind or an --overlay, finding 241)
            return None;
        } else if self.leads_home(&to) {
            if within(&self.home, &to) || within(&self.home_r, &to) {
                Lead::HoldsHome
            } else {
                Lead::Home
            }
        } else {
            use std::os::unix::fs::FileTypeExt;
            let live = |at: &[u8]| {
                if writable(at) {
                    crate::repo::refuse_path(&to, false).map(Lead::Live)
                } else {
                    None
                }
            };
            match std::fs::metadata(shell::path(&to)) {
                Ok(m) if m.is_file() => writable(&to).then_some(Lead::File)?,
                Ok(m) if m.file_type().is_char_device() || m.file_type().is_block_device() => {
                    return None;
                }
                Ok(_) => live(&to)?,
                Err(_) => {
                    // (a write through it would make the missing part in its nearest folder)
                    let mut at = to.clone();
                    loop {
                        let cut = at.iter().rposition(|&b| b == b'/').unwrap_or(0);
                        at.truncate(cut.max(1));
                        if at == b"/" || shell::path(&at).is_dir() {
                            break;
                        }
                    }
                    live(&at)?
                }
            }
        };
        Some(Out { to, lead })
    }
}

/// Where a walk stopped short: the folder (in the box HOME) it did not look into, and why.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    pub rel: Vec<u8>,
    pub why: Vec<u8>,
}

/// Each link under REL in the box HOME OWN (empty: all of it), as F(its path in the box HOME, the
/// absolute path of its folder). Dot folders first (`.config` and `.local` before the rest at the
/// top), so a big repo clone does not use up the entries before the look's links; no link followed.
/// Stops at CAPS' entries or below its depth and says where, the first such place.
pub fn walk_links<E>(
    own: &[u8],
    rel: &[u8],
    caps: Caps,
    f: impl FnMut(&[u8], &[u8]) -> Result<(), E>,
) -> Result<Option<Cut>, E> {
    walk(own, rel, caps.depth, caps.entries, f)
}

/// walk_links with its caps: DEPTH folders, ENTRIES entries.
fn walk<E>(
    own: &[u8],
    rel: &[u8],
    max_depth: usize,
    max_entries: usize,
    mut f: impl FnMut(&[u8], &[u8]) -> Result<(), E>,
) -> Result<Option<Cut>, E> {
    let mut todo: Vec<(Vec<u8>, usize)> = vec![(rel.to_vec(), 0)];
    let mut seen = 0usize;
    let mut cut: Option<Cut> = None;
    let rank = |n: &[u8], top: bool| -> u8 {
        if top && (n == b".config" || n == b".local") {
            0
        } else if n.starts_with(b".") {
            1
        } else {
            2
        }
    };
    while let Some((rel, depth)) = todo.pop() {
        let at = if rel.is_empty() {
            own.to_vec()
        } else {
            cat!(own, "/", rel)
        };
        let Ok(rd) = std::fs::read_dir(shell::path(&at)) else {
            continue;
        };
        let top = rel.is_empty();
        // (the type from the folder's own listing, d_type: no lstat per entry where the filesystem
        // gives it, finding 756; std lstats where it does not)
        let mut names: Vec<(Vec<u8>, Option<std::fs::FileType>)> = rd
            .flatten()
            .map(|e| {
                let t = e.file_type().ok();
                (std::os::unix::ffi::OsStringExt::into_vec(e.file_name()), t)
            })
            .collect();
        names.sort_by(|(a, _), (b, _)| (rank(a, top), a).cmp(&(rank(b, top), b)));
        let mut dirs = Vec::new();
        for (name, ft) in names {
            seen += 1;
            let r = if rel.is_empty() {
                name
            } else {
                cat!(rel, "/", name)
            };
            if seen > max_entries {
                return Ok(Some(Cut {
                    rel: r,
                    why: cat!("after ", max_entries.to_string(), " entries"),
                }));
            }
            let Some(m) = ft else {
                continue;
            };
            if m.is_dir() {
                if depth < max_depth {
                    dirs.push(r);
                } else if cut.is_none() {
                    cut = Some(Cut {
                        rel: r,
                        why: cat!(max_depth.to_string(), " folders deep"),
                    });
                }
            } else if m.is_symlink() {
                f(&r, &at)?;
            }
        }
        // (popped in the order they were sorted)
        todo.extend(dirs.into_iter().rev().map(|r| (r, depth + 1)));
    }
    Ok(cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("omabox-homelinks-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(d).unwrap()
    }

    fn b(p: &std::path::Path) -> Vec<u8> {
        shell::bytes(p).to_vec()
    }

    fn ln(t: &str, at: &std::path::Path) {
        std::os::unix::fs::symlink(t, at).unwrap();
    }

    /// Finding 752: a link to `/` (a folder holding your HOME) and one that goes through it with a
    /// relative target lead into your HOME; one inside the box HOME, to /usr, or a loop does not.
    #[test]
    fn links_judged_by_where_they_resolve() {
        let d = tmp("rule");
        let user = d.join("u");
        let bx = user.join(".cache/omabox/b/home");
        std::fs::create_dir_all(bx.join("x/y")).unwrap();
        std::fs::create_dir_all(user.join(".ssh")).unwrap();
        let v = View::new(&b(&bx), &b(&user)).unwrap();
        let into = |r: &str| {
            v.judge(r.as_bytes())
                .is_some_and(|o| matches!(o.lead, Lead::Home | Lead::HoldsHome))
        };
        ln("/", &bx.join("r"));
        assert!(into("r"), "a link to /");
        let rel = format!("r{}/.ssh/id_rsa", user.display());
        ln(&rel, &bx.join("a"));
        assert!(into("a"), "through r -> /, by its text inside the box HOME");
        ln(&format!("r{}/.ssh/new", user.display()), &bx.join("gone"));
        assert!(into("gone"), "one whose file is not there yet");
        ln(d.to_str().unwrap(), &bx.join("up"));
        assert!(into("up"), "a folder that holds your HOME");
        // a relative chain: c -> x/b2 -> ../../../.. (the box dir, in your HOME)
        ln("../../../..", &bx.join("x/b2"));
        ln("x/b2", &bx.join("c"));
        assert!(into("c"), "a relative chain out of the box HOME");
        ln("y/../y", &bx.join("x/in"));
        ln("x/in", &bx.join("c2"));
        assert!(!into("c2"), "a relative chain that stays inside");
        ln("/usr/share", &bx.join("usr"));
        assert!(!into("usr"), "/usr");
        ln("loop2", &bx.join("loop1"));
        ln("loop1", &bx.join("loop2"));
        assert_eq!(resolve(&b(&bx.join("loop1"))), None, "a loop");
        assert!(!into("loop1"), "a loop leads nowhere");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Finding 756: what else leads out, by the same rule (up's pass and path --home): a file you can
    /// write, your session's live places you can write (a missing file there too), anything through
    /// /proc; what stays: a device, a folder you can write elsewhere, a missing file there.
    #[test]
    fn what_else_leads_out() {
        let d = tmp("else");
        let user = d.join("u");
        let bx = user.join(".cache/omabox/b/home");
        std::fs::create_dir_all(&bx).unwrap();
        std::fs::create_dir_all(d.join("data/sets")).unwrap();
        std::fs::write(d.join("data/f.txt"), "x").unwrap();
        let v = View::new(&b(&bx), &b(&user)).unwrap();
        let lead = |t: &str, name: &str| {
            ln(t, &bx.join(name));
            v.judge(name.as_bytes()).map(|o| o.lead)
        };
        assert_eq!(lead("/dev/null", "null"), None, "a device");
        let data = format!("{}/data/sets", d.display());
        assert_eq!(
            lead(&data, "sets"),
            None,
            "a folder you can write elsewhere"
        );
        assert_eq!(
            lead(&format!("{data}/new"), "new"),
            None,
            "a missing file there"
        );
        let f = format!("{}/data/f.txt", d.display());
        assert_eq!(lead(&f, "f"), Some(Lead::File), "a file you can write");
        for (t, name) in [
            ("/proc/self/cwd/x", "cwd"),
            ("/dev/fd/3", "fd"),
            ("/dev/stdout", "stdout"),
            ("/proc/1/root/etc/passwd", "root"),
        ] {
            assert_eq!(lead(t, name), Some(Lead::Proc), "{t}");
        }
        let o = v.judge(b"cwd").unwrap();
        assert_eq!(
            o.to, b"/proc/self/cwd/x",
            "not resolved as omabox's own cwd"
        );
        if std::path::Path::new("/dev/shm").is_dir() && writable(b"/dev/shm") {
            assert!(
                matches!(lead("/dev/shm", "shm"), Some(Lead::Live(_))),
                "/dev/shm"
            );
        }
        let run = format!("/run/user/{}", crate::procfs::uid());
        if writable(run.as_bytes()) {
            assert!(matches!(lead(&run, "rt"), Some(Lead::Live(_))), "{run}");
            let gone = format!("{run}/omabox-none-{}/x.service", std::process::id());
            assert!(
                matches!(lead(&gone, "rtnew"), Some(Lead::Live(_))),
                "{gone}"
            );
        }
        // (and a note for each says what is true of it)
        assert_eq!(
            v.judge(b"f").unwrap().what(),
            format!("it leads to {f} (a file you can write, outside your HOME)").into_bytes()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Finding 756: a link to what the box mounts at its own path (a --plugin in the repo, finding
    /// 241) is the box's own view, even in your HOME; one beside that mount is not.
    #[test]
    fn same_path_mounts_are_the_boxs() {
        let d = tmp("mounts");
        let user = d.join("u");
        let bx = user.join(".cache/omabox/b/home");
        let repo = user.join("code/r");
        std::fs::create_dir_all(bx.join(".config/omarchy/plugins")).unwrap();
        std::fs::create_dir_all(repo.join("widget")).unwrap();
        std::fs::create_dir_all(user.join("code/other")).unwrap();
        ln(
            repo.join("widget").to_str().unwrap(),
            &bx.join(".config/omarchy/plugins/w"),
        );
        ln(user.join("code/other").to_str().unwrap(), &bx.join("other"));
        let v = View::new(&b(&bx), &b(&user)).unwrap();
        assert_eq!(
            v.judge(b".config/omarchy/plugins/w").map(|o| o.lead),
            Some(Lead::Home)
        );
        let v = v.with_mounts(vec![(b(&repo), b(&repo))]);
        assert_eq!(
            v.judge(b".config/omarchy/plugins/w"),
            None,
            "the repo, same path"
        );
        assert_eq!(
            v.judge(b"other").map(|o| o.lead),
            Some(Lead::Home),
            "beside it"
        );
        let v = v.with_mounts(vec![(b(&user.join("code/other")), b(&repo))]);
        assert!(
            v.judge(b".config/omarchy/plugins/w").is_some(),
            "a mount from elsewhere there"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Finding 752: dot folders are walked first, and a cap hit says where.
    #[test]
    fn walk_says_where_it_stops() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join(".config/a")).unwrap();
        std::fs::create_dir_all(d.join("code")).unwrap();
        ln("/x", &d.join(".config/a/l"));
        ln("/x", &d.join("code/l"));
        let mut order = Vec::new();
        let cut = walk_links::<()>(&b(&d), b"", UP_CAPS, |r, _| {
            order.push(r.to_vec());
            Ok(())
        })
        .unwrap();
        assert_eq!(cut, None);
        assert_eq!(order, [b".config/a/l".to_vec(), b"code/l".to_vec()]);
        std::fs::create_dir_all(d.join("deep/d/d/d")).unwrap();
        let cut = walk::<()>(&b(&d), b"", 2, 100, |_, _| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(cut.rel, b"deep/d/d", "{cut:?}");
        assert_eq!(cut.why, b"2 folders deep");
        // (entries: .config, code, deep; then .config/a, a's l: the cap stops at the fifth)
        let mut n = 0;
        let cut = walk::<()>(&b(&d), b"", 9, 4, |_, _| {
            n += 1;
            Ok(())
        })
        .unwrap()
        .unwrap();
        assert_eq!((cut.rel.as_slice(), n), (&b".config/a/l"[..], 0), "{cut:?}");
        assert_eq!(cut.why, b"after 4 entries");
        let _ = std::fs::remove_dir_all(&d);
    }
}
