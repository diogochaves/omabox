//! `omabox click [-b NAME] [--window SEL [--no-raise] | --in SHOT] [--wait] X Y [BTN] [--double]
//! [--steps N | --glide DURATION] [--mod MODS]`, `omabox click ... --find TEXT [--window SEL | --layer
//! SEL] [--index N] [BTN]` and `omabox scroll ... X Y DY [DX] [--source ...]`
//! (#134, finding 213): the pointer there, then a click or a scroll. X Y are screen pixels; with
//! --window the window's own (finding 81), with --in a shot's image's. --steps travels there from
//! where the pointer is (#38), --glide in that time (#176), --mod holds modifiers down (#25), --wait
//! watches what it does (finding 82). --find (#318) clicks the middle of what the accessibility tree
//! names TEXT, read fresh by omabox-tree --click, with the real pointer as X Y would.

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::cmd::hyprctl::code;
use crate::cmd::tree;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, say};
use crate::input::{
    BOX_POINTER, glide_arg, glide_to, mods_add, num, on_screen, pointer_at, pointer_marks,
    steps_arg, steps_to, wheel_note, with_mods, xy,
};
use crate::marks::{epoch_us, mark, rec_glide, rec_input};
use crate::num::awk_num;
use crate::screen::{Point, extent, shot_point};
use crate::select::select_box;
use crate::settle::{Settle, cursor_now, cursor_rect};
use crate::shell;
use crate::shellnote::shell_note;
use crate::win::{Raise, jq_r, screen_layer, win_point};

/// A button word.
fn button(w: &[u8]) -> bool {
    matches!(
        w,
        b"left" | b"right" | b"middle" | b"side" | b"extra" | b"back" | b"forward"
    )
}

/// `on_box "$BOX_POINTER" --extent EXT SEQ...`: the pointer tool's status (a refusal ends the
/// command).
pub fn pointer_tool(ctx: &Ctx, ext: &[u8], seq: &[Vec<u8>]) -> Res<i32> {
    let mut a = vec![
        BOX_POINTER.as_bytes().to_vec(),
        b"--extent".to_vec(),
        ext.to_vec(),
    ];
    a.extend(seq.iter().cloned());
    Ok(on_box(ctx, &a, false, Out::Inherit, ErrTo::Inherit)
        .or_exit()?
        .rc)
}

/// Under set -e: a status that is not 0 ends the command with it.
pub fn sh_e(rc: i32) -> Res<()> {
    code(rc)
}

/// X Y mapped to the screen (click's and scroll's): from a shot's image (--in), then from a
/// window's coordinates (win_point, which raises it unless --no-raise); with --window, --wait's -g
/// and --ignore moved as X Y is (F8).
fn to_screen(
    ctx: &Ctx,
    st: &mut Settle,
    raise: bool,
    inp: &[u8],
    sels: &mut Vec<Vec<u8>>,
    x: i64,
    y: i64,
) -> Res<(i64, i64)> {
    let (mut x, mut y) = (x, y);
    if !inp.is_empty() {
        match shot_point(ctx, inp, &num(x), &num(y))? {
            Point::Window(a, wx, wy) => {
                (x, y) = (wx, wy);
                *sels = vec![a];
            }
            Point::Screen(sx, sy) => (x, y) = (sx, sy),
        }
    }
    if !sels.is_empty() {
        let r = if raise { Raise::Yes } else { Raise::No };
        let p = win_point(ctx, r, &num(x), &num(y), sels)?;
        let (px, py) = xy(&p);
        let px = crate::num::read(ctx, "the window's point", &px)?;
        let py = crate::num::read(ctx, "the window's point", &py)?;
        if st.window {
            st.in_window(ctx, &num(px.saturating_sub(x)), &num(py.saturating_sub(y)))?;
        }
        (x, y) = (px, py);
    } else {
        // The screen's X Y: a layer over the window there said as --window says it (#361).
        screen_layer(ctx, &num(x), &num(y));
    }
    Ok((x, y))
}

/// --find TEXT: where omabox-tree --click says the one node named TEXT is, in the window SELS picks
/// (none: the active one; to the screen as --window maps X Y, SELS set to it) or on the layer LAYER
/// (the screen's coordinates already); its line said.
fn found(
    ctx: &mut Ctx,
    st: &mut Settle,
    raise: bool,
    sels: &mut Vec<Vec<u8>>,
    layer: &[u8],
    find: &[u8],
    index: Option<i64>,
) -> Res<(i64, i64)> {
    let mut targs = vec![b"--click".to_vec(), find.to_vec()];
    if let Some(n) = index {
        targs.extend([b"--index".to_vec(), num(n)]);
    }
    let r = tree::read(ctx, "click", sels, layer, targs, true)?;
    match r.rc {
        0 => {}
        1 | 9 => {
            say(cat!("click: ", r.what, ": ", r.why));
            return Err(Exit(1));
        }
        8 => {
            say(cat!("click: ", r.what, ": ", r.why));
            return Err(Exit(2));
        }
        _ => {
            tree::fail(ctx, "click", &r)?;
            return Err(Exit(1));
        }
    }
    let (first, line) = match r.out.iter().position(|&c| c == b'\n') {
        Some(i) => (&r.out[..i], shell::subst(&r.out[i + 1..])),
        None => (&r.out[..], Vec::new()),
    };
    let (wx, wy) = xy(first);
    let (Some(x), Some(y)) = (shell::dec(&wx), shell::dec(&wy)) else {
        return Err(ctx.die(cat!("click: omabox-tree gave no point (", r.out, ")")));
    };
    say(cat!("click: ", line, ": at ", num(x), ",", num(y)));
    if !layer.is_empty() {
        return Ok((x, y));
    }
    *sels = vec![jq_r(".address", &r.win)];
    to_screen(ctx, st, raise, &[], sels, x, y)
}

/// --wait: the cursor where it is and where it goes is not the change looked for.
fn ignore_cursor(ctx: &Ctx, st: &mut Settle, x: i64, y: i64) -> Res<()> {
    let p = cursor_now(ctx)?;
    if !p.is_empty() {
        let r = cursor_rect(&p).ok_or(Exit(1))?;
        st.args.extend([b"--ignore".to_vec(), r]);
    }
    let r = cursor_rect(&cat!(num(x), " ", num(y))).ok_or(Exit(1))?;
    st.args.extend([b"--ignore".to_vec(), r]);
    Ok(())
}

/// The watch begun and `ready` read: SETTLE_FACTS and SETTLE_FXY set first.
fn watch(ctx: &Ctx, st: &mut Settle, facts: &'static str, x: i64, y: i64) -> Res<()> {
    st.facts = facts;
    st.fxy = cat!(num(x), " ", num(y));
    let mut tool = vec![b"settle".to_vec()];
    tool.extend(st.args.iter().cloned());
    st.begin(ctx, &tool)?;
    let json = st.json;
    st.ready(ctx, json)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut btn, mut double, mut xyv) =
        (Vec::new(), b"left".to_vec(), false, Vec::new());
    let (mut sels, mut raise, mut inp) = (Vec::new(), true, Vec::new());
    let (mut steps, mut mods, mut glide): (Option<i64>, Vec<u8>, Option<i64>) =
        (None, Vec::new(), None);
    let (mut find, mut layer, mut index) = (Vec::new(), Vec::new(), None);
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
            b"--in" => {
                optarg(ctx, a, v)?;
                inp = args[i + 1].clone();
                i += 1;
            }
            b"--no-raise" => raise = false,
            b"--double" => double = true,
            b"--find" => {
                optarg(ctx, a, v)?;
                if args[i + 1].is_empty() {
                    return Err(ctx.die("click: --find takes a name (any case, or part of one)"));
                }
                find = args[i + 1].clone();
                i += 1;
            }
            b"-l" | b"--layer" => {
                optarg(ctx, a, v)?;
                layer = args[i + 1].clone();
                i += 1;
            }
            b"--index" => {
                optarg(ctx, a, v)?;
                let n = &args[i + 1];
                if !shell::re_match("^[1-9][0-9]{0,3}$", n) {
                    return Err(ctx.die(cat!("click: --index is 1 or more, got ", n)));
                }
                index = shell::dec(n);
                i += 1;
            }
            b"--steps" => {
                optarg(ctx, a, v)?;
                steps = Some(steps_arg(ctx, "click", &args[i + 1])?);
                i += 1;
            }
            b"--glide" => {
                optarg(ctx, a, v)?;
                glide = Some(glide_arg(ctx, "click", &args[i + 1])?);
                i += 1;
            }
            b"--mod" => {
                optarg(ctx, a, v)?;
                mods = mods_add(ctx, &mods, &args[i + 1])?;
                i += 1;
            }
            w if button(w) => btn = w.to_vec(),
            w if w == b"-g" || w.starts_with(b"--") => {
                st.opt(ctx, &args[i..])?;
                if st.took == 0 {
                    return Err(unknown_opt(ctx, "click", a));
                }
                i += st.took - 1;
            }
            w => {
                if !shell::is_num(w) {
                    return Err(unexpected(
                        ctx,
                        "click",
                        w,
                        "(X Y in whole pixels, then a button: left, right, middle, side, extra, back or forward)",
                    ));
                }
                xyv.push(shell::dec(w).unwrap_or(0));
            }
        }
        i += 1;
    }
    // --find without --window clicks in the active window: its coordinates, as --window's.
    let on_window = sels.len() + usize::from(!find.is_empty() && layer.is_empty());
    st.opts_check(ctx, on_window)?;
    if find.is_empty() {
        if !layer.is_empty() || index.is_some() {
            return Err(ctx.die("click: --layer and --index go with --find TEXT"));
        }
        if xyv.len() != 2 {
            let s = if xyv.len() == 1 { "" } else { "s" };
            return Err(ctx.die(format!("click: need X Y, got {} number{s}", xyv.len())));
        }
    } else if !xyv.is_empty() {
        return Err(ctx.die("click: --find TEXT or X Y, not both"));
    } else if !inp.is_empty() {
        return Err(ctx.die("click: --find TEXT or --in SHOT, not both"));
    } else if !layer.is_empty() && !sels.is_empty() {
        return Err(ctx.die("click: --window or --layer, not both"));
    }
    if !inp.is_empty() && !sels.is_empty() {
        return Err(ctx.die("click: --in or --window, not both"));
    }
    if !raise && inp.is_empty() && sels.is_empty() && (find.is_empty() || !layer.is_empty()) {
        return Err(ctx.die("click: --no-raise goes with --window or --in"));
    }
    if steps.is_some() && glide.is_some() {
        return Err(ctx.die("click: --steps or --glide, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    // A recording's glide (record start --glide) unless --steps says how to get there.
    if steps.is_none() && glide.is_none() {
        glide = rec_glide(ctx).and_then(|g| shell::dec(&g));
    }
    let (x, y) = if find.is_empty() {
        to_screen(ctx, &mut st, raise, &inp, &mut sels, xyv[0], xyv[1])?
    } else {
        found(ctx, &mut st, raise, &mut sels, &layer, &find, index)?
    };
    let mut seq = vec![
        b"move".to_vec(),
        num(x),
        num(y),
        b"click".to_vec(),
        btn.clone(),
    ];
    if double {
        seq.extend([b"click".to_vec(), btn.clone()]);
    }
    let ext = extent(ctx)?;
    on_screen(ctx, "click", &num(x), &num(y), &ext)?;
    // --steps or --glide: the way there first, on its own, so --wait watches what the click does,
    // not the pointer crossing the screen, and --mod holds its modifiers for the click alone.
    if steps.unwrap_or(1) > 1 || glide.is_some() {
        let (fx, fy) = pointer_at(ctx)?;
        let mut way = Vec::new();
        match glide {
            Some(g) => glide_to(&mut way, [&num(fx), &num(fy), &num(x), &num(y)], g),
            None => steps_to(&mut way, [fx, fy, x, y], steps.unwrap_or(1)),
        }
        let t0 = epoch_us();
        if !way.is_empty() {
            sh_e(pointer_tool(ctx, &ext, &way)?)?;
            rec_input(ctx, &t0, &[cat!("ptr ", ext, " ", way.join(&b' '))]);
        }
    }
    // The mark (for a peek window) has the screen pixels the click went to, however they were given.
    let mut marks = vec![cat!("ptr ", ext, " ", seq.join(&b' '))];
    if !mods.is_empty() {
        marks.push(cat!("combo ", mods, "+click"));
    }
    if !st.wait {
        let t0 = epoch_us();
        sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
        mark(ctx, &t0, false, &marks);
        return Ok(());
    }
    ignore_cursor(ctx, &mut st, x, y)?;
    watch(ctx, &mut st, "click", x, y)?;
    let t0 = epoch_us();
    sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
    mark(ctx, &t0, false, &marks);
    let json = st.json;
    code(st.end(ctx, "settle", json, b"the click was sent")?)
}

/// `awk -v a=V 'BEGIN { exit a == 0 }'` and the like: a number awk reads from a word.
fn zero(v: &[u8]) -> bool {
    awk_num(v) == 0.0
}

pub fn scroll(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut sels, mut raise) = (Vec::new(), Vec::new(), true);
    let mut xyv: Vec<Vec<u8>> = Vec::new();
    let (mut inp, mut source, mut mods) = (Vec::new(), Vec::new(), Vec::new());
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
            b"--in" => {
                optarg(ctx, a, v)?;
                inp = args[i + 1].clone();
                i += 1;
            }
            b"--no-raise" => raise = false,
            b"--mod" => {
                optarg(ctx, a, v)?;
                mods = mods_add(ctx, &mods, &args[i + 1])?;
                i += 1;
            }
            b"--source" => {
                optarg(ctx, a, v)?;
                let s = &args[i + 1];
                if !shell::re_match("^(wheel|finger|continuous|tilt)$", s) {
                    return Err(ctx.die(cat!(
                        "scroll: --source is wheel, finger, continuous or tilt, got ",
                        s
                    )));
                }
                source = s.clone();
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("scroll"),
            w if w == b"-g" || w.starts_with(b"--") => {
                st.opt(ctx, &args[i..])?;
                if st.took == 0 {
                    return Err(unknown_opt(ctx, "scroll", a));
                }
                i += st.took - 1;
            }
            w => {
                if !shell::re_match(r"^-?[0-9]+(\.[0-9]+)?$", w) {
                    return Err(unexpected(
                        ctx,
                        "scroll",
                        w,
                        "(X Y DY [DX]: numbers; 15 is one wheel notch, positive down and right)",
                    ));
                }
                xyv.push(w.to_vec());
            }
        }
        i += 1;
    }
    st.opts_check(ctx, sels.len())?;
    if !(3..=4).contains(&xyv.len()) {
        return Err(
            ctx.die("scroll: need X Y DY [DX] (15 is one wheel notch; positive is down and right)")
        );
    }
    if !(shell::is_num(&xyv[0]) && shell::is_num(&xyv[1])) {
        return Err(ctx.die(cat!(
            "scroll: X Y are whole pixels, got ",
            xyv[0],
            " ",
            xyv[1]
        )));
    }
    let dx: Vec<u8> = xyv.get(3).cloned().unwrap_or_default();
    let (a, b) = (
        awk_num(&xyv[2]),
        awk_num(if dx.is_empty() { b"0" } else { &dx }),
    );
    let inr = |v: f64| (-10_000.0..=10_000.0).contains(&v);
    if !(inr(a) && inr(b) && (a != 0.0 || b != 0.0)) {
        return Err(ctx.die("scroll: DY and DX are -10000 to 10000, not both 0"));
    }
    if !inp.is_empty() && !sels.is_empty() {
        return Err(ctx.die("scroll: --in or --window, not both"));
    }
    if !raise && inp.is_empty() && sels.is_empty() {
        return Err(ctx.die("scroll: --no-raise goes with --window or --in"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let (x0, y0) = (
        shell::dec(&xyv[0]).unwrap_or(0),
        shell::dec(&xyv[1]).unwrap_or(0),
    );
    let (x, y) = to_screen(ctx, &mut st, raise, &inp, &mut sels, x0, y0)?;
    let ext = extent(ctx)?;
    on_screen(ctx, "scroll", &num(x), &num(y), &ext)?;
    let mut seq = vec![b"move".to_vec(), num(x), num(y)];
    if !source.is_empty() {
        seq.extend([b"source".to_vec(), source.clone()]);
    }
    if !zero(&xyv[2]) {
        seq.extend([b"scroll".to_vec(), xyv[2].clone()]);
    }
    if !dx.is_empty() && !zero(&dx) {
        seq.extend([b"hscroll".to_vec(), dx.clone()]);
    }
    if source == b"wheel" || source == b"tilt" {
        let note = wheel_note(&xyv[2], if dx.is_empty() { b"0" } else { &dx });
        if !note.is_empty() {
            say(cat!(
                "scroll: a ",
                source,
                " turns in notches of 15: ",
                note
            ));
        }
    }
    // --mod (#167): Omarchy's SUPER+wheel binds (workspaces, groups), held as click and drag hold them.
    let mut marks = vec![cat!("ptr ", ext, " ", pointer_marks(&seq))];
    if !mods.is_empty() {
        marks.push(cat!("combo ", mods, "+scroll"));
    }
    let t0 = epoch_us();
    if !st.wait {
        sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
        mark(ctx, &t0, false, &marks);
        return Ok(());
    }
    ignore_cursor(ctx, &mut st, x, y)?;
    watch(ctx, &mut st, "scroll", x, y)?;
    let t0 = epoch_us();
    sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
    mark(ctx, &t0, false, &marks);
    let json = st.json;
    code(st.end(ctx, "settle", json, b"the scroll was sent")?)
}
