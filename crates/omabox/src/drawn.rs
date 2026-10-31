//! Where a layer spanning the screen draws over a window (#283, finding 618). Hyprland gives a
//! layer's rectangle only (finding 379), and Omarchy's notifications and dock span the screen while
//! taking input on their items (findings 398, 421), so a point under them was "may". What the box
//! can learn is where the layer draws: the screen's pixels there against the window's own (grim -T),
//! which differ only where something is drawn over the window. A toast or a dock icon is a cluster
//! of such pixels; a point inside one's box is under the item, a point far from all is not.

use crate::exec::{ErrTo, Out, on_box};
use crate::exit::Ctx;
use crate::jq::Args;
use crate::screen::win_buf_geom;
use crate::shell::{self, strip_nl};

/// An RGB image, as grim's PPM gives it.
#[derive(Debug, PartialEq, Eq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// A rectangle: x, y, w, h.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    fn holds(&self, x: i64, y: i64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    fn cut(&self, o: &Rect) -> Option<Rect> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = (
            (self.x + self.w).min(o.x + o.w),
            (self.y + self.h).min(o.y + o.h),
        );
        (x1 > x0 && y1 > y0).then_some(Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

/// grim's PPM ("P6\nW H\n255\n", then RGB); None for anything else or one cut short.
pub fn ppm(b: &[u8]) -> Option<Img> {
    let mut at = 0;
    let mut field = || -> Option<usize> {
        while at < b.len() && b[at].is_ascii_whitespace() {
            at += 1;
        }
        let s = at;
        while at < b.len() && !b[at].is_ascii_whitespace() {
            at += 1;
        }
        let f = &b[s..at];
        if f == b"P6" {
            return Some(0);
        }
        std::str::from_utf8(f).ok()?.parse().ok()
    };
    let (m, w, h, v) = (field()?, field()?, field()?, field()?);
    if m != 0 || v != 255 || w == 0 || h == 0 {
        return None;
    }
    // (one whitespace byte after the maximum value, then the pixels)
    let start = at + 1;
    let n = w.checked_mul(h)?.checked_mul(3)?;
    let px = b.get(start..start.checked_add(n)?)?.to_vec();
    Some(Img { w, h, px })
}

/// A channel apart by more than this is drawn over (a toast's border on a dark window is ~30 apart;
/// a window's opacity rule over a dark wallpaper well under it).
const DRAWN: u8 = 24;
/// The window's own pixels count as still where its two grabs are this close (a blinking cursor,
/// an animation in the window are not the layer).
const STILL: u8 = 8;
/// Cells of this many pixels; drawn cells this many cells apart are one item (a toast's text and its
/// border); a point this many cells from an item's box is on it.
const CELL: usize = 4;
const GAP: usize = 6;
const PAD: usize = 2;

fn apart(a: &[u8], b: &[u8], i: usize) -> u8 {
    (0..3)
        .map(|k| a[i + k].abs_diff(b[i + k]))
        .max()
        .unwrap_or(0)
}

/// What drawn_at found at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// Something is drawn there (an item of the layer: a toast, a dock icon).
    Here,
    /// Nothing is drawn there or near it.
    NotHere,
    /// It cannot tell: images of other sizes (a scaled output), a point outside them, one cluster
    /// over the whole region (something drawn everywhere: a dimming, a backdrop), a grab that failed.
    Unknown,
}

/// drawn_at SCREEN OWN OWN2 PX PY IGNORE: whether something is drawn over the window at image pixel
/// PX,PY: SCREEN the screen's pixels of a region, OWN and OWN2 the window's own of the same region,
/// grabbed before and after it, IGNORE regions (image pixels) left out (the pointer, the window's
/// rounded corners, what else is known to cover it). Here when the point is in (or by) the box of a
/// cluster of drawn pixels.
pub fn drawn_at(scr: &Img, own: &Img, own2: &Img, px: i64, py: i64, ignore: &[Rect]) -> Drawn {
    let (w, h) = (scr.w, scr.h);
    let n = w * h * 3;
    if (own.w, own.h) != (w, h)
        || (own2.w, own2.h) != (w, h)
        || scr.px.len() != n
        || own.px.len() != n
        || own2.px.len() != n
    {
        return Drawn::Unknown;
    }
    if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
        return Drawn::Unknown;
    }
    let (gw, gh) = (w.div_ceil(CELL), h.div_ceil(CELL));
    let mut cell = vec![false; gw * gh];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            if apart(&scr.px, &own.px, i) > DRAWN
                && apart(&own.px, &own2.px, i) <= STILL
                && !ignore.iter().any(|r| r.holds(x as i64, y as i64))
            {
                cell[(y / CELL) * gw + x / CELL] = true;
            }
        }
    }
    // Drawn cells GAP apart joined: each cell's reach GAP cells around it, then its parts.
    let mut near = vec![false; gw * gh];
    for cy in 0..gh {
        for cx in 0..gw {
            if cell[cy * gw + cx] {
                for ny in cy.saturating_sub(GAP)..(cy + GAP + 1).min(gh) {
                    for nx in cx.saturating_sub(GAP)..(cx + GAP + 1).min(gw) {
                        near[ny * gw + nx] = true;
                    }
                }
            }
        }
    }
    let mut part = vec![usize::MAX; gw * gh];
    // (per part: the box of its drawn cells, x0 y0 x1 y1 inclusive)
    let mut boxes: Vec<[usize; 4]> = Vec::new();
    for start in 0..gw * gh {
        if !near[start] || part[start] != usize::MAX {
            continue;
        }
        let id = boxes.len();
        let mut b = [usize::MAX, usize::MAX, 0, 0];
        let mut stack = vec![start];
        part[start] = id;
        while let Some(c) = stack.pop() {
            let (cx, cy) = (c % gw, c / gw);
            if cell[c] {
                b = [b[0].min(cx), b[1].min(cy), b[2].max(cx), b[3].max(cy)];
            }
            let mut push = |n: usize| {
                if near[n] && part[n] == usize::MAX {
                    part[n] = id;
                    stack.push(n);
                }
            };
            if cx > 0 {
                push(c - 1);
            }
            if cx + 1 < gw {
                push(c + 1);
            }
            if cy > 0 {
                push(c - gw);
            }
            if cy + 1 < gh {
                push(c + gw);
            }
        }
        boxes.push(b);
    }
    let (cx, cy) = (px as usize / CELL, py as usize / CELL);
    let Some(on) = boxes.iter().find(|b| {
        b[0] != usize::MAX
            && cx + PAD >= b[0]
            && cx <= b[2] + PAD
            && cy + PAD >= b[1]
            && cy <= b[3] + PAD
    }) else {
        return Drawn::NotHere;
    };
    // (one item the whole region: drawn everywhere, not an item)
    let whole = on[0] <= 1 && on[1] <= 1 && on[2] + 2 >= gw && on[3] + 2 >= gh;
    if whole && gw > 2 * GAP && gh > 2 * GAP {
        return Drawn::Unknown;
    }
    Drawn::Here
}

/// What else is known to cover the window: left out of the drawn pixels (image pixels of a grab).
const CORNER: i64 = 48;
/// The pointer's image around its hot spot (a 24 px theme's arrow or hand).
const POINTER: i64 = 48;

/// One grab in the box: grim's PPM on stdout, read; None when it failed.
fn grab(ctx: &Ctx, args: &[&[u8]]) -> Option<Img> {
    let mut argv: Vec<Vec<u8>> = [b"timeout".as_slice(), b"10", b"grim", b"-t", b"ppm"]
        .iter()
        .map(|a| a.to_vec())
        .collect();
    argv.extend(args.iter().map(|a| a.to_vec()));
    argv.push(b"-".to_vec());
    let r = on_box(ctx, &argv, true, Out::Capture(None), ErrTo::Null);
    if r.rc != 0 {
        return None;
    }
    ppm(&r.out)
}

/// `hyprctl -j WHAT` in the box, its words dropped; None when it failed (never dies: a click at the
/// screen's X Y leaves a desktop that does not answer to the input's own errors).
fn quiet(ctx: &Ctx, what: &[u8]) -> Option<Vec<u8>> {
    let argv: Vec<Vec<u8>> = [b"timeout".as_slice(), b"6", b"hyprctl", b"-j", what]
        .iter()
        .map(|a| a.to_vec())
        .collect();
    let r = on_box(
        ctx,
        &argv,
        true,
        Out::Capture(Some(16_777_216)),
        ErrTo::Null,
    );
    (r.rc == 0).then(|| strip_nl(&r.out))
}

fn nums(line: &[u8]) -> Vec<i64> {
    line.split(|&c| c == b' ').filter_map(shell::dec).collect()
}

/// layer_drawn WINDOW LAYER SX SY: whether a layer spanning the screen (its rectangle LAYER) draws
/// at the screen's SX,SY over WINDOW (a WIN_JQ object): the screen's pixels of where both are against
/// the window's own (grim -T, before and after), the pointer, the window's corners, the windows over
/// it and the layers certainly over it left out. Three grabs, only when such a layer is over the
/// point.
pub fn layer_drawn(ctx: &Ctx, w: &[u8], lay: Rect, sx: i64, sy: i64) -> Drawn {
    let r = Args::new(&["-r"])
        .arg(crate::jqprog::DRAWN_JQ)
        .run(Some(w), crate::jq::Err::Null);
    if !r.ok() {
        return Drawn::Unknown;
    }
    let text = r.text();
    let mut lines = text.split(|&c| c == b'\n');
    let head = lines.next().unwrap_or_default();
    let mut f = head.splitn(2, |&c| c == b' ');
    let (id, at) = (
        f.next().unwrap_or_default(),
        nums(f.next().unwrap_or_default()),
    );
    if !shell::re_match("^[0-9a-fA-F]+$", id) || at.len() != 4 {
        return Drawn::Unknown;
    }
    let win = Rect {
        x: at[0],
        y: at[1],
        w: at[2],
        h: at[3],
    };
    let Some(reg) = lay.cut(&win) else {
        return Drawn::Unknown;
    };
    if !reg.holds(sx, sy) {
        return Drawn::Unknown;
    }
    let mut over: Vec<Rect> = lines
        .map(nums)
        .filter(|n| n.len() == 4)
        .map(|n| Rect {
            x: n[0],
            y: n[1],
            w: n[2],
            h: n[3],
        })
        .collect();
    for (cx, cy) in [
        (win.x, win.y),
        (win.x + win.w - CORNER, win.y),
        (win.x, win.y + win.h - CORNER),
        (win.x + win.w - CORNER, win.y + win.h - CORNER),
    ] {
        over.push(Rect {
            x: cx,
            y: cy,
            w: CORNER,
            h: CORNER,
        });
    }
    let cur = quiet(ctx, b"cursorpos").unwrap_or_default();
    let c = Args::new(&["-r", r#""\(.x | floor) \(.y | floor)""#])
        .run(Some(&cur), crate::jq::Err::Null);
    if let [x, y] = nums(&c.text())[..] {
        over.push(Rect {
            x: x - POINTER / 4,
            y: y - POINTER / 4,
            w: POINTER,
            h: POINTER,
        });
    }
    // The window's own pixels of the region: in its buffer's pixels, at its scale (finding 300).
    let addr = crate::win::jq_r(".address", w);
    let (Some(cl), Some(mons)) = (quiet(ctx, b"clients"), quiet(ctx, b"monitors")) else {
        return Drawn::Unknown;
    };
    let r = Args::new(&["-r"])
        .kv("--arg", "a", &addr)
        .kv("--argjson", "m", &mons)
        .arg(crate::jqprog::WIN_SCALE_JQ)
        .run(Some(&cl), crate::jq::Err::Null);
    let sc = r.text();
    if !r.ok() || !shell::re_match(r"^[0-9]+(\.[0-9]+)?$", &sc) {
        return Drawn::Unknown;
    }
    let s = |v: i64| v.to_string().into_bytes();
    let g = win_buf_geom([
        &s(reg.x - win.x),
        &s(reg.y - win.y),
        &s(reg.w),
        &s(reg.h),
        &sc,
        &s(win.w),
        &s(win.h),
    ]);
    let sg = format!("{},{} {}x{}", reg.x, reg.y, reg.w, reg.h).into_bytes();
    let (Some(own), Some(scr), Some(own2)) = (
        grab(ctx, &[b"-T", id, b"-g", &g]),
        grab(ctx, &[b"-g", &sg]),
        grab(ctx, &[b"-T", id, b"-g", &g]),
    ) else {
        return Drawn::Unknown;
    };
    // Screen coordinates to the grab's pixels (an output's scale makes it larger than the region).
    let (iw, ih) = (scr.w as i64, scr.h as i64);
    let mx = |v: i64| (v - reg.x) * iw / reg.w;
    let my = |v: i64| (v - reg.y) * ih / reg.h;
    let ig: Vec<Rect> = over
        .iter()
        .map(|o| Rect {
            x: mx(o.x),
            y: my(o.y),
            w: (o.w * iw + reg.w - 1) / reg.w,
            h: (o.h * ih + reg.h - 1) / reg.h,
        })
        .collect();
    drawn_at(&scr, &own, &own2, mx(sx), my(sy), &ig)
}

/// line_rect LINE: the rectangle of a WIN_LAYER_JQ line (`... WxH at X,Y`, read from the right: a
/// namespace may hold spaces); None for a line not so made.
pub fn line_rect(lay: &[u8]) -> Option<Rect> {
    let f: Vec<&[u8]> = lay.rsplitn(4, |&c| c == b' ').collect();
    let (xy, kw, wh) = (f.first()?, f.get(1)?, f.get(2)?);
    if *kw != b"at" {
        return None;
    }
    let two = |v: &[u8], sep: u8| -> Option<(i64, i64)> {
        let i = v.iter().position(|&c| c == sep)?;
        Some((shell::dec(&v[..i])?, shell::dec(&v[i + 1..])?))
    };
    let ((x, y), (w, h)) = (two(xy, b',')?, two(wh, b'x')?);
    (w > 0 && h > 0).then_some(Rect { x, y, w, h })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A W x H image of one colour.
    fn img(w: usize, h: usize, c: [u8; 3]) -> Img {
        Img {
            w,
            h,
            px: c.repeat(w * h),
        }
    }

    /// The image with a rectangle's outline (a toast's border) drawn in a colour.
    fn ring(mut i: Img, r: Rect, c: [u8; 3]) -> Img {
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                if x == r.x || y == r.y || x == r.x + r.w - 1 || y == r.y + r.h - 1 {
                    let k = (y as usize * i.w + x as usize) * 3;
                    i.px[k..k + 3].copy_from_slice(&c);
                }
            }
        }
        i
    }

    const BG: [u8; 3] = [0x12, 0x13, 0x16];
    const EDGE: [u8; 3] = [0x40, 0x42, 0x48];
    const TOAST: Rect = Rect {
        x: 20,
        y: 10,
        w: 300,
        h: 70,
    };

    #[test]
    fn a_toast_is_drawn_where_it_is_and_nowhere_else() {
        let own = img(400, 600, BG);
        let scr = ring(img(400, 600, BG), TOAST, EDGE);
        // inside its border, its empty middle too, and just by it
        assert_eq!(drawn_at(&scr, &own, &own, 170, 45, &[]), Drawn::Here);
        assert_eq!(drawn_at(&scr, &own, &own, 22, 12, &[]), Drawn::Here);
        assert_eq!(drawn_at(&scr, &own, &own, 170, 84, &[]), Drawn::Here);
        // far below it, and beside it past the pad
        assert_eq!(drawn_at(&scr, &own, &own, 170, 500, &[]), Drawn::NotHere);
        assert_eq!(drawn_at(&scr, &own, &own, 350, 45, &[]), Drawn::NotHere);
        // nothing drawn: not here
        assert_eq!(drawn_at(&own, &own, &own, 170, 45, &[]), Drawn::NotHere);
    }

    #[test]
    fn a_faint_difference_is_not_drawn() {
        // (a window's opacity over a dark wallpaper: a few levels everywhere)
        let own = img(200, 200, BG);
        let scr = img(200, 200, [0x15, 0x16, 0x19]);
        assert_eq!(drawn_at(&scr, &own, &own, 50, 50, &[]), Drawn::NotHere);
    }

    #[test]
    fn drawn_everywhere_cannot_tell() {
        let own = img(200, 200, BG);
        let scr = img(200, 200, [0x80, 0x80, 0x80]);
        assert_eq!(drawn_at(&scr, &own, &own, 50, 50, &[]), Drawn::Unknown);
    }

    #[test]
    fn the_window_moving_or_left_out_is_not_drawn() {
        let own = img(400, 600, BG);
        let scr = ring(img(400, 600, BG), TOAST, EDGE);
        // the window's own pixels changed there between its grabs (a cursor blinking)
        let own2 = ring(img(400, 600, BG), TOAST, EDGE);
        assert_eq!(drawn_at(&scr, &own, &own2, 170, 45, &[]), Drawn::NotHere);
        // or left out (the pointer, a window over it)
        let out = [Rect {
            x: 0,
            y: 0,
            w: 400,
            h: 100,
        }];
        assert_eq!(drawn_at(&scr, &own, &own, 170, 45, &out), Drawn::NotHere);
    }

    #[test]
    fn other_sizes_or_a_point_outside_cannot_tell() {
        let own = img(400, 600, BG);
        let scr = ring(img(800, 1200, BG), TOAST, EDGE);
        assert_eq!(drawn_at(&scr, &own, &own, 170, 45, &[]), Drawn::Unknown);
        assert_eq!(drawn_at(&own, &own, &own, 400, 45, &[]), Drawn::Unknown);
        assert_eq!(drawn_at(&own, &own, &own, -1, 45, &[]), Drawn::Unknown);
    }

    #[test]
    fn grims_ppm_read() {
        let mut b = b"P6\n2 1\n255\n".to_vec();
        b.extend([1, 2, 3, 4, 5, 6]);
        assert_eq!(
            ppm(&b),
            Some(Img {
                w: 2,
                h: 1,
                px: vec![1, 2, 3, 4, 5, 6]
            })
        );
        assert_eq!(ppm(&b[..b.len() - 1]), None);
        assert_eq!(ppm(b"P5\n2 1\n255\n\0\0"), None);
        assert_eq!(ppm(b""), None);
    }

    #[test]
    fn a_layer_lines_rectangle() {
        assert_eq!(
            line_rect(br#"possible "omarchy notifications" overlay 465x1080 at 1455,0"#),
            Some(Rect {
                x: 1455,
                y: 0,
                w: 465,
                h: 1080
            })
        );
        assert_eq!(
            line_rect(b"possible \"x\" overlay 465x1080 by 1455,0"),
            None
        );
        assert_eq!(line_rect(b"possible"), None);
    }
}
