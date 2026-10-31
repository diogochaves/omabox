//! What every shot leaves and reads (finding 81): $D/shots.tsv (what each shot is of, for `--in`),
//! the last whole frame of each thing shot for --changed ($D/changed, finding 243), where an image
//! goes (shot_path, a jailed caller's fd: fd_file), the shot-fit setting (#188), a region cut to the
//! screen or window (#240), and what ImageMagick says two frames differ in.

use crate::cat;
use crate::exit::{Ctx, Res, say};
use crate::jail::in_jail;
use crate::num::{awk_num, awk_str, fixed};
use crate::screen::stamp;
use crate::shell::{self, strip_nl};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// `$(dirname -- "$V")`, as coreutils has it.
pub fn dirname(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let mut end = p.len();
    while end > 1 && p[end - 1] == b'/' {
        end -= 1;
    }
    match p[..end].iter().rposition(|&c| c == b'/') {
        None => b".".to_vec(),
        Some(i) => {
            let mut j = i;
            while j > 0 && p[j - 1] == b'/' {
                j -= 1;
            }
            if j == 0 {
                b"/".to_vec()
            } else {
                p[..j].to_vec()
            }
        }
    }
}

/// `mkdir -p -- DIR`: its own words on stderr when it cannot.
pub fn mkdir_p(dir: &[u8]) -> bool {
    let mut c = crate::sig::command("mkdir");
    c.arg("-p")
        .arg("--")
        .arg(std::ffi::OsStr::from_bytes(dir))
        .stdin(Stdio::null());
    crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
}

/// `readlink -f -- V`, empty when it fails (`[ "$X" = "$(readlink -f -- "$V")" ]`).
pub fn real(v: &[u8]) -> Vec<u8> {
    crate::repo::readlink_f(v).unwrap_or_default()
}

/// `X=$(readlink -f -- "$V")`: the path, or the command ended, 1, saying why (bash's set -e ended
/// it there unsaid: finding 442).
pub fn real_or_end(ctx: &crate::exit::Ctx, v: &[u8]) -> Res<Vec<u8>> {
    crate::repo::readlink_f(v).ok_or_else(|| {
        ctx.die(cat!(
            ctx.label(),
            "cannot resolve ",
            v,
            " (a folder on its way is missing or cannot be looked into, or a link loops)"
        ))
    })
}

/// `mv -f -- SRC DST` as coreutils' mv: into DST when it is a directory (a link to one too), else
/// over it; mv's own words on stderr when it cannot (it is run for that).
pub fn mv_f(src: &[u8], dst: &[u8]) -> bool {
    let d = shell::path(dst);
    let to = if d.is_dir() {
        cat!(dst, "/", shell::basename(src))
    } else {
        dst.to_vec()
    };
    if std::fs::rename(shell::path(src), shell::path(&to)).is_ok() {
        return true;
    }
    let mut c = crate::sig::command("mv");
    c.args(["-f", "--"])
        .arg(std::ffi::OsStr::from_bytes(src))
        .arg(std::ffi::OsStr::from_bytes(dst))
        .stdin(Stdio::null());
    crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
}

/// "cannot write PATH: WHY": a file omabox could not write, in its words (bash's were its redirection
/// line's, "PATH: WHY" after its script and line: finding 441).
pub fn cant_write(p: &[u8], e: &std::io::Error) -> Vec<u8> {
    cat!("cannot write ", p, ": ", why(e))
}

/// strerror's words for an error, as bash says them after a redirection that failed.
pub fn why(e: &std::io::Error) -> String {
    let code = e.raw_os_error().unwrap_or(libc::EIO);
    // SAFETY: strerror's string for a valid errno, read at once.
    unsafe {
        std::ffi::CStr::from_ptr(libc::strerror(code))
            .to_string_lossy()
            .into_owned()
    }
}

/// The first bytes of every PNG.
const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// What omabox writes over at a path its caller named (#338, finding 621): nothing there, or a
/// regular file, never a link, that is empty or holds a PNG (an image of its own, a `mktemp`'s
/// file). The refusal's words for anything else: a text file of the user's is never overwritten
/// by a pre-approved shot.
pub fn overwrite_refusal(p: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let path = shell::path(p);
    // (a path that cannot be looked at cannot be written either: its own words come where it is)
    let Ok(m) = std::fs::symlink_metadata(&path) else {
        return None;
    };
    if m.file_type().is_symlink() {
        return Some(cat!(
            p,
            " is a link: omabox writes through none (name the file itself, or a new path)"
        ));
    }
    if !m.is_file() {
        return Some(cat!(p, " is there and is not a file (name a new path)"));
    }
    if m.len() == 0 {
        return None;
    }
    let mut head = [0u8; 8];
    let png = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == PNG_MAGIC;
    (!png).then(|| {
        cat!(
            p,
            " is there and is not a PNG: omabox writes over no file but an image (name a new path, or remove it first)"
        )
    })
}

/// A file omabox writes at a path its caller named: made or truncated, never through a link there.
pub fn create_nofollow(p: &[u8]) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(shell::path(p))
}

/// shot_path OUT: OUT's directory made, then OUT (resolved) written once and removed as OUT.part, so
/// a path that cannot be written stops here, saying why (issue #63). OUT resolved, its FILE. OUT and
/// OUT.part are written over only as overwrite_refusal lets them be (#338).
pub fn shot_path(ctx: &Ctx, out: &[u8]) -> Res<(Vec<u8>, Vec<u8>)> {
    if let Some(m) = overwrite_refusal(out) {
        return Err(ctx.die(cat!("shot: ", m)));
    }
    let dir = dirname(out);
    if !mkdir_p(&dir) {
        return Err(ctx.die(cat!("shot: cannot make the directory ", dir)));
    }
    let out = real_or_end(ctx, out)?;
    let file = cat!(out, ".part");
    if let Some(m) = overwrite_refusal(&out).or_else(|| overwrite_refusal(&file)) {
        return Err(ctx.die(cat!("shot: ", m)));
    }
    if let Err(e) = create_nofollow(&file) {
        return Err(ctx.die(cat!("shot: cannot write ", out, ": ", why(&e))));
    }
    let _ = std::fs::remove_file(shell::path(&file));
    Ok((out, file))
}

/// stat -f's names of the filesystems fd_file takes (ext4 says ext2/ext3). No network filesystem: a
/// hard mount whose server is gone blocks a read for ever.
const FD_FS: &str = "ext2/ext3 btrfs xfs tmpfs f2fs zfs overlayfs bcachefs ramfs jfs reiserfs nilfs2 exfat vfat msdos ntfs ntfs3";

/// fd_file N r|w WHAT: fd N (3-9), a jailed caller's file its omabox opened and passed, checked
/// without opening anything: a regular file, open for reading or for writing as WHAT uses it, on a
/// filesystem of files on a disk or in memory (FD_FS).
pub fn fd_file(ctx: &Ctx, n: &[u8], write: bool, what: &str) -> Res<()> {
    if !shell::re_match("^[3-9]$", n) {
        return Err(ctx.die(cat!(what, ": an fd is 3 to 9, got ", n)));
    }
    let dev = cat!("/dev/fd/", n);
    // (`[ -f ]` as bash's test has it: a mode of no type, an eventfd's, is a regular file too)
    let f_ok = std::fs::metadata(shell::path(&dev)).is_ok_and(|m| {
        use std::os::unix::fs::MetadataExt;
        m.is_file() || m.mode() & libc::S_IFMT == 0
    });
    if !f_ok {
        return Err(ctx.die(cat!(what, ": fd ", n, " is not a file")));
    }
    let info = std::fs::read(shell::path(&cat!("/proc/self/fdinfo/", n))).unwrap_or_default();
    let fl = info
        .split(|&c| c == b'\n')
        .find(|l| l.starts_with(b"flags:"))
        .map(|l| shell::words(l).get(1).cloned().unwrap_or_default())
        .unwrap_or_default();
    if !shell::re_match("^[0-7]{1,11}$", &fl) {
        return Err(ctx.die(cat!(what, ": fd ", n, " cannot be looked at")));
    }
    let f = i64::from_str_radix(&String::from_utf8_lossy(&fl), 8).unwrap_or(0);
    if f & 0o10_000_000 != 0 {
        return Err(ctx.die(cat!(
            what,
            ": fd ",
            n,
            " is O_PATH, not open for reading or writing"
        )));
    }
    match (write, f & 3) {
        (false, 0 | 2) | (true, 1 | 2) => {}
        (false, _) => return Err(ctx.die(cat!(what, ": fd ", n, " is not open for reading"))),
        (true, _) => return Err(ctx.die(cat!(what, ": fd ", n, " is not open for writing"))),
    }
    let mut c = crate::sig::command("stat");
    c.args(["-f", "-c", "%T"])
        .arg(std::ffi::OsStr::from_bytes(&dev))
        .stderr(Stdio::null());
    // (`t=$(stat ...)`)
    let t = crate::sig::run_output(c)
        .ok()
        .filter(|(rc, _)| *rc == 0)
        .map(|(_, out)| strip_nl(&out));
    let Some(t) = t else {
        return Err(ctx.die(cat!(what, ": fd ", n, ": cannot tell its filesystem")));
    };
    if !FD_FS.split(' ').any(|k| k.as_bytes() == t.as_slice()) {
        return Err(ctx.die(cat!(
            what,
            ": fd ",
            n,
            " is not a file on a disk or in memory (its filesystem: ",
            t,
            ")"
        )));
    }
    Ok(())
}

/// conf_check shot-fit V: its normal form (off, or a longest side), or None.
pub fn fit_check(v: &[u8]) -> Option<Vec<u8>> {
    match v {
        b"off" | b"0" => Some(b"off".to_vec()),
        _ => shell::re_match("^[1-9][0-9]{1,4}$", v).then(|| v.to_vec()),
    }
}

/// conf_get shot-fit: the config's last `shot-fit = V` (blanks around taken off), off when none; one
/// that does not check out is said and ignored.
fn fit_conf() -> Vec<u8> {
    crate::conf::conf_get("shot-fit", fit_check, b"off")
}

/// shot_fit_default (#188): the --fit a shot takes when it is given none: $OMABOX_SHOT_FIT (a jailed
/// caller's, sent by its omabox), else the shot-fit setting; empty when off. A bad $OMABOX_SHOT_FIT
/// is said and refused (Err), not ignored.
pub fn shot_fit_default() -> Result<Vec<u8>, ()> {
    let e = if in_jail() {
        shell::env_or_empty("OMABOX_RELAY_SHOT_FIT")
    } else {
        shell::env_or_empty("OMABOX_SHOT_FIT")
    };
    let v = if e.is_empty() {
        fit_conf()
    } else {
        match fit_check(&e) {
            Some(v) => v,
            None => {
                say(cat!(
                    "shot: OMABOX_SHOT_FIT is off, 0 or the longest side in pixels (10-99999), got ",
                    e
                ));
                return Err(());
            }
        }
    };
    Ok(if v == b"off" { Vec::new() } else { v })
}

/// two_files CMD SO_FAR NEW: a second file named where a command takes one: refused since 1.0
/// (#287), where the last was taken.
pub fn two_files(ctx: &Ctx, cmd: &str, so_far: &[u8], new: &[u8]) -> Res<()> {
    if so_far.is_empty() {
        return Ok(());
    }
    Err(ctx.usage_die(cat!(
        cmd,
        ": two files, ",
        so_far,
        " and ",
        new,
        ": name one (refused since 1.0, where the last was taken)"
    )))
}

/// geom_clip X Y W H LW LH (#240): the region's part inside 0,0 LWxLH, and what was cut (by edge,
/// ", " between); None when nothing of it is inside.
pub fn geom_clip(r: [i64; 4], lw: i64, lh: i64) -> Option<([i64; 4], Vec<u8>)> {
    let [mut x, mut y, w, h] = r;
    let (mut x1, mut y1) = (x.saturating_add(w), y.saturating_add(h));
    let mut cut: Vec<String> = Vec::new();
    if x < 0 {
        cut.push(format!("{} px past the left edge", x.saturating_neg()));
        x = 0;
    }
    if y < 0 {
        cut.push(format!("{} px past the top edge", y.saturating_neg()));
        y = 0;
    }
    if x1 > lw {
        cut.push(format!("{} px past the right edge", x1.saturating_sub(lw)));
        x1 = lw;
    }
    if y1 > lh {
        cut.push(format!("{} px past the bottom edge", y1.saturating_sub(lh)));
        y1 = lh;
    }
    if !(x1 > x && y1 > y) {
        return None;
    }
    Some((
        [x, y, x1.saturating_sub(x), y1.saturating_sub(y)],
        cut.join(", ").into_bytes(),
    ))
}

/// png_size FILE: its width and height, from its header; why not said when it cannot read the file,
/// or the file ends before its 16th byte (in omabox's words: bash's were od's, finding 441).
pub fn png_size(f: &[u8]) -> Option<(i64, i64)> {
    let data = match std::fs::read(shell::path(f)) {
        Ok(d) => d,
        Err(e) => {
            say_raw(&cat!("omabox: cannot read ", f, ": ", why(&e), "\n"));
            return None;
        }
    };
    if data.len() < 16 {
        say_raw(&cat!("omabox: ", f, " is too short for a PNG\n"));
        return None;
    }
    let b = data.get(16..24)?;
    let n = |o: usize| {
        (i64::from(b[o]) << 24)
            | (i64::from(b[o + 1]) << 16)
            | (i64::from(b[o + 2]) << 8)
            | i64::from(b[o + 3])
    };
    Some((n(0), n(4)))
}

/// `wc -l < FILE`.
fn lines_in(p: &std::path::Path) -> usize {
    std::fs::read(p)
        .map(|d| d.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0)
}

/// `tail -n N FILE > FILE.new && mv FILE.new FILE`.
fn keep_last(p: &std::path::Path, new: &std::path::Path, n: usize) {
    let Ok(d) = std::fs::read(p) else { return };
    let mut starts = vec![0];
    for (i, &c) in d.iter().enumerate() {
        if c == b'\n' && i + 1 < d.len() {
            starts.push(i + 1);
        }
    }
    let from = if d.is_empty() {
        0
    } else {
        starts[starts.len().saturating_sub(n)]
    };
    if std::fs::write(new, &d[from..]).is_ok() {
        let _ = mv_f(shell::bytes(new), shell::bytes(p));
    }
}

/// `printf ... >> FILE` (or `>`): a write that fails ends the command, 1, as under set -e, said.
fn write_rec(p: &std::path::Path, data: &[u8], append: bool) -> Res<()> {
    let r = std::fs::OpenOptions::new()
        .append(append)
        .write(true)
        .truncate(!append)
        .create(true)
        .open(p)
        .and_then(|mut f| f.write_all(data));
    if let Err(e) = r {
        let _ =
            std::io::stderr().write_all(&cat!("omabox: ", cant_write(shell::bytes(p), &e), "\n"));
        return Err(crate::exit::Exit(1));
    }
    Ok(())
}

/// shot_record PATH MODE WHERE GEOMETRY [FILE]: what a shot is of, for `--in`: path, its size and
/// mtime, the box's mode, `window ADDRESS` or `screen X,Y`, what it covers and the image's size.
/// FILE: another name for it (a jailed caller's /dev/fd/N). The last 200 kept past 500.
pub fn shot_record(
    ctx: &Ctx,
    path: &[u8],
    mode: &[u8],
    where_: &[u8],
    geo: &[u8],
    file: &[u8],
) -> Res<()> {
    if path.iter().any(|&c| c == b'\t' || c == b'\n') {
        say("shot: a path with a tab or newline: no --in for this one");
        return Ok(());
    }
    let f = ctx.at("shots.tsv");
    let st = stamp(if file.is_empty() { path } else { file });
    write_rec(
        &f,
        &cat!(path, "\t", st, "\t", mode, "\t", where_, "\t", geo, "\n"),
        true,
    )?;
    if lines_in(&f) > 500 {
        keep_last(&f, &ctx.at("shots.tsv.new"), 200);
    }
    Ok(())
}

/// shot_ref_save FILE KEY SHOT MODE GEOMETRY POINTER: the last whole frame of each thing shot
/// ($D/changed/KEY.png, KEY.tsv: the shot, the mode, "LWxLH IWxIH", the pointer then, when in ms), for
/// a later --changed; the 8 latest kept.
pub fn shot_ref_save(ctx: &Ctx, file: &[u8], key: &[u8], v: [&[u8]; 4]) -> Res<()> {
    let [shot, mode, geo, ptr] = v;
    let dir = cat!(ctx.d, "/changed");
    if !mkdir_p(&dir) {
        return Ok(());
    }
    let tmp = shell::path(&cat!(dir, "/.", key, ".png"));
    let png = shell::path(&cat!(dir, "/", key, ".png"));
    if std::fs::copy(shell::path(file), &tmp).is_err()
        || !mv_f(shell::bytes(&tmp), shell::bytes(&png))
    {
        let _ = std::fs::remove_file(&tmp);
        return Ok(());
    }
    let tsv = shell::path(&cat!(dir, "/", key, ".tsv"));
    let line = cat!(
        shot,
        "\t",
        mode,
        "\t",
        geo,
        "\t",
        ptr,
        "\t",
        crate::settle::now_ms().to_string(),
        "\n"
    );
    write_rec(&tsv, &line, false)?;
    // The pointer of a screen shot by its path too, for a later --since it.
    if ptr != b"-" {
        let p = shell::path(&cat!(dir, "/pointers.tsv"));
        write_rec(&p, &cat!(shot, "\t", ptr, "\n"), true)?;
        if lines_in(&p) > 200 {
            keep_last(&p, &shell::path(&cat!(dir, "/pointers.new")), 100);
        }
    }
    // (find's %T@ newest first, sort -rn: ties by the line, backwards)
    // (mtime, find's line, the name)
    type Png = ((i64, i64), Vec<u8>, Vec<u8>);
    let mut pngs: Vec<Png> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(shell::path(&dir)) {
        for e in rd.flatten() {
            let n = e.file_name().as_bytes().to_vec();
            if n.first() == Some(&b'.') || !n.ends_with(b".png") {
                continue;
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                continue;
            };
            use std::os::unix::fs::MetadataExt;
            let t = (m.mtime(), m.mtime_nsec());
            let line = cat!(format!("{}.{:09}0 ", t.0, t.1), n);
            pngs.push((t, line, n));
        }
    }
    pngs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, _, n) in pngs.iter().skip(8) {
        let base = &n[..n.len() - 4];
        let _ = std::fs::remove_file(shell::path(&cat!(dir, "/", n)));
        let _ = std::fs::remove_file(shell::path(&cat!(dir, "/", base, ".tsv")));
    }
    Ok(())
}

/// A caret: a change 4 px thin and 40 long at most (in layout pixels); tools/still's THIN and
/// CARET_LONG are the same. shot --changed and wait --since/--like leave one alone out.
pub fn caret_shaped(w: i64, h: i64) -> bool {
    (w <= 4 && h <= 40) || (h <= 4 && w <= 40)
}

/// A region X,Y,W,H in layout pixels as the image's, outwards ("X,Y,W,H"), for a mask; None when the
/// layout has no size.
pub fn to_image_px(r: [i64; 4], lw: i64, lh: i64, iw: i64, ih: i64) -> Option<Vec<u8>> {
    let [x, y, w, h] = r;
    let d = |a: i64, b: i64| {
        if b == 0 {
            None
        } else {
            Some(a.saturating_div(b))
        }
    };
    let a = d(x.saturating_mul(iw), lw)?;
    let b = d(y.saturating_mul(ih), lh)?;
    let c = d(
        w.saturating_mul(iw).saturating_add(lw).saturating_sub(1),
        lw,
    )?;
    let e = d(
        h.saturating_mul(ih).saturating_add(lh).saturating_sub(1),
        lh,
    )?;
    Some(format!("{a},{b},{c},{e}").into_bytes())
}

/// `-draw "rectangle X0,Y0 X1,Y1"` for each MASK (X,Y,W,H in image pixels: `IFS=, read`).
fn mask_draws(masks: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let mut d = Vec::new();
    for m in masks {
        let f = shell::read_ifs(m, 4, b",");
        let (x, y) = (&f[0], &f[1]);
        let w = shell::dec(&f[2])?;
        let h = shell::dec(&f[3])?;
        let (xi, yi) = (shell::dec(x)?, shell::dec(y)?);
        d.push(b"-draw".to_vec());
        d.push(cat!(
            "rectangle ",
            x,
            ",",
            y,
            " ",
            xi.saturating_add(w).saturating_sub(1).to_string(),
            ",",
            yi.saturating_add(h).saturating_sub(1).to_string()
        ));
    }
    Some(d)
}

/// `magick A B -fill black MASKS... REST... 2>/dev/null`'s output; None when it failed.
fn magick_diff(a: &[u8], b: &[u8], masks: &[Vec<u8>], rest: &[&str]) -> Option<Vec<u8>> {
    let draws = mask_draws(masks)?;
    let mut c = crate::sig::command("magick");
    c.arg(std::ffi::OsStr::from_bytes(a))
        .arg(std::ffi::OsStr::from_bytes(b))
        .args(["-fill", "black"])
        .args(draws.iter().map(|d| std::ffi::OsStr::from_bytes(d)))
        .args(rest)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let o = c.output().ok()?;
    o.status.success().then_some(o.stdout)
}

/// diff_box A B [MASK]...: the box around the pixels that differ between two images of one size,
/// [X, Y, W, H] in image pixels; Some(None) when none do. A black border before the trim: %@ trims
/// the corners' colour, and a corner may have changed too (finding 243).
pub fn diff_box(a: &[u8], b: &[u8], masks: &[Vec<u8>]) -> Option<Option<[i64; 4]>> {
    let out = magick_diff(
        a,
        b,
        masks,
        &[
            "-compose",
            "difference",
            "-composite",
            "-alpha",
            "off",
            "-grayscale",
            "Brightness",
            "-threshold",
            "0",
            "-bordercolor",
            "black",
            "-border",
            "1",
            "-format",
            "%@",
            "info:",
        ],
    )?;
    let g = shell::subst(&out);
    let re = shell::Re::new(r"^([0-9]+)x([0-9]+)\+(-?[0-9]+)\+(-?[0-9]+)$")?;
    let m: Vec<i64> = (1..=4)
        .map(|i| re.group(&g, i).and_then(|v| shell::dec(&v)))
        .collect::<Option<Vec<i64>>>()?;
    if m[0] == 0 || m[1] == 0 {
        return Some(None);
    }
    Some(Some([m[2] - 1, m[3] - 1, m[0], m[1]]))
}

/// diff_parts A B [MASK]...: the separate places two images differ (32 px or more apart), [X, Y, W,
/// H] each in image pixels, to the nearest 8 px outwards (awk's numbers, as words).
pub fn diff_parts(a: &[u8], b: &[u8], masks: &[Vec<u8>]) -> Vec<[Vec<u8>; 4]> {
    let Some(out) = magick_diff(
        a,
        b,
        masks,
        &[
            "-compose",
            "difference",
            "-composite",
            "-alpha",
            "off",
            "-grayscale",
            "Brightness",
            "-threshold",
            "0",
            "-scale",
            "12.5%",
            "-threshold",
            "0",
            "-bordercolor",
            "black",
            "-border",
            "2",
            "-morphology",
            "Dilate",
            "Square:2",
            "-define",
            "connected-components:verbose=true",
            "-connected-components",
            "8",
            "null:",
        ],
    ) else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    for (k, l) in out.split(|&c| c == b'\n').enumerate() {
        let f = shell::words(l);
        if k == 0 || f.last().map(Vec::as_slice) != Some(b"gray(255)") {
            continue;
        }
        let g: Vec<&[u8]> = f
            .get(1)
            .map(|v| v.split(|&c| c == b'x' || c == b'+').collect())
            .unwrap_or_default();
        let gv = |i: usize| awk_num(g.get(i).copied().unwrap_or_default());
        let (x, y) = (gv(2), gv(3));
        let (mut w, mut h) = (gv(0) - 4.0, gv(1) - 4.0);
        if w < 1.0 {
            w = 1.0;
        }
        if h < 1.0 {
            h = 1.0;
        }
        let s = |v: f64| awk_str(v * 8.0).into_bytes();
        parts.push([s(x), s(y), s(w), s(h)]);
    }
    parts
}

/// `awk -v a=A -v b=B 'BEGIN { s = sprintf("%.2f", a / b); sub(/\.?0+$/, "", s); print s }'`: A/B
/// to 2 decimals, its trailing zeros off; gawk's fatal words for B 0 (nothing printed).
pub fn ratio(a: i64, b: i64) -> Vec<u8> {
    if b == 0 {
        say_raw(b"awk: cmd. line:1: fatal: division by zero attempted\n");
        return Vec::new();
    }
    let s = fixed(a as f64 / b as f64, 2);
    let t = s.trim_end_matches('0');
    let t = if t.len() == s.len() {
        t
    } else {
        t.strip_suffix('.').unwrap_or(t)
    };
    t.as_bytes().to_vec()
}

/// Bytes on stderr as they are.
pub fn say_raw(v: &[u8]) {
    let _ = std::io::stderr().write_all(v);
}

/// zero_note MONITORS WAYLAND_SCREEN [OTHER] (#213, finding 286): what to say of the outputs listed
/// 0x0 in a box drawing on NVIDIA (WAYLAND_SCREEN true); empty otherwise. OTHER, the first render
/// node on another driver ("renderD128 amdgpu"), looked up when None.
pub fn zero_note(mons: &[u8], wayland_screen: &[u8], other: Option<&[u8]>) -> Vec<u8> {
    if wayland_screen != b"true" {
        return Vec::new();
    }
    let r = crate::jq::jq(
        &["-r", crate::jqprog::ZERO_NOTE_JQ],
        Some(mons),
        crate::jq::Err::Null,
    );
    let z = if r.ok() { r.text() } else { Vec::new() };
    if z.is_empty() {
        return Vec::new();
    }
    let other = match other {
        Some(o) => o.to_vec(),
        None => other_gpu(b"nvidia"),
    };
    let mut m = cat!(
        z,
        " listed 0x0: this box draws on NVIDIA, where a headless output (hyprctl output create headless) gets no mode (omabox log --grep REJECTED). omabox monitor add makes one that works; "
    );
    if other.is_empty() {
        m.extend(b"for a program that makes its own, a machine with a GPU other than NVIDIA's (this one has none)");
    } else {
        let (n, d) = match other.iter().position(|&c| c == b' ') {
            Some(i) => (other[..i].to_vec(), other[i + 1..].to_vec()),
            None => (other.clone(), other.clone()),
        };
        m.extend(cat!(
            "for a program that makes its own, a box on another GPU: OMABOX_RENDER_NODE=/dev/dri/",
            n,
            " omabox up --new (",
            n,
            " is ",
            d,
            ")"
        ));
    }
    m
}

/// other_gpu DRIVER: the first usable render node (readable and writable) on another driver, as
/// "renderD128 amdgpu"; empty for none.
pub fn other_gpu(driver: &[u8]) -> Vec<u8> {
    let mut nodes: Vec<Vec<u8>> = std::fs::read_dir("/dev/dri")
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().as_bytes().to_vec())
                .filter(|n| n.starts_with(b"renderD"))
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    for n in nodes {
        let p = std::ffi::CString::new(cat!("/dev/dri/", n)).unwrap_or_default();
        // SAFETY: a valid C string; access only reads.
        if unsafe { libc::access(p.as_ptr(), libc::R_OK | libc::W_OK) } != 0 {
            continue;
        }
        let d = crate::repo::readlink_f(&cat!("/sys/class/drm/", n, "/device/driver"))
            .map(|p| shell::basename(&p))
            .unwrap_or_default();
        if d != driver {
            return cat!(n, " ", d);
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_coreutils_and_awk() {
        for (p, d) in [
            ("", "."),
            ("ab/", "."),
            ("/", "/"),
            ("//", "/"),
            ("/a/", "/"),
            ("a//b///", "a"),
            ("///a", "/"),
            ("..", "."),
            ("a/b", "a"),
        ] {
            assert_eq!(dirname(p.as_bytes()), d.as_bytes(), "{p}");
        }
        assert_eq!(ratio(1920, 960), b"2");
        assert_eq!(ratio(1920, 1280), b"1.5");
        assert_eq!(ratio(1000, 100), b"10");
        assert_eq!(ratio(1, 3), b"0.33");
        assert_eq!(ratio(1, 1000), b"0");
    }

    #[test]
    fn regions_cut() {
        assert_eq!(
            geom_clip([0, 0, 10, 10], 100, 100),
            Some(([0, 0, 10, 10], Vec::new()))
        );
        assert_eq!(
            geom_clip([-5, 95, 10, 10], 100, 100),
            Some((
                [0, 95, 5, 5],
                b"5 px past the left edge, 5 px past the bottom edge".to_vec()
            ))
        );
        assert_eq!(geom_clip([100, 0, 10, 10], 100, 100), None);
    }
}
