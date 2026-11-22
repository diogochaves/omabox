//! `omabox mode [-b NAME] [--json | WxH[@HZ][,scale=S]|host]`: show or change a headless box's mode
//! live. `hyprctl keyword monitor` fails under the Lua config, so it goes through eval; omabox.mode
//! keeps it over a reload (finding 296 for the scale, #161 for the monitors placed next to it, #163
//! for a main screen away, finding 234 for NVIDIA's labwc output).

use crate::args::{optarg, unknown_opt};
use crate::boxes::{box_alive, box_file, lock_file, need_box};
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Res, out, say};
use crate::hypr::{hung_text, hypr_answers_in};
use crate::jq::{self, Args};
use crate::mon::{
    self, has_monitors, jq_box_json, main_away, main_scale_live, monitors_place, monitors_record,
    parse_mode, size_text, split_scale, w,
};
use crate::num::awk_num;
use crate::screen::box_screen;
use crate::select::select_box;
use crate::settle::{after, past};
use crate::shell;
use std::time::Duration;

/// "NAME MODE" as `${screen%% *}` and `${screen#* }` part it.
pub fn screen_parts(s: &[u8]) -> (&[u8], &[u8]) {
    match s.iter().position(|&c| c == b' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, s),
    }
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut want, mut json) = (Vec::new(), Vec::new(), false);
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
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "mode", a)),
            _ => {
                if !want.is_empty() {
                    return Err(ctx.usage_die(cat!("mode: one mode, got ", want, " and ", a)));
                }
                want = a.clone();
            }
        }
        i += 1;
    }
    if json && !want.is_empty() {
        return Err(ctx.usage_die("mode: --json shows the mode (omabox mode --json), it sets none"));
    }
    // The mode's form, before any box is asked (host's size is read later).
    if !want.is_empty() {
        let (m, _) = split_scale(ctx, "mode", &want)?;
        if m != b"host" && !shell::re_match(r"^[1-9][0-9]*x[1-9][0-9]*(@[0-9]+(\.[0-9]+)?)?$", &m) {
            return Err(ctx.die(cat!(
                "mode: a mode is WxH, WxH@HZ or host, with ,scale=S or not, got ",
                want
            )));
        }
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // Never another monitor's mode while the main screen is away (#163).
    let away = main_away(ctx);
    if !away.is_empty() {
        let a = shell::read_fields(&away, 2).remove(0);
        return Err(ctx.die(cat!(
            "mode: box '",
            ctx.name,
            "''s main screen (",
            a,
            ") is away (omabox output drop): omabox output back ",
            a,
            " -b ",
            ctx.name
        )));
    }
    let mut screen = box_screen(ctx)?;
    // --json (#273, audit J6): the main screen as monitor list --json gives each monitor.
    if want.is_empty() {
        let sc = main_scale_live(ctx)?;
        let sc = if sc.is_empty() { b"1".to_vec() } else { sc };
        if json {
            let (n, m) = screen_parts(&screen);
            let r = Args::new(&["-nc"])
                .kv("--arg", "n", n)
                .kv("--arg", "m", m)
                .kv("--arg", "s", &sc)
                .arg(crate::jqprog::MODE_JSON_JQ)
                .run(None, jq::Err::Inherit);
            out(&r.out)?;
            return crate::cmd::hyprctl::code(r.rc);
        }
        return out(cat!(size_text(&screen, &sc), "\n"));
    }
    if !BoxMeta::load(&ctx.d).headless() {
        return Err(ctx.die(cat!(
            "mode: box '",
            ctx.name,
            "' is interactive: its window's size is its mode"
        )));
    }
    // No scale given is 1, as for up (finding 296).
    let (m, sc) = split_scale(ctx, "mode", &want)?;
    let want = parse_mode(ctx, &m)?;
    // On NVIDIA labwc's output first: the main window, maximized there, follows it (finding 234).
    if BoxMeta::load(&ctx.d).wayland_screen {
        let Some(parent) = mon::parent_display(ctx) else {
            return Err(ctx.die("mode: no usable parent Wayland display"));
        };
        let r = on_box(
            ctx,
            &w(&[
                b"env",
                &cat!("WAYLAND_DISPLAY=", parent),
                b"/usr/bin/wlr-randr",
                b"--output",
                b"HEADLESS-1",
                b"--custom-mode",
                &cat!(want, "Hz"),
            ]),
            false,
            Out::Inherit,
            ErrTo::Inherit,
        )
        .or_exit()?;
        if r.rc != 0 {
            return Err(ctx.die(cat!(
                "mode: could not resize the private parent output to ",
                want
            )));
        }
    }
    let (sname, _) = screen_parts(&screen);
    let lua = cat!(
        "hl.monitor({ output = '",
        sname,
        "', mode = '",
        want,
        "', position = '0x0', scale = ",
        sc,
        " })"
    );
    let r = on_box(
        ctx,
        &w(&[b"hyprctl", b"eval", &lua]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    // (audit E13: whether the box answers at all, as hyprctl says)
    if res != b"ok" {
        if !hypr_answers_in(ctx, "1")? {
            let t = hung_text(ctx, "")?;
            return Err(ctx.die(t));
        }
        return Err(ctx.die(cat!(
            "mode: box '",
            ctx.name,
            "''s Hyprland refused ",
            want,
            ": ",
            res,
            " (omabox log -b ",
            ctx.name,
            " hyprland)"
        )));
    }
    let deadline = after(5);
    let mut live: Vec<u8> = Vec::new();
    loop {
        screen = box_screen(ctx)?;
        if screen_parts(&screen).1 == want.as_slice() {
            live = main_scale_live(ctx)?;
            let d = awk_num(&live) - awk_num(&sc);
            if d < 0.2 && d > -0.2 {
                break;
            }
        }
        if past(deadline) {
            let l = if live.is_empty() {
                b"?".to_vec()
            } else {
                live.clone()
            };
            return Err(ctx.die(cat!(
                "mode: asked for ",
                size_text(&want, &sc),
                ", box reports ",
                size_text(screen_parts(&screen).1, &l)
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // As Hyprland made it: it may round a scale to one that divides the screen into whole pixels.
    if live != sc {
        say(cat!(
            "mode: scale ",
            sc,
            " is ",
            live,
            " in the box (Hyprland rounds it to whole pixels)"
        ));
    }
    let sc = live;
    // Kept only once the box has it: omabox.mode for a config reload, box.json for `ls`, under the
    // box's lock as take_over writes it (finding 93).
    if !box_file(ctx, &ctx.at("run/omabox.mode"), &cat!(want, " ", sc)) {
        return Err(ctx.die(cat!(
            "mode: could not record ",
            want,
            " for the box's config reloads"
        )));
    }
    let Some(lock) = lock_file(
        &ctx.in_boxes(cat!(".lock-", ctx.name)),
        Duration::from_secs(60),
    ) else {
        return Err(ctx.die(cat!(
            "mode: box '",
            ctx.name,
            "' is busy (an up or down); ",
            want,
            " is set, but ls will not show it"
        )));
    };
    if !box_alive(ctx)? {
        return Err(ctx.die(cat!("mode: box '", ctx.name, "' went down meanwhile")));
    }
    if !jq_box_json(
        ctx,
        Args::new(&[])
            .kv("--arg", "s", &want)
            .kv("--argjson", "sc", &sc)
            .arg(crate::jqprog::MODE_RECORD_JQ),
    ) {
        return Err(ctx.die(cat!(
            "mode: ",
            want,
            " is set, but ls will not show it (box.json not written)"
        )));
    }
    drop(lock);
    // Its other monitors placed right of or below it move with its new size (#161).
    if has_monitors(ctx) {
        let Ok(moved) = monitors_place(ctx) else {
            return Err(ctx.die(cat!(
                "mode: ",
                want,
                " is set, but the monitors placed next to it were not moved (omabox monitor list -b ",
                ctx.name,
                ")"
            )));
        };
        if !moved.is_empty() {
            say(cat!("mode: moved the monitors placed next to it: ", moved));
        }
        monitors_record(ctx)?;
    }
    out(cat!(size_text(&screen, &sc), "\n"))
}
