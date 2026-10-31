//! `omabox drag [-b NAME] [--window SEL [--no-raise] | --in SHOT] [--wait | --shot FILE] X1 Y1 X2 Y2
//! [BTN]` (#25): press at the first point, move to the second in --steps (10) or --glide, hold there
//! --hold DURATION (a drop target reacting to the hover), release. --window and --in map both points
//! as for click; --shot FILE takes a shot while the button is still down (the pointer tool pauses for
//! it); --mod MODS holds modifiers from the press to the release; --wait watches what it does
//! (finding 172).

use crate::args::{optarg, unexpected, unknown_opt};
use crate::boxes::need_box;
use crate::cat;
use crate::cmd::click::{pointer_tool, sh_e};
use crate::cmd::hyprctl::code;
use crate::exec::{ErrTo, In, Out, pipe};
use crate::exit::{Ctx, Exit, Res, say};
use crate::input::{
    BOX_POINTER, glide_arg, glide_to, mods_add, num, on_screen, path_rects, pointer_at, read_line,
    steps_arg, steps_to, with_mods, xy,
};
use crate::jail::in_jail;
use crate::marks::{epoch_us, mark, rec_glide, rec_input};
use crate::num::ms_duration;
use crate::screen::{Point, extent, shot_point_act};
use crate::select::select_box;
use crate::settle::{Lines, Settle};
use crate::shell;
use crate::win::{Raise, screen_layer, win_point};
use std::os::fd::AsRawFd;
use std::time::Duration;

fn button(w: &[u8]) -> bool {
    matches!(
        w,
        b"left" | b"right" | b"middle" | b"side" | b"extra" | b"back" | b"forward"
    )
}

/// drag_shot: the pointer tool says "paused" once the button is down at the end; the shot is taken
/// then (shot in a subshell of its own: its refusals are its status), and a line on the tool's
/// stdin lets it release. A shot that fails still releases the button.
fn drag_shot(ctx: &mut Ctx, ext: &[u8], seq: &[Vec<u8>], shot: &[Vec<u8>]) -> Res<i32> {
    let ((in_r, in_w), (out_r, out_w)) = match (pipe(), pipe()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return Err(ctx.die("could not make the pipes for the pointer tool")),
    };
    let mut argv = vec![
        BOX_POINTER.as_bytes().to_vec(),
        b"--extent".to_vec(),
        ext.to_vec(),
    ];
    argv.extend(seq.iter().cloned());
    let c = ctx.clone();
    // (bash's coproc: SIGINT and SIGQUIT ignored in it, so a Ctrl-C leaves it to release the button)
    let tool = std::thread::spawn(move || {
        let r = crate::exec::on_box_bg(
            &c,
            &argv,
            In::Fd(in_r),
            Out::Fd(out_w),
            ErrTo::Inherit,
            None,
        );
        r.rc
    });
    let mut lines = Lines::new(out_r);
    let mut rc = 0;
    if read_line(&mut lines, Duration::from_secs(120)).is_some_and(|l| l == b"paused") {
        let mut sub = ctx.clone();
        if let Err(Exit(r)) = crate::cmd::shot::run(&mut sub, shot) {
            rc = i32::from(r);
        }
    } else {
        rc = 1;
        say("drag: the pointer did not get to the end of the drag");
    }
    // (A tool already gone: bash's `echo >&"$din"` dies of SIGPIPE (141), or, once bash has reaped
    // the coproc and closed its fds, says `"$din": Bad file descriptor` and the tool's status stands.
    // Which one is the reaping's race; this write is racy the same way.)
    // SAFETY: one byte of ours to an fd we own.
    unsafe {
        libc::write(in_w.as_raw_fd(), b"\n".as_ptr().cast(), 1);
    }
    drop(in_w);
    let trc = tool.join().unwrap_or(1);
    drop(lines);
    if trc != 0 {
        rc = trc;
    }
    Ok(rc)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut btn, mut xyv, mut sels) =
        (Vec::new(), b"left".to_vec(), Vec::new(), Vec::new());
    let (mut raise, mut inp, mut hold, mut shot, mut mods) =
        (true, Vec::new(), 0i64, Vec::new(), Vec::new());
    let (mut steps, mut glide, mut shot_args): (Option<i64>, Option<i64>, Vec<Vec<u8>>) =
        (None, None, Vec::new());
    let mut st = Settle::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        let val = || args[i + 1].clone();
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = val();
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(val());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            b"--in" => {
                optarg(ctx, a, v)?;
                inp = val();
                i += 1;
            }
            b"--no-raise" => raise = false,
            b"--steps" => {
                optarg(ctx, a, v)?;
                steps = Some(steps_arg(ctx, "drag", &val())?);
                i += 1;
            }
            b"--glide" => {
                optarg(ctx, a, v)?;
                glide = Some(glide_arg(ctx, "drag", &val())?);
                i += 1;
            }
            b"--hold" => {
                optarg(ctx, a, v)?;
                match ms_duration(&val()) {
                    Some(h) if h <= 60_000 => hold = h,
                    _ => {
                        return Err(ctx.die(cat!(
                            "drag: --hold takes a duration up to 60s, like 300ms or 2s (a bare number is seconds), got ",
                            val()
                        )));
                    }
                }
                i += 1;
            }
            b"--shot" => {
                optarg(ctx, a, v)?;
                shot = val();
                i += 1;
            }
            // The held shot like any shot (#122): smaller, a part, one monitor.
            b"--shot-fit" | b"--shot-g" | b"--shot-monitor" => {
                optarg(ctx, a, v)?;
                let o: &[u8] = match a.as_slice() {
                    b"--shot-fit" => b"--fit",
                    b"--shot-g" => b"-g",
                    _ => b"--monitor",
                };
                shot_args.extend([o.to_vec(), val()]);
                i += 1;
            }
            b"--mod" => {
                optarg(ctx, a, v)?;
                mods = mods_add(ctx, &mods, &val())?;
                i += 1;
            }
            w if button(w) => btn = w.to_vec(),
            w if w == b"-g" || w.starts_with(b"--") => {
                st.opt(ctx, &args[i..])?;
                if st.took == 0 {
                    return Err(unknown_opt(ctx, "drag", a));
                }
                i += st.took - 1;
            }
            w => {
                if !shell::is_num(w) {
                    return Err(unexpected(
                        ctx,
                        "drag",
                        w,
                        "(X1 Y1 X2 Y2 in whole pixels, then a button: left, right, middle, side, extra, back or forward)",
                    ));
                }
                xyv.push(shell::dec(w).unwrap_or(0));
            }
        }
        i += 1;
    }
    st.opts_check(ctx, sels.len())?;
    if xyv.len() != 4 {
        return Err(ctx.die("drag: need X1 Y1 X2 Y2"));
    }
    if !inp.is_empty() && !sels.is_empty() {
        return Err(ctx.die("drag: --in or --window, not both"));
    }
    if !raise && inp.is_empty() && sels.is_empty() {
        return Err(ctx.die("drag: --no-raise goes with --window or --in"));
    }
    if !shot.is_empty() && st.wait {
        return Err(ctx.die("drag: --shot or --wait, not both (the shot is the look)"));
    }
    if !shot_args.is_empty() && shot.is_empty() {
        return Err(ctx.die("drag: --shot-fit, --shot-g and --shot-monitor go with --shot FILE"));
    }
    if !shot.is_empty() && in_jail() {
        ctx.parsed();
        return Err(ctx.die(
            "drag: --shot is not for an agent inside ai-jail yet: omabox shot after the drag, or --hold and a shot of its own",
        ));
    }
    if steps.is_some() && glide.is_some() {
        return Err(ctx.die("drag: --steps or --glide, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // A recording's glide (record start --glide) unless --steps says how to get there.
    if steps.is_none() && glide.is_none() {
        glide = rec_glide(ctx).and_then(|g| shell::dec(&g));
    }
    let steps = steps.unwrap_or(10);
    // Both points to screen pixels: from a shot's image, then from a window's coordinates.
    let mut pts: Vec<i64> = Vec::new();
    for k in [0usize, 2] {
        let (mut x, mut y) = (xyv[k], xyv[k + 1]);
        let mut wsel = sels.clone();
        if !inp.is_empty() {
            match shot_point_act(ctx, &inp, &num(x), &num(y))? {
                Point::Window(a, wx, wy) => {
                    (x, y) = (wx, wy);
                    wsel = vec![a];
                }
                Point::Screen(sx, sy) => (x, y) = (sx, sy),
            }
        }
        if !wsel.is_empty() {
            let r = if raise { Raise::Yes } else { Raise::No };
            let p = win_point(ctx, r, &num(x), &num(y), &wsel)?;
            let (px, py) = xy(&p);
            let px = crate::num::read(ctx, "the window's point", &px)?;
            let py = crate::num::read(ctx, "the window's point", &py)?;
            // (--wait's -g and --ignore beside --window: the window's own, F8; where it is at the start)
            if st.window && k == 0 {
                st.in_window(ctx, &num(px.saturating_sub(x)), &num(py.saturating_sub(y)))?;
            }
            (x, y) = (px, py);
        } else if k == 0 {
            // The screen's start: a layer over the window there said (#361); the end needs no
            // word, the press's surface keeps the pointer for the drag.
            screen_layer(ctx, &num(x), &num(y));
        }
        pts.extend([x, y]);
    }
    let (x1, y1, x2, y2) = (pts[0], pts[1], pts[2], pts[3]);
    let mut way = Vec::new();
    match glide {
        Some(g) => {
            glide_to(&mut way, [&num(x1), &num(y1), &num(x2), &num(y2)], g);
            if way.is_empty() {
                way = vec![b"move".to_vec(), num(x2), num(y2)];
            }
        }
        None => steps_to(&mut way, [x1, y1, x2, y2], steps),
    }
    let mut seq = vec![
        b"move".to_vec(),
        num(x1),
        num(y1),
        b"down".to_vec(),
        btn.clone(),
    ];
    seq.extend(way.iter().cloned());
    if hold != 0 {
        seq.extend([b"sleep".to_vec(), num(hold)]);
    }
    if !shot.is_empty() {
        seq.push(b"pause".to_vec());
    }
    seq.extend([b"up".to_vec(), btn.clone()]);
    let ext = extent(ctx)?;
    on_screen(ctx, "drag", &num(x1), &num(y1), &ext)?;
    on_screen(ctx, "drag", &num(x2), &num(y2), &ext)?;
    // --glide: to the start first, gliding as the drag will.
    if let Some(g) = glide {
        let (fx, fy) = pointer_at(ctx)?;
        let mut to = Vec::new();
        glide_to(&mut to, [&num(fx), &num(fy), &num(x1), &num(y1)], g);
        let t0 = epoch_us();
        if !to.is_empty() {
            sh_e(pointer_tool(ctx, &ext, &to)?)?;
            rec_input(ctx, &t0, &[cat!("ptr ", ext, " ", to.join(&b' '))]);
        }
    }
    // --wait (finding 172): to the start first, on its own (as click --steps, finding 111), then the
    // cursor at each point of the path is not the change looked for; what the drag does beside it is.
    if st.wait {
        sh_e(pointer_tool(
            ctx,
            &ext,
            &[b"move".to_vec(), num(x1), num(y1)],
        )?)?;
        for r in path_rects(x1, y1, &way).ok_or(Exit(1))? {
            st.args.extend([b"--ignore".to_vec(), r]);
        }
        st.facts = "drag";
        st.fxy = cat!(num(x1), " ", num(y1));
        let mut tool = vec![b"settle".to_vec()];
        tool.extend(st.args.iter().cloned());
        st.begin(ctx, &tool)?;
        let json = st.json;
        st.ready(ctx, json)?;
    }
    let t0 = epoch_us();
    if shot.is_empty() {
        sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
    } else {
        let mut sargs = vec![
            b"-b".to_vec(),
            ctx.name.clone(),
            b"-o".to_vec(),
            shot.clone(),
        ];
        sargs.extend(shot_args.iter().cloned());
        let rc = with_mods(ctx, &mods, |c| drag_shot(c, &ext, &seq, &sargs))?;
        sh_e(rc)?;
    }
    // For a recording, the tool's whole sequence; for a peek window, where it started and ended (its
    // steps would pass the mark's 64 tokens).
    let mut rec = Vec::new();
    if !mods.is_empty() {
        rec.push(cat!("combo ", mods, "+drag"));
    }
    let shown: Vec<&[u8]> = seq
        .iter()
        .map(|w| {
            if w == b"pause" {
                &b""[..]
            } else {
                w.as_slice()
            }
        })
        .collect();
    rec.push(cat!("ptr ", ext, " ", shown.join(&b' ')));
    rec_input(ctx, &t0, &rec);
    if !mods.is_empty() {
        mark(ctx, b"", true, &[cat!("combo ", mods, "+drag")]);
    }
    mark(
        ctx,
        b"",
        true,
        &[cat!(
            "ptr ",
            ext,
            " move ",
            num(x1),
            " ",
            num(y1),
            " down ",
            btn,
            " move ",
            num(x2),
            " ",
            num(y2),
            " up ",
            btn
        )],
    );
    if st.wait {
        let json = st.json;
        st.expect_sels = sels;
        return code(st.end(ctx, "settle", json, b"the drag was sent")?);
    }
    Ok(())
}
