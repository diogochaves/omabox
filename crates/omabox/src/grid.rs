//! shot --grid N (#268, finding 620): N lettered columns and N numbered rows drawn over a shot, their
//! labels in a margin top and left so nothing under them is hidden, for an agent that reads a point
//! off a shot with no accessibility tree. `--cell C3` (or `C3:D4`, the cells a line cuts a target
//! between) is a closer look at that cell, cropped, enlarged and gridded again; `click --in SHOT
//! --cell C3` clicks its middle. A grid shot's record (shots.tsv) has a sixth field, "grid N M", M
//! the margin, which --in takes off an image point first.

use crate::cat;
use crate::shell::{self, dec};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;

/// The labels' margin, image pixels, top and left.
pub const MARGIN: i64 = 28;
/// The smallest cell, in image pixels, that holds its label: a finer grid is refused.
pub const MIN_CELL: i64 = 12;

/// --grid N: 2 to 26 (A to Z).
pub fn n_arg(v: &[u8]) -> Option<i64> {
    shell::re_match("^([2-9]|1[0-9]|2[0-6])$", v)
        .then(|| dec(v))
        .flatten()
}

/// Cells from column C0, row R0 to C1, R1 (0-based, inclusive): one cell, or a rectangle of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub c0: i64,
    pub r0: i64,
    pub c1: i64,
    pub r1: i64,
}

/// "C3": column C, row 3, 0-based.
fn one(v: &[u8]) -> Option<(i64, i64)> {
    let (l, d) = v.split_first()?;
    let c = l.to_ascii_uppercase();
    if !c.is_ascii_uppercase() || !shell::re_match("^[1-9][0-9]?$", d) {
        return None;
    }
    Some((i64::from(c - b'A'), dec(d)? - 1))
}

/// --cell C3 or C3:D4 (any case, either corner first).
pub fn cell_arg(v: &[u8]) -> Option<Span> {
    let (a, b) = match v.iter().position(|&c| c == b':') {
        Some(i) => (&v[..i], &v[i + 1..]),
        None => (v, v),
    };
    let ((ca, ra), (cb, rb)) = (one(a)?, one(b)?);
    Some(Span {
        c0: ca.min(cb),
        r0: ra.min(rb),
        c1: ca.max(cb),
        r1: ra.max(rb),
    })
}

/// Column K's letter.
pub fn col(k: i64) -> char {
    char::from(b'A' + u8::try_from(k.clamp(0, 25)).unwrap_or(0))
}

impl Span {
    /// Inside an N x N grid.
    pub fn fits(&self, n: i64) -> bool {
        self.c1 < n && self.r1 < n
    }

    /// Its name: C3, or C3:D4.
    pub fn name(&self) -> String {
        let a = format!("{}{}", col(self.c0), self.r0 + 1);
        if self.c0 == self.c1 && self.r0 == self.r1 {
            a
        } else {
            format!("{a}:{}{}", col(self.c1), self.r1 + 1)
        }
    }
}

/// "a cell past an N x N grid" in words, for a refusal.
pub fn past(s: &Span, n: i64) -> String {
    format!(
        "{} is past the {n}x{n} grid (columns A-{}, rows 1-{n})",
        s.name(),
        col(n - 1)
    )
}

/// Where line K of N falls along LEN (0 and LEN the edges).
pub fn at(len: i64, n: i64, k: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    len.saturating_mul(k) / n
}

/// Span S of the region X,Y WxH cut in N x N, as X,Y WxH.
pub fn rect(r: [i64; 4], n: i64, s: Span) -> [i64; 4] {
    let [x, y, w, h] = r;
    let (x0, x1) = (at(w, n, s.c0), at(w, n, s.c1 + 1));
    let (y0, y1) = (at(h, n, s.r0), at(h, n, s.r1 + 1));
    [x + x0, y + y0, x1 - x0, y1 - y0]
}

/// Each cell's box in words, in the region's coordinates: "columns (x) A 0-239, B 240-479, ...;
/// rows (y) 1 0-134, ...".
pub fn boxes(r: [i64; 4], n: i64) -> String {
    let [x, y, w, h] = r;
    let cols: Vec<String> = (0..n)
        .map(|k| format!("{} {}-{}", col(k), x + at(w, n, k), x + at(w, n, k + 1) - 1))
        .collect();
    let rows: Vec<String> = (0..n)
        .map(|k| format!("{} {}-{}", k + 1, y + at(h, n, k), y + at(h, n, k + 1) - 1))
        .collect();
    format!(
        "columns (x) {}; rows (y) {}",
        cols.join(", "),
        rows.join(", ")
    )
}

/// What shot --json says of a grid over region R as an image IW x IH (its own pixels): N, the
/// margin, the file's size, each column's and row's place in the region's coordinates.
pub fn json(r: [i64; 4], n: i64, iw: i64, ih: i64) -> String {
    let [x, y, w, h] = r;
    let cols: Vec<String> = (0..n)
        .map(|k| {
            let a = at(w, n, k);
            format!(
                r#"{{"name":"{}","x":{},"w":{}}}"#,
                col(k),
                x + a,
                at(w, n, k + 1) - a
            )
        })
        .collect();
    let rows: Vec<String> = (0..n)
        .map(|k| {
            let a = at(h, n, k);
            format!(
                r#"{{"name":"{}","y":{},"h":{}}}"#,
                k + 1,
                y + a,
                at(h, n, k + 1) - a
            )
        })
        .collect();
    format!(
        r#"{{"n":{n},"margin":{MARGIN},"file":{{"w":{},"h":{}}},"columns":[{}],"rows":[{}]}}"#,
        iw + MARGIN,
        ih + MARGIN,
        cols.join(","),
        rows.join(",")
    )
}

/// The record's sixth field for a grid shot.
pub fn rec_field(n: i64) -> Vec<u8> {
    format!("grid {n} {MARGIN}").into_bytes()
}

/// A record's sixth field read: N and the margin; None when it is no grid's.
pub fn rec_parse(v: &[u8]) -> Option<(i64, i64)> {
    let f = shell::read_fields(v, 3);
    if f[0] != b"grid" {
        return None;
    }
    let (n, m) = (dec(&f[1])?, dec(&f[2])?);
    (n >= 2 && m >= 0).then_some((n, m))
}

/// The image pixel at the middle of span S of a grid shot of LW x LH as an IW x IH image (its
/// content, after the margin M): what --in maps back to the layout's middle of the span.
pub fn middle_px(sizes: [i64; 4], n: i64, m: i64, s: Span) -> Option<(i64, i64)> {
    let [lw, lh, iw, ih] = sizes;
    if lw <= 0 || lh <= 0 {
        return None;
    }
    let mx = (at(lw, n, s.c0) + at(lw, n, s.c1 + 1)) / 2;
    let my = (at(lh, n, s.r0) + at(lh, n, s.r1 + 1)) / 2;
    let px = (mx.saturating_mul(iw) / lw).clamp(0, iw - 1);
    let py = (my.saturating_mul(ih) / lh).clamp(0, ih - 1);
    Some((m + px, m + py))
}

/// Where grid line K of N is drawn in an image IMG pixels long of a layout LEN long (content
/// pixels; the last line on the last pixel).
fn line_px(len: i64, img: i64, n: i64, k: i64) -> i64 {
    if len <= 0 {
        return 0;
    }
    (at(len, n, k).saturating_mul(img) / len).clamp(0, img - 1)
}

/// magick's words that draw an N x N grid on an IW x IH image of an LW x LH layout: the margin, the
/// lines (dark under light, seen on any colour) through it as ticks, the labels in it.
pub fn draw_args(sizes: [i64; 4], n: i64, font: &[u8]) -> Vec<Vec<u8>> {
    let [lw, lh, iw, ih] = sizes;
    let m = MARGIN;
    let (xs, ys): (Vec<i64>, Vec<i64>) = (
        (0..=n).map(|k| m + line_px(lw, iw, n, k)).collect(),
        (0..=n).map(|k| m + line_px(lh, ih, n, k)).collect(),
    );
    let mut lines = String::new();
    for x in &xs {
        lines.push_str(&format!("line {x},0 {x},{} ", m + ih - 1));
    }
    for y in &ys {
        lines.push_str(&format!("line 0,{y} {},{y} ", m + iw - 1));
    }
    let lines = lines.trim_end().to_string();
    let mut a: Vec<String> = vec![
        "-background".into(),
        "#101010".into(),
        "-gravity".into(),
        "northwest".into(),
        "-splice".into(),
        format!("{m}x{m}"),
        "-fill".into(),
        "none".into(),
        "-stroke".into(),
        "#000000".into(),
        "-strokewidth".into(),
        "3".into(),
        "-draw".into(),
        lines.clone(),
        "-stroke".into(),
        "#ffd500".into(),
        "-strokewidth".into(),
        "1".into(),
        "-draw".into(),
        lines,
        "-stroke".into(),
        "none".into(),
        "-fill".into(),
        "#ffffff".into(),
    ];
    if !font.is_empty() {
        a.extend(["-font".into(), String::from_utf8_lossy(font).into_owned()]);
    }
    a.extend(["-pointsize".into(), "16".into()]);
    // A glyph of a 16 pt monospace is about 10 px wide and 12 tall.
    for k in 0..n as usize {
        let cx = (xs[k] + xs[k + 1]) / 2;
        a.extend(["-annotate".into(), format!("+{}+5", (cx - 5).max(0))]);
        a.push(col(k as i64).to_string());
        let cy = (ys[k] + ys[k + 1]) / 2;
        let label = (k + 1).to_string();
        let tw = 10 * i64::try_from(label.len()).unwrap_or(1);
        a.extend([
            "-annotate".into(),
            format!("+{}+{}", ((m - tw) / 2).max(0), (cy - 9).max(0)),
        ]);
        a.push(label);
    }
    a.extend([
        "-depth".into(),
        "8".into(),
        "-define".into(),
        "png:compression-level=1".into(),
        "png:-".into(),
    ]);
    a.into_iter().map(String::into_bytes).collect()
}

/// A bold monospace font's file, as fontconfig picks it; empty when it cannot say (magick's own
/// default then).
fn font() -> Vec<u8> {
    let mut c = crate::sig::command("fc-match");
    c.args(["-f", "%{file}", "monospace:bold"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match crate::sig::run_output(c) {
        Ok((0, f)) if f.starts_with(b"/") && shell::path(&f).is_file() => f,
        _ => Vec::new(),
    }
}

/// The grid drawn on SRC (an IW x IH image of an LW x LH layout) into TO; magick's words when it
/// fails.
pub fn draw(src: &[u8], to: OwnedFd, sizes: [i64; 4], n: i64) -> Result<(), Vec<u8>> {
    let mut c = crate::sig::command("magick");
    c.arg(std::ffi::OsStr::from_bytes(&cat!("png:", src)))
        .args(
            draw_args(sizes, n, &font())
                .iter()
                .map(|w| std::ffi::OsStr::from_bytes(w)),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(to))
        .stderr(std::process::Stdio::piped());
    let r = crate::sig::run_child(c, false, |ch| {
        let mut e = Vec::new();
        if let Some(mut o) = ch.stderr.take() {
            let _ = std::io::Read::read_to_end(&mut o, &mut e);
        }
        e
    });
    match r {
        Ok((_, (0, _))) => Ok(()),
        Ok((e, (rc, _))) => {
            let e = String::from_utf8_lossy(&e).replace('\n', " ");
            let e = e.trim();
            Err(if e.is_empty() {
                format!("magick ended with status {rc}").into_bytes()
            } else {
                e.as_bytes().to_vec()
            })
        }
        Err(e) => Err(format!("cannot run magick: {e}").into_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_read() {
        let s = |v: &str| cell_arg(v.as_bytes());
        assert_eq!(
            s("C3"),
            Some(Span {
                c0: 2,
                r0: 2,
                c1: 2,
                r1: 2
            })
        );
        assert_eq!(s("c3"), s("C3"), "any case");
        assert_eq!(s("D4:B2"), s("B2:D4"), "either corner first");
        assert_eq!(s("D2:B4").map(|s| s.name()), Some("B2:D4".to_string()));
        assert_eq!(s("C3:C3").map(|s| s.name()), Some("C3".to_string()));
        for bad in [
            "", "3C", "C0", "C", "CC3", "C100", "C3:", ":C3", "C3:D", "Ä3", "C03",
        ] {
            assert_eq!(s(bad), None, "{bad}");
        }
        assert!(s("H8").unwrap().fits(8));
        assert!(!s("I1").unwrap().fits(8));
        assert!(!s("A9").unwrap().fits(8));
        assert_eq!(
            past(&s("I1").unwrap(), 8),
            "I1 is past the 8x8 grid (columns A-H, rows 1-8)"
        );
    }

    #[test]
    fn n_read() {
        assert_eq!(n_arg(b"2"), Some(2));
        assert_eq!(n_arg(b"26"), Some(26));
        for bad in ["0", "1", "27", "08", "x", ""] {
            assert_eq!(n_arg(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn cells_cut() {
        // 1920x1080 in 8: 240 x 135 each.
        assert_eq!(
            rect([0, 0, 1920, 1080], 8, cell_arg(b"C3").unwrap()),
            [480, 270, 240, 135]
        );
        assert_eq!(
            rect([100, 50, 1920, 1080], 8, cell_arg(b"C3:D3").unwrap()),
            [580, 320, 480, 135],
            "a span, from the region's corner"
        );
        // Uneven: 1000 in 3 is 333, 333, 334, the whole covered.
        assert_eq!(
            rect([0, 0, 1000, 1000], 3, cell_arg(b"C3").unwrap()),
            [666, 666, 334, 334]
        );
        assert_eq!(
            boxes([10, 0, 100, 40], 2),
            "columns (x) A 10-59, B 60-109; rows (y) 1 0-19, 2 20-39"
        );
    }

    #[test]
    fn records() {
        assert_eq!(rec_field(8), b"grid 8 28");
        assert_eq!(rec_parse(b"grid 8 28"), Some((8, 28)));
        assert_eq!(rec_parse(b""), None);
        assert_eq!(rec_parse(b"grid x 28"), None);
    }

    #[test]
    fn middles() {
        // A whole 1920x1080 shot at 1000x562 behind a 28 px margin: C3's middle is 600,337 on the
        // screen, image pixel 312,175 of the content.
        let (x, y) = middle_px([1920, 1080, 1000, 562], 8, 28, cell_arg(b"C3").unwrap()).unwrap();
        assert_eq!((x, y), (28 + 312, 28 + 175));
        // At 1:1 it is the middle itself.
        assert_eq!(
            middle_px([1920, 1080, 1920, 1080], 8, 28, cell_arg(b"A1").unwrap()),
            Some((28 + 120, 28 + 67))
        );
    }

    #[test]
    fn lines_drawn() {
        let a = draw_args([1920, 1080, 1000, 562], 2, b"");
        let s: Vec<String> = a
            .iter()
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
        let d = s.iter().position(|w| w == "-draw").unwrap();
        // Lines through the margin, at 0, the middle and the last pixel of the content.
        assert_eq!(
            s[d + 1],
            "line 28,0 28,589 line 528,0 528,589 line 1027,0 1027,589 line 0,28 1027,28 line 0,309 1027,309 line 0,589 1027,589"
        );
        assert!(s.contains(&"A".to_string()) && s.contains(&"B".to_string()));
        assert!(s.contains(&"2".to_string()));
        assert!(!s.contains(&"-font".to_string()), "no font: magick's own");
        assert_eq!(s.last().map(String::as_str), Some("png:-"));
    }
}
