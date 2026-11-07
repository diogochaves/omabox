//! `omabox path`: the box dir, its HOME, its runtime dir, or where each of its logs is.

use crate::args::name_arg;
use crate::boxes::{box_alive, need_box, take_over};
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::json;
use crate::logs::{LOG_NAMES, log_host_path, log_where};
use crate::select::select_box;
use crate::shell;
use serde_json::{Map, Value};

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut logs, mut rundir, mut home, mut as_json) = (false, false, false, false);
    let mut rest = Vec::new();
    for a in args {
        match a.as_slice() {
            b"--logs" => logs = true,
            b"--run" => rundir = true,
            b"--home" => home = true,
            b"--json" => as_json = true,
            _ => rest.push(a.clone()),
        }
    }
    let kinds = u8::from(logs) + u8::from(rundir) + u8::from(home);
    if kinds > 1 {
        return Err(ctx.die("path: one of --home, --run and --logs"));
    }
    if as_json && !logs {
        return Err(ctx.die("path: --json goes with --logs (path --logs --json)"));
    }
    // (name_arg says why; its words are still being read: a usage error.)
    let name = name_arg(ctx, "path", &rest).map_err(|_| ctx.refused())?;
    select_box(ctx, &name)?;
    // Seeding a box's HOME through its path is using it.
    if box_alive(ctx)? {
        crate::whose::mark_used(ctx);
        if !take_over(ctx, false)? {
            need_box(ctx)?;
        }
    }
    // This session's box, up under another directory's name: named, not a path of no box (#327).
    if ctx.name_derived
        && !shell::path(&ctx.d).is_dir()
        && !crate::select::session_boxes(ctx).is_empty()
    {
        need_box(ctx)?;
    }
    if kinds != 1 {
        return out(cat!(ctx.d, "\n"));
    }
    if !shell::path(&ctx.d).is_dir() {
        need_box(ctx)?;
    }
    // --home (#272): the box's HOME (/home/sbx, or your HOME's path in a desk box) on the host, a
    // literal path for an agent's own file tools (Read, Write, Edit), where `$(omabox path)/home`
    // inside a command is refused in a worktree.
    if home {
        // (finding 710: a link in it into your real HOME is said, as following it edits your files;
        // a desk box's only, whose HOME is at your HOME's path: an Omarchy box's seeded theme links
        // into your HOME by design, to what it mounts read-only there)
        let user = shell::env_or_empty("HOME");
        let desk = crate::boxes::box_home(&ctx.d) != crate::boxes::SBX_HOME.as_bytes();
        if let Some((n, first, to)) = desk
            .then(|| real_home_links(&cat!(ctx.d, "/home"), &user))
            .flatten()
        {
            say(cat!(
                "path: box '",
                ctx.name,
                "''s HOME holds ",
                n.to_string(),
                if n == 1 { " link" } else { " links" },
                " into your real HOME (",
                first,
                " -> ",
                to,
                "): on the host, following one reaches your own files, not the box's. Edit the box's files by their paths in this folder or through `omabox run`, never through a link"
            ));
        }
        return out(cat!(ctx.d, "/home\n"));
    }
    // --run (#208): the box's runtime dir, its /run/user/$UID, as the host sees it. The host's own
    // /run/user/$UID is the real session's: an app's state read there is the desktop's, not the box's.
    if rundir {
        return out(cat!(ctx.d, "/run\n"));
    }
    // --logs (issue #41): each log the box has, where it is on the host (`omabox log NAME` reads it).
    let mut found: Vec<(&str, Vec<u8>)> = Vec::new();
    for s in LOG_NAMES {
        let Some(w) = log_where(ctx, s) else { continue };
        let w = log_host_path(ctx, &w);
        if shell::path(&w).exists() {
            found.push((s, w));
        }
    }
    // --json (#273, audit J7): {NAME: PATH}, the logs there are.
    if as_json {
        let m: Map<String, Value> = found
            .into_iter()
            .map(|(s, w)| {
                (
                    s.to_string(),
                    Value::String(String::from_utf8_lossy(&w).into()),
                )
            })
            .collect();
        return out(cat!(json::compact(&Value::Object(m)), "\n"));
    }
    let text: Vec<u8> = found
        .iter()
        .flat_map(|(s, w)| cat!(format!("{s:<9} "), w, "\n"))
        .collect();
    out(text)
}

/// How many folders deep, and how many entries in all, real_home_links looks.
const SCAN_DEPTH: usize = 6;
const SCAN_ENTRIES: usize = 20_000;

/// The links in a box HOME's folder (DIR, as the host sees it) that lead into the user's real HOME
/// (USER, at its path or below; one into the box's own folder is not), each target taken lexically
/// from the link's folder (finding 710): how many, the first one's path in the folder and its
/// target. None when there are none. At most SCAN_DEPTH folders deep and SCAN_ENTRIES entries.
fn real_home_links(dir: &[u8], user: &[u8]) -> Option<(u64, Vec<u8>, Vec<u8>)> {
    use crate::bwrap::lexical;
    let user = lexical(user);
    if user == b"/" || user.is_empty() {
        return None;
    }
    let own = std::fs::canonicalize(shell::path(dir)).ok()?;
    let own = shell::bytes(&own).to_vec();
    let under = |t: &[u8], h: &[u8]| t == h || t.starts_with(&cat!(h, "/"));
    let mut todo: Vec<(Vec<u8>, usize)> = vec![(Vec::new(), 0)];
    let (mut n, mut seen, mut first) = (0u64, 0usize, None);
    while let Some((rel, depth)) = todo.pop() {
        let at = if rel.is_empty() {
            own.clone()
        } else {
            cat!(own, "/", rel)
        };
        let Ok(rd) = std::fs::read_dir(shell::path(&at)) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > SCAN_ENTRIES {
                return first.map(|(f, t)| (n, f, t));
            }
            let name = shell::bytes(std::path::Path::new(&e.file_name())).to_vec();
            let r = if rel.is_empty() {
                name
            } else {
                cat!(rel, "/", name)
            };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                let Ok(raw) = std::fs::read_link(e.path()) else {
                    continue;
                };
                let raw = shell::bytes(&raw).to_vec();
                let to = if raw.starts_with(b"/") {
                    lexical(&raw)
                } else {
                    lexical(&cat!(at, "/", raw))
                };
                if under(&to, &user) && !under(&to, &own) {
                    n += 1;
                    first.get_or_insert((r, raw));
                }
            } else if ft.is_dir() && depth < SCAN_DEPTH {
                todo.push((r, depth + 1));
            }
        }
    }
    first.map(|(f, t)| (n, f, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// path --home's note (finding 710): links into the real HOME, absolute or climbing out, counted;
    /// one into the box's folder (a relative one omabox makes), into /usr, or below the depth, not.
    #[test]
    fn real_home_links_found() {
        let d = std::env::temp_dir().join(format!("omabox-pathlinks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let user = d.join("u");
        let bx = user.join(".cache/omabox/b/home");
        let cur = bx.join(".local/state/omarchy/current");
        std::fs::create_dir_all(&cur).unwrap();
        std::fs::create_dir_all(bx.join("Pictures")).unwrap();
        let ln = |t: &str, at: &std::path::Path| std::os::unix::fs::symlink(t, at).unwrap();
        let ub = shell::bytes(&user).to_vec();
        let db = shell::bytes(&bx).to_vec();
        ln("../../../../Pictures/w.png", &cur.join("background"));
        ln("/usr/share/x", &bx.join("share"));
        assert_eq!(
            real_home_links(&db, &ub),
            None,
            "relative into the box, /usr: none"
        );
        let real = format!("{}/Pictures/w.png", user.display());
        ln(&real, &cur.join("abs"));
        ln("../../../../../../../../.bashrc", &cur.join("climb"));
        let deep = bx.join("a/b/c/d/e/f/g");
        std::fs::create_dir_all(&deep).unwrap();
        ln(&real, &deep.join("too-deep"));
        let (n, first, _) = real_home_links(&db, &ub).unwrap();
        assert_eq!(n, 2, "{}", String::from_utf8_lossy(&first));
        assert!(first.starts_with(b".local/state/omarchy/current/"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
