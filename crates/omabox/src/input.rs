//! Input to a box, as click, scroll, pointer, drag and keys send it: the pointer tool's moves (--steps
//! #38, --glide #176), its place on the screen, --mod's modifiers held by the keyboard tool (#25),
//! keys -s and pointer sleep's durations (#273), and the keyboard tool with the box's own keymap.

use crate::cat;
use crate::exec::{ErrTo, In, Out, on_box, pipe};
use crate::exit::{Ctx, Res, say};
use crate::hypr::hypr_json;
use crate::jq;
use crate::num::{awk_num, ms_duration};
use crate::screen::extent;
use crate::settle::{Got, Lines, cursor_now};
use crate::shell::{self, dec, strip_nl, test_int};
use std::time::Duration;

pub const BOX_KEYBOARD: &str = "/opt/omabox/bin/omabox-keyboard";
pub const BOX_POINTER: &str = "/opt/omabox/bin/omabox-pointer";
/// How long --mod's modifiers are held at most.
const MODS_MAX_MS: &str = "180000";

/// `${V%x*}` and `${V#*x}` of a "WxH".
pub fn wh(ext: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let w = ext
        .iter()
        .rposition(|&c| c == b'x')
        .map_or(ext.to_vec(), |i| ext[..i].to_vec());
    let h = ext
        .iter()
        .position(|&c| c == b'x')
        .map_or(ext.to_vec(), |i| ext[i + 1..].to_vec());
    (w, h)
}

/// `${P% *}` and `${P#* }` of an "X Y".
pub fn xy(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let x = p
        .iter()
        .rposition(|&c| c == b' ')
        .map_or(p.to_vec(), |i| p[..i].to_vec());
    let y = p
        .iter()
        .position(|&c| c == b' ')
        .map_or(p.to_vec(), |i| p[i + 1..].to_vec());
    (x, y)
}

/// on_screen CMD X Y EXTENT: X Y on the box's screen of EXTENT (WxH), or the command ends saying so
/// (finding 176): past it the pointer tool refused the number with its usage.
pub fn on_screen(ctx: &Ctx, cmd: &str, x: &[u8], y: &[u8], ext: &[u8]) -> Res<()> {
    let (w, h) = wh(ext);
    let lt = |a: &[u8], b: &[u8]| test_int(a).zip(test_int(b)).is_some_and(|(a, b)| a < b);
    if lt(x, &w) && lt(y, &h) {
        return Ok(());
    }
    Err(ctx.die(cat!(
        cmd,
        ": ",
        x,
        ",",
        y,
        " is off the ",
        ext,
        " screen of box '",
        ctx.name,
        "'"
    )))
}

/// steps_arg CMD N: --steps is 1-999 (1 is a jump).
pub fn steps_arg(ctx: &Ctx, cmd: &str, n: &[u8]) -> Res<i64> {
    if shell::re_match("^[1-9][0-9]{0,2}$", n) {
        return Ok(shell::dec(n).unwrap_or(1));
    }
    Err(ctx.die(cat!(cmd, ": --steps is 1-999, got ", n)))
}

/// glide_arg CMD DURATION: --glide (#176, finding 319), 1 ms to 10 s, as ms.
pub fn glide_arg(ctx: &Ctx, cmd: &str, v: &[u8]) -> Res<i64> {
    match ms_duration(v) {
        Some(g) if g <= 10_000 => Ok(g),
        _ => Err(ctx.die(cat!(
            cmd,
            ": --glide takes a duration up to 10s, like 400ms or 1s, got ",
            v
        ))),
    }
}

/// pause_ms "keys -s" VALUE (#273, #287): a duration as the tools' milliseconds, 0 to 10m. A bare
/// whole number was milliseconds up to 0.8.0: refused, naming the unit; one with decimals never was
/// theirs. Usage errors either way.
pub fn pause_ms(ctx: &Ctx, what: &str, v: &[u8]) -> Res<i64> {
    if shell::re_match(r"^0+(\.0+)?(ms|s|m|h)?$", v) {
        return Ok(0);
    }
    let mut c = ctx.clone();
    c.die_rc = Some(2);
    if shell::re_match("^[0-9]{1,6}$", v) {
        let n = shell::dec(v).unwrap_or(0);
        return Err(c.usage_die(cat!(
            what,
            " ",
            v,
            " is refused since 1.0 (a bare number was milliseconds there, and is seconds everywhere else): write ",
            what,
            " ",
            n.to_string(),
            "ms"
        )));
    }
    if !shell::re_match(r"^[0-9.]+$", v) {
        if let Some(ms) = ms_duration(v).filter(|&ms| ms <= 600_000) {
            return Ok(ms);
        }
    }
    Err(c.die(cat!(
        what,
        " takes a duration up to 10m, like 300ms or 1.5s, got ",
        v
    )))
}

/// `--mod`'s words: split at `+` and `,`, empty ones kept but for a last one, each taken as it is
/// (bash split an unquoted `$V` under IFS=+, and globbed each word against the current folder, so
/// `--mod '*'` there named its files: finding 439).
pub fn mod_words(v: &[u8]) -> Vec<Vec<u8>> {
    if v.is_empty() {
        return Vec::new();
    }
    let mut fields: Vec<&[u8]> = v.split(|c| matches!(c, b'+' | b',')).collect();
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    fields.into_iter().map(<[u8]>::to_vec).collect()
}

/// mods_add SO_FAR MODS (click, drag, pointer, scroll --mod; #25): modifiers as the keyboard tool
/// names them, from one or more --mod (ctrl, ctrl+shift, ctrl,shift), each once.
pub fn mods_add(ctx: &Ctx, so_far: &[u8], mods: &[u8]) -> Res<Vec<u8>> {
    let mut out = so_far.to_vec();
    for m in mod_words(mods) {
        let m = shell::lower(&m);
        let m: &[u8] = match m.as_slice() {
            b"super" | b"ctrl" | b"shift" | b"alt" | b"altgr" => &m,
            b"control" => b"ctrl",
            b"logo" | b"win" | b"mod" => b"super",
            _ => {
                return Err(ctx.die(cat!(
                    ctx.label(),
                    "--mod: '",
                    m,
                    "' is not a modifier (super, ctrl, shift, alt, altgr)"
                )));
            }
        };
        if !crate::exit::contains(&cat!("+", out, "+"), &cat!("+", m, "+")) {
            out = if out.is_empty() {
                m.to_vec()
            } else {
                cat!(out, "+", m)
            };
        }
    }
    if out.is_empty() {
        return Err(ctx.die(cat!(
            ctx.label(),
            "--mod: no modifier given (super, ctrl, shift, alt or altgr; several as ctrl+shift)"
        )));
    }
    Ok(out)
}

/// steps_to X0 Y0 X1 Y1 N: the moves there in N steps, the last at X1,Y1, appended to SEQ: the pointer
/// passes over what lies between (#38), as a hand-moved one does.
pub fn steps_to(seq: &mut Vec<Vec<u8>>, p: [i64; 4], n: i64) {
    let [x0, y0, x1, y1] = p;
    let mut k = 1;
    while k <= n {
        let x = x0.saturating_add(x1.saturating_sub(x0).saturating_mul(k).saturating_div(n));
        let y = y0.saturating_add(y1.saturating_sub(y0).saturating_mul(k).saturating_div(n));
        seq.extend([b"move".to_vec(), num(x), num(y)]);
        k += 1;
    }
}

/// glide_to X0 Y0 X1 Y1 MS: for a video, in about MS of real time, eased in and out (smoothstep), a
/// move each 40 ms, appended to SEQ (awk's arithmetic); nothing when it is there already.
pub fn glide_to(seq: &mut Vec<Vec<u8>>, p: [&[u8]; 4], ms: i64) {
    let [x0, y0, x1, y1] = p;
    if cat!(x0, " ", y0) == cat!(x1, " ", y1) {
        return;
    }
    let n = (ms.saturating_add(20) / 40).max(2);
    let (x0, y0, x1, y1) = (awk_num(x0), awk_num(y0), awk_num(x1), awk_num(y1));
    let nf = n as f64;
    for i in 1..=n {
        let u = i as f64 / nf;
        let s = u * u * (3.0 - 2.0 * u);
        let x = (x0 + (x1 - x0) * s + 0.5).trunc();
        let y = (y0 + (y1 - y0) * s + 0.5).trunc();
        seq.extend([b"move".to_vec(), awk_d(x), awk_d(y)]);
    }
}

/// gawk's %d of a value.
fn awk_d(v: f64) -> Vec<u8> {
    crate::num::fixed(v.trunc(), 0).into_bytes()
}

pub fn num(n: i64) -> Vec<u8> {
    n.to_string().into_bytes()
}

/// path_rects X Y [move X Y]...: cursor_rects along a path, at most 30 (omabox-still takes 48
/// --ignore: these, 2 for the cursor, 16 of --mask); past that, consecutive points share one.
pub fn path_rects(x: i64, y: i64, seq: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let (mut xs, mut ys) = (vec![x], vec![y]);
    let mut i = 0;
    while i + 3 <= seq.len() {
        xs.push(dec(&seq[i + 1])?);
        ys.push(dec(&seq[i + 2])?);
        i += 3;
    }
    let n = xs.len();
    let per = n.div_ceil(30);
    let mut out = Vec::new();
    let mut k = 0;
    while k < n {
        let (mut x0, mut y0, mut x1, mut y1) = (xs[k], ys[k], xs[k], ys[k]);
        let mut j = k + 1;
        while j < k + per && j < n {
            x0 = x0.min(xs[j]);
            x1 = x1.max(xs[j]);
            y0 = y0.min(ys[j]);
            y1 = y1.max(ys[j]);
            j += 1;
        }
        out.push(
            format!(
                "{},{},{},{}",
                x0.saturating_sub(16),
                y0.saturating_sub(16),
                x1.saturating_sub(x0).saturating_add(64),
                y1.saturating_sub(y0).saturating_add(64)
            )
            .into_bytes(),
        );
        k += per;
    }
    Some(out)
}

/// wheel_note DY DX (#167): what a wheel makes of them, whole notches of 15 (the nearest, at least
/// one), for each that is not whole notches already; empty otherwise. awk's.
pub fn wheel_note(dy: &[u8], dx: &[u8]) -> Vec<u8> {
    let one = |a: &str, d: &[u8]| -> Vec<u8> {
        let v = awk_num(d);
        if v == 0.0 {
            return Vec::new();
        }
        let m = if v < 0.0 { -v } else { v };
        let mut n = (m / 15.0 + 0.5).trunc();
        if n < 1.0 {
            n = 1.0;
        }
        if n * 15.0 == m {
            return Vec::new();
        }
        let es = if n > 1.0 { "es" } else { "" };
        let t = if v < 0.0 { -15.0 * n } else { 15.0 * n };
        cat!(
            a,
            " ",
            d,
            " is ",
            awk_d(n),
            " notch",
            es,
            " (",
            awk_d(t),
            ")"
        )
    };
    let (a, b) = (one("DY", dy), one("DX", dx));
    if !a.is_empty() && !b.is_empty() {
        return cat!(a, ", ", b);
    }
    cat!(a, b)
}

/// pointer_marks SEQ...: a pointer sequence as a peek window's mark: peek knows scroll, not hscroll
/// or source (it would drop the whole mark).
pub fn pointer_marks(seq: &[Vec<u8>]) -> Vec<u8> {
    let mut pm: Vec<&[u8]> = Vec::new();
    let mut i = 0;
    while i < seq.len() {
        match seq[i].as_slice() {
            b"source" => i += 1,
            b"hscroll" => pm.push(b"scroll"),
            w => pm.push(w),
        }
        i += 1;
    }
    pm.join(&b' ')
}

/// pointer_at: from where the pointer is, "X Y" inside the layout: where a move that travels starts.
pub fn pointer_at(ctx: &Ctx) -> Res<(i64, i64)> {
    let p = cursor_now(ctx)?;
    if p.is_empty() {
        return Err(ctx.die(cat!(
            "cannot tell where the pointer of box '",
            ctx.name,
            "' is (hyprctl cursorpos)"
        )));
    }
    let ext = extent(ctx)?;
    let (px, py) = xy(&p);
    let (ew, eh) = wh(&ext);
    // (a number omabox cannot read: said, 1; bash's arithmetic ended it in its own words: finding 448)
    let n = |v: &[u8]| {
        dec(v).ok_or_else(|| {
            ctx.die(cat!(
                "cannot read the pointer of box '",
                ctx.name,
                "' (",
                p,
                ") or its screen (",
                ext,
                ")"
            ))
        })
    };
    let (x, y, w, h) = (n(&px)?, n(&py)?, n(&ew)?, n(&eh)?);
    Ok((
        if x < w { x } else { w.saturating_sub(1) },
        if y < h { y } else { h.saturating_sub(1) },
    ))
}

/// `$(jq -r PROGRAM <<<"$J" | cut -d, -f1)`.
fn first_field(j: &[u8], prog: &str) -> Vec<u8> {
    let r = jq::jq(&["-r", prog], Some(j), jq::Err::Inherit);
    let lines: Vec<&[u8]> = r
        .out
        .split(|&c| c == b'\n')
        .map(|l| l.split(|&c| c == b',').next().unwrap_or_default())
        .collect();
    strip_nl(&lines.join(&b'\n'))
}

/// An input:kb_* option's value: empty when unset, which Hyprland says as `[[EMPTY]]` (a config that
/// leaves kb_variant, kb_model or kb_options alone, as `--desktop plain`'s does: #247). Passed on as
/// written, xkb could build no keymap from it and every `keys` failed. (An empty layout is `us`, below.)
const KB_STR: &str = r#".str // "" | if . == "[[EMPTY]]" then "" else . end"#;

/// kb_tool: the keyboard tool with the box's own keyboard settings, so keycodes mean to the keymap
/// what they mean to its binds (kb_options like altwin:swap_lalt_lwin move Super). A Hyprland that
/// does not answer is said in words (hypr_json).
pub fn kb_tool(ctx: &Ctx) -> Res<Vec<Vec<u8>>> {
    let j = strip_nl(&hypr_json(ctx, &["getoption", "input:kb_layout"])?);
    let layout = first_field(&j, KB_STR);
    let j = strip_nl(&hypr_json(ctx, &["getoption", "input:kb_variant"])?);
    let variant = first_field(&j, KB_STR);
    let j = strip_nl(&hypr_json(ctx, &["getoption", "input:kb_model"])?);
    let model = jq::jq(&["-r", KB_STR], Some(&j), jq::Err::Inherit).text();
    let j = strip_nl(&hypr_json(ctx, &["getoption", "input:kb_options"])?);
    let options = jq::jq(&["-r", KB_STR], Some(&j), jq::Err::Inherit).text();
    Ok(vec![
        BOX_KEYBOARD.as_bytes().to_vec(),
        b"--layout".to_vec(),
        if layout.is_empty() {
            b"us".to_vec()
        } else {
            layout
        },
        b"--variant".to_vec(),
        variant,
        b"--model".to_vec(),
        model,
        b"--options".to_vec(),
        options,
    ])
}

/// `read -r -t SECS -u FD line`: a whole line in time, its blanks (IFS's) at either end gone.
pub fn read_line(lines: &mut Lines, t: Duration) -> Option<Vec<u8>> {
    match lines.read(t) {
        Got::Line(l) => Some(
            shell::read_fields(&l, 1)
                .into_iter()
                .next()
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

/// with_mods MODS ACT: MODS held down on the box's keyboard while ACT runs (#25): the keyboard tool
/// presses them, says "paused", and lets go when its stdin closes (after ACT, or when this omabox
/// ends), on a signal, or after MODS_MAX_MS. A tool that did not end well may have left them down:
/// a new keyboard's first event sets the seat's modifiers again, to none (finding 112). ACT's status,
/// or 1 when the modifiers were not held.
pub fn with_mods(ctx: &mut Ctx, mods: &[u8], act: impl FnOnce(&mut Ctx) -> Res<i32>) -> Res<i32> {
    if mods.is_empty() {
        return act(ctx);
    }
    let kb = kb_tool(ctx)?;
    // (bash's fifos were in a `mktemp -d` dir, removed at once: one that cannot be made ends the
    // command there, mktemp's words said; pipes here, the dir made and removed all the same)
    let (_, dir) = shell::mktemp_args(&[b"-d"])?;
    let _ = std::fs::remove_dir(shell::path(&dir));
    let ((in_r, in_w), (out_r, out_w)) = match (pipe(), pipe()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return Err(ctx.die("could not make the pipes for the keyboard tool")),
    };
    let mut argv = kb.clone();
    argv.extend([b"-m".to_vec(), mods.to_vec(), b"-p".to_vec()]);
    argv.push(MODS_MAX_MS.as_bytes().to_vec());
    let c = ctx.clone();
    // (bash's `&`: SIGINT and SIGQUIT ignored in it, so a Ctrl-C or Ctrl-\ leaves it to let go)
    let tool = std::thread::spawn(move || {
        crate::exec::on_box_bg(
            &c,
            &argv,
            In::Fd(in_r),
            Out::Fd(out_w),
            ErrTo::Inherit,
            None,
        )
        .rc
    });
    let mut lines = Lines::new(out_r);
    let mut rc = if read_line(&mut lines, Duration::from_secs(10)).is_some_and(|l| l == b"paused") {
        // (A refusal ends the command, the tool's stdin closed with it: it lets go.)
        act(ctx)?
    } else {
        say(cat!("could not hold ", mods, " down: nothing was sent"));
        1
    };
    // Closing its stdin is the tool's "let go".
    drop(in_w);
    drop(lines);
    let krc = tool.join().unwrap_or(1);
    if krc != 0 {
        let mut a = kb;
        a.extend([b"-s".to_vec(), b"0".to_vec()]);
        let _ = on_box(ctx, &a, false, Out::Null, ErrTo::Null);
        if rc == 0 {
            rc = 1;
            say(cat!(
                "the keyboard let go of ",
                mods,
                " before the end: see above"
            ));
        }
    }
    Ok(rc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|a| a.as_bytes().to_vec()).collect()
    }

    #[test]
    fn moves_as_bash_and_awk_make_them() {
        let mut s = Vec::new();
        steps_to(&mut s, [0, 0, 10, -7], 3);
        assert_eq!(
            s,
            w(&["move", "3", "-2", "move", "6", "-4", "move", "10", "-7"])
        );
        let mut g = Vec::new();
        glide_to(&mut g, [b"0", b"0", b"100", b"50"], 100);
        assert_eq!(
            g,
            w(&["move", "26", "13", "move", "74", "37", "move", "100", "50"])
        );
        let mut none = Vec::new();
        glide_to(&mut none, [b"5", b"5", b"5", b"5"], 400);
        assert!(none.is_empty());
        // (t_unit_record's: 400 ms is 10 moves, eased, ending there)
        let mut g = Vec::new();
        glide_to(&mut g, [b"0", b"0", b"100", b"0"], 400);
        let xs: Vec<&[u8]> = g.chunks(3).map(|m| m[1].as_slice()).collect();
        assert_eq!(
            xs,
            [
                &b"3"[..],
                b"10",
                b"22",
                b"35",
                b"50",
                b"65",
                b"78",
                b"90",
                b"97",
                b"100"
            ]
        );
        assert!(g.chunks(3).all(|m| m[0] == b"move" && m[2] == b"0"));
        assert_eq!(
            path_rects(10, 20, &w(&["move", "30", "40"])),
            Some(w(&["-6,4,64,64", "14,24,64,64"]))
        );
    }

    /// t_unit_pointer's cases (#167, drag --wait), as bash's helpers gave them.
    #[test]
    fn pointer_cases_as_bash() {
        assert_eq!(
            pointer_marks(&w(&[
                "move", "1", "2", "source", "wheel", "scroll", "15", "hscroll", "-3"
            ])),
            b"move 1 2 scroll 15 scroll -3"
        );
        assert_eq!(wheel_note(b"20", b"0"), b"DY 20 is 1 notch (15)");
        assert_eq!(wheel_note(b"45", b"-30"), b"");
        assert_eq!(
            wheel_note(b"5", b"-37"),
            b"DY 5 is 1 notch (15), DX -37 is 2 notches (-30)"
        );
        assert_eq!(wheel_note(b"0", b"7.5"), b"DX 7.5 is 1 notch (15)");
        let mut s = Vec::new();
        steps_to(&mut s, [0, 0, 10, 21], 3);
        assert_eq!(
            s,
            w(&["move", "3", "7", "move", "6", "14", "move", "10", "21"])
        );
        let mut s = Vec::new();
        steps_to(&mut s, [10, 10, 0, 0], 2);
        assert_eq!(s, w(&["move", "5", "5", "move", "0", "0"]));
        let mut s = Vec::new();
        steps_to(&mut s, [100, 100, 0, 0], 2);
        assert_eq!(
            path_rects(100, 100, &s),
            Some(w(&["84,84,64,64", "34,34,64,64", "-16,-16,64,64"]))
        );
        // 999 steps: 30 rects (omabox-still takes 32), from the start to the end.
        let mut s = Vec::new();
        steps_to(&mut s, [0, 0, 1000, 1000], 999);
        let r = path_rects(0, 0, &s).unwrap();
        assert_eq!(r.len(), 30);
        let xywh = |v: &[u8]| -> Vec<i64> {
            v.split(|&c| c == b',')
                .map(|n| std::str::from_utf8(n).unwrap().parse().unwrap())
                .collect()
        };
        let (a, e) = (xywh(&r[0]), xywh(&r[29]));
        assert_eq!((a[0], a[1]), (-16, -16));
        assert_eq!((e[0] + e[2] - 64, e[1] + e[3] - 64), (984, 984));
    }

    #[test]
    fn words_of_scroll_and_pointer() {
        assert_eq!(wheel_note(b"20", b"0"), b"DY 20 is 1 notch (15)");
        assert_eq!(
            wheel_note(b"-40", b"7.5"),
            b"DY -40 is 3 notches (-45), DX 7.5 is 1 notch (15)"
        );
        assert_eq!(wheel_note(b"30", b"-15"), b"");
        assert_eq!(
            pointer_marks(&w(&["move", "1", "2", "source", "wheel", "hscroll", "5"])),
            b"move 1 2 scroll 5"
        );
        assert_eq!(mod_words(b"+a++b+"), w(&["", "a", "", "b"]));
        assert_eq!(mod_words(b",+"), w(&["", ""]));
        assert_eq!(mod_words(b"ctrl+*,.?"), w(&["ctrl", "*", ".?"]));
        assert!(mod_words(b"").is_empty());
        assert_eq!(xy(b"3 4"), (b"3".to_vec(), b"4".to_vec()));
        assert_eq!(wh(b"1920x1080"), (b"1920".to_vec(), b"1080".to_vec()));
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use crate::exit::Exit;

    #[test]
    fn options_as_bash_reads_them() {
        let c = Ctx::new(b"click", b"/x".to_vec());
        assert_eq!(
            mods_add(&c, b"", b"Control,WIN"),
            Ok(b"ctrl+super".to_vec())
        );
        assert_eq!(mods_add(&c, b"ctrl", b"ctrl+alt"), Ok(b"ctrl+alt".to_vec()));
        // (t_unit_pointer's: names, any case, each once, in order given; added to earlier ones)
        assert_eq!(
            mods_add(&c, b"", b"Control,shift+CTRL+win"),
            Ok(b"ctrl+shift+super".to_vec())
        );
        assert_eq!(mods_add(&c, b"alt", b"ctrl+alt"), Ok(b"alt+ctrl".to_vec()));
        assert_eq!(mods_add(&c, b"", b"+"), Err(Exit(2)));
        assert_eq!(mods_add(&c, b"", b"nope"), Err(Exit(2)));
        assert_eq!(steps_arg(&c, "click", b"999"), Ok(999));
        assert_eq!(steps_arg(&c, "click", b"010"), Err(Exit(2)));
        assert_eq!(glide_arg(&c, "click", b"1.5s"), Ok(1500));
        assert_eq!(glide_arg(&c, "click", b"11s"), Err(Exit(2)));
        assert_eq!(pause_ms(&c, "keys -s", b"0.0ms"), Ok(0));
        assert_eq!(pause_ms(&c, "keys -s", b"50ms"), Ok(50));
        assert_eq!(pause_ms(&c, "keys -s", b"50"), Err(Exit(2)));
        assert_eq!(pause_ms(&c, "keys -s", b"1.5"), Err(Exit(2)));
        assert_eq!(pause_ms(&c, "keys -s", b"11m"), Err(Exit(2)));
    }
}
