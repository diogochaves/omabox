//! `omabox pointer [-b NAME] [--window SEL | --in SHOT] [--steps N | --glide DURATION] [--mod MODS]
//! -- ARGS...`: the pointer tool's own sequence (move X Y [--steps N], click/down/up [BTN], scroll
//! DY, hscroll DX, source ..., sleep DURATION). --in maps each move from a shot's image, --window from
//! that window's coordinates (#25); raw: it raises nothing. --steps travels each move from where the
//! pointer is (#38), --glide in that time (#176), --mod holds modifiers for the whole sequence. Also
//! `omabox pointer hide|show` (#228).

use crate::args::optarg;
use crate::boxes::need_box;
use crate::cat;
use crate::cmd::click::{pointer_tool, sh_e};
use crate::exit::{Ctx, Exit, Res};
use crate::input::{
    glide_arg, glide_to, mods_add, num, on_screen, pause_ms, pointer_at, pointer_marks, steps_arg,
    steps_to, with_mods, xy,
};
use crate::marks::{epoch_us, mark, rec_glide, rec_input};
use crate::ptr::pointer_shown;
use crate::screen::{Point, extent, shot_point_act};
use crate::select::select_box;
use crate::shell::{self, dec};
use crate::win::{Raise, win_point};

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut inp, mut sels, mut mods) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut steps, mut glide): (Option<i64>, Option<i64>) = (None, None);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        match a.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, a, v)?;
                name = args[i + 1].clone();
                i += 2;
            }
            b"--in" => {
                optarg(ctx, a, v)?;
                inp = args[i + 1].clone();
                i += 2;
            }
            b"-w" | b"--window" => {
                optarg(ctx, a, v)?;
                sels.push(args[i + 1].clone());
                i += 2;
            }
            b"--active" => {
                sels.push(b"active".to_vec());
                i += 1;
            }
            b"--steps" => {
                optarg(ctx, a, v)?;
                steps = Some(steps_arg(ctx, "pointer", &args[i + 1])?);
                i += 2;
            }
            b"--glide" => {
                optarg(ctx, a, v)?;
                glide = Some(glide_arg(ctx, "pointer", &args[i + 1])?);
                i += 2;
            }
            b"--mod" => {
                optarg(ctx, a, v)?;
                mods = mods_add(ctx, &mods, &args[i + 1])?;
                i += 2;
            }
            b"--" => {
                i += 1;
                break;
            }
            _ => break,
        }
    }
    let rest = &args[i.min(args.len())..];
    if !inp.is_empty() && !sels.is_empty() {
        return Err(ctx.die("pointer: --in or --window, not both"));
    }
    if rest.iter().any(|a| a == b"--hold") {
        return Err(ctx.die(
            "pointer: --hold is omabox's own (it never returns); to keep a button down: down, move ..., up (or omabox drag)",
        ));
    }
    // sleep DURATION (#273): as the tool's milliseconds, read before any box is asked.
    let mut raw: Vec<Vec<u8>> = Vec::new();
    let mut k = 0;
    while k < rest.len() {
        if rest[k] == b"sleep" && k + 1 < rest.len() {
            let ms = pause_ms(ctx, "pointer sleep", &rest[k + 1])?;
            raw.extend([b"sleep".to_vec(), num(ms)]);
            k += 2;
        } else if let Some(n) = rest[k].strip_prefix(b"--steps=".as_slice()) {
            // (a move's own --steps=N, as every option takes its value: #273)
            raw.extend([b"--steps".to_vec(), n.to_vec()]);
            k += 1;
        } else {
            raw.push(rest[k].clone());
            k += 1;
        }
    }
    if let Some(w @ (b"hide" | b"show")) = raw.first().map(Vec::as_slice) {
        if !(raw.len() == 1
            && inp.is_empty()
            && mods.is_empty()
            && glide.is_none()
            && sels.is_empty()
            && steps.unwrap_or(1) == 1)
        {
            return Err(ctx.die(cat!("pointer ", w, ": nothing else with it")));
        }
        let hide = w == b"hide";
        select_box(ctx, &name)?;
        need_box(ctx)?;
        return pointer_shown(ctx, hide);
    }
    if steps.is_some() && glide.is_some() {
        return Err(ctx.die("pointer: --steps or --glide, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    // A recording's glide (record start --glide) unless --steps says how to get there.
    if steps.is_none() && glide.is_none() {
        glide = rec_glide(ctx).and_then(|g| shell::dec(&g));
    }
    let steps = steps.unwrap_or(1);
    // Moves to screen pixels (--in, --window), in steps from where the pointer was (--steps). The
    // mark has the moves as asked, in screen pixels: a step each would pass peek's 64 tokens.
    let (mut seq, mut marks): (Vec<Vec<u8>>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
    let mut cur: Option<(i64, i64)> = None;
    let ext = extent(ctx)?;
    let mapped = !inp.is_empty() || !sels.is_empty();
    let mut k = 0;
    while k < raw.len() {
        let r = &raw[k..];
        if r[0] != b"move" || r.len() < 3 {
            seq.push(r[0].clone());
            marks.push(r[0].clone());
            k += 1;
            continue;
        }
        let (mut n, mut g) = (steps, glide);
        let own = r.get(3).is_some_and(|w| w == b"--steps");
        if own {
            let Some(v) = r.get(4) else {
                return Err(ctx.die("pointer: --steps needs a value"));
            };
            n = steps_arg(ctx, "pointer", v)?;
            g = None;
        }
        if !(shell::is_num(&r[1]) && shell::is_num(&r[2])) {
            // Left to the tool, which refuses it, when nothing here needs the numbers.
            if mapped || n != 1 || g.is_some() {
                return Err(ctx.die(cat!(
                    "pointer: move takes whole pixels, got ",
                    r[1],
                    " ",
                    r[2]
                )));
            }
            seq.extend(r[..3].iter().cloned());
            marks.extend(r[..3].iter().cloned());
            k += 3;
            continue;
        }
        let (mut px, mut py) = (
            shell::dec(&r[1]).unwrap_or(0),
            shell::dec(&r[2]).unwrap_or(0),
        );
        let at = |p: Vec<u8>| -> Res<(i64, i64)> {
            let (a, b) = xy(&p);
            Ok((dec(&a).ok_or(Exit(1))?, dec(&b).ok_or(Exit(1))?))
        };
        if !sels.is_empty() {
            (px, py) = at(win_point(ctx, Raise::Pointer, &num(px), &num(py), &sels)?)?;
        } else if !inp.is_empty() {
            match shot_point_act(ctx, &inp, &num(px), &num(py))? {
                Point::Window(wa, wx, wy) => {
                    (px, py) = at(win_point(ctx, Raise::Pointer, &num(wx), &num(wy), &[wa])?)?;
                }
                Point::Screen(sx, sy) => (px, py) = (sx, sy),
            }
        }
        on_screen(ctx, "pointer", &num(px), &num(py), &ext)?;
        if let Some(g) = g {
            let (cx, cy) = match cur {
                Some(c) => c,
                None => pointer_at(ctx)?,
            };
            let mut way = Vec::new();
            glide_to(&mut way, [&num(cx), &num(cy), &num(px), &num(py)], g);
            if way.is_empty() {
                way = vec![b"move".to_vec(), num(px), num(py)];
            }
            seq.extend(way);
        } else if n > 1 {
            let (cx, cy) = match cur {
                Some(c) => c,
                None => pointer_at(ctx)?,
            };
            steps_to(&mut seq, [cx, cy, px, py], n);
        } else {
            seq.extend([b"move".to_vec(), num(px), num(py)]);
        }
        marks.extend([b"move".to_vec(), num(px), num(py)]);
        cur = Some((px, py));
        k += if own { 5 } else { 3 };
    }
    let t0 = epoch_us();
    sh_e(with_mods(ctx, &mods, |c| pointer_tool(c, &ext, &seq))?)?;
    // A recording has every move the tool made (a glide's steps); peek, the moves as asked.
    let mut rec = Vec::new();
    if !mods.is_empty() {
        rec.push(cat!("combo ", mods));
    }
    rec.push(cat!("ptr ", ext, " ", pointer_marks(&seq)));
    rec_input(ctx, &t0, &rec);
    if !mods.is_empty() {
        mark(ctx, b"", true, &[cat!("combo ", mods)]);
    }
    mark(
        ctx,
        b"",
        true,
        &[cat!("ptr ", ext, " ", pointer_marks(&marks))],
    );
    Ok(())
}
