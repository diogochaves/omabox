//! `omabox pid [-b NAME] [hyprland|shell] [--in-box] [--json]` (#331, finding 586): the host pid of
//! the box's Hyprland (or its shell), for /proc/PID/maps, perf or strace on the host; `--in-box` the
//! pid as the box numbers it (what `gdb --pid` and `omabox run -- kill` take). `which` goes the
//! other way. Hyprland's from its instance lock matched through NSpid (hypr.rs hypr_host_pid, as
//! `ls` finds it); the shell's from the pid share/shell.sh writes, found among the box's processes
//! the same way. Nothing printed and exit 1 when it is not running, so `P=$(omabox pid) || ...`
//! never leaves an empty pid behind.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{RealBox, need_box};
use crate::cat;
use crate::exit::{Ctx, Res, out};
use crate::procfs;
use crate::select::select_box;
use crate::shellnote::{shell_pid, shell_running, shell_state};
use serde_json::{Map, Value};

/// The host pid of the box process whose own pid (in the box) is INNER, among the box's processes
/// ("HOST_PID BOX_PID BOX_SID START", jobs.rs box_procs).
pub fn host_of(procs: &[[Vec<u8>; 4]], inner: &[u8]) -> Option<Vec<u8>> {
    procs.iter().find(|p| p[1] == inner).map(|p| p[0].clone())
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut what, mut inside, mut json) = (Vec::new(), Vec::new(), false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--in-box" => inside = true,
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("pid"),
            x if x.starts_with(b"-") => return Err(unknown_opt(ctx, "pid", a)),
            b"hyprland" | b"shell" if what.is_empty() => what = a.clone(),
            x => return Err(unexpected(ctx, "pid", x, "(hyprland or shell)")),
        }
        i += 1;
    }
    if what.is_empty() {
        what = b"hyprland".to_vec();
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let (host, inner) = if what == b"hyprland" {
        let host = match crate::hypr::hypr_host_pid(&RealBox, ctx) {
            Some(Some(p)) => p,
            Some(None) => {
                return Err(ctx.die(cat!("pid: box '", ctx.name, "' has no Hyprland running")));
            }
            None => {
                return Err(ctx.die(cat!(
                    "pid: cannot read box '",
                    ctx.name,
                    "''s processes from this pid namespace (a sandbox's?): run omabox outside it"
                )));
            }
        };
        let s =
            std::fs::read(crate::shell::path(&cat!("/proc/", host, "/status"))).unwrap_or_default();
        let inner = procfs::status_last(&s, "NSpid");
        (host, inner)
    } else {
        let Some(inner) = shell_pid(ctx).filter(|p| shell_running(ctx, p)) else {
            return Err(ctx.die(cat!(
                "pid: box '",
                ctx.name,
                "' has no shell running (",
                shell_state(ctx),
                ")"
            )));
        };
        let Some(host) = host_of(&crate::jobs::box_procs(ctx), &inner) else {
            return Err(ctx.die(cat!(
                "pid: the shell of box '",
                ctx.name,
                "' (pid ",
                inner,
                " in it) is not among its processes on the host (it just ended, or a sandbox hides them)"
            )));
        };
        (host, inner)
    };
    if json {
        let mut m = Map::new();
        let num = |v: &[u8]| crate::shell::dec(v).map_or(Value::Null, |n| Value::Number(n.into()));
        m.insert(
            "box".into(),
            Value::String(String::from_utf8_lossy(&ctx.name).into_owned()),
        );
        m.insert(
            "process".into(),
            Value::String(String::from_utf8_lossy(&what).into_owned()),
        );
        m.insert("pid".into(), num(&host));
        m.insert("box_pid".into(), num(&inner));
        return out(cat!(crate::json::compact(&Value::Object(m)), "\n"));
    }
    out(cat!(if inside { &inner } else { &host }, "\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // #331: the box's own pid to the host's, through the box's process list.
    #[test]
    fn hosts() {
        let p = |h: &str, b: &str| -> [Vec<u8>; 4] {
            [h.into(), b.into(), b"1".to_vec(), b"9".to_vec()]
        };
        let procs = [p("4100", "1"), p("4123", "23"), p("4230", "230")];
        assert_eq!(host_of(&procs, b"23"), Some(b"4123".to_vec()));
        assert_eq!(host_of(&procs, b"2"), None);
        assert_eq!(host_of(&[], b"1"), None);
    }
}
