//! `omabox path`: the box dir, its HOME, its runtime dir, or where each of its logs is.

use crate::args::name_arg;
use crate::boxes::{box_alive, need_box, take_over, touch};
use crate::cat;
use crate::exit::{Ctx, Res, out};
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
        touch(&ctx.at("used"));
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
    // --home (#272): the box's /home/sbx on the host, a literal path for an agent's own file tools
    // (Read, Write, Edit), where `$(omabox path)/home` inside a command is refused in a worktree.
    if home {
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
