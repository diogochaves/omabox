//! `omabox dnd [-b NAME] [on|off] [--clear]` (#309, finding 611): the box's Do Not Disturb, as its
//! own Omarchy shell keeps it (never the host's). Bare: on or off on stdout. On: new notifications
//! stay off the box's screen (Omarchy lets a critical notify-send, a verification code and its own
//! action toasts through). --clear: the toasts on its screen dismissed, as a click on each would.

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::need_box;
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::omshell;
use crate::select::select_box;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut want, mut clear) = (Vec::new(), Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"on" | b"off" => {
                if !want.is_empty() {
                    return Err(ctx.usage_die("dnd: on or off, once"));
                }
                want = a.clone();
            }
            b"--clear" => clear = true,
            b"-h" | b"--help" => return crate::args::usage("dnd"),
            w if w.starts_with(b"-") => return Err(unknown_opt(ctx, "dnd", a)),
            w => {
                if ctx.in_boxes(w).is_dir() {
                    return Err(unexpected(ctx, "dnd", w, ""));
                }
                return Err(unknown_word(
                    ctx,
                    "dnd",
                    "argument",
                    w,
                    "(on, off, or nothing to see which it is; a box is -b NAME)",
                    &["on", "off"],
                ));
            }
        }
        i += 1;
    }
    ctx.parsed();
    select_box(ctx, &name)?;
    need_box(ctx)?;
    omshell::need_dnd(ctx, "dnd")?;
    if !want.is_empty() {
        let on = want == b"on";
        omshell::dnd_set(ctx, "dnd", on)?;
        say(if on {
            cat!(
                "box '",
                ctx.name,
                "': Do Not Disturb on: new notifications stay off its screen (Omarchy still shows a critical notify-send, a verification code and its own action toasts)"
            )
        } else {
            cat!(
                "box '",
                ctx.name,
                "': Do Not Disturb off: notifications show again"
            )
        });
    }
    if clear {
        let n = omshell::toasts_clear(ctx, "dnd")?;
        say(match n {
            Some(1) => cat!("box '", ctx.name, "': 1 toast cleared"),
            Some(n) => cat!("box '", ctx.name, "': ", n.to_string(), " toasts cleared"),
            None => cat!("box '", ctx.name, "': its toasts cleared"),
        });
    }
    if want.is_empty() && !clear {
        let s = omshell::dnd_state(ctx, "dnd")?;
        return out(cat!(s, "\n"));
    }
    Ok(())
}
