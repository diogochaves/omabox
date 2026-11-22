//! `omabox monitor [-b NAME] (add SPEC [--name N] | remove NAME | list [--json])` (#122): more
//! monitors in a box. A headless box's is a headless output its Hyprland makes (on NVIDIA a Wayland
//! output, a window on its private labwc: #165, finding 234), kept in omabox.monitors, which the
//! box's config reads on every load and places them from; an interactive box's a window on the
//! user's desktop (#123, #174: deskwin.rs). The main screen is never removed here.

use crate::args::{optarg, unknown_opt, unknown_word};
use crate::boxes::need_box;
use crate::boxmeta::BoxMeta;
use crate::cat;
use crate::deskwin::{desk_prune, desk_relay, monitor_add_window};
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::hypr::{HYPR_LIMIT, hypr_json};
use crate::jq::{self, Args};
use crate::mon::{
    self, dropped_file, dropped_rec, labwc_apply, line_starts, monitor_place, monitor_spec,
    monitors_file, monitors_place, monitors_record, monitors_without, monitors_write, pad, w,
    with_line,
};
use crate::num::awk_num;
use crate::screen::main_screen;
use crate::select::select_box;
use crate::settle::{after, past};
use crate::shell::{self, strip_nl};
use std::time::Duration;

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut act, mut rest, mut mname, mut json) = (
        Vec::new(),
        None::<Vec<u8>>,
        Vec::<Vec<u8>>::new(),
        Vec::new(),
        false,
    );
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, args.get(i + 1))?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"--name" => {
                optarg(ctx, a, args.get(i + 1))?;
                mname = args[i + 1].clone();
                i += 1;
            }
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("monitor"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "monitor", a)),
            _ => {
                // (`[ -z "$act" ]`: an empty first word leaves the place open)
                if act.as_ref().is_none_or(Vec::is_empty) {
                    act = Some(a.clone());
                } else {
                    rest.push(a.clone());
                }
            }
        }
        i += 1;
    }
    let act = act.unwrap_or_default();
    match act.as_slice() {
        b"add" => {
            if rest.len() != 1 {
                return Err(
                    ctx.usage_die("monitor add: one SPEC, WxH[@HZ][,scale=S][,right|below|X,Y]")
                );
            }
            if monitor_spec(&rest[0]).is_err() {
                return Err(ctx.refused());
            }
        }
        b"remove" => {
            if rest.len() != 1 || !mname.is_empty() {
                return Err(ctx.usage_die(
                    "monitor remove: which? one monitor's name, no --name (omabox monitor list names them)",
                ));
            }
            if !shell::re_match("^[A-Za-z0-9_-]{1,32}$", &rest[0]) {
                return Err(ctx.usage_die(cat!(
                    "monitor remove: not a monitor's name: ",
                    rest[0],
                    " (omabox monitor list names them)"
                )));
            }
        }
        b"list" => {
            if !rest.is_empty() {
                return Err(ctx.usage_die(cat!(
                    "monitor list takes no arguments, got ",
                    rest.join(&b' ')
                )));
            }
        }
        b"" => return Err(ctx.usage_die("monitor: add SPEC, remove NAME or list")),
        _ => {
            return Err(unknown_word(
                ctx,
                "monitor",
                "subcommand",
                &act,
                "(add SPEC, remove NAME or list)",
                &["add", "remove", "list"],
            ));
        }
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    if act == b"list" {
        return list(ctx, json);
    }
    let n = ctx.name.clone();
    let interactive = BoxMeta::load(&ctx.d).interactive();
    if interactive {
        let fixed = Args::new(&["-r", ".aquamarine.fixed"])
            .arg(cat!(ctx.d, "/box.json"))
            .run_file(jq::Err::Inherit)
            .text();
        if fixed == b"false" {
            return Err(ctx.die(cat!(
                "monitor: box '",
                n,
                "' opens a window per monitor, which needs aquamarine's fix (hyprwm/aquamarine#415): omabox setup --aquamarine, then a new box"
            )));
        }
        // Each monitor's window a class of its own, its rule's alone (finding 429): omabox's build
        // only.
        if act == b"add" {
            let aq = Args::new(&["-r", crate::jqprog::AQ_VERSION_JQ])
                .arg(cat!(ctx.d, "/box.json"))
                .run_file(jq::Err::Inherit)
                .text();
            if !crate::exit::contains(&aq, b"+appid") {
                return Err(ctx.die(cat!(
                    "monitor add: box '",
                    n,
                    "''s aquamarine (",
                    aq,
                    ") cannot give a monitor's window a class of its own, which keeps that window off your workspace (it lacks omabox's patch): omabox setup --aquamarine, then a new box"
                )));
            }
            // ...and its Hyprland asked for it: an older omabox's `up` started it without
            // AQ_WAYLAND_APP_ID_FILES (share/start-hyprland.sh leaves this file when it sets it).
            if !ctx.at("run/omabox.appid-files").is_file() {
                return Err(ctx.die(cat!(
                    "monitor add: box '",
                    n,
                    "' was started by an omabox that does not give a monitor's window a class of its own, which keeps that window off your workspace: start a new box with this omabox"
                )));
            }
        }
    } else if act == b"add" && BoxMeta::load(&ctx.d).wayland_screen {
        // A box started before #165 runs labwc with no config of omabox's.
        if !ctx.at("run/labwc/rc.xml").is_file() {
            return Err(ctx.die(cat!(
                "monitor add: box '",
                n,
                "' draws on NVIDIA and was started by an omabox without monitors there (finding 234): start a new box"
            )));
        }
        if !mname.is_empty() {
            return Err(ctx.die(
                "monitor add: an NVIDIA box's monitors are Wayland outputs, WAYLAND-N as its screen is: no --name",
            ));
        }
    }
    if act == b"add" {
        let line = if interactive {
            if !mname.is_empty() {
                return Err(ctx.die(
                    "monitor add: an interactive box's monitors are named by its windows (WAYLAND-N): no --name",
                ));
            }
            // One add at a time on a box (finding 429): two at once would both take the next free
            // WAYLAND-N. Under the box's lock, as up and down are: its window made, or the add
            // given up, first.
            let Some(_lock) = crate::boxes::lock_file(
                &ctx.in_boxes(cat!(".lock-", ctx.name)),
                Duration::from_secs(60),
            ) else {
                return Err(ctx.die(cat!(
                    "monitor add: box '",
                    n,
                    "' is busy (another monitor add, an up or a down); try again"
                )));
            };
            // (gone while it waited: says so)
            if !crate::boxes::box_alive(ctx)? {
                need_box(ctx)?;
            }
            // (any INT ends the add, its trap removing the output: finding 437; none of its steps
            // is one a Ctrl-C lets go on, trap.rs)
            monitor_add_window(ctx, &rest[0])?
        } else {
            monitor_add(ctx, &rest[0], &mname)?
        };
        say(cat!("box '", ctx.name, "' has a monitor ", line));
        let first = line.split(|&c| c == b' ').next().unwrap_or_default();
        return out(cat!(first, "\n"));
    }
    remove(ctx, &rest[0], interactive)
}

fn list(ctx: &Ctx, json: bool) -> Res<()> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    // The ones `output drop` took away too, as they were, marked dropped (#163).
    let dr = jq::tool(
        "jq",
        &["-Rn", crate::jqprog::LIST_DROPPED_JQ],
        Some(&dropped_file(ctx)),
        jq::Err::Inherit,
    );
    crate::cmd::hyprctl::code(dr.rc)?;
    let dr = dr.text();
    if json {
        let r = Args::new(&["-c"])
            .kv("--argjson", "dr", &dr)
            .arg(crate::jqprog::LIST_JSON_JQ)
            .run(Some(&m), jq::Err::Inherit);
        out(&r.out)?;
        crate::cmd::hyprctl::code(r.rc)?;
    } else {
        let row = |f: [&[u8]; 5]| -> Vec<u8> {
            cat!(
                pad(f[0], 14),
                " ",
                pad(f[1], 16),
                " ",
                pad(f[2], 6),
                " ",
                pad(f[3], 11),
                " ",
                f[4],
                "\n"
            )
        };
        out(row([b"NAME", b"MODE", b"SCALE", b"POSITION", b"WORKSPACE"]))?;
        let r = Args::new(&["-r"])
            .kv("--argjson", "dr", &dr)
            .arg(crate::jqprog::LIST_TABLE_JQ)
            .run(Some(&m), jq::Err::Inherit);
        // (`| while read -r a b c d ws`: each line, the last word taking the rest)
        for l in mon::records(&r.out) {
            let f = shell::read_fields(l, 5);
            out(row([&f[0], &f[1], &f[2], &f[3], &f[4]]))?;
        }
        crate::cmd::hyprctl::code(r.rc)?;
    }
    let zn = crate::shots::zero_note(&m, BoxMeta::load(&ctx.d).wayland_screen, None);
    if !zn.is_empty() {
        say(zn);
    }
    Ok(())
}

fn remove(ctx: &mut Ctx, o: &[u8], interactive: bool) -> Res<()> {
    let n = ctx.name.clone();
    if !dropped_rec(ctx, o).is_empty() {
        return Err(ctx.die(cat!(
            "monitor remove: ",
            o,
            " is dropped (omabox output back ",
            o,
            " -b ",
            n,
            ", then remove it)"
        )));
    }
    let main = match hypr_json(ctx, &["monitors"]) {
        Ok(m) => jq::tool(
            "jq",
            &["-r", &format!("{}.name", main_screen(ctx))],
            Some(&m),
            jq::Err::Inherit,
        )
        .text(),
        Err(_) => Vec::new(),
    };
    if o == main.as_slice() {
        return Err(ctx.die(cat!(
            "monitor remove: ",
            o,
            " is the box's main screen (omabox output drop takes it away for a while)"
        )));
    }
    let mut m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    if !crate::cmd::output::listed(&m, o) {
        let names = jq::jq(
            &["-r", crate::jqprog::MON_NAMES_JQ],
            Some(&m),
            jq::Err::Inherit,
        )
        .text();
        return Err(ctx.die(cat!(
            "monitor remove: box '",
            n,
            "' has no monitor ",
            o,
            " (",
            names,
            ")"
        )));
    }
    if !monitors_write(ctx, &monitors_without(ctx, o)) {
        return Err(ctx.die("monitor remove: could not update the box's record"));
    }
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"output",
            b"remove",
            o,
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res != b"ok" {
        return Err(ctx.die(cat!("monitor remove: removing ", o, " failed: ", res)));
    }
    let end = after(5);
    loop {
        if let Ok(x) = hypr_json(ctx, &["monitors"]) {
            m = strip_nl(&x);
            if !crate::cmd::output::listed(&m, o) {
                break;
            }
        }
        if past(end) {
            return Err(ctx.die(cat!(
                "monitor remove: ",
                o,
                " still listed 5 s after its removal"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // An interactive box's windows: the picture laid out again without it (#174).
    if interactive {
        desk_prune(ctx)?;
        desk_relay(ctx, b"")?;
    }
    // One placed right of or below it now goes next to the one before it, as a reload would.
    let Ok(moved) = monitors_place(ctx) else {
        return Err(ctx.die(cat!(
            "monitor remove: ",
            o,
            " removed, but the monitors placed next to it were not moved (omabox monitor list -b ",
            n,
            ")"
        )));
    };
    // Its screens at their new scales before it returns (#382, finding 519).
    if interactive && !crate::mon::scales_settled(ctx, 5) {
        say(cat!(
            "box '",
            n,
            "': its screens were not at their new scales 5 s after ",
            o,
            " went (omabox monitor list -b ",
            n,
            ")"
        ));
    }
    monitors_record(ctx)?;
    let moved = if moved.is_empty() {
        Vec::new()
    } else {
        cat!("; moved ", moved)
    };
    say(cat!(
        "box '",
        n,
        "': ",
        o,
        " removed (its workspaces moved to the other monitors, as on an unplug)",
        moved
    ));
    Ok(())
}

/// `$(monitor_add SPEC NAME)`: a new monitor (NAME, or the next free HEADLESS-N; on NVIDIA
/// WAYLAND-N, a Wayland output: labwc_rc), placed and made; its line as Hyprland made it ("NAME MODE
/// XxY SCALE"). The scale always set, never left to Hyprland (a new output with no rule came up at
/// 2). Errexit off in it, as in bash's `$(...)`: only its refusals end it.
pub(crate) fn monitor_add(ctx: &mut Ctx, spec: &[u8], given: &[u8]) -> Res<Vec<u8>> {
    let Ok([mode, scale, place]) = monitor_spec(spec) else {
        return Err(Exit(1));
    };
    let (kind, prefix, mut i): (&[u8], &str, i64) = if BoxMeta::load(&ctx.d).wayland_screen {
        (b"wayland", "WAYLAND-", 2)
    } else {
        (b"headless", "HEADLESS-", 3)
    };
    let mut m = hypr_json(ctx, &["monitors"])
        .map(|m| strip_nl(&m))
        .unwrap_or_default();
    let n = ctx.name.clone();
    let name = if given.is_empty() {
        loop {
            let c = format!("{prefix}{i}");
            let both = cat!(monitors_file(ctx), dropped_file(ctx));
            if !(crate::cmd::output::listed(&m, c.as_bytes()) || line_starts(&both, c.as_bytes())) {
                break c.into_bytes();
            }
            i += 1;
        }
    } else {
        if !shell::re_match("^[A-Za-z0-9_-]{1,32}$", given) {
            return Err(ctx.die(cat!(
                "monitor add: --name is letters, digits, - and _, got ",
                given
            )));
        }
        // The main screen's names (#163 again) and Hyprland's stand-in output's.
        if matches!(
            given,
            b"HEADLESS-1" | b"HEADLESS-2" | b"WAYLAND-1" | b"FALLBACK"
        ) {
            return Err(ctx.die(cat!(
                "monitor add: ",
                given,
                " is a name of the box's main screen (or Hyprland's stand-in output), not for a monitor"
            )));
        }
        if crate::cmd::output::listed(&m, given) {
            return Err(ctx.die(cat!(
                "monitor add: box '",
                n,
                "' already has a monitor ",
                given
            )));
        }
        if !dropped_rec(ctx, given).is_empty() {
            return Err(ctx.die(cat!(
                "monitor add: box '",
                n,
                "' already has a monitor ",
                given,
                ", dropped (omabox output back ",
                given,
                " -b ",
                n,
                ")"
            )));
        }
        given.to_vec()
    };
    // Written first, as asked: the box's config places it from there.
    let rest = strip_nl(&monitors_file(ctx));
    let line = cat!(name, " ", mode, " ", monitor_place(&place), " ", scale);
    if !monitors_write(ctx, &with_line(&rest, &line)) {
        return Err(ctx.die(cat!(
            "monitor add: could not record ",
            name,
            " for the box's config reloads"
        )));
    }
    // On NVIDIA labwc's rule for its window first, to be there when it maps.
    if kind != b"headless" && !labwc_apply(ctx)? {
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: could not give box '",
            n,
            "''s labwc the size of ",
            name,
            "'s window"
        )));
    }
    // Its rule set before its output exists (#379, finding 510): an output with no rule of its own
    // comes up on Hyprland's hardcoded one, at scale 2, until the place below sets it; a Wayland one
    // (NVIDIA) at 1280x720 too, aquamarine's size for a window not configured yet. A 1280x720 SPEC
    // at another scale was read in that gap as made at 2, and kept so. (Its result unread: the place
    // below says why one failed.)
    on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"eval",
            b"omabox_place_monitors()",
        ]),
        false,
        Out::Null,
        ErrTo::Null,
    )
    .or_exit()?;
    let r = on_box(
        ctx,
        &w(&[
            b"timeout",
            HYPR_LIMIT.as_bytes(),
            b"hyprctl",
            b"output",
            b"create",
            kind,
            &name,
        ]),
        false,
        Out::Capture(None),
        ErrTo::Out,
    );
    let res = shell::subst(&r.out);
    if res != b"ok" {
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!("monitor add: creating ", name, " failed: ", res)));
    }
    if monitors_place(ctx).is_err() {
        on_box(
            ctx,
            &w(&[b"hyprctl", b"output", b"remove", &name]),
            false,
            Out::Null,
            ErrTo::Null,
        )
        .or_exit()?;
        monitors_write(ctx, &rest);
        return Err(ctx.die(cat!(
            "monitor add: ",
            name,
            " was made, but setting its mode failed"
        )));
    }
    let end = after(5);
    let wmode = mode
        .split(|&c| c == b'@')
        .next()
        .unwrap_or_default()
        .to_vec();
    let made;
    let mut seen = Vec::new();
    loop {
        if let Ok(x) = hypr_json(ctx, &["monitors"]) {
            m = strip_nl(&x);
            let r = Args::new(&["-er"])
                .kv("--arg", "n", &name)
                .kv("--arg", "w", &wmode)
                .arg(crate::jqprog::ADD_LINE_JQ)
                .run(Some(&m), jq::Err::Null);
            // At its size and at the scale asked (#379): Hyprland may round a scale to whole pixels
            // (within 0.2, as `mode` takes it), never make it another.
            if r.ok() {
                let f = shell::read_fields(&r.text(), 4);
                if (awk_num(&f[3]) - awk_num(&scale)).abs() < 0.2 {
                    made = r.text();
                    break;
                }
                seen = mon::size_text(&f[1], &f[3]);
            }
        }
        if past(end) {
            if seen.is_empty() {
                return Err(ctx.die(cat!(
                    "monitor add: ",
                    name,
                    " did not come up as ",
                    mode,
                    " (omabox monitor list -b ",
                    n,
                    ")"
                )));
            }
            // At another scale: not kept (a reload would keep it so), the record as it was.
            on_box(
                ctx,
                &w(&[b"hyprctl", b"output", b"remove", &name]),
                false,
                Out::Null,
                ErrTo::Null,
            )
            .or_exit()?;
            monitors_write(ctx, &rest);
            return Err(ctx.die(cat!(
                "monitor add: ",
                name,
                " did not come up as ",
                mon::size_text(&mode, &scale),
                ": Hyprland made it ",
                seen,
                " (omabox log -b ",
                n,
                " hyprland)"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Kept with its mode and scale as Hyprland made them (a scale may be rounded): the next one is
    // placed by those.
    let f = shell::read_fields(&made, 4);
    let line = cat!(name, " ", f[1], " ", monitor_place(&place), " ", f[3]);
    if !monitors_write(ctx, &with_line(&rest, &line)) {
        return Err(ctx.die(cat!(
            "monitor add: could not record ",
            name,
            " for the box's config reloads"
        )));
    }
    monitors_record(ctx)?;
    Ok(made)
}
