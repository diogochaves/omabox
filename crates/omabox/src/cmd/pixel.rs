//! `omabox pixel [-b NAME] [--window SEL | --active | --in SHOT] [--json] X Y [X Y]...` (#133,
//! finding 209): the colour of screen (or window) pixels, from one grab of the box around the points
//! as PPM, read at each point. A window's points are its own pixels (grim -T, captured twice as shot
//! does); --in maps a shot's pixels as click does.

use crate::args::{optarg, unknown_opt};
use crate::boxes::{box_alive, need_box};
use crate::cat;
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Exit, Res, out};
use crate::hypr::{hung_text, hypr_answers, hypr_json};
use crate::jq::{self, Args};
use crate::jqprog::JQ_MON;
use crate::screen::{Point, extent, shot_point, win_buf_geom, win_scale};
use crate::select::select_box;
use crate::shell::{self, strip_nl};
use crate::shellnote::shell_note;
use crate::win::{box_windows, jq_r, win_desc, win_select};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;

/// `[ A -lt B ]`.
fn lt(a: &[u8], b: &[u8]) -> bool {
    shell::test_int(a)
        .zip(shell::test_int(b))
        .is_some_and(|(a, b)| a < b)
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut sels, mut inp, mut json) = (Vec::new(), Vec::new(), Vec::new(), false);
    let mut xy: Vec<i64> = Vec::new();
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
            b"--json" => json = true,
            b"-h" | b"--help" => return crate::args::usage("pixel"),
            _ if a.starts_with(b"-") => return Err(unknown_opt(ctx, "pixel", a)),
            _ => {
                // (`$((10#N))`: decimal, whatever its zeros)
                let n = shell::re_match("^[0-9]{1,5}$", a)
                    .then(|| shell::dec(a))
                    .flatten();
                let Some(n) = n else {
                    return Err(ctx.die(cat!("pixel: bad argument ", a, " (X Y, in pixels)")));
                };
                xy.push(n);
            }
        }
        i += 1;
    }
    if xy.len() < 2 || xy.len() % 2 != 0 {
        return Err(ctx.die("pixel: need X Y (several: X1 Y1 X2 Y2 ...)"));
    }
    if xy.len() > 64 {
        return Err(ctx.die("pixel: at most 32 points"));
    }
    if !inp.is_empty() && !sels.is_empty() {
        return Err(ctx.die("pixel: --in or --window, not both"));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    shell_note(ctx)?;
    let (mut pts, mut window, mut addr): (Vec<i64>, bool, Vec<u8>) =
        (Vec::new(), false, Vec::new());
    if !inp.is_empty() {
        for k in (0..xy.len()).step_by(2) {
            let (x, y) = (xy[k].to_string(), xy[k + 1].to_string());
            match shot_point(ctx, &inp, x.as_bytes(), y.as_bytes())? {
                Point::Window(a, px, py) => {
                    window = true;
                    addr = a;
                    pts.extend([px, py]);
                }
                Point::Screen(px, py) => pts.extend([px, py]),
            }
        }
    } else {
        pts = xy.clone();
    }
    let (lim_w, lim_h, mut id, mut w);
    (id, w) = (Vec::new(), Vec::new());
    if !sels.is_empty() || window {
        window = true;
        let wins = box_windows(ctx, None)?;
        w = if addr.is_empty() {
            win_select(ctx, &wins, &sels)?
        } else {
            Args::new(&["-c"])
                .kv("--arg", "a", &addr)
                .arg(crate::jqprog::BY_ADDRESS_JQ)
                .run(Some(&wins), jq::Err::Inherit)
                .text()
        };
        id = jq_r(".stableId", &w);
        if !shell::re_match("^[0-9a-fA-F]+$", &id) {
            return Err(ctx.die(cat!(
                "pixel: window ",
                win_desc(&w),
                " has no stableId to capture it by (",
                id,
                ")"
            )));
        }
        let f = shell::read_fields(&jq_r(crate::jqprog::WIN_SIZE2_JQ, &w), 2);
        (lim_w, lim_h) = (f[0].clone(), f[1].clone());
    } else {
        let ext = extent(ctx)?;
        let xi = ext.iter().rposition(|&c| c == b'x');
        lim_w = xi.map_or(ext.clone(), |i| ext[..i].to_vec());
        lim_h = ext
            .iter()
            .position(|&c| c == b'x')
            .map_or(ext.clone(), |i| ext[i + 1..].to_vec());
        let mons = strip_nl(&hypr_json(ctx, &["monitors"])?);
        let p = format!(
            "[{}]",
            pts.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
        );
        let r = Args::new(&["-r"])
            .kv("--argjson", "p", p)
            .kv("--argjson", "lw", &lim_w)
            .kv("--argjson", "lh", &lim_h)
            .arg(format!("{JQ_MON}{}", crate::jqprog::PIXEL_OFF_JQ))
            .run(Some(&mons), jq::Err::Inherit);
        let off = if r.ok() { r.text() } else { Vec::new() };
        if !off.is_empty() {
            return Err(ctx.die(cat!("pixel: ", off)));
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (99_999i64, 99_999i64, 0i64, 0i64);
    for k in (0..pts.len()).step_by(2) {
        let (px, py) = (pts[k].to_string(), pts[k + 1].to_string());
        if !(lt(px.as_bytes(), &lim_w) && lt(py.as_bytes(), &lim_h)) {
            let what = if window {
                cat!("window (", win_desc(&w), ")")
            } else {
                b"screen".to_vec()
            };
            return Err(ctx.die(cat!(
                "pixel: ",
                px,
                ",",
                py,
                " is outside the ",
                what,
                " (",
                lim_w,
                "x",
                lim_h,
                ")"
            )));
        }
        x0 = x0.min(pts[k]);
        x1 = x1.max(pts[k]);
        y0 = y0.min(pts[k + 1]);
        y1 = y1.max(pts[k + 1]);
    }
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut cut = None;
    let (mut mons, mut held) = (None, None);
    let mut grim: Vec<Vec<u8>> = vec![
        b"timeout".to_vec(),
        b"10".to_vec(),
        b"grim".to_vec(),
        b"-t".to_vec(),
        b"ppm".to_vec(),
    ];
    if !id.is_empty() {
        // A window's -g is in its buffer's pixels, at its monitor's scale (finding 300).
        let wsc = win_scale(ctx, &jq_r(".address", &w))?;
        let (sx, sy, sw, sh) = (
            x0.to_string(),
            y0.to_string(),
            bw.to_string(),
            bh.to_string(),
        );
        let g = win_buf_geom([
            sx.as_bytes(),
            sy.as_bytes(),
            sw.as_bytes(),
            sh.as_bytes(),
            &wsc,
            &lim_w,
            &lim_h,
        ]);
        grim.extend([b"-T".to_vec(), id.clone(), b"-g".to_vec(), g]);
    } else {
        // At a fractional scale: grabbed from a whole pixel and cut, so a colour read is a pixel of
        // the screen's, not grim's blend of two (#423, finding 641), through the pixel of the last
        // point, which grim's size can stop short of (#425, finding 643).
        let m = crate::screen::mons(ctx)?;
        if let Some((c, map)) = crate::screen::px_hold_in(&m, [x0, y0, bw, bh]) {
            (cut, held) = (Some(c), Some(map));
        }
        mons = Some(m);
        let r = cut.map_or([x0, y0, bw, bh], |c| c.grab);
        grim.extend([
            b"-g".to_vec(),
            format!("{},{} {}x{}", r[0], r[1], r[2], r[3]).into_bytes(),
        ]);
    }
    grim.push(b"-".to_vec());
    // (Both removed on every return, and on INT, TERM or HUP: bash's set -e exits left the image in
    // $TMPDIR, and so did a signal, finding 443.)
    let (mut f, fp) = shell::mktemp()?;
    let fp2 = fp.clone();
    let _fp = crate::trap::on_exit(move || {
        let _ = std::fs::remove_file(shell::path(&fp2));
    });
    let (mut ef, ep) = shell::mktemp()?;
    let ep2 = ep.clone();
    let _ep = crate::trap::on_exit(move || {
        let _ = std::fs::remove_file(shell::path(&ep2));
    });
    // (A window's first frame off screen can be old: captured twice, as shot does.)
    if !id.is_empty() {
        on_box(ctx, &grim, false, Out::Null, ErrTo::Null).or_exit()?;
    }
    let fd = f.try_clone().map_err(|_| Exit(1))?;
    let r = on_box(
        ctx,
        &grim,
        false,
        Out::Fd(fd.into()),
        ErrTo::Fd(ef.as_raw_fd()),
    )
    .or_exit()?;
    // `why=$(tr '\n' ' ' < "$err" | sed 's/ *$//')`
    let mut et = Vec::new();
    let _ = ef.rewind();
    let _ = ef.read_to_end(&mut et);
    let mut why: Vec<u8> = et
        .iter()
        .map(|&c| if c == b'\n' { b' ' } else { c })
        .collect();
    while why.last() == Some(&b' ') {
        why.pop();
    }
    let why = shell::subst(&why);
    let mut img = Vec::new();
    let _ = f.rewind();
    let _ = f.read_to_end(&mut img);
    if r.rc != 0 || img.is_empty() {
        if !box_alive(ctx)? {
            return Err(ctx.die(cat!("pixel: box '", ctx.name, "' went down")));
        }
        if !hypr_answers(ctx)? {
            let t = hung_text(ctx, "")?;
            return Err(ctx.die(cat!("pixel: ", t)));
        }
        let w = if why.is_empty() {
            Vec::new()
        } else {
            cat!(" (", why, ")")
        };
        return Err(ctx.die(cat!("pixel: no frame from box '", ctx.name, "'", w)));
    }
    if let Some(c) = cut {
        let Some(i) = crate::imgfind::ppm(&img) else {
            return Err(ctx.die("pixel: grim's image is not a PPM it reads"));
        };
        img = i.crop(c.crop).to_ppm();
        let put = f.rewind().and_then(|()| f.set_len(0));
        if put.and_then(|()| f.write_all(&img)).is_err() {
            return Err(ctx.die(cat!("pixel: cannot write ", fp)));
        }
    }
    // grim's PPM: "P6\nW H\n255\n", then RGB. At an output scale over 1 the grab is larger than the
    // box.
    // (A header cut short is said: bash's `read` of a line without its newline ended it under set
    // -e, 1, unsaid, finding 442)
    let mut lines = img.splitn(4, |&c| c == b'\n');
    let mut whole = Vec::new();
    for _ in 0..3 {
        whole.push(lines.next().unwrap_or_default());
    }
    if lines.next().is_none() {
        return Err(ctx.die("pixel: grim's image is short"));
    }
    let line = |l: &[u8]| -> Vec<u8> {
        let l: Vec<u8> = l.iter().copied().filter(|&c| c != 0).collect();
        shell::read_fields(&l, 1).remove(0)
    };
    let (magic, dims, maxv) = (line(whole[0]), line(whole[1]), line(whole[2]));
    let d = shell::read_fields(&dims, 2);
    let (iw, ih) = (&d[0], &d[1]);
    let num = |v: &[u8]| shell::is_num(v);
    if !(magic == b"P6" && maxv == b"255" && num(iw) && num(ih)) {
        return Err(ctx.die("pixel: grim's image is not a PPM it reads"));
    }
    let hdr = (magic.len() + dims.len() + maxv.len() + 3) as i64;
    // (decimal, an 08 is 8: bash's arithmetic ended pixel there in its own words, finding 448)
    let (Some(iw), Some(ih)) = (shell::dec(iw), shell::dec(ih)) else {
        return Err(ctx.die("pixel: grim's image is not a PPM it reads"));
    };
    // A point on the screen is read from the pixel its monitor maps it to, as shot -g cuts and find
    // reports (#425); a window's by its buffer's size.
    let map = held.or_else(|| {
        let (w, h) = (
            usize::try_from(iw).unwrap_or(0),
            usize::try_from(ih).unwrap_or(0),
        );
        let m = mons.as_ref()?;
        Some(crate::screen::px_map_in(m, [x0, y0, bw, bh], w, h))
    });
    let mut text = Vec::new();
    for k in (0..pts.len()).step_by(2) {
        let (px, py) = match &map {
            Some(m) => {
                let [px, py] = m.pixel(pts[k], pts[k + 1]);
                (px.clamp(0, (iw - 1).max(0)), py.clamp(0, (ih - 1).max(0)))
            }
            None => (
                (pts[k] - x0).saturating_mul(iw) / bw,
                (pts[k + 1] - y0).saturating_mul(ih) / bh,
            ),
        };
        let at = hdr + (py * iw + px) * 3;
        let mut rgb = [0u8; 3];
        let got = u64::try_from(at)
            .ok()
            .and_then(|at| f.seek(SeekFrom::Start(at)).ok())
            .and_then(|_| f.read_exact(&mut rgb).ok());
        if got.is_none() {
            out(&text)?;
            // (A point at or past the image's end is said: bash's `read` of od's nothing ended it
            // under set -e, 1, unsaid but for od's own line past it, the image left: findings 441,
            // 442. Part of a pixel: short.)
            if at >= img.len() as i64 {
                return Err(ctx.die("pixel: the point is past the end of grim's image"));
            }
            return Err(ctx.die("pixel: grim's image is short"));
        }
        let hex = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
        if json {
            text.extend_from_slice(
                format!(
                    "{{\"x\":{},\"y\":{},\"hex\":\"{hex}\",\"r\":{},\"g\":{},\"b\":{}}}\n",
                    xy[k],
                    xy[k + 1],
                    rgb[0],
                    rgb[1],
                    rgb[2]
                )
                .as_bytes(),
            );
        } else if xy.len() == 2 {
            text.extend_from_slice(cat!(hex, "\n").as_slice());
        } else {
            text.extend_from_slice(format!("{},{} {hex}\n", xy[k], xy[k + 1]).as_bytes());
        }
    }
    out(text)
}
