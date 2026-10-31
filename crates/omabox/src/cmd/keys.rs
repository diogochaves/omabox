//! `omabox keys [-b NAME] [--window SEL] [--wait] (COMBO | -t TEXT | --pass VAR | -s DURATION |
//! down:KEY | up:KEY)...`: keys through tools/keyboard (not wtype: finding 13), with the box's own
//! layout, so keycodes mean what Hyprland thinks they mean. --pass types a variable of the caller's
//! (finding 84): its value goes to the tool's stdin, never on a command line.

use crate::args::{optarg, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::cmd::click::sh_e;
use crate::cmd::hyprctl::code;
use crate::exec::{ErrTo, ExecOpts, In, Out, box_exec, caller_env, feed, on_box_io, pipe};
use crate::exit::{Ctx, Exit, Res};
use crate::input::{kb_tool, pause_ms};
use crate::marks::{epoch_us, mark};
use crate::select::select_box;
use crate::settle::{Settle, cursor_now, cursor_rect};
use crate::shell::{self, char_count, cntrl_to_space, head_chars, strip_nl};
use crate::shellnote::shell_note;
use crate::win::{box_windows, jq_r, win_desc, win_raise, win_refocus, win_select};

/// keys_send: the keyboard tool (KB), each --pass value on its stdin, NUL-terminated; a held key
/// (down:/up:, #230) by a holder the tool forks off in the box (finding 343): in its pid namespace,
/// so the holder ends with the box, and no --pass values there (finding 307).
fn keys_send(ctx: &mut Ctx, kb: &[Vec<u8>], pass: &[Vec<u8>], held: bool) -> Res<i32> {
    if held {
        let o = ExecOpts {
            stdin_null: true,
            sys: true,
            ..ExecOpts::default()
        };
        return box_exec(ctx, kb, o);
    }
    let mut data = Vec::new();
    for v in pass {
        data.extend(caller_env(v).unwrap_or_default());
        data.push(0);
    }
    let Ok((r, w)) = pipe() else {
        return Err(ctx.die("could not make a pipe for the keyboard tool"));
    };
    let f = feed(w, data);
    let ran = on_box_io(ctx, kb, In::Fd(r), Out::Inherit, ErrTo::Inherit, None);
    let _ = f.join();
    Ok(ran.or_exit()?.rc)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut name: Vec<u8> = Vec::new();
    let mut toks: Vec<Vec<u8>> = Vec::new();
    let mut sels: Vec<Vec<u8>> = Vec::new();
    let mut pass: Vec<Vec<u8>> = Vec::new();
    let mut st = Settle::default();
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
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(args[i + 1].clone());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            // A value is the tool's, whatever it looks like (-t --wait types "--wait").
            b"-t" => {
                let Some(v) = v else {
                    return Err(ctx.die(cat!("keys: ", a, " needs a value")));
                };
                toks.extend([a.clone(), v.clone()]);
                i += 1;
            }
            // --delay MS (#273, audit F13): refused since 1.0 (#287).
            b"--delay" => {
                return Err(ctx.usage_die(
                    "keys --delay is refused since 1.0: for a pause between keys, -s DURATION (keys a -s 50ms b)",
                ));
            }
            b"-s" => {
                let Some(v) = v else {
                    return Err(ctx.die("keys: -s needs a value"));
                };
                let t = pause_ms(ctx, "keys -s", v)?;
                toks.extend([b"-s".to_vec(), t.to_string().into_bytes()]);
                i += 1;
            }
            b"-m" | b"-p" => {
                return Err(ctx.die(cat!(
                    "keys: ",
                    a,
                    " is omabox's own (to hold a modifier across a click: click, drag or pointer --mod)"
                )));
            }
            b"-T" => {
                return Err(
                    ctx.die("keys: -T is omabox's own (--pass VAR types one of your variables)")
                );
            }
            b"--pass" => {
                optarg(ctx, a, v)?;
                let n = &args[i + 1];
                if !shell::re_match("^[A-Za-z_][A-Za-z0-9_]*$", n) {
                    return Err(ctx.die(cat!("keys: --pass takes a variable name, got ", n)));
                }
                if caller_env(n).is_none() {
                    return Err(ctx.die(cat!(
                        "keys: --pass ",
                        n,
                        ": not in this command's environment (export it first)"
                    )));
                }
                pass.push(n.clone());
                toks.push(b"-T".to_vec());
                i += 1;
            }
            _ => {
                st.opt(ctx, &args[i..])?;
                if st.took != 0 {
                    i += st.took - 1;
                } else if a.len() > 2 && a.starts_with(b"--") {
                    // (an option the tool would take for one of its own: --hold never returns, #273)
                    return Err(unknown_opt(ctx, "keys", a));
                } else {
                    toks.push(a.clone());
                }
            }
        }
        i += 1;
    }
    st.opts_check(ctx, sels.len())?;
    // (audit E8: what to send, by example)
    if toks.is_empty() {
        return Err(ctx.usage_die(
            "keys: nothing to send: a combo (keys super+space), text (keys -t hello), a key (keys Return), or several (keys ctrl+a -t hi Tab)",
        ));
    }
    let held = toks
        .iter()
        .any(|t| t.starts_with(b"down:") || t.starts_with(b"up:"));
    if held && !pass.is_empty() {
        return Err(ctx.die("keys: down:/up: and --pass in calls of their own"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    // --window: that window focused first (finding 81), as a click on it would. A layer that holds
    // the keyboard against any window is refused before anything is sent or focused (#201, finding
    // 276). The active window is focused again: being active does not say it has the keyboard.
    if !sels.is_empty() {
        let wins = box_windows(ctx, None)?;
        let w = win_select(ctx, &wins, &sels)?;
        let kl = crate::desk::kb_layers(ctx);
        if !kl.is_empty() {
            return Err(ctx.die(cat!(
                "keys --window: a layer (",
                kl,
                ") has the keyboard: the keys would go to it, not to ",
                win_desc(&w),
                "; nothing sent (close it first, e.g. keys Escape: twice in a launcher with text in its field)"
            )));
        }
        if jq_r(".active", &w) == b"true" {
            win_refocus(ctx, &w)?;
        } else {
            win_raise(ctx, &w)?;
        }
        // --wait's -g and --ignore: the window's own coordinates, where it is once focused (F8).
        if st.wait {
            // (`$(box_windows)` as an argument: its failure said, then an empty list)
            let wins = box_windows(ctx, None).unwrap_or_default();
            let w = win_select(ctx, &wins, &[jq_r(".address", &w)])?;
            st.in_window(ctx, &jq_r(".at[0]", &w), &jq_r(".at[1]", &w))?;
        }
    }
    let mut kb = kb_tool(ctx)?;
    kb.extend(toks.iter().cloned());
    // Marks for a peek window, once sent: a secret only as its length.
    let mut lines = Vec::new();
    let (mut k, mut pi) = (0, 0);
    while k < toks.len() {
        match toks[k].as_slice() {
            b"-s" => k += 1,
            b"-t" => {
                let t = cntrl_to_space(toks.get(k + 1).map(Vec::as_slice).unwrap_or_default());
                if !t.is_empty() {
                    lines.push(cat!("text ", head_chars(&t, 200)));
                }
                k += 1;
            }
            b"-T" => {
                let t = strip_nl(&pass.get(pi).and_then(|v| caller_env(v)).unwrap_or_default());
                if !t.is_empty() {
                    lines.push(format!("secret {}", char_count(&t)).into_bytes());
                }
                pi += 1;
            }
            t => lines.push(cat!("combo ", t)),
        }
        k += 1;
    }
    if !st.wait {
        let t0 = epoch_us();
        sh_e(keys_send(ctx, &kb, &pass, held)?)?;
        mark(ctx, &t0, false, &lines);
        return Ok(());
    }
    // --wait: the screen before the keys, then a change and quiet after them. The cursor hides on a
    // key press: not a change.
    let p = cursor_now(ctx)?;
    if !p.is_empty() {
        let r = cursor_rect(&p).ok_or(Exit(1))?;
        st.args.extend([b"--ignore".to_vec(), r]);
    }
    st.facts = "keys";
    let mut tool = vec![b"settle".to_vec()];
    tool.extend(st.args.iter().cloned());
    st.begin(ctx, &tool)?;
    let json = st.json;
    st.ready(ctx, json)?;
    let t0 = epoch_us();
    sh_e(keys_send(ctx, &kb, &pass, held)?)?;
    mark(ctx, &t0, false, &lines);
    st.expect_sels = sels;
    code(st.end(ctx, "settle", json, b"the keys were sent")?)
}
