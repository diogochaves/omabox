//! `omabox record` (#176, #225, #226, #259, #280, findings 306-320, 356): a video of a box's screen
//! by wf-recorder in the box (one an output for the whole layout, composed at stop), its marks and
//! holds in the video's own clock, its sound, its input drawn in, still stretches cut, a contact
//! sheet. Every ffmpeg and ffprobe runs in the box, bounded (rec_ff); the host reads the box's files
//! only as the bytes of a `cat` there (box_copy, finding 297). A jailed caller's files come and go as
//! fds its own omabox opened (#224). The shell, awk and jq it runs are bin/omabox's (recprog.rs).

use crate::args::{optarg, unexpected, unknown_opt, unknown_word};
use crate::boxes::{meta, need_box};
use crate::cat;
use crate::exec::{ExecOpts, Redir, box_exec, dup_hi, pipe, read_all};
use crate::exit::{Ctx, Exit, Res, out, say};
use crate::jail::in_jail;
use crate::jq::{self, Args};
use crate::num::ms_duration;
use crate::recprog as P;
use crate::select::select_box;
use crate::shell::{self, strip_nl};
use std::ffi::OsStr;
use std::io::Write;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

pub(crate) const REC_DIR: &str = "/home/sbx/.cache/omabox-record";
pub(crate) const REC_FF_FLOOR: i64 = 60;
const AUDIO_SINK: &str = "omabox-speaker";
pub(crate) const REC_YUV_ARGS: &str = "in_range=full:in_color_matrix=bt601:out_range=limited:out_color_matrix=bt709:flags=accurate_rnd+full_chroma_int";
pub(crate) const REC_TAGS_F: &str =
    "setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709";
pub(crate) const REC_TAGS: [&str; 10] = [
    "-pix_fmt",
    "yuv420p",
    "-color_range",
    "tv",
    "-colorspace",
    "bt709",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
];
pub(crate) const REC_AAC: [&str; 4] = ["-c:a", "aac", "-b:a", "192k"];

pub(crate) fn rec_yuv() -> String {
    format!("scale={REC_YUV_ARGS},{REC_TAGS_F}")
}

/// REC_MAX: the largest video omabox copies out of a box or into one (#224), 16 GiB, lowered by
/// $OMABOX_RECORD_MAX (the suite's).
pub(crate) fn rec_max() -> i64 {
    let max = 16i64 << 30;
    let e = shell::env_or_empty("OMABOX_RECORD_MAX");
    if shell::re_match("^[0-9]{1,12}$", &e) {
        if let Some(v) = shell::dec(&e) {
            if v < max {
                return v;
            }
        }
    }
    max
}

pub(crate) fn w<A: AsRef<[u8]>>(v: &[A]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_ref().to_vec()).collect()
}

pub(crate) fn num(v: &[u8]) -> i64 {
    shell::dec(v).unwrap_or(0)
}

pub(crate) fn now_us() -> i64 {
    num(&crate::marks::epoch_us())
}

pub(crate) fn rm(p: &std::path::Path) {
    let _ = std::fs::remove_file(p);
}

/// The recording's state: what `$D/record` says (record_running's globals).
#[derive(Default, Clone)]
pub(crate) struct Rec {
    pub pid: Vec<u8>,
    pub out: Vec<u8>,
    pub fps: Vec<u8>,
    pub start: Vec<u8>,
    pub file: Vec<u8>,
    pub n: usize,
    /// --crf (#278): the video's H.264 quality, None for REC_CRF.
    pub crf: Option<i64>,
}

/// Where a box_sys call's stderr goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Err2 {
    Inherit,
    Null,
    Out,
}

/// One of the box's helpers (box_sys): its stdout captured (`$(...)`, LIMIT bytes at most), its
/// stderr ERR, its stdin /dev/null when NULL_IN. A refusal of box_exec's is its status here, said
/// where its stderr goes (inside `$(...)` it ends only the substitution).
pub(crate) fn sys_cap(
    ctx: &mut Ctx,
    argv: &[Vec<u8>],
    limit: Option<usize>,
    err: Err2,
    null_in: bool,
) -> (i32, Vec<u8>) {
    sys_cap_bg(ctx, argv, limit, err, null_in, false)
}

/// sys_cap, BG: inside a `<(...)` (SIGINT and SIGQUIT ignored in it).
pub(crate) fn sys_cap_bg(
    ctx: &mut Ctx,
    argv: &[Vec<u8>],
    limit: Option<usize>,
    err: Err2,
    null_in: bool,
    bg: bool,
) -> (i32, Vec<u8>) {
    let Ok((r, w0)) = pipe() else {
        return (1, Vec::new());
    };
    // (the dup alone kept: the reader's EOF comes once the command's ends are closed)
    let Ok(wr) = dup_hi(w0.as_raw_fd()) else {
        return (1, Vec::new());
    };
    drop(w0);
    let reader = std::thread::spawn(move || read_all(r, limit));
    let rc = {
        let _e = match err {
            Err2::Null => Some(Redir::null(2)),
            Err2::Out => Some(Redir::to(2, wr.as_raw_fd())),
            Err2::Inherit => None,
        };
        box_exec(
            ctx,
            argv,
            ExecOpts {
                sys: true,
                stdin_null: null_in,
                stdout: Some(wr.as_raw_fd()),
                bg,
                ..ExecOpts::default()
            },
        )
    };
    drop(wr);
    let out = reader.join().unwrap_or_default();
    match rc {
        Ok(rc) => (rc, out),
        Err(Exit(c)) => (i32::from(c), out),
    }
}

/// box_sys ARGV, its output as it comes (or to stderr, `>&2`), its stderr ERR.
pub(crate) fn sys_run(
    ctx: &mut Ctx,
    argv: &[Vec<u8>],
    to_err: bool,
    err: Err2,
    null_in: bool,
) -> Res<i32> {
    let _e = match err {
        Err2::Null => Some(Redir::null(2)),
        _ => None,
    };
    box_exec(
        ctx,
        argv,
        ExecOpts {
            sys: true,
            stdin_null: null_in,
            stdout_to_stderr: to_err,
            ..ExecOpts::default()
        },
    )
}

/// `box_sys ARGV... 2>/dev/null` as a step of its own (not in a `$(...)`): its status; box_exec's
/// refusal (the box gone) ends the command, unsaid, as its die does whatever `|| true` follows.
pub(crate) fn sys_q(ctx: &mut Ctx, argv: &[Vec<u8>]) -> Res<i32> {
    sys_run(ctx, argv, false, Err2::Null, false)
}

/// `box_sys /usr/bin/sh -c SCRIPT omabox-record ARGS... 2>/dev/null`, a step of its own.
pub(crate) fn sh_quiet(ctx: &mut Ctx, shell_: &str, script: &str, args: &[Vec<u8>]) -> Res<i32> {
    let mut a = w(&[shell_, "-c", script, "omabox-record"]);
    a.extend(args.iter().cloned());
    sys_q(ctx, &a)
}

// --- the recording's state ----------------------------------------------------------------------

/// record_running: the recording in this box (REC), its wf-recorders all still running; false when
/// there is none, or one of them is gone. A record that cannot be read is dropped.
pub(crate) fn record_running(ctx: &mut Ctx, rec: &mut Rec) -> Res<bool> {
    rec.n = 1;
    let f = ctx.at("record");
    if !f.is_file() {
        return Ok(false);
    }
    let data = std::fs::read(&f).unwrap_or_default();
    let fl = shell::read_fields(&data, 5);
    rec.pid = fl[0].clone();
    rec.out = fl[1].clone();
    rec.fps = fl[2].clone();
    rec.start = fl[3].clone();
    rec.file = fl[4].clone();
    // (`read` fails at a line with no newline, or none: the record dropped)
    if !data.contains(&b'\n') {
        rm(&f);
        return Ok(false);
    }
    if !shell::re_match("^[0-9]+(,[0-9]+)*$", &rec.pid) {
        return Ok(false);
    }
    rec.n = rec.pid.iter().filter(|&&c| c == b',').count() + 1;
    let pids: Vec<Vec<u8>> = rec.pid.split(|&c| c == b',').map(<[u8]>::to_vec).collect();
    let mut a = w(&["/usr/bin/sh", "-c", P::RUNNING_SH, "omabox-record"]);
    a.extend(pids);
    let rc = {
        let _e = Redir::null(2);
        box_exec(
            ctx,
            &a,
            ExecOpts {
                sys: true,
                ..ExecOpts::default()
            },
        )?
    };
    Ok(rc == 0)
}

/// rec_dirs: the folders in the box its wf-recorders write to.
pub(crate) fn rec_dirs(rec: &Rec) -> Vec<Vec<u8>> {
    if rec.n > 1 {
        (1..=rec.n)
            .map(|i| cat!(REC_DIR, "/m", i.to_string()))
            .collect()
    } else {
        vec![REC_DIR.as_bytes().to_vec()]
    }
}

/// record_names: "A, B, C" from "A,B,C" or "A B C".
fn record_names(v: &[u8]) -> Vec<u8> {
    let s: Vec<u8> = v
        .iter()
        .map(|&c| if c == b',' { b' ' } else { c })
        .collect();
    let mut o = Vec::new();
    for &c in &s {
        if c == b' ' {
            o.extend_from_slice(b", ");
        } else {
            o.push(c);
        }
    }
    o
}

/// The layout file's first line's W H S F.
fn layout_head(ctx: &Ctx) -> Vec<Vec<u8>> {
    let d = std::fs::read(ctx.at("record.layout")).unwrap_or_default();
    shell::read_fields(&d, 4)
}

/// record_size: the layout video's size, even.
pub(crate) fn record_size(ctx: &Ctx) -> Vec<u8> {
    let f = layout_head(ctx);
    let (lw, lh) = (num(&f[0]), num(&f[1]));
    format!("{}x{}", lw + lw % 2, lh + lh % 2).into_bytes()
}

/// record_how: how the layout was laid out.
fn record_how(ctx: &Ctx) -> Vec<u8> {
    let f = layout_head(ctx);
    let mut o = cat!("the layout at scale ", f[2]);
    if f[3] != b"1" {
        o.extend_from_slice(&cat!(", scaled by ", f[3], " to fit H.264's largest frame"));
    }
    let (lw, lh) = (num(&f[0]), num(&f[1]));
    if lw % 2 + lh % 2 != 0 {
        o.extend_from_slice(&cat!(", a black edge making ", f[0], "x", f[1], " even"));
    }
    o
}

/// record_firsts: each wf-recorder's first frame, "MONO_US WALL_US", or None for one not come.
pub(crate) fn record_firsts(ctx: &mut Ctx, rec: &Rec) -> Vec<Option<(i64, i64)>> {
    let mut a = w(&["/usr/bin/sh", "-c", P::FIRSTS_SH, "omabox-record"]);
    a.extend(rec_dirs(rec));
    // (`< <(box_sys ...)`: SIGINT and SIGQUIT ignored in it)
    let (_, o) = sys_cap_bg(ctx, &a, Some(4096), Err2::Null, false, true);
    let re = shell::Re::new(r"^([0-9]{1,17}) ([0-9]{1,17}) ([0-9]{1,17}) ([0-9]{1,17})$");
    let mut lines: Vec<&[u8]> = o.split(|&c| c == b'\n').collect();
    // (`while read`: a last line with no newline is not read)
    lines.pop();
    lines
        .into_iter()
        .map(|l| {
            let caps = re.as_ref()?.captures(l, false)?;
            let g = |i: usize| {
                caps.get(i)
                    .copied()
                    .flatten()
                    .map_or(0, |(a, b)| shell::dec(&l[a..b]).unwrap_or(0))
            };
            Some((g(1), g(1) + (g(2) + g(4)) / 2 - g(3)))
        })
        .collect()
}

/// record_first_us: the video's first frame's wall-clock time (the latest of the layout's firsts).
pub(crate) fn record_first_us(ctx: &mut Ctx, rec: &Rec) -> Option<i64> {
    record_firsts(ctx, rec)
        .into_iter()
        .flatten()
        .map(|(_, wl)| wl)
        .max()
}

/// The words a stopped-by-itself recording is said with.
fn stopped_text(ctx: &Ctx, what: &str) -> Vec<u8> {
    cat!(
        what,
        ": box '",
        ctx.name,
        "''s recording stopped by itself (omabox record stop keeps what it has)"
    )
}

// --- the command -----------------------------------------------------------------------------------

#[derive(Default)]
struct Opts {
    name: Vec<u8>,
    sub: Vec<u8>,
    out: Vec<u8>,
    monitor: Vec<u8>,
    fps: Vec<u8>,
    crf: Option<i64>,
    hold: i64,
    holdset: bool,
    ract: Vec<Vec<u8>>,
    nopointer: bool,
    text: Vec<Vec<u8>>,
    video: Vec<u8>,
    audio: bool,
    forms: Option<i64>,
    hdur: Option<i64>,
    sheet: bool,
    frames: Vec<u8>,
    every: Option<i64>,
    fit: Vec<u8>,
    more: SheetMore,
    glide: Option<i64>,
    show: bool,
    caps: bool,
    cut: Option<i64>,
    as_: Vec<u8>,
    tofd: Vec<u8>,
    mfd: Vec<u8>,
    vfd: Vec<u8>,
    fromfd: Vec<u8>,
    mfromfd: Vec<u8>,
    sfile: bool,
    json: bool,
}

/// REC_CRF: the video's H.264 quality unless --crf says (#278): small files, easy to read.
pub(crate) const REC_CRF: i64 = 18;
/// The raw stream's (wf-recorder's, ultrafast): 14, or the video's own when --crf asks for more.
pub(crate) fn raw_crf(crf: Option<i64>) -> i64 {
    crf.map_or(14, |c| c.min(14))
}

/// --crf N: 0 (lossless) to 51, as libx264 takes it.
fn crf_arg(ctx: &Ctx, c: &[u8]) -> Res<i64> {
    match shell::dec(c) {
        Some(n) if shell::re_match("^[0-9]{1,2}$", c) && n <= 51 => Ok(n),
        _ => Err(ctx.die(cat!(
            "record: --crf is the video's H.264 quality, 0 (lossless) to 51 (18 by default; lower is better and larger), got ",
            c
        ))),
    }
}

fn say_exit2(m: impl AsRef<[u8]>) -> Exit {
    say(m);
    Exit(2)
}

fn parse(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<Opts> {
    let mut o = Opts {
        fps: b"30".to_vec(),
        hold: 1000,
        ..Opts::default()
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let v = args.get(i + 1);
        let val = |ctx: &Ctx| -> Res<Vec<u8>> {
            optarg(ctx, a, v)?;
            Ok(args[i + 1].clone())
        };
        match a.as_slice() {
            b"-b" | b"--box" => {
                o.name = val(ctx)?;
                i += 1;
            }
            b"-o" | b"--output" => {
                o.out = val(ctx)?;
                i += 1;
            }
            b"--json" => o.json = true,
            b"--to-fd" => {
                o.tofd = val(ctx)?;
                i += 1;
            }
            b"--marks-fd" => {
                o.mfd = val(ctx)?;
                i += 1;
            }
            b"--vtt-fd" => {
                o.vfd = val(ctx)?;
                i += 1;
            }
            b"--from-fd" => {
                o.fromfd = val(ctx)?;
                i += 1;
            }
            b"--marks-from-fd" => {
                o.mfromfd = val(ctx)?;
                i += 1;
            }
            b"--as" => {
                o.as_ = val(ctx)?;
                i += 1;
            }
            b"--file" => o.sfile = true,
            b"--monitor" => {
                o.monitor = val(ctx)?;
                i += 1;
            }
            b"--no-pointer" => o.nopointer = true,
            b"--audio" => o.audio = true,
            b"--glide" => {
                let g = val(ctx)?;
                o.glide = Some(crate::input::glide_arg(ctx, "record", &g)?);
                i += 1;
            }
            b"--show-input" => o.show = true,
            b"--captions" => o.caps = true,
            // --cut-idle [DURATION]: 1s unless a duration follows.
            b"--cut-idle" => {
                o.cut = Some(1000);
                if let Some(d) =
                    v.filter(|d| shell::re_match("^[0-9]{1,6}(\\.[0-9]{1,3})?(ms|s|m)?$", d))
                {
                    match ms_duration(d) {
                        Some(c) if (100..=600_000).contains(&c) => o.cut = Some(c),
                        _ => {
                            return Err(ctx.die(cat!(
                                "record: --cut-idle takes a duration from 100ms to 10m, like 1s, got ",
                                d
                            )));
                        }
                    }
                    i += 1;
                }
            }
            b"--fps" => {
                o.fps = val(ctx)?;
                i += 1;
            }
            // --crf N (#278): the video's quality, for footage that will be edited.
            b"--crf" => {
                let c = val(ctx)?;
                o.crf = Some(crf_arg(ctx, &c)?);
                i += 1;
            }
            b"--hold" => {
                let h = val(ctx)?;
                match ms_duration(&h) {
                    Some(x) if x <= 60_000 => o.hold = x,
                    _ => {
                        return Err(ctx.die(cat!(
                            "record: --hold takes a duration up to 60s, like 500ms or 2s, got ",
                            h
                        )));
                    }
                }
                o.holdset = true;
                i += 1;
            }
            b"--for" => {
                let f = val(ctx)?;
                match ms_duration(&f) {
                    Some(x) if x <= 3_600_000 => o.forms = Some(x),
                    _ => {
                        return Err(say_exit2(cat!(
                            "record: --for takes a duration up to 60m, like 5s or 1.5m (a bare number is seconds), got ",
                            f
                        )));
                    }
                }
                i += 1;
            }
            b"--sheet" => o.sheet = true,
            b"--frames" => {
                let f = val(ctx)?;
                let ok = shell::re_match("^[0-9]{1,3}$", &f)
                    && shell::test_int(&f).is_some_and(|n| (1..=100).contains(&n));
                if !ok {
                    return Err(ctx.die(cat!("record: --frames is 1 to 100, got ", f)));
                }
                o.frames = f;
                o.sheet = true;
                i += 1;
            }
            b"--every" => {
                let e = val(ctx)?;
                o.every = Some(ms_duration(&e).ok_or_else(|| {
                    ctx.die(cat!(
                        "record: --every takes a duration, like 500ms or 2s, got ",
                        e
                    ))
                })?);
                o.sheet = true;
                i += 1;
            }
            // A range of the video, a region of it, every frame, the frames where it changes (#260).
            b"--from" | b"--to" => {
                let t = val(ctx)?;
                let ms = ms_duration(&t).filter(|&m| m <= 360_000_000).ok_or_else(|| {
                    ctx.die(cat!(
                        "record: ",
                        a,
                        " takes a time in the video, like 1.5s or 900ms (a bare number is seconds), got ",
                        t
                    ))
                })?;
                if a == b"--from" {
                    o.more.from = Some(ms);
                } else {
                    o.more.to = Some(ms);
                }
                o.sheet = true;
                i += 1;
            }
            b"-g" => {
                let g = val(ctx)?;
                o.more.geo = Some(sheet_geo(&g).ok_or_else(|| {
                    ctx.die(cat!(
                        "record: -g takes a region of the video, in its pixels, like \"100,200 640x80\", got ",
                        g
                    ))
                })?);
                o.sheet = true;
                i += 1;
            }
            b"--every-frame" => {
                o.more.every_frame = true;
                o.sheet = true;
            }
            b"--changed" => {
                o.more.changed = true;
                o.sheet = true;
            }
            b"--fit" => {
                let f = val(ctx)?;
                if !shell::re_match("^(0|[1-9][0-9]{2,4})$", &f) {
                    return Err(ctx.die(cat!(
                        "record: --fit is 0 (off) or the sheet's longest side in pixels (100-99999), got ",
                        f
                    )));
                }
                o.fit = f;
                o.sheet = true;
                i += 1;
            }
            // A mark's text may start with a dash after --.
            b"--" => {
                if o.sub == b"mark" {
                    o.text.extend(args[i + 1..].iter().cloned());
                } else {
                    o.ract = args[i + 1..].to_vec();
                }
                break;
            }
            x if x.starts_with(b"-") => return Err(unknown_opt(ctx, "record", a)),
            x => {
                if o.sub == b"mark" {
                    o.text.push(x.to_vec());
                } else if o.sub == b"sheet" && o.video.is_empty() {
                    o.video = x.to_vec();
                } else if o.sub == b"hold" && o.hdur.is_none() {
                    match ms_duration(x) {
                        Some(h) if h <= 3_600_000 => o.hdur = Some(h),
                        _ => {
                            return Err(say_exit2(cat!(
                                "record hold: a duration up to 60m, like 2s or 500ms (a bare number is seconds), got ",
                                x
                            )));
                        }
                    }
                } else if o.sub.is_empty() {
                    match x {
                        b"start" | b"stop" | b"status" | b"mark" | b"sheet" | b"hold" => {
                            o.sub = x.to_vec();
                        }
                        _ => {
                            return Err(unknown_word(
                                ctx,
                                "record",
                                "subcommand",
                                x,
                                "(start, stop, status, mark, hold, sheet, -- ACTION: an omabox command, or --for DURATION)",
                                &["start", "stop", "status", "mark", "hold", "sheet"],
                            ));
                        }
                    }
                } else {
                    return Err(unexpected(ctx, cat!("record ", o.sub), x, ""));
                }
            }
        }
        i += 1;
    }
    Ok(o)
}

/// record_action_check: -- ACTION's action, one that acts, on the recorded box.
pub fn action_check(ctx: &Ctx, ract: &[Vec<u8>]) -> Res<()> {
    let first = ract.first().cloned().unwrap_or_default();
    match first.as_slice() {
        b"keys" | b"click" | b"scroll" | b"drag" | b"pointer" | b"run" | b"hyprctl" | b"lua"
        | b"output" | b"monitor" | b"mode" | b"window" | b"restart-shell" | b"wait" => {}
        b"tray" if crate::cmd::tray::acts(&ract[1..]) => {}
        _ => {
            let got = if first.is_empty() {
                b"nothing".to_vec()
            } else if first == b"tray" {
                cat!(first, crate::cmd::tray::LISTING_NOTE)
            } else {
                first
            };
            return Err(ctx.die(cat!(
                "record -- ACTION: an omabox command that acts (keys, click, scroll, drag, pointer, run, hyprctl, lua, output, monitor, mode, window, tray menu|activate|click, restart-shell, wait), got ",
                got
            )));
        }
    }
    let own: Vec<&Vec<u8>> = ract.iter().take_while(|x| x.as_slice() != b"--").collect();
    let joined = cat!(
        " ",
        own.iter()
            .map(|x| x.as_slice())
            .collect::<Vec<_>>()
            .join(&b' '),
        " "
    );
    if crate::exit::contains(&joined, b" -b ") || crate::exit::contains(&joined, b" --box ") {
        return Err(ctx.die("record -- ACTION: the action runs on the recorded box (no -b)"));
    }
    Ok(())
}

pub fn run(ctx: &mut Ctx, args: &[Vec<u8>]) -> Res<()> {
    crate::trap::stop_on_signals();
    let mut o = parse(ctx, args)?;
    let sub = o.sub.clone();
    let s = sub.as_slice();
    let fps_ok =
        shell::is_num(&o.fps) && shell::test_int(&o.fps).is_some_and(|f| (1..=120).contains(&f));
    if !fps_ok {
        return Err(ctx.die(cat!("record: --fps is 1 to 120, got ", o.fps)));
    }
    // (as the number read: 010 is 10 for wf-recorder, ffmpeg and stop alike, where bash recorded
    // the word and stop's arithmetic read it as octal, finding 448)
    o.fps = shell::test_int(&o.fps)
        .unwrap_or(30)
        .to_string()
        .into_bytes();
    if o.forms.is_some() {
        if !o.ract.is_empty() {
            return Err(say_exit2(
                "record: --for DURATION or -- ACTION, not both (after an action: --hold DURATION)",
            ));
        }
        if !s.is_empty() {
            return Err(say_exit2(cat!(
                "record: --for records for that long (start, a hold, stop): not with ",
                sub,
                " (inside a take: record hold DURATION)"
            )));
        }
        if o.holdset {
            return Err(say_exit2(
                "record: --hold goes with -- ACTION or mark (--for is the whole length)",
            ));
        }
    } else if !o.ract.is_empty() {
        if !s.is_empty() {
            return Err(ctx.die(cat!(
                "record: -- ACTION records one action (start, recording, stop): not with ",
                sub
            )));
        }
        action_check(ctx, &o.ract)?;
    } else if s.is_empty() {
        return Err(ctx.die(
            "record: start, stop, status, mark, hold, sheet, -- ACTION or --for DURATION (omabox help record)",
        ));
    }
    let any = |l: &[&[u8]]| l.contains(&s);
    if any(&[b"start", b"stop", b"status", b"sheet", b"hold"]) && o.holdset {
        return Err(say_exit2(cat!(
            "record ",
            sub,
            ": --hold goes with -- ACTION or mark (between steps: record hold DURATION)"
        )));
    }
    if s == b"hold" {
        if o.hdur.is_none() {
            return Err(say_exit2(
                "record hold: how long, like: omabox record hold 2s",
            ));
        }
        if !o.out.is_empty() || !o.monitor.is_empty() {
            return Err(say_exit2("record hold: -o and --monitor go with start"));
        }
    }
    let later: &[&[u8]] = &[b"stop", b"status", b"mark", b"sheet", b"hold"];
    if any(later) && o.nopointer {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --no-pointer goes with start, -- ACTION or --for"
        )));
    }
    if any(later) && o.audio {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --audio goes with start, -- ACTION or --for"
        )));
    }
    if any(later) && o.crf.is_some() {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --crf goes with start, -- ACTION or --for"
        )));
    }
    if any(later) && o.glide.is_some() {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --glide goes with start, -- ACTION or --for"
        )));
    }
    if any(&[b"start", b"status", b"mark", b"sheet", b"hold"])
        && (o.show || o.caps || o.cut.is_some())
    {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --show-input, --captions and --cut-idle go with stop, -- ACTION or --for"
        )));
    }
    if any(&[b"start", b"status", b"mark", b"hold"]) && o.sheet {
        return Err(ctx.die(cat!(
            "record ",
            sub,
            ": --sheet, --frames, --every and --fit go with stop, sheet, -- ACTION or --for"
        )));
    }
    if !o.frames.is_empty() && o.every.is_some() {
        return Err(ctx.die("record: --frames or --every, not both"));
    }
    if o.more.changed && (!o.frames.is_empty() || o.every.is_some() || o.more.every_frame) {
        return Err(ctx.die(
            "record: --changed picks its own frames: not with --frames, --every or --every-frame",
        ));
    }
    if o.more.every_frame && (!o.frames.is_empty() || o.every.is_some()) {
        return Err(ctx.die("record: --every-frame, --frames or --every, one of them"));
    }
    if let (Some(f), Some(t)) = (o.more.from, o.more.to) {
        if f > t {
            return Err(ctx.die("record: --from is after --to"));
        }
    }
    match s {
        b"mark" => {
            if o.text.is_empty() {
                return Err(
                    ctx.die("record mark: the mark's text, like: omabox record mark \"scene 2\"")
                );
            }
            if !o.out.is_empty() || !o.monitor.is_empty() {
                return Err(ctx.die("record mark: -o and --monitor go with start"));
            }
        }
        b"sheet" => {
            if o.video.is_empty() && o.fromfd.is_empty() {
                return Err(ctx.die("record sheet: the video, like: omabox record sheet demo.mp4"));
            }
            if !o.video.is_empty() && !o.fromfd.is_empty() {
                return Err(ctx.die("record sheet: VIDEO or --from-fd, not both"));
            }
            // (A jailed agent's path is never looked at here, not even whether it is there.)
            if o.fromfd.is_empty() {
                if in_jail() {
                    ctx.parsed();
                    return Err(ctx.die(
                        "record sheet: a jailed agent's video and sheet are opened by its own omabox (--from-fd, --to-fd), never at a path here",
                    ));
                }
                if !shell::path(&o.video).is_file() {
                    return Err(ctx.die(cat!("record sheet: no video ", o.video)));
                }
            }
            if !o.monitor.is_empty() {
                return Err(ctx.die("record sheet: --monitor goes with start"));
            }
        }
        _ => {}
    }
    // The fds and names a jailed caller's omabox passes (#224), checked before anything is done.
    if o.sfile && s != b"status" {
        return Err(ctx.die("record: --file goes with status"));
    }
    if o.json && s != b"status" {
        return Err(ctx.die("record: --json goes with status (record status --json)"));
    }
    if !o.tofd.is_empty() && s != b"stop" && s != b"sheet" {
        return Err(ctx.die("record: --to-fd goes with stop or sheet"));
    }
    if (!o.mfd.is_empty() || !o.vfd.is_empty()) && s != b"stop" {
        return Err(ctx.die("record: --marks-fd and --vtt-fd go with stop"));
    }
    if (!o.fromfd.is_empty() || !o.mfromfd.is_empty()) && s != b"sheet" {
        return Err(ctx.die("record: --from-fd and --marks-from-fd go with sheet"));
    }
    if !o.mfromfd.is_empty() && o.fromfd.is_empty() {
        return Err(ctx.die("record sheet: --marks-from-fd goes with --from-fd"));
    }
    if !o.as_.is_empty() {
        if !matches!(s, b"start" | b"stop" | b"sheet") {
            return Err(ctx.die("record: --as goes with start, stop or sheet"));
        }
        let ok = o.as_.starts_with(b"/")
            && shell::cntrl_to_space(&o.as_) == o.as_
            && shell::char_count(&o.as_) <= 4096;
        if !ok {
            return Err(ctx.die("record: --as takes an absolute file name"));
        }
        if s != b"start" && o.tofd.is_empty() {
            return Err(ctx.die(cat!("record ", sub, ": --as goes with --to-fd")));
        }
    }
    if s == b"stop"
        && !(o.tofd.is_empty() && o.mfd.is_empty() && o.vfd.is_empty())
        && !(!o.tofd.is_empty()
            && !o.mfd.is_empty()
            && !o.vfd.is_empty()
            && !o.as_.is_empty()
            && !o.sheet)
    {
        return Err(ctx.die(
            "record stop: --to-fd, --marks-fd and --vtt-fd go together, with --as NAME, without --sheet (its sheet is made from the file: record sheet)",
        ));
    }
    if s == b"sheet"
        && !(o.tofd.is_empty() && o.fromfd.is_empty())
        && !(!o.tofd.is_empty() && !o.fromfd.is_empty() && !o.as_.is_empty() && o.out.is_empty())
    {
        return Err(
            ctx.die("record sheet: --from-fd and --to-fd go together, with --as NAME, without -o")
        );
    }
    let mut fdl: Vec<Vec<u8>> = Vec::new();
    for (f, wr) in [
        (&o.tofd, true),
        (&o.mfd, true),
        (&o.vfd, true),
        (&o.fromfd, false),
        (&o.mfromfd, false),
    ] {
        if !f.is_empty() {
            crate::shots::fd_file(ctx, f, wr, "record")?;
            fdl.push(f.clone());
        }
    }
    let mut uniq = fdl.clone();
    uniq.sort();
    uniq.dedup();
    if uniq.len() != fdl.len() {
        return Err(ctx.die("record: each fd once"));
    }
    // A jailed agent (#224): its omabox resolves -o, opens the files and runs an action itself.
    if in_jail() {
        let fail = |ctx: &mut Ctx, m: &str| -> Exit {
            ctx.parsed();
            ctx.die(m)
        };
        if !o.ract.is_empty() {
            return Err(fail(
                ctx,
                "record -- ACTION: a jailed agent's own omabox runs it (start, the action, stop), never the broker",
            ));
        }
        if o.forms.is_some() {
            return Err(fail(
                ctx,
                "record --for: a jailed agent's own omabox runs it (start, a hold, stop), never the broker",
            ));
        }
        if !o.out.is_empty() {
            return Err(fail(
                ctx,
                "record: a jailed agent's -o is resolved by its own omabox, never at a path here",
            ));
        }
        match s {
            b"start" if o.as_.is_empty() => {
                return Err(fail(
                    ctx,
                    "record start: a jailed agent's video is named by its own omabox (--as), never opened here",
                ));
            }
            b"stop" if o.tofd.is_empty() => {
                return Err(fail(
                    ctx,
                    "record stop: a jailed agent's video is written by its own omabox (--to-fd), never at a path here",
                ));
            }
            b"sheet" if o.fromfd.is_empty() => {
                return Err(fail(
                    ctx,
                    "record sheet: a jailed agent's video and sheet are opened by its own omabox (--from-fd, --to-fd), never at a path here",
                ));
            }
            _ => {}
        }
    }
    ctx.parsed();
    if o.sheet || s == b"sheet" {
        if !crate::exec::executable_on_path(b"magick") {
            return Err(ctx.die(
                "record: a contact sheet needs ImageMagick's magick (package imagemagick, part of Omarchy)",
            ));
        }
        if o.fit.is_empty() {
            o.fit = crate::shots::shot_fit_default().map_err(|()| ctx.refused())?;
        }
        if o.fit == b"0" {
            o.fit.clear();
        }
    }
    select_box(ctx, &o.name)?;
    // A sheet needs no box (#279): with none named and none up, its ffmpeg runs here, sandboxed.
    if s == b"sheet" && ctx.name_derived && !crate::boxes::box_alive(ctx)? {
        return crate::cmd::record_stop::sheet_file_host(ctx, &o_sheet(&o));
    }
    need_box(ctx)?;
    let call_us = num(&crate::call_us());
    let mut rec = Rec::default();
    match s {
        b"status" => {
            return if o.json {
                status_json(ctx, &mut rec)
            } else {
                status(ctx, &mut rec, o.sfile)
            };
        }
        b"mark" => {
            let t: Vec<&[u8]> = o.text.iter().map(Vec::as_slice).collect();
            record_mark(ctx, &mut rec, &t.join(&b' '), call_us)?;
            if o.holdset && !record_hold(ctx, &mut rec, o.hold, call_us, false)? {
                return Err(Exit(1));
            }
            return Ok(());
        }
        b"hold" => {
            return if record_hold(ctx, &mut rec, o.hdur.unwrap_or(0), call_us, false)? {
                Ok(())
            } else {
                Err(Exit(1))
            };
        }
        b"sheet" => return crate::cmd::record_stop::sheet_file(ctx, &o_sheet(&o)),
        b"stop" => {
            if !o.out.is_empty() || !o.monitor.is_empty() {
                return Err(ctx.die("record stop: -o and --monitor go with start"));
            }
            return crate::cmd::record_stop::stop(ctx, &mut rec, &o_stop(&o));
        }
        _ => {}
    }
    if !o.as_.is_empty() && !o.out.is_empty() {
        return Err(ctx.die("record start: -o or --as, not both"));
    }
    let file = if o.as_.is_empty() {
        o.out.clone()
    } else {
        o.as_.clone()
    };
    record_start(ctx, &mut rec, &file, &o, !o.as_.is_empty())?;
    // Once it records, the pointer stays hidden until `record stop` (finding 320).
    crate::ptr::hold_keep();
    if o.forms.is_none() && o.ract.is_empty() {
        return Ok(());
    }
    let st = o_stop_plain(&o);
    // --for and -- ACTION stop what they started: an INT, TERM or HUP meanwhile stops the recording
    // as their end would (the video so far kept, the pointer given back), then exits 128+N (bash's
    // left it recording, the pointer hidden: finding 455). Forgotten before their own stop.
    let stop_trap = {
        let (mut c, st) = (ctx.clone(), o_stop_plain(&o));
        crate::trap::on_exit_code(move || {
            let mut rec = Rec::default();
            let _ = crate::cmd::record_stop::stop(&mut c, &mut rec, &st);
        })
    };
    // --for (#259): the hold from its first frame, in the video's clock; stopped whatever it says.
    if let Some(forms) = o.forms {
        let held = record_hold(ctx, &mut rec, forms, call_us, true);
        stop_trap.forget();
        let held = held?;
        crate::cmd::record_stop::stop(ctx, &mut rec, &st)?;
        return if held { Ok(()) } else { Err(Exit(1)) };
    }
    let mut c = crate::sig::omabox_cmd();
    c.arg("-b").arg(OsStr::from_bytes(&ctx.name));
    c.args(o.ract.iter().map(|a| OsStr::from_bytes(a)));
    // (`>&2`: its output on this one's stderr)
    if let Ok(e) = dup_hi(2) {
        c.stdout(Stdio::from(e));
    }
    let rc = crate::sig::run_status(c).unwrap_or(127);
    let secs = format!("{}.{:03}", o.hold / 1000, o.hold % 1000);
    crate::trap::sleep(secs.as_bytes());
    stop_trap.forget();
    crate::cmd::record_stop::stop(ctx, &mut rec, &st)?;
    if rc != 0 {
        say(cat!(
            "record: the action ended ",
            rc.to_string(),
            " (the recording is kept)"
        ));
        return Err(Exit(u8::try_from(rc & 0xff).unwrap_or(1)));
    }
    Ok(())
}

/// What record_stop takes.
pub struct StopOpts {
    pub sheet: bool,
    pub frames: Vec<u8>,
    pub every: Option<i64>,
    pub fit: Vec<u8>,
    pub more: SheetMore,
    pub show: bool,
    pub caps: bool,
    pub cut: Option<i64>,
    pub tofd: Vec<u8>,
    pub mfd: Vec<u8>,
    pub vfd: Vec<u8>,
    pub as_: Vec<u8>,
}

fn o_stop(o: &Opts) -> StopOpts {
    StopOpts {
        sheet: o.sheet,
        frames: o.frames.clone(),
        every: o.every,
        fit: o.fit.clone(),
        more: o.more.clone(),
        show: o.show,
        caps: o.caps,
        cut: o.cut,
        tofd: o.tofd.clone(),
        mfd: o.mfd.clone(),
        vfd: o.vfd.clone(),
        as_: o.as_.clone(),
    }
}

/// record_stop's words after start (--for, -- ACTION): no fds, no name.
fn o_stop_plain(o: &Opts) -> StopOpts {
    StopOpts {
        tofd: Vec::new(),
        mfd: Vec::new(),
        vfd: Vec::new(),
        as_: Vec::new(),
        ..o_stop(o)
    }
}

/// A contact sheet's frames beyond --frames and --every (#260): a range of the video (ms), a region
/// of it (X Y W H, the video's pixels), every frame, or the frames where the region changes.
#[derive(Clone, Default)]
pub struct SheetMore {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub geo: Option<(i64, i64, i64, i64)>,
    pub every_frame: bool,
    pub changed: bool,
}

impl SheetMore {
    /// Any of them: the labels then say each frame's time to the ms and its number.
    pub fn any(&self) -> bool {
        self.from.is_some()
            || self.to.is_some()
            || self.geo.is_some()
            || self.every_frame
            || self.changed
    }
}

/// -g "X,Y WxH": a region of the video, in its pixels.
pub(crate) fn sheet_geo(g: &[u8]) -> Option<(i64, i64, i64, i64)> {
    let re = shell::Re::new("^([0-9]{1,5}),([0-9]{1,5}) ([1-9][0-9]{0,4})x([1-9][0-9]{0,4})$")?;
    let c = re.captures(g, false)?;
    let n = |i: usize| c.get(i).copied().flatten().map(|(a, b)| num(&g[a..b]));
    Some((n(1)?, n(2)?, n(3)?, n(4)?))
}

/// What record_sheet_file takes.
pub struct SheetOpts {
    pub video: Vec<u8>,
    pub out: Vec<u8>,
    pub frames: Vec<u8>,
    pub every: Option<i64>,
    pub fit: Vec<u8>,
    pub more: SheetMore,
    pub fromfd: Vec<u8>,
    pub mfromfd: Vec<u8>,
    pub tofd: Vec<u8>,
    pub as_: Vec<u8>,
}

fn o_sheet(o: &Opts) -> SheetOpts {
    SheetOpts {
        video: o.video.clone(),
        out: o.out.clone(),
        frames: o.frames.clone(),
        every: o.every,
        fit: o.fit.clone(),
        more: o.more.clone(),
        fromfd: o.fromfd.clone(),
        mfromfd: o.mfromfd.clone(),
        tofd: o.tofd.clone(),
        as_: o.as_.clone(),
    }
}

// --- status, mark, hold -------------------------------------------------------------------------------

fn status(ctx: &mut Ctx, rec: &mut Rec, file: bool) -> Res<()> {
    if file {
        let data = std::fs::read(ctx.at("record")).ok();
        let f = data
            .as_ref()
            .filter(|d| d.contains(&b'\n'))
            .map(|d| shell::read_fields(d, 5)[4].clone())
            .unwrap_or_default();
        if f.is_empty() {
            return Err(ctx.die(cat!("record: box '", ctx.name, "' is not recording")));
        }
        // A jailed caller's own recordings only (#224).
        if in_jail() && !ctx.at("record.as").is_file() {
            return Err(ctx.die(cat!(
                "record: box '",
                ctx.name,
                "''s recording was started outside the jail: it is stopped there"
            )));
        }
        return out(cat!(f, "\n"));
    }
    let snd: &[u8] = if ctx.at("record.audio").is_file() {
        b" and its sound"
    } else {
        b""
    };
    let whose: &[u8] = if ctx.at("record.as").is_file() {
        b" (a jailed agent's file, written by its own omabox)"
    } else {
        b""
    };
    if record_running(ctx, rec)? {
        if in_jail() && !ctx.at("record.as").is_file() {
            rec.file = b"a file outside the jail".to_vec();
        }
        if rec.n > 1 && ctx.at("record.layout").is_file() {
            out(cat!(
                "recording ",
                record_names(&rec.out),
                " of box '",
                ctx.name,
                "'",
                snd,
                " as one ",
                record_size(ctx),
                " video at ",
                rec.fps,
                " fps, to ",
                rec.file,
                whose,
                " (omabox record stop)\n"
            ))
        } else {
            out(cat!(
                "recording ",
                rec.out,
                " of box '",
                ctx.name,
                "'",
                snd,
                " at ",
                rec.fps,
                " fps, to ",
                rec.file,
                whose,
                " (omabox record stop)\n"
            ))
        }
    } else if ctx.at("record").is_file() {
        out(cat!(
            "box '",
            ctx.name,
            "': its recording stopped by itself (omabox record stop keeps what it has)\n"
        ))
    } else {
        out(cat!("box '", ctx.name, "': not recording\n"))
    }
}

fn status_json(ctx: &mut Ctx, rec: &mut Rec) -> Res<()> {
    let (mut r, mut stopped) = (false, false);
    if record_running(ctx, rec)? {
        r = true;
    } else if ctx.at("record").is_file() {
        stopped = true;
    }
    let mut file = Vec::new();
    if (r || stopped) && (!in_jail() || ctx.at("record.as").is_file()) {
        file = rec.file.clone();
    }
    let tf = |b: bool| if b { "true" } else { "false" };
    let o = Args::new(&["-nc"])
        .kv("--argjson", "r", tf(r))
        .kv("--argjson", "s", tf(stopped))
        .kv("--arg", "out", &rec.out)
        .kv("--arg", "fps", &rec.fps)
        .kv("--arg", "f", &file)
        .kv("--argjson", "a", tf(ctx.at("record.audio").is_file()))
        .kv("--arg", "t", &rec.start)
        .arg(P::STATUS_JQ)
        .run(None, jq::Err::Inherit);
    out(&o.out)?;
    crate::cmd::hyprctl::code(o.rc)
}

/// record_mark TEXT: a mark at this omabox's call (CALL_US), put in the video's clock at stop.
fn record_mark(ctx: &mut Ctx, rec: &mut Rec, text: &[u8], call_us: i64) -> Res<()> {
    let mut mtext = shell::cntrl_to_space(text);
    if !record_running(ctx, rec)? {
        if ctx.at("record").is_file() {
            return Err(ctx.die(stopped_text(ctx, "record mark")));
        }
        return Err(ctx.die(cat!(
            "record mark: box '",
            ctx.name,
            "' is not recording (omabox record start)"
        )));
    }
    if in_jail() && !ctx.at("record.as").is_file() {
        return Err(ctx.die(cat!(
            "record mark: box '",
            ctx.name,
            "''s recording was started outside the jail: its marks are made there"
        )));
    }
    // Bounded (#224): a mark 1000 characters at most, a recording's 1 MiB of them.
    if shell::char_count(&mtext) > 1000 {
        mtext = cat!(shell::head_chars(&mtext, 997), "...");
    }
    let mf = ctx.at("record.marks");
    let msz = std::fs::metadata(&mf).map(|m| m.len()).unwrap_or(0);
    if msz >= 1_048_576 {
        return Err(ctx.die(cat!(
            "record mark: box '",
            ctx.name,
            "''s recording has 1 MiB of marks already"
        )));
    }
    let wrote = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&mf)
        .and_then(|mut f| f.write_all(&cat!(call_us.to_string(), "\t", mtext, "\n")));
    if wrote.is_err() {
        return Err(ctx.die(cat!("record mark: cannot write ", shell::bytes(&mf))));
    }
    match record_first_us(ctx, rec) {
        Some(first) => {
            let ms = ((call_us - first + 500) / 1000).max(0);
            out(cat!(
                format!("{}.{:03}s ", ms / 1000, ms % 1000),
                mtext,
                "\n"
            ))
        }
        None => out(cat!("before the first frame: ", mtext, "\n")),
    }
}

/// record_hold DURATION_MS FROM_US [QUIET]: returns once the video has DURATION_MS from FROM_US on,
/// in its own clock (#280, finding 306): each wf-recorder's latest frame at or past that time.
fn record_hold(ctx: &mut Ctx, rec: &mut Rec, dur: i64, from: i64, quiet: bool) -> Res<bool> {
    if !record_running(ctx, rec)? {
        if ctx.at("record").is_file() {
            say(stopped_text(ctx, "record hold"));
            return Ok(false);
        }
        return Err(say_exit2(cat!(
            "record hold: box '",
            ctx.name,
            "' is not recording (omabox record start, then hold between the steps of the take)"
        )));
    }
    if in_jail() && !ctx.at("record.as").is_file() {
        return Err(say_exit2(cat!(
            "record hold: box '",
            ctx.name,
            "''s recording was started outside the jail: its holds are made there"
        )));
    }
    // Its first frame (start waits only 0.5 s for wf-recorder): 5 s at most.
    let end5 = crate::settle::after(5);
    let first = loop {
        if let Some(f) = record_first_us(ctx, rec) {
            break f;
        }
        if crate::settle::past(end5) {
            say(cat!(
                "record hold: no frame came from box '",
                ctx.name,
                "''s recording in 5 s"
            ));
            return Ok(false);
        }
        crate::trap::sleep(b"0.1");
    };
    let from = from.max(first);
    let end = from + dur * 1000;
    let firsts = record_firsts(ctx, rec);
    let dirs = rec_dirs(rec);
    let mut args: Vec<Vec<u8>> = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        let Some(Some((m, wl))) = firsts.get(i) else {
            continue;
        };
        args.push(d.clone());
        args.push((m + end - wl).to_string().into_bytes());
    }
    if args.is_empty() {
        say(cat!(
            "record hold: no frame came from box '",
            ctx.name,
            "''s recording"
        ));
        return Ok(false);
    }
    // The wall-clock part on the host, in steps of 30 s at most, each touching the box's use.
    loop {
        let mut left = end - 50_000 - now_us();
        if left <= 0 {
            break;
        }
        left = left.min(30_000_000);
        let s = format!("{}.{:06}", left / 1_000_000, left % 1_000_000);
        crate::trap::sleep(s.as_bytes());
        crate::boxes::touch(&ctx.at("used"));
        if !record_running(ctx, rec)? {
            say(stopped_text(ctx, "record hold"));
            return Ok(false);
        }
    }
    // (Under timeout: last-frame is the box's file, which its processes could make a FIFO.)
    let left = (end + 5_000_000 - now_us()) / 1_000_000 + 3;
    let mut a = w(&[
        "/usr/bin/timeout",
        "-k",
        "1",
        &left.to_string(),
        "/usr/bin/bash",
        "-c",
        P::HOLD_SH,
        "omabox-record",
        &(end + 5_000_000).to_string(),
    ]);
    a.extend(args);
    let rc = sys_q(ctx, &a)?;
    if rc != 0 {
        if !record_running(ctx, rec)? {
            say(stopped_text(ctx, "record hold"));
        } else {
            say(cat!(
                "record hold: box '",
                ctx.name,
                "''s recording gave no frame past the hold's end 5 s after it (the recording goes on)"
            ));
        }
        return Ok(false);
    }
    if quiet {
        return Ok(true);
    }
    let a0 = (from - first + 500) / 1000;
    let b0 = a0 + dur;
    out(format!(
        "held {}.{:03}s to {}.{:03}s\n",
        a0 / 1000,
        a0 % 1000,
        b0 / 1000,
        b0 % 1000
    ))?;
    Ok(true)
}

// --- start -----------------------------------------------------------------------------------------------

/// `$(jq -r PROG <<<INPUT)`.
pub(crate) fn jq_r(prog: &str, input: &[u8]) -> (i32, Vec<u8>) {
    let r = jq::jq(&["-r", prog], Some(input), jq::Err::Inherit);
    (r.rc, r.text())
}

/// record_start FILE O AS: the recording started (MONITOR all: the whole layout, #225; AUDIO: the
/// box's sound too, #226; GLIDE: click, pointer and drag glide while it records, #176). With AS
/// (#224), FILE is only a name, a jailed caller's: nothing is made or opened at it here.
fn record_start(ctx: &mut Ctx, rec: &mut Rec, file: &[u8], o: &Opts, asname: bool) -> Res<()> {
    let mut file = file.to_vec();
    let mut mon = o.monitor.clone();
    // By their paths: the box runs them from /usr/bin (box_sys, finding 322).
    if !crate::exec::executable(b"/usr/bin/wf-recorder")
        || !crate::exec::executable(b"/usr/bin/ffmpeg")
    {
        return Err(ctx.die(
            "record: needs wf-recorder and ffmpeg in /usr/bin (sudo pacman -S wf-recorder ffmpeg)",
        ));
    }
    if record_running(ctx, rec)? {
        if in_jail() && !ctx.at("record.as").is_file() {
            return Err(ctx.die(cat!(
                "record: box '",
                ctx.name,
                "' is recording already (started outside the jail)"
            )));
        }
        return Err(ctx.die(cat!(
            "record: box '",
            ctx.name,
            "' is recording already, to ",
            rec.file,
            " (omabox record stop)"
        )));
    }
    if o.audio && meta(&ctx.d, "audio").0 != b"true" {
        return Err(ctx.die(cat!(
            "record --audio: box '",
            ctx.name,
            "' has no audio: a box gets it at up (omabox up --audio; down first, or up --new --audio)"
        )));
    }
    for f in [
        "record",
        "record.marks",
        "record.layout",
        "record.audio",
        "record.input",
        "record.geo",
        "record.glide",
        "record.crf",
        "record.as",
    ] {
        rm(&ctx.at(f));
    }
    let mons = strip_nl(&crate::hypr::hypr_json(ctx, &["monitors"])?);
    let ms = crate::screen::main_screen(ctx);
    let mut layout = Vec::new();
    let mut names: Vec<Vec<u8>> = Vec::new();
    let jq_mon = crate::jqprog::JQ_MON;
    if mon == b"all" {
        let (rc, l) = jq_r(&format!("{jq_mon}{}", P::LAYOUT_JQ), &mons);
        if rc != 0 || l.is_empty() {
            return Err(ctx.die(cat!(
                "record: box '",
                ctx.name,
                "' has no monitor to record"
            )));
        }
        layout = l;
        names = layout
            .split(|&c| c == b'\n')
            .skip(1)
            .map(|l| l.split(|&c| c == b' ').next().unwrap_or_default().to_vec())
            .collect();
        // One monitor (the others away or never added): its own recording, as --monitor NAME.
        if names.len() <= 1 {
            mon = names.first().cloned().unwrap_or_default();
            layout.clear();
        }
    } else if mon.is_empty() {
        mon = jq_r(&format!("{ms} // empty | .name"), &mons).1;
        if mon.is_empty() {
            return Err(ctx.die(cat!(
                "record: box '",
                ctx.name,
                "''s main screen is away (omabox output back)"
            )));
        }
    } else {
        let has = Args::new(&["-e"])
            .kv("--arg", "n", &mon)
            .arg("any(.[]; .name == $n and (.disabled | not))")
            .run(Some(&mons), jq::Err::Inherit)
            .ok();
        if !has {
            let all = jq_r(r#"map(.name) | join(", ")"#, &mons).1;
            return Err(ctx.die(cat!(
                "record: box '",
                ctx.name,
                "' has no monitor ",
                mon,
                " (",
                all,
                ")"
            )));
        }
    }
    if layout.is_empty() {
        names = vec![mon.clone()];
    }
    // Where the layout's pixels are in the video (#176, for --show-input): "X0 Y0 K", then in a
    // layout's video "X Y W H", the main screen's place in it, where the captions go.
    let geo = if !layout.is_empty() {
        let mainn = jq_r(&format!("{ms} // empty | .name"), &mons).1;
        let mut g = jq_r(&format!("{jq_mon}{}", P::GEO_MIN_JQ), &mons).1;
        let head = layout.split(|&c| c == b'\n').next().unwrap_or_default();
        // (`geo+=" $(head ... | awk ...)"`, twice)
        let k = jq::awk(&[P::GEO_K_AWK], Some(head), jq::Err::Inherit).text();
        let rest: Vec<&[u8]> = layout.split(|&c| c == b'\n').skip(1).collect();
        let m = jq::awk(
            &[
                b"-v".to_vec(),
                cat!("m=", mainn),
                P::GEO_MAIN_AWK.as_bytes().to_vec(),
            ],
            Some(&rest.join(&b'\n')),
            jq::Err::Inherit,
        )
        .text();
        g.extend_from_slice(&cat!(" ", k, " ", m));
        g
    } else {
        // (`geo=$(jq ... | head -n 1)`)
        let r = Args::new(&["-r"])
            .kv("--arg", "n", &mon)
            .arg(P::GEO_ONE_JQ)
            .run(Some(&mons), jq::Err::Inherit);
        r.text()
            .split(|&c| c == b'\n')
            .next()
            .unwrap_or_default()
            .to_vec()
    };
    if !asname {
        if file.is_empty() {
            let tmp = shell::env("TMPDIR")
                .filter(|t| !t.is_empty())
                .unwrap_or(b"/tmp".to_vec());
            let t = shell::date("%H%M%S");
            file = cat!(tmp, "/omabox-", ctx.name, "-", t, ".mp4");
        }
        let dir = crate::shots::dirname(&file);
        if !crate::shots::mkdir_p(&dir) {
            return Err(ctx.die(cat!("record: cannot make the directory ", dir)));
        }
        file = crate::shots::real_or_end(ctx, &file)?;
        let part = cat!(file, ".part");
        if let Err(e) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(shell::path(&part))
        {
            return Err(ctx.die(cat!(
                "record: cannot write ",
                file,
                ": ",
                crate::shots::why(&e)
            )));
        }
        rm(&shell::path(&part));
    }
    // --no-pointer (#228): hidden before the first frame; `record stop` shows it again (and this
    // omabox, should it fail before the recording runs).
    if o.nopointer {
        crate::ptr::hold_until_exit(ctx, format!("record {}", std::process::id()).as_bytes())?;
    }
    // The sound first (#226), so it is there from the first frame on.
    let apid = if o.audio {
        // (`apid=$(record_audio_start) || exit 1`)
        match audio_start(ctx) {
            Ok(p) => p,
            Err(_) => return Err(Exit(1)),
        }
    } else {
        Vec::new()
    };
    let start = crate::marks::epoch_us();
    let mut pairs: Vec<Vec<u8>> = Vec::new();
    if !layout.is_empty() {
        for (i, n) in names.iter().enumerate() {
            pairs.push(cat!(REC_DIR, "/m", (i + 1).to_string()));
            pairs.push(n.clone());
        }
    } else {
        pairs = vec![REC_DIR.as_bytes().to_vec(), mon.clone()];
    }
    // One wf-recorder an output, each in a session of its own, its pid said on fd 3 (a
    // `<(...)`: SIGINT and SIGQUIT ignored in it, the script puts them back for wf-recorder).
    let pids = {
        let (r, w0) = pipe().map_err(|_| Exit(1))?;
        let wr: OwnedFd = dup_hi(w0.as_raw_fd()).map_err(|_| Exit(1))?;
        drop(w0);
        let reader = std::thread::spawn(move || read_all(r, Some(4096)));
        let mut a = w(&["/usr/bin/bash", "-c", P::START_SH, "omabox-record"]);
        a.push(raw_crf(o.crf).to_string().into_bytes());
        a.extend(pairs.iter().cloned());
        let _ = box_exec(
            ctx,
            &a,
            ExecOpts {
                sys: true,
                stdin_null: true,
                stdout_to_stderr: true,
                fd3: Some(wr),
                bg: true,
                ..ExecOpts::default()
            },
        );
        let o = reader.join().unwrap_or_default();
        let mut v: Vec<Vec<u8>> = o.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
        if v.last().is_some_and(Vec::is_empty) {
            v.pop();
        }
        v
    };
    if pids.len() != names.len() {
        audio_end(ctx, &apid)?;
        return Err(ctx.die(cat!(
            "record: could not start wf-recorder in box '",
            ctx.name,
            "'"
        )));
    }
    for p in &pids {
        if !shell::is_num(p) {
            audio_end(ctx, &apid)?;
            return Err(ctx.die(cat!(
                "record: could not start wf-recorder in box '",
                ctx.name,
                "' (no pid: ",
                p,
                ")"
            )));
        }
    }
    // record.as first (#224): a recording is never there without it.
    if asname && std::fs::write(ctx.at("record.as"), b"").is_err() {
        let mut a = w(&["/usr/bin/kill", "-KILL"]);
        a.extend(pids.iter().cloned());
        sys_q(ctx, &a)?;
        audio_end(ctx, &apid)?;
        return Err(ctx.die(cat!("record: cannot write in ", ctx.d)));
    }
    let put = |f: &str, v: Vec<u8>| -> Res<()> {
        // (its failure said, and the command ended, 1: finding 441)
        std::fs::write(ctx.at(f), v).map_err(|e| {
            ctx.die(cat!(
                "record: ",
                crate::shots::cant_write(shell::bytes(&ctx.at(f)), &e)
            ))
        })
    };
    put(
        "record",
        cat!(
            pids.join(&b','),
            " ",
            names.join(&b','),
            " ",
            o.fps,
            " ",
            start,
            " ",
            file,
            "\n"
        ),
    )?;
    if !apid.is_empty() {
        put("record.audio", cat!(apid, "\n"))?;
    }
    if !layout.is_empty() {
        put("record.layout", cat!(layout, "\n"))?;
    }
    put("record.geo", cat!(geo, "\n"))?;
    if let Some(g) = o.glide {
        put("record.glide", format!("{g}\n").into_bytes())?;
    }
    if let Some(c) = o.crf {
        put("record.crf", format!("{c}\n").into_bytes())?;
    }
    // Still running half a second on: not ended at once (a bad output or codec).
    if sh_quiet(ctx, "/usr/bin/sh", P::ALIVE_SH, &pids)? != 0 {
        for f in [
            "record",
            "record.layout",
            "record.audio",
            "record.geo",
            "record.glide",
            "record.crf",
            "record.as",
        ] {
            rm(&ctx.at(f));
        }
        audio_end(ctx, &apid)?;
        if pids.len() != 1 {
            let mut a = w(&["/usr/bin/kill", "-KILL"]);
            a.extend(pids.iter().cloned());
            sys_q(ctx, &a)?;
        }
        let dirs: Vec<Vec<u8>> = pairs.iter().step_by(2).cloned().collect();
        let mut a = w(&["/usr/bin/sh", "-c", P::TAILS_SH, "omabox-record"]);
        a.extend(dirs);
        let (_, t) = sys_cap(ctx, &a, Some(2000), Err2::Null, false);
        let t: Vec<u8> = t
            .iter()
            .map(|&c| if c == b'\n' { b' ' } else { c })
            .collect();
        return Err(ctx.die(cat!("record: wf-recorder ended at once: ", strip_nl(&t))));
    }
    let snd: &[u8] = if apid.is_empty() {
        b""
    } else {
        b" and its sound"
    };
    if !layout.is_empty() {
        say(cat!(
            "record: recording ",
            record_names(&names.join(&b' ')),
            " of box '",
            ctx.name,
            "'",
            snd,
            " as one ",
            record_size(ctx),
            " video (",
            record_how(ctx),
            ") to ",
            file,
            " (omabox record stop)"
        ));
    } else {
        say(cat!(
            "record: recording ",
            mon,
            " of box '",
            ctx.name,
            "'",
            snd,
            " to ",
            file,
            " (omabox record stop)"
        ));
    }
    Ok(())
}

/// record_audio_start: the box's sound (#226) by ffmpeg in the box, its pid once it has its first
/// samples (5 s at most); the refusal said (a `$(...)`'s die: the caller exits 1).
fn audio_start(ctx: &mut Ctx) -> Res<Vec<u8>> {
    let apid = {
        let Ok((r, w0)) = pipe() else {
            return Err(Exit(1));
        };
        let wr: OwnedFd = dup_hi(w0.as_raw_fd()).map_err(|_| Exit(1))?;
        drop(w0);
        let reader = std::thread::spawn(move || read_all(r, Some(64)));
        let a = w(&[
            "/usr/bin/bash",
            "-c",
            P::AUDIO_SH,
            "omabox-record",
            REC_DIR,
            AUDIO_SINK,
        ]);
        let _ = box_exec(
            ctx,
            &a,
            ExecOpts {
                sys: true,
                stdin_null: true,
                stdout_to_stderr: true,
                fd3: Some(wr),
                ..ExecOpts::default()
            },
        );
        strip_nl(&reader.join().unwrap_or_default())
    };
    if !shell::is_num(&apid) {
        return Err(ctx.die(cat!(
            "record --audio: could not start ffmpeg in box '",
            ctx.name,
            "'"
        )));
    }
    let t = sh_quiet(
        ctx,
        "/usr/bin/sh",
        P::AUDIO_WAIT_SH,
        &[cat!(REC_DIR, "/audio.log"), apid.clone()],
    )
    .unwrap_or_else(|Exit(c)| i32::from(c));
    if t != 0 {
        audio_end(ctx, &apid)?;
        let (_, l) = sys_cap(
            ctx,
            &w(&[
                "/usr/bin/tail".as_bytes(),
                b"-n",
                b"3",
                &cat!(REC_DIR, "/audio.log"),
            ]),
            Some(2000),
            Err2::Null,
            false,
        );
        let l: Vec<u8> = l
            .iter()
            .map(|&c| if c == b'\n' { b' ' } else { c })
            .collect();
        return Err(ctx.die(cat!(
            "record --audio: ffmpeg could not read box '",
            ctx.name,
            "''s sound: ",
            strip_nl(&l)
        )));
    }
    Ok(apid)
}

/// record_audio_end PID: the recording's ffmpeg told to finish (INT), then waited for (5 s, then
/// KILL), in the box. Nothing for no pid.
pub fn audio_end(ctx: &mut Ctx, pid: &[u8]) -> Res<()> {
    if !shell::is_num(pid) {
        return Ok(());
    }
    sh_quiet(ctx, "/usr/bin/sh", P::AUDIO_END_SH, &[pid.to_vec()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(mons: &str) -> String {
        let (rc, l) = jq_r(
            &format!("{}{}", crate::jqprog::JQ_MON, P::LAYOUT_JQ),
            mons.as_bytes(),
        );
        assert_eq!(rc, 0);
        String::from_utf8(l).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn mon(n: &str, w: i64, h: i64, x: i64, y: i64, s: &str, t: i64, off: bool) -> String {
        format!(
            r#"{{"name":"{n}","width":{w},"height":{h},"x":{x},"y":{y},"scale":{s},"transform":{t},"disabled":{off}}}"#
        )
    }

    /// t_unit_record's record_layout (#225) and JQ_MON (finding 311) cases: the layout at its highest
    /// scale, a turned monitor turned, one off or 0x0 left out, past H.264's largest frame scaled
    /// down (16384 a side, 35651584 px). jq is the recipe's dependency.
    #[test]
    fn record_layouts() {
        let m = [
            mon("A", 1280, 720, 0, 0, "1", 0, false),
            mon("B", 1280, 720, 1280, 0, "1", 1, false),
            mon("C", 1280, 720, 0, 720, "2", 0, false),
            mon("D", 800, 600, 5000, 0, "1", 0, true),
            mon("Z", 0, 0, 0, 5000, "1", 0, false),
        ];
        assert_eq!(
            layout(&format!("[{}]", m.join(","))).replace('\n', "|"),
            "4000 2560 2 1|A 0 0 2560 1440|B 2560 0 1440 2560|C 0 1440 1280 720"
        );
        let two = [
            mon("A", 1920, 1080, 0, 0, "1", 0, false),
            mon("B", 2560, 1440, 1920, 0, "1.6", 0, false),
        ];
        assert_eq!(
            layout(&format!("[{}]", two.join(","))).replace('\n', "|"),
            "5632 1728 1.6 1|A 0 0 3072 1728|B 3072 0 2560 1440"
        );
        let five: Vec<String> = (0..5)
            .map(|i| mon(&format!("M{i}"), 3840, 2160, i * 3840, 0, "1", 0, false))
            .collect();
        let l = layout(&format!("[{}]", five.join(",")));
        assert_eq!(l.lines().next(), Some("16384 1843 1 0.853"));
        let nine: Vec<String> = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| {
                mon(
                    &format!("M{i}{j}"),
                    3840,
                    2160,
                    i * 3840,
                    j * 2160,
                    "1",
                    0,
                    false,
                )
            })
            .collect();
        let l = layout(&format!("[{}]", nine.join(",")));
        let f: Vec<f64> = l
            .lines()
            .next()
            .unwrap()
            .split(' ')
            .map(|x| x.parse().unwrap())
            .collect();
        assert!(f[0] * f[1] <= 35_651_584.0 && f[0] * f[1] > 35_000_000.0 && f[3] < 1.0);
        assert_eq!(layout("[]"), "");
        // JQ_MON: transforms 1, 3, 5, 7 swap a monitor's sides, the others do not.
        let all: Vec<String> = (0..8)
            .map(|t| format!(r#"{{"width":2560,"height":1440,"scale":2,"transform":{t}}}"#))
            .collect();
        let (rc, sides) = jq_r(
            &format!(
                "{}{}",
                crate::jqprog::JQ_MON,
                r#"map("\(lw)x\(lh)") | join(" ")"#
            ),
            format!("[{}]", all.join(",")).as_bytes(),
        );
        assert_eq!(rc, 0);
        assert_eq!(
            String::from_utf8(sides).unwrap(),
            "1280x720 720x1280 1280x720 720x1280 1280x720 720x1280 1280x720 720x1280"
        );
    }

    #[test]
    fn names_listed() {
        assert_eq!(record_names(b"A,B,C"), b"A, B, C");
        assert_eq!(record_names(b"A B"), b"A, B");
        assert_eq!(record_names(b"A"), b"A");
    }
}
