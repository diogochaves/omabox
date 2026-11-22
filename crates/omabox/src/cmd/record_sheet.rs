//! `record sheet` (#176, #224, #279, findings 604, 755): a contact sheet of a video, its frames
//! made in the box (rec_ff) or, with no box up, here in a sandbox of their own (host_ff); the video
//! read as a regular file, through no link in a box's folder (open_video).

use super::record::{
    Err2, REC_FF_FLOOR, SheetMore, SheetOpts, num, rec_dir, rec_max, rm, sys_cap, sys_q, w,
};
use super::record_stop::{
    FF_SECS, Ff, Tmps, box_copy, bytes_h, ff_bound, rec_ff, rec_ff_with, replace_all, size_of,
    write_in_box,
};

use crate::cat;
use crate::exec::{dup_hi, pipe};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jq;
use crate::num::ms_s;
use crate::recprog as P;
use crate::shell::{self, strip_nl};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::Ordering;
/// `$(stat -L -c %s /dev/fd/N)`.
fn fd_size(fd: &[u8]) -> Option<i64> {
    size_of(&shell::path(&cat!("/dev/fd/", fd)))
}

/// record sheet VIDEO (#176): the video copied into the box, its sheet beside it (or -o), with
/// VIDEO's marks; a jailed caller's through its fds (#224), the video at most REC_MAX bytes, the
/// marks 1 MiB.
pub fn sheet_file(ctx: &mut Ctx, so: &SheetOpts) -> Res<()> {
    let mut tmps = Tmps::new();
    let r = sheet_file_in(ctx, so, &mut tmps);
    drop(tmps);
    r
}

fn sheet_file_in(ctx: &mut Ctx, so: &SheetOpts, tmps: &mut Tmps) -> Res<()> {
    let base = so.video.strip_suffix(b".mp4").unwrap_or(&so.video).to_vec();
    let bv = format!("{}/sheet-{}.mp4", rec_dir(&ctx.d), std::process::id()).into_bytes();
    let max = rec_max();
    let (tsv, outp, tmpd, sheet);
    let mut size: i64;
    if !so.fromfd.is_empty() {
        size = fd_size(&so.fromfd).unwrap_or(-1);
        if size < 0 || size > max {
            let s = if size < 0 { 0 } else { size };
            return Err(ctx.die(cat!(
                "record sheet: the video is ",
                bytes_h(s),
                ": more than omabox copies into a box (",
                bytes_h(max),
                ")"
            )));
        }
        tmpd = crate::trap::made(|| {
            let (_, t) = shell::mktemp_args(&[b"-d", &cat!(ctx.d, "/.record-XXXXXX")]).ok()?;
            tmps.add(t.clone());
            Some(t)
        })
        .ok_or_else(|| ctx.die(cat!("record sheet: cannot write in ", ctx.d)))?;
        tsv = cat!(tmpd, "/v.marks.tsv");
        outp = cat!(tmpd, "/v.sheet.png");
        sheet = crate::shots::OutFile::in_dir(&tmpd, b"v.sheet.png", true)
            .map_err(|m| ctx.die(cat!("record sheet: ", m)))?;
        // (In 10 s: a file the caller passed is not one to wait on.)
        if !so.mfromfd.is_empty() {
            let ok = std::fs::File::create(shell::path(&tsv))
                .ok()
                .and_then(|f| {
                    let fd = dup_hi(num(&so.mfromfd) as i32).ok()?;
                    let mut c = crate::sig::command("timeout");
                    c.args(["10", "head", "-c", "1048576"])
                        .stdin(std::process::Stdio::from(fd))
                        .stdout(std::process::Stdio::from(f));
                    crate::sig::run_status(c).ok()
                })
                .is_some_and(|rc| rc == 0);
            if !ok {
                let _ = std::fs::remove_dir_all(shell::path(&tmpd));
                return Err(ctx.die("record sheet: cannot read the marks"));
            }
        }
        // Read from the fd as it is, never reopened; no more than its size said.
        let fd = dup_hi(num(&so.fromfd) as i32).map_err(|_| Exit(1))?;
        let ok = copy_in(ctx, &bv, InFrom::Fd(fd, size))? == 0;
        if !ok {
            sys_q(ctx, &w(&[b"/usr/bin/rm".as_slice(), b"-f", b"--", &bv]))?;
            let _ = std::fs::remove_dir_all(shell::path(&tmpd));
            return Err(ctx.die(cat!(
                "record sheet: could not copy the video into box '",
                ctx.name,
                "'"
            )));
        }
    } else {
        tmpd = Vec::new();
        outp = if so.out.is_empty() {
            cat!(base, ".sheet.png")
        } else {
            so.out.clone()
        };
        sheet = sheet_out(ctx, &outp)?;
        tsv = cat!(base, ".marks.tsv");
        // (a regular file, no link at its name or in a box's folder on its way, never a FIFO, and
        // no more of it than its size said, REC_MAX at most: finding 755)
        let (f, sz) = open_video(&so.video, max).map_err(|m| ctx.die(cat!("record sheet: ", m)))?;
        size = sz;
        let ok = copy_in(ctx, &bv, InFrom::Fd(OwnedFd::from(f), sz))? == 0;
        if !ok {
            sys_q(ctx, &w(&[b"/usr/bin/rm".as_slice(), b"-f", b"--", &bv]))?;
            return Err(ctx.die(cat!(
                "record sheet: could not copy ",
                so.video,
                " into box '",
                ctx.name,
                "'"
            )));
        }
    }
    // Its ffprobe's bound by its size, at 10 MiB/s, as box_copy's.
    if !(0..=999_999_999_999_999).contains(&size) {
        size = 0;
    }
    FF_SECS.store(REC_FF_FLOOR + size / 10_485_760, Ordering::SeqCst);
    let sp = record_sheet(
        ctx,
        Src::Box(&bv),
        crate::shots::open_regular(&tsv),
        &sheet,
        &so.frames,
        so.every,
        &so.fit,
        &so.more,
        &so.as_,
    );
    sys_q(ctx, &w(&[b"/usr/bin/rm".as_slice(), b"-f", b"--", &bv]))?;
    let sp = sp.ok().flatten();
    let mut rc_ok = sp.is_some();
    if !tmpd.is_empty() {
        if rc_ok {
            let wrote = std::fs::read(shell::path(&outp)).ok().and_then(|d| {
                let fd = dup_hi(num(&so.tofd) as i32).ok()?;
                let mut f = std::fs::File::from(fd);
                f.write_all(&d).ok()
            });
            if wrote.is_none() {
                rc_ok = false;
                say("record sheet: could not write the sheet into the caller's file");
            }
        }
        let _ = std::fs::remove_dir_all(shell::path(&tmpd));
    }
    match (rc_ok, sp) {
        (true, Some(sp)) => out(cat!(sp, "\n")),
        _ => Err(Exit(1)),
    }
}

/// record sheet VIDEO with no box up (#279, finding 604): the same sheet, its ffmpeg and ffprobe run
/// here in host_sandbox, reading the video from an fd this omabox opened (a jailed caller's, its
/// --from-fd); the frames, and a jailed caller's marks and sheet, in a folder of omabox's cache
/// (on disk, not /tmp's memory), gone however it ends.
/// A sheet's OUT (and OUT.part) written over only as shot's are (#338's review): nothing there, or
/// an image, never a link; refused before any video is read. Its folder held, where the sheet is
/// made and renamed (finding 749).
fn sheet_out(ctx: &Ctx, outp: &[u8]) -> Res<crate::shots::OutFile> {
    crate::shots::OutFile::at(outp, true).map_err(|m| ctx.die(cat!("record sheet: ", m)))
}

pub fn sheet_file_host(ctx: &mut Ctx, so: &SheetOpts) -> Res<()> {
    for has in [
        crate::exec::executable(b"/usr/bin/ffmpeg"),
        crate::exec::executable(b"/usr/bin/ffprobe"),
        crate::exec::executable(b"/usr/bin/bwrap"),
        crate::exec::executable(b"/usr/bin/timeout"),
    ] {
        if !has {
            return Err(ctx.die(cat!(
                "record sheet: no box '",
                ctx.name,
                "' is up, and a sheet made without one needs ffmpeg, ffprobe, bwrap and timeout in /usr/bin"
            )));
        }
    }
    let cache = cat!(
        shell::env("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.cache")),
        "/omabox"
    );
    let mut tmps = Tmps::new();
    let tmpd = if crate::shots::mkdir_p(&cache) {
        crate::trap::made(|| {
            let (_, t) = shell::mktemp_args(&[b"-d", &cat!(cache, "/sheet-XXXXXX")]).ok()?;
            tmps.add(t.clone());
            Some(t)
        })
    } else {
        None
    }
    .ok_or_else(|| ctx.die(cat!("record sheet: cannot write in ", cache)))?;
    let r = sheet_host_in(ctx, so, &tmpd);
    let _ = std::fs::remove_dir_all(shell::path(&tmpd));
    drop(tmps);
    r
}

fn sheet_host_in(ctx: &mut Ctx, so: &SheetOpts, tmpd: &[u8]) -> Res<()> {
    let (fd, size, tsv, outp, sheet) = if so.fromfd.is_empty() {
        let base = so.video.strip_suffix(b".mp4").unwrap_or(&so.video).to_vec();
        let (f, size) =
            open_video(&so.video, i64::MAX).map_err(|m| ctx.die(cat!("record sheet: ", m)))?;
        let outp = if so.out.is_empty() {
            cat!(base, ".sheet.png")
        } else {
            so.out.clone()
        };
        let sheet = sheet_out(ctx, &outp)?;
        (
            OwnedFd::from(f),
            size,
            cat!(base, ".marks.tsv"),
            outp,
            sheet,
        )
    } else {
        // (as the box's way: its marks read in 10 s at most, 1 MiB; the video as the fd is)
        let tsv = cat!(tmpd, "/v.marks.tsv");
        if !so.mfromfd.is_empty() {
            let ok = std::fs::File::create(shell::path(&tsv))
                .ok()
                .and_then(|f| {
                    let fd = dup_hi(num(&so.mfromfd) as i32).ok()?;
                    let mut c = crate::sig::command("timeout");
                    c.args(["10", "head", "-c", "1048576"])
                        .stdin(std::process::Stdio::from(fd))
                        .stdout(std::process::Stdio::from(f));
                    crate::sig::run_status(c).ok()
                })
                .is_some_and(|rc| rc == 0);
            if !ok {
                return Err(ctx.die("record sheet: cannot read the marks"));
            }
        }
        let fd = dup_hi(num(&so.fromfd) as i32).map_err(|_| Exit(1))?;
        let size = fd_size(&so.fromfd).unwrap_or(0);
        let sheet = crate::shots::OutFile::in_dir(tmpd, b"v.sheet.png", true)
            .map_err(|m| ctx.die(cat!("record sheet: ", m)))?;
        (fd, size, tsv, cat!(tmpd, "/v.sheet.png"), sheet)
    };
    // Only the frames' folder goes into the sandbox, not the marks or the sheet beside it.
    let fdir = cat!(tmpd, "/frames");
    if std::fs::create_dir(shell::path(&fdir)).is_err() {
        return Err(ctx.die(cat!("record sheet: cannot write in ", tmpd)));
    }
    say(cat!(
        "record sheet: no box '",
        ctx.name,
        "' up: ffmpeg reads the video here, sandboxed (no network, no display, nothing of yours but the video)"
    ));
    // Its ffprobe's bound by its size, at 10 MiB/s, as a box's.
    FF_SECS.store(
        REC_FF_FLOOR + size.clamp(0, 999_999_999_999_999) / 10_485_760,
        Ordering::SeqCst,
    );
    let sp = record_sheet(
        ctx,
        Src::Host {
            fd: fd.as_raw_fd(),
            dir: &fdir,
        },
        crate::shots::open_regular(&tsv),
        &sheet,
        &so.frames,
        so.every,
        &so.fit,
        &so.more,
        &so.as_,
    )?;
    drop(fd);
    let Some(sp) = sp else {
        return Err(Exit(1));
    };
    if !so.fromfd.is_empty() {
        let wrote = std::fs::read(shell::path(&outp)).ok().and_then(|d| {
            let fd = dup_hi(num(&so.tofd) as i32).ok()?;
            let mut f = std::fs::File::from(fd);
            f.write_all(&d).ok()
        });
        if wrote.is_none() {
            say("record sheet: could not write the sheet into the caller's file");
            return Err(Exit(1));
        }
    }
    out(cat!(sp, "\n"))
}

enum InFrom {
    Fd(OwnedFd, i64),
}

/// VIDEO, a file `record sheet` was named, open for reading and its size (finding 755): its folder
/// held as walked from `/` with no link in a box's folder followed (shots::hold_folder: a box HOME's
/// link to a host folder is refused), then the file in it as the regular file it is (a link at its
/// name followed only outside a box's folders, a FIFO or a device refused, never waited on), at
/// most MAX bytes. Err: the words for why.
pub(crate) fn open_video(v: &[u8], max: i64) -> Result<(std::fs::File, i64), Vec<u8>> {
    open_video_in(v, max, &crate::shots::box_writable)
}

pub(crate) fn open_video_in(
    v: &[u8],
    max: i64,
    guarded: &dyn Fn(&[u8]) -> bool,
) -> Result<(std::fs::File, i64), Vec<u8>> {
    // (a link at the name is followed only outside a box's folders, as a folder link is: the
    // user's own link to a video is fine, one a box planted is not)
    let mut cur = v.to_vec();
    let mut hops = 0;
    let f = loop {
        let name = shell::basename(&cur);
        let (folder, held) =
            crate::shots::hold_folder(&crate::shots::dirname(&cur), false, guarded)
                .map_err(|m| cat!("cannot read ", v, ": ", m))?;
        if let Some(f) = held.open_regular(&name) {
            break f;
        }
        let p = if folder == b"/" {
            cat!("/", name)
        } else {
            cat!(folder, "/", name)
        };
        let why = match std::fs::symlink_metadata(shell::path(&p)) {
            Err(e) => crate::shots::why(&e),
            Ok(m) if m.file_type().is_symlink() && !guarded(&folder) && hops < 40 => {
                let t = std::fs::read_link(shell::path(&p))
                    .map_err(|e| cat!("cannot read ", v, ": ", crate::shots::why(&e)))?;
                let t = shell::bytes(&t).to_vec();
                cur = if t.first() == Some(&b'/') {
                    t
                } else {
                    cat!(folder, "/", t)
                };
                hops += 1;
                continue;
            }
            Ok(m) if m.file_type().is_symlink() && hops >= 40 => "too many links".to_string(),
            Ok(m) if m.file_type().is_symlink() => {
                "it is a link in a box's folder (omabox reads through none there: name the file itself)"
                    .to_string()
            }
            Ok(_) => "it is not a regular file".to_string(),
        };
        return Err(cat!("cannot read ", v, ": ", why));
    };
    let size = f
        .metadata()
        .ok()
        .and_then(|m| i64::try_from(m.len()).ok())
        .unwrap_or(-1);
    if size < 0 || size > max {
        return Err(cat!(
            v,
            " is ",
            bytes_h(size.max(0)),
            ": more than omabox copies into a box (",
            bytes_h(max),
            ")"
        ));
    }
    Ok((f, size))
}

/// `box_sys /usr/bin/bash -c 'mkdir -p "${1%/*}" && cat > "$1"' omabox-record PATH` fed the video
/// (`head -c SIZE <&FD |`): its status (pipefail's).
fn copy_in(ctx: &mut Ctx, bv: &[u8], from: InFrom) -> Res<i32> {
    match from {
        InFrom::Fd(fd, size) => {
            let Ok((r, wr)) = pipe() else { return Ok(1) };
            let lim = u64::try_from(size).unwrap_or(0);
            let feeder = std::thread::spawn(move || {
                let mut src = std::fs::File::from(fd).take(lim);
                let mut d = std::fs::File::from(wr);
                let _ = std::io::copy(&mut src, &mut d);
            });
            let rc = write_in_box(
                ctx,
                P::WRITE_SH,
                "/usr/bin/bash",
                &String::from_utf8_lossy(bv),
                r,
            );
            let _ = feeder.join();
            rc
        }
    }
}

/// record_sheet VIDEO_IN_BOX MARKS_TSV OUT FRAMES EVERY_MS FIT [SHOWN] (#176): a contact sheet of the
/// video, each tile labelled with its number and time, the marks since the tile before under it.
/// Some(what it prints), None when it failed (said).
#[allow(clippy::too_many_arguments)]
pub(super) fn record_sheet(
    ctx: &mut Ctx,
    v: Src,
    tsv: Option<std::fs::File>,
    png: &crate::shots::OutFile,
    frames: &[u8],
    every: Option<i64>,
    fit: &[u8],
    more: &SheetMore,
    shown: &[u8],
) -> Res<Option<Vec<u8>>> {
    let nstr = if frames.is_empty() {
        b"15".to_vec()
    } else {
        frames.to_vec()
    };
    let mut a = w(&[
        "/usr/bin/ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets,r_frame_rate,width,height",
        "-of",
        "json",
    ]);
    a.extend(v.input(false));
    let (rc, j) = ff_at(ctx, v, Ff::Out, &a)?;
    let j = if rc == 0 {
        j[..j.len().min(65536)].to_vec()
    } else {
        Vec::new()
    };
    // (`read ... < <(jq -r ...)`, a process substitution: a plain step here, inside the `$(...)` both
    // callers run record_sheet in, which an INT to omabox alone already ends at)
    let r = jq::jq(&["-r", P::SHEET_JQ], Some(&strip_nl(&j)), jq::Err::Null);
    let f = shell::read_fields(&r.out, 5);
    let ok = r.ok()
        && shell::re_match(
            "^[1-9][0-9]{0,8} [1-9][0-9]{0,6} [1-9][0-9]{0,6} [1-9][0-9]{0,5} [1-9][0-9]{0,5}$",
            &f.join(&b' '),
        );
    if !ok {
        say("record: the contact sheet: ffprobe could not read the video's frames");
        return Ok(None);
    }
    let (nb, numr, den, vw, vh) = (num(&f[0]), num(&f[1]), num(&f[2]), num(&f[3]), num(&f[4]));
    FF_SECS.store(ff_bound(nb, vw * vh), Ordering::SeqCst);
    // -g (#260): a region inside the video.
    if let Some((gx, gy, gw, gh)) = more.geo {
        if gx + gw > vw || gy + gh > vh {
            say(format!(
                "record: the contact sheet: -g {gx},{gy} {gw}x{gh} is not inside the video ({vw}x{vh})"
            ));
            return Ok(None);
        }
    }
    // The frames' times as the labels round them, and the range asked for (#260).
    let ms_of = |k: i64| (k * 1000 * den + numr / 2) / numr;
    let (kf, kt) = match sheet_range(nb, numr, den, more.from, more.to) {
        Ok(r) => r,
        Err(m) => {
            say(cat!("record: the contact sheet: ", m));
            return Ok(None);
        }
    };
    let span = kt - kf + 1;
    let (ft, tt) = (ms_s3(ms_of(kf)), ms_s3(ms_of(kt)));
    let mut ks: Vec<i64> = Vec::new();
    let mut nchanged = None;
    if more.changed {
        let Some(ch) = sheet_changed(ctx, v, kf, kt, more.geo)? else {
            say("record: the contact sheet: ffmpeg could not compare the frames");
            return Ok(None);
        };
        if ch.len() > 99 {
            say(cat!(
                "record: the contact sheet: ",
                ch.len().to_string(),
                " frames change from ",
                ft,
                " to ",
                tt,
                ": more than 100 with the first (a shorter --from and --to, or a smaller -g)"
            ));
            return Ok(None);
        }
        nchanged = Some(ch.len());
        ks.push(kf);
        ks.extend(ch);
    } else if more.every_frame {
        if span > 100 {
            say(cat!(
                "record: the contact sheet: --every-frame from ",
                ft,
                " to ",
                tt,
                " is ",
                span.to_string(),
                " frames: more than 100 (a shorter --from and --to)"
            ));
            return Ok(None);
        }
        ks = (kf..=kt).collect();
    } else if let Some(every) = every {
        let mut last = -1;
        let mut i = 0i64;
        loop {
            let k = kf + (2 * i * every * numr + 1000 * den) / (2000 * den);
            if k > kt {
                break;
            }
            if k != last {
                ks.push(k);
            }
            last = k;
            if ks.len() > 100 {
                say(cat!(
                    "record: the contact sheet: --every ",
                    ms_s(every.to_string().as_bytes()),
                    " gives more than 100 frames: a longer one, or --frames N"
                ));
                return Ok(None);
            }
            i += 1;
        }
    } else {
        // --frames N (1-100, decimal: `010` is 10), at most the range's frames. Bash's loop read a 0
        // in front as octal: an 08 picked no frame, a 01 divided by 0 and ended the sheet, each in
        // its own words (finding 448).
        let n = shell::dec(&nstr).unwrap_or(15).min(span);
        if n == 1 {
            ks = vec![kf];
        } else {
            for i in 0..n {
                ks.push(kf + (2 * i * (span - 1) + n - 1) / (2 * (n - 1)));
            }
        }
    }
    let cnt = ks.len() as i64;
    let cols = cnt.min(5);
    let mut tw = 320;
    if !fit.is_empty() {
        // (every way above picks one frame at least: no sheet without)
        if cols == 0 {
            say("record: the contact sheet: no frame picked");
            return Ok(None);
        }
        tw = num(fit) / cols - 6;
    }
    // -g (#260): that region of each frame, inside the video, made larger (pixel for pixel, up to
    // 8 times) when it is narrower than a tile.
    let mut crop = String::new();
    let mut src_w = vw;
    if let Some((gx, gy, gw, gh)) = more.geo {
        crop = format!(",crop={gw}:{gh}:{gx}:{gy}:exact=1");
        src_w = gw * 8;
    }
    if tw > src_w {
        tw = src_w;
    }
    tw = tw / 2 * 2;
    if tw < 16 {
        tw = 16;
    }
    let up = match more.geo {
        Some((_, _, gw, _)) if tw > gw => ":flags=neighbor",
        _ => "",
    };
    let sel: Vec<String> = ks.iter().map(|k| format!("eq(n,{k})")).collect();
    // The frames, in a folder of the box's (Src::Box), or (Src::Host, #279) in the sandbox's /out,
    // which is the host folder the caller made.
    let bd = match v {
        Src::Box(_) => format!("{}/sheet-{}", rec_dir(&ctx.d), std::process::id()),
        Src::Host { .. } => "/out".to_string(),
    };
    let made = match v {
        Src::Box(_) => {
            sys_cap(
                ctx,
                &w(&["/usr/bin/bash", "-c", P::SHEETDIR_SH, "omabox-record", &bd]),
                None,
                Err2::Inherit,
                false,
            )
            .0 == 0
        }
        Src::Host { .. } => true,
    };
    let made = made && {
        let mut a = w(&["/usr/bin/ffmpeg", "-v", "error"]);
        a.extend(v.input(true));
        a.extend(w(&[
            "-vf".to_string(),
            format!("select='{}'{crop},scale={tw}:-2{up}", sel.join("+")),
            "-fps_mode".into(),
            "passthrough".into(),
            format!("{bd}/t-%03d.png"),
        ]));
        ff_at(ctx, v, Ff::Plain { to_err: true }, &a)?.0 == 0
    };
    let rm_bd = |ctx: &mut Ctx| {
        if let Src::Box(_) = v {
            let _ = sys_cap(
                ctx,
                &w(&["/usr/bin/rm", "-rf", "--", &bd]),
                None,
                Err2::Null,
                false,
            );
        }
    };
    if !made {
        rm_bd(ctx);
        say("record: the contact sheet: ffmpeg could not take the frames");
        return Ok(None);
    }
    // The frames copied out into a folder of the box dir's (gone with the box, and on any end of
    // this one), each at most 16 MiB, all of them 256. A host sheet's are where ffmpeg wrote them,
    // read under the same bounds.
    // (its own trap, in its subshell: the main shell's ends with the signal unless a code trap of
    // its own, rec_traps, is on; armed with the making: #360)
    let (tmp, tmp_g) = match v {
        Src::Box(_) => {
            let Some((tmp, g)) = crate::trap::made(|| {
                let (_, tmp) =
                    shell::mktemp_args(&[b"-d", &cat!(ctx.d, "/.record-XXXXXX")]).ok()?;
                let t = tmp.clone();
                let undo = move || {
                    let _ = std::fs::remove_dir_all(shell::path(&t));
                };
                let g = if crate::trap::code_active() {
                    crate::trap::on_exit_code(undo)
                } else {
                    crate::trap::on_exit(undo)
                };
                Some((tmp, g))
            }) else {
                return Ok(None);
            };
            (tmp, Some(g))
        }
        Src::Host { dir, .. } => (dir.to_vec(), None),
    };
    let frame = |i: usize| match v {
        Src::Box(_) => format!("{}/{:03}.png", String::from_utf8_lossy(&tmp), i + 1),
        Src::Host { .. } => format!("{}/t-{:03}.png", String::from_utf8_lossy(&tmp), i + 1),
    };
    let mut total = 0i64;
    let mut ms: Vec<i64> = Vec::new();
    let mut lab: Vec<Vec<u8>> = Vec::new();
    for (i, k) in ks.iter().enumerate() {
        let f = frame(i);
        let got = match v {
            Src::Box(_) => {
                let src = format!("{bd}/t-{:03}.png", i + 1);
                std::fs::File::create(&f).ok().and_then(|fh| {
                    let _e = crate::exec::Redir::null(2);
                    crate::exit::muted(|| box_copy(ctx, src.as_bytes(), 16_777_216, &fh))
                })
            }
            // (a regular file, not a link, as ffmpeg wrote it)
            Src::Host { .. } => std::fs::symlink_metadata(&f)
                .ok()
                .filter(|m| m.file_type().is_file() && m.len() <= 16_777_216)
                .and_then(|m| i64::try_from(m.len()).ok()),
        };
        let ok = got.is_some_and(|s| {
            total += s;
            size_of(std::path::Path::new(&f)) == Some(s)
                && crate::exit::muted(|| {
                    let _e = crate::exec::Redir::null(2);
                    crate::shots::png_size(f.as_bytes()).is_some()
                })
                && total <= 268_435_456
        });
        if !ok {
            rm_bd(ctx);
            drop(tmp_g);
            say(cat!(
                "record: the contact sheet: frame ",
                (i + 1).to_string(),
                " did not come out"
            ));
            return Ok(None);
        }
        let m = ms_of(*k);
        ms.push(m);
        lab.push(if more.any() {
            format!("{}: {} f{k}", i + 1, String::from_utf8_lossy(&ms_s3(m))).into_bytes()
        } else {
            format!("{}: {}.{:02}s", i + 1, m / 1000, m % 1000 / 10).into_bytes()
        });
    }
    rm_bd(ctx);
    // The marks, each under the first tile at or after it (the last tile for any after that).
    let mut has_marks = false;
    // (the marks as the regular file the caller opened, no link followed, 16 MiB at most)
    if let Some(f) = tsv {
        use std::io::Read;
        let mut d = Vec::new();
        let _ = f.take(16_777_216).read_to_end(&mut d);
        has_marks = !d.is_empty();
        // (`while read`: a last line with no newline is not read)
        let mut lines: Vec<&[u8]> = d.split(|&c| c == b'\n').skip(1).collect();
        lines.pop();
        for l in lines {
            let f = shell::read_ifs(l, 3, b"\t");
            if !shell::re_match("^[0-9]{1,9}$", &f[0]) {
                continue;
            }
            let m = num(&f[0]);
            // (in a range, only the marks since the frame before it, up to its last: #260)
            if (kf > 0 && m <= ms_of(kf - 1)) || (kt < nb - 1 && m > ms_of(kt)) {
                continue;
            }
            let mut i = 0usize;
            while (i as i64) < cnt - 1 && ms[i] < m {
                i += 1;
            }
            let mut line = f[2].clone();
            if shell::char_count(&line) > 40 {
                line = cat!(shell::head_chars(&line, 37), "...");
            }
            // (bash's `lab[i]+=`: one made where there is none, no frame picked)
            if lab.len() <= i {
                lab.resize(i + 1, Vec::new());
            }
            lab[i].extend_from_slice(&cat!(
                "\n> ",
                format!("{}.{:02}s ", m / 1000, m % 1000 / 10),
                line
            ));
        }
    }
    // magick reads % and \ in a label as escapes.
    let mut args: Vec<Vec<u8>> = vec![b"montage".to_vec()];
    let font = crate::grid::font("sans");
    if !font.is_empty() {
        args.extend([b"-font".to_vec(), font]);
    }
    for (i, l) in lab
        .iter()
        .take(usize::try_from(cnt).unwrap_or(0))
        .enumerate()
    {
        let l = replace_all(&replace_all(l, b"\\", b"\\\\"), b"%", b"%%");
        args.push(b"-label".to_vec());
        args.push(l);
        args.push(format!("png:{}", frame(i)).into_bytes());
    }
    // magick writes into a folder of omabox's own (the frames' copies, or beside a host sheet's
    // frames, out of the sandbox's reach), and the sheet is put at PNG from there, made and renamed
    // in its folder held: never through a link at PNG or PNG.part (finding 749).
    let made = match v {
        Src::Box(_) => cat!(tmp, "/sheet.png"),
        Src::Host { dir, .. } => cat!(crate::shots::dirname(dir), "/.sheet-made.png"),
    };
    args.extend(w(&[
        b"-tile".as_slice(),
        format!("{cols}x").as_bytes(),
        b"-geometry",
        b"+3+3",
        b"-pointsize",
        b"16",
        b"-background",
        b"white",
        b"-alpha",
        b"off",
        b"-depth",
        b"8",
        &cat!("PNG:", made),
    ]));
    let r = jq::tool("magick", &args, None, jq::Err::Out);
    let why = crate::grid::last_line(&r.out).to_vec();
    let mut err = !r.ok();
    if !err && !fit.is_empty() {
        err = !jq::tool(
            "magick",
            &w(&[
                cat!("PNG:", made),
                b"-resize".to_vec(),
                cat!(fit, "x", fit, ">"),
                b"-depth".to_vec(),
                b"8".to_vec(),
                cat!("PNG:", made),
            ]),
            None,
            jq::Err::Null,
        )
        .ok();
    }
    let mut sz = Vec::new();
    let put = if err {
        None
    } else {
        sz = crate::shots::png_size(&made)
            .map(|(a, b)| format!("{a}x{b}").into_bytes())
            .unwrap_or_default();
        Some(match crate::shots::open_regular(&made) {
            Some(mut f) => png.put(&mut f),
            None => Err(cat!("cannot read ", made)),
        })
    };
    rm(&shell::path(&made));
    drop(tmp_g);
    if err {
        say(cat!(
            "record: the contact sheet failed (magick montage, to ",
            png.path(),
            if why.is_empty() {
                b"".as_slice()
            } else {
                b": "
            },
            why,
            ")"
        ));
        return Ok(None);
    }
    if let Some(Err(m)) = put {
        say(cat!("record: the contact sheet: ", m));
        return Ok(None);
    }
    let png = png.path();
    let shown = if shown.is_empty() {
        png.clone()
    } else {
        shown.to_vec()
    };
    let lastm = ms.last().copied().unwrap_or(0);
    let what = match (nchanged, more.geo) {
        (Some(0), _) => cat!(" frame: nothing changed from ", ft, " to ", tt),
        (Some(_), Some((gx, gy, gw, gh))) => cat!(
            " frames (",
            ft,
            ", then each frame where ",
            format!("{gx},{gy} {gw}x{gh}"),
            " changed, to ",
            tt,
            ")"
        ),
        (Some(_), None) => cat!(
            " frames (",
            ft,
            ", then each frame that changed, to ",
            tt,
            ")"
        ),
        (None, _) if more.from.is_some() || more.to.is_some() => {
            cat!(" frames (", ft, " to ", tt, ")")
        }
        (None, _) => cat!(
            " frames (",
            format!("{}.{:02}s", lastm / 1000, lastm % 1000 / 10),
            " of video)"
        ),
    };
    say(cat!(
        "record: a ",
        sz,
        " contact sheet of ",
        cnt.to_string(),
        what,
        if more.any() {
            ", each labelled with its time and frame number"
        } else {
            ", each labelled with its time"
        },
        if has_marks {
            ", the marks under the frame after them"
        } else {
            ""
        },
        ": ",
        shown
    ));
    Ok(Some(shown))
}

/// A time in ms as "4.210s".
fn ms_s3(m: i64) -> Vec<u8> {
    format!("{}.{:03}s", m / 1000, m % 1000).into_bytes()
}

/// sheet_range NB NUM DEN FROM TO (#260): the first and last frame of a video of NB frames at
/// NUM/DEN fps whose times (as the labels round them) are at or after FROM and at or before TO; the
/// whole video without them. Err(why) when no frame is in it.
fn sheet_range(
    nb: i64,
    numr: i64,
    den: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<(i64, i64), Vec<u8>> {
    let ms_of = |k: i64| (k * 1000 * den + numr / 2) / numr;
    let mut kf = 0;
    if let Some(f) = from {
        kf = (f * numr / (1000 * den)).min(nb);
        while kf > 0 && ms_of(kf - 1) >= f {
            kf -= 1;
        }
        while kf < nb && ms_of(kf) < f {
            kf += 1;
        }
        if kf >= nb {
            return Err(cat!(
                "--from ",
                ms_s3(f),
                " is past the video's last frame (",
                ms_s3(ms_of(nb - 1)),
                ")"
            ));
        }
    }
    let mut kt = nb - 1;
    if let Some(t) = to {
        kt = (t * numr / (1000 * den)).min(nb - 1);
        while kt >= 0 && ms_of(kt) > t {
            kt -= 1;
        }
        while kt + 1 < nb && ms_of(kt + 1) <= t {
            kt += 1;
        }
    }
    if kt < kf {
        return Err(cat!(
            "no frame from ",
            ms_s3(from.unwrap_or(0)),
            " to ",
            ms_s3(to.unwrap_or(0)),
            " (one each ",
            ms_s3(1000 * den / numr),
            ")"
        ));
    }
    Ok((kf, kt))
}

/// CHANGED_LEVEL: how far a pixel of the region must move from the frame before (of 255, in Y, U or
/// V) for the frame to count as changed (#260): H.264's own noise in a still picture stays within 5
/// (finding 603).
const CHANGED_LEVEL: i64 = 6;

/// The filter --changed reads (#260): the frames KF to KT numbered by their pts, the region GEO, each
/// frame's difference from the one before in all three planes side by side, and the frames where one
/// pixel of it is past CHANGED_LEVEL printed on stdout ("frame:I pts:K ..." lines).
fn changed_filter(kf: i64, kt: i64, geo: Option<(i64, i64, i64, i64)>) -> String {
    let crop = geo.map_or(String::new(), |(x, y, w, h)| {
        format!(",crop={w}:{h}:{x}:{y}:exact=1")
    });
    format!(
        "setpts=N,trim=start_frame={kf}:end_frame={},format=yuv444p{crop},tblend=all_mode=difference,\
         extractplanes=y+u+v[a][b][c];[a][b][c]hstack=3,signalstats,\
         metadata=mode=select:key=lavfi.signalstats.YMAX:value={CHANGED_LEVEL}:function=greater,\
         metadata=mode=print:key=lavfi.signalstats.YMAX:file=-",
        kt + 1
    )
}

/// changed_frames OUT KF KT: the frames changed_filter's output names, in (KF, KT], in order, once.
fn changed_frames(o: &[u8], kf: i64, kt: i64) -> Vec<i64> {
    let re = shell::Re::new("^frame:[0-9]{1,9} +pts:([0-9]{1,9}) ");
    let mut v: Vec<i64> = Vec::new();
    for l in o.split(|&c| c == b'\n') {
        let Some(c) = re.as_ref().and_then(|r| r.captures(l, false)) else {
            continue;
        };
        let Some(k) = c
            .get(1)
            .copied()
            .flatten()
            .and_then(|(a, b)| shell::dec(&l[a..b]))
        else {
            continue;
        };
        if k > kf && k <= kt && v.last().is_none_or(|&p| k > p) {
            v.push(k);
        }
    }
    v
}

/// sheet_changed V KF KT GEO (#260): the frames after KF, to KT, where GEO (the whole frame without it)
/// differs from the frame before; None when ffmpeg could not tell.
fn sheet_changed(
    ctx: &mut Ctx,
    v: Src,
    kf: i64,
    kt: i64,
    geo: Option<(i64, i64, i64, i64)>,
) -> Res<Option<Vec<i64>>> {
    let mut a = w(&[
        "/usr/bin/ffmpeg",
        "-nostdin",
        "-hide_banner",
        "-nostats",
        "-v",
        "error",
    ]);
    a.extend(v.input(true));
    a.extend(w(&[
        "-map".to_string(),
        "0:v:0".into(),
        "-vf".into(),
        changed_filter(kf, kt, geo),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]));
    let (rc, o) = ff_at(ctx, v, Ff::Out, &a)?;
    if rc != 0 {
        return Ok(None);
    }
    Ok(Some(changed_frames(&o, kf, kt)))
}

/// Where a contact sheet's ffmpeg and ffprobe read the video (#279): a file in the box, or here,
/// with no box up, an fd of this omabox's given to them in a sandbox of their own (host_ff).
#[derive(Clone, Copy)]
pub(super) enum Src<'a> {
    Box(&'a [u8]),
    Host { fd: RawFd, dir: &'a [u8] },
}

impl Src<'_> {
    /// The input's words: the box's path, or the fd (`fd:`, the only protocol it may open), after
    /// `-i` for ffmpeg (I).
    fn input(self, i: bool) -> Vec<Vec<u8>> {
        let mut a = Vec::new();
        match self {
            Src::Box(v) => {
                if i {
                    a.push(b"-i".to_vec());
                }
                a.push(v.to_vec());
            }
            Src::Host { .. } => {
                a.extend(w(&["-protocol_whitelist", "fd", "-fd", "3"]));
                if i {
                    a.push(b"-i".to_vec());
                }
                a.push(b"fd:".to_vec());
            }
        }
        a
    }
}

/// rec_ff, where V says: in the box, or here in host_ff's sandbox.
fn ff_at(ctx: &mut Ctx, v: Src, mode: Ff, argv: &[Vec<u8>]) -> Res<(i32, Vec<u8>)> {
    match v {
        Src::Box(_) => rec_ff(ctx, mode, argv),
        Src::Host { fd, dir } => rec_ff_with(ctx, mode, argv, |_, a| Ok(host_ff(fd, dir, mode, a))),
    }
}

/// host_sandbox DIR: bwrap's words for a contact sheet's ffmpeg with no box up (#279, finding 604):
/// every namespace its own (no network, no host process, no IPC), no environment (no display to
/// open, no runtime dir), the system's /usr read-only and a fresh /proc, /dev and /tmp, nothing else
/// of the host but DIR, writable at /out; it dies with omabox.
fn host_sandbox(dir: &[u8]) -> Vec<Vec<u8>> {
    let mut a = w(&[
        "/usr/bin/bwrap",
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--ro-bind",
        "/usr",
        "/usr",
    ]);
    // /bin, /sbin, /lib, /lib64 as the host's links into /usr (#247: Debian's /lib64 is usr/lib64).
    for (target, link) in crate::bwrap::root_links(b"") {
        a.extend([b"--symlink".to_vec(), target, link.as_bytes().to_vec()]);
    }
    a.extend(w(&[
        "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--bind",
    ]));
    a.push(dir.to_vec());
    a.extend(w(&["/out", "--chdir", "/out", "--"]));
    a
}

/// host_ff FD DIR MODE ARGV (#279): ARGV in host_sandbox(DIR), the video FD its fd 3 and no other fd
/// of this omabox's, read from its start; its status, and its stdout (with its stderr, Ff::Err)
/// captured as rec_ff's, 16 MiB at most. A crash makes no core (none to notify the desktop of).
fn host_ff(fd: RawFd, dir: &[u8], mode: Ff, argv: &[Vec<u8>]) -> (i32, Vec<u8>) {
    // SAFETY: lseek on an fd of this process; it moves the offset its dup in the child shares.
    unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
    let mut a = host_sandbox(dir);
    a.extend(argv.iter().cloned());
    let mut c = crate::exec::command(
        &a,
        crate::exec::Fds {
            fd3: Some(fd),
            only: true,
            nocore: true,
            ..crate::exec::Fds::default()
        },
    );
    c.stdin(std::process::Stdio::null());
    let mut reader = None;
    match mode {
        Ff::Plain { to_err } => {
            if to_err {
                if let Ok(e) = dup_hi(2) {
                    c.stdout(std::process::Stdio::from(e));
                }
            }
        }
        Ff::Out | Ff::Err => {
            let Ok((r, wr)) = pipe() else {
                return (1, Vec::new());
            };
            if mode == Ff::Err {
                match wr.try_clone() {
                    Ok(e) => c.stderr(std::process::Stdio::from(e)),
                    Err(_) => return (1, Vec::new()),
                };
            } else {
                c.stderr(std::process::Stdio::null());
            }
            c.stdout(std::process::Stdio::from(wr));
            reader = Some(std::thread::spawn(move || {
                crate::exec::read_all(r, Some(16_777_216))
            }));
        }
    }
    let rc = crate::sig::run_status(c).unwrap_or(126);
    let out = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    (rc, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding 755: `record sheet VIDEO`'s file opened as the regular file it is: no link at its
    /// name, none in a box's folder on its way, no FIFO waited on, no device, no more than MAX.
    #[test]
    fn sheet_video_opens_no_link_no_fifo() {
        use std::os::unix::fs::symlink;
        let t = std::env::temp_dir().join(format!("omabox-sheetvid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let home = t.join("homes/b/home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(t.join("host")).unwrap();
        std::fs::write(t.join("host/keys.env"), "KEY=1").unwrap();
        std::fs::write(home.join("take.mp4"), "video").unwrap();
        let root = shell::bytes(&t.join("homes")).to_vec();
        let g = move |p: &[u8]| crate::shots::under_box_part(p, &root, b"home");
        let p = |r: &str| shell::bytes(&t.join(r)).to_vec();
        let (f, n) = open_video_in(&p("homes/b/home/take.mp4"), 100, &g).unwrap();
        assert_eq!(n, 5);
        drop(f);
        assert!(
            open_video_in(&p("homes/b/home/take.mp4"), 4, &g).is_err(),
            "over MAX"
        );
        symlink(t.join("host/keys.env"), home.join("ln.mp4")).unwrap();
        assert!(open_video_in(&p("homes/b/home/ln.mp4"), 100, &g).is_err());
        symlink("/dev/zero", home.join("zero.mp4")).unwrap();
        assert!(open_video_in(&p("homes/b/home/zero.mp4"), 100, &g).is_err());
        assert!(
            open_video_in(b"/dev/zero", i64::MAX, &g).is_err(),
            "a device"
        );
        symlink(t.join("host"), home.join("dir")).unwrap();
        std::fs::copy(home.join("take.mp4"), t.join("host/v.mp4")).unwrap();
        assert!(open_video_in(&p("homes/b/home/dir/v.mp4"), 100, &g).is_err());
        assert!(
            open_video_in(&p("host/v.mp4"), 100, &g).is_ok(),
            "outside a box"
        );
        // The user's own link, outside a box's folders, to a regular video: followed.
        symlink(t.join("host/v.mp4"), t.join("mine.mp4")).unwrap();
        assert_eq!(open_video_in(&p("mine.mp4"), 100, &g).unwrap().1, 5);
        // ...but not one that leads on to a link a box planted, or to /dev/zero.
        symlink(home.join("ln.mp4"), t.join("via.mp4")).unwrap();
        assert!(open_video_in(&p("via.mp4"), 100, &g).is_err());
        symlink("/dev/zero", t.join("z.mp4")).unwrap();
        assert!(open_video_in(&p("z.mp4"), i64::MAX, &g).is_err());
        let c = std::ffi::CString::new(p("homes/b/home/fifo.mp4")).unwrap();
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // A link of the user's to a FIFO: followed, the FIFO refused, not waited on.
        symlink(home.join("fifo.mp4"), t.join("tofifo.mp4")).unwrap();
        let tofifo = p("tofifo.mp4");
        let (tx2, rx2) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx2.send(open_video_in(&tofifo, 100, &|_: &[u8]| false).is_err());
        });
        assert!(
            rx2.recv_timeout(std::time::Duration::from_secs(5))
                .expect("waited on a FIFO")
        );
        let fifo = p("homes/b/home/fifo.mp4");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(open_video_in(&fifo, 100, &|_: &[u8]| false).is_err());
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("waited on a FIFO")
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    /// A contact sheet's range (#260): the frames whose label times are in it, at 30 fps and at an
    /// fps that is no whole number of ms a frame.
    #[test]
    fn sheet_ranges() {
        assert_eq!(sheet_range(90, 30, 1, None, None), Ok((0, 89)));
        assert_eq!(sheet_range(90, 30, 1, Some(1300), Some(1500)), Ok((39, 45)));
        // (frame 1 is at 33 ms as the labels say it: 33 takes it, 34 does not)
        assert_eq!(sheet_range(90, 30, 1, Some(33), Some(33)), Ok((1, 1)));
        assert_eq!(sheet_range(90, 30, 1, Some(34), None), Ok((2, 89)));
        assert_eq!(sheet_range(90, 30, 1, None, Some(99_000)), Ok((0, 89)));
        assert_eq!(sheet_range(90, 30, 1, Some(2967), None), Ok((89, 89)));
        assert!(sheet_range(90, 30, 1, Some(2968), None).is_err());
        assert!(sheet_range(90, 30, 1, Some(1010), Some(1020)).is_err());
        assert_eq!(
            sheet_range(1000, 30000, 1001, Some(1001), Some(1035)),
            Ok((30, 31))
        );
    }

    /// --changed's frames (#260): the pts of each line ffmpeg printed, after the first frame and up
    /// to the last, once each; anything else ignored.
    #[test]
    fn changed_frames_read() {
        let o = b"frame:0    pts:40      pts_time:0.0333\nlavfi.signalstats.YMAX=235\n\
                  frame:1    pts:43      pts_time:0.0358\nlavfi.signalstats.YMAX=235\n\
                  frame:2    pts:43      pts_time:0.0358\nframe:x pts:7 \nframe:3    pts:99      pts_time:1\n";
        assert_eq!(changed_frames(o, 39, 60), vec![40, 43]);
        assert_eq!(changed_frames(o, 40, 99), vec![43, 99]);
        assert!(changed_frames(b"", 0, 10).is_empty());
        let f = changed_filter(39, 45, Some((30, 30, 100, 60)));
        assert!(f.starts_with("setpts=N,trim=start_frame=39:end_frame=46,format=yuv444p,crop=100:60:30:30:exact=1,tblend="));
        assert!(f.contains(":value=6:function=greater,"));
        assert!(!changed_filter(0, 9, None).contains("crop"));
    }

    /// A sheet's sandbox with no box up (#279): every namespace its own, no environment, the host's
    /// /usr read-only and the frames' folder, nothing else of it.
    #[test]
    fn host_sandbox_holds_only_usr_and_the_folder() {
        let a: Vec<String> = host_sandbox(b"/c/omabox/sheet-x/frames")
            .iter()
            .map(|x| String::from_utf8_lossy(x).into_owned())
            .collect();
        for f in [
            "--unshare-all",
            "--clearenv",
            "--die-with-parent",
            "--new-session",
        ] {
            assert!(a.iter().any(|x| x == f), "{f}");
        }
        assert!(!a.iter().any(|x| x == "--share-net"));
        let binds: Vec<(&str, &str, &str)> = a
            .windows(3)
            .filter(|w| w[0].contains("bind"))
            .map(|w| (w[0].as_str(), w[1].as_str(), w[2].as_str()))
            .collect();
        assert_eq!(
            binds,
            vec![
                ("--ro-bind", "/usr", "/usr"),
                ("--bind", "/c/omabox/sheet-x/frames", "/out")
            ]
        );
        assert_eq!(a.last().map(String::as_str), Some("--"));
    }

    #[test]
    fn sheet_geos() {
        use super::super::record::sheet_geo;
        assert_eq!(sheet_geo(b"30,30 100x60"), Some((30, 30, 100, 60)));
        assert_eq!(sheet_geo(b"0,0 1x1"), Some((0, 0, 1, 1)));
        for bad in [
            &b"30,30 0x60"[..],
            b"30 30 100x60",
            b"-1,0 10x10",
            b"1,1 10x10 ",
            b"",
        ] {
            assert_eq!(sheet_geo(bad), None);
        }
    }
}
