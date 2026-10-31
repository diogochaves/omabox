//! `record stop` and `record sheet` (#176, #224, #225, #226, findings 306, 312, 319, 356): the
//! recording ended, made into the video in the box (rec_ff: every ffmpeg bounded), its marks and
//! captions beside it, the input drawn in, still stretches cut, a contact sheet; copied out as the
//! bytes of the box's `cat` (box_copy, finding 297).

use super::record::{
    Err2, REC_AAC, REC_CRF, REC_DIR, REC_FF_FLOOR, REC_TAGS, REC_TAGS_F, REC_YUV_ARGS, Rec,
    SheetMore, SheetOpts, StopOpts, audio_end, now_us, num, rec_dirs, rec_max, rec_yuv,
    record_first_us, record_firsts, record_running, record_size, rm, sys_cap, sys_q, sys_run, w,
};
use crate::cat;
use crate::exec::{ExecOpts, box_exec, dup_hi, pipe};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jail::in_jail;
use crate::jq;
use crate::num::ms_s;
use crate::recprog as P;
use crate::shell::{self, strip_nl};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// REC_FF_SECS: rec_ff's bound once one is known (0: not yet, REC_FF_FLOOR).
static FF_SECS: AtomicI64 = AtomicI64::new(0);

/// rec_ff_bound FRAMES PIXELS: REC_FF_FLOOR, and 5 s more for each second of 1080p at 30 fps.
fn ff_bound(frames: i64, px: i64) -> i64 {
    REC_FF_FLOOR + (frames * px + 12_441_599) / 12_441_600
}

/// How rec_ff gives its output: as it comes (to stderr with TO_ERR), or captured (-o, -e with its
/// stderr), printed when it ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ff {
    Plain { to_err: bool },
    Out,
    Err,
}

/// rec_ff MODE /usr/bin/FFMPEG ARG... (finding 356): in the box under timeout, a run that ran out of
/// time run once more, a second said, 124. Its status, and (captured) its output and a newline.
fn rec_ff(ctx: &mut Ctx, mode: Ff, argv: &[Vec<u8>]) -> Res<(i32, Vec<u8>)> {
    rec_ff_with(ctx, mode, argv, |ctx, a| {
        Ok(match mode {
            Ff::Out => sys_cap(ctx, a, Some(16_777_216), Err2::Null, true),
            Ff::Err => sys_cap(ctx, a, Some(16_777_216), Err2::Out, true),
            Ff::Plain { to_err } => (sys_run(ctx, a, to_err, Err2::Inherit, true)?, Vec::new()),
        })
    })
}

/// rec_ff, each run made by RUN (in the box: sys_cap, sys_run): its status and what it captured.
fn rec_ff_with(
    ctx: &mut Ctx,
    mode: Ff,
    argv: &[Vec<u8>],
    mut run: impl FnMut(&mut Ctx, &[Vec<u8>]) -> Res<(i32, Vec<u8>)>,
) -> Res<(i32, Vec<u8>)> {
    let mut secs = match FF_SECS.load(Ordering::SeqCst) {
        0 => REC_FF_FLOOR,
        s => s,
    };
    let e = shell::env_or_empty("OMABOX_RECORD_FF_SECS");
    if shell::re_match("^[1-9][0-9]{0,6}$", &e) {
        if let Some(v) = shell::dec(&e) {
            if v < secs {
                secs = v;
            }
        }
    }
    let mut a = w(&["/usr/bin/timeout", "-k", "5", &secs.to_string()]);
    a.extend(argv.iter().cloned());
    let tool = shell::basename(argv.first().map_or(&b""[..], |x| x));
    let (mut rc, mut o) = (0, Vec::new());
    for t in 1..=2 {
        (rc, o) = run(ctx, &a)?;
        if rc != 124 && rc != 137 {
            break;
        }
        if t == 1 {
            say(cat!(
                "record: ",
                tool,
                " did not finish in ",
                secs.to_string(),
                " s (stuck: finding 356): killed, run once more"
            ));
        } else {
            say(cat!(
                "record: ",
                tool,
                " did not finish in ",
                secs.to_string(),
                " s again: killed"
            ));
            rc = 124;
        }
    }
    let o = strip_nl(&o);
    let printed = if mode != (Ff::Plain { to_err: false })
        && mode != (Ff::Plain { to_err: true })
        && !o.is_empty()
    {
        cat!(o, "\n")
    } else {
        Vec::new()
    };
    Ok((rc, printed))
}

/// `$(rec_ff -o ARGV...)`.
fn rec_ff_subst(ctx: &mut Ctx, argv: &[Vec<u8>]) -> Res<(i32, Vec<u8>)> {
    rec_ff(ctx, Ff::Out, argv)
}

/// `$(rec_ff -o ... | head -c N | head -n 1) || V=""`.
fn ff_line(ctx: &mut Ctx, argv: &[Vec<u8>], n: usize) -> Res<Vec<u8>> {
    let (rc, o) = rec_ff(ctx, Ff::Out, argv)?;
    if rc != 0 {
        return Ok(Vec::new());
    }
    let o = &o[..o.len().min(n)];
    Ok(o.split(|&c| c == b'\n').next().unwrap_or_default().to_vec())
}

/// `$(awk -v K=V ... 'BEGIN { PROG }')`.
fn awk_begin(vars: &[(&str, &[u8])], prog: &str) -> Vec<u8> {
    let mut a: Vec<Vec<u8>> = Vec::new();
    for (k, v) in vars {
        a.push(b"-v".to_vec());
        a.push(cat!(k, "=", v));
    }
    a.push(prog.as_bytes().to_vec());
    jq::tool("awk", &a, None, jq::Err::Inherit).text()
}

/// bytes_h B: a size in words.
fn bytes_h(b: &[u8]) -> Vec<u8> {
    awk_begin(&[("b", b)], P::BYTES_H_AWK)
}

/// rec_crf CUT CRF: the first pass's quality, the video's (--crf's, #278, or REC_CRF), 12 at most when
/// --cut-idle encodes it once more.
fn rec_crf(cut: Option<i64>, crf: Option<i64>) -> String {
    let c = crf.unwrap_or(REC_CRF);
    if cut.is_some() { c.min(12) } else { c }.to_string()
}

/// box_copy's reading of the box's stat (ST: "%F|%s|%b|%B"): the size to copy, or None when FILE is
/// not a regular file, is sparse, or is more than MAX bytes (said).
fn copy_size(ctx: &Ctx, file: &[u8], max: i64, st: &[u8]) -> Option<i64> {
    let f = shell::read_ifs(&strip_nl(st), 4, b"|");
    let ok = f[0] == b"regular file"
        && shell::re_match(
            "^[0-9]{1,15} [0-9]{1,15} [0-9]{1,6}$",
            &cat!(f[1], " ", f[2], " ", f[3]),
        );
    if !ok {
        say(cat!(
            "record: ",
            file,
            " in box '",
            ctx.name,
            "' is not a file"
        ));
        return None;
    }
    let (size, blocks, bsz) = (num(&f[1]), num(&f[2]), num(&f[3]));
    if blocks * bsz + 1_048_576 < size {
        say(cat!(
            "record: ",
            file,
            " in box '",
            ctx.name,
            "' is sparse (",
            size.to_string(),
            " bytes, ",
            (blocks * bsz).to_string(),
            " on disk)"
        ));
        return None;
    }
    if size > max {
        say(cat!(
            "record: ",
            file,
            " in box '",
            ctx.name,
            "' is ",
            bytes_h(size.to_string().as_bytes()),
            ": more than omabox copies out of a box (",
            bytes_h(max.to_string().as_bytes()),
            ")"
        ));
        return None;
    }
    Some(size)
}

/// box_copy's cat of FILE in the box, given a time for SIZE (10 s, and 1 a 10 MiB): a file swapped
/// for a FIFO after its stat is not read for ever.
fn copy_argv(file: &[u8], size: i64) -> Vec<Vec<u8>> {
    w(&[
        b"/usr/bin/timeout".as_slice(),
        b"-k",
        b"1",
        (10 + size / 10_485_760).to_string().as_bytes(),
        b"/usr/bin/cat",
        b"--",
        file,
    ])
}

/// box_copy FILE_IN_BOX MAX TO: the file's bytes into TO, read in the box by its cat, when the box's
/// stat says it is a regular file, not sparse, of at most MAX bytes; never more than that size, in a
/// time for it. Some(its size) when the cat ran to its end, None when refused (said) or cut short.
fn box_copy(ctx: &mut Ctx, file: &[u8], max: i64, to: &std::fs::File) -> Option<i64> {
    box_copy_with(ctx, file, max, to, |ctx, a, wr| match wr {
        None => sys_cap(ctx, a, Some(200), Err2::Null, false).1,
        Some(fd) => {
            let rc = box_exec(
                ctx,
                a,
                ExecOpts {
                    sys: true,
                    stdout: Some(fd),
                    ..ExecOpts::default()
                },
            );
            if matches!(rc, Ok(0)) {
                b"0".to_vec()
            } else {
                b"1".to_vec()
            }
        }
    })
}

/// box_copy with how it runs a command in the box: RUN(ctx, argv, None) is the stat, its output;
/// RUN(ctx, argv, Some(fd)) the cat into FD, "0" when it ran to its end (a test runs both on the
/// host: box_copy_bounds_a_fifo).
fn box_copy_with(
    ctx: &mut Ctx,
    file: &[u8],
    max: i64,
    to: &std::fs::File,
    mut run: impl FnMut(&mut Ctx, &[Vec<u8>], Option<std::os::fd::RawFd>) -> Vec<u8>,
) -> Option<i64> {
    let st = run(
        ctx,
        &w(&[
            b"/usr/bin/stat".as_slice(),
            b"-c",
            b"%F|%s|%b|%B",
            b"--",
            file,
        ]),
        None,
    );
    let size = copy_size(ctx, file, max, &st)?;
    // (`| head -c SIZE`: streamed, never held here whole; the pipe closed once SIZE came)
    let (r, w0) = pipe().ok()?;
    let wr = dup_hi(w0.as_raw_fd()).ok()?;
    drop(w0);
    let dst = to.try_clone().ok()?;
    let lim = u64::try_from(size).unwrap_or(0);
    let copier = std::thread::spawn(move || {
        let mut src = std::fs::File::from(r).take(lim);
        let mut d = dst;
        let _ = std::io::copy(&mut src, &mut d);
    });
    let a = copy_argv(file, size);
    let rc = run(ctx, &a, Some(wr.as_raw_fd()));
    drop(wr);
    let _ = copier.join();
    (rc == b"0").then_some(size)
}

/// `stat -L -c %s FILE`.
fn size_of(p: &std::path::Path) -> Option<i64> {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| i64::try_from(m.len()).ok())
}

/// The files record_stop makes here (the box dir's, the video's .part, the input's temp file),
/// removed however it ends, by one trap from the first of them on (rec_traps': `exit 130|143|129`
/// on a signal). Bash's had the input's file under its EXIT trap alone, so that a signal ended the
/// stop there as it did before finding 432; one trap is enough (finding 452).
struct Tmps {
    v: Arc<Mutex<Vec<Vec<u8>>>>,
    part: Arc<Mutex<Vec<u8>>>,
    trap: Option<crate::trap::Trap>,
}

impl Tmps {
    fn new() -> Tmps {
        Tmps {
            v: Arc::new(Mutex::new(Vec::new())),
            part: Arc::new(Mutex::new(Vec::new())),
            trap: None,
        }
    }

    fn undo(&self) -> impl FnOnce() + Send + 'static {
        let (v, p) = (self.v.clone(), self.part.clone());
        move || {
            // (`rm -rf --`: a folder, or the input's file)
            for d in v.lock().map(|x| x.clone()).unwrap_or_default() {
                let p = shell::path(&d);
                if std::fs::remove_dir_all(&p).is_err() {
                    rm(&p);
                }
            }
            let part = p.lock().map(|x| x.clone()).unwrap_or_default();
            if !part.is_empty() {
                rm(&shell::path(&part));
            }
        }
    }

    /// rec_traps: the trap set (once).
    fn arm(&mut self) {
        if self.trap.is_none() {
            self.trap = Some(crate::trap::on_exit_code(self.undo()));
        }
    }

    fn push(&mut self, d: Vec<u8>) {
        if let Ok(mut v) = self.v.lock() {
            v.push(d);
        }
    }

    fn add(&mut self, d: Vec<u8>) {
        self.push(d);
        self.arm();
    }

    fn set_part(&mut self, p: &[u8]) {
        if let Ok(mut x) = self.part.lock() {
            *x = p.to_vec();
        }
        self.arm();
    }
}

/// record_stop: the video, its marks beside it, and with SHEET its contact sheet (#176); with the
/// fds (a jailed caller's, #224) written through them, nothing opened at a path of the caller's.
pub fn stop(ctx: &mut Ctx, rec: &mut Rec, so: &StopOpts) -> Res<()> {
    let mut tmps = Tmps::new();
    let r = stop_in(ctx, rec, so, &mut tmps);
    drop(tmps);
    r
}

fn stop_in(ctx: &mut Ctx, rec: &mut Rec, so: &StopOpts, tmps: &mut Tmps) -> Res<()> {
    let alive = record_running(ctx, rec)?;
    if !alive && !ctx.at("record").is_file() {
        return Err(ctx.die(cat!("record stop: box '", ctx.name, "' is not recording")));
    }
    if in_jail() && !ctx.at("record.as").is_file() {
        return Err(ctx.die(cat!(
            "record stop: box '",
            ctx.name,
            "''s recording was started outside the jail: it is stopped there"
        )));
    }
    if !so.as_.is_empty() && so.as_ != rec.file {
        return Err(ctx.die(cat!(
            "record stop: box '",
            ctx.name,
            "' records to ",
            rec.file,
            ", not ",
            so.as_
        )));
    }
    // A jailed agent's recording stopped from outside the jail: a file in omabox's cache folder.
    if so.tofd.is_empty() && ctx.at("record.as").is_file() {
        let asname = rec.file.clone();
        let cache = cat!(
            shell::env("XDG_CACHE_HOME")
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| cat!(shell::env_or_empty("HOME"), "/.cache")),
            "/omabox/records"
        );
        if !crate::shots::mkdir_p(&cache) {
            return Err(ctx.die(cat!("record stop: cannot make ", cache)));
        }
        let t = shell::date("%Y%m%d-%H%M%S");
        rec.file = cat!(cache, "/omabox-", ctx.name, "-", t, ".mp4");
        say(cat!(
            "record: a jailed agent started this recording, to its file ",
            asname,
            ": written to ",
            rec.file,
            " instead (never at a path a jail named)"
        ));
    }
    let apid = if ctx.at("record.audio").is_file() {
        std::fs::read(ctx.at("record.audio"))
            .map(|v| strip_nl(&v))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    // Each wf-recorder still running (the layout's: one may have ended by itself) told to finish.
    if alive || rec.n > 1 {
        let rpids: Vec<Vec<u8>> = shell::words(
            &rec.pid
                .iter()
                .map(|&c| if c == b',' { b' ' } else { c })
                .collect::<Vec<u8>>(),
        );
        let end = crate::settle::after(15);
        let still = |ctx: &mut Ctx, s: &str| {
            let mut a = w(&[
                "/usr/bin/env",
                &format!("S={s}"),
                "/usr/bin/sh",
                "-c",
                P::STILL_SH,
                "omabox-record",
            ]);
            a.extend(rpids.iter().cloned());
            sys_q(ctx, &a)
        };
        still(ctx, "INT")?;
        audio_end(ctx, &apid)?;
        while still(ctx, "")? == 0 {
            if crate::settle::past(end) {
                still(ctx, "KILL")?;
                say(
                    "record: wf-recorder did not finish in 15 s: killed (its file may lack its last frames)",
                );
                break;
            }
            crate::trap::sleep(b"0.1");
        }
    }
    // A recording that stopped by itself still has its sound's ffmpeg to end.
    if !alive && rec.n <= 1 {
        audio_end(ctx, &apid)?;
    }
    // Its --crf (#278), read before the files go.
    rec.crf = std::fs::read(ctx.at("record.crf"))
        .ok()
        .and_then(|v| shell::dec(&strip_nl(&v)))
        .filter(|c| (0..=51).contains(c));
    for f in ["record", "record.audio", "record.as", "record.crf"] {
        rm(&ctx.at(f));
    }
    // The bound on its ffmpeg runs (finding 356): as many frames as it ran for, of its size.
    let px = if rec.n > 1 {
        let s = record_size(ctx);
        let (a, b) = s.split_at(s.iter().position(|&c| c == b'x').unwrap_or(0));
        num(a) * num(b.get(1..).unwrap_or_default())
    } else {
        let l = ff_line(
            ctx,
            &w(&[
                "/usr/bin/ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
                &format!("{REC_DIR}/raw.mkv"),
            ]),
            usize::MAX,
        )?;
        let re = shell::Re::new("^([1-9][0-9]{0,4}),([1-9][0-9]{0,4})$");
        match re.as_ref().and_then(|r| r.captures(&l, false)) {
            Some(c) => {
                let g = |i: usize| {
                    c.get(i)
                        .copied()
                        .flatten()
                        .map_or(0, |(a, b)| num(&l[a..b]))
                };
                g(1) * g(2)
            }
            None => 3840 * 2160,
        }
    };
    let mut fr = 0;
    if shell::re_match("^[0-9]{1,17} [0-9]{1,3}$", &cat!(rec.start, " ", rec.fps)) {
        // (decimal: an fps of 08 is 8, where bash's arithmetic ended the command in its own words,
        // finding 448)
        let st = crate::num::read(ctx, "the recording's start", &rec.start)?;
        let f = crate::num::read(ctx, "the recording's fps", &rec.fps)?;
        fr = (now_us() - st) * f / 1_000_000;
    }
    let fr = fr.max(0);
    FF_SECS.store(ff_bound(fr, px), Ordering::SeqCst);
    let fps = rec.fps.clone();
    // Its sound (#226), put in the video's clock: AFILT, the filter for its input.
    let mut afilt = Vec::new();
    if !apid.is_empty() {
        // (`if rec_ff -o ... | head -c 4096 | grep -q .`: a pipeline, no substitution)
        let (rc, o) = rec_ff(
            ctx,
            Ff::Out,
            &w(&[
                "/usr/bin/ffprobe",
                "-v",
                "error",
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=codec_name",
                "-of",
                "csv=p=0",
                &format!("{REC_DIR}/audio.mka"),
            ]),
        )?;
        let o = &o[..o.len().min(4096)];
        if rc == 0 && o.split(|&c| c == b'\n').any(shell::grep_dot) {
            let vfirst = record_first_us(ctx, rec).unwrap_or_else(|| num(&rec.start));
            match audio_filter(ctx, vfirst) {
                Some(f) => afilt = f,
                None => {
                    afilt = b"asetpts=N/SR/TB,apad".to_vec();
                    say(
                        "record: could not tell when the sound started: it may be out of step with the picture",
                    );
                }
            }
        } else {
            say(cat!(
                "record: no sound came out (ffmpeg's log, ",
                REC_DIR,
                "/audio.log in box '",
                ctx.name,
                "', is kept): a video without it"
            ));
        }
    }
    let marks = if ctx.at("record.marks").is_file() {
        std::fs::read(ctx.at("record.marks"))
            .map(|v| strip_nl(&v))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    rm(&ctx.at("record.marks"));
    rm(&ctx.at("record.glide"));
    // The input it saw (#176): taken out of the box dir's name for it.
    let mut input = Vec::new();
    // (gone however the stop ends from here, by its trap, armed with the making: findings 432, 452,
    // #360)
    if ctx.at("record.input").is_file() {
        crate::trap::made(|| {
            if let Ok((_, t)) = shell::mktemp() {
                tmps.add(t.clone());
                if crate::shots::mv_f(shell::bytes(&ctx.at("record.input")), &t) {
                    input = t;
                }
            }
        });
    }
    let geo = if ctx.at("record.geo").is_file() {
        let d = std::fs::read(ctx.at("record.geo")).unwrap_or_default();
        if d.contains(&b'\n') {
            shell::read_fields(&d, 1)[0].clone()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    rm(&ctx.at("record.input"));
    rm(&ctx.at("record.geo"));
    // Its --no-pointer's hold given back (#228).
    let pf = std::fs::read(ctx.at("pointer")).unwrap_or_default();
    if pf
        .split(|&c| c == b'\n')
        .any(|l| l.starts_with(b"hold record "))
    {
        let mut p = crate::ptr::Ptr::default();
        if crate::ptr::ptr_change(ctx, &mut p, crate::ptr::Op::Release, b"record", false)?.is_err()
        {
            return Err(crate::ptr::ptr_fail(ctx, &p, b"record stop"));
        }
    }
    // The video's clock: its first frame's wall-clock time.
    let mut first = Vec::new();
    if !marks.is_empty() || so.show || so.caps || so.cut.is_some() {
        first = match record_first_us(ctx, rec) {
            Some(f) => f.to_string().into_bytes(),
            None => {
                say(
                    "record: could not tell when the first frame came: the marks are timed from record start (late by up to a few tenths of a second)",
                );
                rec.start.clone()
            }
        };
    }
    let input_s = !input.is_empty() && size_of(&shell::path(&input)).is_some_and(|s| s > 0);
    // --show-input, --captions (#176): drawn by the ass filter at the end of the first pass.
    let mut vass = Vec::new();
    if so.show || so.caps {
        if so.show && !input_s {
            say(
                "record --show-input: nothing to show (no click, scroll, pointer, drag or keys while it recorded)",
            );
        }
        if so.caps && marks.is_empty() {
            say("record --captions: no marks to show (record mark TEXT while it records)");
        }
        if (so.show && input_s) || (so.caps && !marks.is_empty()) {
            match record_ass(ctx, rec, &input, &marks, &first, &geo, so.show, so.caps)? {
                Some(v) => vass = v,
                None => say(
                    "record: could not draw the input or captions into the video: a video without them",
                ),
            }
        }
    }
    if rec.n > 1 {
        if !record_compose(ctx, rec, &afilt, &vass, so.cut)? {
            return Err(ctx.die(cat!(
                "record: ffmpeg could not make the video (the monitors' raw.mkv in ",
                REC_DIR,
                "/m1 to m",
                rec.n.to_string(),
                " in box '",
                ctx.name,
                "' are kept)"
            )));
        }
        rm(&ctx.at("record.layout"));
    } else {
        // A steady REC_FPS, the video's clock still wf-recorder's (finding 306).
        let mut a = w(&[
            "/usr/bin/ffmpeg",
            "-v",
            "error",
            "-y",
            "-i",
            &format!("{REC_DIR}/raw.mkv"),
        ]);
        if !afilt.is_empty() {
            a.extend(w(&["-i", &format!("{REC_DIR}/audio.mka")]));
        }
        a.push(b"-vf".to_vec());
        a.push(cat!("fps=", fps, ",", rec_yuv(), vass));
        a.extend(w(&[
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &rec_crf(so.cut, rec.crf),
        ]));
        a.extend(w(&REC_TAGS));
        if !afilt.is_empty() {
            a.extend(w(&["-map", "0:v:0", "-map", "1:a:0", "-af"]));
            a.push(afilt.clone());
            a.extend(w(&REC_AAC));
            a.push(b"-shortest".to_vec());
        }
        a.extend(w(&[
            "-movflags",
            "+faststart",
            &format!("{REC_DIR}/out.mp4"),
        ]));
        if rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0 != 0 {
            return Err(ctx.die(cat!(
                "record: ffmpeg could not make the video (",
                REC_DIR,
                "/raw.mkv in box '",
                ctx.name,
                "' is kept)"
            )));
        }
    }
    // --cut-idle (#176): its still stretches cut short, CUTS the frames taken out.
    let mut cuts = Vec::new();
    if let Some(c) = so.cut {
        match record_cut(ctx, rec, c, &first, &input, &marks, so.show, &afilt)? {
            Some(x) => cuts = x,
            None => say("record --cut-idle: could not cut it: the video as it was recorded"),
        }
    }
    if !input.is_empty() {
        rm(&shell::path(&input));
    }
    let (irc, info) = rec_ff_subst(
        ctx,
        &w(&[
            "/usr/bin/ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height:format=duration",
            "-of",
            "csv=p=0",
            &format!("{REC_DIR}/out.mp4"),
        ]),
    )?;
    // (`| head -c 4096 | paste -sd ' '`; its status not read)
    let info = if irc == 0 {
        &info[..info.len().min(4096)]
    } else {
        &b""[..]
    };
    let info: Vec<u8> = {
        let mut v: Vec<&[u8]> = info.split(|&c| c == b'\n').collect();
        if v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v.join(&b' ')
    };
    let mut dur: Option<i64> = None;
    if let Some(c) =
        shell::Re::new(r" ([0-9]+)(\.([0-9]{1,3}))?[0-9]*$").and_then(|r| r.captures(&info, false))
    {
        let g = |i: usize| {
            c.get(i)
                .copied()
                .flatten()
                .map(|(a, b)| info[a..b].to_vec())
                .unwrap_or_default()
        };
        let mut frac = g(3);
        while frac.len() < 3 {
            frac.push(b'0');
        }
        dur = Some(shell::dec(&g(1)).unwrap_or(0) * 1000 + shell::dec(&frac).unwrap_or(0));
    }
    let durb = dur.map(|d| d.to_string().into_bytes()).unwrap_or_default();
    let base = rec.file.strip_suffix(b".mp4").unwrap_or(&rec.file).to_vec();
    let max = rec_max();
    let outmp4 = format!("{REC_DIR}/out.mp4").into_bytes();
    if !so.tofd.is_empty() {
        // Streamed into the caller's file, as the bytes of the box's `cat`.
        let fd = num(&so.tofd) as i32;
        let ok = match dup_hi(fd) {
            Ok(d) => {
                let f = std::fs::File::from(d);
                let got = box_copy(ctx, &outmp4, max, &f);
                got.is_some_and(|s| size_of(&shell::path(&cat!("/dev/fd/", so.tofd))) == Some(s))
            }
            Err(_) => false,
        };
        if !ok {
            return Err(ctx.die(cat!(
                "record: could not copy the video out of box '",
                ctx.name,
                "' (into the caller's file; ",
                REC_DIR,
                "/out.mp4 there is kept)"
            )));
        }
    } else {
        let part = cat!(rec.file, ".part");
        tmps.set_part(&part);
        let ok = match std::fs::File::create(shell::path(&part)) {
            Ok(f) => {
                let got = box_copy(ctx, &outmp4, max, &f);
                got.is_some_and(|s| size_of(&shell::path(&part)) == Some(s))
            }
            Err(e) => {
                say(cat!("record: ", crate::shots::cant_write(&part, &e)));
                false
            }
        };
        if !ok {
            rm(&shell::path(&part));
            return Err(ctx.die(cat!(
                "record: could not copy the video out of box '",
                ctx.name,
                "' (",
                REC_DIR,
                "/out.mp4 there is kept)"
            )));
        }
        // (mv's failure said, 1, the .part gone with rec_traps: bash's set -e ended it unsaid but
        // for mv's own words, finding 442)
        if !crate::shots::mv_f(&part, &rec.file) {
            return Err(ctx.die(cat!("record: could not put the video at ", rec.file)));
        }
        tmps.set_part(b"");
    }
    if !so.tofd.is_empty() {
        // The marks and captions made in a folder of the box dir's own, then copied into the
        // caller's files.
        let tmpd = crate::trap::made(|| {
            let (_, t) = shell::mktemp_args(&[b"-d", &cat!(ctx.d, "/.record-XXXXXX")]).ok()?;
            tmps.add(t.clone());
            Some(t)
        })
        .ok_or_else(|| ctx.die(cat!("record: cannot write in ", ctx.d)))?;
        record_marks(
            ctx,
            &marks,
            &first,
            &durb,
            &cat!(tmpd, "/v"),
            &cuts,
            &base,
            &fps,
        )?;
        let tsv = cat!(tmpd, "/v.marks.tsv");
        if shell::path(&tsv).is_file() {
            // (`cat -- FILE >&FD`: a foreground child, as bash's)
            let put = |src: &[u8], fd: &[u8]| -> bool {
                let Ok(o) = dup_hi(num(fd) as i32) else {
                    return false;
                };
                let mut c = crate::sig::command("cat");
                c.arg("--")
                    .arg(std::ffi::OsStr::from_bytes(src))
                    .stdout(std::process::Stdio::from(o));
                crate::sig::run_status(c).is_ok_and(|rc| rc == 0)
            };
            if !put(&tsv, &so.mfd) || !put(&cat!(tmpd, "/v.vtt"), &so.vfd) {
                let _ = std::fs::remove_dir_all(shell::path(&tmpd));
                return Err(ctx.die("record: could not write the marks into the caller's files"));
            }
        }
        let _ = std::fs::remove_dir_all(shell::path(&tmpd));
    } else {
        record_marks(ctx, &marks, &first, &durb, &base, &cuts, &base, &fps)?;
    }
    let mut sp = Vec::new();
    if so.sheet {
        // (`sp=$(record_sheet ...) || sp=""`: a refusal in it ends only the substitution)
        sp = record_sheet(
            ctx,
            &outmp4,
            &cat!(base, ".marks.tsv"),
            &cat!(base, ".sheet.png"),
            &so.frames,
            so.every,
            &so.fit,
            &so.more,
            b"",
        )
        .ok()
        .flatten()
        .unwrap_or_default();
    }
    let mut rmd = w(&[
        format!("{REC_DIR}/raw.mkv"),
        format!("{REC_DIR}/out.mp4"),
        format!("{REC_DIR}/first-frame"),
        format!("{REC_DIR}/last-frame"),
        format!("{REC_DIR}/overlay.ass"),
        format!("{REC_DIR}/cut.mp4"),
    ]);
    if !afilt.is_empty() {
        rmd.extend(w(&[
            format!("{REC_DIR}/audio.mka"),
            format!("{REC_DIR}/audio.log"),
        ]));
    }
    if rec.n != 1 {
        rmd.extend(rec_dirs(rec));
    }
    let mut a = w(&["/usr/bin/rm", "-rf", "--"]);
    a.extend(rmd);
    sys_q(ctx, &a)?;
    if let Some(c) = shell::Re::new("^([0-9]+),([0-9]+) ").and_then(|r| r.captures(&info, false)) {
        let g = |i: usize| {
            c.get(i)
                .copied()
                .flatten()
                .map(|(a, b)| info[a..b].to_vec())
                .unwrap_or_default()
        };
        let d = if durb.is_empty() {
            b"0".to_vec()
        } else {
            durb.clone()
        };
        say(cat!(
            "record: ",
            g(1),
            "x",
            g(2),
            ", ",
            ms_s(&d),
            " at ",
            fps,
            " fps",
            match rec.crf {
                Some(0) =>
                    ", CRF 0 (lossless: H.264's High 4:4:4 Predictive, which browsers do not play)"
                        .to_string(),
                Some(c) => format!(", CRF {c}"),
                None => String::new(),
            },
            if afilt.is_empty() { "" } else { ", with sound" }
        ));
    }
    out(cat!(rec.file, "\n"))?;
    if !sp.is_empty() {
        out(cat!(sp, "\n"))?;
    }
    if so.sheet && sp.is_empty() {
        return Err(Exit(1));
    }
    Ok(())
}

/// record_audio_filter FIRST_US: the filter putting the sound in the video's clock (#226), from the
/// start ffmpeg's log says; None when it says none.
fn audio_filter(ctx: &mut Ctx, first: i64) -> Option<Vec<u8>> {
    let (_, a) = sys_cap(
        ctx,
        &w(&[
            "/usr/bin/sed",
            "-n",
            P::AUDIO_START_SED,
            &format!("{REC_DIR}/audio.log"),
        ]),
        Some(256),
        Err2::Null,
        false,
    );
    let a = a.split(|&c| c == b'\n').next().unwrap_or_default().to_vec();
    if !shell::re_match("^[0-9]{7,18}$", &a) {
        return None;
    }
    let off = shell::dec(&a).unwrap_or(0) - first;
    let ms = awk_begin(
        &[("o", off.abs().to_string().as_bytes())],
        r#"BEGIN { printf "%.3f", o / 1000 }"#,
    );
    if off >= 0 {
        Some(cat!("asetpts=N/SR/TB,adelay=delays=", ms, ":all=1,apad"))
    } else {
        let s = awk_begin(&[("m", &ms)], r#"BEGIN { printf "%.6f", m / 1000 }"#);
        Some(cat!(
            "asetpts=N/SR/TB,atrim=start=",
            s,
            ",asetpts=PTS-STARTPTS,apad"
        ))
    }
}

/// record_compose: the layout's recordings in one video (#225), in step by their first frames.
fn record_compose(
    ctx: &mut Ctx,
    rec: &Rec,
    afilt: &[u8],
    vass: &[u8],
    cut: Option<i64>,
) -> Res<bool> {
    let lay = std::fs::read(ctx.at("record.layout")).unwrap_or_default();
    if !lay.contains(&b'\n') {
        return Ok(false);
    }
    let mut lines: Vec<&[u8]> = lay.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let head = shell::read_fields(lines.first().copied().unwrap_or_default(), 3);
    let (ow, oh) = (num(&head[0]), num(&head[1]));
    let lay: Vec<&[u8]> = lines.into_iter().skip(1).collect();
    let dirs = rec_dirs(rec);
    let firsts = record_firsts(ctx, rec);
    let mut durs = Vec::new();
    for d in &dirs {
        let l = ff_line(
            ctx,
            &w(&[
                b"/usr/bin/ffprobe".as_slice(),
                b"-v",
                b"error",
                b"-show_entries",
                b"format=duration",
                b"-of",
                b"csv=p=0",
                &cat!(d, "/raw.mkv"),
            ]),
            256,
        )?;
        durs.push(l);
    }
    let n = dirs.len();
    if lay.len() != n || firsts.len() != n {
        say("record: the layout's recordings do not match its record");
        return Ok(false);
    }
    let Some(t0) = firsts.iter().flatten().map(|(m, _)| *m).max() else {
        say("record: no monitor gave a frame");
        return Ok(false);
    };
    let fps = String::from_utf8_lossy(&rec.fps).into_owned();
    let mut args = w(&[
        "-v".to_string(),
        "error".into(),
        "-y".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("color=c=black:s={}x{}:r={fps}", ow + ow % 2, oh + oh % 2),
    ]);
    // The black in the video's colours too (finding 312).
    let mut fc = format!("[0:v]format=yuv420p,{REC_TAGS_F}[bg];").into_bytes();
    let (mut k, mut last, mut end, mut left) = (0usize, b"bg".to_vec(), b"0".to_vec(), Vec::new());
    for i in 0..n {
        let l = shell::read_fields(lay[i], 5);
        let Some((m, _)) = firsts[i] else {
            if !left.is_empty() {
                left.extend_from_slice(b", ");
            }
            left.extend_from_slice(&l[0]);
            continue;
        };
        k += 1;
        let off = awk_begin(
            &[("o", (m - t0).to_string().as_bytes())],
            r#"BEGIN { printf "%.6f", o / 1e6 }"#,
        );
        let d = &durs[i];
        if shell::re_match("^[0-9]+(\\.[0-9]+)?$", d) {
            end = awk_begin(
                &[("e", &end), ("o", &off), ("d", d)],
                r#"BEGIN { x = o + d; printf "%.6f", (x > e ? x : e) }"#,
            );
        }
        args.push(b"-i".to_vec());
        args.push(cat!(dirs[i], "/raw.mkv"));
        fc.extend_from_slice(&cat!(
            "[",
            k.to_string(),
            ":v]setpts=PTS-STARTPTS+(",
            off,
            ")/TB,fps=",
            fps,
            ":start_time=0,scale=",
            l[3],
            ":",
            l[4],
            ":",
            REC_YUV_ARGS,
            ",",
            REC_TAGS_F,
            "[s",
            k.to_string(),
            "];"
        ));
        fc.extend_from_slice(&cat!(
            "[",
            last,
            "][s",
            k.to_string(),
            "]overlay=x=",
            l[1],
            ":y=",
            l[2],
            ":eof_action=pass[o",
            k.to_string(),
            "];"
        ));
        last = cat!("o", k.to_string());
    }
    if !left.is_empty() {
        say(cat!(
            "record: no frame came from ",
            left,
            ": black in the video"
        ));
    }
    // No length known (a wf-recorder killed): until the first to end.
    let mut tlim: Vec<Vec<u8>> = Vec::new();
    let positive = jq::tool(
        "awk",
        &[
            b"-v".to_vec(),
            cat!("e=", end),
            b"BEGIN { exit !(e > 0) }".to_vec(),
        ],
        None,
        jq::Err::Inherit,
    )
    .ok();
    if positive {
        tlim = vec![b"-t".to_vec(), end.clone()];
    } else {
        fc = replace_all(&fc, b"eof_action=pass", b"shortest=1");
    }
    let vass_tail = vass.strip_prefix(b",").unwrap_or(vass);
    fc.extend_from_slice(&cat!(
        "[",
        last,
        "]",
        vass_tail,
        if vass.is_empty() { "" } else { "," },
        REC_TAGS_F,
        "[v]"
    ));
    let mut amap: Vec<Vec<u8>> = Vec::new();
    if !afilt.is_empty() {
        args.extend(w(&["-i", &format!("{REC_DIR}/audio.mka")]));
        fc.extend_from_slice(&cat!(";[", (k + 1).to_string(), ":a]", afilt, "[a]"));
        amap = w(&["-map", "[a]"]);
        amap.extend(w(&REC_AAC));
        amap.push(b"-shortest".to_vec());
    }
    args.push(b"-filter_complex".to_vec());
    args.push(fc);
    args.extend(w(&[
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        &rec_crf(cut, rec.crf),
    ]));
    args.extend(w(&REC_TAGS));
    args.extend(amap);
    args.extend(tlim);
    args.extend(w(&[
        "-movflags",
        "+faststart",
        &format!("{REC_DIR}/out.mp4"),
    ]));
    let mut a = vec![b"/usr/bin/ffmpeg".to_vec()];
    a.extend(args);
    Ok(rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0 == 0)
}

fn replace_all(v: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if v[i..].starts_with(from) {
            o.extend_from_slice(to);
            i += from.len();
        } else {
            o.push(v[i]);
            i += 1;
        }
    }
    o
}

/// `sort ARGS` of INPUT (in byte order: sig::command), its output.
fn sort(args: &[&str], input: &[u8]) -> Vec<u8> {
    jq::tool("sort", args, Some(input), jq::Err::Inherit).out
}

/// record_marks MARKS FIRST_US DURATION_MS BASE CUTS SHOWN: the marks in the video's clock as
/// BASE.marks.tsv and BASE.vtt; no marks: no files, an earlier take's gone.
#[allow(clippy::too_many_arguments)]
fn record_marks(
    ctx: &Ctx,
    marks: &[u8],
    first: &[u8],
    dur: &[u8],
    base: &[u8],
    cuts: &[u8],
    shown: &[u8],
    fps: &[u8],
) -> Res<()> {
    let (tsv, vtt) = (cat!(base, ".marks.tsv"), cat!(base, ".vtt"));
    if marks.is_empty() {
        rm(&shell::path(&tsv));
        rm(&shell::path(&vtt));
        return Ok(());
    }
    let dur = if dur.is_empty() {
        b"0".to_vec()
    } else {
        dur.to_vec()
    };
    let fps = if fps.is_empty() {
        b"30".to_vec()
    } else {
        fps.to_vec()
    };
    let sorted = sort(&["-t", "\t", "-k1,1n"], &cat!(marks, "\n"));
    let r = jq::tool(
        "awk",
        &[
            b"-F".to_vec(),
            b"\t".to_vec(),
            b"-v".to_vec(),
            cat!("t0=", first),
            b"-v".to_vec(),
            cat!("cuts=", cuts),
            b"-v".to_vec(),
            cat!("fps=", fps),
            P::MARKS_AWK.as_bytes().to_vec(),
        ],
        Some(&sorted),
        jq::Err::Inherit,
    );
    let tpart = cat!(tsv, ".part");
    let wrote = std::fs::write(shell::path(&tpart), cat!("ms\ttime\ttext\n", r.out));
    if wrote.is_err() || !r.ok() {
        if let Err(e) = wrote {
            say(cat!("record: ", crate::shots::cant_write(&tpart, &e)));
        }
        rm(&shell::path(&tpart));
        return Err(ctx.die(cat!("record: cannot write ", tsv)));
    }
    if !crate::shots::mv_f(&tpart, &tsv) {
        return Err(ctx.die(cat!("record: cannot write ", tsv)));
    }
    let v = jq::tool(
        "awk",
        &[
            b"-F".to_vec(),
            b"\t".to_vec(),
            b"-v".to_vec(),
            cat!("dur=", dur),
            P::VTT_AWK.as_bytes().to_vec(),
            tsv.clone(),
        ],
        None,
        jq::Err::Inherit,
    );
    let vpart = cat!(vtt, ".part");
    let wrote = std::fs::write(shell::path(&vpart), &v.out);
    if wrote.is_err() || !v.ok() {
        if let Err(e) = wrote {
            say(cat!("record: ", crate::shots::cant_write(&vpart, &e)));
        }
        rm(&shell::path(&vpart));
        return Err(ctx.die(cat!("record: cannot write ", vtt)));
    }
    if !crate::shots::mv_f(&vpart, &vtt) {
        return Err(ctx.die(cat!("record: cannot write ", vtt)));
    }
    let n = std::fs::read(shell::path(&tsv))
        .map(|d| d.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0) as i64
        - 1;
    say(cat!(
        "record: ",
        n.to_string(),
        " mark",
        if n == 1 { "" } else { "s" },
        " in ",
        shown,
        ".marks.tsv (ms in the video), and ",
        shown,
        ".vtt (captions)"
    ));
    Ok(())
}

/// record_ass INPUT MARKS FIRST_US GEO SHOW CAPTIONS: the overlay script in the box as
/// REC_DIR/overlay.ass; Some(the filter that draws it), None when it cannot (said by the caller).
#[allow(clippy::too_many_arguments)]
fn record_ass(
    ctx: &mut Ctx,
    rec: &Rec,
    input: &[u8],
    marks: &[u8],
    first: &[u8],
    geo: &[u8],
    show: bool,
    caps: bool,
) -> Res<Option<Vec<u8>>> {
    let g = shell::read_fields(geo, 8);
    let ok = shell::re_match("^-?[0-9]{1,6}$", &g[0])
        && shell::re_match("^-?[0-9]{1,6}$", &g[1])
        && shell::re_match("^[0-9]{1,3}(\\.[0-9]{1,12})?$", &g[2])
        && shell::is_num(first);
    if !ok {
        return Ok(None);
    }
    let (vw, vh) = if rec.n > 1 {
        let s = record_size(ctx);
        let i = s.iter().position(|&c| c == b'x').unwrap_or(0);
        (s[..i].to_vec(), s.get(i + 1..).unwrap_or_default().to_vec())
    } else {
        let l = ff_line(
            ctx,
            &w(&[
                "/usr/bin/ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
                &format!("{REC_DIR}/raw.mkv"),
            ]),
            256,
        )?;
        let Some(c) = shell::Re::new("^([1-9][0-9]{0,4}),([1-9][0-9]{0,4})$")
            .and_then(|r| r.captures(&l, false))
        else {
            return Ok(None);
        };
        let gg = |i: usize| {
            c.get(i)
                .copied()
                .flatten()
                .map(|(a, b)| l[a..b].to_vec())
                .unwrap_or_default()
        };
        (gg(1), gg(2))
    };
    // (removed however this ends, armed with the making: #360)
    let Some((tmp, _tmp_g)) = crate::trap::made(|| {
        let (_, tmp) = shell::mktemp().ok()?;
        let t = tmp.clone();
        Some((tmp, crate::trap::on_exit(move || rm(&shell::path(&t)))))
    }) else {
        return Ok(None);
    };
    let area0 = cat!(g[3], " ", g[4], " ", g[5], " ", g[6]);
    let area = if shell::re_match(
        "^[0-9]{1,5} [0-9]{1,5} [1-9][0-9]{0,4} [1-9][0-9]{0,4}$",
        &area0,
    ) {
        area0
    } else {
        cat!("0 0 ", vw, " ", vh)
    };
    let mut ok = match record_overlay(
        input,
        marks,
        first,
        &g[0],
        &g[1],
        &g[2],
        &vw,
        &vh,
        b"359999990",
        show,
        caps,
        &area,
    ) {
        Some(s) => std::fs::write(shell::path(&tmp), s).is_ok(),
        None => false,
    };
    if ok {
        let f = std::fs::File::open(shell::path(&tmp)).ok();
        ok = match f {
            Some(f) => {
                let fd = OwnedFd::from(f);
                write_in_box(
                    ctx,
                    P::WRITE_SH,
                    "/usr/bin/sh",
                    &format!("{REC_DIR}/overlay.ass"),
                    fd,
                )? == 0
            }
            None => false,
        };
    }
    rm(&shell::path(&tmp));
    if !ok {
        return Ok(None);
    }
    Ok(Some(format!(",ass={REC_DIR}/overlay.ass").into_bytes()))
}

/// `box_sys SHELL -c SCRIPT omabox-record PATH < FD`: its status.
fn write_in_box(ctx: &mut Ctx, script: &str, sh: &str, path: &str, fd: OwnedFd) -> Res<i32> {
    let a = w(&[sh, "-c", script, "omabox-record", path]);
    // (`< FILE`: the file the command's stdin; box_exec's refusal ends the command, its die)
    box_exec(
        ctx,
        &a,
        ExecOpts {
            sys: true,
            stdin: Some(fd.as_raw_fd()),
            ..ExecOpts::default()
        },
    )
}

/// record_overlay: the ASS script drawing peek's marks at the times the input happened (bin/omabox's
/// awk, verbatim, in C.UTF-8). None when awk fails.
#[allow(clippy::too_many_arguments)]
pub fn record_overlay(
    input: &[u8],
    marks: &[u8],
    first: &[u8],
    x0: &[u8],
    y0: &[u8],
    k: &[u8],
    vw: &[u8],
    vh: &[u8],
    dur: &[u8],
    show: bool,
    caps: bool,
    area: &[u8],
) -> Option<Vec<u8>> {
    let mut feed = Vec::new();
    if !input.is_empty() && shell::path(input).is_file() && show {
        let mut c = crate::sig::command("sort");
        c.args(["-s", "-n", "-k1,1", "--"])
            .arg(std::ffi::OsStr::from_encoded_bytes_checked(input))
            .stdin(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        let (_, o) = crate::sig::run_output(c).ok()?;
        feed.extend(o);
    }
    if caps && !marks.is_empty() {
        let s = sort(&["-s", "-t", "\t", "-k1,1n"], &cat!(marks, "\n"));
        for l in s.split_inclusive(|&c| c == b'\n') {
            feed.extend_from_slice(b"M ");
            feed.extend_from_slice(l);
        }
    }
    let mut c = crate::sig::command("awk");
    c.env("LC_ALL", "C.UTF-8");
    let vars: [(&str, &[u8]); 9] = [
        ("first", first),
        ("x0", x0),
        ("y0", y0),
        ("k", k),
        ("W", vw),
        ("H", vh),
        ("dur", dur),
        ("area", area),
        ("", b""),
    ];
    for (n, v) in vars.iter().take(8) {
        c.arg("-v");
        c.arg(std::ffi::OsStr::from_encoded_bytes_checked(&cat!(
            n, "=", v
        )));
    }
    c.arg(P::OVERLAY_AWK);
    // (typed text a character each, as UTF-8 whatever the caller's locale: gawk's length and substr
    // follow LC_CTYPE, so under C an é was two pieces of a byte each, finding 447)
    c.env("LC_ALL", "C.UTF-8");
    c.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    // (a foreground child, sig::run_child: fed, read, its end recorded)
    let (out, (rc, _)) = crate::sig::run_child(c, false, |ch| {
        let feeder = ch.stdin.take().map(|si| crate::exec::feed(si.into(), feed));
        let mut out = Vec::new();
        if let Some(mut o) = ch.stdout.take() {
            let _ = std::io::Read::read_to_end(&mut o, &mut out);
        }
        if let Some(f) = feeder {
            let _ = f.join();
        }
        out
    })
    .ok()?;
    (rc == 0).then_some(out)
}

/// OsStr from bytes (Unix).
trait FromBytes {
    fn from_encoded_bytes_checked(b: &[u8]) -> &std::ffi::OsStr;
}

impl FromBytes for std::ffi::OsStr {
    fn from_encoded_bytes_checked(b: &[u8]) -> &std::ffi::OsStr {
        std::os::unix::ffi::OsStrExt::from_bytes(b)
    }
}

/// record_cut CUT_MS FIRST_US INPUT MARKS SHOW (finding 319): every still stretch longer than CUT_MS
/// cut to it. Some(the cuts, "FROM TO" frames a line), None when it could not.
#[allow(clippy::too_many_arguments)]
fn record_cut(
    ctx: &mut Ctx,
    rec: &Rec,
    cut: i64,
    first: &[u8],
    input: &[u8],
    marks: &[u8],
    show: bool,
    afilt: &[u8],
) -> Res<Option<Vec<u8>>> {
    let outmp4 = format!("{REC_DIR}/out.mp4");
    let nb = ff_line(
        ctx,
        &w(&[
            "/usr/bin/ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_packets",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
            &outmp4,
        ]),
        256,
    )?;
    if !shell::re_match("^[1-9][0-9]{0,8}$", &nb) {
        return Ok(None);
    }
    let cut_b = cut.to_string().into_bytes();
    let d = awk_begin(&[("m", &cut_b)], r#"BEGIN { printf "%.3f", m / 1000 }"#);
    // What ffmpeg says, a value a line at most: the host reads no more than that, and checks each.
    let grep_o = |ctx: &mut Ctx, a: &[Vec<u8>], re: &str| -> Res<Vec<u8>> {
        // (`fz=$(rec_ff ... | grep -oE RE | head ...)`: the grep in the substitution too)
        let (rc, g) = rec_ff(ctx, Ff::Err, a).map(|(rc, o)| {
            (
                rc,
                jq::tool("grep", &["-oE", re], Some(&o), jq::Err::Inherit),
            )
        })?;
        if rc != 0 || !g.ok() {
            return Ok(Vec::new());
        }
        let lines: Vec<&[u8]> = g
            .out
            .split_inclusive(|&c| c == b'\n')
            .take(20_000)
            .collect();
        Ok(strip_nl(&lines.concat()))
    };
    let fz = grep_o(
        ctx,
        &w(&[
            "/usr/bin/ffmpeg".to_string(),
            "-nostdin".into(),
            "-hide_banner".into(),
            "-nostats".into(),
            "-i".into(),
            outmp4.clone(),
            "-map".into(),
            "0:v:0".into(),
            "-vf".into(),
            format!("freezedetect=n=-60dB:d={}", String::from_utf8_lossy(&d)),
            "-f".into(),
            "null".into(),
            "-".into(),
        ]),
        r"freeze_(start|end): [0-9]{1,9}(\.[0-9]{1,9})?",
    )?;
    let mut sl = Vec::new();
    if !afilt.is_empty() {
        sl = grep_o(
            ctx,
            &w(&[
                "/usr/bin/ffmpeg",
                "-nostdin",
                "-hide_banner",
                "-nostats",
                "-i",
                &outmp4,
                "-map",
                "0:a:0",
                "-af",
                "silencedetect=n=-50dB:d=0.1",
                "-f",
                "null",
                "-",
            ]),
            r"silence_(start|end): -?[0-9]{1,9}(\.[0-9]{1,9})?",
        )?;
    }
    // The times something happened, ms in the video ("A FROM TO"): the input, the marks.
    // (`act=$( { awk ...; awk ...; } )`)
    let act = {
        let mut act = Vec::new();
        let input_s = !input.is_empty() && size_of(&shell::path(input)).is_some_and(|s| s > 0);
        if input_s {
            let x: &[u8] = if show { b"3000" } else { b"0" };
            act.extend(
                jq::tool(
                    "awk",
                    &[
                        b"-v".to_vec(),
                        cat!("f=", first),
                        b"-v".to_vec(),
                        cat!("x=", x),
                        P::CUT_INPUT_AWK.as_bytes().to_vec(),
                        input.to_vec(),
                    ],
                    None,
                    jq::Err::Inherit,
                )
                .out,
            );
        }
        if !marks.is_empty() {
            act.extend(
                jq::tool(
                    "awk",
                    &[
                        b"-F".to_vec(),
                        b"\t".to_vec(),
                        b"-v".to_vec(),
                        cat!("f=", first),
                        P::CUT_MARKS_AWK.as_bytes().to_vec(),
                    ],
                    Some(&cat!(marks, "\n")),
                    jq::Err::Inherit,
                )
                .out,
            );
        }
        act
    };
    let act = strip_nl(&act);
    // (`cuts=$( { ...; sed ...; sed ...; } | awk ... | sort -n -u)`)
    let fps = rec.fps.clone();
    let cuts = {
        let sedp = |v: &[u8], prog: &str| {
            jq::tool("sed", &["-n", prog], Some(&cat!(v, "\n")), jq::Err::Inherit).out
        };
        let mut feed = cat!(act, "\n");
        feed.extend(sedp(&fz, r"s/^freeze_\(start\|end\): /F\1 /p"));
        feed.extend(sedp(&sl, r"s/^silence_\(start\|end\): /S\1 /p"));
        let cuts_raw = jq::tool(
            "awk",
            &[
                b"-v".to_vec(),
                cat!("fps=", fps),
                b"-v".to_vec(),
                cat!("nb=", nb),
                b"-v".to_vec(),
                cat!("cut=", cut_b),
                P::CUT_AWK.as_bytes().to_vec(),
            ],
            Some(&feed),
            jq::Err::Inherit,
        )
        .out;
        strip_nl(&sort(&["-n", "-u"], &cuts_raw))
    };
    if cuts.is_empty() {
        say(cat!(
            "record --cut-idle: no still stretch longer than ",
            ms_s(&cut_b)
        ));
        return Ok(Some(Vec::new()));
    }
    let frac = |f: i64| {
        awk_begin(
            &[("f", f.to_string().as_bytes()), ("r", &fps)],
            r#"BEGIN { printf "%.6f", f / r }"#,
        )
    };
    let (mut sel, mut alist, mut ac, mut rcuts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut n, mut p, mut tot) = (0usize, 0i64, 0i64);
    for l in cuts.split(|&c| c == b'\n') {
        let f = shell::read_fields(l, 2);
        let (mut c1, c2) = (num(&f[0]), num(&f[1]));
        if c1 < p {
            c1 = p;
        }
        if c2 <= c1 {
            continue;
        }
        if !sel.is_empty() {
            sel.push(b'+');
        }
        sel.extend_from_slice(format!("between(n,{c1},{})", c2 - 1).as_bytes());
        if !afilt.is_empty() {
            ac.extend_from_slice(&cat!(
                "[0:a]atrim=start=",
                frac(p),
                ":end=",
                frac(c1),
                ",asetpts=PTS-STARTPTS[a",
                n.to_string(),
                "];"
            ));
            alist.extend_from_slice(&cat!("[a", n.to_string(), "]"));
        }
        n += 1;
        tot += c2 - c1;
        p = c2;
        rcuts.extend_from_slice(format!("{c1} {c2}\n").as_bytes());
    }
    if n == 0 {
        return Ok(Some(Vec::new()));
    }
    let mut fc = cat!("[0:v]select='not(", sel, ")',setpts=N/FRAME_RATE/TB[v]");
    let mut amap: Vec<Vec<u8>> = Vec::new();
    if !afilt.is_empty() {
        ac.extend_from_slice(&cat!(
            "[0:a]atrim=start=",
            frac(p),
            ",asetpts=PTS-STARTPTS[a",
            n.to_string(),
            "];"
        ));
        alist.extend_from_slice(&cat!("[a", n.to_string(), "]"));
        fc.extend_from_slice(&cat!(
            ";",
            ac,
            alist,
            "concat=n=",
            (n + 1).to_string(),
            ":v=0:a=1[a]"
        ));
        amap = w(&["-map", "[a]"]);
        amap.extend(w(&REC_AAC));
        amap.push(b"-shortest".to_vec());
    }
    let mut a = w(&[
        "/usr/bin/ffmpeg",
        "-v",
        "error",
        "-y",
        "-i",
        &outmp4,
        "-filter_complex",
    ]);
    a.push(fc);
    a.extend(w(&[
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "medium",
        "-crf",
        &rec.crf.unwrap_or(REC_CRF).to_string(),
    ]));
    a.extend(w(&REC_TAGS));
    a.extend(amap);
    a.extend(w(&[
        "-movflags",
        "+faststart",
        &format!("{REC_DIR}/cut.mp4"),
    ]));
    if rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0 != 0 {
        return Ok(None);
    }
    let mv = w(&[
        "/usr/bin/mv",
        "-f",
        "--",
        &format!("{REC_DIR}/cut.mp4"),
        &outmp4,
    ]);
    if sys_run(ctx, &mv, false, Err2::Inherit, false)? != 0 {
        return Ok(None);
    }
    let fpsn = num(&fps).max(1);
    say(cat!(
        "record: ",
        n.to_string(),
        " still stretch",
        if n == 1 { "" } else { "es" },
        " cut to ",
        ms_s(&cut_b),
        " (",
        ms_s((tot * 1000 / fpsn).to_string().as_bytes()),
        " out)"
    ));
    Ok(Some(rcuts))
}

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
    let bv = format!("{REC_DIR}/sheet-{}.mp4", std::process::id()).into_bytes();
    let max = rec_max();
    let (tsv, outp, tmpd);
    let mut size: i64;
    if !so.fromfd.is_empty() {
        size = fd_size(&so.fromfd).unwrap_or(-1);
        if size < 0 || size > max {
            let s = if size < 0 { 0 } else { size };
            return Err(ctx.die(cat!(
                "record sheet: the video is ",
                bytes_h(s.to_string().as_bytes()),
                ": more than omabox copies into a box (",
                bytes_h(max.to_string().as_bytes()),
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
        tsv = cat!(base, ".marks.tsv");
        let f = std::fs::File::open(shell::path(&so.video));
        let ok = match f {
            Ok(f) => copy_in(ctx, &bv, InFrom::File(f))? == 0,
            Err(e) => {
                say(cat!(
                    "record: cannot read ",
                    so.video,
                    ": ",
                    crate::shots::why(&e)
                ));
                false
            }
        };
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
        size = size_of(&shell::path(&so.video)).unwrap_or(0);
    }
    // Its ffprobe's bound by its size, at 10 MiB/s, as box_copy's.
    if !(0..=999_999_999_999_999).contains(&size) {
        size = 0;
    }
    FF_SECS.store(REC_FF_FLOOR + size / 10_485_760, Ordering::SeqCst);
    let sp = record_sheet(
        ctx, &bv, &tsv, &outp, &so.frames, so.every, &so.fit, &so.more, &so.as_,
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

enum InFrom {
    File(std::fs::File),
    Fd(OwnedFd, i64),
}

/// `box_sys /usr/bin/bash -c 'mkdir -p "${1%/*}" && cat > "$1"' omabox-record PATH` fed the video
/// (`< VIDEO`, or `head -c SIZE <&FD |`): its status (pipefail's).
fn copy_in(ctx: &mut Ctx, bv: &[u8], from: InFrom) -> Res<i32> {
    match from {
        InFrom::File(f) => write_in_box(
            ctx,
            P::WRITE_SH,
            "/usr/bin/bash",
            &String::from_utf8_lossy(bv),
            OwnedFd::from(f),
        ),
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
fn record_sheet(
    ctx: &mut Ctx,
    v: &[u8],
    tsv: &[u8],
    png: &[u8],
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
    let (rc, j) = rec_ff(
        ctx,
        Ff::Out,
        &w(&[
            b"/usr/bin/ffprobe".as_slice(),
            b"-v",
            b"error",
            b"-select_streams",
            b"v:0",
            b"-count_packets",
            b"-show_entries",
            b"stream=nb_read_packets,r_frame_rate,width,height",
            b"-of",
            b"json",
            v,
        ]),
    )?;
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
    let bd = format!("{REC_DIR}/sheet-{}", std::process::id());
    let made = sys_cap(
        ctx,
        &w(&["/usr/bin/bash", "-c", P::SHEETDIR_SH, "omabox-record", &bd]),
        None,
        Err2::Inherit,
        false,
    )
    .0 == 0
        && rec_ff(
            ctx,
            Ff::Plain { to_err: true },
            &w(&[
                b"/usr/bin/ffmpeg".as_slice(),
                b"-v",
                b"error",
                b"-i",
                v,
                b"-vf",
                format!("select='{}'{crop},scale={tw}:-2{up}", sel.join("+")).as_bytes(),
                b"-fps_mode",
                b"passthrough",
                format!("{bd}/t-%03d.png").as_bytes(),
            ]),
        )?
        .0 == 0;
    let rm_bd = |ctx: &mut Ctx| {
        let _ = sys_cap(
            ctx,
            &w(&["/usr/bin/rm", "-rf", "--", &bd]),
            None,
            Err2::Null,
            false,
        );
    };
    if !made {
        rm_bd(ctx);
        say("record: the contact sheet: ffmpeg could not take the frames");
        return Ok(None);
    }
    // The frames copied out into a folder of the box dir's (gone with the box, and on any end of
    // this one), each at most 16 MiB, all of them 256.
    // (its own trap, in its subshell: the main shell's ends with the signal unless a code trap of
    // its own, rec_traps, is on; armed with the making: #360)
    let Some((tmp, tmp_g)) = crate::trap::made(|| {
        let (_, tmp) = shell::mktemp_args(&[b"-d", &cat!(ctx.d, "/.record-XXXXXX")]).ok()?;
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
    let mut total = 0i64;
    let mut ms: Vec<i64> = Vec::new();
    let mut lab: Vec<Vec<u8>> = Vec::new();
    for (i, k) in ks.iter().enumerate() {
        let f = format!("{}/{:03}.png", String::from_utf8_lossy(&tmp), i + 1);
        let src = format!("{bd}/t-{:03}.png", i + 1);
        let got = std::fs::File::create(&f).ok().and_then(|fh| {
            let _e = crate::exec::Redir::null(2);
            crate::exit::muted(|| box_copy(ctx, src.as_bytes(), 16_777_216, &fh))
        });
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
    if shell::path(tsv).is_file() {
        let d = std::fs::read(shell::path(tsv)).unwrap_or_default();
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
    for (i, l) in lab
        .iter()
        .take(usize::try_from(cnt).unwrap_or(0))
        .enumerate()
    {
        let l = replace_all(&replace_all(l, b"\\", b"\\\\"), b"%", b"%%");
        args.push(b"-label".to_vec());
        args.push(l);
        args.push(format!("png:{}/{:03}.png", String::from_utf8_lossy(&tmp), i + 1).into_bytes());
    }
    let part = cat!(png, ".part");
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
        &cat!("PNG:", part),
    ]));
    let mut err = !crate::shots::mkdir_p(&crate::shots::dirname(png))
        || !jq::tool("magick", &args, None, jq::Err::Null).ok();
    if !err && !fit.is_empty() {
        err = !jq::tool(
            "magick",
            &w(&[
                cat!("PNG:", part),
                b"-resize".to_vec(),
                cat!(fit, "x", fit, ">"),
                b"-depth".to_vec(),
                b"8".to_vec(),
                cat!("PNG:", part),
            ]),
            None,
            jq::Err::Null,
        )
        .ok();
    }
    drop(tmp_g);
    if err {
        rm(&shell::path(&part));
        say(cat!(
            "record: the contact sheet failed (magick montage, to ",
            png,
            ")"
        ));
        return Ok(None);
    }
    crate::shots::mv_f(&part, png);
    let png = crate::repo::readlink_f(png).unwrap_or_default();
    let sz = crate::shots::png_size(&png)
        .map(|(a, b)| format!("{a}x{b}").into_bytes())
        .unwrap_or_default();
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
    v: &[u8],
    kf: i64,
    kt: i64,
    geo: Option<(i64, i64, i64, i64)>,
) -> Res<Option<Vec<i64>>> {
    let (rc, o) = rec_ff(
        ctx,
        Ff::Out,
        &w(&[
            b"/usr/bin/ffmpeg".as_slice(),
            b"-nostdin",
            b"-hide_banner",
            b"-nostats",
            b"-v",
            b"error",
            b"-i",
            v,
            b"-map",
            b"0:v:0",
            b"-vf",
            changed_filter(kf, kt, geo).as_bytes(),
            b"-f",
            b"null",
            b"-",
        ]),
    )?;
    if rc != 0 {
        return Ok(None);
    }
    Ok(Some(changed_frames(&o, kf, kt)))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn bounds() {
        assert_eq!(ff_bound(0, 0), 60);
        assert_eq!(ff_bound(30, 1920 * 1080), 65);
        // (t_unit_rec_ff's: 60 s, and 5 s a second of 1080p at 30 fps)
        assert_eq!(
            (ff_bound(1, 1), ff_bound(1800, 2_073_600)),
            (61, 360),
            "the bound: 60 s, and 5 s a second of 1080p at 30 fps"
        );
        assert_eq!(rec_crf(Some(1), None), "12");
        assert_eq!(rec_crf(None, None), "18");
        // --crf (#278): the video's, the first pass of a cut 12 at most.
        assert_eq!(rec_crf(None, Some(8)), "8");
        assert_eq!(rec_crf(Some(1), Some(8)), "8");
        assert_eq!(rec_crf(Some(1), Some(30)), "12");
        assert_eq!(super::super::record::raw_crf(None), 14);
        assert_eq!(super::super::record::raw_crf(Some(8)), 8);
        assert_eq!(super::super::record::raw_crf(Some(30)), 14);
        assert_eq!(replace_all(b"a=pass,b=pass", b"pass", b"x"), b"a=x,b=x");
    }

    // t_unit_record's record_marks: wall-clock µs to ms from the first frame (in order; one before
    // it at 0), the wall-clock time as events has it, WebVTT cues from each to the next (the last to
    // the end), its text escaped; none, and an earlier take's files go; with a cut, the marks shifted.
    #[test]
    #[ignore = "runs awk: run by test/run.sh (cargo test -- --ignored)"]
    fn record_marks_as_bash() {
        let d = std::env::temp_dir().join(format!("omabox-rmarks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let rb = shell::bytes(&d.join("take")).to_vec();
        let mut ctx = Ctx::new(b"record", Vec::new());
        ctx.parsed();
        let marks = |m: &str, first: &str, dur: &str, cuts: &str| {
            let (r, _) = crate::exit::heard(|| {
                record_marks(
                    &ctx,
                    m.as_bytes(),
                    first.as_bytes(),
                    dur.as_bytes(),
                    &rb,
                    cuts.as_bytes(),
                    &rb,
                    b"",
                )
            });
            assert!(r.is_ok());
        };
        let lines = |f: &str, sep: &str| -> String {
            std::fs::read_to_string(d.join(f))
                .unwrap()
                .lines()
                .collect::<Vec<_>>()
                .join(sep)
        };
        marks(
            "1000002500000\tscene two\n1000000500400\tscene <one> & more\n999999900000\tearly",
            "1000000000000",
            "4000",
            "",
        );
        assert_eq!(
            lines("take.marks.tsv", "|").replace('\t', " "),
            "ms time text|0 999999.900 early|500 1000000.500 scene <one> & more|2500 1000002.500 scene two",
            "record_marks: the TSV"
        );
        assert_eq!(
            lines("take.vtt", "|"),
            "WEBVTT||1|00:00:00.000 --> 00:00:00.500|early||2|00:00:00.500 --> 00:00:02.500|scene &lt;one&gt; &amp; more||3|00:00:02.500 --> 00:00:04.000|scene two",
            "...the WebVTT"
        );
        marks("", "", "0", "");
        assert!(
            !d.join("take.marks.tsv").exists() && !d.join("take.vtt").exists(),
            "...no marks: an earlier take's files go"
        );
        marks(
            "1000002500000\tafter\n1000000500000\tbefore\n1000001100000\tin",
            "1000000000000",
            "4000",
            "30 60\n",
        );
        let cut: Vec<String> = std::fs::read_to_string(d.join("take.marks.tsv"))
            .unwrap()
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                format!("{} {}", f[0], f[2])
            })
            .collect();
        assert_eq!(
            cut.join("|"),
            "ms text|500 before|1000 in|1500 after",
            "record_marks with a cut (frames 30 to 60, 1 s at 30 fps): one after it 1 s earlier, one in it at its start"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // t_jail_record's (#224 re-check): copies out of a box are capped. A fallocated 64 MiB file
    // (blocks the box reserved, as its stat gives them) is no bound: refused past the cap, said; a
    // file swapped for a FIFO after its stat is read under a time for its size, not for ever.
    #[test]
    fn box_copy_is_capped() {
        if !crate::exec::tools_or_skip("box_copy_is_capped", &["awk"]) {
            return;
        }
        let mut ctx = Ctx::new(b"record", Vec::new());
        ctx.name = b"jr".to_vec();
        let (n, said) = crate::exit::heard(|| {
            copy_size(
                &ctx,
                b"/home/sbx/big",
                1_048_576,
                b"regular file|67108864|131072|512\n",
            )
        });
        assert_eq!(n, None);
        assert!(
            said.contains("is 64.0 MiB: more than omabox copies out of a box (1.0 MiB)"),
            "a fallocated 64 MiB file past box_copy's cap: refused, said: {said}"
        );
        let (n, said) = crate::exit::heard(|| {
            copy_size(
                &ctx,
                b"/home/sbx/sparse",
                1 << 40,
                b"regular file|67108864|8|512\n",
            )
        });
        assert_eq!(n, None, "...a sparse one: refused");
        assert!(said.contains("is sparse"), "{said}");
        let (n, said) =
            crate::exit::heard(|| copy_size(&ctx, b"/home/sbx/fifo", 100, b"fifo|0|0|512\n"));
        assert_eq!(n, None, "...one that is no regular file: refused");
        assert!(said.contains("is not a file"), "{said}");
        assert_eq!(
            copy_size(&ctx, b"/home/sbx/fifo", 100, b"regular file|10|8|512\n"),
            Some(10),
            "a file the stat called 10 bytes: copied as 10"
        );
        assert_eq!(
            copy_argv(b"/home/sbx/fifo", 10).join(&b' '),
            b"/usr/bin/timeout -k 1 10 /usr/bin/cat -- /home/sbx/fifo",
            "...by a cat given 10 s (a FIFO there is not read for ever)"
        );
        assert_eq!(
            copy_argv(b"f", 104_857_600)[3],
            b"20",
            "...and a second more a 10 MiB"
        );
    }

    // S7, end to end (#324 wave 6-flip: t_jail_record's live FIFO case went with lib): a file the
    // box's stat called regular and 10 bytes, a FIFO with no writer by the time the cat opens it, is
    // read under the time its size gives (10 s, killed a second after), never for ever: nothing
    // copied, None, and back in that time. Stat and cat run on the host, as the box's would.
    #[test]
    #[ignore = "starts processes and waits 10 s: run by test/run.sh (cargo test -- --ignored)"]
    fn box_copy_bounds_a_fifo() {
        use std::os::fd::FromRawFd;
        let d = std::env::temp_dir().join(format!("omabox-boxcopy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let fifo = d.join("video.mp4");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: mkfifo with a NUL-terminated path of ours.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let to = std::fs::File::create(d.join("out")).unwrap();
        let mut ctx = Ctx::new(b"record", Vec::new());
        let mut argvs = Vec::new();
        let t0 = std::time::Instant::now();
        let got = box_copy_with(
            &mut ctx,
            fifo.as_os_str().as_bytes(),
            100,
            &to,
            |_, a, fd| {
                argvs.push(a[0].clone());
                match fd {
                    None => b"regular file|10|8|512\n".to_vec(),
                    Some(fd) => {
                        let mut cmd =
                            std::process::Command::new(std::ffi::OsStr::from_bytes(&a[0]));
                        cmd.args(a[1..].iter().map(|x| std::ffi::OsStr::from_bytes(x)));
                        // SAFETY: dup of an fd this test holds open for the call.
                        let out = unsafe { std::os::fd::OwnedFd::from_raw_fd(libc::dup(fd)) };
                        let st = cmd.stdout(out).stdin(std::process::Stdio::null()).status();
                        if st.is_ok_and(|s| s.success()) {
                            b"0".to_vec()
                        } else {
                            b"1".to_vec()
                        }
                    }
                }
            },
        );
        let took = t0.elapsed();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(
            argvs,
            [b"/usr/bin/stat".to_vec(), b"/usr/bin/timeout".to_vec()]
        );
        assert_eq!(got, None, "a FIFO with no writer: nothing copied");
        assert!(
            took >= std::time::Duration::from_secs(9) && took <= std::time::Duration::from_secs(14),
            "...cut at its size's time (10 s), not read for ever: {took:?}"
        );
    }

    // t_unit_rec_ff (finding 356): rec_ff on commands that only wait, each run here on the host
    // (bash's box_sys stub), its bound 1 s (OMABOX_RECORD_FF_SECS): a run out of time is run once
    // more, then said, exit 124; stuck once: run again; a failure that is not a timeout: not again.
    #[test]
    #[ignore = "starts processes: run by test/run.sh (cargo test -- --ignored)"]
    fn rec_ff_runs_once_more() {
        let host = |_: &mut Ctx, a: &[Vec<u8>]| -> Res<(i32, Vec<u8>)> {
            let o = std::process::Command::new(std::ffi::OsStr::from_bytes(&a[0]))
                .args(a[1..].iter().map(|x| std::ffi::OsStr::from_bytes(x)))
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            Ok((o.status.code().unwrap_or(128), o.stdout))
        };
        let ff = |mode: Ff, a: &[&str]| {
            let mut ctx = Ctx::new(b"record", Vec::new());
            let a: Vec<Vec<u8>> = a.iter().map(|x| x.as_bytes().to_vec()).collect();
            let (r, said) = crate::exit::heard(|| {
                shell::with_env(&[("OMABOX_RECORD_FF_SECS", Some("1"))], || {
                    rec_ff_with(&mut ctx, mode, &a, host)
                })
            });
            let (rc, out) = r.unwrap();
            format!("{said}{}rc={rc}", String::from_utf8_lossy(&out))
        };
        let t0 = std::time::Instant::now();
        assert_eq!(
            ff(Ff::Out, &["/usr/bin/sh", "-c", "echo x; exec sleep 30"]),
            "omabox: record: sh did not finish in 1 s (stuck: finding 356): killed, run once more\nomabox: record: sh did not finish in 1 s again: killed\nx\nrc=124",
            "rec_ff: a run out of time is run once more, then said; the last run's output, exit 124"
        );
        assert!(
            t0.elapsed() <= std::time::Duration::from_secs(8),
            "...in its two bounds"
        );
        let f = std::env::temp_dir().join(format!("omabox-recff-{}", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let fs = f.display().to_string();
        assert_eq!(
            ff(
                Ff::Out,
                &[
                    "/usr/bin/sh",
                    "-c",
                    "[ -e \"$1\" ] && echo ok && exit 0; : > \"$1\"; exec sleep 30",
                    "sh",
                    &fs
                ]
            ),
            "omabox: record: sh did not finish in 1 s (stuck: finding 356): killed, run once more\nok\nrc=0",
            "...stuck once: run again, its output and exit"
        );
        let _ = std::fs::remove_file(&f);
        assert_eq!(
            ff(Ff::Plain { to_err: false }, &["/usr/bin/false"]),
            "rc=1",
            "...a failure that is not a timeout: not run again"
        );
    }
}
