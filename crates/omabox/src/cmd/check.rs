//! `omabox check run | point | diff | wants` (#252, findings 357-363): visual checks. A scenario's
//! checkpoints (shots, and with --window the window's tree) against a baseline, compared exactly in
//! the rendering users see, specks (parts under a checkpoint's speck) left out; holes (see-through
//! parts inside a surface) found on a clean box's sentinel wallpaper. Every refusal and setup error
//! exits 2 (#281): 1 is "changed"; 3 a scenario that failed, 4 no baseline. ImageMagick and
//! share/check.py do the pixels and the layout; this side runs them as bash did, word for word.

use crate::args::{optarg, unknown_opt, unknown_word};
use crate::boxes::need_box;
use crate::cat;
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jq::{self, Args};
use crate::select::select_box;
use crate::settle::{geom_parse, geom_text};
use crate::shell::{self, dec, strip_nl};
use crate::shots::png_arg;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

const CHECK_SPECK: i64 = 32;
const CHECK_GAP: &str = "12";
const CHECK_SPECK_MIN: i64 = 16;
const CHECK_SPECK_AGREED: i64 = 8;
const CHECK_CORNER: &str = "32";
const CHECK_ID_RE: &str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$";
const CLEAN_WALLPAPER: &str = "#ff00ff";
const WAIT_MAX_MS: i64 = 600_000;

/// check --json's helpers (#273, audit J1), bin/omabox's CHK_JQ byte for byte.
pub const CHK_JQ: &str = r#"def regions: split("\n") | map(select(. != "") | capture("^(?<x>-?[0-9]+),(?<y>-?[0-9]+) (?<w>[0-9]+)x(?<h>[0-9]+) (?<p>[0-9]+)")
    | {x: (.x | tonumber), y: (.y | tonumber), w: (.w | tonumber), h: (.h | tonumber), pixels: (.p | tonumber)});
  def specks: if . == "" then null else split(" ") | map(tonumber) | {count: .[0], pixels: .[1], largest: (.[2] // null)} end;
  def size: if . == "" then null else split(" -> ") | {from: .[0], to: .[1]} end;
  def path: if . == "" then null else . end;"#;
/// check diff --json's object.
pub const CHK_DIFF_JQ: &str = r#"
    {result: $res, size: ($size | size), regions: ($regs | regions), specks: ($sp | specks), marked: ($marked | path),
     report: ($rep | path), report_scaled: ($rs | path)}"#;
/// check run --json's object.
pub const CHK_RUN_JQ: &str = r#"
    ($sum | split("\t")) as $s
    | {result: (if $rc == 0 then "same" else "changed" end), base: $s[0], shots: $s[1], report: ($s[2] | path),
       setup: ($s[3] | path), report_scaled: ($s[12] | path),
       only: (if $s[13] == "" then null else $s[13] | split(",") end),
       counts: {checkpoints: ($s[4] | tonumber), same: ($s[5] | tonumber), changed: ($s[6] | tonumber), as_another: ($s[7] | tonumber),
         holes: ($s[8] | tonumber), new: ($s[9] | tonumber), missing: ($s[10] | tonumber), skipped: ($s[11] | tonumber)},
       checkpoints: .}"#;
/// One checkpoint of check run --json.
pub const CHK_POINT_JQ: &str = r#"
    {id: $id, result: $res, as: ($as | path), size: ($size | size), regions: ($regs | regions),
     holes: ($holes | regions),
     tree: ($tree | split("\n") | map(select(. != ""))), specks: ($sp | specks), marked: ($marked | path),
     approved_holes: ($ok | tonumber), still: (if $still == "" then null else $still == "true" end)}"#;
/// The box of a run, for the baseline's check.json.
pub const CHK_ENV_JQ: &str = r#"first(.[] | select(.name == $n)) | {omabox: $v, hyprland: .hyprland_version, omarchy: .omarchy_version, theme,
      size, gpu: (.render.driver // null), locale: (.clean.locale // null), timezone: (.clean.timezone // null), clean}"#;
/// A baseline from another setup.
pub const CHK_SETUP_JQ: &str = r#"($a[0] // {}) as $x | ($b[0] // {}) as $y | [("omabox", "hyprland", "omarchy", "gpu", "theme", "size", "locale", "timezone")
        | select($x[.] != $y[.] and ((. != "locale" and . != "timezone") or $x[.] != null))
        | "\(.) \($x[.] // "?") -> \($y[.] // "?")"] | join(", ")"#;

// --- tools ------------------------------------------------------------------------------------------

fn w<A: AsRef<[u8]>>(v: &[A]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_ref().to_vec()).collect()
}

/// CHECK_PY: the checkout's share/check.py (a #[test]'s binary has no checkout: the source tree's).
fn check_py() -> Vec<u8> {
    #[cfg(test)]
    return concat!(env!("CARGO_MANIFEST_DIR"), "/../../share/check.py")
        .as_bytes()
        .to_vec();
    #[cfg(not(test))]
    shell::bytes(&crate::root().join("share/check.py")).to_vec()
}

/// `magick ARGS... 2>/dev/null`: its status and stdout. Every file in ARGS goes as `png_arg`'s.
fn magick(args: &[Vec<u8>]) -> jq::Ran {
    jq::tool("magick", args, None, jq::Err::Null)
}

/// `$(magick identify -format FMT FILE 2>/dev/null)`.
fn identify(fmt: &str, f: &[u8]) -> Option<Vec<u8>> {
    let r = magick(&w(&[
        b"identify".as_slice(),
        b"-format",
        fmt.as_bytes(),
        &png_arg(f),
    ]));
    r.ok().then(|| r.text())
}

/// `/usr/bin/python3 -I $CHECK_PY ARGS...`, INPUT its stdin as given (None: the caller's), its
/// stderr the caller's (or none).
fn py(args: &[Vec<u8>], input: Option<&[u8]>, err_null: bool) -> jq::Ran {
    let mut a = vec![b"-I".to_vec(), check_py()];
    a.extend(args.iter().cloned());
    let mut c = crate::sig::command("/usr/bin/python3");
    c.args(a.iter().map(|x| OsStr::from_bytes(x)));
    c.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::inherit()
    });
    c.stdout(Stdio::piped());
    c.stderr(if err_null {
        Stdio::null()
    } else {
        Stdio::inherit()
    });
    // (a foreground child, sig::run_child: fed, read, its end recorded)
    let ran = crate::sig::run_child(c, false, |child| {
        let feeder = match (input, child.stdin.take()) {
            (Some(i), Some(si)) => Some(crate::exec::feed(si.into(), i.to_vec())),
            _ => None,
        };
        let out = child
            .stdout
            .take()
            .map(|s| crate::exec::read_all(s.into(), None))
            .unwrap_or_default();
        if let Some(f) = feeder {
            let _ = f.join();
        }
        out
    });
    match ran {
        Ok((out, (rc, _))) => jq::Ran { rc, out },
        Err(e) => {
            let (rc, said) = py_not_run(&e);
            if !err_null {
                let _ = std::io::stderr().write_all(&said);
            }
            jq::Ran {
                rc,
                out: Vec::new(),
            }
        }
    }
}

/// Why /usr/bin/python3 did not start, in omabox's words, and the shell's code for it: 127 not there,
/// 126 there but not runnable (finding 441's rule; bash said "/usr/bin/python3: No such file or
/// directory" whatever the cause).
fn py_not_run(e: &std::io::Error) -> (i32, Vec<u8>) {
    let code = e.raw_os_error().unwrap_or(libc::EIO);
    // SAFETY: strerror's string for a valid errno, read at once.
    let why = unsafe {
        std::ffi::CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .into_owned()
    };
    let rc = if code == libc::ENOENT { 127 } else { 126 };
    (
        rc,
        cat!("omabox: check: cannot run /usr/bin/python3: ", why, "\n"),
    )
}

/// `<<<"$V"`: V and a newline.
fn here(v: &[u8]) -> Vec<u8> {
    cat!(v, "\n")
}

/// `$(grep -v '^specks ' <<<"$RES" || true)` and `$(sed -n 's/^specks //p' <<<"$RES")`.
fn split_specks(res: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut regs, mut sp): (Vec<&[u8]>, Vec<&[u8]>) = (Vec::new(), Vec::new());
    for l in res.split(|&c| c == b'\n') {
        match l.strip_prefix(b"specks ") {
            Some(s) => sp.push(s),
            None => regs.push(l),
        }
    }
    (strip_nl(&regs.join(&b'\n')), strip_nl(&sp.join(&b'\n')))
}

/// `${V% *}`: V less its last space and what follows (V when it has none).
fn pct_sp(v: &[u8]) -> &[u8] {
    v.iter().rposition(|&c| c == b' ').map_or(v, |i| &v[..i])
}

/// `${V##* }`: what follows V's last space (V when it has none).
fn hash2_sp(v: &[u8]) -> &[u8] {
    v.iter()
        .rposition(|&c| c == b' ')
        .map_or(v, |i| &v[i + 1..])
}

/// `${V#* }`: what follows V's first space (V when it has none).
fn hash_sp(v: &[u8]) -> &[u8] {
    v.iter().position(|&c| c == b' ').map_or(v, |i| &v[i + 1..])
}

/// Lines of V as `while IFS= read -r l; ... done <<<"$V"` reads them (one empty line for an empty V).
fn lines(v: &[u8]) -> Vec<&[u8]> {
    v.split(|&c| c == b'\n').collect()
}

/// A file's lines as `while read ... done < FILE` reads them: a last line without its newline too;
/// none when the file cannot be read.
fn file_lines(p: &[u8]) -> Vec<Vec<u8>> {
    let Ok(d) = std::fs::read(shell::path(p)) else {
        return Vec::new();
    };
    let mut v: Vec<Vec<u8>> = d.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
    if v.last().is_some_and(Vec::is_empty) {
        v.pop();
    }
    v
}

/// `grep -q RE FILE` for an RE that is a fixed start of a line.
fn has_line_starting(p: &[u8], lead: &[u8]) -> bool {
    file_lines(p).iter().any(|l| l.starts_with(lead))
}

/// `$(grep -c '^LEAD' FILE || true)`.
fn count_starting(p: &[u8], lead: &[u8]) -> usize {
    file_lines(p).iter().filter(|l| l.starts_with(lead)).count()
}

/// The sum of WORDS (decimal, 0 for an empty one); None, said, at the first that is no number
/// (finding 448: bash's arithmetic ended there in its own words, at an 08).
fn arith(words: &[&[u8]]) -> Option<i64> {
    let mut t = 0i64;
    for v in words {
        if v.is_empty() {
            continue;
        }
        match dec(v) {
            Some(n) => t = t.saturating_add(n),
            None => {
                crate::exit::say(cat!("check: cannot read '", v, "' as a number"));
                return None;
            }
        }
    }
    Some(t)
}

/// A word as a number, 0 for one that is none (the callers' numbers are checked before).
fn ai(v: &[u8]) -> i64 {
    dec(v).unwrap_or(0)
}

/// A region line's "X,Y WxH", as its four numbers (`^(-?[0-9]+),(-?[0-9]+)\ ([0-9]+)x([0-9]+)`).
fn region_of(r: &[u8]) -> Option<[Vec<u8>; 4]> {
    let re = shell::Re::new(r"^(-?[0-9]+),(-?[0-9]+) ([0-9]+)x([0-9]+)")?;
    let m = re.captures(r, false)?;
    let g = |i: usize| {
        m.get(i)
            .copied()
            .flatten()
            .map(|(a, b)| r[a..b].to_vec())
            .unwrap_or_default()
    };
    Some([g(1), g(2), g(3), g(4)])
}

/// check_mask_of: a region line's "X Y W H", for a mask.
fn mask_of(r: &[u8]) -> Option<Vec<u8>> {
    region_of(r).map(|[x, y, w, h]| cat!(x, " ", y, " ", w, " ", h))
}

/// `${M// /$'\t'}`.
fn tabs(m: &[u8]) -> Vec<u8> {
    m.iter()
        .map(|&c| if c == b' ' { b'\t' } else { c })
        .collect()
}

/// `printf '%s\n' LINE >> FILE` (its failure bash's redirection line, unsaid here).
fn append(p: &[u8], data: &[u8]) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(shell::path(p))
    {
        let _ = f.write_all(data);
    }
}

fn rm(p: &[u8]) {
    let _ = std::fs::remove_file(shell::path(p));
}

/// A step that failed: WHY said, and the command ended, 2 (check's for its own failures; bash's set
/// -e ended it with the step's status, mostly 1, check's "changed": finding 442).
fn failed(ctx: &Ctx, why: Vec<u8>) -> Exit {
    say(cat!("check: ", why));
    ctx.refused()
}

/// `mkdir -p DIR`, or why not said and the command ended (failed).
fn mkdirs(ctx: &Ctx, d: &[u8]) -> Res<()> {
    std::fs::create_dir_all(shell::path(d))
        .map_err(|e| failed(ctx, cat!("cannot make ", d, ": ", crate::shots::why(&e))))
}

/// `cp -- SRC TO`, TO a file's path, or why not said and the command ended (failed).
fn copy(ctx: &Ctx, src: &[u8], to: &[u8]) -> Res<()> {
    std::fs::copy(shell::path(src), shell::path(to))
        .map(|_| ())
        .map_err(|e| {
            failed(
                ctx,
                cat!("cannot copy ", src, " to ", to, ": ", crate::shots::why(&e)),
            )
        })
}

/// `: > FILE` / `CMD > FILE`: the file written, or its failure said and the command ended, 2 (check's
/// for its own failures; bash's set -e ended it 1, check's "changed", with its redirection line
/// alone: findings 441, 442).
fn put(p: &[u8], data: &[u8]) -> Res<()> {
    std::fs::write(shell::path(p), data).map_err(|e| {
        say(cat!("check: ", crate::shots::cant_write(p, &e)));
        Exit(2)
    })
}

fn run_tool(name: &str, args: &[Vec<u8>]) -> i32 {
    let mut c = crate::sig::command(name);
    c.args(args.iter().map(|a| OsStr::from_bytes(a)));
    crate::sig::run_status(c).unwrap_or(127)
}

/// `cut -d' ' -f1,2 <<<"$REGIONS"`: each line's first two fields.
fn cut12(v: &[u8]) -> Vec<Vec<u8>> {
    lines(v)
        .into_iter()
        .map(|l| {
            if !l.contains(&b' ') {
                return l.to_vec();
            }
            let mut f = l.splitn(3, |&c| c == b' ');
            let a = f.next().unwrap_or_default();
            let b = f.next().unwrap_or_default();
            cat!(a, " ", b)
        })
        .collect()
}

/// `sed -n 's/^tree //p' FILE > OUT`.
fn tree_lines_to(src: &[u8], dst: &[u8]) {
    let mut o = Vec::new();
    for l in file_lines(src) {
        if let Some(t) = l.strip_prefix(b"tree ") {
            o.extend_from_slice(t);
            o.push(b'\n');
        }
    }
    let _ = std::fs::write(shell::path(dst), o);
}

// --- the comparison ------------------------------------------------------------------------------------

/// check_regions A B SPECK [MASK]...: where two shots of one size differ, "X,Y WxH PIXELS" a region,
/// then "specks N PIXELS LARGEST"; each MASK ("X Y W H") painted black in both first. None when
/// magick or check.py fails (never an empty answer read as "same").
fn check_regions(fa: &[u8], fb: &[u8], speck: &[u8], masks: &[Vec<u8>]) -> Option<Vec<u8>> {
    check_regions_in(fa, fb, speck, masks)
}

fn check_regions_in(fa: &[u8], fb: &[u8], speck: &[u8], masks: &[Vec<u8>]) -> Option<Vec<u8>> {
    let mut draw: Vec<Vec<u8>> = Vec::new();
    for m in masks {
        let f = shell::read_fields(m, 4);
        // (`$((x + w - 1))`; the first point is the words as they are)
        let x2 = arith(&[&f[0], &f[2]])?;
        let y2 = arith(&[&f[1], &f[3]])?;
        draw.push(b"-draw".to_vec());
        draw.push(cat!(
            "rectangle ",
            f[0],
            ",",
            f[1],
            " ",
            (x2 - 1).to_string(),
            ",",
            (y2 - 1).to_string()
        ));
    }
    let (pa, pb) = (png_arg(fa), png_arg(fb));
    let mut a = w(&[pa.as_slice(), &pb, b"-alpha", b"off", b"-fill", b"black"]);
    a.extend(draw);
    a.extend(w(&[
        "-compose",
        "difference",
        "-composite",
        "-alpha",
        "off",
        "-grayscale",
        "Brightness",
        "-threshold",
        "0",
        "-define",
        "connected-components:verbose=true",
        "-connected-components",
        "8",
        "null:",
    ]));
    let cc = magick(&a);
    if !cc.ok() {
        return None;
    }
    let r = py(
        &w(&[b"regions".as_slice(), speck, CHECK_GAP.as_bytes()]),
        Some(&here(&cc.text())),
        false,
    );
    r.ok().then(|| r.text())
}

/// check_mark NEW OUT REGION...: the regions drawn on the new shot.
fn check_mark(new: &[u8], outp: &[u8], regions: &[Vec<u8>]) -> bool {
    let mut a = w(&[
        png_arg(new).as_slice(),
        b"-fill",
        b"none",
        b"-stroke",
        b"#ff2020",
        b"-strokewidth",
        b"2",
    ]);
    for r in regions {
        let Some([x, y, ww, h]) = region_of(r) else {
            continue;
        };
        let (x, y, ww, h) = (ai(&x), ai(&y), ai(&ww), ai(&h));
        a.push(b"-draw".to_vec());
        a.push(format!("rectangle {},{} {},{}", x - 2, y - 2, x + ww + 1, y + h + 1).into_bytes());
    }
    a.push(png_arg(outp));
    magick(&a).ok()
}

/// check_report PLAN OUT.png [KIND]: the report of a plan (check.py's layout, magick's drawing);
/// what was left out of it ("" when nothing). None when check.py or magick fails.
fn check_report(plan: &[u8], png: &[u8], kind: &str) -> Option<Vec<u8>> {
    check_report_in(plan, png, kind, 1)
}

/// The people's copy of a report (#402): the same plan laid out as check_report's, drawn N times its
/// size, to PNG@Nx.png; its path and what check_report gives (None when N is under 2 or it failed).
fn scaled_report(plan: &[u8], png: &[u8], kind: &str, n: u32) -> Option<(Vec<u8>, Vec<u8>)> {
    if n < 2 {
        return None;
    }
    let p = scaled_path(png, n);
    check_report_in(plan, &p, kind, n).map(|left| (p, left))
}

fn check_report_in(plan: &[u8], png: &[u8], kind: &str, scale: u32) -> Option<Vec<u8>> {
    let font = jq::tool(
        "fc-match",
        &["-f", "%{file}", "monospace"],
        None,
        jq::Err::Null,
    );
    let mut fontp = if font.ok() { font.text() } else { Vec::new() };
    if fontp.is_empty() || !shell::path(&fontp).is_file() {
        fontp.clear();
    }
    let part = cat!(png, ".part");
    let argsf = cat!(plan, ".args");
    let mut pa = w(&[kind.as_bytes(), plan, &part, &fontp]);
    if scale > 1 {
        pa.push(scale.to_string().into_bytes());
    }
    let r = py(&pa, None, false);
    // (`> "$plan.args"`: written whatever the status)
    let _ = std::fs::write(shell::path(&argsf), &r.out);
    if !r.ok() {
        rm(&argsf);
        return None;
    }
    let data = std::fs::read(shell::path(&argsf)).unwrap_or_default();
    rm(&argsf);
    let mut a: Vec<Vec<u8>> = data.split(|&c| c == 0).map(<[u8]>::to_vec).collect();
    if a.last().is_some_and(Vec::is_empty) {
        a.pop();
    }
    if a.len() <= 1 {
        return None;
    }
    if !magick(&a[1..]).ok() {
        rm(&part);
        return None;
    }
    // (A report that could not be put in place is none: the caller says so, its part gone. Bash's
    // mv failed unseen, the caller's `if` holding set -e off: finding 442.)
    if !crate::shots::mv_f(&part, png) {
        rm(&part);
        return None;
    }
    Some(a[0].clone())
}

/// check_holes SHOT KIND [REGION]: a shot's holes, "X,Y WxH PIXELS" a hole, then "corners N".
fn check_holes(f: &[u8], kind: &str, within: &[u8]) -> Option<Vec<u8>> {
    check_holes_in(f, kind, within)
}

fn check_holes_in(f: &[u8], kind: &str, within: &[u8]) -> Option<Vec<u8>> {
    let sz = identify("%w %h", f)?;
    let mask: Vec<Vec<u8>> = if kind == "window" {
        w(&["-alpha", "extract", "-threshold", "0", "-negate"])
    } else {
        w(&[
            "-alpha",
            "off",
            "-fill",
            "black",
            "+opaque",
            CLEAN_WALLPAPER,
            "-fill",
            "white",
            "-opaque",
            CLEAN_WALLPAPER,
        ])
    };
    let mut a = vec![png_arg(f)];
    a.extend(mask);
    a.extend(w(&[
        "-define",
        "connected-components:verbose=true",
        "-connected-components",
        "8",
        "null:",
    ]));
    let cc = magick(&a);
    if !cc.ok() {
        return None;
    }
    let mut pa = w(&["holes", kind]);
    pa.extend(shell::words(&sz));
    pa.extend(w(&[CHECK_CORNER, CHECK_GAP]));
    pa.extend(shell::words(within));
    let r = py(&pa, Some(&here(&cc.text())), false);
    r.ok().then(|| r.text())
}

/// check_holes_off: why a box's screen cannot show holes, or nothing when it can.
fn check_holes_off(ctx: &Ctx) -> Vec<u8> {
    check_holes_off_in(ctx)
}

fn check_holes_off_in(ctx: &Ctx) -> Vec<u8> {
    let bj = cat!(ctx.d, "/box.json");
    let clean = jq::tool(
        "jq",
        &[b"-r".as_slice(), b".clean != null", &bj],
        None,
        jq::Err::Null,
    );
    if clean.text() != b"true" {
        return b"not a clean box".to_vec();
    }
    if jq::tool(
        "jq",
        &[
            b"-e".as_slice(),
            br#".clean.keep | index("wallpaper")"#,
            &bj,
        ],
        None,
        jq::Err::Null,
    )
    .ok()
    {
        return b"--keep wallpaper".to_vec();
    }
    let bg = std::fs::read_link(ctx.at("home/.local/state/omarchy/current/background"))
        .map(|p| shell::bytes(&p).to_vec())
        .unwrap_or_default();
    if bg == b"/home/sbx/.config/omabox/clean-wallpaper.png" {
        Vec::new()
    } else {
        b"the wallpaper is not the sentinel (a theme set in the box?)".to_vec()
    }
}

/// check_hole_what POINT: what a hole is, said of a checkpoint.
fn hole_what(point: &[u8]) -> &'static str {
    if has_line_starting(point, b"of window ") {
        "the window is transparent"
    } else {
        "the wallpaper shows through"
    }
}

/// `while read -r A B; do ...; done < FILE`: each line's first word and the rest.
fn word_rest(p: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    file_lines(p)
        .iter()
        .map(|l| {
            let f = shell::read_fields(l, 2);
            (f[0].clone(), f[1].clone())
        })
        .collect()
}

/// check_new_holes BASE NEW: the holes of NEW's point that BASE's has none overlapping.
fn check_new_holes(base: &[u8], new: &[u8]) -> Vec<Vec<u8>> {
    let old: Vec<Vec<u8>> = word_rest(base)
        .into_iter()
        .filter(|(l, _)| l == b"hole")
        .map(|(_, b)| b)
        .collect();
    let mut outv = Vec::new();
    'new: for (l, nh) in word_rest(new) {
        if l != b"hole" {
            continue;
        }
        let Some([nx, ny, nw, nh_]) = region_of(&nh) else {
            continue;
        };
        let (nx, ny, nw, nhh) = (ai(&nx), ai(&ny), ai(&nw), ai(&nh_));
        // (`for b in ${old[@]+"${old[@]}"}`: each baseline hole whole)
        for b in &old {
            let Some([bx, by, bw, bh]) = region_of(b) else {
                continue;
            };
            let (bx, by, bw, bh) = (ai(&bx), ai(&by), ai(&bw), ai(&bh));
            if nx < bx + bw && bx < nx + nw && ny < by + bh && by < ny + nhh {
                continue 'new;
            }
        }
        outv.push(nh);
    }
    outv
}

/// check_ids DIR: the IDs a run took, in the order taken (the .point files' times).
fn check_ids(dir: &[u8]) -> Vec<Vec<u8>> {
    check_ids_in(dir)
}

fn check_ids_in(dir: &[u8]) -> Vec<Vec<u8>> {
    if !shell::path(dir).is_dir() {
        return Vec::new();
    }
    let mut c = crate::sig::command("find");
    c.arg(OsStr::from_bytes(dir))
        .args(["-maxdepth", "1", "-name", "*.point", "-printf", "%T@ %f\n"])
        .stdin(Stdio::inherit())
        .stderr(Stdio::null());
    let find = crate::sig::run_output(c)
        .map(|(_, o)| o)
        .unwrap_or_default();
    let sorted = jq::tool("sort", &["-n"], Some(&find), jq::Err::Inherit).out;
    let sed = jq::tool(
        "sed",
        &[r"s/^[^ ]* //; s/\.point$//"],
        Some(&sorted),
        jq::Err::Inherit,
    );
    // (`for id in $(check_ids ...)`: split at blanks)
    shell::words(&sed.out)
}

/// What check_one sets: CHK_REGIONS, CHK_SPECKS, CHK_SIZE, CHK_TREE, CHK_TMASKS.
#[derive(Default, Clone)]
struct Chk {
    regions: Vec<u8>,
    specks: Vec<u8>,
    size: Vec<u8>,
    tree: Vec<u8>,
    tmasks: Vec<Vec<u8>>,
}

/// check_tree_masks POINT WxH: the tree diff's view of a checkpoint's --ignore regions.
fn tree_masks(point: &[u8], wh: &[u8]) -> Vec<Vec<u8>> {
    let Some(caps) = shell::Re::new("^([0-9]+)x([0-9]+)$").and_then(|re| re.captures(wh, false))
    else {
        return Vec::new();
    };
    let g = |i: usize| {
        caps.get(i)
            .copied()
            .flatten()
            .map(|(a, b)| wh[a..b].to_vec())
            .unwrap_or_default()
    };
    if !has_line_starting(point, b"ignore ") {
        return Vec::new();
    }
    let mut m = vec![b"--shot".to_vec(), g(1), g(2)];
    for l in file_lines(point) {
        let f = shell::read_fields(&l, 5);
        if f[0] == b"ignore" {
            m.push(b"--mask".to_vec());
            m.extend(f[1..].iter().cloned());
        }
    }
    m
}

/// check_one BASE NEW ID SPECK [UNSTABLE]: one checkpoint of NEW against BASE, its --ignore regions
/// and what UNSTABLE lists left out. None when magick fails.
fn check_one(base: &[u8], new: &[u8], id: &[u8], speck: i64, unstable: &[u8]) -> Option<Chk> {
    let mut c = Chk {
        specks: b"0 0 0".to_vec(),
        ..Chk::default()
    };
    let mut masks: Vec<Vec<u8>> = word_rest(&cat!(new, "/", id, ".point"))
        .into_iter()
        .filter(|(l, _)| l == b"ignore")
        .map(|(_, m)| m)
        .collect();
    let has_unst = !unstable.is_empty() && shell::path(unstable).is_file();
    if has_unst {
        for l in file_lines(unstable) {
            if l.starts_with(b"tree ") {
                continue;
            }
            if let Some(m) = mask_of(&l) {
                masks.push(m);
            }
        }
    }
    let (bp, np) = (cat!(base, "/", id, ".png"), cat!(new, "/", id, ".png"));
    let sa = identify("%wx%h", &bp)?;
    let sb = identify("%wx%h", &np)?;
    if sa != sb {
        c.size = cat!(sa, " -> ", sb);
    } else {
        let res = check_regions(&bp, &np, speck.to_string().as_bytes(), &masks)?;
        (c.regions, c.specks) = split_specks(&res);
    }
    let (bt, nt) = (
        cat!(base, "/", id, ".tree.json"),
        cat!(new, "/", id, ".tree.json"),
    );
    if shell::path(&bt).is_file() && shell::path(&nt).is_file() {
        let mut ign = Vec::new();
        if has_unst {
            ign = cat!(new, "/", id, ".tree-ignore");
            tree_lines_to(unstable, &ign);
        }
        c.tmasks = tree_masks(&cat!(new, "/", id, ".point"), &sb);
        let mut a = w(&[b"tree".as_slice(), &bt, &nt, &ign]);
        a.extend(c.tmasks.iter().cloned());
        // (`CHK_TREE=$(python3 ... tree ...)`)
        let r = py(&a, None, false);
        c.tree = r.text();
        if !ign.is_empty() {
            rm(&ign);
        }
    }
    Some(c)
}

/// check_change_key BASE NEW ID BASE_TREE NEW_TREE [HOLE...]: what a changed checkpoint's change
/// is, to tell one seen before (#257).
fn change_key(
    c: &Chk,
    base: &[u8],
    new: &[u8],
    id: &[u8],
    bt: &[u8],
    nt: &[u8],
    holes: &[Vec<u8>],
) -> Vec<u8> {
    change_key_in(c, base, new, id, bt, nt, holes)
}

fn change_key_in(
    c: &Chk,
    base: &[u8],
    new: &[u8],
    id: &[u8],
    bt: &[u8],
    nt: &[u8],
    holes: &[Vec<u8>],
) -> Vec<u8> {
    let mut k = cat!("size ", c.size, "\ntree ", c.tree, "\n");
    if !c.size.is_empty() {
    } else if !c.tree.is_empty() && !bt.is_empty() && !c.regions.is_empty() {
        let mut ra: Vec<Vec<u8>> = Vec::new();
        for r in lines(&c.regions) {
            if let Some(m) = mask_of(r) {
                ra.push(b"--region".to_vec());
                ra.extend(shell::words(&m));
            }
        }
        let mut ign = Vec::new();
        let unst = cat!(base, "/", id, ".unstable");
        if shell::path(&unst).is_file() {
            ign = cat!(new, "/", id, ".key-ignore");
            tree_lines_to(&unst, &ign);
        }
        let np = cat!(new, "/", id, ".png");
        let mut tm = tree_masks(
            &cat!(new, "/", id, ".point"),
            &identify("%wx%h", &np).unwrap_or_default(),
        );
        if tm.is_empty() {
            tm = vec![b"--shot".to_vec()];
            tm.extend(shell::words(&identify("%w %h", &np).unwrap_or_default()));
        }
        let mut a = w(&[b"unexplained".as_slice(), bt, nt, &ign]);
        a.extend(tm);
        a.extend(ra);
        let r = py(&a, None, true);
        for l in r.out.split_inclusive(|&ch| ch == b'\n') {
            k.extend_from_slice(b"region ");
            k.extend_from_slice(l);
        }
        if !ign.is_empty() {
            rm(&ign);
        }
    } else if !c.regions.is_empty() {
        for l in cut12(&c.regions) {
            k.extend_from_slice(&cat!("region ", l, "\n"));
        }
    }
    for r in holes {
        k.extend_from_slice(&cat!("hole ", pct_sp(r), "\n"));
    }
    strip_nl(&k)
}

/// check_speck_text SPECK LARGEST GIVEN: what a checkpoint's speck is, said at --approve.
fn speck_text(ps: i64, big: i64, given: bool) -> String {
    if given {
        format!("specks under {ps} px left out (--speck)")
    } else if big == 0 {
        format!("specks under {ps} px left out: the runs agreed to the pixel")
    } else {
        format!("specks under {ps} px left out: the runs' largest stray part was {big} px")
    }
}

/// `s` unless N is 1.
fn s_unless1(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// What check run passes on to its comparison and its reports: --only's patterns (#376; none: all
/// checkpoints) and --report-scale (#402; 0 or 1: report.png alone).
#[derive(Default, Clone)]
struct Opts {
    only: Vec<Vec<u8>>,
    scale: u32,
}

/// An --only pattern: a checkpoint's ID or a glob of IDs (`*`, `?`, `[...]`).
fn only_ok(p: &[u8]) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && p.iter()
            .all(|&c| c.is_ascii_alphanumeric() || b"._-*?[]!".contains(&c))
}

/// --only's value, comma-separated patterns, each checked (None: one is not a pattern).
fn only_split(v: &[u8]) -> Option<Vec<Vec<u8>>> {
    let ps: Vec<Vec<u8>> = v
        .split(|&c| c == b',')
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect();
    (!ps.is_empty() && ps.iter().all(|p| only_ok(p))).then_some(ps)
}

/// Whether ID matches the glob PAT (fnmatch, as a shell's case).
fn glob_match(pat: &[u8], id: &[u8]) -> bool {
    let (Ok(p), Ok(s)) = (
        std::ffi::CString::new(pat.to_vec()),
        std::ffi::CString::new(id.to_vec()),
    ) else {
        return false;
    };
    // SAFETY: two NUL-ended strings alive for the call.
    unsafe { libc::fnmatch(p.as_ptr(), s.as_ptr(), 0) == 0 }
}

/// Whether a run with --only ONLY takes checkpoint ID (no --only: every one).
fn wanted(only: &[Vec<u8>], id: &[u8]) -> bool {
    only.is_empty() || only.iter().any(|p| glob_match(p, id))
}

/// The --only of the run a scenario is in, from OMABOX_CHECK_ONLY (none: every checkpoint).
fn env_only() -> Vec<Vec<u8>> {
    let v = shell::env_or_empty("OMABOX_CHECK_ONLY");
    v.split(|&c| c == b',')
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// The IDs of a baseline's checkpoints (its .point files), sorted.
fn base_ids(base: &[u8]) -> Vec<Vec<u8>> {
    shell::glob_ext(base, b".point")
        .iter()
        .map(|r| {
            let id = shell::basename(r);
            id.strip_suffix(b".point").unwrap_or(&id).to_vec()
        })
        .collect()
}

/// REPORT.png's people's copy at N (#402): REPORT@Nx.png beside it.
fn scaled_path(png: &[u8], n: u32) -> Vec<u8> {
    let stem = png.strip_suffix(b".png").unwrap_or(png);
    cat!(stem, "@", n.to_string(), "x.png")
}

// --- the command ------------------------------------------------------------------------------------

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    ctx.die_rc = Some(2);
    crate::trap::stop_on_signals();
    let mut a: &[Vec<u8>] = args;
    let mut name = Vec::new();
    if a.first().is_some_and(|x| x == b"-b" || x == b"--box") {
        optarg(ctx, &a[0], a.get(1))?;
        name = a[1].clone();
        a = &a[2..];
    }
    let sub = a.first().cloned().unwrap_or_default();
    if !a.is_empty() {
        a = &a[1..];
    }
    if !name.is_empty() && sub != b"point" {
        return Err(ctx.die(cat!(
            "check ",
            sub,
            ": -b NAME goes with check point (run starts boxes of its own, diff reads files)"
        )));
    }
    match sub.as_slice() {
        b"run" => check_run(ctx, a),
        b"point" => {
            let mut v = Vec::new();
            if !name.is_empty() {
                v.push(b"-b".to_vec());
                v.push(name);
            }
            v.extend(a.iter().cloned());
            check_point(ctx, &v)
        }
        b"diff" => check_diff(ctx, a),
        b"wants" => check_wants(ctx, a),
        b"-h" | b"--help" => crate::args::usage("check"),
        b"" => {
            crate::args::usage("check")?;
            Err(Exit(2))
        }
        _ => Err(unknown_word(
            ctx,
            "check",
            "subcommand",
            &sub,
            "(run, point, diff or wants)",
            &["run", "point", "diff", "wants"],
        )),
    }
}

/// check wants ID... (#376): whether the run the scenario is in takes any of these checkpoints (its
/// --only, in OMABOX_CHECK_ONLY; without one, every checkpoint): exit 0 yes, 1 no. A scenario skips
/// a section with it, where the time is (driving the app to a screen, not the shot).
fn check_wants(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let mut ids: Vec<Vec<u8>> = Vec::new();
    for x in args {
        match x.as_slice() {
            b"-h" | b"--help" => return crate::args::usage("check"),
            y if y.starts_with(b"-") => return Err(unknown_opt(ctx, "check wants", x)),
            y => {
                if !shell::re_match(CHECK_ID_RE, y) {
                    return Err(ctx.die(cat!(
                        "check wants: an ID is letters, digits, . _ - (64 at most, not starting with . _ -), got ",
                        y
                    )));
                }
                ids.push(y.to_vec());
            }
        }
    }
    if ids.is_empty() {
        return Err(ctx.die(
            "check wants: which checkpoints? (their IDs: if omabox check wants settings about; then ...)",
        ));
    }
    ctx.parsed();
    let only = env_only();
    if ids.iter().any(|id| wanted(&only, id)) {
        Ok(())
    } else {
        Err(Exit(1))
    }
}

fn need_magick(ctx: &Ctx) -> Res<()> {
    if crate::exec::executable_on_path(b"magick") {
        return Ok(());
    }
    Err(ctx.die("check: needs ImageMagick's magick (package imagemagick, part of Omarchy)"))
}

/// `jq -nc --arg K V ... "$CHK_JQ"PROG`: one line of check's JSON, on stdout or appended to a file.
fn chk_json(pairs: &[(&str, &[u8])], prog: &str) -> Vec<u8> {
    let mut a = Args::new(&["-nc"]);
    for (k, v) in pairs {
        a = a.kv("--arg", k, v);
    }
    a.arg(format!("{CHK_JQ}{prog}"))
        .run(None, jq::Err::Inherit)
        .out
}

// --- check diff ---

fn check_diff(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut files, mut masks) = (Vec::<Vec<u8>>::new(), Vec::<Vec<u8>>::new());
    let (mut speck, mut outp, mut report, mut json) = (CHECK_SPECK, Vec::new(), Vec::new(), false);
    let mut scale = 0u32;
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        let v = args.get(i + 1);
        match x.as_slice() {
            b"--json" => json = true,
            b"--ignore" => {
                optarg(ctx, x, v)?;
                let g = geom_parse(&args[i + 1]).ok_or_else(|| {
                    ctx.die(cat!(
                        "check diff: --ignore is \"X,Y WxH\", got ",
                        args[i + 1]
                    ))
                })?;
                masks.push(g.join(&b' '));
                i += 1;
            }
            b"--speck" => {
                optarg(ctx, x, v)?;
                let s = &args[i + 1];
                if !shell::re_match("^[0-9]{1,6}$", s) {
                    return Err(ctx.die(cat!("check diff: --speck is a number of pixels, got ", s)));
                }
                speck = shell::dec(s).unwrap_or(0);
                i += 1;
            }
            b"-o" | b"--output" => {
                optarg(ctx, x, v)?;
                outp = args[i + 1].clone();
                i += 1;
            }
            b"--report" => {
                optarg(ctx, x, v)?;
                report = args[i + 1].clone();
                i += 1;
            }
            b"--report-scale" => {
                optarg(ctx, x, v)?;
                scale = report_scale(ctx, "check diff", &args[i + 1])?;
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("check"),
            y if y.starts_with(b"-") => return Err(unknown_opt(ctx, "check diff", x)),
            _ => files.push(x.clone()),
        }
        i += 1;
    }
    if files.len() != 2 {
        return Err(ctx.die("check diff: two shots, A.png B.png"));
    }
    if scale > 0 && report.is_empty() {
        return Err(ctx.die(
            "check diff: --report-scale goes with --report R.png (it writes R@Nx.png beside it)",
        ));
    }
    ctx.parsed();
    need_magick(ctx)?;
    for f in &files {
        if !shell::path(f).is_file() {
            return Err(ctx.die(cat!("check diff: no such file: ", f)));
        }
        if !report.is_empty() && f.iter().any(|&c| c == b'\t' || c == b'\n') {
            return Err(
                ctx.die("check diff: --report takes no file whose name has a tab or a newline")
            );
        }
    }
    let sa = identify("%wx%h", &files[0])
        .ok_or_else(|| ctx.die(cat!("check diff: not an image: ", files[0])))?;
    let sb = identify("%wx%h", &files[1])
        .ok_or_else(|| ctx.die(cat!("check diff: not an image: ", files[1])))?;
    // The report's plan: check run's, one pair (the shots' names its label).
    let mut plan = Vec::new();
    if !report.is_empty() {
        let tmpd = shell::env("TMPDIR")
            .filter(|t| !t.is_empty())
            .unwrap_or(b"/tmp".to_vec());
        let (_, p) = shell::mktemp_in(&cat!(tmpd, "/omabox-check-XXXXXX"))
            .map_err(|_| ctx.die("check diff: no temporary file for the report"))?;
        plan = p;
        let mut pid = shell::basename(&files[1]);
        if pid.ends_with(b".png") {
            pid.truncate(pid.len() - 4);
        }
        put(
            &plan,
            &cat!(
                "title\tcheck diff: ",
                shell::basename(&files[0]),
                ", then ",
                shell::basename(&files[1]),
                "; red: pixels that changed\npoint\t",
                pid,
                "\t",
                files[0],
                "\t",
                files[1],
                "\t\t\t\n"
            ),
        )?;
    }
    // A refusal past here takes the plan with it (finding 431: bash's left it in $TMPDIR).
    let plan_g = PlanGuard(plan.clone());
    // (With --json the text lines are not said: the object at the end says it all.)
    let said = |v: &[u8]| -> Res<()> { if json { Ok(()) } else { out(v) } };
    let diff_json = |res: &str,
                     size: &[u8],
                     regs: &[u8],
                     sp: &[u8],
                     marked: &[u8],
                     (rep, rs): (&[u8], &[u8])| {
        if json {
            let o = chk_json(
                &[
                    ("res", res.as_bytes()),
                    ("size", size),
                    ("regs", regs),
                    ("sp", sp),
                    ("marked", marked),
                    ("rep", rep),
                    ("rs", rs),
                ],
                CHK_DIFF_JQ,
            );
            let _ = out(&o);
        }
    };
    if sa != sb {
        said(&cat!("size ", sa, " -> ", sb, "\n"))?;
        let (mut rep, mut rs) = (Vec::new(), Vec::new());
        if !plan.is_empty() {
            append(&plan, &cat!("size\t", sa, " -> ", sb, "\n"));
            let (ok, s) = diff_report(&plan, &report, scale, &said);
            if ok {
                rep = report.clone();
            }
            rs = s;
        }
        plan_g.forget();
        diff_json("changed", &cat!(sa, " -> ", sb), b"", b"", b"", (&rep, &rs));
        return Err(Exit(1));
    }
    let res = check_regions(&files[0], &files[1], speck.to_string().as_bytes(), &masks)
        .ok_or_else(|| ctx.die("check diff: could not compare them (magick)"))?;
    let (regions, spl) = split_specks(&res);
    let sp = pct_sp(&spl).to_vec();
    if regions.is_empty() {
        if !plan.is_empty() {
            rm(&plan);
        }
        plan_g.forget();
        said(&cat!(
            "same (specks: ",
            pct_sp(&sp),
            ", ",
            hash_sp(&sp),
            " px)\n"
        ))?;
        diff_json("same", b"", b"", &spl, b"", (b"", b""));
        return Ok(());
    }
    for r in lines(&regions) {
        said(&cat!(pct_sp(r), " (", hash2_sp(r), " px)\n"))?;
        if !plan.is_empty() {
            if let Some(m) = mask_of(r) {
                append(&plan, &cat!("region\t", tabs(&m), "\n"));
            }
        }
    }
    said(&cat!("specks: ", pct_sp(&sp), ", ", hash_sp(&sp), " px\n"))?;
    if !outp.is_empty() {
        let rs: Vec<Vec<u8>> = lines(&regions).iter().map(|l| l.to_vec()).collect();
        if !check_mark(&files[1], &outp, &rs) {
            return Err(ctx.die(cat!("check diff: could not draw ", outp, " (magick)")));
        }
    }
    let (mut rep, mut rs) = (Vec::new(), Vec::new());
    if !plan.is_empty() {
        let (ok, s) = diff_report(&plan, &report, scale, &said);
        if ok {
            rep = report.clone();
        }
        rs = s;
    }
    plan_g.forget();
    diff_json("changed", b"", &regions, &spl, &outp, (&rep, &rs));
    Err(Exit(1))
}

/// check diff's plan in $TMPDIR, removed when a refusal ends the command first (finding 431).
struct PlanGuard(Vec<u8>);

impl PlanGuard {
    fn forget(self) {
        std::mem::forget(self);
    }
}

impl Drop for PlanGuard {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            rm(&self.0);
        }
    }
}

/// check_diff_report PLAN REPORT: the report of a plan (and at --report-scale N its people's copy,
/// REPORT@Nx.png: #402), then the plan gone; a failure said (the answer stands). Whether the report
/// was drawn, and the copy's path ("" for none).
fn diff_report(
    plan: &[u8],
    report: &[u8],
    scale: u32,
    said: &dyn Fn(&[u8]) -> Res<()>,
) -> (bool, Vec<u8>) {
    let ok = match check_report(plan, report, "sheet") {
        Some(left) => {
            let more = if left.is_empty() {
                Vec::new()
            } else {
                cat!(" (not in it, no room: ", left, ")")
            };
            let _ = said(&cat!("report: ", report, more, "\n"));
            true
        }
        None => {
            say(cat!(
                "check diff: the report could not be drawn (magick, to ",
                report,
                ")"
            ));
            false
        }
    };
    let scaled = people_copy(plan, report, "sheet", scale, "report", "check diff", said);
    rm(plan);
    (ok, scaled)
}

/// The people's copy of a report or overview at --report-scale N (#402), drawn and said ("LEAD at
/// Nx: PATH ..."), or why not on stderr (WHO's words); its path, "" for none (N under 2 too).
fn people_copy(
    plan: &[u8],
    png: &[u8],
    kind: &str,
    n: u32,
    lead: &str,
    who: &str,
    said: &dyn Fn(&[u8]) -> Res<()>,
) -> Vec<u8> {
    if n < 2 {
        return Vec::new();
    }
    match scaled_report(plan, png, kind, n) {
        Some((p, pages)) => {
            // (an overview's pages after the first, a line each)
            let more = if kind == "overview" && !pages.is_empty() {
                let pl = lines(&pages);
                let joined: Vec<u8> = pl.iter().flat_map(|l| cat!(" ", l)).collect();
                cat!(" and ", pl.len().to_string(), " more pages:", joined)
            } else {
                Vec::new()
            };
            let _ = said(&cat!(
                lead,
                " at ",
                n.to_string(),
                "x: ",
                p,
                more,
                " (for people: the same, ",
                n.to_string(),
                " times its size; agents read the 1x one)\n"
            ));
            p
        }
        None => {
            say(cat!(
                who,
                ": the ",
                lead,
                " at ",
                n.to_string(),
                "x could not be drawn (magick, to ",
                scaled_path(png, n),
                ")"
            ));
            Vec::new()
        }
    }
}

/// --report-scale's value: 2 to 4 (#402).
fn report_scale(ctx: &Ctx, who: &str, v: &[u8]) -> Res<u32> {
    if !shell::re_match("^[2-4]$", v) {
        return Err(ctx.die(cat!(
            who,
            ": --report-scale is 2 to 4 (report.png itself is 1, sized for an agent), got ",
            v
        )));
    }
    Ok(u32::try_from(shell::dec(v).unwrap_or(2)).unwrap_or(2))
}

// --- check point ---

/// check_ignore_px OX OY LW LH IW IH X Y W H: a checkpoint's --ignore region in its shot's pixels,
/// "X Y W H", cut to the shot and rounded outwards; None when none of it is in the shot.
fn ignore_px(v: [i64; 10]) -> Option<Vec<u8>> {
    let [ox, oy, lw, lh, iw, ih, mut x, mut y, gw, gh] = v;
    let (mut x2, mut y2) = (x + gw, y + gh);
    if x <= ox {
        x = ox;
    }
    if y <= oy {
        y = oy;
    }
    if x2 >= ox + lw {
        x2 = ox + lw;
    }
    if y2 >= oy + lh {
        y2 = oy + lh;
    }
    if !(x2 > x && y2 > y) || lw == 0 || lh == 0 {
        return None;
    }
    let (x, y) = ((x - ox) * iw / lw, (y - oy) * ih / lh);
    let (x2, y2) = (
        ((x2 - ox) * iw + lw - 1) / lw,
        ((y2 - oy) * ih + lh - 1) / lh,
    );
    Some(format!("{x} {y} {} {}", x2 - x, y2 - y).into_bytes())
}

/// ignore_px's region cut to the shot, as "WxH at X,Y" in its own coordinates, when the shot cut it
/// (#342: a region given in the wrong coordinates was kept as a sliver, without a word); None when
/// it lies wholly inside, or wholly outside (ignore_px's None, said apart).
fn ignore_cut(v: [i64; 10]) -> Option<String> {
    let [ox, oy, lw, lh, _, _, x, y, gw, gh] = v;
    let (cx, cy) = (x.max(ox), y.max(oy));
    let (cx2, cy2) = ((x + gw).min(ox + lw), (y + gh).min(oy + lh));
    let cut = (cx, cy, cx2, cy2) != (x, y, x + gw, y + gh);
    (cut && cx2 > cx && cy2 > cy).then(|| format!("{}x{} at {cx},{cy}", cx2 - cx, cy2 - cy))
}

/// `"$self" -b NAME ARGS... >/dev/null 2>FILE` (FILE None: 2>&1 to /dev/null), with ENV: its status.
fn self_cmd(ctx: &Ctx, args: &[Vec<u8>], err: Option<&[u8]>, env: &[(&str, &str)]) -> i32 {
    let mut c = crate::sig::omabox_cmd();
    c.arg("-b").arg(OsStr::from_bytes(&ctx.name));
    c.args(args.iter().map(|a| OsStr::from_bytes(a)));
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdout(Stdio::null());
    match err {
        Some(p) => match std::fs::File::create(shell::path(p)) {
            Ok(f) => c.stderr(Stdio::from(f)),
            Err(e) => {
                say(cat!("check: ", crate::shots::cant_write(p, &e)));
                return 1;
            }
        },
        None => c.stderr(Stdio::null()),
    };
    crate::sig::run_status(c).unwrap_or(127)
}

/// `$(tail -n 1 FILE)`.
fn tail1(p: &[u8]) -> Vec<u8> {
    let d = std::fs::read(shell::path(p)).unwrap_or_default();
    let d = d.strip_suffix(b"\n").unwrap_or(&d);
    let l = d.rsplit(|&c| c == b'\n').next().unwrap_or_default();
    strip_nl(l)
}

fn check_point(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut name, mut id, mut sels, mut geom) =
        (Vec::new(), Vec::new(), Vec::<Vec<u8>>::new(), Vec::new());
    let mut masks: Vec<Vec<u8>> = Vec::new();
    let mut outd = shell::env_or_empty("OMABOX_CHECK_OUT");
    let mut tmo = b"15s".to_vec();
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        let v = args.get(i + 1);
        match x.as_slice() {
            b"-b" | b"--box" => {
                optarg(ctx, x, v)?;
                name = args[i + 1].clone();
                i += 1;
            }
            b"-w" | b"--window" => {
                optarg(ctx, x, v)?;
                sels.push(args[i + 1].clone());
                i += 1;
            }
            b"--active" => sels.push(b"active".to_vec()),
            b"-g" => {
                optarg(ctx, x, v)?;
                let Some(g) = geom_parse(&args[i + 1]) else {
                    return Err(ctx.die(cat!("check point: -g is \"X,Y WxH\", got ", args[i + 1])));
                };
                geom = geom_text(&g);
                i += 1;
            }
            b"--ignore" => {
                optarg(ctx, x, v)?;
                let g = geom_parse(&args[i + 1]).ok_or_else(|| {
                    ctx.die(cat!(
                        "check point: --ignore is \"X,Y WxH\" (with --window in its coordinates, else the screen's), got ",
                        args[i + 1]
                    ))
                })?;
                masks.push(g.join(&b' '));
                i += 1;
            }
            b"--timeout" => {
                optarg(ctx, x, v)?;
                let t = &args[i + 1];
                if !crate::num::ms_duration(t).is_some_and(|ms| ms <= WAIT_MAX_MS) {
                    return Err(ctx.die(cat!(
                        "check point: --timeout takes a duration up to 10m, like 15s (a bare number is seconds), got ",
                        t
                    )));
                }
                tmo = t.clone();
                i += 1;
            }
            b"-o" | b"--output" => {
                optarg(ctx, x, v)?;
                outd = args[i + 1].clone();
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("check"),
            y if y.starts_with(b"-") => return Err(unknown_opt(ctx, "check point", x)),
            y => {
                if !id.is_empty() {
                    return Err(ctx.die(cat!("check point: one ID, got ", id, " and ", y)));
                }
                id = y.to_vec();
            }
        }
        i += 1;
    }
    if id.is_empty() {
        return Err(ctx.die("check point: which checkpoint? (an ID: letters, digits, . _ -)"));
    }
    if !shell::re_match(CHECK_ID_RE, &id) {
        return Err(ctx.die(cat!(
            "check point: an ID is letters, digits, . _ - (64 at most, not starting with . _ -), got ",
            id
        )));
    }
    if outd.is_empty() {
        return Err(ctx.die(
            "check point: outside a check run (omabox check run -- SCENARIO calls it), give -o DIR",
        ));
    }
    // A checkpoint the run's --only does not name is not taken (#376): no wait, no shot.
    let only = env_only();
    if !wanted(&only, &id) {
        say(cat!(
            "check point ",
            id,
            ": skipped, not in --only (",
            only.join(&b','),
            ")"
        ));
        return Ok(());
    }
    mkdirs(ctx, &outd)?;
    let f = |ext: &str| cat!(outd, "/", id, ext);
    let png = f(".png");
    // (`[ ! -e ]`: a link to nothing is no shot taken)
    if shell::path(&png).exists() {
        return Err(ctx.die(cat!(
            "check point: ",
            id,
            " was taken already in this run (each ID once)"
        )));
    }
    select_box(ctx, &name)?;
    need_box(ctx)?;
    let mut wargs: Vec<Vec<u8>> = Vec::new();
    for s in &sels {
        wargs.push(b"--window".to_vec());
        wargs.push(s.clone());
    }
    let mut sargs = wargs.clone();
    if !geom.is_empty() {
        sargs.push(b"-g".to_vec());
        sargs.push(geom.clone());
        if sels.is_empty() {
            wargs = vec![b"-g".to_vec(), geom.clone()];
        }
    }
    let mut wa = w(&[
        b"wait".as_slice(),
        b"--timeout",
        &tmo,
        b"still",
        b"--quiet",
        b"500ms",
    ]);
    wa.extend(wargs.iter().cloned());
    let mut settled = "yes";
    match self_cmd(ctx, &wa, Some(&f(".wait")), &[]) {
        0 => {}
        124 => {
            settled = "no";
            say(cat!(
                "check point ",
                id,
                ": not still after ",
                tmo,
                ": taken anyway, marked unsettled (",
                tail1(&f(".wait")),
                ")"
            ));
        }
        _ => return Err(ctx.die(cat!("check point ", id, ": ", tail1(&f(".wait"))))),
    }
    rm(&f(".wait"));
    // Full size (--fit 0): a baseline is the same image on every machine (#281).
    let mut sa = vec![b"shot".to_vec()];
    sa.extend(sargs.iter().cloned());
    sa.extend(w(&[b"--fit".as_slice(), b"0", b"-o", &png]));
    if self_cmd(
        ctx,
        &sa,
        Some(&f(".shot")),
        &[("OMABOX_SHOT_NO_BASELINE", "1")],
    ) != 0
    {
        return Err(ctx.die(cat!(
            "check point ",
            id,
            ": the shot failed: ",
            tail1(&f(".shot"))
        )));
    }
    rm(&f(".shot"));
    // --ignore regions into the shot's pixels (#281): what the shot covers is in its record.
    if !masks.is_empty() {
        let p = crate::repo::readlink_f(&png).unwrap_or_default();
        let rec = std::fs::read(ctx.at("shots.tsv"))
            .ok()
            .map(|d| {
                let mut last = Vec::new();
                for l in d.split(|&c| c == b'\n') {
                    if l.split(|&c| c == b'\t').next() == Some(p.as_slice())
                        && !(l.is_empty() && p.is_empty())
                    {
                        last = l.to_vec();
                    }
                }
                last
            })
            .unwrap_or_default();
        let fr = shell::read_ifs(&rec, 5, b"\t");
        let (kind, geo) = (fr[3].clone(), fr[4].clone());
        let gs = shell::read_ifs(&geo, 4, b"x ");
        let (lw, lh, iw, ih) = (&gs[0], &gs[1], &gs[2], &gs[3]);
        if !shell::re_match(
            "^[1-9][0-9]* [1-9][0-9]* [1-9][0-9]* [1-9][0-9]*$",
            &cat!(lw, " ", lh, " ", iw, " ", ih),
        ) {
            rm(&png);
            return Err(ctx.die(cat!(
                "check point ",
                id,
                ": no record of what its shot covers, for --ignore (a path with a tab or newline?)"
            )));
        }
        let (mut ox, mut oy) = (b"0".to_vec(), b"0".to_vec());
        let org = if shell::re_match("^window .* ", &kind) {
            Some(shell::read_fields(&kind, 4)[2].clone())
        } else {
            kind.strip_prefix(b"screen ").map(<[u8]>::to_vec)
        };
        if let Some(org) = org {
            ox = org
                .iter()
                .rposition(|&c| c == b',')
                .map_or(org.clone(), |i| org[..i].to_vec());
            oy = org
                .iter()
                .position(|&c| c == b',')
                .map_or(org.clone(), |i| org[i + 1..].to_vec());
        }
        let space = if sels.is_empty() {
            "the screen's coordinates"
        } else {
            "the window's coordinates"
        };
        let mut px = Vec::new();
        for g in &masks {
            let gw = shell::read_fields(g, 4);
            // (`local ... x2=$(($7 + $9)) y2=$(($8 + ${10}))`: one that is no number is said, and
            // the region left out)
            let ok = arith(&[&gw[0], &gw[2]]).is_some() && arith(&[&gw[1], &gw[3]]).is_some();
            let v = [
                ai(&ox),
                ai(&oy),
                ai(lw),
                ai(lh),
                ai(iw),
                ai(ih),
                ai(&gw[0]),
                ai(&gw[1]),
                ai(&gw[2]),
                ai(&gw[3]),
            ];
            match ignore_px(v).filter(|_| ok) {
                Some(m) => {
                    // (part of it outside the shot: what was kept said, #342)
                    if let Some(c) = ignore_cut(v) {
                        say(cat!(
                            "check point ",
                            id,
                            ": --ignore \"",
                            gw[0],
                            ",",
                            gw[1],
                            " ",
                            gw[2],
                            "x",
                            gw[3],
                            "\" is partly outside the shot (",
                            space,
                            "; it covers ",
                            lw,
                            "x",
                            lh,
                            " at ",
                            ox,
                            ",",
                            oy,
                            "): cut to ",
                            c
                        ));
                    }
                    px.push(m);
                }
                None => say(cat!(
                    "check point ",
                    id,
                    ": --ignore \"",
                    gw[0],
                    ",",
                    gw[1],
                    " ",
                    gw[2],
                    "x",
                    gw[3],
                    "\" has nothing in the shot (",
                    space,
                    "; it covers ",
                    lw,
                    "x",
                    lh,
                    " at ",
                    ox,
                    ",",
                    oy,
                    "): left out"
                )),
            }
        }
        masks = px;
    }
    let mut tree = "none";
    if !sels.is_empty() {
        let tj = f(".tree.json");
        let mut ta = vec![b"tree".to_vec()];
        ta.extend(wargs.iter().cloned());
        ta.push(b"--json".to_vec());
        let mut rc = self_out(ctx, &ta, &tj);
        if rc == 0 && !geom.is_empty() {
            let crop = f(".tree.crop");
            rc = 1;
            if let Some(cg) = geom_parse(&geom) {
                let mut a = w(&[b"crop-tree".as_slice(), &tj]);
                a.extend(cg.iter().cloned());
                let r = py(&a, None, false);
                let wrote = std::fs::write(shell::path(&crop), &r.out).is_ok();
                if r.ok() && wrote && crate::shots::mv_f(&crop, &tj) {
                    rc = 0;
                }
            }
            rm(&crop);
        }
        if rc == 0 {
            tree = "yes";
        } else {
            rm(&tj);
        }
    }
    // Holes: a window's own transparent pixels; on the screen, the sentinel wallpaper enclosed by
    // surfaces. A region's are the screen's, shot once more whole.
    let (mut hoff, mut hres) = (Vec::new(), Vec::new());
    let unreadable = b"could not read the shot (magick)".to_vec();
    if !sels.is_empty() {
        match check_holes(&png, "window", b"") {
            Some(h) => hres = h,
            None => hoff = unreadable,
        }
    } else {
        hoff = check_holes_off(ctx);
        if hoff.is_empty() && geom.is_empty() {
            match check_holes(&png, "screen", b"") {
                Some(h) => hres = h,
                None => hoff = unreadable,
            }
        } else if hoff.is_empty() {
            let g = geom_parse(&geom).unwrap_or_default();
            let iws = crate::shots::png_size(&png)
                .map_or(g[2].clone(), |(a, _)| a.to_string().into_bytes());
            let sp = f(".screen.png");
            let sa = w(&[b"shot".as_slice(), b"--fit", b"0", b"-o", &sp]);
            if self_cmd(ctx, &sa, None, &[("OMABOX_SHOT_NO_BASELINE", "1")]) == 0 {
                // (`$((x * iw / w))` ...: one that is no number is said, its holes unread)
                let mut vals = Vec::new();
                for v in [&g[0], &iws, &g[2], &g[1], &g[3]] {
                    match arith(&[v]) {
                        Some(n) => vals.push(n),
                        None => break,
                    }
                }
                let within = (vals.len() == 5 && vals[2] != 0).then(|| {
                    let (x, iw, gw, y, h) = (vals[0], vals[1], vals[2], vals[3], vals[4]);
                    format!(
                        "{} {} {} {}",
                        x * iw / gw,
                        y * iw / gw,
                        String::from_utf8_lossy(&iws),
                        h * iw / gw
                    )
                    .into_bytes()
                });
                match within.and_then(|wn| check_holes(&sp, "screen", &wn)) {
                    Some(hh) => hres = hh,
                    None => hoff = unreadable,
                }
            } else {
                hoff = b"the screen's shot failed".to_vec();
            }
            rm(&sp);
        }
    }
    // A hole inside an --ignore region is left out with it.
    let mut holes = Vec::new();
    let hre = shell::Re::new(r"^(-?[0-9]+),(-?[0-9]+) ([0-9]+)x([0-9]+) [0-9]+$");
    'h: for hr in lines(&hres) {
        let Some(caps) = hre.as_ref().and_then(|re| re.captures(hr, false)) else {
            continue;
        };
        let g = |i: usize| {
            ai(&caps
                .get(i)
                .copied()
                .flatten()
                .map(|(a, b)| hr[a..b].to_vec())
                .unwrap_or_default())
        };
        for m in &masks {
            let mf = shell::read_fields(m, 4);
            let (x, y, mw, mh) = (ai(&mf[0]), ai(&mf[1]), ai(&mf[2]), ai(&mf[3]));
            if g(1) >= x && g(2) >= y && g(1) + g(3) <= x + mw && g(2) + g(4) <= y + mh {
                continue 'h;
            }
        }
        holes.extend_from_slice(&cat!("hole ", hr, "\n"));
    }
    let mut point = Vec::new();
    if !sels.is_empty() {
        point.extend_from_slice(&cat!("of window ", sels.join(&b' '), "\n"));
        if !geom.is_empty() {
            point.extend_from_slice(&cat!("crop ", geom, "\n"));
        }
    } else if !geom.is_empty() {
        point.extend_from_slice(&cat!("of region ", geom, "\n"));
    } else {
        point.extend_from_slice(b"of screen\n");
    }
    point.extend_from_slice(&cat!("settled ", settled, "\ntree ", tree, "\n"));
    for g in &masks {
        point.extend_from_slice(&cat!("ignore ", g, "\n"));
    }
    if !hoff.is_empty() {
        point.extend_from_slice(&cat!("holes off: ", hoff, "\n"));
    } else {
        point.extend_from_slice(b"holes checked\n");
        point.extend_from_slice(&holes);
    }
    let pf = f(".point");
    if let Err(e) = std::fs::write(shell::path(&pf), &point) {
        return Err(failed(ctx, crate::shots::cant_write(&pf, &e)));
    }
    if hoff.is_empty() && !holes.is_empty() {
        let what = hole_what(&pf);
        for l in lines(strip_nl1(&holes)) {
            let hr = shell::read_fields(l, 2)[1].clone();
            say(cat!(
                "check point ",
                id,
                ": ",
                what,
                " at ",
                pct_sp(&hr),
                " (",
                hash2_sp(&hr),
                " px): a hole"
            ));
        }
    }
    out(cat!(png, "\n"))
}

/// `${V%$'\n'}`.
fn strip_nl1(v: &[u8]) -> &[u8] {
    v.strip_suffix(b"\n").unwrap_or(v)
}

/// `"$self" -b NAME ARGS... > FILE 2>/dev/null`: its status.
fn self_out(ctx: &Ctx, args: &[Vec<u8>], file: &[u8]) -> i32 {
    let f = match std::fs::File::create(shell::path(file)) {
        Ok(f) => f,
        Err(e) => {
            say(cat!("check: ", crate::shots::cant_write(file, &e)));
            return 1;
        }
    };
    let mut c = crate::sig::omabox_cmd();
    c.arg("-b").arg(OsStr::from_bytes(&ctx.name));
    c.args(args.iter().map(|a| OsStr::from_bytes(a)));
    c.stdout(Stdio::from(f)).stderr(Stdio::null());
    crate::sig::run_status(c).unwrap_or(127)
}

// --- check run ---

/// check_env BOX: what the box of a run was (`ls --json` | jq), `{}` when that fails.
fn check_env(box_name: &[u8]) -> Vec<u8> {
    let mut c = crate::sig::omabox_cmd();
    c.args(["ls", "--json"])
        .stdin(Stdio::inherit())
        .stderr(Stdio::null());
    let (lrc, lout) = crate::sig::run_output(c).unwrap_or((127, Vec::new()));
    let ver = crate::cmd::version::version_text();
    let r = jq::tool(
        "jq",
        &Args::new(&["-c"])
            .kv("--arg", "n", box_name)
            .kv("--arg", "v", &ver)
            .arg(CHK_ENV_JQ)
            .into_words(),
        Some(&lout),
        jq::Err::Null,
    );
    let mut o = r.out.clone();
    if lrc != 0 || !r.ok() {
        o.extend_from_slice(b"{}\n");
    }
    o
}

/// `out=$(cd "$out" && pwd)`: OUT as cd -L makes it (logical, its . and .. taken out by name).
fn logical(dir: &[u8]) -> Vec<u8> {
    let full = if dir.starts_with(b"/") {
        dir.to_vec()
    } else {
        cat!(shell::pwd(), "/", dir)
    };
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in full.split(|&c| c == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    if parts.is_empty() {
        return b"/".to_vec();
    }
    let mut o = Vec::new();
    for p in parts {
        o.push(b'/');
        o.extend_from_slice(p);
    }
    // (a path whose .. went past a link: cd then takes the physical one)
    match (
        std::fs::metadata(shell::path(&o)),
        std::fs::metadata(shell::path(&full)),
    ) {
        (Ok(a), Ok(b))
            if std::os::unix::fs::MetadataExt::ino(&a)
                == std::os::unix::fs::MetadataExt::ino(&b)
                && std::os::unix::fs::MetadataExt::dev(&a)
                    == std::os::unix::fs::MetadataExt::dev(&b) =>
        {
            o
        }
        _ => std::fs::canonicalize(shell::path(&full))
            .map(|p| shell::bytes(&p).to_vec())
            .unwrap_or(o),
    }
}

/// Whether DIR has a file matching `*.png` (compgen -G "$base/*.png": hidden ones aside).
fn has_png(dir: &[u8]) -> bool {
    std::fs::read_dir(shell::path(dir)).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            let n = e.file_name();
            let n = n.as_bytes();
            n.ends_with(b".png") && n.first() != Some(&b'.')
        })
    })
}

/// `$(grep . FILE | tail -n 1)`.
fn last_line(p: &[u8]) -> Vec<u8> {
    let d = std::fs::read(shell::path(p)).unwrap_or_default();
    let g = shell::grep_lines(&d);
    g.rsplit(|&c| c == b'\n')
        .next()
        .unwrap_or_default()
        .to_vec()
}

fn check_run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    let (mut approve, mut runs, mut base, mut outd, mut speck, mut json) = (
        false,
        Vec::new(),
        b"omabox-check".to_vec(),
        Vec::new(),
        Vec::new(),
        false,
    );
    let mut upopts: Vec<Vec<u8>> = Vec::new();
    let mut scen: Vec<Vec<u8>> = Vec::new();
    let mut opts = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        let v = args.get(i + 1);
        match x.as_slice() {
            b"--approve" => approve = true,
            b"--json" => json = true,
            b"--only" => {
                optarg(ctx, x, v)?;
                let Some(ps) = only_split(&args[i + 1]) else {
                    return Err(ctx.die(cat!(
                        "check run: --only takes checkpoint IDs or globs of them (letters, digits, . _ - and * ? [ ]), comma-separated, like main,settings-*; got ",
                        args[i + 1]
                    )));
                };
                opts.only.extend(ps);
                i += 1;
            }
            b"--report-scale" => {
                optarg(ctx, x, v)?;
                opts.scale = report_scale(ctx, "check run", &args[i + 1])?;
                i += 1;
            }
            b"--runs" => {
                optarg(ctx, x, v)?;
                if !shell::re_match("^[2-9]$", &args[i + 1]) {
                    return Err(ctx.die(cat!(
                        "check run: --runs is 2 to 9 (for --approve), got ",
                        args[i + 1]
                    )));
                }
                runs = args[i + 1].clone();
                i += 1;
            }
            b"--base" => {
                optarg(ctx, x, v)?;
                base = args[i + 1].clone();
                i += 1;
            }
            b"-o" | b"--output" => {
                optarg(ctx, x, v)?;
                outd = args[i + 1].clone();
                i += 1;
            }
            b"--speck" => {
                optarg(ctx, x, v)?;
                if !shell::re_match("^[0-9]{1,6}$", &args[i + 1]) {
                    return Err(ctx.die(cat!(
                        "check run: --speck is a number of pixels, got ",
                        args[i + 1]
                    )));
                }
                speck = shell::dec(&args[i + 1])
                    .unwrap_or(0)
                    .to_string()
                    .into_bytes();
                i += 1;
            }
            b"-h" | b"--help" => return crate::args::usage("check"),
            b"--" => {
                scen = args[i + 1..].to_vec();
                break;
            }
            b"-b" | b"--box" | b"--new" | b"--interactive" | b"-i" => {
                return Err(ctx.die(cat!(
                    "check run: the scenario gets a box of its own (no ",
                    x,
                    ")"
                )));
            }
            b"--clean" => {}
            b"--idle" => {
                optarg(ctx, x, v)?;
                match idle_duration(&args[i + 1], "--idle ") {
                    Ok(_) => {}
                    Err(2) => return Err(Exit(2)),
                    Err(_) => {
                        return Err(ctx.die(cat!(
                            "check run: --idle takes a duration like 30m or 2h, or 0 (never), got ",
                            args[i + 1]
                        )));
                    }
                }
                upopts.push(x.clone());
                upopts.push(args[i + 1].clone());
                i += 1;
            }
            b"--size" | b"--plugin" | b"--net" | b"--allow" | b"--addr" | b"--from"
            | b"--hyprland" | b"--quickshell" | b"--omarchy" | b"--seed" | b"--no-seed"
            | b"--monitor" | b"--theme-dir" | b"--theme" | b"--keep" | b"--env" | b"--overlay"
            | b"--ro-bind" => {
                optarg(ctx, x, v)?;
                upopts.push(x.clone());
                upopts.push(args[i + 1].clone());
                i += 1;
            }
            b"--xwayland" | b"--autoreload" | b"--no-shell" | b"--systemd" | b"--audio"
            | b"--polkit" | b"--stock-bar" | b"--no-pointer" => upopts.push(x.clone()),
            b"--desk-shell" => {
                return Err(ctx.die(
                    "check run: a check's box is clean (--keep bar keeps your bar), not your desk's shell",
                ));
            }
            y if y.starts_with(b"-") => return Err(unknown_opt(ctx, "check run", x)),
            y => {
                return Err(ctx.die(cat!(
                    "check run: the scenario goes after -- (omabox check run [OPTIONS] -- SCENARIO...), got ",
                    y
                )));
            }
        }
        i += 1;
    }
    if scen.is_empty() {
        return Err(
            ctx.die("check run: which scenario? omabox check run [OPTIONS] -- SCENARIO [ARG...]")
        );
    }
    if !runs.is_empty() && !approve {
        return Err(ctx.die("check run: --runs goes with --approve (a check is one run)"));
    }
    if json && approve {
        return Err(ctx.die(
            "check run: --json reports a check against its baseline, not --approve (whose baseline.png is for looking at)",
        ));
    }
    ctx.parsed();
    if crate::jail::in_jail() {
        return Err(ctx.die(
            "check run: not inside ai-jail yet (it starts boxes of its own and reads their shots on the host)",
        ));
    }
    need_magick(ctx)?;
    if !crate::exec::executable(b"/usr/bin/python3") {
        return Err(ctx.die("check: needs /usr/bin/python3 (package python)"));
    }
    let scen_text = scen.join(&b' ');
    // No baseline: exit 4, its own (1 is "changed").
    if !approve && !has_png(&base) {
        say(cat!(
            "check run: no baseline in ",
            base,
            ": omabox check run --approve [--base DIR] -- ",
            scen_text,
            " makes one"
        ));
        if json {
            let o = Args::new(&["-nc"])
                .kv("--arg", "b", &base)
                .arg(r#"{result: "no_baseline", base: $b}"#)
                .run(None, jq::Err::Inherit)
                .out;
            let _ = out(&o);
        }
        return Err(Exit(4));
    }
    // A check's --only names checkpoints of its baseline: one that names none (a typo) is said
    // before a run is spent on it. (--approve may name a new one.)
    if !approve {
        let ids = base_ids(&base);
        for p in &opts.only {
            if !ids.iter().any(|id| glob_match(p, id)) {
                return Err(ctx.die(cat!(
                    "check run: --only ",
                    p,
                    " names no checkpoint of the baseline in ",
                    base,
                    " (it has: ",
                    ids.join(&b' '),
                    "; a new one: --approve --only ID)"
                )));
            }
        }
    }
    if outd.is_empty() {
        let state = shell::env("XDG_STATE_HOME")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.local/state"));
        let date = shell::date("%Y%m%d-%H%M%S");
        outd = cat!(
            state,
            "/omabox/check/",
            date,
            "-",
            std::process::id().to_string()
        );
    }
    mkdirs(ctx, &outd)?;
    enter(ctx, &outd)?;
    let outd = logical(&outd);
    let nruns: usize = if approve {
        if runs.is_empty() {
            3
        } else {
            usize::try_from(shell::dec(&runs).unwrap_or(3)).unwrap_or(3)
        }
    } else {
        1
    };
    let boxes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let bx = boxes.clone();
    // Every box made so far taken down however this ends (bash's EXIT trap): on INT, TERM or HUP
    // once the step in progress is over (its up, its scenario: bounded, trap.rs), unless a terminal's
    // Ctrl-C was taken by the scenario and it went on.
    let trap = crate::trap::on_exit(move || {
        let all = bx.lock().map(|v| v.clone()).unwrap_or_default();
        for b in all {
            let _ = crate::sig::omabox_cmd()
                .arg("down")
                .arg(OsStr::from_bytes(&b))
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
    let prefix = shell::env("OMABOX_CHECK_NAME")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| cat!("check-", std::process::id().to_string()));
    let root_bin = shell::bytes(&crate::root().join("bin")).to_vec();
    if !opts.only.is_empty() {
        say(cat!(
            "check: --only ",
            opts.only.join(&b','),
            ": the other checkpoints are skipped (check point does not take them; `omabox check wants ID` tells the scenario)"
        ));
    }
    for k in 1..=nruns {
        let bname = cat!(prefix, "-", k.to_string());
        say(cat!(
            "check: run ",
            k.to_string(),
            " of ",
            nruns.to_string(),
            ": a clean box (",
            bname,
            "), then: ",
            scen_text
        ));
        if let Ok(mut b) = boxes.lock() {
            b.push(bname.clone());
        }
        let t0 = std::time::Instant::now();
        let uplog = cat!(outd, "/run", k.to_string(), ".up.log");
        let mut c = crate::sig::omabox_cmd();
        c.env("OMABOX_SESSION", "")
            .arg("up")
            .arg(OsStr::from_bytes(&bname))
            .args(["--clean", "--idle", "30m"])
            .args(upopts.iter().map(|a| OsStr::from_bytes(a)))
            .stdout(Stdio::null());
        let rc = match std::fs::File::create(shell::path(&uplog)) {
            Ok(f) => {
                c.stderr(Stdio::from(f));
                crate::sig::run_status(c).unwrap_or(127)
            }
            Err(e) => {
                say(cat!("check: ", crate::shots::cant_write(&uplog, &e)));
                1
            }
        };
        if rc != 0 {
            return Err(ctx.die(cat!(
                "check: the box for run ",
                k.to_string(),
                " did not start: ",
                last_line(&uplog),
                " (",
                uplog,
                ")"
            )));
        }
        say(cat!(
            "check: run ",
            k.to_string(),
            " of ",
            nruns.to_string(),
            ": box up in ",
            t0.elapsed().as_secs().to_string(),
            " s"
        ));
        if k == 1 {
            let e = check_env(&bname);
            put(&cat!(outd, "/env.json"), &e)?;
        }
        let rund = cat!(outd, "/run", k.to_string());
        mkdirs(ctx, &rund)?;
        // The scenario's `omabox` is this one (its bin first), driving this run's box ($OMABOX).
        let path = cat!(root_bin, ":", shell::env_or_empty("PATH"));
        let runlog = cat!(outd, "/run", k.to_string(), ".log");
        // (a Ctrl-C from the terminal the scenario outlives lets the run go on, as before:
        // sig::takes_ctrl_c)
        let only = opts.only.join(&b',');
        let rc = crate::sig::takes_ctrl_c(|| scenario(&scen, &path, &bname, &rund, &runlog, &only));
        let mut c = crate::sig::omabox_cmd();
        c.arg("down")
            .arg(OsStr::from_bytes(&bname))
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = crate::sig::run_status(c);
        if rc != 0 {
            say(cat!(
                "check: the scenario failed in run ",
                k.to_string(),
                " (exit ",
                rc.to_string(),
                "): ",
                last_line(&runlog),
                " (",
                runlog,
                ")"
            ));
            if json {
                let o = Args::new(&["-nc"])
                    .kv("--argjson", "rc", rc.to_string())
                    .kv("--arg", "log", &runlog)
                    .arg(r#"{result: "failed", exit: $rc, log: $log}"#)
                    .run(None, jq::Err::Inherit)
                    .out;
                let _ = out(&o);
            }
            return Err(Exit(3));
        }
        if check_ids(&rund).is_empty() {
            let msg = if opts.only.is_empty() {
                b"the scenario took no checkpoint (omabox check point ID in it)".to_vec()
            } else {
                cat!(
                    "the scenario took none of the checkpoints --only names (",
                    only,
                    ")"
                )
            };
            say(cat!("check: ", msg));
            if json {
                let o = Args::new(&["-nc"])
                    .kv("--arg", "log", &runlog)
                    .kv("--arg", "m", &msg)
                    .arg(r#"{result: "failed", exit: 0, log: $log, message: ($m | sub(" \\(omabox check point ID in it\\)$"; ""))}"#)
                    .run(None, jq::Err::Inherit)
                    .out;
                let _ = out(&o);
            }
            return Err(Exit(3));
        }
    }
    trap.forget();
    let spk = if speck.is_empty() {
        None
    } else {
        Some(ai(&speck))
    };
    if approve {
        return check_approve(ctx, &outd, &base, spk, nruns, &opts);
    }
    if json {
        check_compare_json(ctx, &outd, &base, spk, &opts)
    } else {
        check_compare(ctx, &outd, &base, spk, None, &opts).map(|_| ())
    }
}

/// The out folder entered (`$(cd "$out" && pwd)`), or why not said and the command ended (failed:
/// bash's were cd's words and 1, check's "changed": findings 441, 442).
fn enter(ctx: &Ctx, d: &[u8]) -> Res<()> {
    if shell::path(d).is_dir() {
        return Ok(());
    }
    let why = match std::fs::metadata(shell::path(d)) {
        Ok(_) => "Not a directory".to_string(),
        Err(e) => crate::shots::why(&e),
    };
    Err(failed(ctx, cat!("cannot enter ", d, ": ", why)))
}

/// The scenario, as bash's `PATH=... OMABOX=BOX OMABOX_SESSION="" OMABOX_CHECK_OUT=DIR SCEN... >
/// LOG 2>&1`: its status (127 when there is none such, 126 when it cannot run). ONLY, --only's
/// patterns comma-separated, is its OMABOX_CHECK_ONLY (#376; none set when empty).
fn scenario(
    scen: &[Vec<u8>],
    path: &[u8],
    bname: &[u8],
    rund: &[u8],
    log: &[u8],
    only: &[u8],
) -> i32 {
    let f = match std::fs::File::create(shell::path(log)) {
        Ok(f) => f,
        Err(e) => {
            say(cat!("check: ", crate::shots::cant_write(log, &e)));
            return 1;
        }
    };
    let Ok(f2) = f.try_clone() else { return 1 };
    // (Why it cannot run, in omabox's words, where its stderr goes: the run's log. Bash's were its
    // own, "bash: X: bad interpreter: ...": finding 441. The codes are bash's: 127 none such, 126
    // one that cannot run.)
    let prog = &scen[0];
    let said = |m: Vec<u8>, rc: i32| -> i32 {
        let mut e = &f2;
        let _ = e.write_all(&cat!(
            "omabox: check: cannot run scenario ",
            prog,
            ": ",
            m,
            "\n"
        ));
        rc
    };
    // (bash looks the scenario up in the PATH it is given: the first executable file, else the
    // first file there, which it then cannot run)
    let exe: Vec<u8> = if prog.contains(&b'/') {
        prog.clone()
    } else {
        let cands: Vec<Vec<u8>> = path
            .split(|&c| c == b':')
            .map(|d| cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", prog))
            .filter(|p| shell::path(p).is_file())
            .collect();
        match cands
            .iter()
            .find(|p| crate::exec::executable(p))
            .or(cands.first())
        {
            Some(p) => p.clone(),
            None => return said(b"not found in PATH".to_vec(), 127),
        }
    };
    let pth = shell::path(&exe);
    match std::fs::metadata(&pth) {
        Err(_) => return said(b"no such file".to_vec(), 127),
        Ok(m) if m.is_dir() => return said(b"it is a directory".to_vec(), 126),
        Ok(_) if !crate::exec::executable(&exe) => {
            return said(b"it is not executable (chmod +x)".to_vec(), 126);
        }
        Ok(_) => {}
    }
    // The kernel runs it or says why: execve here in the child (std's execvp would hand a file it
    // cannot run to /bin/sh, which bash never does), then bash's own ways with what it said.
    let mut envv: Vec<(Vec<u8>, Vec<u8>)> = std::env::vars_os()
        .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
        .filter(|(k, _)| {
            !matches!(
                k.as_slice(),
                b"PATH"
                    | b"OMABOX"
                    | b"OMABOX_SESSION"
                    | b"OMABOX_CHECK_OUT"
                    | b"OMABOX_CHECK_ONLY"
            )
        })
        .collect();
    envv.extend([
        (b"PATH".to_vec(), path.to_vec()),
        (b"OMABOX".to_vec(), bname.to_vec()),
        (b"OMABOX_SESSION".to_vec(), Vec::new()),
        (b"OMABOX_CHECK_OUT".to_vec(), rund.to_vec()),
    ]);
    if !only.is_empty() {
        envv.push((b"OMABOX_CHECK_ONLY".to_vec(), only.to_vec()));
    }
    let envp: Vec<std::ffi::CString> = envv
        .into_iter()
        .filter_map(|(k, v)| std::ffi::CString::new(cat!(k, "=", v)).ok())
        .collect();
    let mut argv = vec![prog.clone()];
    argv.extend(scen[1..].iter().cloned());
    match exec_in_child(&exe, &argv, &envp, &f2) {
        Ok(rc) => rc,
        Err(e) => match e.raw_os_error() {
            Some(libc::ENOEXEC) => {
                let mut head = [0u8; 256];
                let n = std::fs::File::open(&pth)
                    .and_then(|mut h| std::io::Read::read(&mut h, &mut head))
                    .unwrap_or(0);
                let line = head[..n].split(|&b| b == b'\n').next().unwrap_or_default();
                if line.contains(&0) {
                    return said(
                        b"it is a binary this system cannot run (Exec format error)".to_vec(),
                        126,
                    );
                }
                // (No #!: a script run by bash, $0 its path, as bash ran it: callers rely on it.)
                let mut av = vec![b"bash".to_vec(), exe.clone()];
                av.extend(scen[1..].iter().cloned());
                match exec_in_child(b"/usr/bin/bash", &av, &envp, &f2) {
                    Ok(rc) => rc,
                    Err(e) => said(cat!("bash: ", crate::shots::why(&e)), 126),
                }
            }
            Some(errno) => {
                // (E2BIG, ENOMEM: the words alone, #! or not)
                let plain = matches!(errno, libc::E2BIG | libc::ENOMEM);
                match interp_of(&pth).filter(|_| !plain) {
                    Some(name) => {
                        let what = if errno == libc::ENOENT {
                            "is not there".to_string()
                        } else {
                            format!("cannot run: {}", crate::shots::why(&e))
                        };
                        let crlf = if crate::exit::contains(&name, b"^M") {
                            " (CRLF line ends?)"
                        } else {
                            ""
                        };
                        said(cat!("its #! interpreter '", name, "' ", what, crlf), 126)
                    }
                    // (an ELF whose loader is not there)
                    None if errno == libc::ENOENT => said(
                        b"a file it needs to start (its ELF loader) is not there".to_vec(),
                        127,
                    ),
                    None => said(crate::shots::why(&e).into_bytes(), 126),
                }
            }
            None => said(crate::shots::why(&e).into_bytes(), 126),
        },
    }
}

/// A `#!` file's interpreter as the kernel reads it: its first line (256 bytes at most), blanks
/// after the #! skipped, up to a blank or newline; each CR in it shown ^M (any other byte as it is).
/// None for a file with no #!.
fn interp_of(p: &std::path::Path) -> Option<Vec<u8>> {
    let mut head = [0u8; 256];
    let n = std::fs::File::open(p)
        .and_then(|mut h| std::io::Read::read(&mut h, &mut head))
        .unwrap_or(0);
    let rest = head[..n].strip_prefix(b"#!")?;
    let st = rest
        .iter()
        .position(|&c| c != b' ' && c != b'\t')
        .unwrap_or(rest.len());
    let rest = &rest[st..];
    let end = rest
        .iter()
        .position(|&c| matches!(c, b' ' | b'\t' | b'\n'))
        .unwrap_or(rest.len());
    let mut name = Vec::new();
    for &c in &rest[..end] {
        if c == b'\r' {
            name.extend_from_slice(b"^M");
        } else {
            name.push(c);
        }
    }
    Some(name)
}

/// PROGRAM run with ARGV and ENVP, its stdout and stderr OUT, by execve in the child: its status,
/// or the errno the kernel refused it with.
fn exec_in_child(
    program: &[u8],
    argv: &[Vec<u8>],
    envp: &[std::ffi::CString],
    out: &std::fs::File,
) -> std::io::Result<i32> {
    let prog = std::ffi::CString::new(program.to_vec())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::ENOENT))?;
    let cargv: Vec<std::ffi::CString> = argv
        .iter()
        .filter_map(|a| std::ffi::CString::new(a.clone()).ok())
        .collect();
    // (as addresses: the closure must be Send; they point into prog, cargv and envp, alive until
    // the spawn below has returned, and the child has its own copy of them)
    let mut pa: Vec<usize> = cargv.iter().map(|c| c.as_ptr() as usize).collect();
    pa.push(0);
    let mut pe: Vec<usize> = envp.iter().map(|c| c.as_ptr() as usize).collect();
    pe.push(0);
    let pp = prog.as_ptr() as usize;
    let mut c = crate::sig::command(OsStr::from_bytes(program));
    c.stdout(Stdio::from(out.try_clone()?));
    c.stderr(Stdio::from(out.try_clone()?));
    // SAFETY: execve is async-signal-safe; its arguments are NUL-ended arrays of pointers to
    // NUL-ended strings that outlive the spawn; it returns only when the kernel refused.
    unsafe {
        c.pre_exec(move || {
            libc::execve(
                pp as *const libc::c_char,
                pa.as_ptr().cast(),
                pe.as_ptr().cast(),
            );
            Err(std::io::Error::last_os_error())
        });
    }
    // (a foreground child, sig::run_child: a group Ctrl-C that kills it is the foreground's)
    let r = crate::sig::run_status(c);
    drop((prog, cargv));
    r
}

use std::os::unix::process::CommandExt;

/// idle_duration VALUE HOW: up --idle's reading, whole seconds (rounded up); Err(2) for a bare
/// number (said, refused since 1.0), Err(1) for anything else that is no duration.
pub fn idle_duration(v: &[u8], how: &str) -> Result<i64, i32> {
    if shell::re_match("^0+(\\.0+)?(ms|s|m|h)?$", v) {
        return Ok(0);
    }
    if shell::re_match("^[0-9]{1,6}$", v) {
        say(cat!(
            how,
            v,
            " is refused since 1.0 (a bare number was minutes there, and is seconds everywhere else): write ",
            how,
            shell::dec(v).unwrap_or(0).to_string(),
            "m"
        ));
        return Err(2);
    }
    if shell::re_match("^[0-9.]+$", v) {
        return Err(1);
    }
    crate::num::ms_duration(v)
        .map(|ms| (ms + 999) / 1000)
        .ok_or(1)
}

// --- the comparisons ---

/// chk_point_json ID RESULT AS HOLES MARKED APPROVED STILL: one checkpoint of check run --json, a
/// line of JSONL (when there is one).
#[allow(clippy::too_many_arguments)]
fn point_json(
    jsonl: Option<&[u8]>,
    c: &Chk,
    id: &[u8],
    res: &str,
    as_: &[u8],
    holes: &[u8],
    marked: &[u8],
    ok: i64,
    still: &str,
) {
    let Some(f) = jsonl else { return };
    let none = matches!(res, "new" | "missing");
    let e: &[u8] = b"";
    let o = chk_json(
        &[
            ("id", id),
            ("res", res.as_bytes()),
            ("as", as_),
            ("holes", holes),
            ("marked", marked),
            ("ok", ok.to_string().as_bytes()),
            ("still", still.as_bytes()),
            ("size", if none { e } else { &c.size }),
            ("regs", if none { e } else { &c.regions }),
            ("tree", if none { e } else { &c.tree }),
            ("sp", if none { e } else { &c.specks }),
        ],
        CHK_POINT_JQ,
    );
    append(f, &o);
}

/// chk_still POINT: false when a checkpoint's .point says it was not still when taken.
fn chk_still(p: &[u8]) -> &'static str {
    if file_lines(p).iter().any(|l| l == b"settled no") {
        "false"
    } else {
        "true"
    }
}

fn check_compare_json(
    ctx: &Ctx,
    outd: &[u8],
    base: &[u8],
    speck: Option<i64>,
    opts: &Opts,
) -> Res<()> {
    let jsonl = cat!(outd, "/.check.jsonl");
    put(&jsonl, b"")?;
    let (crc, sum) = {
        // (a #[test] hearing out's lines keeps fd 1, which its other threads share)
        #[cfg(test)]
        let _o = (!crate::exit::hearing_out()).then(|| crate::exec::Redir::null(1));
        #[cfg(not(test))]
        let _o = crate::exec::Redir::null(1);
        match check_compare(ctx, outd, base, speck, Some(&jsonl), opts) {
            Ok(s) => (0, s),
            Err(Exit(1)) => (1, LAST_SUM.with(|s| s.borrow().clone())),
            Err(e) => return Err(e),
        }
    };
    let o = Args::new(&["-sc"])
        .kv("--arg", "sum", &sum)
        .kv("--argjson", "rc", crc.to_string())
        .arg(format!("{CHK_JQ}{CHK_RUN_JQ}"))
        .arg(&jsonl)
        .run_file(jq::Err::Inherit)
        .out;
    out(&o)?;
    rm(&jsonl);
    if crc == 0 { Ok(()) } else { Err(Exit(1)) }
}

thread_local! {
    /// CHK_SUM of the last check_compare, for check run --json when it ends "changed".
    static LAST_SUM: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// check_compare OUT BASE SPECK: a run against the baseline: same, changed (regions, tree), new,
/// missing; Err(1) unless all same. Its CHK_SUM is the Ok's (and LAST_SUM's). With --only, the
/// baseline's checkpoints it does not name are skipped (counted, not missing: #376).
fn check_compare(
    ctx: &Ctx,
    outd: &[u8],
    base: &[u8],
    speck: Option<i64>,
    jsonl: Option<&[u8]>,
    opts: &Opts,
) -> Res<Vec<u8>> {
    let run1 = cat!(outd, "/run1");
    let plan = cat!(outd, "/report.plan");
    let body = cat!(plan, ".body");
    put(&body, b"")?;
    let (mut fail, mut same, mut n, mut changed, mut nholed, mut new, mut missing, mut alike) = (
        false, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize,
    );
    let mut setup = Vec::new();
    let mut firsts: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut skipped = 0usize;
    for id in check_ids(&run1) {
        if !wanted(&opts.only, &id) {
            continue;
        }
        n += 1;
        let np = cat!(run1, "/", id, ".point");
        if !shell::path(&cat!(base, "/", id, ".png")).is_file() {
            out(cat!(
                "new      ",
                id,
                ": no baseline (omabox check run --approve adds it)\n"
            ))?;
            fail = true;
            new += 1;
            point_json(jsonl, &Chk::default(), &id, "new", b"", b"", b"", 0, "");
            continue;
        }
        let ps = match speck {
            Some(s) => s,
            None => {
                let mut v = None;
                for l in file_lines(&cat!(base, "/", id, ".point")) {
                    if let Some(d) = l.strip_prefix(b"speck ") {
                        if shell::re_match("^[0-9]{1,6}$", d) {
                            v = shell::dec(d);
                            break;
                        }
                    }
                }
                v.unwrap_or(CHECK_SPECK)
            }
        };
        let c = check_one(base, &run1, &id, ps, &cat!(base, "/", id, ".unstable"))
            .ok_or_else(|| ctx.die(cat!("check: could not compare ", id, " (magick)")))?;
        // Holes the baseline does not have fail it even where the shots match.
        let holes = check_new_holes(&cat!(base, "/", id, ".point"), &np);
        let old = count_starting(&np, b"hole ") as i64 - holes.len() as i64;
        let what = hole_what(&np);
        let spa = shell::words(&c.specks);
        let mut sp = Vec::new();
        let s0 = spa.first().cloned().unwrap_or(b"0".to_vec());
        if s0 != b"0" {
            sp = cat!(
                s0,
                " speck",
                if s0 == b"1" { "" } else { "s" },
                " left out, the largest ",
                spa.get(2).cloned().unwrap_or(b"?".to_vec()),
                " px"
            );
        }
        let hole_text: Vec<u8> = holes.join(&b'\n');
        let okn = old.max(0);
        if c.size.is_empty() && c.regions.is_empty() && c.tree.is_empty() && holes.is_empty() {
            same += 1;
            let tail = if sp.is_empty() {
                Vec::new()
            } else {
                cat!(" (", sp, ")")
            };
            out(cat!("same     ", id, tail, "\n"))?;
            if old > 0 {
                out(cat!(
                    "  note: ",
                    old.to_string(),
                    " hole",
                    if old == 1 { "" } else { "s" },
                    " as in the baseline (",
                    what,
                    " there: approved)\n"
                ))?;
            }
            point_json(jsonl, &c, &id, "same", b"", b"", b"", okn, chk_still(&np));
            continue;
        }
        fail = true;
        let anychange = !(c.size.is_empty() && c.regions.is_empty() && c.tree.is_empty());
        if anychange {
            changed += 1;
        } else {
            nholed += 1;
        }
        let (mut bt, mut nt) = (Vec::new(), Vec::new());
        let (btf, ntf) = (
            cat!(base, "/", id, ".tree.json"),
            cat!(run1, "/", id, ".tree.json"),
        );
        if shell::path(&btf).is_file() && shell::path(&ntf).is_file() {
            bt = btf;
            nt = ntf;
        }
        let regs: Vec<Vec<u8>> = if c.regions.is_empty() {
            Vec::new()
        } else {
            lines(&c.regions).iter().map(|l| l.to_vec()).collect()
        };
        let key = change_key(&c, base, &run1, &id, &bt, &nt, &holes);
        let png1 = cat!(run1, "/", id, ".png");
        let diffp = cat!(outd, "/", id, ".diff.png");
        let res_word = if anychange { "changed" } else { "holes" };
        if let Some(first) = firsts.get(&key).cloned() {
            alike += 1;
            append(&body, &cat!("also\t", first, "\t", id, "\n"));
            let nr = regs.len();
            if !c.size.is_empty() {
                out(cat!(
                    "changed  ",
                    id,
                    ": as ",
                    first,
                    " (size ",
                    c.size,
                    ")\n"
                ))?;
            } else if !c.tree.is_empty() {
                out(cat!(
                    "changed  ",
                    id,
                    ": as ",
                    first,
                    " (its tree lines; ",
                    nr.to_string(),
                    " region",
                    s_unless1(nr),
                    ", all on what they name)\n"
                ))?;
            } else {
                out(cat!(
                    "changed  ",
                    id,
                    ": as ",
                    first,
                    " (its ",
                    nr.to_string(),
                    " region",
                    s_unless1(nr),
                    if holes.is_empty() { "" } else { " and holes" },
                    ")\n"
                ))?;
            }
            let mut all = regs.clone();
            all.extend(holes.iter().cloned());
            let mut mk = Vec::new();
            if c.size.is_empty() && !all.is_empty() && check_mark(&png1, &diffp, &all) {
                mk = diffp.clone();
            }
            if file_lines(&np).iter().any(|l| l == b"settled no") {
                out(cat!("  note: ", id, " was not still when taken\n"))?;
            }
            point_json(
                jsonl,
                &c,
                &id,
                res_word,
                &first,
                &hole_text,
                &mk,
                okn,
                chk_still(&np),
            );
            continue;
        }
        firsts.insert(key, id.clone());
        let unst = cat!(base, "/", id, ".unstable");
        let unst_col = if shell::path(&unst).is_file() {
            unst.clone()
        } else {
            Vec::new()
        };
        append(
            &body,
            &cat!(
                "point\t", id, "\t", base, "/", id, ".png\t", png1, "\t", bt, "\t", nt, "\t",
                unst_col, "\n"
            ),
        );
        if !c.size.is_empty() {
            append(&body, &cat!("size\t", c.size, "\n"));
            out(cat!("changed  ", id, ": size ", c.size, "\n"))?;
        } else if c.regions.is_empty() && c.tree.is_empty() {
            out(cat!(
                "holes    ",
                id,
                ": ",
                what,
                " at ",
                holes.len().to_string(),
                " place",
                s_unless1(holes.len()),
                " the baseline does not have (marked: ",
                diffp,
                ")\n"
            ))?;
        } else {
            let marked = if regs.len() + holes.len() == 0 {
                Vec::new()
            } else {
                cat!(" (marked: ", diffp, ")")
            };
            out(cat!(
                "changed  ",
                id,
                ": ",
                regs.len().to_string(),
                " region",
                s_unless1(regs.len()),
                marked,
                "\n"
            ))?;
            for r in &regs {
                out(cat!("  at ", pct_sp(r), " (", hash2_sp(r), " px)\n"))?;
                if let Some(m) = mask_of(r) {
                    append(&body, &cat!("region\t", tabs(&m), "\n"));
                }
            }
        }
        for r in &holes {
            out(cat!(
                "  hole: ",
                what,
                " at ",
                pct_sp(r),
                " (",
                hash2_sp(r),
                " px)\n"
            ))?;
            if c.size.is_empty() {
                if let Some(m) = mask_of(r) {
                    append(&body, &cat!("region\t", tabs(&m), "\n"));
                }
            }
        }
        if old > 0 {
            out(cat!(
                "  note: ",
                old.to_string(),
                " hole",
                if old == 1 { "" } else { "s" },
                " as in the baseline (approved)\n"
            ))?;
        }
        let mut all = regs.clone();
        all.extend(holes.iter().cloned());
        let mut mk = Vec::new();
        if c.size.is_empty() && !all.is_empty() && check_mark(&png1, &diffp, &all) {
            mk = diffp.clone();
        }
        if !c.tree.is_empty() {
            for r in lines(&c.tree) {
                out(cat!("  tree: ", r, "\n"))?;
            }
        }
        if !sp.is_empty() {
            out(cat!("  ", sp, " (parts under ", ps.to_string(), " px)\n"))?;
        }
        if file_lines(&np).iter().any(|l| l == b"settled no") {
            out(cat!("  note: ", id, " was not still when taken\n"))?;
        }
        point_json(
            jsonl,
            &c,
            &id,
            res_word,
            b"",
            &hole_text,
            &mk,
            okn,
            chk_still(&np),
        );
    }
    for id in base_ids(base) {
        if !wanted(&opts.only, &id) {
            skipped += 1;
            continue;
        }
        if !shell::path(&cat!(run1, "/", id, ".point")).is_file() {
            out(cat!(
                "missing  ",
                id,
                ": in the baseline, not taken this run\n"
            ))?;
            fail = true;
            missing += 1;
            point_json(jsonl, &Chk::default(), &id, "missing", b"", b"", b"", 0, "");
        }
    }
    let (mut rep, mut rscaled) = (Vec::new(), Vec::new());
    if changed + nholed > 0 {
        let mut p = format!(
            "title\tcheck: {} of {n} changed. Each pair: the baseline, then this run; red: pixels that changed, blue: only the tree\n",
            changed + nholed
        )
        .into_bytes();
        p.extend(std::fs::read(shell::path(&body)).unwrap_or_default());
        put(&plan, &p)?;
        let rp = cat!(outd, "/report.png");
        match check_report(&plan, &rp, "sheet") {
            Some(left) => {
                rep = rp.clone();
                out(cat!(
                    "report:  ",
                    rp,
                    " (every change beside its baseline in one image: look at it first)\n"
                ))?;
                if !left.is_empty() {
                    out(cat!(
                        "  not in it, no room: ",
                        left,
                        " (their changes are listed above)\n"
                    ))?;
                }
            }
            None => say(
                "check: the report could not be drawn (magick); each change is marked in its ID.diff.png",
            ),
        }
        rscaled = people_copy(&plan, &rp, "sheet", opts.scale, "report", "check", &|v| {
            out(v)
        });
    }
    rm(&plan);
    rm(&body);
    // A baseline from another omabox, Hyprland, Omarchy or GPU: differences may be that.
    let cj = cat!(base, "/check.json");
    if shell::path(&cj).is_file() {
        // (`e=$(jq -rn --slurpfile ...)`)
        let r = jq::tool(
            "jq",
            &Args::new(&["-rn"])
                .kv("--slurpfile", "a", &cj)
                .kv("--slurpfile", "b", &cat!(outd, "/env.json"))
                .arg(CHK_SETUP_JQ)
                .into_words(),
            None,
            jq::Err::Null,
        );
        let e = if r.ok() { r.text() } else { Vec::new() };
        if !e.is_empty() {
            out(cat!(
                "note: the baseline was made with another setup (",
                e,
                "): a difference may be that\n"
            ))?;
            setup = e;
        }
    }
    let mut e = String::new();
    if changed > 0 {
        e += &format!(", {changed} changed");
    }
    if alike > 0 {
        e += &format!(" ({alike} as another)");
    }
    if nholed > 0 {
        e += &format!(", {nholed} with new holes");
    }
    if new > 0 {
        e += &format!(", {new} new");
    }
    if missing > 0 {
        e += &format!(", {missing} missing");
    }
    if skipped > 0 {
        e += &format!(", {skipped} skipped (--only)");
    }
    out(cat!(
        "check: ",
        same.to_string(),
        " of ",
        (n + missing).to_string(),
        " same",
        e,
        "; shots in ",
        run1,
        "\n"
    ))?;
    let mut sum = Vec::new();
    for v in [
        base.to_vec(),
        run1.clone(),
        rep,
        setup,
        (n + missing).to_string().into_bytes(),
        same.to_string().into_bytes(),
        changed.to_string().into_bytes(),
        alike.to_string().into_bytes(),
        nholed.to_string().into_bytes(),
        new.to_string().into_bytes(),
        missing.to_string().into_bytes(),
        skipped.to_string().into_bytes(),
        rscaled,
        opts.only.join(&b','),
    ] {
        sum.extend(v);
        sum.push(b'\t');
    }
    LAST_SUM.with(|s| *s.borrow_mut() = sum.clone());
    if fail { Err(Exit(1)) } else { Ok(sum) }
}

/// check_approve OUT BASE SPECK RUNS: the runs made the baseline. With --only (#376), the checkpoints
/// it names are replaced (or added), every other one kept as it was.
fn check_approve(
    ctx: &Ctx,
    outd: &[u8],
    base: &[u8],
    speck: Option<i64>,
    runs: usize,
    opts: &Opts,
) -> Res<()> {
    let cs = speck.unwrap_or(CHECK_SPECK);
    let plan = cat!(outd, "/baseline.plan");
    let body = cat!(plan, ".body");
    let run = |k: usize| cat!(outd, "/run", k.to_string());
    mkdirs(ctx, base)?;
    put(&body, b"")?;
    let ids1 = check_ids(&run(1));
    for k in 2..=runs {
        for id in &ids1 {
            if !shell::path(&cat!(run(k), "/", id, ".point")).is_file() {
                return Err(ctx.die(cat!(
                    "check: run ",
                    k.to_string(),
                    " took no ",
                    id,
                    ": the scenario must take the same checkpoints each time"
                )));
            }
        }
        for id in check_ids(&run(k)) {
            if !shell::path(&cat!(run(1), "/", id, ".point")).is_file() {
                return Err(ctx.die(cat!(
                    "check: run 1 took no ",
                    id,
                    ": the scenario must take the same checkpoints each time"
                )));
            }
        }
    }
    // A checkpoint the scenario no longer takes leaves the baseline (only the files a check writes);
    // one --only does not name is kept as it was.
    let mut kept = 0usize;
    let mut wspeck: HashMap<Vec<u8>, i64> = HashMap::new();
    for id in base_ids(base) {
        if !wanted(&opts.only, &id) {
            kept += 1;
            // (its window's speck stands for the checkpoints of it approved now)
            let pl = file_lines(&cat!(base, "/", id, ".point"));
            let win = pl.first().and_then(|l| l.strip_prefix(b"of window "));
            let sp = pl
                .iter()
                .filter_map(|l| l.strip_prefix(b"speck "))
                .find(|d| shell::re_match("^[0-9]{1,6}$", d))
                .and_then(shell::dec);
            if let (Some(l), Some(sp), None) = (win, sp, speck) {
                let e = wspeck.entry(l.to_vec()).or_insert(0);
                *e = (*e).max(sp);
            }
            continue;
        }
        if shell::path(&cat!(run(1), "/", id, ".point")).is_file() {
            continue;
        }
        for ext in [".png", ".tree.json", ".point", ".unstable"] {
            rm(&cat!(base, "/", id, ext));
        }
        out(cat!(
            "dropped  ",
            id,
            " (the scenario no longer takes it)\n"
        ))?;
    }
    let mut n = 0usize;
    let mut unst = 0usize;
    let mut holed: Vec<Vec<u8>> = Vec::new();
    let mut hoff = Vec::new();
    let mut pspeck: HashMap<Vec<u8>, i64> = HashMap::new();
    let mut wof: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let cmp_fail = |id: &[u8]| ctx.die(cat!("check: could not compare ", id, "'s runs (magick)"));
    for id in check_ids(&run(1)) {
        n += 1;
        // Which runs match which: the baseline is the run with the most matches.
        let (mut best, mut most) = (1usize, -1i64);
        for k in 1..=runs {
            let mut agree = 0i64;
            for j in 1..=runs {
                if j == k {
                    continue;
                }
                let c = check_one(&run(k), &run(j), &id, cs, b"").ok_or_else(|| cmp_fail(&id))?;
                if c.size.is_empty() && c.regions.is_empty() && c.tree.is_empty() {
                    agree += 1;
                }
            }
            if agree > most {
                best = k;
                most = agree;
            }
        }
        let uf = cat!(outd, "/", id, ".unstable");
        put(&uf, b"")?;
        let mut odd: Vec<usize> = Vec::new();
        for j in 1..=runs {
            if j == best {
                continue;
            }
            let c = check_one(&run(best), &run(j), &id, cs, b"").ok_or_else(|| cmp_fail(&id))?;
            if !c.size.is_empty() {
                return Err(ctx.die(cat!(
                    "check: ",
                    id,
                    " is ",
                    c.size,
                    " between runs ",
                    best.to_string(),
                    " and ",
                    j.to_string(),
                    ": a window that is not the same size each time cannot have a baseline"
                )));
            }
            if c.regions.is_empty() && c.tree.is_empty() {
                continue;
            }
            odd.push(j);
            let mut add = Vec::new();
            if !c.regions.is_empty() {
                for l in cut12(&c.regions) {
                    add.extend_from_slice(&cat!(l, "\n"));
                }
            }
            if !c.tree.is_empty() {
                let jp = cat!(run(j), "/", id, ".png");
                let tm = tree_masks(
                    &cat!(run(j), "/", id, ".point"),
                    &identify("%wx%h", &jp).unwrap_or_default(),
                );
                let mut a = w(&[
                    b"tree-keys".as_slice(),
                    &cat!(run(best), "/", id, ".tree.json"),
                    &cat!(run(j), "/", id, ".tree.json"),
                    b"",
                ]);
                a.extend(tm);
                let r = py(&a, None, false);
                for l in r.out.split_inclusive(|&ch| ch == b'\n') {
                    add.extend_from_slice(b"tree ");
                    add.extend_from_slice(l);
                }
            }
            append(&uf, &add);
        }
        if run_tool(
            "sort",
            &[b"-u".to_vec(), b"-o".to_vec(), uf.clone(), uf.clone()],
        ) != 0
        {
            return Err(failed(ctx, cat!("cannot sort ", uf)));
        }
        // The noise: what is left between the runs outside the unstable regions, all specks.
        let mut big = 0i64;
        for j in 1..=runs {
            if j == best {
                continue;
            }
            let c = check_one(&run(best), &run(j), &id, cs, &uf).ok_or_else(|| cmp_fail(&id))?;
            let last = ai(hash2_sp(&c.specks));
            if last > big {
                big = last;
            }
        }
        let ps = match speck {
            Some(s) => s,
            None => {
                let mut ps = 2 * big;
                if big == 0 {
                    ps = CHECK_SPECK_AGREED;
                } else if ps < CHECK_SPECK_MIN {
                    ps = CHECK_SPECK_MIN;
                }
                ps.min(CHECK_SPECK)
            }
        };
        let rb = run(best);
        for ext in [".png", ".point"] {
            copy(ctx, &cat!(rb, "/", id, ext), &cat!(base, "/", id, ext))?;
        }
        pspeck.insert(id.clone(), ps);
        let bpoint = cat!(base, "/", id, ".point");
        let first = file_lines(&bpoint).first().cloned().unwrap_or_default();
        if let Some(l) = first.strip_prefix(b"of window ") {
            let l = l.to_vec();
            wof.insert(id.clone(), l.clone());
            if wspeck.get(&l).copied().unwrap_or(0) < ps {
                wspeck.insert(l, ps);
            }
        }
        let rbt = cat!(rb, "/", id, ".tree.json");
        if shell::path(&rbt).is_file() {
            copy(ctx, &rbt, &cat!(base, "/", id, ".tree.json"))?;
        } else {
            rm(&cat!(base, "/", id, ".tree.json"));
        }
        let given = speck.is_some();
        let unst_data = std::fs::read(shell::path(&uf)).unwrap_or_default();
        // (its --ignore regions are left out too: counted with the unstable ones, #342)
        if !unst_data.is_empty() || has_line_starting(&bpoint, b"ignore ") {
            unst += 1;
        }
        if !unst_data.is_empty() {
            copy(ctx, &uf, &cat!(base, "/", shell::basename(&uf)))?;
            if odd.len() + 1 < runs {
                let odds: Vec<String> = odd.iter().map(ToString::to_string).collect();
                out(cat!(
                    "unstable ",
                    id,
                    ": run ",
                    odds.join(" "),
                    " of ",
                    runs.to_string(),
                    " differed from the others: an app that is not the same from one start to the next, or a time that changed between runs (left out from now on; that run: ",
                    outd,
                    "/run",
                    odd.first().map(ToString::to_string).unwrap_or_default(),
                    "/",
                    id,
                    ".png):\n"
                ))?;
            } else {
                out(cat!(
                    "unstable ",
                    id,
                    ": every run differed: data or a time the scenario does not pin (left out from now on):\n"
                ))?;
            }
            for l in file_lines(&uf) {
                out(cat!("  ", l, "\n"))?;
            }
            out(cat!("  ", speck_text(ps, big, given), "\n"))?;
        } else {
            rm(&cat!(base, "/", id, ".unstable"));
            out(cat!(
                "baseline ",
                id,
                " (",
                speck_text(ps, big, given),
                ")\n"
            ))?;
        }
        let rpoint = cat!(rb, "/", id, ".point");
        if file_lines(&rpoint).iter().any(|l| l == b"settled no") {
            out(cat!("  note: ", id, " was not still when taken\n"))?;
        }
        // Holes go into the baseline as they are (a check fails only on new ones): said here.
        if has_line_starting(&rpoint, b"hole ") {
            holed.push(id.clone());
            let what = hole_what(&rpoint);
            for (l, r) in word_rest(&rpoint) {
                if l == b"hole" {
                    out(cat!(
                        "holes    ",
                        id,
                        ": ",
                        what,
                        " at ",
                        pct_sp(&r),
                        " (",
                        hash2_sp(&r),
                        " px)\n"
                    ))?;
                }
            }
        }
        if hoff.is_empty() {
            for l in file_lines(&rpoint) {
                if let Some(h) = l.strip_prefix(b"holes off: ") {
                    if !hoff.is_empty() {
                        hoff.push(b'\n');
                    }
                    hoff.extend_from_slice(h);
                }
            }
        }
        // The overview's card: the checkpoint as kept, what is left out of it, its holes.
        let what = file_lines(&bpoint)
            .first()
            .and_then(|l| l.strip_prefix(b"of ").map(<[u8]>::to_vec))
            .unwrap_or_default();
        let bun = cat!(base, "/", id, ".unstable");
        let l = if shell::path(&bun).is_file() {
            file_lines(&bun)
                .iter()
                .filter(|x| !x.starts_with(b"tree "))
                .count()
        } else {
            0
        };
        let r = count_starting(&bpoint, b"hole ");
        let lt = if l == 0 {
            String::new()
        } else {
            format!(", {l} unstable (amber)")
        };
        let rt = if r == 0 {
            String::new()
        } else {
            format!(", {r} hole{} (red)", if r == 1 { "" } else { "s" })
        };
        append(
            &body,
            &cat!(
                "point\t", id, "\t", base, "/", id, ".png\t", what, lt, rt, "\n"
            ),
        );
        if shell::path(&bun).is_file() {
            for x in file_lines(&bun) {
                if let Some(m) = mask_of(&x) {
                    append(&body, &cat!("unstable\t", tabs(&m), "\n"));
                }
            }
        }
        for (x, r) in word_rest(&bpoint) {
            if x != b"hole" {
                continue;
            }
            if let Some(m) = mask_of(&r) {
                append(&body, &cat!("hole\t", tabs(&m), "\n"));
            }
        }
    }
    // Each checkpoint's speck in its .point, a window's largest for each of its checkpoints.
    for id in check_ids(&run(1)) {
        let mut ps = pspeck.get(&id).copied().unwrap_or(0);
        if speck.is_none() {
            if let Some(l) = wof.get(&id) {
                let wsp = wspeck.get(l).copied().unwrap_or(0);
                if wsp > ps {
                    ps = wsp;
                    out(cat!(
                        "speck    ",
                        id,
                        ": under ",
                        ps.to_string(),
                        " px, as the other checkpoints of its window (the app's noise; its own runs differed by less)\n"
                    ))?;
                }
            }
        }
        append(
            &cat!(base, "/", id, ".point"),
            format!("speck {ps}\n").as_bytes(),
        );
    }
    // The setup the baseline was made with. A part approved with --only on another setup than the
    // rest's keeps the rest's (said): the next checks say it, which is true of the rest.
    let cj = cat!(base, "/check.json");
    let mut other_setup = Vec::new();
    if kept > 0 && shell::path(&cj).is_file() {
        let r = jq::tool(
            "jq",
            &Args::new(&["-rn"])
                .kv("--slurpfile", "a", &cj)
                .kv("--slurpfile", "b", &cat!(outd, "/env.json"))
                .arg(CHK_SETUP_JQ)
                .into_words(),
            None,
            jq::Err::Null,
        );
        if r.ok() {
            other_setup = r.text();
        }
    }
    if other_setup.is_empty() {
        copy(ctx, &cat!(outd, "/env.json"), &cj)?;
    }
    if opts.only.is_empty() {
        out(cat!(
            "check: baseline of ",
            n.to_string(),
            " checkpoints in ",
            base,
            " (",
            unst.to_string(),
            " with regions left out); runs in ",
            outd,
            "\n"
        ))?;
    } else {
        out(cat!(
            "check: ",
            n.to_string(),
            " checkpoint",
            s_unless1(n),
            " approved in ",
            base,
            " (--only ",
            opts.only.join(&b','),
            "; ",
            unst.to_string(),
            " with regions left out), ",
            kept.to_string(),
            " other",
            s_unless1(kept),
            " kept as ",
            if kept == 1 { "it was" } else { "they were" },
            "; runs in ",
            outd,
            "\n"
        ))?;
    }
    if !other_setup.is_empty() {
        out(cat!(
            "note: the baseline's other checkpoints were made with another setup (",
            other_setup,
            "): its check.json stays theirs; --approve without --only brings them all to this one\n"
        ))?;
    }
    // The baseline in one image (finding 363): a person or an agent looks before keeping it. With
    // --only, the checkpoints approved now.
    let mut p = if opts.only.is_empty() {
        cat!(
            "title\tthe baseline in ",
            base,
            ", each checkpoint as a check will compare it; amber: unstable, left out; red: holes\n"
        )
    } else {
        cat!(
            "title\tthe checkpoints approved in ",
            base,
            " (--only ",
            opts.only.join(&b','),
            "; the others kept), each as a check will compare it; amber: unstable, left out; red: holes\n"
        )
    };
    p.extend(std::fs::read(shell::path(&body)).unwrap_or_default());
    put(&plan, &p)?;
    let bpng = cat!(outd, "/baseline.png");
    match check_report(&plan, &bpng, "overview") {
        Some(pages) if pages.is_empty() => {
            out(cat!(
                "overview: ",
                bpng,
                " (the baseline's ",
                n.to_string(),
                " checkpoints in one image)\n"
            ))?;
            out(
                "check: look at it before you keep this baseline: what most runs showed is kept, a wrong screen too (an error, a page not loaded yet)\n",
            )?;
        }
        Some(pages) => {
            // Too many to read in one look (#256): pages, each one look.
            let pl = lines(&pages);
            let joined: Vec<u8> = pl.iter().flat_map(|l| cat!(l, " ")).collect();
            out(cat!(
                "overview: ",
                bpng,
                " and ",
                pl.len().to_string(),
                " more pages: ",
                joined,
                "(the baseline's ",
                n.to_string(),
                " checkpoints, one look a page)\n"
            ))?;
            out(
                "check: look at them before you keep this baseline: what most runs showed is kept, a wrong screen too (an error, a page not loaded yet)\n",
            )?;
        }
        None => say(cat!(
            "check: the baseline's image could not be drawn (magick): look at its shots in ",
            base,
            " before you keep it"
        )),
    }
    people_copy(
        &plan,
        &bpng,
        "overview",
        opts.scale,
        "overview",
        "check",
        &|v| out(v),
    );
    rm(&plan);
    rm(&body);
    if !hoff.is_empty() {
        out(cat!(
            "note: no holes looked for on the screen: ",
            hoff,
            "\n"
        ))?;
    }
    if !holed.is_empty() {
        out(cat!(
            "check: HOLES in ",
            holed.len().to_string(),
            " of ",
            n.to_string(),
            " checkpoints (",
            holed.join(&b' '),
            "): see-through inside a window, the bar or a popup (a transparent part, a gap, a surface that did not draw). Approved as they are, a check fails only on new ones: look at them (the shots in ",
            base,
            "), fix them and approve again, unless that is how it should look.\n"
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit;

    // A SCENARIO is a program: one named like a shell builtin or a function of bash's omabox
    // (`say`, `shopt`) that names no program in PATH is not there, 127, said in the run's log;
    // one that names a program there runs it (finding 451: bash ran its builtin or function).
    #[test]
    fn a_scenario_is_a_program_whatever_its_name() {
        let d = std::env::temp_dir().join(format!("omabox-scen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let empty = shell::bytes(&d).to_vec();
        let log = cat!(empty, "/log");
        for name in ["say", "shopt", "true"] {
            let rc = scenario(&[name.as_bytes().to_vec()], &empty, b"b", &empty, &log, b"");
            assert_eq!(rc, 127, "{name}");
            assert_eq!(
                std::fs::read(shell::path(&log)).unwrap(),
                cat!(
                    "omabox: check: cannot run scenario ",
                    name,
                    ": not found in PATH\n"
                )
            );
        }
        for (name, want) in [("true", 0), ("false", 1)] {
            let rc = scenario(
                &[name.as_bytes().to_vec()],
                b"/usr/bin",
                b"b",
                &empty,
                &log,
                b"",
            );
            assert_eq!(rc, want, "{name}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ignore_regions_in_shot_pixels() {
        // (a 200x100 window shot at scale 2, its origin 10,20)
        assert_eq!(
            ignore_px([10, 20, 200, 100, 400, 200, 0, 0, 30, 30]),
            Some(b"0 0 40 20".to_vec())
        );
        assert_eq!(ignore_px([0, 0, 100, 100, 100, 100, 200, 200, 5, 5]), None);
        assert_eq!(
            ignore_px([0, 0, 3, 3, 10, 10, 1, 1, 1, 1]),
            Some(b"3 3 4 4".to_vec())
        );
    }

    #[test]
    fn words_cut_as_bash() {
        assert_eq!(pct_sp(b"a b c"), b"a b");
        assert_eq!(hash2_sp(b"a b c"), b"c");
        assert_eq!(hash_sp(b"a b c"), b"b c");
        assert_eq!(pct_sp(b"abc"), b"abc");
        assert_eq!(
            split_specks(b"1,2 3x4 5\nspecks 2 10 7"),
            (b"1,2 3x4 5".to_vec(), b"2 10 7".to_vec())
        );
        assert_eq!(
            split_specks(b"specks 0 0 0"),
            (Vec::new(), b"0 0 0".to_vec())
        );
        assert_eq!(
            cut12(b"1,2 3x4 5\nx"),
            vec![b"1,2 3x4".to_vec(), b"x".to_vec()]
        );
        assert_eq!(mask_of(b"-1,2 3x4 5"), Some(b"-1 2 3 4".to_vec()));
    }

    #[test]
    fn python_not_run_said() {
        let e = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(
            py_not_run(&e),
            (
                127,
                b"omabox: check: cannot run /usr/bin/python3: No such file or directory\n".to_vec()
            )
        );
        let e = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(py_not_run(&e).0, 126);
    }

    // A folder that is a file, or not there: said, 2 (check's own failure), not 1 ("changed").
    #[test]
    fn out_folder_not_entered() {
        let mut ctx = Ctx::new(b"check", b"/x".to_vec());
        ctx.die_rc = Some(2);
        ctx.parsed();
        let f = std::env::temp_dir().join(format!("omabox-enter-{}", std::process::id()));
        let _ = std::fs::write(&f, b"");
        assert_eq!(enter(&ctx, shell::bytes(&f)), Err(Exit(2)));
        let _ = std::fs::remove_file(&f);
        assert_eq!(enter(&ctx, shell::bytes(&f)), Err(Exit(2)));
        assert_eq!(enter(&ctx, b"/"), Ok(()));
    }

    #[test]
    fn interpreters_as_the_kernel_reads_them() {
        let d = std::env::temp_dir().join(format!("omabox-interp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let t = |body: &[u8]| {
            let f = d.join("s");
            let _ = std::fs::write(&f, body);
            interp_of(&f)
        };
        assert_eq!(t(b"#!/bin/sh\r\nx\n"), Some(b"/bin/sh^M".to_vec()));
        // (whole: bash's 128-byte sample less its last byte cut it: finding 441)
        assert_eq!(
            t(b"#!/nonexistent/interp"),
            Some(b"/nonexistent/interp".to_vec())
        );
        assert_eq!(t(b"#! /x -e\n"), Some(b"/x".to_vec()));
        assert_eq!(t(b"#!/bin/s\rh x\n"), Some(b"/bin/s^Mh".to_vec()));
        assert_eq!(t(b"\x7fELF"), None);
        let longer = cat!("#!/", [b'a'; 130].as_slice(), "\n");
        assert_eq!(t(&longer).map(|v| v.len()), Some(131));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ignore_px_as_bash() {
        // (t_unit_check's check_ignore_px: OX OY LW LH IW IH X Y W H)
        let t = |v| ignore_px(v).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(
            t([100, 0, 400, 26, 400, 26, 150, 5, 20, 10]).as_deref(),
            Some("50 5 20 10"),
            "a crop at 100,0 at scale 1: less its origin"
        );
        assert_eq!(
            t([100, 0, 400, 26, 800, 52, 150, 5, 20, 10]).as_deref(),
            Some("100 10 40 20"),
            "...at scale 2: twice that"
        );
        assert_eq!(
            t([0, 0, 100, 100, 150, 150, 1, 1, 3, 3]).as_deref(),
            Some("1 1 5 5"),
            "...at 1.5, rounded outwards (1.5 to 6)"
        );
        assert_eq!(
            t([100, 0, 400, 26, 400, 26, 90, -5, 20, 40]).as_deref(),
            Some("0 0 10 26"),
            "...cut to the shot"
        );
        assert_eq!(
            t([100, 0, 400, 26, 400, 26, 900, 0, 10, 10]),
            None,
            "...none of it in the shot: fails"
        );
        // ignore_cut (#342): what the shot kept of one it cut, in the coordinates given
        let c = |v| ignore_cut(v);
        assert_eq!(
            c([100, 0, 400, 26, 400, 26, 150, 5, 20, 10]),
            None,
            "inside"
        );
        assert_eq!(
            c([100, 0, 400, 26, 400, 26, 90, -5, 20, 40]).as_deref(),
            Some("10x26 at 100,0")
        );
        assert_eq!(
            c([568, 78, 760, 875, 760, 875, 230, 130, 430, 52]).as_deref(),
            Some("92x52 at 568,130"),
            "#342's: given relative to the crop"
        );
        assert_eq!(
            c([100, 0, 400, 26, 400, 26, 900, 0, 10, 10]),
            None,
            "outside"
        );
    }

    /// A scratch dir of this test's own, empty.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("omabox-check-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pb(p: &std::path::Path) -> Vec<u8> {
        shell::bytes(p).to_vec()
    }

    fn write(p: &std::path::Path, data: &str) {
        std::fs::write(p, data).unwrap();
    }

    // check_new_holes BASE NEW (t_unit_check's two cases).
    #[test]
    fn new_holes_as_bash() {
        let d = scratch("nh");
        write(
            &d.join("hb.point"),
            "of screen\nholes checked\nhole 10,10 5x5 25\n",
        );
        write(
            &d.join("hn.point"),
            "of screen\nholes checked\nhole 12,12 5x5 25\nhole 100,100 5x5 25\n",
        );
        write(&d.join("a.json"), "[{\"depth\":0}]");
        assert_eq!(
            check_new_holes(&pb(&d.join("hb.point")), &pb(&d.join("hn.point"))),
            vec![b"100,100 5x5 25".to_vec()],
            "new holes: those the baseline has none overlapping"
        );
        assert_eq!(
            check_new_holes(&pb(&d.join("a.json")), &pb(&d.join("hn.point"))),
            vec![b"12,12 5x5 25".to_vec(), b"100,100 5x5 25".to_vec()],
            "...a baseline from before holes: all of them"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // check_holes_off (t_unit_check's four cases): whether a box's screen can show holes.
    #[test]
    fn holes_off_as_bash() {
        let d = scratch("hoff");
        let bg = d.join("home/.local/state/omarchy/current/background");
        std::fs::create_dir_all(bg.parent().unwrap()).unwrap();
        let mut ctx = Ctx::new(b"check", b"/x".to_vec());
        ctx.d = pb(&d);
        let off = |json: &str| {
            write(&d.join("box.json"), json);
            String::from_utf8(check_holes_off(&ctx)).unwrap()
        };
        assert_eq!(off("{}"), "not a clean box", "holes off: not a clean box");
        assert_eq!(
            off(r#"{"clean": {"keep": ["clock", "wallpaper"]}}"#),
            "--keep wallpaper"
        );
        std::os::unix::fs::symlink("/home/sbx/.config/omabox/clean-wallpaper.png", &bg).unwrap();
        assert_eq!(
            off(r#"{"clean": {"keep": []}}"#),
            "",
            "...clean, the sentinel's wallpaper: on"
        );
        std::fs::remove_file(&bg).unwrap();
        std::os::unix::fs::symlink(
            "/home/sbx/.local/state/omarchy/current/theme/backgrounds/1.jpg",
            &bg,
        )
        .unwrap();
        assert!(
            off(r#"{"clean": {"keep": []}}"#).contains("not the sentinel"),
            "...a theme's wallpaper since"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `magick ARGS...`, which must work.
    fn mg(args: &[&std::ffi::OsStr]) {
        let st = std::process::Command::new("magick")
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "magick {args:?}");
    }

    macro_rules! os {
        ($($a:expr),* $(,)?) => { &[$(AsRef::<std::ffi::OsStr>::as_ref(&$a)),*] };
    }

    /// t_unit_check's made images: a.png (a block), specks.png (two stray pixels), new.png (a new
    /// block).
    fn images(d: &std::path::Path) {
        mg(os![
            "-size",
            "200x100",
            "xc:#202030",
            "-fill",
            "#c0c0d0",
            "-draw",
            "rectangle 20,20 80,40",
            d.join("a.png")
        ]);
        mg(os![
            d.join("a.png"),
            "-fill",
            "#c1c0d0",
            "-draw",
            "point 150,80",
            "-draw",
            "point 10,90",
            d.join("specks.png")
        ]);
        mg(os![
            d.join("a.png"),
            "-fill",
            "#c0c0d0",
            "-draw",
            "rectangle 120,20 160,40",
            d.join("new.png")
        ]);
    }

    // check_holes SHOT KIND [REGION] (finding 362; t_unit_check's seven cases).
    #[test]
    #[ignore = "runs magick and python: run by test/run.sh (cargo test -- --ignored)"]
    fn holes_as_bash() {
        let d = scratch("holes");
        images(&d);
        let p = |n: &str| pb(&d.join(n));
        mg(os![
            "-size",
            "400x300",
            "xc:#ff00ff",
            "-fill",
            "#202030",
            "-draw",
            "rectangle 10,10 190,289",
            "-draw",
            "rectangle 200,10 389,289",
            "-fill",
            "#ff00ff",
            "-draw",
            "rectangle 60,100 99,129",
            d.join("hole.png")
        ]);
        let h = |f: &str, kind: &str, within: &str| {
            String::from_utf8(check_holes(&p(f), kind, within.as_bytes()).unwrap()).unwrap()
        };
        assert_eq!(
            h("hole.png", "screen", ""),
            "60,100 40x30 1200\ncorners 0",
            "holes: one inside a window; the gap between two windows reaches the margin, open"
        );
        assert_eq!(
            h("hole.png", "screen", "50 90 30 30"),
            "10,10 20x20 400\ncorners 0",
            "...a region cutting it: its part, in the region's pixels"
        );
        assert_eq!(
            h("hole.png", "screen", "250 90 30 30"),
            "corners 0",
            "...a region beside it: none"
        );
        mg(os![
            "-size",
            "400x300",
            "xc:#ff00ff",
            "-fill",
            "#7aa2f7",
            "-draw",
            "rectangle 10,10 389,289",
            "-fill",
            "#ff00ff",
            "-draw",
            "rectangle 12,12 387,287",
            "-fill",
            "#202030",
            "-draw",
            "roundrectangle 12,12 387,287 14,14",
            d.join("round.png")
        ]);
        assert_eq!(
            h("round.png", "screen", ""),
            "corners 4",
            "...rounded content inside a window's border: its 4 corners, no hole"
        );
        mg(os![
            "-size",
            "400x300",
            "xc:#ff00ff",
            "-fill",
            "#202030",
            "-draw",
            "rectangle 10,10 389,289",
            "-fill",
            "#ff00ff",
            "-draw",
            "rectangle 380,100 399,140",
            d.join("edge.png")
        ]);
        assert_eq!(
            h("edge.png", "screen", ""),
            "corners 0",
            "...a gap that reaches the screen's edge: open wallpaper"
        );
        mg(os![
            "-size",
            "200x100",
            "xc:none",
            "-fill",
            "#202030",
            "-draw",
            "roundrectangle 0,0 199,99 12,12",
            "(",
            "-size",
            "40x20",
            "xc:none",
            ")",
            "-geometry",
            "+50+30",
            "-compose",
            "copy",
            "-composite",
            "(",
            "-size",
            "20x30",
            "xc:none",
            ")",
            "-geometry",
            "+180+40",
            "-compose",
            "copy",
            "-composite",
            d.join("win.png")
        ]);
        assert_eq!(
            h("win.png", "window", ""),
            "50,30 40x20 800\n180,40 20x30 600\ncorners 4",
            "...a window's own pixels: its transparent parts, one at its edge too; its rounded corners not"
        );
        assert_eq!(
            h("a.png", "window", ""),
            "corners 0",
            "...an opaque window: none"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    fn check_ctx() -> Ctx {
        let mut ctx = Ctx::new(b"check", b"/x".to_vec());
        ctx.die_rc = Some(2);
        ctx.parsed();
        ctx
    }

    /// Whether OUT has a line starting with LEAD.
    fn line_at(out: &str, lead: &str) -> bool {
        out.lines().any(|l| l.starts_with(lead))
    }

    /// The `^speck` line of a .point.
    fn speck_of(p: &std::path::Path) -> String {
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("speck"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    const SCREEN: &str = "of screen\nsettled yes\ntree none\nholes off: test\n";
    const ENV: &str = r#"{"gpu":"x","locale":"en_US.UTF-8","timezone":"UTC"}"#;

    // check_approve and check_compare on made runs (finding 363, #257; t_unit_check's cases).
    #[test]
    #[ignore = "runs magick and python: run by test/run.sh (cargo test -- --ignored)"]
    fn approve_and_compare_as_bash() {
        let d = scratch("ap");
        images(&d);
        let ctx = check_ctx();
        let cp = |a: &str, b: &std::path::Path| {
            std::fs::copy(d.join(a), b).unwrap();
        };
        // --approve: each checkpoint's speck is twice the largest stray part between its runs (8 when
        // they agreed to the pixel), and one image of the baseline is written.
        let ap = d.join("ap");
        for k in 1..=3 {
            let r = ap.join(format!("run{k}"));
            std::fs::create_dir_all(&r).unwrap();
            write(&r.join("p.point"), SCREEN);
            write(&r.join("q.point"), SCREEN);
            cp("a.png", &r.join("q.png"));
            cp(
                if k == 2 { "specks.png" } else { "a.png" },
                &r.join("p.png"),
            );
        }
        write(&ap.join("env.json"), ENV);
        let approve = |outd: &std::path::Path, base: &str, speck: Option<i64>| {
            exit::heard_all(|| {
                check_approve(
                    &ctx,
                    &pb(outd),
                    &pb(&d.join(base)),
                    speck,
                    3,
                    &Opts::default(),
                )
            })
        };
        let (rc, out) = approve(&ap, "apbase", None);
        assert_eq!(rc, Ok(()), "{out}");
        assert!(
            out.lines().any(
                |l| l.starts_with("baseline p (specks under 16 px left out: the runs")
                    && l.contains("largest stray part was 1 px)")
            ),
            "approve: a checkpoint whose runs differ by a pixel: specks under 16 px (twice 1, 16 at least): {out}"
        );
        assert!(
            line_at(
                &out,
                "baseline q (specks under 8 px left out: the runs agreed to the pixel)"
            ),
            "...one whose runs agreed: specks under 8 px: {out}"
        );
        assert_eq!(
            (
                speck_of(&d.join("apbase/p.point")),
                speck_of(&d.join("apbase/q.point"))
            ),
            ("speck 16".into(), "speck 8".into()),
            "...kept in its .point"
        );
        let ls: Vec<&str> = out.lines().collect();
        assert!(
            ls.windows(2).any(|w| w[0].starts_with("overview: ")
                && w[0].contains("/ap/baseline.png ")
                && w[1].starts_with("check: look at it before you keep this baseline")),
            "...the baseline's image, and to look at it: {out}"
        );
        assert!(
            std::fs::metadata(ap.join("baseline.png")).is_ok_and(|m| m.len() > 0),
            "...written"
        );
        let (_, out) = approve(&ap, "apbase2", Some(5));
        assert!(
            line_at(&out, "baseline q (specks under 5 px left out (--speck))"),
            "...--speck given: that one: {out}"
        );
        assert_eq!(
            speck_of(&d.join("apbase2/q.point")),
            "speck 5",
            "...in the .point"
        );
        // Checkpoints of one window share its largest speck (the noise is the app's).
        let aw = d.join("aw");
        for k in 1..=3 {
            let r = aw.join(format!("run{k}"));
            std::fs::create_dir_all(&r).unwrap();
            write(
                &r.join("w1.point"),
                "of window W\nsettled yes\ntree none\nholes off: test\n",
            );
            write(
                &r.join("w2.point"),
                "of window W\ncrop 0,0 50x50\nsettled yes\ntree none\nholes off: test\n",
            );
            std::fs::copy(ap.join(format!("run{k}/p.png")), r.join("w1.png")).unwrap();
            cp("a.png", &r.join("w2.png"));
        }
        write(&aw.join("env.json"), ENV);
        let (_, out) = approve(&aw, "awbase", None);
        assert!(
            line_at(
                &out,
                "speck    w2: under 16 px, as the other checkpoints of its window"
            ),
            "...a crop of a window whose runs agreed takes the window's speck, said: {out}"
        );
        assert_eq!(
            speck_of(&d.join("awbase/w2.point")),
            "speck 16",
            "...in its .point"
        );
        for k in 1..=3 {
            cp("a.png", &aw.join(format!("run{k}/w1.png")));
        }
        let (rc, _) = approve(&aw, "awbase0", None);
        assert_eq!(
            (
                speck_of(&d.join("awbase0/w1.point")),
                speck_of(&d.join("awbase0/w2.point")),
                rc
            ),
            ("speck 8".into(), "speck 8".into(), Ok(())),
            "...a window whose checkpoints all agreed: 8 for both, exit 0"
        );
        // A check with each checkpoint's speck: a 10 px part, so p (speck 16) is the same and says what
        // it left out, q (8, its runs agreed) changes; a baseline from before specks were measured: 32.
        let c = ap.join("c");
        let c1 = c.join("run1");
        std::fs::create_dir_all(&c1).unwrap();
        std::fs::copy(ap.join("run1/p.point"), c1.join("p.point")).unwrap();
        std::fs::copy(ap.join("run1/q.point"), c1.join("q.point")).unwrap();
        mg(os![
            d.join("a.png"),
            "-fill",
            "#c1c0d0",
            "-draw",
            "rectangle 150,80 154,81",
            c1.join("p.png")
        ]);
        std::fs::copy(c1.join("p.png"), c1.join("q.png")).unwrap();
        write(&c.join("env.json"), ENV);
        let base = d.join("apbase");
        let compare = |speck: Option<i64>| {
            exit::heard_all(|| {
                check_compare(&ctx, &pb(&c), &pb(&base), speck, None, &Opts::default()).map(|_| ())
            })
        };
        let (rc, out) = compare(None);
        assert!(
            out.lines()
                .any(|l| l == "same     p (1 speck left out, the largest 10 px)"),
            "check: the same, saying what it left out: {out}"
        );
        assert!(
            line_at(&out, "changed  q: 1 region"),
            "...where the runs agreed (8): changed: {out}"
        );
        assert!(
            line_at(&out, "check: 1 of 2 same, 1 changed;") && rc == Err(Exit(1)),
            "...counted, exit 1: {out}"
        );
        let qp = base.join("q.point");
        let kept: String = std::fs::read_to_string(&qp)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("speck "))
            .map(|l| format!("{l}\n"))
            .collect();
        write(&qp, &kept);
        let (_, out) = compare(None);
        assert!(
            line_at(&out, "same     q (1 speck left out, the largest 10 px)"),
            "...a baseline without a speck: 32, as before: {out}"
        );
        let (_, out) = compare(Some(0));
        assert!(
            line_at(&out, "changed  p: 1 region"),
            "...--speck 0 by hand wins: {out}"
        );
        std::fs::remove_file(c1.join("q.point")).unwrap();
        write(&c1.join("r.point"), SCREEN);
        cp("a.png", &c1.join("r.png"));
        let (rc, out) = compare(None);
        assert!(
            line_at(&out, "check: 1 of 3 same, 1 new, 1 missing;") && rc == Err(Exit(1)),
            "...new and missing counted: {out}"
        );
        write(
            &base.join("check.json"),
            r#"{"gpu":"x","locale":"pt_BR.UTF-8","timezone":"America/Sao_Paulo"}"#,
        );
        let (_, out) = compare(None);
        assert!(
            out.contains(
                "another setup (locale pt_BR.UTF-8 -> en_US.UTF-8, timezone America/Sao_Paulo -> UTC)"
            ),
            "...another locale and timezone said: {out}"
        );
        write(&base.join("check.json"), r#"{"gpu":"x"}"#);
        let (_, out) = compare(None);
        assert!(
            !out.contains("another setup"),
            "...not for a baseline from before they were recorded: {out}"
        );
        // One change in several checkpoints (#257): said once, a later one "as" the first.
        let (gb, gc) = (d.join("gbase"), d.join("gc"));
        let g1 = gc.join("run1");
        std::fs::create_dir_all(&gb).unwrap();
        std::fs::create_dir_all(&g1).unwrap();
        for id in ["s1", "s2", "s3"] {
            write(
                &gb.join(format!("{id}.point")),
                &format!("{SCREEN}speck 8\n"),
            );
            cp("a.png", &gb.join(format!("{id}.png")));
            write(&g1.join(format!("{id}.point")), SCREEN);
        }
        cp("new.png", &g1.join("s1.png"));
        cp("new.png", &g1.join("s2.png"));
        mg(os![
            d.join("a.png"),
            "-fill",
            "#c0c0d0",
            "-draw",
            "rectangle 120,60 160,80",
            g1.join("s3.png")
        ]);
        write(&gc.join("env.json"), ENV);
        write(&gb.join("check.json"), ENV);
        let (rc, out) = exit::heard_all(|| {
            check_compare(&ctx, &pb(&gc), &pb(&gb), None, None, &Opts::default()).map(|_| ())
        });
        assert!(
            out.lines()
                .any(|l| l == "changed  s2: as s1 (its 1 region)"),
            "check: one change in two checkpoints: the second said as the first: {out}"
        );
        assert!(
            !line_at(&out, "changed  s2: 1 region"),
            "...not its regions again: {out}"
        );
        assert!(
            line_at(&out, "changed  s3: 1 region"),
            "...another change: its own lines: {out}"
        );
        assert!(
            line_at(&out, "check: 0 of 3 same, 3 changed (1 as another);") && rc == Err(Exit(1)),
            "...each counted as changed, the alike said; exit 1: {out}"
        );
        assert!(
            std::fs::metadata(gc.join("s2.diff.png")).is_ok_and(|m| m.len() > 0),
            "...its diff image written all the same"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // check run --json's object (check_compare_json; t_unit_json's cases), on a baseline and a run
    // made here.
    #[test]
    #[ignore = "runs magick and python: run by test/run.sh (cargo test -- --ignored)"]
    fn compare_json_as_bash() {
        let d = scratch("json");
        mg(os!["-size", "200x100", "xc:white", d.join("a.png")]);
        mg(os![
            "-size",
            "200x100",
            "xc:white",
            "-fill",
            "black",
            "-draw",
            "rectangle 10,10 60,40",
            d.join("b.png")
        ]);
        for (f, src) in [
            ("base/p1", "a"),
            ("base/p2", "a"),
            ("base/gone", "a"),
            ("out/run1/p1", "a"),
            ("out/run1/p2", "b"),
            ("out/run1/p3", "a"),
        ] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::copy(d.join(format!("{src}.png")), p.with_extension("png")).unwrap();
        }
        for f in [
            "base/p1",
            "base/p2",
            "base/gone",
            "out/run1/p1",
            "out/run1/p2",
            "out/run1/p3",
        ] {
            write(&d.join(f).with_extension("point"), "of screen\n");
        }
        write(&d.join("out/run1/p2.point"), "of screen\nsettled no\n");
        let ctx = check_ctx();
        let (rc, out) = exit::heard_all(|| {
            check_compare_json(
                &ctx,
                &pb(&d.join("out")),
                &pb(&d.join("base")),
                None,
                &Opts::default(),
            )
        });
        let j: serde_json::Value =
            serde_json::from_str(out.lines().last().unwrap_or_default()).unwrap();
        let c = &j["counts"];
        assert_eq!(
            (
                j["result"].as_str(),
                [
                    &c["checkpoints"],
                    &c["same"],
                    &c["changed"],
                    &c["new"],
                    &c["missing"]
                ]
                .map(|v| v.as_i64()),
                rc
            ),
            (Some("changed"), [4, 1, 1, 1, 1].map(Some), Err(Exit(1))),
            "check run --json: result, counts, exit 1"
        );
        let ids: Vec<String> = j["checkpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                format!(
                    "{}:{}",
                    p["id"].as_str().unwrap(),
                    p["result"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(
            ids.join(" "),
            "p1:same p2:changed p3:new gone:missing",
            "...each checkpoint's id and result"
        );
        let p2 = &j["checkpoints"][1];
        assert_eq!(
            (
                p2["regions"].to_string(),
                p2["marked"]
                    .as_str()
                    .is_some_and(|m| m.ends_with("p2.diff.png")),
                &p2["still"]
            ),
            (
                r#"[{"x":10,"y":10,"w":51,"h":31,"pixels":1581}]"#.to_string(),
                true,
                &serde_json::Value::Bool(false)
            ),
            "...a changed one: its region, marked copy, not still"
        );
        assert_eq!(
            (
                j["base"].as_str(),
                j["shots"].as_str(),
                j["report"]
                    .as_str()
                    .is_some_and(|r| r.ends_with("report.png"))
            ),
            (
                Some(d.join("base").to_str().unwrap()),
                Some(d.join("out/run1").to_str().unwrap()),
                true
            ),
            "...base, shots, the report"
        );
        assert!(
            !d.join("out/.check.jsonl").exists(),
            "...its gathering file gone"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn idle_as_up_reads_it() {
        assert_eq!(idle_duration(b"0", "--idle "), Ok(0));
        assert_eq!(idle_duration(b"30m", "--idle "), Ok(1800));
        assert_eq!(idle_duration(b"1.5", "--idle "), Err(1));
        assert_eq!(idle_duration(b"x", "--idle "), Err(1));
    }

    // --only's patterns (#376): IDs or globs of them, comma-separated; none: every checkpoint.
    #[test]
    fn only_names_checkpoints_by_id_or_glob() {
        let v = |s: &str| s.as_bytes().to_vec();
        assert_eq!(
            only_split(b"main,settings-*"),
            Some(vec![v("main"), v("settings-*")])
        );
        assert_eq!(
            only_split(b"a,,b"),
            Some(vec![v("a"), v("b")]),
            "empty parts"
        );
        assert_eq!(only_split(b","), None, "nothing named");
        assert_eq!(only_split(b"a b"), None, "a blank");
        assert_eq!(only_split(b"../x"), None, "a path");
        let only = vec![v("main"), v("settings-*"), v("tab[12]")];
        let took: Vec<&str> = ["main", "main2", "settings-a", "settings", "tab1", "tab3"]
            .into_iter()
            .filter(|id| wanted(&only, id.as_bytes()))
            .collect();
        assert_eq!(took, ["main", "settings-a", "tab1"]);
        assert!(wanted(&[], b"anything"), "no --only: every one");
        assert_eq!(scaled_path(b"/o/report.png", 2), v("/o/report@2x.png"));
        assert_eq!(scaled_path(b"r", 3), v("r@3x.png"));
    }

    // check run --only (#376): a check of the named checkpoints, the baseline's others skipped (not
    // missing); --approve --only replaces those and keeps every other one as it was.
    #[test]
    #[ignore = "runs magick and python: run by test/run.sh (cargo test -- --ignored)"]
    fn only_checks_and_approves_the_named() {
        let d = scratch("only");
        images(&d);
        let ctx = check_ctx();
        let only = |ps: &[&str]| Opts {
            only: ps.iter().map(|p| p.as_bytes().to_vec()).collect(),
            scale: 0,
        };
        let put_point = |dir: &std::path::Path, id: &str, img: &str, point: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::copy(d.join(img), dir.join(format!("{id}.png"))).unwrap();
            write(&dir.join(format!("{id}.point")), point);
        };
        let base = d.join("base");
        for id in ["main", "set-a", "set-b"] {
            put_point(&base, id, "a.png", &format!("{SCREEN}speck 8\n"));
        }
        write(&base.join("check.json"), ENV);
        // A run that took set-a alone (its scenario skipped the rest), changed.
        let run1 = d.join("c/run1");
        put_point(&run1, "set-a", "new.png", SCREEN);
        write(&d.join("c/env.json"), ENV);
        let (rc, out) = exit::heard_all(|| {
            check_compare(
                &ctx,
                &pb(&d.join("c")),
                &pb(&base),
                None,
                None,
                &only(&["set-*"]),
            )
            .map(|_| ())
        });
        assert_eq!(rc, Err(Exit(1)), "{out}");
        assert!(line_at(&out, "changed  set-a: 1 region"), "{out}");
        assert!(
            line_at(&out, "missing  set-b"),
            "set-b named, not taken: {out}"
        );
        assert!(
            !out.contains("missing  main"),
            "main not named: skipped: {out}"
        );
        assert!(
            line_at(
                &out,
                "check: 0 of 2 same, 1 changed, 1 missing, 1 skipped (--only)"
            ),
            "{out}"
        );
        // --json: skipped counted, the patterns said.
        let (_, out) = exit::heard_all(|| {
            check_compare_json(&ctx, &pb(&d.join("c")), &pb(&base), None, &only(&["set-a"]))
        });
        let j: serde_json::Value =
            serde_json::from_str(out.lines().last().unwrap_or_default()).unwrap();
        assert_eq!(
            (
                j["counts"]["checkpoints"].as_i64(),
                j["counts"]["skipped"].as_i64(),
                j["only"].to_string(),
                j["report_scaled"].is_null()
            ),
            (Some(1), Some(2), r#"["set-a"]"#.to_string(), true),
            "{out}"
        );
        // --report-scale 2: report@2x.png beside report.png, twice its size.
        let (_, out) = exit::heard_all(|| {
            let o = Opts {
                scale: 2,
                ..only(&["set-a"])
            };
            check_compare(&ctx, &pb(&d.join("c")), &pb(&base), None, None, &o).map(|_| ())
        });
        let (r1, r2) = (d.join("c/report.png"), d.join("c/report@2x.png"));
        assert!(line_at(&out, "report at 2x: "), "{out}");
        let size = |p: &std::path::Path| crate::shots::png_size(&pb(p));
        let (Some((w1, h1)), Some((w2, h2))) = (size(&r1), size(&r2)) else {
            panic!("no reports: {out}");
        };
        assert_eq!(
            (w2, h2),
            (2 * w1, 2 * h1),
            "the same report, twice its size"
        );
        // --approve --only set-a: set-a replaced, main and set-b kept as they were (byte for byte).
        let ap = d.join("ap");
        for k in 1..=3 {
            put_point(&ap.join(format!("run{k}")), "set-a", "new.png", SCREEN);
        }
        write(&ap.join("env.json"), ENV);
        let before = std::fs::read(base.join("main.png")).unwrap();
        let (rc, out) = exit::heard_all(|| {
            check_approve(&ctx, &pb(&ap), &pb(&base), None, 3, &only(&["set-a"]))
        });
        assert_eq!(rc, Ok(()), "{out}");
        assert!(!out.contains("dropped"), "nothing dropped: {out}");
        assert!(
            line_at(&out, "check: 1 checkpoint approved in ")
                && out.contains("2 others kept as they were"),
            "{out}"
        );
        assert_eq!(
            (
                std::fs::read(base.join("main.png")).unwrap() == before,
                base.join("set-b.point").is_file(),
                std::fs::read(base.join("set-a.png")).unwrap()
                    == std::fs::read(d.join("new.png")).unwrap()
            ),
            (true, true, true),
            "main and set-b kept, set-a replaced"
        );
        // Another setup: check.json stays the rest's, said.
        write(
            &ap.join("env.json"),
            r#"{"gpu":"y","locale":"en_US.UTF-8","timezone":"UTC"}"#,
        );
        let (_, out) = exit::heard_all(|| {
            check_approve(&ctx, &pb(&ap), &pb(&base), None, 3, &only(&["set-a"]))
        });
        assert!(
            line_at(
                &out,
                "note: the baseline's other checkpoints were made with another setup (gpu x -> y)"
            ),
            "{out}"
        );
        assert_eq!(
            std::fs::read_to_string(base.join("check.json")).unwrap(),
            ENV
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
