//! `omabox reload [-b NAME]` (#140): the box's copy of the config, from this omabox's share/,
//! reloaded once. Written in the box's mount namespace, as a new file renamed over the old: the
//! runtime dir is the box's to change, and a link it put there resolves inside the box.

use crate::args::name_arg;
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exec::{ErrTo, In, Out, on_box_io};
use crate::exit::{Ctx, Exit, Res, say};
use crate::hypr::{HYPR_LIMIT, hung_text, hypr_down_text, hypr_errors_note, hypr_lost};
use crate::select::select_box;

/// Written only when it differs: in an --autoreload box the write is a reload of its own. $1: the
/// copy's name in the runtime dir.
const WRITE_SH: &str = r#"cfg=$XDG_RUNTIME_DIR/$1; cat > "$cfg.new" &&
    if cmp -s "$cfg.new" "$cfg"; then rm -f "$cfg.new"; else mv -f "$cfg.new" "$cfg"; fi"#;

/// share/FILE written to the box's copy COPY: whether it was.
fn write_copy(ctx: &mut Ctx, file: &str, copy: &str) -> Res<bool> {
    let src = crate::root().join("share").join(file);
    Ok(match std::fs::File::open(&src) {
        Ok(f) => {
            on_box_io(
                ctx,
                &[
                    b"sh".to_vec(),
                    b"-c".to_vec(),
                    WRITE_SH.as_bytes().to_vec(),
                    b"sh".to_vec(),
                    copy.as_bytes().to_vec(),
                ],
                In::Fd(f.into()),
                Out::Inherit,
                ErrTo::Inherit,
                None,
            )
            .or_exit()?
            .rc == 0
        }
        Err(_) => false,
    })
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let name = name_arg(ctx, "reload", args)?;
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let src = crate::root().join("share/hyprland.lua");
    let src_text = crate::shell::bytes(&src).to_vec();
    let n = ctx.name.clone();
    // A plain box's desktop (#247) first, so the two copies are of one omabox: share/plain.lua.
    let plain = crate::upspec::Desktop::of_box(&crate::boxes::meta(&ctx.d, "desktop").0)
        == crate::upspec::Desktop::Plain;
    let wrote = (!plain || write_copy(ctx, "plain.lua", "omabox-plain.lua")?)
        && write_copy(ctx, "hyprland.lua", "omabox-hyprland.lua")?;
    if !wrote {
        return Err(ctx.die(cat!(
            "reload: could not write the config copy in box '",
            n,
            "'"
        )));
    }
    let r = on_box_io(
        ctx,
        &[
            b"timeout".to_vec(),
            HYPR_LIMIT.as_bytes().to_vec(),
            b"hyprctl".to_vec(),
            b"reload".to_vec(),
        ],
        In::Inherit,
        Out::Null,
        ErrTo::Inherit,
        None,
    )
    .or_exit()?;
    if r.rc != 0 {
        if !box_alive(ctx)? {
            return Err(ctx.die(hypr_down_text(ctx)));
        }
        if r.rc == 124 {
            let t = hung_text(ctx, &format!("{HYPR_LIMIT} s"))?;
            return Err(ctx.die(t));
        }
        if let Some(lost) = hypr_lost(ctx)? {
            return Err(ctx.die(lost));
        }
        return Err(ctx.die(format!("reload: hyprctl reload failed (exit {})", r.rc)));
    }
    say(cat!(
        "box '",
        n,
        "': config reloaded from ",
        src_text,
        " (binds, rules and Lua state added since are gone)"
    ));
    if !hypr_errors_note(ctx)? {
        return Err(Exit(1));
    }
    Ok(())
}
