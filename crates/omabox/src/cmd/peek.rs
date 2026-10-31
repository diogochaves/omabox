//! `omabox peek [-b NAME] [--fps N] [--workspace WS] [--monitor NAME] [--focus]`: a live, view-only
//! window of a headless box on the user's workspace 9 (or the configured one, finding 70), opened
//! without focus by the host Hyprland's exec rules: frames copied out of the box (tools/peek),
//! nothing sent in. --focus (the bar widget's click: the user asked) then brings it forward, and for
//! an interactive box its own window; nothing is focused unasked. The host's Lua gets checked,
//! tame values only (finding 63).

use crate::args::{optarg, unknown_opt};
use crate::boxes::{BoxPid, box_env, box_pid, meta, need_box};
use crate::cat;
use crate::deskwin::ws_check;
use crate::exit::{Ctx, Res, say};
use crate::host::{HostAsked, HostSilent, Query, host_query, host_session_die, lua_str};
use crate::hypr::hypr_json;
use crate::jq::{self, Args};
use crate::marks::peek_pid;
use crate::screen::main_screen;
use crate::select::select_box;
use crate::settle::ws_text;
use crate::shell::{self, strip_nl};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    // (an INT, TERM or HUP stops it once its foreground step is over: trap.rs)
    crate::trap::stop_on_signals();
    let (mut name, mut fps, mut focus, mut ws, mut monitor) =
        (Vec::new(), b"10".to_vec(), false, Vec::new(), Vec::new());
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--fps" => {
                optarg(ctx, a, v)?;
                fps = args[i + 1].clone();
                i += 1;
            }
            b"--workspace" => {
                optarg(ctx, a, v)?;
                let w = &args[i + 1];
                ws = ws_check(w).ok_or_else(|| {
                    ctx.die(cat!(
                        "--workspace: 1-99, special (the scratchpad, SUPER+S) or special:NAME, got ",
                        w
                    ))
                })?;
                i += 1;
            }
            b"--focus" => focus = true,
            b"--monitor" => {
                optarg(ctx, a, v)?;
                let m = &args[i + 1];
                if !shell::re_match("^[A-Za-z0-9_-]{1,32}$", m) {
                    return Err(ctx.die(cat!(
                        "peek: --monitor takes a monitor's name (omabox monitor list), got ",
                        m
                    )));
                }
                monitor = m.clone();
                i += 1;
            }
            w if w.starts_with(b"-") => return Err(unknown_opt(ctx, "peek", a)),
            w => {
                if !name.is_empty() {
                    return Err(ctx.die(cat!("peek: one box name, got ", name, " and ", w)));
                }
                name = w.to_vec();
            }
        }
        i += 1;
    }
    if !shell::re_match("^[1-9][0-9]?$", &fps) {
        return Err(ctx.die(cat!("peek: --fps is 1 to 99, got ", fps)));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // (--focus: the user asked for a change on their desktop, S8's exception: HostAsked)
    let asked = if focus {
        Some(HostAsked::user_asked(ctx)?)
    } else {
        host_session_die(ctx)?;
        None
    };
    if meta(&ctx.d, "mode").0 == b"interactive" {
        // omabox-wlfd connects and execs bwrap, so the host sees the window as the outer bwrap's.
        if let Some(asked) = &asked {
            let pid = match box_pid(&ctx.d) {
                BoxPid::Pid(p) => crate::procfs::proc_ppid(&p).unwrap_or_default(),
                _ => Vec::new(),
            };
            return focus_pid(ctx, asked, &pid);
        }
        say(cat!(
            "box '",
            ctx.name,
            "' is interactive: it is already a window on ",
            ws_text(&meta(&ctx.d, "workspace").0)
        ));
        return Ok(());
    }
    // The screen it shows (#122): the main one, or --monitor's; one peek window each.
    let mons = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let r = jq::jq(
        &["-r", &format!("{}.name // empty", main_screen(ctx))],
        Some(&mons),
        jq::Err::Inherit,
    );
    if !r.ok() {
        return Err(crate::exit::Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    let mut output = r.text();
    if !monitor.is_empty() {
        let has = Args::new(&["-e"])
            .kv("--arg", "n", &monitor)
            .arg("any(.[]; .name == $n and (.disabled | not))")
            .run(Some(&mons), jq::Err::Inherit)
            .ok();
        if !has {
            let names = jq::jq(
                &["-r", r#"map(.name) | join(", ")"#],
                Some(&mons),
                jq::Err::Inherit,
            )
            .text();
            return Err(ctx.die(cat!(
                "peek: box '",
                ctx.name,
                "' has no monitor ",
                monitor,
                " (",
                names,
                ")"
            )));
        }
        if monitor == output {
            monitor.clear();
        }
    }
    if !monitor.is_empty() {
        output = monitor.clone();
    }
    if !shell::re_match("^[A-Za-z0-9_-]+$", &output) {
        return Err(ctx.die(cat!(
            "peek: box '",
            ctx.name,
            "' reports no usable output (",
            output,
            ")"
        )));
    }
    if let Some(pid) = peek_pid(ctx, Some(&output)) {
        if let Some(asked) = &asked {
            return focus_pid(ctx, asked, &pid);
        }
        let of = if monitor.is_empty() {
            Vec::new()
        } else {
            cat!(" of ", monitor)
        };
        say(cat!("box '", ctx.name, "' already has a peek window", of));
        return Ok(());
    }
    if ws.is_empty() {
        ws = crate::conf::workspace();
    }
    ws_note(&ws);
    let peek = crate::root().join("tools/peek/omabox-peek");
    let peek = shell::bytes(&peek).to_vec();
    if !crate::exec::executable(&peek) {
        return Err(ctx.die(cat!("peek: missing ", peek, ": run install.sh")));
    }
    // peek runs on the host (it is a window there) and connects to the box's socket from the host's
    // side of its runtime dir, which the box can write: only a real socket, no symlink (finding 63;
    // tools/peek also checks every frame it is sent).
    let Some(wd) = box_env(&ctx.d, "WAYLAND_DISPLAY") else {
        return Err(ctx.die(cat!("peek: box '", ctx.name, "' has no WAYLAND_DISPLAY")));
    };
    let sock = cat!(ctx.d, "/run/", wd);
    let is_sock =
        std::fs::symlink_metadata(shell::path(&sock)).is_ok_and(|m| m.file_type().is_socket());
    if !is_sock {
        return Err(ctx.die(cat!("peek: ", sock, " is not the box's Wayland socket")));
    }
    // A marks file (finding 85) only we can read; peek reads what is added to it. Fresh for the
    // box's first peek window, shared by the others (#164: made anew, the windows open lost it).
    let marks = ctx.at("marks");
    if peek_pid(ctx, None).is_none() {
        let _ = std::fs::remove_file(&marks);
    }
    if !marks.is_file() {
        if let Err(e) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&marks)
        {
            // (said in omabox's words, 1: finding 441)
            return Err(ctx.die(cat!(
                "peek: ",
                crate::shots::cant_write(shell::bytes(&marks), &e)
            )));
        }
    }
    let marker = peek_marker(ctx);
    let env = if marker.is_empty() {
        Vec::new()
    } else {
        cat!("env ", marker, " ")
    };
    let mon_t = if monitor.is_empty() {
        Vec::new()
    } else {
        cat!(" · ", monitor)
    };
    // Launched by the host Hyprland: its exec rules put exactly this window on that workspace,
    // unfocused.
    let cmd = cat!(
        env,
        lua_str(ctx, &peek),
        " --box ",
        lua_str(ctx, &sock),
        " --output ",
        output,
        " --fps ",
        fps,
        " --title \"omabox peek: ",
        ctx.name,
        mon_t,
        "\" --marks ",
        lua_str(ctx, shell::bytes(&marks))
    );
    HostSilent::new(ctx).exec_peek(&cmd, &ws)?;
    say(cat!(
        "peek at box '",
        ctx.name,
        "' on ",
        ws_text(&ws),
        " (view only; close it with SUPER+W, or it goes with the box)"
    ));
    let Some(asked) = asked else {
        return Ok(());
    };
    // (5 s on the monotonic clock: settle::after)
    let deadline = crate::settle::after(5);
    let pid = loop {
        if let Some(pid) = peek_pid(ctx, Some(&output)) {
            let (rc, cl) = host_query(ctx, Query::Clients, false);
            let seen = Args::new(&["-e"])
                .kv("--argjson", "p", &pid)
                .arg("any(.pid == $p)")
                .run(Some(&shell::strip_nl(&cl)), jq::Err::Inherit)
                .ok();
            if rc == 0 && seen {
                break pid;
            }
        }
        if crate::settle::past(deadline) {
            return Err(ctx.die(cat!(
                "peek: its window did not come up on your desktop in 5 s, so nothing was focused (omabox peek -b ",
                ctx.name,
                " without --focus opens it on ",
                ws_text(&ws),
                "; omabox ls: is the box up?)"
            )));
        }
        crate::trap::sleep(b"0.05");
    };
    focus_pid(ctx, &asked, &pid)
}

/// ws_note WS: one line when WS is the scratchpad on an Omarchy whose scratchpad is its Quake
/// console (4.5: default/hypr/qconsole.lua, told by the host's tree, never its version; #292,
/// finding 423).
pub fn ws_note(ws: &[u8]) {
    if ws != b"special:scratchpad" {
        return;
    }
    let om = shell::env("OMARCHY_PATH")
        .filter(|v| !v.is_empty())
        .unwrap_or(b"/usr/share/omarchy".to_vec());
    if shell::path(&cat!(om, "/default/hypr/qconsole.lua")).is_file() {
        say(
            "note: on this Omarchy the scratchpad is its Quake console: a half-height panel that starts the default agent when it opens empty; special:omabox gives box windows a special workspace of their own, full size",
        );
    }
}

/// Who asked for a peek window, for its process's environment (finding 121), where the suite's
/// leak detector reads it: OMABOX_SUITE=ID for a command the suite ran, OMABOX_PEEK_BY=you for
/// `omabox peek` itself; nothing else.
fn peek_marker(ctx: &Ctx) -> Vec<u8> {
    let suite = shell::env_or_empty("OMABOX_SUITE");
    if !suite.is_empty() {
        if shell::re_match("^[A-Za-z0-9_-]{1,32}$", &suite) {
            return cat!("OMABOX_SUITE=", suite);
        }
        return Vec::new();
    }
    if ctx.main_cmd == b"peek" {
        return b"OMABOX_PEEK_BY=you".to_vec();
    }
    Vec::new()
}

/// focus_pid PID: a host window focused by its pid (Hyprland switches to its workspace), on the
/// user's request (--focus: ASKED).
fn focus_pid(ctx: &Ctx, asked: &HostAsked, pid: &[u8]) -> Res<()> {
    if !shell::is_num(pid) {
        let lead = if ctx.main_cmd.is_empty() {
            Vec::new()
        } else {
            cat!(ctx.main_cmd, ": ")
        };
        return Err(ctx.die(cat!(
            lead,
            "no window to focus (the box's window is gone? omabox ls)"
        )));
    }
    asked.focus_pid(ctx, pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t_unit_leak_scan's peek_marker (finding 121): a command the suite ran; `omabox peek` by you;
    /// any other command none; an OMABOX_SUITE that is not a tame word none.
    #[test]
    fn peek_marker_as_bash() {
        let mut ctx = Ctx::new(b"peek", b"/x".to_vec());
        let m = |ctx: &Ctx, suite: Option<&str>| {
            shell::with_env(&[("OMABOX_SUITE", suite)], || {
                String::from_utf8(peek_marker(ctx)).unwrap()
            })
        };
        assert_eq!(m(&ctx, Some("t9")), "OMABOX_SUITE=t9");
        assert_eq!(m(&ctx, None), "OMABOX_PEEK_BY=you");
        assert_eq!(m(&ctx, Some("t1' x")), "");
        ctx.main_cmd = b"up".to_vec();
        assert_eq!(m(&ctx, None), "");
    }
}
