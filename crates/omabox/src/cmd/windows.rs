//! `omabox windows [-b NAME] [--json]`: the box's mapped windows (finding 81), bottom to top as they
//! stack, where each is, what covers it, and the layers over them (#263, findings 398, 421).

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::{meta, need_box};
use crate::cat;
use crate::exit::{Ctx, Res, out};
use crate::jq;
use crate::select::select_box;
use crate::shellnote::shell_note;
use crate::win::box_windows;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut json) = (Vec::new(), false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "windows", a)),
            _ => return Err(unexpected(ctx, "windows", a, "")),
        }
        i += 1;
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    // (a dock hidden on its workspace taken out: it takes no input there, #309)
    let lys = crate::omshell::input_layers(ctx)?;
    let wins = box_windows(ctx, Some(&lys))?;
    if json {
        let r = jq::jq(
            &["-c", crate::jqprog::WINDOWS_JSON_JQ],
            Some(&wins),
            jq::Err::Inherit,
        );
        out(&r.out)?;
        return crate::cmd::hyprctl::code(r.rc);
    }
    // A box on a Hyprland build of its own (finding 116) says so first; one whose shell runs a
    // Quickshell build (finding 358) too: its layer surfaces are not listed here.
    let built = |key: &str, ver: &str, what: &str| -> Res<()> {
        let b = meta(&ctx.d, key).0;
        if b.is_empty() {
            return Ok(());
        }
        let v = meta(&ctx.d, ver).0;
        let v = if v.is_empty() {
            Vec::new()
        } else {
            cat!(" (", v, ")")
        };
        out(cat!("box ", ctx.name, " ", what, " ", b, v, "\n"))
    };
    built("hyprland", "hyprland_version", "runs Hyprland")?;
    built(
        "quickshell",
        "quickshell_version",
        "runs its shell on Quickshell",
    )?;
    // Bottom to top, as they stack.
    out(format!(
        "{:<16} {:<8} {:<10} {:<20} {:<22} {:<16} {}\n",
        "ADDRESS", "PID", "WORKSPACE", "STATE", "GEOMETRY", "CLASS", "TITLE"
    ))?;
    let t = jq::jq(
        &["-r", crate::jqprog::WINDOWS_TABLE_JQ],
        Some(&wins),
        jq::Err::Inherit,
    );
    out(&t.out)?;
    if !t.ok() {
        return crate::cmd::hyprctl::code(t.rc);
    }
    let l = jq::jq(
        &["-r", crate::jqprog::WINDOWS_LAYERS_JQ],
        Some(&wins),
        jq::Err::Inherit,
    );
    out(&l.out)?;
    crate::cmd::hyprctl::code(l.rc)
}
