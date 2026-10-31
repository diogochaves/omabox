//! Finding an image on the screen (#267, finding 616): a template (a crop of an earlier shot) matched
//! against a frame grim took, for `find --image`, `click --image` and `wait image`. Plain Rust, no
//! image library: grim's PPM and ImageMagick's PAM are read here, and a PNG's text chunks (the scale a
//! shot was taken at, its comment) read and written.
//!
//! The score is normalised correlation of the colours, penalised for a contrast or a tone that is not
//! the template's (`2 Σ T'I' / (Σ T'² + Σ I'²)` times `1 - max |mean T - mean I| / 255`, per channel),
//! 1 for the same pixels: a grey switch does not pass for its blue self, and a brighter or dimmer copy
//! scores less. A template of one colour, where correlation is undefined, is scored by its squared
//! difference (`1 - RMS / 255`); a template's transparent pixels (alpha under 128) are not compared.
//! The search is coarse to fine: an exhaustive grey correlation on the template halved up to three
//! times (while it keeps 8 px a side), then each local best followed down, two pixels around, to the
//! full image, where the colours decide. Overlapping matches keep the best one.

/// An RGB image, 3 bytes a pixel, rows top down.
#[derive(Clone)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// A template: RGB, and which pixels count (None: all of them).
#[derive(Clone)]
pub struct Tpl {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    pub opaque: Option<Vec<bool>>,
}

impl Tpl {
    /// How many of its pixels count.
    pub fn counted(&self) -> usize {
        self.opaque
            .as_ref()
            .map_or(self.w * self.h, |m| m.iter().filter(|&&o| o).count())
    }
}

/// A match: the template's top left in the image, its score.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub x: usize,
    pub y: usize,
    pub score: f64,
}

/// What a search found: the matches at or over the score asked for (overlaps removed, in reading
/// order), the best score seen (passing or not), whether there were more than MAX_HITS.
#[derive(Debug, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub best: Option<Hit>,
    pub more: bool,
    /// The template is of one colour: scored by difference.
    pub flat: bool,
}

/// At most this many matches are kept (a template of one colour on a plain background matches
/// everywhere).
pub const MAX_HITS: usize = 1000;

/// The smallest template side searched for.
pub const MIN_SIDE: usize = 4;

// --- Image files -----------------------------------------------------------------------------------

/// A netpbm header's next token (comments skipped), from AT; its end.
fn token(b: &[u8], mut at: usize) -> Option<(&[u8], usize)> {
    loop {
        while at < b.len() && b[at].is_ascii_whitespace() {
            at += 1;
        }
        if at < b.len() && b[at] == b'#' {
            while at < b.len() && b[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        break;
    }
    let s = at;
    while at < b.len() && !b[at].is_ascii_whitespace() {
        at += 1;
    }
    (at > s).then_some((&b[s..at], at))
}

fn num(t: &[u8]) -> Option<usize> {
    if t.is_empty() || t.len() > 6 || !t.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(t).ok()?.parse().ok()
}

/// grim's PPM (P6, maxval 255).
pub fn ppm(b: &[u8]) -> Option<Rgb> {
    let (m, at) = token(b, 0)?;
    if m != b"P6" {
        return None;
    }
    let (w, at) = token(b, at)?;
    let (h, at) = token(b, at)?;
    let (mx, at) = token(b, at)?;
    let (w, h) = (num(w)?, num(h)?);
    if num(mx)? != 255 || w == 0 || h == 0 {
        return None;
    }
    // (one whitespace byte after maxval, then the pixels)
    let data = b.get(at + 1..)?;
    let n = w.checked_mul(h)?.checked_mul(3)?;
    (data.len() >= n).then(|| Rgb {
        w,
        h,
        px: data[..n].to_vec(),
    })
}

/// ImageMagick's PAM (`pam:-`): RGB_ALPHA, RGB, GRAYSCALE(_ALPHA), 8 bits. Alpha under 128 is a
/// pixel that does not count; a template all opaque has no mask.
pub fn pam(b: &[u8]) -> Option<Tpl> {
    let mut lines = 0usize;
    let mut at = 0usize;
    let (mut w, mut h, mut depth, mut maxv) = (0usize, 0usize, 0usize, 0usize);
    loop {
        let end = at + b.get(at..)?.iter().position(|&c| c == b'\n')?;
        let line = &b[at..end];
        at = end + 1;
        lines += 1;
        if lines == 1 {
            if line != b"P7" {
                return None;
            }
            continue;
        }
        if line.first() == Some(&b'#') || line.is_empty() {
            continue;
        }
        if line == b"ENDHDR" {
            break;
        }
        let mut f = line.split(|&c| c == b' ').filter(|f| !f.is_empty());
        let (k, v) = (f.next()?, f.next().unwrap_or_default());
        match k {
            b"WIDTH" => w = num(v)?,
            b"HEIGHT" => h = num(v)?,
            b"DEPTH" => depth = num(v)?,
            b"MAXVAL" => maxv = num(v)?,
            _ => {}
        }
        if lines > 64 {
            return None;
        }
    }
    if w == 0 || h == 0 || maxv != 255 || !(1..=4).contains(&depth) {
        return None;
    }
    let n = w.checked_mul(h)?;
    let data = b.get(at..at.checked_add(n.checked_mul(depth)?)?)?;
    let (mut px, mut op) = (Vec::with_capacity(n * 3), Vec::with_capacity(n));
    for p in data.chunks_exact(depth) {
        let (c, a) = match depth {
            1 => ([p[0]; 3], 255),
            2 => ([p[0]; 3], p[1]),
            3 => ([p[0], p[1], p[2]], 255),
            _ => ([p[0], p[1], p[2]], p[3]),
        };
        px.extend_from_slice(&c);
        op.push(a >= 128);
    }
    let opaque = (!op.iter().all(|&o| o)).then_some(op);
    Some(Tpl { w, h, px, opaque })
}

// --- PNG text ------------------------------------------------------------------------------------

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Whether B starts as a PNG does.
pub fn is_png(b: &[u8]) -> bool {
    b.starts_with(PNG_SIG)
}

/// A PNG's width and height, from its header.
pub fn png_dims(b: &[u8]) -> Option<(usize, usize)> {
    if !is_png(b) {
        return None;
    }
    let n = |o: usize| -> Option<usize> {
        Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
    };
    Some((n(16)?, n(20)?))
}

/// A chunk: its type, its data, where it starts and ends in the file.
type Chunk<'a> = ([u8; 4], &'a [u8], usize, usize);

/// A PNG's chunks; None at a chunk cut short.
fn chunks(b: &[u8]) -> Option<Vec<Chunk<'_>>> {
    if !is_png(b) {
        return None;
    }
    let mut at = PNG_SIG.len();
    let mut v = Vec::new();
    while at < b.len() {
        let len = u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
        let ty: [u8; 4] = b.get(at + 4..at + 8)?.try_into().ok()?;
        let end = at.checked_add(12)?.checked_add(len)?;
        let data = b.get(at + 8..at + 8 + len)?;
        if end > b.len() {
            return None;
        }
        v.push((ty, data, at, end));
        at = end;
        if &ty == b"IEND" {
            break;
        }
    }
    Some(v)
}

/// A tEXt chunk's comment (keyword `Comment`, any case).
fn comment_of(data: &[u8]) -> Option<&[u8]> {
    let i = data.iter().position(|&c| c == 0)?;
    data[..i]
        .eq_ignore_ascii_case(b"comment")
        .then(|| &data[i + 1..])
}

/// The scale a shot records in its PNG comment (`omabox scale=S`, image pixels per layout pixel);
/// None when it has none.
pub fn png_scale(b: &[u8]) -> Option<f64> {
    for (ty, data, ..) in chunks(b)? {
        if &ty != b"tEXt" {
            continue;
        }
        let Some(c) = comment_of(data) else { continue };
        let Some(rest) = c.strip_prefix(b"omabox ") else {
            continue;
        };
        for w in rest.split(|&c| c == b' ') {
            if let Some(v) = w.strip_prefix(b"scale=") {
                let s: f64 = std::str::from_utf8(v).ok()?.parse().ok()?;
                return (s.is_finite() && s > 0.0 && s <= 64.0).then_some(s);
            }
        }
    }
    None
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for p in parts {
        for &b in *p {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
    }
    !c
}

/// B with TEXT as its comment (a tEXt chunk `Comment` right after IHDR), an earlier omabox comment
/// taken out; None for what is not a PNG.
pub fn png_with_comment(b: &[u8], text: &[u8]) -> Option<Vec<u8>> {
    let cs = chunks(b)?;
    let (ty, _, _, ihdr_end) = *cs.first()?;
    if &ty != b"IHDR" {
        return None;
    }
    let mut data = b"Comment\0".to_vec();
    data.extend_from_slice(text);
    let mut out = Vec::with_capacity(b.len() + data.len() + 12);
    out.extend_from_slice(&b[..ihdr_end]);
    out.extend_from_slice(&u32::try_from(data.len()).ok()?.to_be_bytes());
    out.extend_from_slice(b"tEXt");
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc32(&[b"tEXt", &data]).to_be_bytes());
    let mut at = ihdr_end;
    for (ty, d, s, e) in &cs[1..] {
        if ty == b"tEXt" && comment_of(d).is_some_and(|c| c.starts_with(b"omabox ")) {
            out.extend_from_slice(&b[at..*s]);
            at = *e;
        }
    }
    out.extend_from_slice(&b[at..]);
    Some(out)
}

// --- Matching -----------------------------------------------------------------------------------

/// A grey plane.
struct Plane {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

fn grey(px: &[u8], w: usize, h: usize) -> Plane {
    let v = px
        .chunks_exact(3)
        .map(|p| {
            (77.0 * f32::from(p[0]) + 150.0 * f32::from(p[1]) + 29.0 * f32::from(p[2])) / 256.0
        })
        .collect();
    Plane { w, h, v }
}

/// A plane halved: each pixel the mean of the 2x2 it covers (of those that count, with a mask; one
/// counts when at least two of its four did).
fn half(p: &Plane, mask: Option<&[bool]>) -> (Plane, Option<Vec<bool>>) {
    let (w, h) = (p.w / 2, p.h / 2);
    let mut v = vec![0f32; w * h];
    let mut m = mask.map(|_| vec![false; w * h]);
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            let mut n = 0;
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let i = (2 * y + dy) * p.w + 2 * x + dx;
                if mask.is_none_or(|m| m[i]) {
                    s += p.v[i];
                    n += 1;
                }
            }
            if n > 0 {
                v[y * w + x] = s / n as f32;
            }
            if let Some(m) = m.as_mut() {
                m[y * w + x] = n >= 2;
            }
        }
    }
    (Plane { w, h, v }, m)
}

/// Each row's running sums of a plane and of its squares (w + 1 a row), for a run's sum at once.
struct Sums {
    w1: usize,
    s: Vec<f64>,
    q: Vec<f64>,
}

fn sums(p: &Plane) -> Sums {
    let w1 = p.w + 1;
    let (mut s, mut q) = (vec![0f64; w1 * p.h], vec![0f64; w1 * p.h]);
    for y in 0..p.h {
        for x in 0..p.w {
            let v = f64::from(p.v[y * p.w + x]);
            s[y * w1 + x + 1] = s[y * w1 + x] + v;
            q[y * w1 + x + 1] = q[y * w1 + x] + v * v;
        }
    }
    Sums { w1, s, q }
}

/// A template at one level: the runs of pixels that count (row, first x, length, where their values
/// start), those values less their mean, how many, the sum of their squares.
struct Runs {
    w: usize,
    h: usize,
    runs: Vec<(usize, usize, usize, usize)>,
    vals: Vec<f32>,
    n: f64,
    stt: f64,
}

fn runs(p: &Plane, mask: Option<&[bool]>) -> Runs {
    let mut rs = Vec::new();
    let mut vals = Vec::new();
    for y in 0..p.h {
        let mut x = 0;
        while x < p.w {
            if mask.is_some_and(|m| !m[y * p.w + x]) {
                x += 1;
                continue;
            }
            let s = x;
            while x < p.w && mask.is_none_or(|m| m[y * p.w + x]) {
                x += 1;
            }
            rs.push((y, s, x - s, vals.len()));
            vals.extend_from_slice(&p.v[y * p.w + s..y * p.w + x]);
        }
    }
    let n = vals.len() as f64;
    let mean = vals.iter().map(|&v| f64::from(v)).sum::<f64>() / n.max(1.0);
    let mut stt = 0f64;
    for v in &mut vals {
        let d = f64::from(*v) - mean;
        *v = d as f32;
        stt += d * d;
    }
    Runs {
        w: p.w,
        h: p.h,
        runs: rs,
        vals,
        n,
        stt,
    }
}

/// The grey correlation of template T at X,Y of plane P.
fn ncc(p: &Plane, s: &Sums, t: &Runs, x: usize, y: usize) -> f32 {
    let (mut num, mut si, mut qi) = (0f32, 0f64, 0f64);
    for &(dy, x0, len, off) in &t.runs {
        let row = (y + dy) * p.w + x + x0;
        let a = &p.v[row..row + len];
        let b = &t.vals[off..off + len];
        num += a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
        let r = (y + dy) * s.w1 + x + x0;
        si += s.s[r + len] - s.s[r];
        qi += s.q[r + len] - s.q[r];
    }
    let var = qi - si * si / t.n;
    // (a window of one grey, a quarter level from its mean, correlates with nothing)
    if var <= 0.25 * t.n || t.stt <= 0.0 {
        return 0.0;
    }
    (f64::from(num) / (t.stt * var).sqrt()) as f32
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
}

/// The score of every place of T in P, rows split between threads.
fn score_map(p: &Plane, s: &Sums, t: &Runs) -> (usize, usize, Vec<f32>) {
    let (pw, ph) = (p.w + 1 - t.w, p.h + 1 - t.h);
    let mut map = vec![0f32; pw * ph];
    let per = ph.div_ceil(threads()).max(1);
    std::thread::scope(|sc| {
        for (k, chunk) in map.chunks_mut(per * pw).enumerate() {
            sc.spawn(move || {
                for (i, v) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % pw, k * per + i / pw);
                    *v = ncc(p, s, t, x, y);
                }
            });
        }
    });
    (pw, ph, map)
}

/// The places of a score map that are local bests at or over THR, best first, at most CAP.
fn peaks(pw: usize, ph: usize, map: &[f32], thr: f32, cap: usize) -> Vec<(usize, usize, f32)> {
    let mut v = Vec::new();
    for y in 0..ph {
        for x in 0..pw {
            let s = map[y * pw + x];
            if s < thr {
                continue;
            }
            let mut best = true;
            'n: for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= pw as i64 || ny >= ph as i64 {
                        continue;
                    }
                    let o = map[ny as usize * pw + nx as usize];
                    // (on a plateau, the first in reading order)
                    let before = dy < 0 || (dy == 0 && dx < 0);
                    if o > s || (before && o == s) {
                        best = false;
                        break 'n;
                    }
                }
            }
            if best {
                v.push((x, y, s));
            }
        }
    }
    v.sort_by(|a, b| b.2.total_cmp(&a.2));
    v.truncate(cap);
    v
}

/// The template's colours at full size: per channel, its pixels that count (in run order) less
/// their mean, the means, the sum of the squares; one colour when its spread is under 2 levels.
struct Colour {
    runs: Vec<(usize, usize, usize, usize)>,
    vals: [Vec<f32>; 3],
    mean: [f64; 3],
    stt: f64,
    n: f64,
    flat: bool,
}

fn colour(t: &Tpl) -> Colour {
    let mut rs = Vec::new();
    let mut vals: [Vec<f32>; 3] = Default::default();
    let on = |i: usize| t.opaque.as_ref().is_none_or(|m| m[i]);
    for y in 0..t.h {
        let mut x = 0;
        while x < t.w {
            if !on(y * t.w + x) {
                x += 1;
                continue;
            }
            let s = x;
            while x < t.w && on(y * t.w + x) {
                x += 1;
            }
            rs.push((y, s, x - s, vals[0].len()));
            for xx in s..x {
                for (c, v) in vals.iter_mut().enumerate() {
                    v.push(f32::from(t.px[(y * t.w + xx) * 3 + c]));
                }
            }
        }
    }
    let n = vals[0].len() as f64;
    let mut mean = [0f64; 3];
    let mut stt = 0f64;
    for c in 0..3 {
        mean[c] = vals[c].iter().map(|&v| f64::from(v)).sum::<f64>() / n.max(1.0);
        for v in &mut vals[c] {
            let d = f64::from(*v) - mean[c];
            *v = d as f32;
            stt += d * d;
        }
    }
    // (spread: the root of the mean square, over the three channels)
    let flat = (stt / (3.0 * n.max(1.0))).sqrt() < 2.0;
    Colour {
        runs: rs,
        vals,
        mean,
        stt,
        n,
        flat,
    }
}

/// The colour score of the template at X,Y of IMG (see the module's notes).
fn colour_score(img: &Rgb, t: &Colour, x: usize, y: usize) -> f64 {
    let (mut si, mut qi, mut ti) = ([0f64; 3], [0f64; 3], 0f64);
    for &(dy, x0, len, off) in &t.runs {
        let row = ((y + dy) * img.w + x + x0) * 3;
        for k in 0..len {
            for c in 0..3 {
                let v = f64::from(img.px[row + k * 3 + c]);
                si[c] += v;
                qi[c] += v * v;
                ti += f64::from(t.vals[c][off + k]) * v;
            }
        }
    }
    let n = t.n;
    if t.flat {
        // Σ (I - μT)² = ΣI² - 2μTΣI + nμT², over the channels
        let mut d = 0f64;
        for c in 0..3 {
            d += qi[c] - 2.0 * t.mean[c] * si[c] + n * t.mean[c] * t.mean[c];
        }
        return 1.0 - (d.max(0.0) / (3.0 * n)).sqrt() / 255.0;
    }
    let mut sii = 0f64;
    let mut tone = 0f64;
    for c in 0..3 {
        sii += (qi[c] - si[c] * si[c] / n).max(0.0);
        tone = f64::max(tone, (t.mean[c] - si[c] / n).abs());
    }
    let corr = 2.0 * ti / (t.stt + sii);
    corr * (1.0 - tone / 255.0)
}

/// A region of the image not compared (x0, y0, x1, y1, in its pixels).
pub type Ignore = Option<[usize; 4]>;

/// The score at X,Y: colour_score, or with IGN over the template's place, its pixels there left out
/// (each sum made again: the template's means change); 0 when less than a quarter is left.
fn score_at(img: &Rgb, tpl: &Tpl, t: &Colour, x: usize, y: usize, ign: Ignore) -> f64 {
    let Some([x0, y0, x1, y1]) = ign else {
        return colour_score(img, t, x, y);
    };
    if x1 <= x || y1 <= y || x0 >= x + tpl.w || y0 >= y + tpl.h {
        return colour_score(img, t, x, y);
    }
    let (mut st, mut stt, mut si, mut sii, mut sti) =
        ([0f64; 3], [0f64; 3], [0f64; 3], [0f64; 3], [0f64; 3]);
    let mut n = 0f64;
    for ty in 0..tpl.h {
        for tx in 0..tpl.w {
            let (ix, iy) = (x + tx, y + ty);
            if tpl.opaque.as_ref().is_some_and(|m| !m[ty * tpl.w + tx])
                || (ix >= x0 && ix < x1 && iy >= y0 && iy < y1)
            {
                continue;
            }
            n += 1.0;
            for c in 0..3 {
                let tv = f64::from(tpl.px[(ty * tpl.w + tx) * 3 + c]);
                let iv = f64::from(img.px[(iy * img.w + ix) * 3 + c]);
                st[c] += tv;
                stt[c] += tv * tv;
                si[c] += iv;
                sii[c] += iv * iv;
                sti[c] += tv * iv;
            }
        }
    }
    if n < 0.25 * t.n || n < 16.0 {
        return 0.0;
    }
    if t.flat {
        let d: f64 = (0..3).map(|c| stt[c] - 2.0 * sti[c] + sii[c]).sum();
        return 1.0 - (d.max(0.0) / (3.0 * n)).sqrt() / 255.0;
    }
    let (mut cov, mut vt, mut vi, mut tone) = (0f64, 0f64, 0f64, 0f64);
    for c in 0..3 {
        cov += sti[c] - st[c] * si[c] / n;
        vt += (stt[c] - st[c] * st[c] / n).max(0.0);
        vi += (sii[c] - si[c] * si[c] / n).max(0.0);
        tone = f64::max(tone, ((st[c] - si[c]) / n).abs());
    }
    if vt + vi <= 0.0 {
        return 0.0;
    }
    2.0 * cov / (vt + vi) * (1.0 - tone / 255.0)
}

/// RGB blurred 3x3 (a box, the edges by what of it is inside).
fn blur(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut o = vec![0u8; px.len()];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut k) = ([0u32; 3], 0u32);
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    for (c, v) in s.iter_mut().enumerate() {
                        *v += u32::from(px[(yy * w + xx) * 3 + c]);
                    }
                    k += 1;
                }
            }
            for c in 0..3 {
                o[(y * w + x) * 3 + c] = ((s[c] + k / 2) / k) as u8;
            }
        }
    }
    o
}

/// The places of a template of one colour scoring SIM or more, at full size; with the best.
fn flat_search(img: &Rgb, tpl: &Tpl, t: &Colour, sim: f64, ign: Ignore) -> (Vec<Hit>, Option<Hit>) {
    let (tw, th) = (tpl.w, tpl.h);
    let (pw, ph) = (img.w + 1 - tw, img.h + 1 - th);
    let per = ph.div_ceil(threads()).max(1);
    let mut parts: Vec<(Vec<Hit>, Option<Hit>)> = Vec::new();
    std::thread::scope(|sc| {
        let hs: Vec<_> = (0..ph.div_ceil(per))
            .map(|k| {
                sc.spawn(move || {
                    let (mut v, mut best): (Vec<Hit>, Option<Hit>) = (Vec::new(), None);
                    for y in k * per..((k + 1) * per).min(ph) {
                        for x in 0..pw {
                            let score = score_at(img, tpl, t, x, y, ign);
                            let h = Hit { x, y, score };
                            if best.is_none_or(|b| score > b.score) {
                                best = Some(h);
                            }
                            if score >= sim {
                                v.push(h);
                            }
                        }
                    }
                    (v, best)
                })
            })
            .collect();
        for h in hs {
            if let Ok(r) = h.join() {
                parts.push(r);
            }
        }
    });
    let mut all = Vec::new();
    let mut best: Option<Hit> = None;
    for (v, b) in parts {
        all.extend(v);
        if let Some(b) = b
            && best.is_none_or(|x| b.score > x.score)
        {
            best = Some(b);
        }
    }
    (all, best)
}

/// The best per place first, each kept unless a better one kept overlaps it (their corners closer
/// than half the template each way); MAX_HITS at most, then MORE.
fn keep_apart(mut v: Vec<Hit>, tw: usize, th: usize) -> (Vec<Hit>, bool) {
    v.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then((a.y, a.x).cmp(&(b.y, b.x)))
    });
    let (hw, hh) = (tw.div_ceil(2).max(1), th.div_ceil(2).max(1));
    let mut grid: std::collections::HashMap<(usize, usize), Vec<usize>> =
        std::collections::HashMap::new();
    let mut kept: Vec<Hit> = Vec::new();
    for h in v {
        let (gx, gy) = (h.x / hw, h.y / hh);
        let mut near = false;
        'g: for cy in gy.saturating_sub(1)..=gy + 1 {
            for cx in gx.saturating_sub(1)..=gx + 1 {
                for &i in grid.get(&(cx, cy)).map_or(&[][..], Vec::as_slice) {
                    let k = kept[i];
                    if k.x.abs_diff(h.x) < hw && k.y.abs_diff(h.y) < hh {
                        near = true;
                        break 'g;
                    }
                }
            }
        }
        if near {
            continue;
        }
        if kept.len() == MAX_HITS {
            return (kept, true);
        }
        grid.entry((gx, gy)).or_default().push(kept.len());
        kept.push(h);
    }
    (kept, false)
}

/// Reading order: rows (a match within half the template's height of a row's first is in it), each
/// left to right.
fn reading_order(v: &mut [Hit], th: usize) {
    v.sort_by_key(|h| (h.y, h.x));
    let mut s = 0;
    while s < v.len() {
        let mut e = s + 1;
        while e < v.len() && v[e].y - v[s].y < th.div_ceil(2).max(1) {
            e += 1;
        }
        v[s..e].sort_by_key(|h| h.x);
        s = e;
    }
}

/// The coarsest level searched whole (0: the image itself): halved while the template keeps 8 px a
/// side and 24 that count.
fn top_level(img: &Rgb, t: &Tpl) -> usize {
    let mut k = 0;
    while k < 3 {
        let (w, h) = (t.w >> (k + 1), t.h >> (k + 1));
        let n = t.counted() >> (2 * (k + 1));
        if w < 8 || h < 8 || n < 24 || (img.w >> (k + 1)) < w || (img.h >> (k + 1)) < h {
            break;
        }
        k += 1;
    }
    k
}

/// Where TPL is in IMG, at SIM (0 to 1) or better. IGN: a region of IMG not compared (where the
/// pointer is drawn: x0, y0, x1, y1). SOFTEN: both blurred first (3x3 twice: a template scaled to
/// the box, whose edges and glyphs the box draws otherwise; text at 1.5 scored 0.75 blurred once, 0.82
/// twice).
pub fn search(img: &Rgb, tpl: &Tpl, sim: f64, ign: Ignore, soften: bool) -> Found {
    if soften {
        let i = blur(&blur(&img.px, img.w, img.h), img.w, img.h);
        let t = blur(&blur(&tpl.px, tpl.w, tpl.h), tpl.w, tpl.h);
        let img = Rgb {
            px: i,
            ..img.clone()
        };
        let tpl = Tpl {
            px: t,
            ..tpl.clone()
        };
        return search(&img, &tpl, sim, ign, false);
    }
    let mut f = Found::default();
    if tpl.w > img.w || tpl.h > img.h || tpl.counted() == 0 {
        return f;
    }
    let col = colour(tpl);
    f.flat = col.flat;
    if col.flat {
        let (v, best) = flat_search(img, tpl, &col, sim, ign);
        f.best = best;
        (f.hits, f.more) = keep_apart(v, tpl.w, tpl.h);
        reading_order(&mut f.hits, tpl.h);
        return f;
    }
    // The pyramid: level 0 the image itself.
    let top = top_level(img, tpl);
    let mut ip = vec![grey(&img.px, img.w, img.h)];
    let mut tp = vec![(grey(&tpl.px, tpl.w, tpl.h), tpl.opaque.clone())];
    for _ in 0..top {
        let i = half(ip.last().expect("a level"), None).0;
        let (tl, tm) = tp.last().expect("a level");
        let t = half(tl, tm.as_deref());
        ip.push(i);
        tp.push(t);
    }
    let ss: Vec<Sums> = ip.iter().map(sums).collect();
    let rs: Vec<Runs> = tp.iter().map(|(p, m)| runs(p, m.as_deref())).collect();
    const COARSE: f32 = 0.4;
    let (pw, ph, map) = score_map(&ip[top], &ss[top], &rs[top]);
    let mut cands: Vec<(usize, usize, f32)> = peaks(pw, ph, &map, COARSE, 1500);
    // (nothing near: the best place anyway, followed down for the best score to say)
    if cands.is_empty()
        && let Some((i, &v)) = map.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))
    {
        cands.push((i % pw, i / pw, v));
    }
    // Each followed down a level at a time: the best of the 5x5 around twice its place.
    for l in (0..top).rev() {
        let (p, s, t) = (&ip[l], &ss[l], &rs[l]);
        let (mx, my) = (p.w - t.w, p.h - t.h);
        let mut next: Vec<(usize, usize, f32)> = Vec::new();
        let mut seen: Option<(usize, usize, f32)> = None;
        for &(cx, cy, _) in &cands {
            let mut best: Option<(usize, usize, f32)> = None;
            for y in (2 * cy).saturating_sub(2)..=(2 * cy + 2).min(my) {
                for x in (2 * cx).saturating_sub(2)..=(2 * cx + 2).min(mx) {
                    let v = ncc(p, s, t, x, y);
                    if best.is_none_or(|b| v > b.2) {
                        best = Some((x, y, v));
                    }
                }
            }
            if let Some(b) = best {
                if seen.is_none_or(|s| b.2 > s.2) {
                    seen = Some(b);
                }
                if b.2 >= COARSE {
                    next.push(b);
                }
            }
        }
        // (none left: the best of them followed on, for the best score to say)
        if next.is_empty() {
            next.extend(seen);
        }
        next.sort_by(|a, b| b.2.total_cmp(&a.2).then((a.1, a.0).cmp(&(b.1, b.0))));
        next.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        cands = next;
    }
    let mut scored = Vec::new();
    for (x, y, _) in cands {
        let score = score_at(img, tpl, &col, x, y, ign);
        let h = Hit { x, y, score };
        if f.best.is_none_or(|b| score > b.score) {
            f.best = Some(h);
        }
        if score >= sim {
            scored.push(h);
        }
    }
    (f.hits, f.more) = keep_apart(scored, tpl.w, tpl.h);
    reading_order(&mut f.hits, tpl.h);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A busy test card: soft gradients, a grid of small marks of several colours, text-like bars.
    fn card(w: usize, h: usize) -> Rgb {
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                px[i] = (20 + (x * 40 / w)) as u8;
                px[i + 1] = (22 + (y * 30 / h)) as u8;
                px[i + 2] = 30;
                // marks every 37 x 29, each its own pattern
                let (cx, cy) = (x % 37, y % 29);
                let id = (x / 37) * 7 + (y / 29) * 13;
                if cx < 20 && cy < 12 && ((x * 73_856_093) ^ (y * 19_349_663)) % 7 < 3 {
                    px[i] = ((id * 53) % 200 + 50) as u8;
                    px[i + 1] = ((id * 97) % 200 + 40) as u8;
                    px[i + 2] = ((id * 31) % 200 + 30) as u8;
                }
            }
        }
        Rgb { w, h, px }
    }

    fn crop(img: &Rgb, x: usize, y: usize, w: usize, h: usize) -> Tpl {
        let mut px = Vec::with_capacity(w * h * 3);
        for yy in y..y + h {
            px.extend_from_slice(&img.px[(yy * img.w + x) * 3..(yy * img.w + x + w) * 3]);
        }
        Tpl {
            w,
            h,
            px,
            opaque: None,
        }
    }

    fn paste(img: &mut Rgb, t: &Tpl, x: usize, y: usize) {
        for yy in 0..t.h {
            for xx in 0..t.w {
                if t.opaque.as_ref().is_some_and(|m| !m[yy * t.w + xx]) {
                    continue;
                }
                let d = ((y + yy) * img.w + x + xx) * 3;
                let s = (yy * t.w + xx) * 3;
                img.px[d..d + 3].copy_from_slice(&t.px[s..s + 3]);
            }
        }
    }

    /// A template drawn in nowhere else: a ring with a dot, in orange.
    fn icon(w: usize, h: usize) -> Tpl {
        let mut px = vec![0u8; w * h * 3];
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
                let i = (y * w + x) * 3;
                let on = (d > cx * 0.6 && d < cx * 0.85) || d < cx * 0.2;
                let c = if on { [250, 140, 20] } else { [12, 14, 18] };
                px[i..i + 3].copy_from_slice(&c);
            }
        }
        Tpl {
            w,
            h,
            px,
            opaque: None,
        }
    }

    #[test]
    fn a_crop_is_found_where_it_was_cut() {
        let img = card(640, 400);
        for &(x, y, w, h) in &[
            (37, 29, 20, 12),
            (101, 203, 33, 21),
            (300, 77, 90, 60),
            (5, 5, 9, 7),
        ] {
            let t = crop(&img, x, y, w, h);
            let f = search(&img, &t, 0.95, None, false);
            assert!(
                f.hits
                    .iter()
                    .any(|h| h.x == x && h.y == y && h.score > 0.999),
                "{x},{y} {w}x{h}: {:?}",
                f.hits
            );
        }
    }

    #[test]
    fn an_icon_pasted_twice_is_two_matches_in_reading_order() {
        let mut img = card(800, 500);
        let t = icon(24, 24);
        paste(&mut img, &t, 600, 300);
        paste(&mut img, &t, 51, 303);
        paste(&mut img, &t, 333, 41);
        let f = search(&img, &t, 0.95, None, false);
        let at: Vec<(usize, usize)> = f.hits.iter().map(|h| (h.x, h.y)).collect();
        assert_eq!(at, vec![(333, 41), (51, 303), (600, 300)], "{:?}", f.hits);
        assert!(!f.more);
    }

    #[test]
    fn nothing_there_is_no_match_with_the_best_said() {
        let img = card(400, 300);
        let f = search(&img, &icon(30, 30), 0.95, None, false);
        assert!(f.hits.is_empty(), "{:?}", f.hits);
        assert!(f.best.is_some_and(|b| b.score < 0.95), "{:?}", f.best);
    }

    #[test]
    fn another_colour_or_contrast_does_not_pass() {
        let mut img = card(400, 300);
        let t = icon(24, 24);
        let mut grey = t.clone();
        for p in grey.px.chunks_exact_mut(3) {
            let g = ((u16::from(p[0]) + u16::from(p[1]) + u16::from(p[2])) / 3) as u8;
            p.copy_from_slice(&[g, g, g]);
        }
        paste(&mut img, &grey, 100, 100);
        let mut dim = t.clone();
        for v in &mut dim.px {
            *v /= 2;
        }
        paste(&mut img, &dim, 200, 150);
        let f = search(&img, &t, 0.95, None, false);
        assert!(f.hits.is_empty(), "{:?}", f.hits);
        paste(&mut img, &t, 300, 40);
        let f = search(&img, &t, 0.95, None, false);
        assert_eq!(f.hits.len(), 1, "{:?}", f.hits);
    }

    #[test]
    fn a_little_noise_still_matches() {
        let mut img = card(500, 300);
        let t = icon(32, 32);
        paste(&mut img, &t, 211, 133);
        // every pixel up to 6 levels off, as antialiasing might draw it
        for (i, v) in img.px.iter_mut().enumerate() {
            let d = ((i * 2_654_435_761) >> 7) % 13;
            *v = v.saturating_add(d as u8).saturating_sub(6);
        }
        let f = search(&img, &t, 0.95, None, false);
        assert_eq!(
            f.hits.iter().map(|h| (h.x, h.y)).collect::<Vec<_>>(),
            vec![(211, 133)],
            "{:?}",
            f.hits
        );
    }

    #[test]
    fn transparent_pixels_are_not_compared() {
        let mut img = card(400, 300);
        let mut t = icon(24, 24);
        paste(&mut img, &t, 150, 90);
        // the template's corners transparent and full of junk: still found
        let mut m = vec![true; 24 * 24];
        for y in 0..24 {
            for x in 0..24 {
                if x < 6 && y < 6 {
                    m[y * 24 + x] = false;
                    t.px[(y * 24 + x) * 3] = 255;
                }
            }
        }
        t.opaque = Some(m);
        let f = search(&img, &t, 0.95, None, false);
        assert_eq!(f.hits.len(), 1, "{:?}", f.hits);
        assert_eq!((f.hits[0].x, f.hits[0].y), (150, 90));
    }

    #[test]
    fn the_pointer_over_it_is_left_out() {
        let mut img = card(400, 300);
        let t = icon(40, 40);
        paste(&mut img, &t, 120, 80);
        // a white arrow over its middle
        for y in 0..24 {
            for x in 0..=y / 2 {
                let i = ((100 + y) * 400 + 140 + x) * 3;
                img.px[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let f = search(&img, &t, 0.95, None, false);
        assert!(f.hits.is_empty(), "{:?}", f.hits);
        let f = search(&img, &t, 0.95, Some([128, 88, 168, 132]), false);
        assert_eq!(
            f.hits.iter().map(|h| (h.x, h.y)).collect::<Vec<_>>(),
            vec![(120, 80)],
            "{:?}",
            f.hits
        );
        // blurred, the same match
        let f = search(&img, &t, 0.95, Some([128, 88, 168, 132]), true);
        assert_eq!(f.hits.len(), 1, "{:?}", f.hits);
    }

    #[test]
    fn one_colour_is_scored_by_difference() {
        let mut img = Rgb {
            w: 200,
            h: 100,
            px: vec![10; 200 * 100 * 3],
        };
        let red = Tpl {
            w: 8,
            h: 8,
            px: [200u8, 30, 30].repeat(64),
            opaque: None,
        };
        paste(&mut img, &red, 50, 40);
        let f = search(&img, &red, 0.95, None, false);
        assert!(f.flat);
        assert_eq!(f.hits.len(), 1, "{:?}", f.hits);
        assert_eq!((f.hits[0].x, f.hits[0].y), (50, 40));
        // the background's colour matches everywhere: many, each apart
        let bg = Tpl {
            w: 4,
            h: 4,
            px: vec![10; 48],
            opaque: None,
        };
        let f = search(&img, &bg, 0.95, None, false);
        assert!(f.hits.len() > 100);
        for (i, a) in f.hits.iter().enumerate() {
            for b in &f.hits[i + 1..] {
                assert!(a.x.abs_diff(b.x) >= 2 || a.y.abs_diff(b.y) >= 2);
            }
        }
    }

    #[test]
    fn a_template_larger_than_the_image_finds_nothing() {
        let img = card(50, 40);
        let f = search(&img, &icon(60, 20), 0.5, None, false);
        assert!(f.hits.is_empty() && f.best.is_none());
    }

    #[test]
    fn netpbm_headers() {
        let mut b = b"P6\n# grim\n2 1\n255\n".to_vec();
        b.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let i = ppm(&b).expect("a ppm");
        assert_eq!((i.w, i.h, i.px), (2, 1, vec![1, 2, 3, 4, 5, 6]));
        assert!(ppm(b"P6\n2 1\n255\n\x01").is_none());
        let mut a = b"P7\n#c\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
            .to_vec();
        a.extend_from_slice(&[1, 2, 3, 255, 4, 5, 6, 0]);
        let t = pam(&a).expect("a pam");
        assert_eq!(t.px, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(t.opaque, Some(vec![true, false]));
        let mut g = b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 2\nMAXVAL 255\nENDHDR\n".to_vec();
        g.extend_from_slice(&[9, 255]);
        let t = pam(&g).expect("grey");
        assert_eq!((t.px, t.opaque), (vec![9, 9, 9], None));
    }

    /// A 1x1 PNG (zlib stored block), as grim writes one but smaller.
    fn tiny_png() -> Vec<u8> {
        let chunk = |ty: &[u8], d: &[u8]| {
            let mut c = (d.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(ty);
            c.extend_from_slice(d);
            c.extend_from_slice(&crc32(&[ty, d]).to_be_bytes());
            c
        };
        let mut b = PNG_SIG.to_vec();
        b.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
        b.extend(chunk(
            b"IDAT",
            &[
                0x78, 0x01, 0x01, 0x04, 0x00, 0xfb, 0xff, 0, 1, 2, 3, 0x00, 0x0d, 0x00, 0x07,
            ],
        ));
        b.extend(chunk(b"IEND", &[]));
        b
    }

    #[test]
    fn a_shot_says_its_scale_in_its_comment() {
        let b = tiny_png();
        assert_eq!(png_scale(&b), None);
        let t = png_with_comment(&b, b"omabox scale=1.5").expect("tagged");
        assert_eq!(png_scale(&t), Some(1.5));
        // tagged again: one comment of omabox's, the new one
        let t2 = png_with_comment(&t, b"omabox scale=2").expect("again");
        assert_eq!(png_scale(&t2), Some(2.0));
        assert_eq!(t2.len(), t.len() - 2);
        // ImageMagick's lower-case keyword is read too
        let mut m = tiny_png();
        let d = b"comment\0omabox scale=0.5";
        let mut c = (d.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(b"tEXt");
        c.extend_from_slice(d);
        c.extend_from_slice(&crc32(&[b"tEXt", d]).to_be_bytes());
        m.splice(33..33, c);
        assert_eq!(png_scale(&m), Some(0.5));
        assert!(png_with_comment(b"GIF89a", b"x").is_none());
        // the CRC is the standard one
        assert_eq!(crc32(&[b"IEND"]), 0xae42_6082);
    }
}
