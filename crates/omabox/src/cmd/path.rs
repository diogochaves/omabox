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
        // every box's since #448, whose seeding leaves none: one there is a save's from before, or
        // planted)
        let user = shell::env_or_empty("HOME");
        let mounts = std::fs::read(ctx.at("box.json"))
            .ok()
            .and_then(|t| serde_json::from_slice::<Value>(&t).ok())
            .map(|bj| crate::cmd::plugin::box_mounts(&bj))
            .unwrap_or_default();
        let found = real_home_links(&cat!(ctx.d, "/home"), &user, mounts);
        if let Some((first, to, what)) = &found.first {
            let n = found.n;
            say(cat!(
                "path: box '",
                ctx.name,
                "''s HOME holds ",
                n.to_string(),
                if n == 1 { " link" } else { " links" },
                " out of it, as up's check judges them (",
                first,
                " -> ",
                to,
                ": ",
                what,
                "): on the host, following one writes there, not in the box. Edit the box's files by their paths in this folder or through `omabox run`, never through a link"
            ));
        }
        // (finding 752: a look cut short is said, not taken for none)
        if let Some(c) = &found.cut {
            say(cat!(
                "path: note: box '",
                ctx.name,
                "''s HOME was checked for links out of it only up to ~/",
                c.rel,
                " (",
                c.why,
                "): links past it were not checked; never edit through a link there"
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

/// What real_home_links found: how many links lead out, the first one's path in the box HOME, target
/// as written and what is true of it (homelinks::Out::what), and where the look stopped short, if it
/// did.
#[derive(Debug, Default)]
struct Found {
    n: u64,
    first: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    cut: Option<crate::homelinks::Cut>,
}

/// The links in a box HOME's folder (DIR, as the host sees it) that lead out of it, USER being your
/// HOME: up's own rule, the same helper (homelinks::View::judge, finding 752, 756: where the host
/// resolves each; into your HOME or a folder that holds it, to a file you can write, to your
/// session's live sockets or secrets, or through /proc). Within homelinks' caps; where it stopped is
/// said. A theme's own folder as a link (.config/omarchy/themes/NAME) is up --theme-dir's: live,
/// as asked.
fn real_home_links(dir: &[u8], user: &[u8], mounts: Vec<(Vec<u8>, Vec<u8>)>) -> Found {
    let Some(view) = crate::homelinks::View::new(dir, user) else {
        return Found::default();
    };
    let view = view.with_mounts(mounts);
    let mut f = Found::default();
    let cut =
        crate::homelinks::walk_links::<()>(&view.own, b"", crate::homelinks::PATH_CAPS, |r, _| {
            let theme_dir = r.starts_with(b".config/omarchy/themes/")
                && !r[b".config/omarchy/themes/".len()..].contains(&b'/');
            if theme_dir {
                return Ok(());
            }
            let Some(o) = view.judge(r) else {
                return Ok(());
            };
            f.n += 1;
            if f.first.is_none() {
                let raw = std::fs::read_link(shell::path(&cat!(view.own, "/", r)))
                    .map_or_else(|_| Vec::new(), |t| shell::bytes(&t).to_vec());
                f.first = Some((r.to_vec(), raw, o.what()));
            }
            Ok(())
        });
    f.cut = cut.unwrap_or(None);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// path --home's note (finding 710, 752, 756): links into the real HOME, absolute or climbing out,
    /// at any depth, or to a folder that holds it, a writable file, through /proc, counted; one into
    /// the box's folder (a relative one omabox makes), into /usr, to /dev/null or a folder
    /// elsewhere, not.
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
        let f = real_home_links(&db, &ub, Vec::new());
        assert!(
            f.first.is_none() && f.cut.is_none(),
            "relative into the box, /usr: none"
        );
        let real = format!("{}/Pictures/w.png", user.display());
        ln(&real, &cur.join("abs"));
        ln("../../../../../../../../.bashrc", &cur.join("climb"));
        let deep = bx.join("a/b/c/d/e/f/g");
        std::fs::create_dir_all(&deep).unwrap();
        ln(&real, &deep.join("deep"));
        // (a --theme-dir's link is the theme's whole folder; one inside a theme is not)
        let themes = bx.join(".config/omarchy/themes");
        std::fs::create_dir_all(themes.join("t/backgrounds")).unwrap();
        ln(
            &format!("{}/dev/live", user.display()),
            &themes.join("live"),
        );
        let f = real_home_links(&db, &ub, Vec::new());
        let (first, _, _) = f.first.unwrap();
        assert_eq!(f.n, 3, "{}", String::from_utf8_lossy(&first));
        assert!(first.starts_with(b".local/state/omarchy/current/"));
        ln(&real, &themes.join("t/backgrounds/1.png"));
        assert_eq!(real_home_links(&db, &ub, Vec::new()).n, 4);
        // (a link to /, and one through it whose text stays in the box HOME)
        ln("/", &bx.join("r"));
        ln(&format!("r{}/.bashrc", user.display()), &bx.join("via"));
        let f = real_home_links(&db, &ub, Vec::new());
        assert_eq!(f.n, 6);
        assert!(f.cut.is_none());
        // (finding 756: up's rule, so a file you can write outside your HOME and a link through
        // /proc count; /dev/null and a folder you can write elsewhere do not)
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::write(d.join("data/f"), "x").unwrap();
        ln("/dev/null", &bx.join("null"));
        ln(&format!("{}/data", d.display()), &bx.join("data"));
        assert_eq!(
            real_home_links(&db, &ub, Vec::new()).n,
            6,
            "a device, a folder elsewhere"
        );
        ln(&format!("{}/data/f", d.display()), &bx.join("f"));
        ln("/proc/self/cwd/x", &bx.join("cwd"));
        assert_eq!(
            real_home_links(&db, &ub, Vec::new()).n,
            8,
            "a writable file, through /proc"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
