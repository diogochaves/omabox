//! `omabox hyprctl [-b NAME] ARGS...`: hyprctl against the box's Hyprland (on_box). Its answer is
//! held until it is known not to be hyprctl's own "didn't respond" (#166): a box gone, or one not
//! answering a ping, is said in words; a failure of a box that answers is hyprctl's, as it came.

use crate::args::optarg;
use crate::boxes::{box_alive, need_box};
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_answers, hypr_down_text, hypr_errors_note};
use crate::jq;
use crate::jqprog::{JQ_MON, NOTIFY_CORNER_JQ};
use crate::select::select_box;
use crate::shell;
use std::io::{Read, Seek};
use std::os::fd::OwnedFd;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut name = Vec::new();
    let mut i = 0;
    while matches!(args.get(i).map(Vec::as_slice), Some(b"-b" | b"--box")) {
        optarg(ctx, &args[i], args.get(i + 1))?;
        name = args[i + 1].clone();
        i += 2;
    }
    let rest = &args[i..];
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // Not for what runs until the caller ends it: rollinglog -f, a repl with no code.
    let joined = crate::cat!(" ", rest.join(&b' '), " ");
    let mut follow = rest.iter().any(|a| a == b"-f" || a == b"--follow")
        && crate::exit::contains(&joined, b" rollinglog ");
    if rest.last().is_some_and(|a| a == b"repl") {
        follow = true;
    }
    let mut argv = vec![b"hyprctl".to_vec()];
    if follow {
        argv.extend(rest.iter().cloned());
        let r = on_box(ctx, &argv, false, Out::Inherit, ErrTo::Inherit).or_exit()?;
        return code(r.rc);
    }
    // Held in an unlinked file, byte for byte, printed once it is not hyprctl's own error.
    let Some(mut f) = temp_file() else {
        return Err(ctx.die("hyprctl: no temporary file"));
    };
    let w = f
        .try_clone()
        .map(OwnedFd::from)
        .map_err(|_| ctx.die("hyprctl: no temporary file"))?;
    let mut argv = vec![
        b"timeout".to_vec(),
        HYPR_LIMIT.as_bytes().to_vec(),
        b"hyprctl".to_vec(),
    ];
    argv.extend(rest.iter().cloned());
    let r = on_box(ctx, &argv, false, Out::Fd(w), ErrTo::Inherit).or_exit()?;
    ctx.box_gone_said |= r.gone_said;
    let mut rc = r.rc;
    if rc != 0 {
        if !box_alive(ctx)? {
            if ctx.box_gone_said {
                return Err(Exit(1));
            }
            return Err(ctx.die(hypr_down_text(ctx)));
        }
        if rc == 124 {
            let t = hung_text(ctx, &format!("{HYPR_LIMIT} s"))?;
            return Err(ctx.die(t));
        }
        if !hypr_answers(ctx)? {
            let t = hung_text(ctx, "")?;
            return Err(ctx.die(t));
        }
    }
    let mut held = Vec::new();
    let _ = f.rewind();
    let _ = f.read_to_end(&mut held);
    let said_ok = shell::strip_nl(&held) == b"ok";
    out(held)?;
    // A reload loads the box HOME's ~/.config/hypr modules again (#246): the config's errors said.
    // A notification is drawn in a corner a shot of the screen's left misses (#359): said where.
    if rc == 0 {
        if let Some(a) = rest.iter().find(|a| !a.starts_with(b"-")) {
            if a == b"reload" && !hypr_errors_note(ctx)? {
                rc = 1;
            }
            if a == b"notify" && said_ok {
                notify_note(ctx);
            }
        }
    }
    code(rc)
}

/// After `hyprctl notify`'s ok (#359): where Hyprland draws it, the focused monitor's top right,
/// and a shot of that corner. Nothing when the monitors cannot be read.
fn notify_note(ctx: &Ctx) {
    let argv: Vec<Vec<u8>> = ["timeout", "6", "hyprctl", "-j", "monitors"]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    let r = on_box(ctx, &argv, true, Out::Capture(Some(1_048_576)), ErrTo::Null);
    if r.rc != 0 {
        return;
    }
    if let Some(t) = notify_text(&ctx.name, &r.out) {
        say(t);
    }
}

/// notify_note's words from `hyprctl -j monitors` (MONS), for box NAME.
fn notify_text(name: &[u8], mons: &[u8]) -> Option<Vec<u8>> {
    let r = jq::jq(
        &["-r", &format!("{JQ_MON}{NOTIFY_CORNER_JQ}")],
        Some(mons),
        jq::Err::Null,
    );
    let t = r.text();
    let f: Vec<&[u8]> = t.split(|&c| c == b' ').collect();
    if !r.ok() || f.len() != 5 {
        return None;
    }
    Some(crate::cat!(
        "note: Hyprland draws it at the top right of the focused monitor, ",
        f[0],
        ", sliding in over 0.6 s: omabox shot -b ",
        name,
        " -g ",
        f[1..].join(&b','),
        " shows that corner"
    ))
}

/// `return RC`.
pub fn code(rc: i32) -> Res<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(Exit(u8::try_from(rc & 0xff).unwrap_or(1)))
    }
}

/// `f=$(mktemp)`, opened and unlinked: mktemp's own words on stderr when it cannot.
fn temp_file() -> Option<std::fs::File> {
    let o = crate::sig::command("mktemp")
        .stderr(std::process::Stdio::inherit())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let p = shell::path(&shell::strip_nl(&o.stdout));
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&p)
        .ok();
    let _ = std::fs::remove_file(&p);
    f
}

#[cfg(test)]
mod tests {
    use super::notify_text;

    #[test]
    fn notify_corner() {
        // Two monitors, the second focused, scaled 2 and turned: its top right, in layout pixels.
        let mons = br#"[{"name":"HEADLESS-2","x":0,"y":0,"width":1920,"height":1080,"scale":1,"transform":0,"focused":false,"disabled":false},
            {"name":"HEADLESS-3","x":1920,"y":0,"width":3440,"height":1440,"scale":2,"transform":1,"focused":true,"disabled":false}]"#;
        assert_eq!(
            notify_text(b"b1", mons).as_deref(),
            Some(&b"note: Hyprland draws it at the top right of the focused monitor, HEADLESS-3, sliding in over 0.6 s: omabox shot -b b1 -g 2280,0,360,573 shows that corner"[..])
        );
        let one = br#"[{"name":"WAYLAND-1","x":0,"y":0,"width":3440,"height":1440,"scale":1,"transform":0,"focused":true,"disabled":false}]"#;
        assert!(
            notify_text(b"b", one)
                .unwrap()
                .ends_with(b"-g 1720,0,1720,480 shows that corner")
        );
        // No monitor focused, or no answer: nothing said.
        assert_eq!(notify_text(b"b", b"[]"), None);
        assert_eq!(notify_text(b"b", b""), None);
    }
}
