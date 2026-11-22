//! Links in a box HOME that lead into your real HOME (#448, finding 714, finding 752): one rule for
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

/// How deep and how many entries in all a walk looks before it stops and says where.
pub const WALK_DEPTH: usize = 64;
pub const WALK_ENTRIES: usize = 200_000;

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
    let mut done: Vec<u8> = Vec::new();
    let mut rest: std::collections::VecDeque<Vec<u8>> =
        path.split(|&c| c == b'/').map(<[u8]>::to_vec).collect();
    let mut hops = 0;
    while let Some(c) = rest.pop_front() {
        match c.as_slice() {
            b"" | b"." => continue,
            b".." => {
                let cut = done.iter().rposition(|&b| b == b'/').unwrap_or(0);
                done.truncate(cut);
                continue;
            }
            _ => {}
        }
        let next = cat!(done, "/", c);
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
    Some(done)
}

/// A box HOME as the host sees it, and your HOME.
pub struct View {
    /// the box HOME, resolved
    pub own: Vec<u8>,
    /// your HOME as written, and resolved
    pub home: Vec<u8>,
    pub home_r: Vec<u8>,
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
        })
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

    /// The link at REL (in the box HOME) resolved, when it leads into your HOME (leads_home).
    pub fn link_into_home(&self, rel: &[u8]) -> Option<Vec<u8>> {
        resolve(&cat!(self.own, "/", rel)).filter(|t| self.leads_home(t))
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
/// Stops at WALK_ENTRIES entries or below WALK_DEPTH folders and says where, the first such place.
pub fn walk_links<E>(
    own: &[u8],
    rel: &[u8],
    f: impl FnMut(&[u8], &[u8]) -> Result<(), E>,
) -> Result<Option<Cut>, E> {
    walk(own, rel, WALK_DEPTH, WALK_ENTRIES, f)
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
        let mut names: Vec<Vec<u8>> = rd
            .flatten()
            .map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name()))
            .collect();
        names.sort_by(|a, b| (rank(a, top), a).cmp(&(rank(b, top), b)));
        let mut dirs = Vec::new();
        for name in names {
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
            let Ok(m) = std::fs::symlink_metadata(shell::path(&cat!(own, "/", r))) else {
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
            } else if m.file_type().is_symlink() {
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
        let into = |r: &str| v.link_into_home(r.as_bytes()).is_some();
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

    /// Finding 752: dot folders are walked first, and a cap hit says where.
    #[test]
    fn walk_says_where_it_stops() {
        let d = tmp("walk");
        std::fs::create_dir_all(d.join(".config/a")).unwrap();
        std::fs::create_dir_all(d.join("code")).unwrap();
        ln("/x", &d.join(".config/a/l"));
        ln("/x", &d.join("code/l"));
        let mut order = Vec::new();
        let cut = walk_links::<()>(&b(&d), b"", |r, _| {
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
