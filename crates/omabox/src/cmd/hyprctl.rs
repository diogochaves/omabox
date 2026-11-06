//! `omabox hyprctl [-b NAME] ARGS...`: hyprctl against the box's Hyprland (on_box). Its answer is
//! held until it is known not to be hyprctl's own "didn't respond" (#166): a box gone, or one not
//! answering a ping, is said in words; a failure of a box that answers is hyprctl's, as it came.
//! A dispatch in the old syntax, which the box's Lua config dispatches nothing for, fails with the
//! Lua for it (#427, finding 677).

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
    let legacy = crate::exit::contains(&held, LEGACY_NOTE);
    out(held)?;
    // The old dispatcher syntax, which a box's Lua config runs as Lua: nothing dispatched (#427). A
    // single dispatch is hyprctl's 7 already, a --batch's 0: non-zero either way, with the Lua for it.
    if legacy {
        for line in legacy_hint(&ctx.name, rest) {
            say(line);
        }
        return code(if rc == 0 { 1 } else { rc });
    }
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

/// What Hyprland 0.56 and 0.57 answer a `dispatch` in the old syntax under a Lua config, after the
/// Lua error its words made: `dispatch X` runs `hl.dispatch(X)` there (#427).
const LEGACY_NOTE: &[u8] = b"Note: dispatch in lua is a shorthand for hl.dispatch(";

/// The words of each dispatch in hyprctl's ARGS that is not Lua (`hl.…`): the one command, or each
/// of --batch's `;`-separated ones. hyprctl's own options come before the command.
fn old_dispatches(args: &[Vec<u8>]) -> Vec<String> {
    let (mut batch, mut words, mut i) = (false, Vec::new(), 0);
    while i < args.len() {
        let a = &args[i];
        if !words.is_empty() || !a.starts_with(b"-") {
            words.push(String::from_utf8_lossy(a).into_owned());
        } else if a == b"--batch" {
            batch = true;
        } else if a == b"-i" || a == b"--instance" {
            i += 1;
        }
        i += 1;
    }
    let text = words.join(" ");
    let cmds: Vec<&str> = if batch {
        text.split(';').collect()
    } else {
        vec![&text]
    };
    cmds.iter()
        .filter_map(|c| {
            let c = c.trim();
            let (verb, arg) = c.split_once(char::is_whitespace).unwrap_or((c, ""));
            let arg = arg.trim();
            (verb == "dispatch" && !arg.is_empty() && !arg.starts_with("hl.")).then(|| arg.into())
        })
        .collect()
}

/// V as a Lua string: None for one with a control character.
fn lua_str(v: &str) -> Option<String> {
    if v.chars().any(char::is_control) {
        return None;
    }
    Some(format!(
        "\"{}\"",
        v.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// The Lua for an old-syntax dispatch (its words, CMD), for the dispatchers agents reach for and
/// whose translation was checked in a box: None for the rest, and for words they do not take.
fn lua_for(cmd: &str) -> Option<String> {
    let (name, arg) = cmd.split_once(char::is_whitespace).unwrap_or((cmd, ""));
    let arg = arg.trim();
    let win = |sel: &str| -> Option<String> {
        if sel.is_empty() {
            Some(String::new())
        } else {
            Some(format!(", window = {}", lua_str(sel)?))
        }
    };
    let d = match name {
        // fullscreen [0|1] [toggle|set|unset]: 0 the whole screen, 1 maximized; it toggles.
        "fullscreen" => {
            let w: Vec<&str> = arg.split_whitespace().collect();
            let mode = match w.first() {
                None | Some(&"0") => "fullscreen",
                Some(&"1") => "maximized",
                _ => return None,
            };
            let action = match w.get(1) {
                None => String::new(),
                Some(a @ (&"toggle" | &"set" | &"unset")) => format!(", action = \"{a}\""),
                _ => return None,
            };
            if w.len() > 2 {
                return None;
            }
            format!("hl.dsp.window.fullscreen({{ mode = \"{mode}\"{action} }})")
        }
        "togglefloating" | "setfloating" | "settiled" => {
            let a = match name {
                "togglefloating" => "toggle",
                "setfloating" => "enable",
                _ => "disable",
            };
            format!("hl.dsp.window.float({{ action = \"{a}\"{} }})", win(arg)?)
        }
        "workspace" if !arg.is_empty() => {
            format!("hl.dsp.focus({{ workspace = {} }})", lua_str(arg)?)
        }
        "focuswindow" if !arg.is_empty() => {
            format!("hl.dsp.focus({{ window = {} }})", lua_str(arg)?)
        }
        "killactive" if arg.is_empty() => "hl.dsp.window.close()".into(),
        "closewindow" if !arg.is_empty() => {
            format!("hl.dsp.window.close({{ window = {} }})", lua_str(arg)?)
        }
        // movetoworkspace[silent] WS[,SEL]
        "movetoworkspace" | "movetoworkspacesilent" => {
            let (ws, sel) = arg.split_once(',').unwrap_or((arg, ""));
            let ws = ws.trim();
            if ws.is_empty() {
                return None;
            }
            let follow = if name == "movetoworkspacesilent" {
                ", follow = false"
            } else {
                ""
            };
            format!(
                "hl.dsp.window.move({{ workspace = {}{}{follow} }})",
                lua_str(ws)?,
                win(sel.trim())?
            )
        }
        // exec CMD; not exec [RULES] CMD, whose rules go elsewhere in Lua.
        "exec" if !arg.is_empty() && !arg.starts_with('[') => {
            format!("hl.dsp.exec_cmd({})", lua_str(arg)?)
        }
        _ => return None,
    };
    Some(format!("hl.dispatch({d})"))
}

/// What `hyprctl` says after Hyprland's LEGACY_NOTE, for box NAME: one line per old-syntax dispatch
/// in ARGS, with its Lua when lua_for has it, else where the Lua dispatchers are.
fn legacy_hint(name: &[u8], args: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let what = "is the old dispatcher syntax: a box's Hyprland runs a Lua config, which took it as Lua and dispatched nothing";
    let mut cmds = old_dispatches(args);
    if cmds.is_empty() {
        cmds.push(String::new());
    }
    cmds.iter()
        .map(|c| {
            let head = if c.is_empty() {
                format!("hyprctl: that dispatch {what}")
            } else {
                format!("hyprctl: dispatch {c} {what}")
            };
            match lua_for(c) {
                Some(l) => crate::cat!(
                    head,
                    ". Instead: omabox lua -b ",
                    shell::quote_q(name),
                    " ",
                    shell::quote_at(l.as_bytes())
                ),
                None => crate::cat!(
                    head,
                    ". Instead, a Lua dispatcher: omabox lua -b ",
                    shell::quote_q(name),
                    " 'hl.dispatch(hl.dsp.…)' (hl.dsp.window.fullscreen, .float, .close, .move; hl.dsp.focus, hl.dsp.exec_cmd; the omabox skill's reference.md)"
                ),
            }
        })
        .collect()
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
    use super::{legacy_hint, lua_for, notify_text, old_dispatches};

    fn w(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn old_dispatch_words() {
        assert_eq!(
            old_dispatches(&w(&["dispatch", "fullscreen", "0"])),
            ["fullscreen 0"]
        );
        // Options before the command; a word like -1 after it is the command's.
        assert_eq!(
            old_dispatches(&w(&["-j", "-i", "0", "dispatch", "workspace", "-1"])),
            ["workspace -1"]
        );
        // Lua dispatches are not old ones; --batch's are each looked at.
        assert!(old_dispatches(&w(&["dispatch", "hl.dsp.window.close()"])).is_empty());
        assert_eq!(
            old_dispatches(&w(&[
                "--batch",
                "dispatch hl.dsp.focus({ workspace = '1' }) ; dispatch togglefloating ;keyword a b; dispatch  killactive "
            ])),
            ["togglefloating", "killactive"]
        );
        assert!(old_dispatches(&w(&["clients"])).is_empty());
        assert!(old_dispatches(&w(&["dispatch"])).is_empty());
    }

    #[test]
    fn old_dispatch_lua() {
        let l = |c: &str| lua_for(c);
        let d = |s: &str| Some(format!("hl.dispatch({s})"));
        // The issue's: what worked in its box.
        assert_eq!(
            l("fullscreen 0"),
            d(r#"hl.dsp.window.fullscreen({ mode = "fullscreen" })"#)
        );
        assert_eq!(
            l("fullscreen"),
            d(r#"hl.dsp.window.fullscreen({ mode = "fullscreen" })"#)
        );
        assert_eq!(
            l("fullscreen 1"),
            d(r#"hl.dsp.window.fullscreen({ mode = "maximized" })"#)
        );
        assert_eq!(
            l("fullscreen 0 unset"),
            d(r#"hl.dsp.window.fullscreen({ mode = "fullscreen", action = "unset" })"#)
        );
        assert_eq!(l("fullscreen 2"), None);
        assert_eq!(l("fullscreen 0 sometimes"), None);
        assert_eq!(l("fullscreen 0 set x"), None);
        assert_eq!(
            l("togglefloating"),
            d(r#"hl.dsp.window.float({ action = "toggle" })"#)
        );
        assert_eq!(
            l("togglefloating class:foot"),
            d(r#"hl.dsp.window.float({ action = "toggle", window = "class:foot" })"#)
        );
        assert_eq!(
            l("setfloating"),
            d(r#"hl.dsp.window.float({ action = "enable" })"#)
        );
        assert_eq!(
            l("settiled address:0x1"),
            d(r#"hl.dsp.window.float({ action = "disable", window = "address:0x1" })"#)
        );
        assert_eq!(l("workspace 2"), d(r#"hl.dsp.focus({ workspace = "2" })"#));
        assert_eq!(l("workspace"), None);
        assert_eq!(
            l("focuswindow title:^A$"),
            d(r#"hl.dsp.focus({ window = "title:^A$" })"#)
        );
        assert_eq!(l("killactive"), d("hl.dsp.window.close()"));
        assert_eq!(l("killactive now"), None);
        assert_eq!(
            l("closewindow class:foot"),
            d(r#"hl.dsp.window.close({ window = "class:foot" })"#)
        );
        assert_eq!(
            l("movetoworkspace 3"),
            d(r#"hl.dsp.window.move({ workspace = "3" })"#)
        );
        assert_eq!(
            l("movetoworkspacesilent 3, class:foot"),
            d(r#"hl.dsp.window.move({ workspace = "3", window = "class:foot", follow = false })"#)
        );
        assert_eq!(l("movetoworkspace ,class:foot"), None);
        // A " and a \ in the words, escaped for Lua.
        assert_eq!(
            l(r#"exec sh -c "echo \""#),
            d(r#"hl.dsp.exec_cmd("sh -c \"echo \\\"")"#)
        );
        assert_eq!(l("exec [float] foot"), None);
        assert_eq!(l("exec"), None);
        assert_eq!(l("exec a\tb"), None);
        assert_eq!(l("pin"), None);
    }

    #[test]
    fn old_dispatch_hint() {
        let h = |a: &[&str]| -> Vec<String> {
            legacy_hint(b"b1", &w(a))
                .iter()
                .map(|l| String::from_utf8_lossy(l).into_owned())
                .collect()
        };
        assert_eq!(
            h(&["dispatch", "fullscreen", "0"]),
            [
                r#"hyprctl: dispatch fullscreen 0 is the old dispatcher syntax: a box's Hyprland runs a Lua config, which took it as Lua and dispatched nothing. Instead: omabox lua -b b1 'hl.dispatch(hl.dsp.window.fullscreen({ mode = "fullscreen" }))'"#
            ]
        );
        // A word with a ' in it: quoted for the shell.
        assert!(h(&["dispatch", "exec", "echo it's"])[0].ends_with(
            r#"Instead: omabox lua -b b1 'hl.dispatch(hl.dsp.exec_cmd("echo it'\''s"))'"#
        ));
        let g = h(&["--batch", "dispatch pin ; dispatch killactive"]);
        assert_eq!(g.len(), 2);
        assert!(g[0].starts_with("hyprctl: dispatch pin is the old dispatcher syntax"));
        assert!(g[0].ends_with("Instead, a Lua dispatcher: omabox lua -b b1 'hl.dispatch(hl.dsp.…)' (hl.dsp.window.fullscreen, .float, .close, .move; hl.dsp.focus, hl.dsp.exec_cmd; the omabox skill's reference.md)"));
        assert!(g[1].ends_with("Instead: omabox lua -b b1 'hl.dispatch(hl.dsp.window.close())'"));
        // No dispatch found in the words (hyprctl's own forms): the general line.
        assert!(h(&["-q"])[0].starts_with("hyprctl: that dispatch is the old dispatcher syntax"));
    }

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
