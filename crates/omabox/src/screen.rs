//! The box's screen as bin/omabox reads it: the layout's size (extent), the main monitor and its mode
//! (main_screen, box_screen: findings 70, 77, #122, #163), a window's capture scale and a region of it
//! in its buffer's pixels (win_scale, win_buf_geom: #227, findings 299, 300), and a recorded shot's
//! pixel back on the box (shot_point, for --in).

use crate::boxes::meta;
use crate::cat;
use crate::exit::{Ctx, Exit, Res};
use crate::hypr::hypr_json;
use crate::jq::{self, Args};
use crate::jqprog::JQ_MON;
use crate::num::awk_num;
use crate::shell::{self, dec, strip_nl};
use crate::win::{box_windows, jq_r, win_desc};
use std::os::unix::fs::MetadataExt;

/// extent: the layout's size, "WxH", or the command ends saying it cannot be read.
pub fn extent(ctx: &Ctx) -> Res<Vec<u8>> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let r = jq::jq(
        &["-er", &format!("{JQ_MON}{}", crate::jqprog::EXTENT_JQ)],
        Some(&m),
        jq::Err::Null,
    );
    if !r.ok() {
        return Err(ctx.die(cat!(
            "cannot read the screen size of box '",
            ctx.name,
            "' (it lists no monitor)"
        )));
    }
    Ok(r.text())
}

/// main_screen: the box's main monitor in `hyprctl -j monitors`, as jq: a headless box's HEADLESS-2
/// (WAYLAND-1 on NVIDIA), an interactive box's first window; never one of `monitor add`'s.
pub fn main_screen(ctx: &Ctx) -> &'static str {
    if meta(&ctx.d, "mode").0 == b"interactive" {
        crate::jqprog::MAIN_SCREEN_I_JQ
    } else {
        crate::jqprog::MAIN_SCREEN_H_JQ
    }
}

/// box_screen: the main screen's output name and mode ("NAME WxH@HZ").
pub fn box_screen(ctx: &Ctx) -> Res<Vec<u8>> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let prog = format!("{}{}", main_screen(ctx), crate::jqprog::BOX_SCREEN_JQ);
    let r = jq::jq(&["-r", &prog], Some(&m), jq::Err::Inherit);
    if !r.ok() {
        return Err(Exit(u8::try_from(r.rc).unwrap_or(1)));
    }
    Ok(r.text())
}

/// win_scale ADDRESS: the scale of a window's capture (grim -T draws it at its monitor's, #227,
/// finding 299); 1 when it cannot be read.
pub fn win_scale(ctx: &Ctx, addr: &[u8]) -> Res<Vec<u8>> {
    let cl = strip_nl(&hypr_json(ctx, &["clients"])?);
    let mons = strip_nl(&hypr_json(ctx, &["monitors"])?);
    let r = Args::new(&["-r"])
        .kv("--arg", "a", addr)
        .kv("--argjson", "m", &mons)
        .arg(crate::jqprog::WIN_SCALE_JQ)
        .run(Some(&cl), jq::Err::Null);
    let sc = if r.ok() { r.text() } else { b"1".to_vec() };
    Ok(if shell::re_match(r"^[0-9]+(\.[0-9]+)?$", &sc) {
        sc
    } else {
        b"1".to_vec()
    })
}

/// win_buf_geom X Y W H SCALE WINDOW_W WINDOW_H: a region of a window (its own coordinates) as grim
/// -T's -g takes it, in the buffer's pixels (finding 300): each edge at SCALE rounded as the buffer's
/// size is, inside the buffer. awk's arithmetic.
pub fn win_buf_geom(v: [&[u8]; 7]) -> Vec<u8> {
    let [x, y, w, h, s, ww, wh] = v.map(awk_num);
    let r = |v: f64| (v * s + 0.5).trunc();
    let (x0, y0) = (r(x), r(y));
    let (mut x1, mut y1) = (r(x + w), r(y + h));
    if x1 > r(ww) {
        x1 = r(ww);
    }
    if y1 > r(wh) {
        y1 = r(wh);
    }
    if x1 <= x0 {
        x1 = x0 + 1.0;
    }
    if y1 <= y0 {
        y1 = y0 + 1.0;
    }
    format!(
        "{},{} {}x{}",
        x0 as i64,
        y0 as i64,
        (x1 - x0) as i64,
        (y1 - y0) as i64
    )
    .into_bytes()
}

/// img_px P IMAGE SCREEN: the screen pixel under the centre of image pixel P; None for a number that
/// is none, or an image of no size.
fn img_px(p: &[u8], image: &[u8], screen: &[u8]) -> Option<i64> {
    let (p, i, s) = (dec(p)?, dec(image)?, dec(screen)?);
    let d = 2i64.saturating_mul(i);
    if d == 0 {
        return None;
    }
    Some(
        2i64.saturating_mul(p)
            .saturating_add(1)
            .saturating_mul(s)
            .saturating_div(d),
    )
}

/// `stat -L -c '%s %.9Y' -- FILE`.
pub fn stamp(f: &[u8]) -> Vec<u8> {
    match std::fs::metadata(shell::path(f)) {
        Ok(m) => format!("{} {}.{:09}", m.size(), m.mtime(), m.mtime_nsec()).into_bytes(),
        Err(_) => Vec::new(),
    }
}

/// The last line of a tab-separated FILE whose first field is KEY (awk's), empty for none.
pub fn tsv_last(file: &std::path::Path, key: &[u8]) -> Vec<u8> {
    let tsv = std::fs::read(file).unwrap_or_default();
    tsv.split(|&c| c == b'\n')
        .rfind(|l| l.split(|&c| c == b'\t').next() == Some(key) && !l.is_empty())
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// Where a shot's pixel is on the box.
pub enum Point {
    Window(Vec<u8>, i64, i64),
    Screen(i64, i64),
}

/// shot_point FILE X Y: image pixel X,Y of a recorded shot (shots.tsv) as the window's own
/// coordinates or the screen's. Refused: a file that is not a shot of this box, changed since, or
/// taken in another screen mode; a window gone or resized.
pub fn shot_point(ctx: &Ctx, file: &[u8], x: &[u8], y: &[u8]) -> Res<Point> {
    Ok(shot_point_ref(ctx, file, x, y)?.0)
}

/// shot_point for an action there (click, scroll, drag, pointer): a point of a screen shot is
/// refused too when what is at it on the screen now is not what was there at the shot (#333,
/// layout_check), said as "click --in: ...".
pub fn shot_point_act(ctx: &Ctx, file: &[u8], x: &[u8], y: &[u8]) -> Res<Point> {
    let (p, r) = shot_point_ref(ctx, file, x, y)?;
    if let Point::Screen(sx, sy) = p {
        let label = format!("{} --in", String::from_utf8_lossy(&ctx.main_cmd));
        crate::shotcmp::layout_check(ctx, &label, &r, sx, sy)?;
    }
    Ok(p)
}

/// shot_point, with the shot's record.
fn shot_point_ref(
    ctx: &Ctx,
    file: &[u8],
    x: &[u8],
    y: &[u8],
) -> Res<(Point, crate::shotcmp::ShotRef)> {
    let r = crate::shotcmp::load(ctx, file, "--in")?;
    let key = r.key.clone();
    let (kind, ref_) = match r.kind.iter().position(|&c| c == b' ') {
        Some(i) => (r.kind[..i].to_vec(), r.kind[i + 1..].to_vec()),
        None => (r.kind.clone(), r.kind.clone()),
    };
    let geo = r.geo.clone();
    let geo = &geo;
    let g = shell::read_ifs(geo, 4, b"x ");
    let (lw, lh, iw, ih) = (&g[0], &g[1], &g[2], &g[3]);
    let lt = |a: &[u8], b: &[u8]| {
        shell::test_int(a)
            .zip(shell::test_int(b))
            .is_some_and(|(a, b)| a < b)
    };
    if !(lt(x, iw) && lt(y, ih)) {
        return Err(ctx.die(cat!(
            "--in: ",
            x,
            ",",
            y,
            " is outside the image (",
            iw,
            "x",
            ih,
            ")"
        )));
    }
    // (a record omabox cannot read: said, 1; finding 448, where bash's arithmetic ended it in its
    // own words)
    let unread = || {
        ctx.die(cat!(
            "--in: ",
            key,
            ": its record in shots.tsv is not one omabox reads"
        ))
    };
    let (Some(rx), Some(ry)) = (img_px(x, iw, lw), img_px(y, ih, lh)) else {
        return Err(unread());
    };
    let half = |v: &[u8], first: bool| -> Option<i64> {
        // ${V%,*} and ${V#*,}
        let p = if first {
            &v[..v.iter().rposition(|&c| c == b',').unwrap_or(v.len())]
        } else {
            &v[v.iter().position(|&c| c == b',').map_or(0, |i| i + 1)..]
        };
        dec(p)
    };
    if kind == b"window" {
        // "ADDRESS", or "ADDRESS X,Y WxH" for a -g crop of it: where the crop starts, the window's
        // size.
        let a = shell::read_fields(&ref_, 3);
        let (addr, mut org, mut wsz) = (a[0].clone(), a[1].clone(), a[2].clone());
        if org.is_empty() {
            org = b"0,0".to_vec();
            wsz = cat!(lw, "x", lh);
        }
        let wins = box_windows(ctx, None)?;
        let w = Args::new(&["-c"])
            .kv("--arg", "a", &addr)
            .arg(crate::jqprog::BY_ADDRESS_JQ)
            .run(Some(&wins), jq::Err::Inherit)
            .text();
        if w.is_empty() {
            return Err(ctx.die(cat!("--in: the window of that shot (", addr, ") is gone")));
        }
        let size = jq_r(crate::jqprog::WIN_SIZE_JQ, &w);
        if size != wsz {
            return Err(ctx.die(cat!(
                "--in: window ",
                win_desc(&w),
                " is ",
                size,
                " now, ",
                wsz,
                " in the shot: take a new one"
            )));
        }
        let (Some(ox), Some(oy)) = (half(&org, true), half(&org, false)) else {
            return Err(unread());
        };
        return Ok((
            Point::Window(addr, ox.saturating_add(rx), oy.saturating_add(ry)),
            r,
        ));
    }
    let (Some(ox), Some(oy)) = (half(&ref_, true), half(&ref_, false)) else {
        return Err(unread());
    };
    Ok((
        Point::Screen(ox.saturating_add(rx), oy.saturating_add(ry)),
        r,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --in (finding 81): image pixel -> screen pixel under its centre (t_unit_window_select's).
    #[test]
    fn img_px_as_bash() {
        let px = |p: &str, i: &str, s: &str| img_px(p.as_bytes(), i.as_bytes(), s.as_bytes());
        assert_eq!(
            (px("0", "1920", "1920"), px("1919", "1920", "1920")),
            (Some(0), Some(1919)),
            "1:1 maps to itself"
        );
        assert_eq!(
            px("500", "1000", "1920"),
            Some(960),
            "x1.92: 500 of 1000 is 960"
        );
        assert_eq!(
            px("999", "1000", "1920"),
            Some(1919),
            "x1.92: the last pixel stays inside"
        );
        assert_eq!(
            px("101", "200", "100"),
            Some(50),
            "x0.5 (an image larger than the screen)"
        );
    }

    // What bash's awk wrote for each.
    #[test]
    fn buffer_regions() {
        let g = |v: [&str; 7]| String::from_utf8(win_buf_geom(v.map(str::as_bytes))).unwrap();
        assert_eq!(g(["10", "20", "5", "5", "1", "100", "100"]), "10,20 5x5");
        assert_eq!(g(["10", "20", "5", "5", "1.5", "100", "100"]), "15,30 8x8");
        assert_eq!(g(["98", "0", "5", "1", "2", "100", "100"]), "196,0 4x2");
        assert_eq!(g(["100", "0", "5", "1", "1", "100", "100"]), "100,0 1x1");
        assert_eq!(img_px(b"3", b"100", b"200"), Some(7));
        assert_eq!(img_px(b"3", b"0", b"200"), None);
    }
}
