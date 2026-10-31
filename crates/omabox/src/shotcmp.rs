//! An earlier shot against the box now (#333, #344, findings 592, 593): what `--in` and `wait
//! --since/--like` read of a shot (its record, the file unchanged, the box in the same mode: load);
//! the desktop's layout at a screen shot ($D/layouts.tsv) against the layout now at a point
//! (layout_check, for click, scroll, drag and pointer --in); and its pixels against a new frame of
//! what it covered (grab, differ: wait change --since, wait still --like), where a caret alone, the
//! pointer and --ignore's regions are no difference.

use crate::cat;
use crate::desk::{Target, desk_facts, desk_snap, desk_target_with};
use crate::exec::{ErrTo, Out, on_box};
use crate::exit::{Ctx, Res};
use crate::jail::in_jail;
use crate::jq;
use crate::screen::{box_screen, stamp, tsv_last};
use crate::shell::{self, dec};
use crate::shots::{caret_shaped, diff_box, mkdir_p, png_size, to_image_px};
use crate::win::{box_windows, jq_r, win_desc};

/// A recorded shot of this box, as shots.tsv has it: FILE the one to read (a jailed caller's
/// /dev/fd/N), KEY its name there, its stamp, what it is of ("screen X,Y",
/// "window ADDRESS" or "window ADDRESS X,Y WxH") and "LWxLH IWxIH".
pub struct ShotRef {
    pub file: Vec<u8>,
    pub key: Vec<u8>,
    pub stamp: Vec<u8>,
    pub kind: Vec<u8>,
    pub geo: Vec<u8>,
    /// A grid shot's sixth field, "grid N M" (#268: N cells a side, an M px margin), else empty.
    pub grid: Vec<u8>,
}

impl ShotRef {
    /// Its layout size and image size.
    pub fn sizes(&self) -> Option<[i64; 4]> {
        let g = shell::read_ifs(&self.geo, 4, b"x ");
        Some([dec(&g[0])?, dec(&g[1])?, dec(&g[2])?, dec(&g[3])?])
    }

    /// A screen shot's top left, in the layout; None for a window's.
    pub fn screen_at(&self) -> Option<(i64, i64)> {
        let p = self.kind.strip_prefix(b"screen ".as_slice())?;
        let i = p.iter().position(|&c| c == b',')?;
        Some((dec(&p[..i])?, dec(&p[i + 1..])?))
    }

    /// A window shot's address (and the crop's words after it, when it is one).
    pub fn window(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let p = self.kind.strip_prefix(b"window ".as_slice())?;
        Some(match p.iter().position(|&c| c == b' ') {
            Some(i) => (p[..i].to_vec(), p[i + 1..].to_vec()),
            None => (p.to_vec(), Vec::new()),
        })
    }
}

/// shot_point's reading of FILE (LABEL in what it says: "--in", "wait --since"): a shot of this box,
/// unchanged since, taken in the box's mode now. A jailed caller's is the /dev/fd/N its omabox passed,
/// with its name ($OMABOX_RELAY_IN_AS).
pub fn load(ctx: &Ctx, file: &[u8], label: &str) -> Res<ShotRef> {
    let (f, key) = if in_jail() {
        let as_ = shell::env_or_empty("OMABOX_RELAY_IN_AS");
        if !(shell::re_match("^/dev/fd/[3-9]$", file)
            && shell::path(file).is_file()
            && !as_.is_empty())
        {
            let mut c = ctx.clone();
            c.parsed();
            return Err(c.die(cat!(
                label,
                ": a jailed agent's shot is read by its own omabox, never at a path here"
            )));
        }
        (file.to_vec(), as_)
    } else {
        match crate::repo::readlink_f(file).filter(|f| shell::path(f).is_file()) {
            Some(f) => (f.clone(), f),
            None => return Err(ctx.die(cat!(label, ": no such file: ", file))),
        }
    };
    let rec = tsv_last(&ctx.at("shots.tsv"), &key);
    if rec.is_empty() {
        return Err(ctx.die(cat!(
            label,
            ": ",
            file,
            " is not a shot of box '",
            ctx.name,
            "' (take one with omabox shot)"
        )));
    }
    // (a grid shot's record has a sixth field, "grid N M": kept apart, never part of the sizes)
    let r = shell::read_ifs(&rec, 6, b"\t");
    if stamp(&f) != r[1] {
        return Err(ctx.die(cat!(
            label,
            ": ",
            key,
            " changed since the shot: take a new one"
        )));
    }
    let screen = box_screen(ctx)?;
    let smode = match screen.iter().position(|&c| c == b' ') {
        Some(i) => screen[i + 1..].to_vec(),
        None => screen.clone(),
    };
    if smode != r[2] {
        return Err(ctx.die(cat!(
            label,
            ": the box's mode is ",
            smode,
            ", the shot was taken at ",
            r[2],
            ": take a new one"
        )));
    }
    Ok(ShotRef {
        file: f,
        key,
        stamp: r[1].clone(),
        kind: r[3].clone(),
        geo: r[4].clone(),
        grid: r[5].clone(),
    })
}

/// layout_save PATH FILE POINTER SNAPSHOT: a screen shot's desktop (desk_snap, read just before the
/// capture) and the pointer then, beside its shots.tsv line (the same stamp), for --in (#333) and
/// wait --since (#344). Snapshots are a few KB: the last 20 kept past 40. Nothing when the desktop
/// could not be read, or the path has a tab or newline (no --in for it).
pub fn layout_save(ctx: &Ctx, path: &[u8], file: &[u8], ptr: &[u8], snap: &[u8]) {
    if snap.is_empty() || path.iter().any(|&c| c == b'\t' || c == b'\n') {
        return;
    }
    let st = stamp(if file.is_empty() { path } else { file });
    let p: &[u8] = if ptr.is_empty() { b"-" } else { ptr };
    let line = cat!(path, "\t", st, "\t", p, "\t", snap, "\n");
    let f = ctx.at(LAYOUTS);
    let ok = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&f)
        .and_then(|mut o| std::io::Write::write_all(&mut o, &line));
    if ok.is_err() {
        return;
    }
    let d = std::fs::read(&f).unwrap_or_default();
    let lines: Vec<&[u8]> = d.split(|&c| c == b'\n').filter(|l| !l.is_empty()).collect();
    if lines.len() > 40 {
        let mut keep = Vec::new();
        for l in &lines[lines.len() - 20..] {
            keep.extend_from_slice(&cat!(*l, "\n"));
        }
        let new = ctx.at("layouts.tsv.new");
        if std::fs::write(&new, keep).is_ok() {
            let _ = std::fs::rename(&new, &f);
        }
    }
}

const LAYOUTS: &str = "layouts.tsv";

/// A shot's layouts.tsv line, when it has one of this stamp: the pointer then ("X Y", or "-") and
/// the desktop then.
fn layout_of(ctx: &Ctx, r: &ShotRef) -> Option<(Vec<u8>, Vec<u8>)> {
    let l = tsv_last(&ctx.at(LAYOUTS), &r.key);
    let f = shell::read_ifs(&l, 4, b"\t");
    (!l.is_empty() && f[1] == r.stamp && !f[3].is_empty()).then(|| (f[2].clone(), f[3].clone()))
}

/// What `click` would be on at X,Y (DESK_TARGET_JQ's target) in snapshot S: its kind, window,
/// layer, point in the window and the window's size (a window resized there, tiled beside a new
/// one, may have laid its content out again), and how it is said ("on foot \"A\" (its 10,20; the
/// window 800x600)").
fn at_point(s: &[u8], x: i64, y: i64) -> Option<(Vec<u8>, Vec<u8>)> {
    let t = desk_target_with(
        Target::At("click", format!("{x} {y}").as_bytes()),
        s,
        Vec::new,
    );
    if t.is_empty() {
        return None;
    }
    let id = jq::Args::new(&["-c"])
        .kv("--argjson", "s", s)
        .arg(AT_POINT_JQ)
        .run(Some(&t), jq::Err::Null);
    let text = jq::jq(&["-r", ".text"], Some(&t), jq::Err::Null);
    if !id.ok() || !text.ok() {
        return None;
    }
    let id = id.text();
    let mut said = text.text();
    // ("the click at X,Y was on ...": only what it was on; a spanning layer's may left out)
    if let Some(i) = said.windows(5).position(|w| w == b" was ") {
        said = said[i + 5..].to_vec();
    }
    if let Some(i) = said.windows(16).position(|w| w == b", or possibly on") {
        said.truncate(i);
    }
    let size = jq::jq(
        &["-r", r#".[4] // empty | "\(.[0])x\(.[1])""#],
        Some(&id),
        jq::Err::Null,
    );
    if size.ok() && !size.text().is_empty() && said.ends_with(b")") {
        said.pop();
        said = cat!(said, "; the window ", size.text(), ")");
    }
    Some((id, said))
}

/// at_point's identity of a target ($s its snapshot).
const AT_POINT_JQ: &str = r#"[.kind, .window.address, .namespace, .at,
  (.window.address as $a | if $a == null then null else first($s.clients[]? | select(.address == $a) | .size) // null end)]"#;

/// layout_moved BEFORE AFTER X Y: what changed at the screen's X,Y between two snapshots, in words,
/// when what is there is not what was (another window or layer, or the same window moved under the
/// point); None when it is the same, or either cannot be read. A change elsewhere on the screen is
/// none (#333's open question: a toast, a clock or a window opening away from the point does not
/// move what a click there gets).
pub fn layout_moved(before: &[u8], after: &[u8], x: i64, y: i64) -> Option<Vec<u8>> {
    let (bi, bs) = at_point(before, x, y)?;
    let (ai, as_) = at_point(after, x, y)?;
    if bi == ai {
        return None;
    }
    let mut m = cat!("then ", bs, ", now ", as_);
    let f = desk_facts(before, after);
    if !f.is_empty() {
        let l = jq::jq(&["-r", ".line"], Some(&f), jq::Err::Null);
        if l.ok() && !l.text().is_empty() {
            m = cat!(m, " (since the shot: ", l.text(), ")");
        }
    }
    Some(m)
}

/// layout_check LABEL SHOT X Y (#333): an action at the screen's X,Y off a screen shot is refused,
/// 1, when what is at that point is not what was there at the shot. One Hyprland read. A shot with
/// no layout recorded (an older omabox's, a --burst frame) or a desktop that cannot be read is not
/// checked.
pub fn layout_check(ctx: &Ctx, label: &str, r: &ShotRef, x: i64, y: i64) -> Res<()> {
    let Some((_, before)) = layout_of(ctx, r) else {
        return Ok(());
    };
    let now = desk_snap(ctx, false);
    if now.is_empty() {
        return Ok(());
    }
    match layout_moved(&before, &now, x, y) {
        None => Ok(()),
        Some(m) => Err(ctx.die(cat!(
            label,
            ": the screen changed at ",
            x.to_string(),
            ",",
            y.to_string(),
            " since ",
            r.key,
            ": ",
            m,
            "; take a new shot"
        ))),
    }
}

/// What a new frame of a shot's place is against the shot.
pub enum Diff {
    /// No difference; or only a caret's (the note says it).
    Same(Vec<u8>),
    /// Different there: X,Y WxH in the shot's coordinates (the screen's, or the window's own).
    At([i64; 4]),
}

/// grab LABEL SHOT: a new frame of what SHOT covers, taken as shot took it (grim -l 1, the region or
/// the window by its stableId, captured twice: finding 81), into a file of $D (removed by the
/// caller), or the window's new size when it is another.
pub fn grab(ctx: &Ctx, label: &str, r: &ShotRef) -> Res<Result<Vec<u8>, Vec<u8>>> {
    let Some([lw, lh, _, _]) = r.sizes() else {
        return Err(unread(ctx, label, r));
    };
    let mut grim: Vec<Vec<u8>> = ["timeout", "10", "grim", "-l", "1"]
        .iter()
        .map(|w| w.as_bytes().to_vec())
        .collect();
    let win = if let Some((x, y)) = r.screen_at() {
        if r.kind != b"screen 0,0" || lw.to_string().into_bytes() != whole_w(ctx)? {
            grim.extend([b"-g".to_vec(), format!("{x},{y} {lw}x{lh}").into_bytes()]);
        }
        false
    } else if let Some((addr, crop)) = r.window() {
        if !crop.is_empty() {
            return Err(ctx.die(cat!(
                label,
                ": ",
                r.key,
                " is a crop of a window (-g): it takes a whole shot of a window, or any shot of the screen"
            )));
        }
        let wins = box_windows(ctx, None)?;
        let w = jq::Args::new(&["-c"])
            .kv("--arg", "a", &addr)
            .arg(crate::jqprog::BY_ADDRESS_JQ)
            .run(Some(&wins), jq::Err::Inherit)
            .text();
        if w.is_empty() {
            return Err(ctx.die(cat!(
                label,
                ": the window of that shot (",
                addr,
                ") is gone"
            )));
        }
        let size = jq_r(crate::jqprog::WIN_SIZE_JQ, &w);
        if size != format!("{lw}x{lh}").into_bytes() {
            return Ok(Err(cat!(
                "window ",
                win_desc(&w),
                " is ",
                size,
                " now, ",
                lw.to_string(),
                "x",
                lh.to_string(),
                " in the shot"
            )));
        }
        let id = jq_r(".stableId", &w);
        if !shell::re_match("^[0-9a-fA-F]+$", &id) {
            return Err(ctx.die(cat!(
                label,
                ": window ",
                win_desc(&w),
                " has no stableId to capture it by"
            )));
        }
        grim.extend([b"-T".to_vec(), id]);
        true
    } else {
        return Err(unread(ctx, label, r));
    };
    grim.push(b"-".to_vec());
    let dir = cat!(ctx.d, "/changed");
    if !mkdir_p(&dir) {
        return Err(ctx.die(cat!(label, ": cannot make ", dir)));
    }
    let (_, tmp) = shell::mktemp_in(&cat!(dir, "/.since-XXXXXX"))?;
    let gone = shell::Rm(tmp.clone());
    for k in 0..if win { 2 } else { 1 } {
        if k == 1 {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let g = on_box(ctx, &grim, true, Out::Capture(None), ErrTo::Null);
        if g.refused {
            return Err(crate::exit::Exit(u8::try_from(g.rc).unwrap_or(1)));
        }
        if g.rc != 0 || g.out.is_empty() {
            return Err(ctx.die(cat!(
                label,
                ": could not take a frame of box '",
                ctx.name,
                "' to compare with ",
                r.key,
                " (grim: exit ",
                g.rc.to_string(),
                ")"
            )));
        }
        if std::fs::write(shell::path(&tmp), &g.out).is_err() {
            return Err(ctx.die(cat!(label, ": cannot write ", tmp)));
        }
    }
    std::mem::forget(gone);
    Ok(Ok(tmp))
}

/// The layout's width now, for "is this screen shot a whole one".
fn whole_w(ctx: &Ctx) -> Res<Vec<u8>> {
    let e = crate::screen::extent(ctx)?;
    Ok(crate::input::wh(&e).0)
}

fn unread(ctx: &Ctx, label: &str, r: &ShotRef) -> crate::exit::Exit {
    ctx.die(cat!(
        label,
        ": ",
        r.key,
        ": its record in shots.tsv is not one omabox reads"
    ))
}

/// The pointer then, of a screen shot ("X Y"), when it was recorded.
pub fn pointer_then(ctx: &Ctx, r: &ShotRef) -> Option<Vec<u8>> {
    layout_of(ctx, r)
        .map(|(p, _)| p)
        .filter(|p| p.as_slice() != b"-" && !p.is_empty())
}

/// differ LABEL SHOT NOW MASKS STRICT: the frame NOW against SHOT's image, leaving out MASKS (X,Y,W,H
/// in the shot's coordinates: its top left 0,0) and, unless STRICT, a caret alone (4 px thin, 40 long
/// at most, as shot --changed and omabox-still have it). An image of another size is refused, said.
pub fn differ(
    ctx: &Ctx,
    label: &str,
    r: &ShotRef,
    now: &[u8],
    masks: &[[i64; 4]],
    strict: bool,
) -> Res<Diff> {
    let Some([lw, lh, iw, ih]) = r.sizes() else {
        return Err(unread(ctx, label, r));
    };
    let Some((nw, nh)) = png_size(now) else {
        return Err(ctx.die(cat!(label, ": the new frame is not a PNG")));
    };
    if (nw, nh) != (iw, ih) {
        return Err(ctx.die(cat!(
            label,
            ": ",
            r.key,
            " is a ",
            iw.to_string(),
            "x",
            ih.to_string(),
            " image of ",
            lw.to_string(),
            "x",
            lh.to_string(),
            ", which is taken as ",
            nw.to_string(),
            "x",
            nh.to_string(),
            " now: a shot scaled or zoomed (--fit, shot-fit, --zoom) is not compared; take one with --fit 0"
        )));
    }
    let mut m = Vec::new();
    for &[x, y, w, h] in masks {
        if w <= 0 || h <= 0 {
            continue;
        }
        let Some(v) = to_image_px([x, y, w, h], lw, lh, iw, ih) else {
            return Err(unread(ctx, label, r));
        };
        m.push(v);
    }
    let Some(d) = diff_box(&r.file, now, &m) else {
        return Err(ctx.die(cat!(label, ": could not compare the frames (magick)")));
    };
    let Some([dx, dy, dw, dh]) = d else {
        return Ok(Diff::Same(Vec::new()));
    };
    // (back to the layout's pixels, outwards)
    let x0 = dx.saturating_mul(lw) / iw.max(1);
    let y0 = dy.saturating_mul(lh) / ih.max(1);
    let x1 = (dx.saturating_add(dw).saturating_mul(lw) + iw - 1) / iw.max(1);
    let y1 = (dy.saturating_add(dh).saturating_mul(lh) + ih - 1) / ih.max(1);
    let (w, h) = (x1 - x0, y1 - y0);
    if !strict && caret_shaped(w, h) {
        return Ok(Diff::Same(
            format!("a caret? {w}x{h} at {x0},{y0} left out").into_bytes(),
        ));
    }
    Ok(Diff::At([x0, y0, w, h]))
}

/// The four rectangles of 0,0 WxH outside R (X,Y,W,H inside it), the empty ones left out: what a
/// region's compare leaves out.
pub fn outside(r: [i64; 4], w: i64, h: i64) -> Vec<[i64; 4]> {
    let [x, y, rw, rh] = r;
    [
        [0, 0, w, y],
        [0, y + rh, w, h - (y + rh)],
        [0, y, x, rh],
        [x + rw, y, w - (x + rw), rh],
    ]
    .into_iter()
    .filter(|m| m[2] > 0 && m[3] > 0)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot as desk_snap has it: one monitor 1920x1080, windows (address, at, size) on
    /// workspace 1, the last on top and focused, LAYERS the top level's.
    fn snap(wins: &[(&str, [i64; 4])], layers: &[(&str, [i64; 4])]) -> Vec<u8> {
        let c: Vec<String> = wins
            .iter()
            .enumerate()
            .map(|(i, (a, [x, y, w, h]))| {
                format!(
                    r#"{{"address":"{a}","mapped":true,"hidden":false,"at":[{x},{y}],"size":[{w},{h}],"workspace":{{"id":1,"name":"1"}},"floating":true,"pseudo":false,"monitor":0,"class":"foot","title":"{a}","pid":{p},"xwayland":false,"pinned":false,"fullscreen":0,"fullscreenClient":0,"grouped":[],"tags":[],"swallowing":"0x0","focusHistoryID":{i},"inhibitingIdle":false,"stableId":"1"}}"#,
                    p = 100 + i
                )
            })
            .collect();
        let l: Vec<String> = layers
            .iter()
            .map(|(n, [x, y, w, h])| {
                format!(
                    r#"{{"address":"0x{n}","x":{x},"y":{y},"w":{w},"h":{h},"namespace":"{n}","pid":9}}"#
                )
            })
            .collect();
        let active = wins.last().map_or("{}".to_string(), |(a, _)| {
            format!(r#"{{"address":"{a}","class":"foot","title":"{a}"}}"#)
        });
        format!(
            r#"{{"events":0,"clients":[{}],"layers":{{"HEADLESS-2":{{"levels":{{"0":[],"1":[],"2":[{}],"3":[]}}}}}},"active":{active},"workspace":{{"id":1,"name":"1"}},"monitors":[{{"id":0,"name":"HEADLESS-2","x":0,"y":0,"width":1920,"height":1080,"scale":1,"transform":0,"activeWorkspace":{{"id":1,"name":"1"}},"specialWorkspace":{{"id":0,"name":""}},"reserved":[0,0,0,0],"focused":true}}]}}"#,
            c.join(","),
            l.join(",")
        )
        .into_bytes()
    }

    // #333: only what is at the point counts: a window over it, the window moved under it, a layer
    // over it; one opening elsewhere, or focus alone, does not.
    #[test]
    fn layout_at_the_point() {
        let a = ("0xa", [100, 100, 800, 600]);
        let before = snap(&[a], &[]);
        assert_eq!(layout_moved(&before, &before, 300, 300), None, "the same");
        let elsewhere = snap(&[a, ("0xb", [1200, 100, 400, 300])], &[]);
        assert_eq!(
            layout_moved(&before, &elsewhere, 300, 300),
            None,
            "a window opened away from the point, focused"
        );
        let over = snap(&[a, ("0xb", [200, 200, 400, 300])], &[]);
        let m = layout_moved(&before, &over, 300, 300).map(|m| String::from_utf8(m).unwrap());
        assert!(
            m.as_deref().is_some_and(|m| m.starts_with(
                r#"then on foot "0xa" (its 200,200; the window 800x600), now on foot "0xb" (its 100,100; the window 400x300)"#
            ) && m
                .contains("since the shot: window opened, focused: foot \"0xb\"")),
            "a window over the point: {m:?}"
        );
        let moved = snap(&[("0xa", [150, 100, 800, 600])], &[]);
        let m = layout_moved(&before, &moved, 300, 300).map(|m| String::from_utf8(m).unwrap());
        assert_eq!(
            m.as_deref(),
            Some(
                r#"then on foot "0xa" (its 200,200; the window 800x600), now on foot "0xa" (its 150,200; the window 800x600)"#
            ),
            "the window moved under the point"
        );
        let tiled = snap(&[("0xa", [100, 100, 400, 600])], &[]);
        let m = layout_moved(&before, &tiled, 300, 300).map(|m| String::from_utf8(m).unwrap());
        assert_eq!(
            m.as_deref(),
            Some(
                r#"then on foot "0xa" (its 200,200; the window 800x600), now on foot "0xa" (its 200,200; the window 400x600)"#
            ),
            "the window resized under the point (tiled beside a new one)"
        );
        let menu = snap(&[a], &[("t333-menu", [250, 250, 200, 200])]);
        let m = layout_moved(&before, &menu, 300, 300).map(|m| String::from_utf8(m).unwrap());
        assert!(
            m.as_deref()
                .is_some_and(|m| m.contains("now on layer t333-menu")),
            "a layer over the point: {m:?}"
        );
        assert_eq!(
            layout_moved(&before, &menu, 900, 900),
            None,
            "...not at a point it does not cover"
        );
        assert_eq!(layout_moved(b"", &before, 1, 1), None, "no snapshot: none");
    }

    #[test]
    fn region_outside() {
        assert_eq!(
            outside([10, 20, 30, 40], 100, 100),
            vec![
                [0, 0, 100, 20],
                [0, 60, 100, 40],
                [0, 20, 10, 40],
                [40, 20, 60, 40]
            ]
        );
        assert_eq!(outside([0, 0, 100, 100], 100, 100), Vec::<[i64; 4]>::new());
    }
}
