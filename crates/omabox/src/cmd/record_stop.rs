//! `record stop` (#176, #224, #225, #226, findings 306, 312, 319, 356): the
//! recording ended, made into the video in the box (rec_ff: every ffmpeg bounded), its marks and
//! captions beside it, the input drawn in, still stretches cut, a contact sheet (record_sheet.rs);
//! copied out as the bytes of the box's `cat` (box_copy, finding 297).

use super::record::{
    Enc, Err2, REC_AAC, REC_CRF, REC_FF_FLOOR, REC_TAGS, REC_TAGS_F, REC_YUV_ARGS, Rec, StopOpts,
    audio_end, now_us, num, rec_dir, rec_dirs, rec_enc, rec_max, rec_yuv, record_first_us,
    record_firsts, record_running, record_size, rm, sys_cap, sys_q, sys_run, w,
};
use super::record_sheet::{Src, record_sheet};
use crate::cat;
use crate::exec::{ExecOpts, box_exec, dup_hi, pipe};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jail::in_jail;
use crate::jq;
use crate::num::ms_s;
use crate::recprog as P;
use crate::shell::{self, strip_nl};
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// REC_FF_SECS: rec_ff's bound once one is known (0: not yet, REC_FF_FLOOR).
pub(super) static FF_SECS: AtomicI64 = AtomicI64::new(0);

/// rec_ff_bound FRAMES PIXELS: REC_FF_FLOOR, and 5 s more for each second of 1080p at 30 fps.
pub(super) fn ff_bound(frames: i64, px: i64) -> i64 {
    REC_FF_FLOOR + (frames * px + 12_441_599) / 12_441_600
}

/// How long a stop waits for wf-recorder to write the frames it captured before it (#444).
const REC_DRAIN_S: i64 = 10;

/// drain: true once each wf-recorder in PIDS (the recording's, in rec_dirs' order) has handed its
/// encoder every frame it captured before this call, false after REC_DRAIN_S (recprog's DRAIN_SH).
fn drain(ctx: &mut Ctx, rec: &Rec, pids: &[Vec<u8>]) -> Res<bool> {
    let dl = now_us() + REC_DRAIN_S * 1_000_000;
    // (Under timeout: frames is the box's file, which its processes could make a FIFO.)
    let mut a = w(&[
        "/usr/bin/timeout",
        "-k",
        "1",
        &(REC_DRAIN_S + 3).to_string(),
        "/usr/bin/bash",
        "-c",
        P::DRAIN_SH,
        "omabox-record",
        &dl.to_string(),
    ]);
    for (d, p) in rec_dirs(ctx, rec).into_iter().zip(pids) {
        a.push(d);
        a.push(p.clone());
    }
    Ok(sys_q(ctx, &a)? == 0)
}

/// stop_len: the video's length up to a stop called at T_STOP_US (#444): to that time and a frame
/// more, in the video's clock (from its first frame, the layout's latest); None when that is unknown.
fn stop_len(ctx: &mut Ctx, rec: &Rec, t_stop: i64) -> Option<Vec<u8>> {
    let f = record_first_us(ctx, rec)?;
    let fps: f64 = std::str::from_utf8(&rec.fps).ok()?.trim().parse().ok()?;
    if fps <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let s = (t_stop - f) as f64 / 1e6 + 1.0 / fps;
    (s > 0.0).then(|| format!("{s:.6}").into_bytes())
}

/// How rec_ff gives its output: as it comes (to stderr with TO_ERR), or captured (-o, -e with its
/// stderr), printed when it ends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ff {
    Plain { to_err: bool },
    Out,
    Err,
}

/// rec_ff MODE /usr/bin/FFMPEG ARG... (finding 356): in the box under timeout, a run that ran out of
/// time run once more, a second said, 124. Its status, and (captured) its output and a newline.
pub(super) fn rec_ff(ctx: &mut Ctx, mode: Ff, argv: &[Vec<u8>]) -> Res<(i32, Vec<u8>)> {
    rec_ff_with(ctx, mode, argv, |ctx, a| {
        Ok(match mode {
            Ff::Out => sys_cap(ctx, a, Some(16_777_216), Err2::Null, true),
            Ff::Err => sys_cap(ctx, a, Some(16_777_216), Err2::Out, true),
            Ff::Plain { to_err } => (sys_run(ctx, a, to_err, Err2::Inherit, true)?, Vec::new()),
        })
    })
}

/// rec_ff, each run made by RUN (in the box: sys_cap, sys_run): its status and what it captured.
pub(super) fn rec_ff_with(
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
pub(super) fn bytes_h(b: &[u8]) -> Vec<u8> {
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
pub(super) fn box_copy(ctx: &mut Ctx, file: &[u8], max: i64, to: &std::fs::File) -> Option<i64> {
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
pub(super) fn size_of(p: &std::path::Path) -> Option<i64> {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| i64::try_from(m.len()).ok())
}

/// The files record_stop makes here (the box dir's, the video's .part, the input's temp file),
/// removed however it ends, by one trap from the first of them on (rec_traps': `exit 130|143|129`
/// on a signal). Bash's had the input's file under its EXIT trap alone, so that a signal ended the
/// stop there as it did before finding 432; one trap is enough (finding 452).
pub(super) struct Tmps {
    v: Arc<Mutex<Vec<Vec<u8>>>>,
    part: Arc<Mutex<Option<crate::shots::OutFile>>>,
    trap: Option<crate::trap::Trap>,
}

impl Tmps {
    pub(super) fn new() -> Tmps {
        Tmps {
            v: Arc::new(Mutex::new(Vec::new())),
            part: Arc::new(Mutex::new(None)),
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
            // (in its folder held: never by a name a box can lead elsewhere)
            if let Some(part) = p.lock().ok().and_then(|x| x.clone()) {
                part.remove_part();
            }
        }
    }

    /// rec_traps: the trap set (once).
    pub(super) fn arm(&mut self) {
        if self.trap.is_none() {
            self.trap = Some(crate::trap::on_exit_code(self.undo()));
        }
    }

    pub(super) fn push(&mut self, d: Vec<u8>) {
        if let Ok(mut v) = self.v.lock() {
            v.push(d);
        }
    }

    pub(super) fn add(&mut self, d: Vec<u8>) {
        self.push(d);
        self.arm();
    }

    pub(super) fn set_part(&mut self, p: Option<&crate::shots::OutFile>) {
        if let Ok(mut x) = self.part.lock() {
            *x = p.cloned();
        }
        self.arm();
    }
}

/// What a take left for its marks, captions, overlay and cut, read once by stop_in: each value the
/// text the awk programs are given (`-v first=...`), never parsed and printed again, so what they
/// make is what bash's made.
#[derive(Default)]
pub(crate) struct Take {
    /// The input it saw (#176): the path of a file of record_input's lines; empty: none.
    pub input: Vec<u8>,
    /// Its marks (record_mark's lines: wall-clock µs, a tab, the text); empty: none.
    pub marks: Vec<u8>,
    /// The video's clock: its first frame's wall-clock µs (empty: no marks, input or cut asked).
    pub first: Vec<u8>,
    /// record.geo's first line: "X0 Y0 SCALE AX AY AW AH ..." of the recorded output.
    pub geo: Vec<u8>,
    /// Its fps (REC_FPS, a start's --fps).
    pub fps: Vec<u8>,
    /// The filter putting its sound in the video's clock; empty: no sound.
    pub afilt: Vec<u8>,
    /// --show-input, --captions.
    pub show: bool,
    pub caps: bool,
    /// The frames --cut-idle took out ("FROM TO" a line), once cut.
    pub cuts: Vec<u8>,
    /// The video's length in ms as ffprobe read it, once made; empty: unknown.
    pub dur: Vec<u8>,
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
    // Its time: the video ends there (the frames the drain lets in after it are not the take's).
    let t_stop = now_us();
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
    // A jailed agent's recording stopped from outside the jail: a file in omabox's records folder
    // (in its data dir: never among the box HOMEs, whose sweep took it, #433).
    let in_records = so.tofd.is_empty() && ctx.at("record.as").is_file();
    if in_records {
        let asname = rec.file.clone();
        let dir = crate::paths::records();
        if !crate::shots::mkdir_p(&dir) {
            return Err(ctx.die(cat!("record stop: cannot make ", dir)));
        }
        let t = shell::date("%Y%m%d-%H%M%S");
        rec.file = cat!(dir, "/omabox-", ctx.name, "-", t, ".mp4");
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
        // Every frame captured before this stop written first (#444, finding 689): wf-recorder drops
        // the frames still queued for its encoder on SIGINT, and under load that was up to a second
        // of video before the stop, past a `record hold`'s end. Quick when it keeps up.
        if !drain(ctx, rec, &rpids)? {
            say(cat!(
                "record: wf-recorder still had frames to write after ",
                REC_DRAIN_S.to_string(),
                " s: the video may end before the stop"
            ));
        }
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
    // Its outputs' turns (#435), read before the files go.
    let turns = std::fs::read(ctx.at("record.turn")).unwrap_or_default();
    for f in [
        "record",
        "record.audio",
        "record.as",
        "record.crf",
        "record.turn",
    ] {
        rm(&ctx.at(f));
    }
    // The bound on its ffmpeg runs (finding 356): as many frames as it ran for, of its size.
    let raw1 = if rec.n > 1 {
        None
    } else {
        raw_size(ctx, rec_dir(&ctx.d).as_bytes())?
    };
    let px = if rec.n > 1 {
        let s = record_size(ctx);
        let (a, b) = s.split_at(s.iter().position(|&c| c == b'x').unwrap_or(0));
        num(a) * num(b.get(1..).unwrap_or_default())
    } else {
        raw1.map_or(3840 * 2160, |(w, h)| w * h)
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
                &format!("{}/audio.mka", rec_dir(&ctx.d)),
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
                &rec_dir(&ctx.d),
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
    let mut take = Take {
        input,
        marks,
        first,
        geo,
        fps,
        afilt,
        show: so.show,
        caps: so.caps,
        ..Take::default()
    };
    let input_s =
        !take.input.is_empty() && size_of(&shell::path(&take.input)).is_some_and(|s| s > 0);
    // --show-input, --captions (#176): drawn by the ass filter at the end of the first pass.
    let mut vass = Vec::new();
    if so.show || so.caps {
        if so.show && !input_s {
            say(
                "record --show-input: nothing to show (no click, scroll, pointer, drag or keys while it recorded)",
            );
        }
        if so.caps && take.marks.is_empty() {
            say("record --captions: no marks to show (record mark TEXT while it records)");
        }
        if (so.show && input_s) || (so.caps && !take.marks.is_empty()) {
            match record_ass(ctx, rec, &take)? {
                Some(v) => vass = v,
                None => say(
                    "record: could not draw the input or captions into the video: a video without them",
                ),
            }
        }
    }
    let lim = stop_len(ctx, rec, t_stop);
    // Made: 0, or ffmpeg's status (124: killed twice, finding 356).
    let made = if rec.n > 1 {
        record_compose(ctx, rec, &take.afilt, &vass, so.cut, &turns, lim.as_deref())?
    } else {
        // A steady REC_FPS, the video's clock still wf-recorder's (finding 306).
        let mut a = w(&[
            "/usr/bin/ffmpeg",
            "-v",
            "error",
            "-y",
            "-i",
            &format!("{}/raw.mkv", rec_dir(&ctx.d)),
        ]);
        if !take.afilt.is_empty() {
            a.extend(w(&["-i", &format!("{}/audio.mka", rec_dir(&ctx.d))]));
        }
        a.push(b"-vf".to_vec());
        let back = raw1.map_or("", |wh| {
            turn_back(turns.split(|&c| c == b'\n').next().unwrap_or_default(), wh)
        });
        a.push(cat!(back, "fps=", take.fps, ",", rec_yuv(), vass));
        a.extend(rec_enc().video_args(&rec_crf(so.cut, rec.crf)));
        a.extend(w(&REC_TAGS));
        if !take.afilt.is_empty() {
            a.extend(w(&["-map", "0:v:0", "-map", "1:a:0", "-af"]));
            a.push(take.afilt.clone());
            a.extend(w(&REC_AAC));
            a.push(b"-shortest".to_vec());
        }
        if let Some(l) = &lim {
            a.push(b"-t".to_vec());
            a.push(l.clone());
        }
        a.extend(w(&[
            "-movflags",
            "+faststart",
            &format!("{}/out.mp4", rec_dir(&ctx.d)),
        ]));
        rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0
    };
    // Its encode stuck twice (finding 693, #445): the raw H.264 copied into the MP4 as it was
    // recorded, what that leaves out said; failed only when that fails too.
    let copied = made == 124
        && record_copy(ctx, rec, &turns, lim.as_deref())?.is_some_and(|t| {
            say(copy_said(
                rec.n > 1,
                &take.fps,
                [
                    !take.afilt.is_empty(),
                    so.show,
                    so.caps,
                    t,
                    so.cut.is_some(),
                ],
            ));
            true
        });
    if made != 0 && !copied {
        return Err(ctx.die(if rec.n > 1 {
            cat!(
                "record: ffmpeg could not make the video (the monitors' raw.mkv in ",
                &rec_dir(&ctx.d),
                "/m1 to m",
                rec.n.to_string(),
                " in box '",
                ctx.name,
                "' are kept)"
            )
        } else {
            cat!(
                "record: ffmpeg could not make the video (",
                &rec_dir(&ctx.d),
                "/raw.mkv in box '",
                ctx.name,
                "' is kept)"
            )
        }));
    }
    if rec.n > 1 {
        rm(&ctx.at("record.layout"));
    }
    // --cut-idle (#176): its still stretches cut short, CUTS the frames taken out (not in a copy:
    // its frames come at no steady fps, and cutting would encode what just got stuck).
    if let Some(c) = so.cut.filter(|_| !copied) {
        match record_cut(ctx, rec, c, &take)? {
            Some(x) => take.cuts = x,
            None => say("record --cut-idle: could not cut it: the video as it was recorded"),
        }
    }
    if !take.input.is_empty() {
        rm(&shell::path(&take.input));
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
            &format!("{}/out.mp4", rec_dir(&ctx.d)),
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
    take.dur = dur.map(|d| d.to_string().into_bytes()).unwrap_or_default();
    let base = rec.file.strip_suffix(b".mp4").unwrap_or(&rec.file).to_vec();
    let max = rec_max();
    let outmp4 = format!("{}/out.mp4", rec_dir(&ctx.d)).into_bytes();
    // (the video's file in its folder held, when it is written at a path: its marks, captions and
    // sheet go beside it there)
    let mut out_file: Option<crate::shots::OutFile> = None;
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
                &rec_dir(&ctx.d),
                "/out.mp4 there is kept)"
            )));
        }
    } else {
        // FILE's folder held, FILE and FILE.part checked again (a box may have planted a link at
        // either since the start), FILE.part made new there and renamed FILE (finding 749).
        let kept = || {
            cat!(
                " (box '",
                ctx.name,
                "''s ",
                &rec_dir(&ctx.d),
                "/out.mp4 there is kept)"
            )
        };
        // (a path record start resolved: taken as it is, no link on its way; omabox's own records
        // folder, for a jailed agent's take, resolved here)
        let vid = if in_records {
            crate::shots::OutFile::at(&rec.file, false)
        } else {
            crate::shots::OutFile::resolved(&rec.file, false)
        };
        let vid = match vid {
            Ok(v) => v,
            Err(m) => return Err(ctx.die(cat!("record: ", m, kept()))),
        };
        tmps.set_part(Some(&vid));
        let ok = match vid.create_part() {
            Ok(f) => {
                let got = box_copy(ctx, &outmp4, max, &f);
                got.is_some_and(|s| {
                    f.metadata().ok().and_then(|m| i64::try_from(m.len()).ok()) == Some(s)
                })
            }
            Err(m) => {
                say(cat!("record: ", m));
                false
            }
        };
        if !ok {
            vid.remove_part();
            return Err(ctx.die(cat!(
                "record: could not copy the video out of box '",
                ctx.name,
                "' (",
                &rec_dir(&ctx.d),
                "/out.mp4 there is kept)"
            )));
        }
        // (the .part gone with it when it cannot: finding 442)
        if let Err(m) = vid.commit() {
            say(cat!("record: ", m));
            return Err(ctx.die(cat!("record: could not put the video at ", rec.file)));
        }
        tmps.set_part(None);
        rec.file = vid.path();
        out_file = Some(vid);
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
        let mt = crate::shots::OutFile::in_dir(&tmpd, b"v.marks.tsv", false)
            .map_err(|m| ctx.die(cat!("record: ", m)))?;
        let mv = mt.beside(b"v.vtt", false);
        record_marks(ctx, &take, (&mt, &mv), &base)?;
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
    } else if let Some(v) = &out_file {
        let bn = v.name.strip_suffix(b".mp4").unwrap_or(&v.name).to_vec();
        let (mt, mv) = (
            v.beside(&cat!(bn, ".marks.tsv"), false),
            v.beside(&cat!(bn, ".vtt"), false),
        );
        record_marks(ctx, &take, (&mt, &mv), &base)?;
    }
    let mut sp = Vec::new();
    if so.sheet {
        // (`sp=$(record_sheet ...) || sp=""`: a refusal in it ends only the substitution)
        // (beside the video in its folder held; the marks read there as the regular file they are)
        let (sheet, tsv) = match &out_file {
            Some(v) => {
                let bn = v.name.strip_suffix(b".mp4").unwrap_or(&v.name).to_vec();
                (
                    Ok(v.beside(&cat!(bn, ".sheet.png"), true)),
                    v.dir.open_regular(&cat!(bn, ".marks.tsv")),
                )
            }
            None => (
                crate::shots::OutFile::at(&cat!(base, ".sheet.png"), true),
                crate::shots::open_regular(&cat!(base, ".marks.tsv")),
            ),
        };
        sp = match sheet {
            Ok(sheet) => record_sheet(
                ctx,
                Src::Box(&outmp4),
                tsv,
                &sheet,
                &so.frames,
                so.every,
                &so.fit,
                &so.more,
                b"",
            )
            .ok()
            .flatten()
            .unwrap_or_default(),
            Err(m) => {
                say(cat!("record: the contact sheet: ", m));
                Vec::new()
            }
        };
    }
    let mut rmd = w(&[
        format!("{}/raw.mkv", rec_dir(&ctx.d)),
        format!("{}/out.mp4", rec_dir(&ctx.d)),
        format!("{}/first-frame", rec_dir(&ctx.d)),
        format!("{}/last-frame", rec_dir(&ctx.d)),
        format!("{}/overlay.ass", rec_dir(&ctx.d)),
        format!("{}/cut.mp4", rec_dir(&ctx.d)),
    ]);
    if !take.afilt.is_empty() {
        rmd.extend(w(&[
            format!("{}/audio.mka", rec_dir(&ctx.d)),
            format!("{}/audio.log", rec_dir(&ctx.d)),
        ]));
    }
    if rec.n != 1 {
        rmd.extend(rec_dirs(ctx, rec));
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
        let d = if take.dur.is_empty() {
            b"0".to_vec()
        } else {
            take.dur.clone()
        };
        say(cat!(
            "record: ",
            g(1),
            "x",
            g(2),
            ", ",
            ms_s(&d),
            " at ",
            take.fps,
            " fps",
            match rec.crf {
                _ if copied => ", copied as recorded (not re-encoded)".to_string(),
                Some(0) =>
                    ", CRF 0 (lossless: H.264's High 4:4:4 Predictive, which browsers do not play)"
                        .to_string(),
                Some(c) => format!(", CRF {c}"),
                None => String::new(),
            },
            if take.afilt.is_empty() {
                ""
            } else {
                ", with sound"
            },
            if rec_enc() == Enc::OpenH264 {
                ", by openh264 (this ffmpeg has no libx264)"
            } else {
                ""
            }
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
            &format!("{}/audio.log", rec_dir(&ctx.d)),
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

/// raw_size DIR: the size of DIR/raw.mkv in the box, as ffprobe reads it; None when it cannot.
fn raw_size(ctx: &mut Ctx, dir: &[u8]) -> Res<Option<(i64, i64)>> {
    let l = ff_line(
        ctx,
        &w(&[
            b"/usr/bin/ffprobe".as_slice(),
            b"-v",
            b"error",
            b"-select_streams",
            b"v:0",
            b"-show_entries",
            b"stream=width,height",
            b"-of",
            b"csv=p=0",
            &cat!(dir, "/raw.mkv"),
        ]),
        usize::MAX,
    )?;
    let re = shell::Re::new("^([1-9][0-9]{0,4}),([1-9][0-9]{0,4})$");
    Ok(re.as_ref().and_then(|r| r.captures(&l, false)).map(|c| {
        let g = |i: usize| {
            c.get(i)
                .copied()
                .flatten()
                .map_or(0, |(a, b)| num(&l[a..b]))
        };
        (g(1), g(2))
    }))
}

/// turn_back TURN RAW: the filter that turns a raw video of RAW's size back upright, for an output
/// whose record.turn line is TURN ("T W H": its transform and mode), or "" when it came upright
/// (#435, finding 653). wf-recorder turns each frame by the output's transform, as wlr-screencopy's
/// frames once came unturned; Hyprland 0.57 hands them over upright already, so a quarter-turned
/// output's video came turned twice: its mode's size, not the turned one. Then the turn wf-recorder
/// made (its transpose_from_transform) is undone. A half turn or a flip keeps the size: not told
/// apart, left as wf-recorder made it.
pub(crate) fn turn_back(turn: &[u8], raw: (i64, i64)) -> &'static str {
    let f = shell::read_fields(turn, 3);
    let (t, mw, mh) = (num(&f[0]), num(&f[1]), num(&f[2]));
    if t & 1 == 0 || mw == mh || raw != (mw, mh) {
        return "";
    }
    match t {
        1 => "transpose=2,",
        3 => "transpose=1,",
        5 => "hflip,transpose=2,",
        7 => "hflip,transpose=1,",
        _ => "",
    }
}

/// record_compose: the layout's recordings in one video (#225), in step by their first frames.
fn record_compose(
    ctx: &mut Ctx,
    rec: &Rec,
    afilt: &[u8],
    vass: &[u8],
    cut: Option<i64>,
    turns: &[u8],
    lim: Option<&[u8]>,
) -> Res<i32> {
    let lay = std::fs::read(ctx.at("record.layout")).unwrap_or_default();
    if !lay.contains(&b'\n') {
        return Ok(1);
    }
    let mut lines: Vec<&[u8]> = lay.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let head = shell::read_fields(lines.first().copied().unwrap_or_default(), 3);
    let (ow, oh) = (num(&head[0]), num(&head[1]));
    let lay: Vec<&[u8]> = lines.into_iter().skip(1).collect();
    let dirs = rec_dirs(ctx, rec);
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
        return Ok(1);
    }
    let Some(t0) = firsts.iter().flatten().map(|(m, _)| *m).max() else {
        say("record: no monitor gave a frame");
        return Ok(1);
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
        let back = match raw_size(ctx, &dirs[i])? {
            Some(wh) => turn_back(turns.split(|&c| c == b'\n').nth(i).unwrap_or_default(), wh),
            None => "",
        };
        fc.extend_from_slice(&cat!(
            "[",
            k.to_string(),
            ":v]",
            back,
            "setpts=PTS-STARTPTS+(",
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
    // At most to the stop's time (#444).
    if let Some(l) = lim {
        let shorter = !positive
            || awk_begin(
                &[("e", &end), ("l", l)],
                r#"BEGIN { printf "%d", (l + 0 < e + 0) }"#,
            ) == b"1";
        if shorter {
            tlim = vec![b"-t".to_vec(), l.to_vec()];
        }
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
        args.extend(w(&["-i", &format!("{}/audio.mka", rec_dir(&ctx.d))]));
        fc.extend_from_slice(&cat!(";[", (k + 1).to_string(), ":a]", afilt, "[a]"));
        amap = w(&["-map", "[a]"]);
        amap.extend(w(&REC_AAC));
        amap.push(b"-shortest".to_vec());
    }
    args.push(b"-filter_complex".to_vec());
    args.push(fc);
    args.extend(w(&["-map", "[v]"]));
    args.extend(rec_enc().video_args(&rec_crf(cut, rec.crf)));
    args.extend(w(&REC_TAGS));
    args.extend(amap);
    args.extend(tlim);
    args.extend(w(&[
        "-movflags",
        "+faststart",
        &format!("{}/out.mp4", rec_dir(&ctx.d)),
    ]));
    let mut a = vec![b"/usr/bin/ffmpeg".to_vec()];
    a.extend(args);
    Ok(rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0)
}

/// record_copy (finding 693, #445): each monitor's raw H.264 (wf-recorder's, libx264's or
/// openh264's) copied into out.mp4 as it was recorded, a video track each, to the stop's time:
/// Some(whether one was recorded turned, left so), None when ffmpeg could not.
fn record_copy(ctx: &mut Ctx, rec: &Rec, turns: &[u8], lim: Option<&[u8]>) -> Res<Option<bool>> {
    let mut a = w(&["/usr/bin/ffmpeg", "-v", "error", "-y"]);
    let (mut k, mut turned) = (0, false);
    for (i, d) in rec_dirs(ctx, rec).iter().enumerate() {
        // (only a stream ffprobe reads: a monitor that gave no frame has none)
        let Some(wh) = raw_size(ctx, d)? else {
            continue;
        };
        let turn = turns.split(|&c| c == b'\n').nth(i).unwrap_or_default();
        turned |= !turn_back(turn, wh).is_empty();
        a.push(b"-i".to_vec());
        a.push(cat!(d, "/raw.mkv"));
        k += 1;
    }
    if k == 0 {
        return Ok(None);
    }
    for i in 0..k {
        a.push(b"-map".to_vec());
        a.push(format!("{i}:v:0").into_bytes());
    }
    a.extend(w(&["-c", "copy"]));
    if let Some(l) = lim {
        a.push(b"-t".to_vec());
        a.push(l.to_vec());
    }
    a.extend(w(&[
        "-movflags",
        "+faststart",
        &format!("{}/out.mp4", rec_dir(&ctx.d)),
    ]));
    Ok((rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0 == 0).then_some(turned))
}

/// What a video record_copy made (of a LAYOUT's monitors or of one) lacks beside one stop encodes
/// (finding 693), said once; LACKS: its sound, --show-input, --captions, a monitor recorded
/// turned, --cut-idle.
fn copy_said(layout: bool, fps: &[u8], lacks: [bool; 5]) -> Vec<u8> {
    let mut v = cat!(
        "record: so the video is ",
        if layout {
            "the monitors' raw H.264 copied into the MP4, a video track each (players show the first), not composed into the layout"
        } else {
            "the raw H.264 copied into the MP4"
        },
        ", not re-encoded (finding 693): no steady ",
        fps,
        " fps (its frames timed as they came)"
    );
    for (on, s) in lacks.into_iter().zip([
        ", no sound",
        ", no input drawn",
        ", no captions",
        ", not turned upright",
        ", no still stretch cut",
    ]) {
        if on {
            v.extend_from_slice(s.as_bytes());
        }
    }
    v.extend_from_slice(b", a larger file");
    v
}

pub(super) fn replace_all(v: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
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

/// record_marks TAKE (TSV, VTT) SHOWN: the take's marks in the video's clock (its first, dur, cuts,
/// fps) as TSV (BASE.marks.tsv) and VTT (BASE.vtt), each made and renamed in its folder held, never
/// through a link (finding 749); no marks: no files, an earlier take's gone.
fn record_marks(
    ctx: &Ctx,
    take: &Take,
    (tsvf, vttf): (&crate::shots::OutFile, &crate::shots::OutFile),
    shown: &[u8],
) -> Res<()> {
    let (marks, first, dur, cuts, fps) =
        (&take.marks, &take.first, &take.dur, &take.cuts, &take.fps);
    let (tsv, vtt) = (tsvf.path(), vttf.path());
    if marks.is_empty() {
        tsvf.remove();
        vttf.remove();
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
    // (the TSV's text kept here: the captions are made from it, never from the file read back by
    // its name, which a box could have swapped since)
    let tsv_text = cat!("ms\ttime\ttext\n", r.out);
    if !r.ok() {
        return Err(ctx.die(cat!("record: cannot write ", tsv)));
    }
    if let Err(m) = tsvf.put(&mut tsv_text.as_slice()) {
        say(cat!("record: ", m));
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
        ],
        Some(&tsv_text),
        jq::Err::Inherit,
    );
    if !v.ok() {
        return Err(ctx.die(cat!("record: cannot write ", vtt)));
    }
    if let Err(m) = vttf.put(&mut v.out.as_slice()) {
        say(cat!("record: ", m));
        return Err(ctx.die(cat!("record: cannot write ", vtt)));
    }
    let n = tsv_text.iter().filter(|&&c| c == b'\n').count() as i64 - 1;
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

/// record_ass TAKE: the overlay script of its input and marks (its first, geo, show, caps) in the
/// box as rec_dir/overlay.ass; Some(the filter that draws it), None when it cannot (said by the
/// caller).
fn record_ass(ctx: &mut Ctx, rec: &Rec, take: &Take) -> Res<Option<Vec<u8>>> {
    let (first, geo) = (&take.first, &take.geo);
    // (the script's path goes into ffmpeg's filter graph as written: a desk box's HOME (#321) with a
    // character the graph reads as its own syntax draws nothing rather than something else)
    if !rec_dir(&ctx.d)
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"/._+@-".contains(&c))
    {
        return Ok(None);
    }
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
                &format!("{}/raw.mkv", rec_dir(&ctx.d)),
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
    let at = OverlayAt {
        x0: &g[0],
        y0: &g[1],
        k: &g[2],
        w: &vw,
        h: &vh,
        dur: b"359999990",
        area: &area,
    };
    let mut ok = match record_overlay(take, &at) {
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
                    &format!("{}/overlay.ass", rec_dir(&ctx.d)),
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
    Ok(Some(
        format!(",ass={}/overlay.ass", rec_dir(&ctx.d)).into_bytes(),
    ))
}

/// `box_sys SHELL -c SCRIPT omabox-record PATH < FD`: its status.
pub(super) fn write_in_box(
    ctx: &mut Ctx,
    script: &str,
    sh: &str,
    path: &str,
    fd: OwnedFd,
) -> Res<i32> {
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

/// Where record_overlay draws, as the text its awk is given: the output's place (X0, Y0) and scale
/// (K) in the layout, the video's size (W x H), how long the script's last event lasts (DUR, ms) and
/// the area recorded ("X Y W H").
pub struct OverlayAt<'a> {
    pub x0: &'a [u8],
    pub y0: &'a [u8],
    pub k: &'a [u8],
    pub w: &'a [u8],
    pub h: &'a [u8],
    pub dur: &'a [u8],
    pub area: &'a [u8],
}

/// record_overlay: the ASS script drawing peek's marks at the times the input happened (bin/omabox's
/// awk, verbatim, in C.UTF-8), of TAKE's input (with show) and marks (with caps) from its first, AT
/// where. None when awk fails.
pub(crate) fn record_overlay(take: &Take, at: &OverlayAt) -> Option<Vec<u8>> {
    let (input, marks, first, show, caps) = (
        &take.input[..],
        &take.marks[..],
        &take.first[..],
        take.show,
        take.caps,
    );
    let OverlayAt {
        x0,
        y0,
        k,
        w: vw,
        h: vh,
        dur,
        area,
    } = *at;
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

/// record_cut CUT_MS TAKE (finding 319): every still stretch longer than CUT_MS cut to it, the
/// take's input (its first, show) and marks kept, its sound cut with it. Some(the cuts, "FROM TO"
/// frames a line), None when it could not.
fn record_cut(ctx: &mut Ctx, rec: &Rec, cut: i64, take: &Take) -> Res<Option<Vec<u8>>> {
    let (first, input, marks, show, afilt) = (
        &take.first,
        &take.input,
        &take.marks,
        take.show,
        &take.afilt,
    );
    let outmp4 = format!("{}/out.mp4", rec_dir(&ctx.d));
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
    a.extend(w(&["-map", "[v]"]));
    a.extend(rec_enc().video_args(&rec.crf.unwrap_or(REC_CRF).to_string()));
    a.extend(w(&REC_TAGS));
    a.extend(amap);
    a.extend(w(&[
        "-movflags",
        "+faststart",
        &format!("{}/cut.mp4", rec_dir(&ctx.d)),
    ]));
    if rec_ff(ctx, Ff::Plain { to_err: true }, &a)?.0 != 0 {
        return Ok(None);
    }
    let mv = w(&[
        "/usr/bin/mv",
        "-f",
        "--",
        &format!("{}/cut.mp4", rec_dir(&ctx.d)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding 693: what a copied video lacks, only what applies, a larger file always.
    #[test]
    fn copy_said_names_what_applies() {
        assert_eq!(
            String::from_utf8(copy_said(false, b"30", [false; 5])).unwrap(),
            "record: so the video is the raw H.264 copied into the MP4, not re-encoded (finding 693): no steady 30 fps (its frames timed as they came), a larger file"
        );
        assert_eq!(
            String::from_utf8(copy_said(true, b"60", [true, false, true, false, true])).unwrap(),
            "record: so the video is the monitors' raw H.264 copied into the MP4, a video track each (players show the first), not composed into the layout, not re-encoded (finding 693): no steady 60 fps (its frames timed as they came), no sound, no captions, no still stretch cut, a larger file"
        );
    }

    /// #435, finding 653: a raw video turned back only when it came turned twice (its mode's size on
    /// a quarter-turned output), by the inverse of wf-recorder's turn; record_turns' lines.
    #[test]
    fn turned_twice_turned_back() {
        assert_eq!(turn_back(b"1 1280 720", (1280, 720)), "transpose=2,");
        assert_eq!(turn_back(b"1 1280 720", (720, 1280)), "");
        assert_eq!(turn_back(b"3 1280 720", (1280, 720)), "transpose=1,");
        assert_eq!(turn_back(b"5 1280 720", (1280, 720)), "hflip,transpose=2,");
        assert_eq!(
            turn_back(b"7 1920 1080", (1920, 1080)),
            "hflip,transpose=1,"
        );
        assert_eq!(turn_back(b"7 1920 1080", (1080, 1920)), "");
        assert_eq!(turn_back(b"0 1280 720", (1280, 720)), "");
        assert_eq!(turn_back(b"2 1280 720", (1280, 720)), "");
        assert_eq!(turn_back(b"1 1080 1080", (1080, 1080)), "");
        assert_eq!(turn_back(b"", (1280, 720)), "");
        let names = [b"A".to_vec(), b"B".to_vec(), b"C".to_vec()];
        let mons = b"A 0 1280 720\nB 1 1280 720\nC 3 1280 720";
        assert_eq!(
            super::super::record::record_turns(&names, mons),
            b"0 1280 720\n1 1280 720\n3 1280 720".to_vec()
        );
        assert_eq!(
            super::super::record::record_turns(&names[..1], mons),
            Vec::<u8>::new()
        );
        assert_eq!(
            super::super::record::record_turns(&[b"Z".to_vec(), b"B".to_vec()], mons),
            b"0 0 0\n1 1280 720".to_vec()
        );
        assert_eq!(
            super::super::record::record_turns(&names, b"A 2 1280 720\nB 4 1280 720"),
            Vec::<u8>::new()
        );
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
        let mt = crate::shots::OutFile::in_dir(shell::bytes(&d), b"take.marks.tsv", false).unwrap();
        let mv = mt.beside(b"take.vtt", false);
        let mut ctx = Ctx::new(b"record", Vec::new());
        ctx.parsed();
        let marks = |m: &str, first: &str, dur: &str, cuts: &str| {
            let (r, _) = crate::exit::heard(|| {
                let take = Take {
                    marks: m.into(),
                    first: first.into(),
                    dur: dur.into(),
                    cuts: cuts.into(),
                    ..Take::default()
                };
                record_marks(&ctx, &take, (&mt, &mv), &rb)
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
        // Finding 749: a link a box planted at a .part's name (or the name) is never written
        // through; the marks are refused, the file it leads to as it was.
        let secret = d.join("secret");
        std::fs::write(&secret, "keep").unwrap();
        for n in ["take.marks.tsv.part", "take.vtt.part", "take.vtt"] {
            let _ = std::fs::remove_file(d.join(n));
            std::os::unix::fs::symlink(&secret, d.join(n)).unwrap();
            let (r, _) = crate::exit::heard(|| {
                let take = Take {
                    marks: b"1000000500000\tx".to_vec(),
                    first: b"1000000000000".to_vec(),
                    dur: b"4000".to_vec(),
                    ..Take::default()
                };
                record_marks(&ctx, &take, (&mt, &mv), &rb)
            });
            assert!(r.is_err(), "{n}: refused");
            assert_eq!(std::fs::read_to_string(&secret).unwrap(), "keep", "{n}");
            std::fs::remove_file(d.join(n)).unwrap();
        }
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
