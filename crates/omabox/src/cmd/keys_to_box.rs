//! `omabox keys-to-box [-b NAME] [on|off]` (issues #22, #55, finding 117): an interactive box's SUPER
//! keys follow focus and the pointer. On, the host is in the "omabox" submap while this box's window
//! has focus and the pointer is over it (share/passthrough.lua). The state is $D/keys-to-box, which
//! the host's Lua reads on each focus change: in the box dir, which the box cannot see, gone with
//! `down`. Off by default.

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{meta, need_box};
use crate::cat;
use crate::exit::{Ctx, Res, out, say};
use crate::host::{HostSilent, PASS_KEY};
use crate::select::select_box;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut want) = (Vec::new(), Vec::new());
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
                    return Err(ctx.die("keys-to-box: on or off, once"));
                }
                want = a.clone();
            }
            b"-h" | b"--help" => return crate::args::usage("keys-to-box"),
            w if w.starts_with(b"-") => return Err(unknown_opt(ctx, "keys-to-box", a)),
            w => {
                if ctx.in_boxes(w).is_dir() {
                    return Err(unexpected(ctx, "keys-to-box", w, ""));
                }
                return Err(unknown_word(
                    ctx,
                    "keys-to-box",
                    "argument",
                    w,
                    "(on, off, or nothing to see which it is; a box is -b NAME)",
                    &["on", "off"],
                ));
            }
        }
        i += 1;
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if meta(&ctx.d, "mode").0 != b"interactive" {
        return Err(ctx.die(cat!(
            "keys-to-box: box '",
            ctx.name,
            "' is headless: it has no window on your desktop to type into (omabox keys sends it keys)"
        )));
    }
    let f = ctx.at("keys-to-box");
    if want.is_empty() {
        return out(if f.is_file() { "on\n" } else { "off\n" });
    }
    if want == b"on" {
        if let Err(e) = std::fs::write(&f, b"on\n") {
            return Err(ctx.die(cat!(crate::shell::bytes(&f), ": ", crate::shots::why(&e))));
        }
    } else {
        let _ = std::fs::remove_file(&f);
    }
    // Installed again if a reload dropped it, then applied to the window focused now: turned on
    // with the box focused, its keys are the box's at once.
    let host = HostSilent::new(ctx);
    host.passthrough()?;
    host.keys_changed()?;
    if want == b"on" {
        say(cat!(
            "box '",
            ctx.name,
            "': SUPER keys go to it while its window has focus and the pointer is over it (",
            PASS_KEY,
            ": back to your desktop until it loses focus)"
        ));
    } else {
        say(cat!(
            "box '",
            ctx.name,
            "': SUPER keys are your desktop's again (",
            PASS_KEY,
            " still passes them to it, once)"
        ));
    }
    Ok(())
}
