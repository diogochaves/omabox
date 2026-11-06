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

/// A monitor as px_cut reads it: its place in the layout, its scale and its size in pixels as laid
/// out (turned with its transform).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonPx {
    pub x: i64,
    pub y: i64,
    pub scale: f64,
    pub pw: i64,
    pub ph: i64,
}

/// The enabled monitors of `hyprctl -j monitors`.
pub fn mons_px(json: &[u8]) -> Vec<MonPx> {
    let Ok(serde_json::Value::Array(a)) = serde_json::from_slice(json) else {
        return Vec::new();
    };
    a.iter()
        .filter(|m| m["disabled"].as_bool() != Some(true))
        .filter_map(|m| {
            let i = |k: &str| m[k].as_f64().map(|v| v as i64);
            let (w, h) = (i("width")?, i("height")?);
            let scale = m["scale"].as_f64().filter(|s| *s > 0.0)?;
            let (pw, ph) = if i("transform").unwrap_or(0) % 2 == 1 {
                (h, w)
            } else {
                (w, h)
            };
            Some(MonPx {
                x: i("x")?,
                y: i("y")?,
                scale,
                pw,
                ph,
            })
        })
        .collect()
}

/// How a region of the screen is taken on whole pixels (#423, finding 641): grim takes GRAB, which
/// starts on a whole pixel of its monitor, and the image is CROP of grim's image (X, Y, W, H in its
/// pixels), the screen's own pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxCut {
    pub grab: [i64; 4],
    pub crop: [i64; 4],
}

/// The monitor that holds the region X Y W H (in the layout) whole; None for one over more than one
/// monitor, or of no size.
fn holder(mons: &[MonPx], [x, y, w, h]: [i64; 4]) -> Option<&MonPx> {
    if w < 1 || h < 1 {
        return None;
    }
    mons.iter().find(|m| {
        let (lw, lh) = (m.pw as f64 / m.scale, m.ph as f64 / m.scale);
        x >= m.x && y >= m.y && (x + w) as f64 <= m.x as f64 + lw + 1e-3 && {
            (y + h) as f64 <= m.y as f64 + lh + 1e-3
        }
    })
}

/// size_scale_in MONS X Y W H IW: an image IW px wide of the region's scale, image pixels per layout
/// pixel: its monitor's when grim's size at it (truncated) is IW, else IW / W. A crop's own sizes
/// are off by up to a pixel (23 at 1.25 is 28 px: 1.217), which find --image took for another
/// scale under 30 px or so (#423, finding 641).
pub fn size_scale_in(mons: &[MonPx], g: [i64; 4], iw: i64) -> f64 {
    let by_size = iw as f64 / g[2].max(1) as f64;
    match holder(mons, g) {
        Some(m) if px_len(g[2], m.scale) == iw => m.scale,
        _ => by_size,
    }
}

/// size_scale X Y W H IW: size_scale_in on the box's monitors now.
pub fn size_scale(ctx: &Ctx, g: [i64; 4], iw: i64) -> Res<f64> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    Ok(size_scale_in(&mons_px(&m), g, iw))
}

/// A whole number, as a scale's product is one (hyprctl writes 4/3 as 1.333333).
fn whole(v: f64) -> bool {
    (v - v.round()).abs() < 1e-3
}

// Layout and pixels (#425, finding 643): the one mapping shot -g's cut, find's regions (-g or the
// screen) and pixel take, so a crop is found where it was cut and a point read where its crop has it.
// A layout pixel L (from its monitor's edge) starts at L x S, rounded: its pixel (30 at 1.25 is 37.5,
// 38); the layout pixel of a pixel P is the one under its centre, so of L's own pixel it is L at any
// scale of 1 or more. A size is cut short to whole pixels, as grim's (63 at 1.25 is 78), and a size
// in pixels is the fewest layout pixels that hold it (78 is 63, 79 is 64: a match's region covers it).

/// A scale as hyprctl writes it, to the 120th it stands for (Wayland's fractional scales: 4/3 is
/// written 1.333333, a 1920 px monitor 1440.0004 layout px at that); any other as it is.
fn exact(s: f64) -> f64 {
    let n = (s * 120.0).round();
    if (s * 120.0 - n).abs() < 1e-3 && n > 0.0 {
        n / 120.0
    } else {
        s
    }
}

/// to_px L S: the pixel layout pixel L starts in, rounded.
pub fn to_px(l: i64, s: f64) -> i64 {
    (l as f64 * exact(s) + 0.5 + 1e-6).floor() as i64
}

/// px_len L S: L layout pixels in whole pixels, cut short (grim's sizes).
pub fn px_len(l: i64, s: f64) -> i64 {
    (l as f64 * exact(s) + 1e-4).floor() as i64
}

/// to_layout P S: the layout pixel under the centre of pixel P.
pub fn to_layout(p: i64, s: f64) -> i64 {
    ((p as f64 + 0.5) / exact(s) + 1e-6).floor() as i64
}

/// layout_len P S: the fewest layout pixels P pixels fit in.
pub fn layout_len(p: i64, s: f64) -> i64 {
    (p as f64 / exact(s) - 1e-4).ceil() as i64
}

/// start_px R LEN S PMAX: where a region R from its monitor's edge, LEN long, starts in pixels and
/// its size in them, kept on a monitor PMAX pixels long (at the far edge it ends on the last one).
fn start_px(r: i64, len: i64, s: f64, pmax: i64) -> (i64, i64) {
    let size = px_len(len, s).clamp(1, pmax.max(1));
    (to_px(r, s).min(pmax - size), size)
}

/// How a grab's pixels sit on the layout: pixel 0 of the image is pixel P0 from AT (a monitor's
/// corner, or the region's own when there is no one monitor's), S pixels to a layout pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxMap {
    pub at: [i64; 2],
    pub p0: [i64; 2],
    pub s: [f64; 2],
}

impl PxMap {
    /// A grab of region G, IW x IH, by its size: a window's own pixels, or a region over more than
    /// one monitor (grim draws it at one scale).
    pub fn by_size([x, y, w, h]: [i64; 4], iw: usize, ih: usize) -> PxMap {
        PxMap {
            at: [x, y],
            p0: [0, 0],
            s: [iw as f64 / w.max(1) as f64, ih as f64 / h.max(1) as f64],
        }
    }

    /// The layout region that holds the image's X Y W H: where its first pixel is, as many layout
    /// pixels as hold it.
    pub fn region(&self, [x, y, w, h]: [i64; 4]) -> [i64; 4] {
        [
            self.at[0] + to_layout(self.p0[0] + x, self.s[0]),
            self.at[1] + to_layout(self.p0[1] + y, self.s[1]),
            layout_len(w, self.s[0]).max(1),
            layout_len(h, self.s[1]).max(1),
        ]
    }

    /// The image's pixel of layout point X Y.
    pub fn pixel(&self, x: i64, y: i64) -> [i64; 2] {
        [
            to_px(x - self.at[0], self.s[0]) - self.p0[0],
            to_px(y - self.at[1], self.s[1]) - self.p0[1],
        ]
    }
}

/// px_map_in MONS G IW IH: how the image of region G (IW x IH, as shot -g, find -g and pixel take
/// it: cut by px_cut_in, or grim's own) sits on the layout: from its monitor's corner, at its scale,
/// when one holds it; else by its size.
pub fn px_map_in(mons: &[MonPx], g: [i64; 4], iw: usize, ih: usize) -> PxMap {
    match holder(mons, g) {
        Some(m) => PxMap {
            at: [m.x, m.y],
            p0: [
                start_px(g[0] - m.x, g[2], m.scale, m.pw).0,
                start_px(g[1] - m.y, g[3], m.scale, m.ph).0,
            ],
            s: [m.scale, m.scale],
        },
        None => PxMap::by_size(g, iw, ih),
    }
}

/// The box's monitors now, as px_cut_in and px_map_in read them.
pub fn mons(ctx: &Ctx) -> Res<Vec<MonPx>> {
    Ok(mons_px(&strip_nl(&hypr_json(ctx, &["monitors"])?)))
}

/// px_cut_in MONS X Y W H: how the region (in the layout, inside one monitor) is taken on whole
/// pixels; None when grim takes it so itself: a scale whose every layout pixel starts on a whole
/// pixel (1, 2), a region starting on one, or a region over more than one monitor (grim draws them at
/// one scale: not the screen's pixels whatever is done).
///
/// grim takes a region that does not start on a whole pixel (30 at 1.25 is 37.5) by resampling the
/// screen, so no pixel of it is the screen's. The image is the screen's pixels from where the
/// region starts, rounded (38), as many as grim makes (its size truncated: 100 at 1.25, 125), so its
/// size and every record of the shot are as before; grim takes the region from the whole pixel at or
/// before that (28, 35 px: a multiple of the scale's denominator, 4 at 1.25, from the monitor's
/// edge), wide enough to hold it.
pub fn px_cut_in(mons: &[MonPx], g: [i64; 4]) -> Option<PxCut> {
    let [x, y, w, h] = g;
    let m = holder(mons, g)?;
    let s = m.scale;
    if whole(s) {
        return None;
    }
    let (rx, ry) = (x - m.x, y - m.y);
    if whole(rx as f64 * s) && whole(ry as f64 * s) {
        return None;
    }
    let ((px, cw), (py, ch)) = (start_px(rx, w, s, m.pw), start_px(ry, h, s, m.ph));
    Some(cut_px(m, [rx, ry], [px, py], [px + cw, py + ch]))
}

/// px_hold_in MONS X Y W H: pixel's cut of a region at a fractional scale (#425): from the pixel of
/// its first layout pixel through that of its last (to_px), each point's own, where grim's size, cut
/// short, can end before the last's (8..10 at 1.25 is 10-12 px, and 10 starts in 13), or a region
/// grim takes itself (from 4 at 1.25) end so too; with how its pixels map. None as px_cut_in for a
/// whole scale or more than one monitor.
pub fn px_hold_in(mons: &[MonPx], g: [i64; 4]) -> Option<(PxCut, PxMap)> {
    let [x, y, w, h] = g;
    let m = holder(mons, g)?;
    let s = m.scale;
    if whole(s) {
        return None;
    }
    let (rx, ry) = (x - m.x, y - m.y);
    let a = [to_px(rx, s), to_px(ry, s)];
    let b = [to_px(rx + w - 1, s) + 1, to_px(ry + h - 1, s) + 1];
    let map = PxMap {
        at: [m.x, m.y],
        p0: a,
        s: [s, s],
    };
    Some((cut_px(m, [rx, ry], a, b), map))
}

/// cut_px M R A B: grim's grab of monitor M from the whole pixel at or before layout point R (from
/// its corner) through pixel B (not B itself), and the crop of it from pixel A: a grab starts on a
/// multiple of the scale's denominator (4 at 1.25, from the monitor's edge) and ends on the layout
/// pixel that holds B's start.
fn cut_px(m: &MonPx, r: [i64; 2], a: [i64; 2], b: [i64; 2]) -> PxCut {
    let s = m.scale;
    // The scale's denominator: layout pixels that make whole pixels (at most 120ths: Wayland's
    // fractional scales); none, and the grab starts at the monitor's edge.
    let q = (1..=120i64).find(|&k| whole(k as f64 * s));
    // One axis: grim's start and width in layout pixels, the crop's start and size in its pixels.
    let axis = |r: i64, a: i64, b: i64, pmax: i64| -> (i64, i64, i64, i64) {
        let r0 = match q {
            Some(q) => (r / q) * q,
            None => 0,
        };
        let lmax = (pmax as f64 / exact(s) + 1e-4) as i64;
        let gw = (layout_len(b, s) - r0).min(lmax - r0).max(1);
        (r0, gw, a - to_px(r0, s), b - a)
    };
    let (x0, gw, cx, cw) = axis(r[0], a[0], b[0], m.pw);
    let (y0, gh, cy, ch) = axis(r[1], a[1], b[1], m.ph);
    PxCut {
        grab: [m.x + x0, m.y + y0, gw, gh],
        crop: [cx, cy, cw, ch],
    }
}

/// px_cut X Y W H: px_cut_in on the box's monitors now.
pub fn px_cut(ctx: &Ctx, g: [i64; 4]) -> Res<Option<PxCut>> {
    let m = strip_nl(&hypr_json(ctx, &["monitors"])?);
    Ok(px_cut_in(&mons_px(&m), g))
}

/// img_px P IMAGE SCREEN: the screen pixel under the centre of image pixel P; None for a number that
/// is none, or an image of no size.
fn img_px(p: &[u8], image: &[u8], screen: &[u8]) -> Option<i64> {
    img_px_n(dec(p)?, dec(image)?, dec(screen)?)
}

/// img_px on numbers: by the image's size over the region's.
fn img_px_n(p: i64, i: i64, s: i64) -> Option<i64> {
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

/// shot_px_in MONS G IW IH X Y: the layout point of image pixel X,Y of a screen shot of region G,
/// IW x IH, for --in (#432, finding 676): the layout pixel under its centre by its monitor's mapping
/// (px_map_in, find's and pixel's) when one monitor holds the region and the image is grim's own size
/// at its scale (a cut crop is that size too); else by its size: a --fit or --zoom shot (a grid's
/// cell is one), or a region over more than one monitor (grim draws it at one scale). None for an
/// image of no size.
pub fn shot_px_in(
    mons: &[MonPx],
    g: [i64; 4],
    [iw, ih]: [i64; 2],
    [x, y]: [i64; 2],
) -> Option<[i64; 2]> {
    if iw > 0
        && ih > 0
        && let Some(m) = holder(mons, g)
        && px_len(g[2], m.scale) == iw
        && px_len(g[3], m.scale) == ih
    {
        let map = px_map_in(mons, g, iw as usize, ih as usize);
        let r = map.region([x, y, 1, 1]);
        return Some([r[0], r[1]]);
    }
    Some([
        g[0].saturating_add(img_px_n(x, iw, g[2])?),
        g[1].saturating_add(img_px_n(y, ih, g[3])?),
    ])
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

/// A grid shot's record (#268): its layout and image sizes ("LWxLH IWxIH", the image's pixels
/// after the margin), N and the margin; refused, said, for a shot that is not one.
pub fn shot_grid(ctx: &Ctx, file: &[u8]) -> Res<([i64; 4], i64, i64)> {
    let r = crate::shotcmp::load(ctx, file, "--in")?;
    let Some((n, m)) = crate::grid::rec_parse(&r.grid) else {
        return Err(ctx.die(cat!(
            "--cell: ",
            r.key,
            " is not a grid shot (take one with shot --grid N)"
        )));
    };
    match r.sizes() {
        Some([a, b, c, d]) if a > 0 && b > 0 && c > 0 && d > 0 => Ok(([a, b, c, d], n, m)),
        _ => Err(ctx.die(cat!(
            "--in: ",
            r.key,
            ": its record in shots.tsv is not one omabox reads"
        ))),
    }
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
    // A grid shot (#268): its pixels start after the labels' margin, top and left.
    let (x, y) = match crate::grid::rec_parse(&r.grid) {
        Some((_, m)) => {
            let (Some(xi), Some(yi), Some(wi), Some(hi)) =
                (shell::test_int(x), shell::test_int(y), dec(iw), dec(ih))
            else {
                return Err(ctx.die(cat!(
                    "--in: ",
                    key,
                    ": its record in shots.tsv is not one omabox reads"
                )));
            };
            if (xi < m || yi < m) && xi < wi + m && yi < hi + m && xi >= 0 && yi >= 0 {
                return Err(ctx.die(cat!(
                    "--in: ",
                    x,
                    ",",
                    y,
                    " is in the grid's margin (its labels, ",
                    m.to_string(),
                    " px top and left): the shot's pixels start at ",
                    m.to_string(),
                    ",",
                    m.to_string()
                )));
            }
            if !(xi < wi + m && yi < hi + m) {
                return Err(ctx.die(cat!(
                    "--in: ",
                    x,
                    ",",
                    y,
                    " is outside the image (",
                    (wi + m).to_string(),
                    "x",
                    (hi + m).to_string(),
                    ")"
                )));
            }
            (
                (xi - m).to_string().into_bytes(),
                (yi - m).to_string().into_bytes(),
            )
        }
        None => (x.to_vec(), y.to_vec()),
    };
    let (x, y) = (x.as_slice(), y.as_slice());
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
    // (on the monitors now: a shot taken in another screen mode was refused)
    let n = |v: &[u8]| dec(v).ok_or_else(unread);
    let Some([sx, sy]) = shot_px_in(
        &mons(ctx)?,
        [ox, oy, n(lw)?, n(lh)?],
        [n(iw)?, n(ih)?],
        [n(x)?, n(y)?],
    ) else {
        return Err(unread());
    };
    Ok((Point::Screen(sx, sy), r))
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

    // #423, finding 641: a region off a whole pixel at a fractional scale is grabbed from one and cut.
    #[test]
    fn a_region_off_a_whole_pixel_is_cut_on_whole_pixels() {
        let mon = |x, y, scale, pw, ph| MonPx {
            x,
            y,
            scale,
            pw,
            ph,
        };
        let one = |scale| vec![mon(0, 0, scale, 1920, 1200)];
        let cut = |g, crop| Some(PxCut { grab: g, crop });
        // 30 at 1.25 is 37.5: grim takes it from 28 (35 px), the image from 38, 125x62 as grim's.
        assert_eq!(
            px_cut_in(&one(1.25), [30, 40, 100, 50]),
            cut([28, 40, 103, 50], [3, 0, 125, 62]),
            "x off, y on"
        );
        assert_eq!(
            px_cut_in(&one(1.25), [76, 100, 40, 20]),
            None,
            "76 and 100 are 95 and 125 px: grim's own"
        );
        assert_eq!(
            px_cut_in(&one(1.25), [31, 33, 10, 10]),
            cut([28, 32, 13, 11], [4, 1, 12, 12]),
            "both off: 38.75 and 41.25 rounded to 39 and 41"
        );
        assert_eq!(
            px_cut_in(&one(1.5), [31, 3, 101, 7]),
            cut([30, 2, 102, 8], [2, 2, 151, 10]),
            "1.5: whole every 2, 46.5 to 47, 4.5 to 5"
        );
        for s in [1.0, 2.0, 3.0] {
            assert_eq!(px_cut_in(&one(s), [31, 33, 10, 10]), None, "scale {s}");
        }
        // At the far edge: 1917.5 rounded to 1918, 2 px, the image ends on the screen's last pixel.
        let c = px_cut_in(&one(1.25), [1534, 957, 2, 3]).unwrap();
        assert_eq!(c.grab[0] + c.grab[2], 1536, "the grab inside the layout");
        assert_eq!((c.crop[2], c.crop[3]), (2, 3));
        assert_eq!(1532 * 5 / 4 + c.crop[0] + c.crop[2], 1920);
        // A second monitor: its own edge is where its pixels start.
        let two = vec![mon(0, 0, 1.0, 1920, 1200), mon(1920, 0, 1.25, 1920, 1200)];
        assert_eq!(
            px_cut_in(&two, [1950, 40, 100, 50]),
            cut([1948, 40, 103, 50], [3, 0, 125, 62])
        );
        assert_eq!(
            px_cut_in(&two, [30, 40, 100, 50]),
            None,
            "on the scale-1 one"
        );
        assert_eq!(
            px_cut_in(&two, [1900, 40, 100, 50]),
            None,
            "over both: left to grim"
        );
        // 4/3, as hyprctl writes it: whole every 3 layout pixels.
        let c = px_cut_in(&[mon(0, 0, 1.333333, 1920, 1200)], [31, 30, 30, 30]).unwrap();
        assert_eq!((c.grab[0], c.crop), (30, [1, 0, 40, 40]));
        // A turned monitor: its pixels as laid out.
        let m = mons_px(
            br#"[{"x":0,"y":0,"width":1920,"height":1200,"scale":1.25,"transform":1},
            {"x":5,"y":0,"width":10,"height":10,"scale":1,"disabled":true}]"#,
        );
        assert_eq!(m, vec![mon(0, 0, 1.25, 1200, 1920)]);
        assert_eq!(
            px_cut_in(&m, [30, 1400, 100, 50]),
            cut([28, 1400, 103, 50], [3, 0, 125, 62])
        );
        assert_eq!(px_cut_in(&m, [30, 1500, 100, 50]), None, "past its 1536");
        // The scale a crop records: its monitor's when grim's size says so.
        assert_eq!(
            size_scale_in(&one(1.25), [30, 40, 23, 9], 28),
            1.25,
            "not 28/23"
        );
        assert_eq!(
            size_scale_in(&one(1.5), [30, 40, 11, 9], 16),
            1.5,
            "not 16/11"
        );
        assert_eq!(
            size_scale_in(&one(1.25), [30, 40, 20, 9], 10),
            0.5,
            "--fit's: by its size"
        );
        assert_eq!(
            size_scale_in(&two, [1900, 40, 100, 9], 125),
            1.25,
            "over two: by its size"
        );
    }

    // #432, finding 676: click --in a pixel of a screen shot cut at a fractional scale is the layout
    // pixel under its centre, as find and pixel map it, not by the image's size over the region's.
    #[test]
    fn a_shots_pixel_is_its_own_layout_pixel() {
        let mon = |x, scale| MonPx {
            x,
            y: 0,
            scale,
            pw: 1920,
            ph: 1200,
        };
        for s in [1.25, 1.5, 1.333333] {
            let m = vec![mon(0, s)];
            let (lw, lh) = ((1920.0 / s + 1e-3) as i64, (1200.0 / s + 1e-3) as i64);
            for x in (0..48).chain(lw - 40..lw) {
                for w in (1..40).filter(|w| x + w <= lw) {
                    let (y, h) = (lh - 7, 7);
                    let g = [x, y, w, h];
                    let (iw, ih) = (px_len(w, s), px_len(h, s));
                    // The screen pixel the image starts on: shot's cut, else grim's own (on a whole one).
                    let p0 = match px_cut_in(&m, g) {
                        Some(c) => [
                            to_px(c.grab[0], s) + c.crop[0],
                            to_px(c.grab[1], s) + c.crop[1],
                        ],
                        None => [to_px(x, s), to_px(y, s)],
                    };
                    for i in 0..iw {
                        let j = i % ih;
                        assert_eq!(
                            shot_px_in(&m, g, [iw, ih], [i, j]),
                            Some([to_layout(p0[0] + i, s), to_layout(p0[1] + j, s)]),
                            "scale {s}: pixel {i},{j} of {g:?} ({iw}x{ih} from {p0:?})"
                        );
                    }
                }
            }
        }
        // By its size: a --zoom or --fit shot (a grid's cell is a --zoom), a region over two
        // monitors; a whole scale maps the same either way.
        let m = vec![mon(0, 1.25)];
        let g = [30, 40, 100, 50];
        assert_eq!(
            shot_px_in(&m, g, [500, 250], [499, 0]),
            Some([129, 40]),
            "--zoom 4"
        );
        assert_eq!(shot_px_in(&m, g, [50, 25], [1, 1]), Some([33, 43]), "--fit");
        let two = vec![mon(0, 1.0), mon(1920, 1.25)];
        assert_eq!(
            shot_px_in(&two, [1900, 0, 100, 10], [125, 12], [124, 11]),
            Some([1999, 9]),
            "over two"
        );
        assert_eq!(
            shot_px_in(&[mon(0, 2.0)], [31, 33, 10, 10], [20, 20], [19, 0]),
            Some([40, 33])
        );
        assert_eq!(shot_px_in(&m, g, [0, 62], [0, 0]), None, "no size");
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

    // #425, finding 643: one mapping, so a crop is found where it was cut, in the whole screen or a
    // -g region around it, and pixel reads a point where its crop has it.
    #[test]
    fn a_crop_maps_back_to_where_it_was_cut() {
        let mon = |scale| {
            vec![MonPx {
                x: 0,
                y: 0,
                scale,
                pw: 1920,
                ph: 1200,
            }]
        };
        // t_find_image's in a VM at 1.25: 66,59 63x16 is 78x20 px from 83,74 (82.5 and 73.75 rounded)
        let m = mon(1.25);
        let screen = px_map_in(&m, [0, 0, 1536, 960], 1920, 1200);
        assert_eq!((to_px(66, 1.25), to_px(59, 1.25)), (83, 74));
        assert_eq!((px_len(63, 1.25), px_len(16, 1.25)), (78, 20));
        assert_eq!(
            screen.region([83, 74, 78, 20]),
            [66, 59, 63, 16],
            "not 62 wide"
        );
        let g = [45, 48, 105, 38];
        let around = px_map_in(&m, g, px_len(105, 1.25) as usize, px_len(38, 1.25) as usize);
        assert_eq!(around.p0, [56, 60], "56.25 and 60 rounded");
        assert_eq!(
            around.region([83 - 56, 74 - 60, 78, 20]),
            [66, 59, 63, 16],
            "not 67"
        );
        assert_eq!(
            layout_len(79, 1.25),
            64,
            "a size between two: the region that holds it"
        );
        // Every crop at 1.25, 1.5 and 4/3 (as hyprctl writes it), anywhere, any size: found where it
        // was cut in the screen and in any region around it, and each of its points read from the
        // pixel its cut has there.
        for s in [1.25, 1.5, 1.333333, 1.0, 2.0] {
            let m = mon(s);
            let (lw, lh) = ((1920.0 / s + 1e-3) as i64, (1200.0 / s + 1e-3) as i64);
            let full = px_map_in(&m, [0, 0, lw, lh], 1920, 1200);
            assert_eq!(
                full.p0,
                [0, 0],
                "scale {s}: the whole screen is its monitor's"
            );
            for x in (7..47).chain(lw - 40..lw) {
                for w in (1..30).filter(|w| x + w <= lw) {
                    let cut = [x, 9, w, 5];
                    let (p, len) = (px_map_in(&m, cut, 0, 0).p0[0], px_len(w, s));
                    assert_eq!(
                        p,
                        to_px(x, s),
                        "scale {s}: {cut:?} starts on its rounded pixel"
                    );
                    assert_eq!(full.region([p, 0, len, 1])[0], x, "scale {s}: {cut:?}");
                    assert_eq!(full.region([p, 0, len, 1])[2], w, "scale {s}: {cut:?}");
                    for gx in (x - 30).max(0)..=x {
                        let gw = (x + w - gx + 3).min(lw - gx);
                        let a = px_map_in(&m, [gx, 0, gw, 50], 0, 0);
                        let r = a.region([p - a.p0[0], 0, len, 1]);
                        assert_eq!((r[0], r[2]), (x, w), "scale {s}: {cut:?} in -g {gx} {gw}");
                    }
                    // (each cut inside what grim makes of its grab)
                    let in_grab = |c: PxCut| {
                        let (g0, cx) = (to_px(c.grab[0], s), c.crop[0]);
                        cx >= 0 && g0 + cx + c.crop[2] <= g0 + px_len(c.grab[2], s)
                    };
                    if let Some(k) = px_cut_in(&m, cut) {
                        assert!(in_grab(k), "scale {s}: {cut:?} {k:?}");
                    }
                    let Some((k, h)) = px_hold_in(&m, cut) else {
                        assert!(whole(s), "scale {s}: pixel's cut of {cut:?}");
                        continue;
                    };
                    assert!(in_grab(k), "scale {s}: pixel's {cut:?} {k:?}");
                    for q in x..x + w {
                        let i = h.pixel(q, 9)[0];
                        assert!(
                            (0..k.crop[2]).contains(&i),
                            "scale {s}: {q} of {cut:?} in {k:?}"
                        );
                        assert_eq!(i + h.p0[0], to_px(q, s), "scale {s}: {q} of {cut:?}");
                        assert_eq!(
                            full.region([to_px(q, s), 0, 1, 1])[0],
                            q,
                            "scale {s}: back to {q}"
                        );
                    }
                }
            }
        }
        // Over two monitors, or a window's own pixels: by the image's size, as before at a whole scale.
        let w = PxMap::by_size([100, 50, 300, 200], 600, 400);
        assert_eq!(w.region([40, 20, 41, 10]), [120, 60, 21, 5]);
        assert_eq!(w.pixel(120, 60), [40, 20]);
        assert_eq!(exact(1.333333), 160.0 / 120.0);
        assert_eq!(exact(1.07), 1.07, "not a 120th: as it is");
    }
}
